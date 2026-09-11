use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 32;
const FORMAT_IEEE_FLOAT: u16 = 3;
const FMT_CHUNK_BYTES: u32 = 18;
const HEADER_AFTER_RIFF: u64 = 38;

pub(crate) fn write(destination: &Path, staged: &Path, sample_rate: u32) -> io::Result<()> {
    let data_len = fs::metadata(staged)?.len();
    if data_len > u32::MAX as u64 || HEADER_AFTER_RIFF + data_len > u32::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "take is too long for WAV",
        ));
    }
    let part = destination.with_extension("onerec-part");
    let result = write_part(&part, staged, sample_rate, data_len as u32);
    if result.is_err() {
        let _ = fs::remove_file(&part);
        return result;
    }
    if let Err(error) = fs::rename(&part, destination) {
        let _ = fs::remove_file(&part);
        return Err(error);
    }
    Ok(())
}

fn write_part(part: &Path, staged: &Path, sample_rate: u32, data_len: u32) -> io::Result<()> {
    let mut out = File::create(part)?;
    write_header(&mut out, sample_rate, data_len)?;
    let mut src = File::open(staged)?;
    io::copy(&mut src, &mut out)?;
    out.sync_all()?;
    Ok(())
}

fn write_header(out: &mut File, sample_rate: u32, data_len: u32) -> io::Result<()> {
    let block_align = CHANNELS * (BITS_PER_SAMPLE / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let riff_size = HEADER_AFTER_RIFF as u32 + data_len;
    out.write_all(b"RIFF")?;
    out.write_all(&riff_size.to_le_bytes())?;
    out.write_all(b"WAVE")?;
    out.write_all(b"fmt ")?;
    out.write_all(&FMT_CHUNK_BYTES.to_le_bytes())?;
    out.write_all(&FORMAT_IEEE_FLOAT.to_le_bytes())?;
    out.write_all(&CHANNELS.to_le_bytes())?;
    out.write_all(&sample_rate.to_le_bytes())?;
    out.write_all(&byte_rate.to_le_bytes())?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&BITS_PER_SAMPLE.to_le_bytes())?;
    // Non-PCM WAVE requires cbSize even when it is zero.
    out.write_all(&0u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    Ok(())
}
