use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mp3lame_encoder::{Bitrate, Builder, Encoder, FlushNoGap, InterleavedPcm, Mode, Quality};

use crate::timeline::MIX_SAMPLE_RATE;

const CHANNELS: u8 = 2;
const STEREO_FRAME_BYTES: u64 = 8;
const CHUNK_FRAMES: usize = MIX_SAMPLE_RATE as usize;
const FLUSH_CAPACITY: usize = 7200;

static NEXT_PART: AtomicU64 = AtomicU64::new(0);

/// Closed CBR presets. LAME `Bitrate` is mapped only inside this module.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExportQuality {
    Compact,
    #[default]
    Standard,
    High,
}

impl ExportQuality {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Standard, Self::High];

    pub const fn index(self) -> usize {
        match self {
            Self::Compact => 0,
            Self::Standard => 1,
            Self::High => 2,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact (128 kbps)",
            Self::Standard => "Standard (192 kbps)",
            Self::High => "High (320 kbps)",
        }
    }

    fn bitrate(self) -> Bitrate {
        match self {
            Self::Compact => Bitrate::Kbps128,
            Self::Standard => Bitrate::Kbps192,
            Self::High => Bitrate::Kbps320,
        }
    }
}

/// PCM stereo frames consumed versus `staging_bytes / 8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveProgress {
    pub done: u64,
    pub total: u64,
}

/// Pollable CBR encode. Drop of an uncommitted part unlinks it.
pub(crate) struct Encode {
    encoder: Encoder,
    src: File,
    part: Option<PartFile>,
    destination: PathBuf,
    total: u64,
    done: u64,
    raw: Vec<u8>,
    pcm: Vec<f32>,
    encoded: Vec<u8>,
}

impl Encode {
    pub(crate) fn start(
        destination: &Path,
        staged: &Path,
        quality: ExportQuality,
    ) -> io::Result<Self> {
        let bytes = fs::metadata(staged)?.len();
        if bytes % STEREO_FRAME_BYTES != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staging file is not a whole number of stereo frames",
            ));
        }
        let encoder = lame(quality)?;
        let src = File::open(staged)?;
        let part = PartFile::create(destination)?;
        Ok(Self {
            encoder,
            src,
            part: Some(part),
            destination: destination.to_path_buf(),
            total: bytes / STEREO_FRAME_BYTES,
            done: 0,
            raw: vec![0u8; CHUNK_FRAMES * STEREO_FRAME_BYTES as usize],
            pcm: Vec::with_capacity(CHUNK_FRAMES * usize::from(CHANNELS)),
            encoded: Vec::new(),
        })
    }

    /// `budget == 0` still encodes one PCM chunk or the final flush.
    pub(crate) fn pump(&mut self, budget: Duration) -> io::Result<bool> {
        if self.part.is_none() {
            return Ok(true);
        }
        let deadline = Instant::now() + budget;
        loop {
            if self.advance()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
        }
    }

    pub(crate) fn progress(&self) -> SaveProgress {
        SaveProgress {
            done: self.done.min(self.total),
            total: self.total,
        }
    }

    fn advance(&mut self) -> io::Result<bool> {
        let n = fill(&mut self.src, &mut self.raw)?;
        if n == 0 {
            return self.flush_and_commit();
        }
        if n as u64 % STEREO_FRAME_BYTES != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staging file is not a whole number of stereo frames",
            ));
        }
        self.pcm.clear();
        self.pcm.extend(
            self.raw[..n]
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap())),
        );
        self.encoded.clear();
        self.encoded
            .reserve(mp3lame_encoder::max_required_buffer_size(
                self.pcm.len() / 2,
            ));
        self.encoder
            .encode_to_vec(InterleavedPcm(self.pcm.as_slice()), &mut self.encoded)
            .map_err(encode_fail)?;
        self.part
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "part file is closed"))?
            .write_all(&self.encoded)?;
        self.done += n as u64 / STEREO_FRAME_BYTES;
        Ok(false)
    }

    fn flush_and_commit(&mut self) -> io::Result<bool> {
        self.encoded.clear();
        self.encoded.reserve(FLUSH_CAPACITY);
        self.encoder
            .flush_to_vec::<FlushNoGap>(&mut self.encoded)
            .map_err(encode_fail)?;
        let mut part = self
            .part
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "part file is closed"))?;
        part.write_all(&self.encoded)?;
        part.commit(&self.destination)?;
        Ok(true)
    }
}

#[cfg(test)]
pub(crate) fn write(destination: &Path, staged: &Path, quality: ExportQuality) -> io::Result<()> {
    let mut encode = Encode::start(destination, staged, quality)?;
    while !encode.pump(Duration::from_secs(60))? {}
    Ok(())
}

fn fill(src: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = src.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

fn lame(quality: ExportQuality) -> io::Result<Encoder> {
    let mut builder = Builder::new().ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "could not allocate the MP3 encoder")
    })?;
    builder.set_num_channels(CHANNELS).map_err(encode_fail)?;
    builder
        .set_sample_rate(MIX_SAMPLE_RATE)
        .map_err(encode_fail)?;
    builder
        .set_output_sample_rate(NonZeroU32::new(MIX_SAMPLE_RATE))
        .map_err(encode_fail)?;
    builder.set_brate(quality.bitrate()).map_err(encode_fail)?;
    builder.set_quality(Quality::Best).map_err(encode_fail)?;
    builder.set_mode(Mode::JointStereo).map_err(encode_fail)?;
    builder.set_to_write_vbr_tag(true).map_err(encode_fail)?;
    builder.build().map_err(encode_fail)
}

fn encode_fail(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, error.to_string())
}

struct PartFile {
    path: PathBuf,
    file: Option<File>,
    committed: bool,
}

impl PartFile {
    fn create(destination: &Path) -> io::Result<Self> {
        loop {
            let n = NEXT_PART.fetch_add(1, Ordering::Relaxed);
            let path = sibling_part(destination, n);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        committed: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "part file is closed"))?
            .write_all(bytes)
    }

    fn commit(mut self, destination: &Path) -> io::Result<()> {
        let file = self
            .file
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "part file is closed"))?;
        file.sync_all()?;
        drop(file);
        fs::rename(&self.path, destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn sibling_part(destination: &Path, n: u64) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("onerec"));
    name.push(format!(".onerec-part-{}-{n}", std::process::id()));
    match destination.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(name),
        _ => PathBuf::from(name),
    }
}

#[cfg(test)]
pub(crate) struct MpegLayer3Frame {
    pub byte_len: usize,
    pub sample_rate: u32,
    pub bitrate_kbps: u16,
    pub channels: u8,
}

#[cfg(test)]
pub(crate) fn parse_mpeg1_layer3_cbr(bytes: &[u8]) -> Result<Vec<MpegLayer3Frame>, String> {
    const BITRATE_KBPS: [u16; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const SAMPLE_RATE: [u32; 4] = [44_100, 48_000, 32_000, 0];
    if bytes.is_empty() {
        return Err("MP3 is empty".into());
    }
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if offset + 4 > bytes.len() {
            return Err(format!("truncated MPEG header at byte {offset}"));
        }
        let h0 = bytes[offset];
        let h1 = bytes[offset + 1];
        let h2 = bytes[offset + 2];
        let h3 = bytes[offset + 3];
        if h0 != 0xFF || h1 & 0xE0 != 0xE0 {
            return Err(format!("missing MPEG sync at byte {offset}"));
        }
        if h1 & 0x18 != 0x18 {
            return Err(format!("not MPEG-1 at byte {offset}"));
        }
        if h1 & 0x06 != 0x02 {
            return Err(format!("not Layer III at byte {offset}"));
        }
        let bitrate_index = (h2 >> 4) as usize;
        let sample_rate_index = ((h2 >> 2) & 0b11) as usize;
        let padding = (h2 >> 1) & 1;
        let channel_mode = h3 >> 6;
        let bitrate_kbps = BITRATE_KBPS[bitrate_index];
        let sample_rate = SAMPLE_RATE[sample_rate_index];
        if bitrate_kbps == 0 {
            return Err(format!("bitrate index {bitrate_index} at byte {offset}"));
        }
        if sample_rate == 0 {
            return Err(format!(
                "sample rate index {sample_rate_index} at byte {offset}"
            ));
        }
        if padding != 0 {
            return Err(format!("padded CBR frame at byte {offset}"));
        }
        if channel_mode > 1 {
            return Err(format!("channel mode {channel_mode} at byte {offset}"));
        }
        // MPEG-1 Layer III CBR length is 144 * bitrate / sample_rate. Compact is 384 B, Standard 576 B, High 960 B.
        let byte_len = (144 * u32::from(bitrate_kbps) * 1000 / sample_rate) as usize;
        if offset + byte_len > bytes.len() {
            return Err(format!("truncated MPEG frame at byte {offset}"));
        }
        frames.push(MpegLayer3Frame {
            byte_len,
            sample_rate,
            bitrate_kbps,
            channels: 2,
        });
        offset += byte_len;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn stage_frames(path: &Path, frames: usize, sample: impl Fn(usize) -> [f32; 2]) {
        let mut bytes = Vec::with_capacity(frames * STEREO_FRAME_BYTES as usize);
        for i in 0..frames {
            let [left, right] = sample(i);
            bytes.extend_from_slice(&left.to_le_bytes());
            bytes.extend_from_slice(&right.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
    }

    fn sine_frame(i: usize) -> [f32; 2] {
        let t = i as f32 / MIX_SAMPLE_RATE as f32;
        [
            (t * 440.0 * 2.0 * PI).sin() * 0.5,
            (t * 660.0 * 2.0 * PI).sin() * 0.25,
        ]
    }

    fn encode_frames(dir: &Path, name: &str, frames: usize, quality: ExportQuality) -> Vec<u8> {
        let staged = dir.join(format!("{name}.f32"));
        let dest = dir.join(name);
        stage_frames(&staged, frames, sine_frame);
        write(&dest, &staged, quality).unwrap();
        fs::read(&dest).unwrap()
    }

    fn assert_cbr_header(frame: &MpegLayer3Frame, bitrate_kbps: u16, byte_len: usize) {
        assert_eq!(frame.byte_len, byte_len);
        assert_eq!(frame.sample_rate, 48_000);
        assert_eq!(frame.bitrate_kbps, bitrate_kbps);
        assert_eq!(frame.channels, 2);
    }

    fn leftovers(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("onerec-part"))
            .count()
    }

    #[test]
    fn writes_one_second_of_mpeg_layer_iii() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = encode_frames(
            dir.path(),
            "one-second",
            CHUNK_FRAMES,
            ExportQuality::Standard,
        );
        assert_eq!(mp3.len(), 24_192);
        let frames = parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert_eq!(frames.len(), 42);
        for frame in &frames {
            assert_cbr_header(frame, 192, 576);
        }
    }

    #[test]
    fn compact_headers_are_128_kbps() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = encode_frames(dir.path(), "compact", CHUNK_FRAMES, ExportQuality::Compact);
        let frames = parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert!(!frames.is_empty());
        for frame in &frames {
            assert_cbr_header(frame, 128, 384);
        }
    }

    #[test]
    fn high_headers_are_320_kbps() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = encode_frames(dir.path(), "high", CHUNK_FRAMES, ExportQuality::High);
        let frames = parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert!(!frames.is_empty());
        for frame in &frames {
            assert_cbr_header(frame, 320, 960);
        }
    }

    #[test]
    fn longer_pcm_yields_more_mpeg_frames() {
        let dir = tempfile::tempdir().unwrap();
        let short = encode_frames(
            dir.path(),
            "one-second",
            CHUNK_FRAMES,
            ExportQuality::Standard,
        );
        let long = encode_frames(
            dir.path(),
            "two-seconds",
            CHUNK_FRAMES * 2,
            ExportQuality::Standard,
        );
        let short_frames = parse_mpeg1_layer3_cbr(&short).unwrap();
        let long_frames = parse_mpeg1_layer3_cbr(&long).unwrap();
        assert!(
            long_frames.len() > short_frames.len(),
            "2 s produced {} frames, 1 s produced {}",
            long_frames.len(),
            short_frames.len()
        );
        for frame in short_frames.iter().chain(&long_frames) {
            assert_cbr_header(frame, 192, 576);
        }
    }

    #[test]
    fn destination_need_not_end_in_mp3() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take");
        stage_frames(&staged, CHUNK_FRAMES, sine_frame);
        write(&dest, &staged, ExportQuality::Standard).unwrap();
        let mp3 = fs::read(&dest).unwrap();
        assert_eq!(mp3.len(), 24_192);
        assert_eq!(parse_mpeg1_layer3_cbr(&mp3).unwrap().len(), 42);
    }

    #[test]
    fn write_replaces_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take.mp3");
        stage_frames(&staged, CHUNK_FRAMES, sine_frame);
        fs::write(&dest, b"old").unwrap();
        write(&dest, &staged, ExportQuality::Standard).unwrap();
        let mp3 = fs::read(&dest).unwrap();
        assert_eq!(mp3.len(), 24_192);
        assert_eq!(parse_mpeg1_layer3_cbr(&mp3).unwrap().len(), 42);
    }

    #[test]
    fn unaligned_staging_leaves_destination_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take.mp3");
        fs::write(&staged, [0u8; 7]).unwrap();
        fs::write(&dest, b"keep").unwrap();
        write(&dest, &staged, ExportQuality::Standard).unwrap_err();
        assert_eq!(fs::read(&dest).unwrap(), b"keep");
        assert_eq!(leftovers(dir.path()), 0);
    }

    #[test]
    fn unwritable_destination_unlinks_the_part() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        stage_frames(&staged, CHUNK_FRAMES, |_| [0.0, 0.0]);
        write(dir.path(), &staged, ExportQuality::Standard).unwrap_err();
        assert_eq!(leftovers(dir.path()), 0);
        assert!(dir.path().is_dir());
    }

    #[test]
    fn pump_reports_partial_pcm_frames_before_commit() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("ten.f32");
        let dest = dir.path().join("ten.mp3");
        stage_frames(&staged, CHUNK_FRAMES * 10, sine_frame);
        let mut encode = Encode::start(&dest, &staged, ExportQuality::Standard).unwrap();
        assert_eq!(
            encode.progress(),
            SaveProgress {
                done: 0,
                total: CHUNK_FRAMES as u64 * 10
            }
        );
        assert!(!encode.pump(Duration::ZERO).unwrap());
        let mid = encode.progress();
        assert!(mid.done > 0, "done stayed 0");
        assert!(
            mid.done < mid.total,
            "done {} reached total {}",
            mid.done,
            mid.total
        );
        while !encode.pump(Duration::from_secs(60)).unwrap() {}
        assert!(dest.exists());
        assert_eq!(leftovers(dir.path()), 0);
    }
}
