use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mp3lame_encoder::{Bitrate, Builder, Encoder, FlushNoGap, InterleavedPcm, Mode, Quality};

use crate::timeline::{MIX_QUANTUM_FRAMES, MIX_SAMPLE_RATE};

const SAMPLE_BYTES: usize = 4;
const FLUSH_CAPACITY: usize = 7200;

static NEXT_PART: AtomicU64 = AtomicU64::new(0);

struct Profile {
    bitrate: Bitrate,
    mode: Mode,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExportQuality {
    #[default]
    Meeting,
    Voice,
    Compact,
    Standard,
    High,
}

impl ExportQuality {
    pub const ALL: [Self; 5] = [
        Self::Meeting,
        Self::Voice,
        Self::Compact,
        Self::Standard,
        Self::High,
    ];

    pub const fn index(self) -> usize {
        match self {
            Self::Meeting => 0,
            Self::Voice => 1,
            Self::Compact => 2,
            Self::Standard => 3,
            Self::High => 4,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub fn from_short_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|quality| quality.short_name() == name)
    }

    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Meeting => "Meeting",
            Self::Voice => "Voice",
            Self::Compact => "Compact",
            Self::Standard => "Standard",
            Self::High => "High",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Meeting => "Meeting (8 kbps, ~4 MB/h)",
            Self::Voice => "Voice (24 kbps, ~11 MB/h)",
            Self::Compact => "Compact (128 kbps, ~56 MB/h)",
            Self::Standard => "Standard (192 kbps, ~84 MB/h)",
            Self::High => "High (320 kbps, ~141 MB/h)",
        }
    }

    fn profile(self) -> Profile {
        match self {
            Self::Meeting => Profile {
                bitrate: Bitrate::Kbps8,
                mode: Mode::Mono,
            },
            Self::Voice => Profile {
                bitrate: Bitrate::Kbps24,
                mode: Mode::Mono,
            },
            Self::Compact => Profile {
                bitrate: Bitrate::Kbps128,
                mode: Mode::JointStereo,
            },
            Self::Standard => Profile {
                bitrate: Bitrate::Kbps192,
                mode: Mode::JointStereo,
            },
            Self::High => Profile {
                bitrate: Bitrate::Kbps320,
                mode: Mode::JointStereo,
            },
        }
    }

    /// Sample rate of the PCM take. Matches the MP3, so Meeting cannot be
    /// re-exported as High later.
    pub const fn staging_hz(self) -> u32 {
        match self {
            Self::Meeting => 8_000,
            Self::Voice => 16_000,
            Self::Compact | Self::Standard | Self::High => 48_000,
        }
    }

    pub const fn staging_channels(self) -> u8 {
        match self {
            Self::Meeting | Self::Voice => 1,
            Self::Compact | Self::Standard | Self::High => 2,
        }
    }

    pub const fn staging_frame_bytes(self) -> usize {
        SAMPLE_BYTES * self.staging_channels() as usize
    }

    pub const fn staging_factor(self) -> usize {
        MIX_SAMPLE_RATE as usize / self.staging_hz() as usize
    }

    const fn chunk_frames(self) -> usize {
        self.staging_hz() as usize / 10
    }
}

const _: () = {
    let mut i = 0;
    while i < ExportQuality::ALL.len() {
        let quality = ExportQuality::ALL[i];
        assert!(MIX_SAMPLE_RATE % quality.staging_hz() == 0);
        assert!(MIX_QUANTUM_FRAMES % quality.staging_factor() == 0);
        assert!(quality.staging_hz() % 10 == 0);
        i += 1;
    }
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveProgress {
    pub done: u64,
    pub total: u64,
}

impl SaveProgress {
    pub fn fraction(self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32
    }

    pub fn percent(self) -> u8 {
        (self.fraction() * 100.0).round().clamp(0.0, 100.0) as u8
    }
}

pub(crate) struct Encode {
    encoder: Encoder,
    src: File,
    part: Option<PartFile>,
    destination: PathBuf,
    total: u64,
    done: u64,
    frame_bytes: u64,
    channels: u8,
    raw: Vec<u8>,
    pcm: Vec<f32>,
    encoded: Vec<u8>,
    armed_flush: bool,
}

impl Encode {
    pub(crate) fn start(
        destination: &Path,
        staged: &Path,
        quality: ExportQuality,
    ) -> io::Result<Self> {
        let frame_bytes = quality.staging_frame_bytes() as u64;
        let bytes = fs::metadata(staged)?.len();
        if bytes % frame_bytes != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staging file is not a whole number of PCM frames",
            ));
        }
        let encoder = lame(quality)?;
        let src = File::open(staged)?;
        let part = PartFile::create(destination)?;
        let chunk_frames = quality.chunk_frames();
        let channels = quality.staging_channels();
        Ok(Self {
            encoder,
            src,
            part: Some(part),
            destination: destination.to_path_buf(),
            total: bytes / frame_bytes,
            done: 0,
            frame_bytes,
            channels,
            raw: vec![0u8; chunk_frames * frame_bytes as usize],
            pcm: Vec::with_capacity(chunk_frames * usize::from(channels)),
            encoded: Vec::new(),
            armed_flush: false,
        })
    }

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
        if self.armed_flush {
            return self.flush_and_commit();
        }
        let n = fill(&mut self.src, &mut self.raw)?;
        if n == 0 {
            self.done = self.total;
            self.armed_flush = true;
            return Ok(false);
        }
        if n as u64 % self.frame_bytes != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staging file is not a whole number of PCM frames",
            ));
        }
        self.pcm.clear();
        self.pcm.extend(
            self.raw[..n]
                .chunks_exact(SAMPLE_BYTES)
                .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap())),
        );
        self.encoded.clear();
        self.encoded
            .reserve(mp3lame_encoder::max_required_buffer_size(
                self.pcm.len() / usize::from(self.channels),
            ));
        self.encoder
            .encode_to_vec(InterleavedPcm(self.pcm.as_slice()), &mut self.encoded)
            .map_err(encode_fail)?;
        self.part
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "part file is closed"))?
            .write_all(&self.encoded)?;
        self.done += n as u64 / self.frame_bytes;
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
    let profile = quality.profile();
    let hz = quality.staging_hz();
    let mut builder = Builder::new().ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "could not allocate the MP3 encoder")
    })?;
    builder
        .set_num_channels(quality.staging_channels())
        .map_err(encode_fail)?;
    builder.set_sample_rate(hz).map_err(encode_fail)?;
    builder
        .set_output_sample_rate(NonZeroU32::new(hz))
        .map_err(encode_fail)?;
    builder.set_brate(profile.bitrate).map_err(encode_fail)?;
    builder.set_quality(Quality::Best).map_err(encode_fail)?;
    builder.set_mode(profile.mode).map_err(encode_fail)?;
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
    let frames = parse_layer3_cbr(bytes)?;
    for frame in &frames {
        if frame.sample_rate != 48_000 || frame.channels != 2 {
            return Err(format!(
                "expected MPEG-1 48 kHz stereo, got {} Hz {} ch",
                frame.sample_rate, frame.channels
            ));
        }
    }
    Ok(frames)
}

#[cfg(test)]
pub(crate) fn parse_layer3_cbr(bytes: &[u8]) -> Result<Vec<MpegLayer3Frame>, String> {
    const MPEG1_BR: [u16; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const MPEG2_BR: [u16; 16] = [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ];
    const MPEG1_SR: [u32; 4] = [44_100, 48_000, 32_000, 0];
    const MPEG2_SR: [u32; 4] = [22_050, 24_000, 16_000, 0];
    const MPEG25_SR: [u32; 4] = [11_025, 12_000, 8_000, 0];
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
        if h1 & 0x06 != 0x02 {
            return Err(format!("not Layer III at byte {offset}"));
        }
        let version = (h1 >> 3) & 0b11;
        let bitrate_index = (h2 >> 4) as usize;
        let sample_rate_index = ((h2 >> 2) & 0b11) as usize;
        let padding = (h2 >> 1) & 1;
        let channel_mode = h3 >> 6;
        let (bitrate_kbps, sample_rate, slot) = match version {
            0b11 => (MPEG1_BR[bitrate_index], MPEG1_SR[sample_rate_index], 144u32),
            0b10 => (MPEG2_BR[bitrate_index], MPEG2_SR[sample_rate_index], 72),
            0b00 => (MPEG2_BR[bitrate_index], MPEG25_SR[sample_rate_index], 72),
            _ => return Err(format!("reserved MPEG version at byte {offset}")),
        };
        if bitrate_kbps == 0 {
            return Err(format!("bitrate index {bitrate_index} at byte {offset}"));
        }
        if sample_rate == 0 {
            return Err(format!(
                "sample rate index {sample_rate_index} at byte {offset}"
            ));
        }
        if channel_mode == 2 {
            return Err(format!("dual-channel mode at byte {offset}"));
        }
        let mut byte_len = (slot * u32::from(bitrate_kbps) * 1000 / sample_rate) as usize;
        if padding != 0 {
            byte_len += 1;
        }
        if offset + byte_len > bytes.len() {
            return Err(format!("truncated MPEG frame at byte {offset}"));
        }
        frames.push(MpegLayer3Frame {
            byte_len,
            sample_rate,
            bitrate_kbps,
            channels: if channel_mode == 3 { 1 } else { 2 },
        });
        offset += byte_len;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SECOND_FRAMES: usize = MIX_SAMPLE_RATE as usize;

    fn stage_frames(
        path: &Path,
        quality: ExportQuality,
        frames: usize,
        sample: impl Fn(usize) -> [f32; 2],
    ) {
        let mut bytes = Vec::with_capacity(frames * quality.staging_frame_bytes());
        for i in 0..frames {
            let [left, right] = sample(i);
            if quality.staging_channels() == 1 {
                bytes.extend_from_slice(&((left + right) * 0.5).clamp(-1.0, 1.0).to_le_bytes());
            } else {
                bytes.extend_from_slice(&left.to_le_bytes());
                bytes.extend_from_slice(&right.to_le_bytes());
            }
        }
        fs::write(path, bytes).unwrap();
    }

    fn sine_frame(i: usize, hz: u32) -> [f32; 2] {
        let t = i as f32 / hz as f32;
        [
            (t * 440.0 * 2.0 * PI).sin() * 0.5,
            (t * 660.0 * 2.0 * PI).sin() * 0.25,
        ]
    }

    fn encode_seconds(dir: &Path, name: &str, seconds: usize, quality: ExportQuality) -> Vec<u8> {
        let hz = quality.staging_hz();
        let frames = hz as usize * seconds;
        let staged = dir.join(format!("{name}.f32"));
        let dest = dir.join(name);
        stage_frames(&staged, quality, frames, |i| sine_frame(i, hz));
        write(&dest, &staged, quality).unwrap();
        fs::read(&dest).unwrap()
    }

    fn assert_cbr_header(
        frame: &MpegLayer3Frame,
        bitrate_kbps: u16,
        sample_rate: u32,
        channels: u8,
    ) {
        assert_eq!(frame.sample_rate, sample_rate);
        assert_eq!(frame.bitrate_kbps, bitrate_kbps);
        assert_eq!(frame.channels, channels);
    }

    fn leftovers(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("onerec-part"))
            .count()
    }

    #[test]
    fn meeting_stages_8khz_mono() {
        assert_eq!(ExportQuality::Meeting.staging_hz(), 8_000);
        assert_eq!(ExportQuality::Meeting.staging_channels(), 1);
        assert_eq!(ExportQuality::Meeting.staging_factor(), 6);
        assert_eq!(ExportQuality::Voice.staging_hz(), 16_000);
        assert_eq!(ExportQuality::Voice.staging_channels(), 1);
        assert_eq!(ExportQuality::High.staging_hz(), 48_000);
        assert_eq!(ExportQuality::High.staging_channels(), 2);
        assert_eq!(ExportQuality::High.staging_factor(), 1);
    }

    #[test]
    fn default_quality_is_meeting() {
        assert_eq!(ExportQuality::default(), ExportQuality::Meeting);
        assert_eq!(ExportQuality::Meeting.index(), 0);
        assert_eq!(ExportQuality::from_index(0), Some(ExportQuality::Meeting));
        assert_eq!(ExportQuality::from_index(5), None);
    }

    #[test]
    fn combo_labels_name_size() {
        for quality in ExportQuality::ALL {
            let label = quality.label();
            assert!(
                label.starts_with(quality.short_name()),
                "{label} does not start with {}",
                quality.short_name()
            );
            assert!(label.contains("kbps"), "{label}");
            assert!(label.contains("MB/h"), "{label}");
        }
    }

    #[test]
    fn percent_rounds_frame_counts() {
        assert_eq!(SaveProgress { done: 0, total: 0 }.percent(), 0);
        assert_eq!(SaveProgress { done: 1, total: 3 }.percent(), 33);
        assert_eq!(
            SaveProgress {
                done: 10,
                total: 10
            }
            .percent(),
            100
        );
    }

    #[test]
    fn writes_one_second_of_mpeg_layer_iii() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = encode_seconds(dir.path(), "one-second", 1, ExportQuality::Standard);
        assert_eq!(mp3.len(), 24_192);
        let frames = parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert_eq!(frames.len(), 42);
        for frame in &frames {
            assert_cbr_header(frame, 192, 48_000, 2);
            assert_eq!(frame.byte_len, 576);
        }
    }

    #[test]
    fn compact_headers_are_128_kbps() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = encode_seconds(dir.path(), "compact", 1, ExportQuality::Compact);
        let frames = parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert!(!frames.is_empty());
        for frame in &frames {
            assert_cbr_header(frame, 128, 48_000, 2);
            assert_eq!(frame.byte_len, 384);
        }
    }

    #[test]
    fn high_headers_are_320_kbps() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = encode_seconds(dir.path(), "high", 1, ExportQuality::High);
        let frames = parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert!(!frames.is_empty());
        for frame in &frames {
            assert_cbr_header(frame, 320, 48_000, 2);
            assert_eq!(frame.byte_len, 960);
        }
    }

    #[test]
    fn meeting_headers_are_8_kbps_8_khz_mono() {
        let dir = tempfile::tempdir().unwrap();
        let meeting = encode_seconds(dir.path(), "meeting", 1, ExportQuality::Meeting);
        let compact = encode_seconds(dir.path(), "compact", 1, ExportQuality::Compact);
        let frames = parse_layer3_cbr(&meeting).unwrap();
        assert!(!frames.is_empty());
        for frame in &frames {
            assert_cbr_header(frame, 8, 8_000, 1);
        }
        assert!(
            meeting.len() < compact.len() / 8,
            "meeting {} B was not far smaller than compact {} B",
            meeting.len(),
            compact.len()
        );
    }

    #[test]
    fn voice_headers_are_24_kbps_16_khz_mono() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = encode_seconds(dir.path(), "voice", 1, ExportQuality::Voice);
        let frames = parse_layer3_cbr(&mp3).unwrap();
        assert!(!frames.is_empty());
        for frame in &frames {
            assert_cbr_header(frame, 24, 16_000, 1);
        }
    }

    #[test]
    fn longer_pcm_yields_more_mpeg_frames() {
        let dir = tempfile::tempdir().unwrap();
        let short = encode_seconds(dir.path(), "one-second", 1, ExportQuality::Standard);
        let long = encode_seconds(dir.path(), "two-seconds", 2, ExportQuality::Standard);
        let short_frames = parse_mpeg1_layer3_cbr(&short).unwrap();
        let long_frames = parse_mpeg1_layer3_cbr(&long).unwrap();
        assert!(
            long_frames.len() > short_frames.len(),
            "2 s produced {} frames, 1 s produced {}",
            long_frames.len(),
            short_frames.len()
        );
        for frame in short_frames.iter().chain(&long_frames) {
            assert_cbr_header(frame, 192, 48_000, 2);
            assert_eq!(frame.byte_len, 576);
        }
    }

    #[test]
    fn destination_need_not_end_in_mp3() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take");
        stage_frames(&staged, ExportQuality::Standard, SECOND_FRAMES, |i| {
            sine_frame(i, MIX_SAMPLE_RATE)
        });
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
        stage_frames(&staged, ExportQuality::Standard, SECOND_FRAMES, |i| {
            sine_frame(i, MIX_SAMPLE_RATE)
        });
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
        stage_frames(&staged, ExportQuality::Standard, SECOND_FRAMES, |_| {
            [0.0, 0.0]
        });
        write(dir.path(), &staged, ExportQuality::Standard).unwrap_err();
        assert_eq!(leftovers(dir.path()), 0);
        assert!(dir.path().is_dir());
    }

    #[test]
    fn pump_reports_partial_pcm_frames_before_commit() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("ten.f32");
        let dest = dir.path().join("ten.mp3");
        let chunk = ExportQuality::Standard.chunk_frames();
        stage_frames(&staged, ExportQuality::Standard, chunk * 10, |i| {
            sine_frame(i, MIX_SAMPLE_RATE)
        });
        let mut encode = Encode::start(&dest, &staged, ExportQuality::Standard).unwrap();
        assert_eq!(
            encode.progress(),
            SaveProgress {
                done: 0,
                total: chunk as u64 * 10
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

    #[test]
    fn pump_reports_full_progress_before_commit() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("one.f32");
        let dest = dir.path().join("one.mp3");
        let chunk = ExportQuality::Standard.chunk_frames();
        stage_frames(&staged, ExportQuality::Standard, chunk, |i| {
            sine_frame(i, MIX_SAMPLE_RATE)
        });
        let mut encode = Encode::start(&dest, &staged, ExportQuality::Standard).unwrap();
        assert!(!encode.pump(Duration::ZERO).unwrap());
        while encode.progress().done < encode.progress().total {
            assert!(!encode.pump(Duration::ZERO).unwrap());
        }
        assert_eq!(encode.progress().percent(), 100);
        assert!(!dest.exists());
        assert!(encode.pump(Duration::from_secs(1)).unwrap());
        assert!(dest.exists());
    }
}
