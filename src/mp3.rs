use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mp3lame_encoder::{Bitrate, Builder, Encoder, FlushNoGap, InterleavedPcm, Mode, Quality};

use crate::timeline::MIX_SAMPLE_RATE;

const CHANNELS: u8 = 2;
const STEREO_FRAME_BYTES: u64 = 8;
const CHUNK_FRAMES: usize = MIX_SAMPLE_RATE as usize;
const FLUSH_CAPACITY: usize = 7200;

static NEXT_PART: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write(destination: &Path, staged: &Path) -> io::Result<()> {
    let bytes = fs::metadata(staged)?.len();
    if bytes % STEREO_FRAME_BYTES != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "staging file is not a whole number of stereo frames",
        ));
    }
    let mut part = PartFile::create(destination)?;
    encode_into(&mut part, staged)?;
    part.commit(destination)
}

fn encode_into(part: &mut PartFile, staged: &Path) -> io::Result<()> {
    let mut encoder = lame()?;
    let mut src = File::open(staged)?;
    let mut raw = vec![0u8; CHUNK_FRAMES * STEREO_FRAME_BYTES as usize];
    let mut pcm = Vec::with_capacity(CHUNK_FRAMES * usize::from(CHANNELS));
    let mut encoded = Vec::new();
    loop {
        let n = fill(&mut src, &mut raw)?;
        if n == 0 {
            break;
        }
        if n as u64 % STEREO_FRAME_BYTES != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staging file is not a whole number of stereo frames",
            ));
        }
        pcm.clear();
        pcm.extend(
            raw[..n]
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap())),
        );
        encoded.clear();
        encoded.reserve(mp3lame_encoder::max_required_buffer_size(pcm.len() / 2));
        encoder
            .encode_to_vec(InterleavedPcm(pcm.as_slice()), &mut encoded)
            .map_err(encode_fail)?;
        part.write_all(&encoded)?;
    }
    encoded.clear();
    encoded.reserve(FLUSH_CAPACITY);
    encoder
        .flush_to_vec::<FlushNoGap>(&mut encoded)
        .map_err(encode_fail)?;
    part.write_all(&encoded)
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

fn lame() -> io::Result<Encoder> {
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
    builder.set_brate(Bitrate::Kbps192).map_err(encode_fail)?;
    builder.set_quality(Quality::Best).map_err(encode_fail)?;
    builder.set_mode(Mode::JointStereo).map_err(encode_fail)?;
    // LAME's 576-byte Info frame is the 42nd CBR frame in a measured 1 s file.
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
        match fs::rename(&self.path, destination) {
            Ok(()) => {
                self.committed = true;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                // Win32 rename refuses to replace an existing destination.
                fs::remove_file(destination)?;
                fs::rename(&self.path, destination)?;
                self.committed = true;
                Ok(())
            }
            Err(error) => Err(error),
        }
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
mod tests {
    use super::*;
    use std::f32::consts::PI;

    struct MpegLayer3Frame {
        byte_len: usize,
        sample_rate: u32,
        bitrate_kbps: u16,
        channels: u8,
    }

    fn parse_mpeg1_layer3_cbr(bytes: &[u8]) -> Result<Vec<MpegLayer3Frame>, String> {
        // MPEG-1 Layer III CBR 192 kbps at 48 kHz is 144 * 192000 / 48000 bytes per frame.
        const FRAME_LEN: usize = 576;
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
            let bitrate_index = h2 >> 4;
            let sample_rate_index = (h2 >> 2) & 0b11;
            let padding = (h2 >> 1) & 1;
            let channel_mode = h3 >> 6;
            if bitrate_index != 11 {
                return Err(format!("bitrate index {bitrate_index} at byte {offset}"));
            }
            if sample_rate_index != 1 {
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
            if offset + FRAME_LEN > bytes.len() {
                return Err(format!("truncated MPEG frame at byte {offset}"));
            }
            frames.push(MpegLayer3Frame {
                byte_len: FRAME_LEN,
                sample_rate: 48_000,
                bitrate_kbps: 192,
                channels: 2,
            });
            offset += FRAME_LEN;
        }
        Ok(frames)
    }

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

    fn encode_frames(dir: &Path, name: &str, frames: usize) -> Vec<u8> {
        let staged = dir.join(format!("{name}.f32"));
        let dest = dir.join(name);
        stage_frames(&staged, frames, sine_frame);
        write(&dest, &staged).unwrap();
        fs::read(&dest).unwrap()
    }

    fn assert_cbr_header(frame: &MpegLayer3Frame) {
        assert_eq!(frame.byte_len, 576);
        assert_eq!(frame.sample_rate, 48_000);
        assert_eq!(frame.bitrate_kbps, 192);
        assert_eq!(frame.channels, 2);
    }

    #[test]
    fn writes_one_second_of_mpeg_layer_iii() {
        let dir = tempfile::tempdir().unwrap();
        let mp3 = encode_frames(dir.path(), "one-second", CHUNK_FRAMES);
        assert_eq!(mp3.len(), 24_192);
        let frames = parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert_eq!(frames.len(), 42);
        for frame in &frames {
            assert_cbr_header(frame);
        }
    }

    #[test]
    fn longer_pcm_yields_more_mpeg_frames() {
        let dir = tempfile::tempdir().unwrap();
        let short = encode_frames(dir.path(), "one-second", CHUNK_FRAMES);
        let long = encode_frames(dir.path(), "two-seconds", CHUNK_FRAMES * 2);
        let short_frames = parse_mpeg1_layer3_cbr(&short).unwrap();
        let long_frames = parse_mpeg1_layer3_cbr(&long).unwrap();
        assert!(
            long_frames.len() > short_frames.len(),
            "2 s produced {} frames, 1 s produced {}",
            long_frames.len(),
            short_frames.len()
        );
        for frame in short_frames.iter().chain(&long_frames) {
            assert_cbr_header(frame);
        }
    }

    #[test]
    fn destination_need_not_end_in_mp3() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take");
        stage_frames(&staged, CHUNK_FRAMES, sine_frame);
        write(&dest, &staged).unwrap();
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
        write(&dest, &staged).unwrap_err();
        assert_eq!(fs::read(&dest).unwrap(), b"keep");
        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("onerec-part"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn unwritable_destination_unlinks_the_part() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        stage_frames(&staged, CHUNK_FRAMES, |_| [0.0, 0.0]);
        write(dir.path(), &staged).unwrap_err();
        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("onerec-part"))
            .count();
        assert_eq!(leftovers, 0);
        assert!(dir.path().is_dir());
    }
}
