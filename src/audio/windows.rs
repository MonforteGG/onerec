use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::sync::mpsc;
use std::time::Duration;

use ::windows::core::{Error as WindowsError, HSTRING};
use ::windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use ::windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use ::windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioCaptureClient, IAudioClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_UNSUPPORTED_FORMAT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVE_FORMAT_PCM,
};
use ::windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use ::windows::Win32::Media::Multimedia::{
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT,
};
use ::windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};

use crate::audio::stream::Tap;
use crate::audio::{AudioError, Branded, CaptureStream, Endpoint, Endpoints};
use crate::capture::CaptureError;
use crate::ids::{MicrophoneId, OutputDeviceId};

const SAMPLE_RATE: u32 = 48_000;
const BUFFER_HNS: i64 = 2_000_000;

const POLL: Duration = Duration::from_millis(15);

pub(crate) fn query() -> Result<Endpoints, AudioError> {
    let _com = Com::enter()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| failure("creating the audio endpoint enumerator", error))?;

    Ok(Endpoints::from_enumerated(
        collect(&enumerator, eCapture, MicrophoneId::parse)?,
        collect(&enumerator, eRender, OutputDeviceId::parse)?,
        default_id(&enumerator, eCapture, MicrophoneId::parse),
        default_id(&enumerator, eRender, OutputDeviceId::parse),
    ))
}

pub(crate) fn open_microphone(id: &MicrophoneId) -> Result<CaptureStream, AudioError> {
    open(id.as_str().to_owned(), Role::Microphone)
}

pub(crate) fn open_loopback(id: &OutputDeviceId) -> Result<CaptureStream, AudioError> {
    open(id.as_str().to_owned(), Role::Loopback)
}

#[derive(Clone, Copy)]
enum Role {
    Microphone,
    Loopback,
}

impl Role {
    fn thread_name(self) -> &'static str {
        match self {
            Role::Microphone => "onerec-wasapi-mic",
            Role::Loopback => "onerec-wasapi-loop",
        }
    }

    fn stream_flags(self) -> u32 {
        let convert = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        match self {
            Role::Microphone => convert,
            Role::Loopback => convert | AUDCLNT_STREAMFLAGS_LOOPBACK,
        }
    }
}

fn open(endpoint: String, role: Role) -> Result<CaptureStream, AudioError> {
    let (opened, opened_rx) = mpsc::sync_channel::<Result<(), AudioError>>(1);
    let stream = CaptureStream::spawn(role.thread_name(), move |tap| {
        let client = match Client::open(&endpoint, role) {
            Ok(client) => {
                let _ = opened.send(Ok(()));
                client
            }
            Err(error) => {
                let _ = opened.send(Err(error));
                return Ok(());
            }
        };
        client.pump(tap)
    })?;
    match opened_rx.recv() {
        Ok(Ok(())) => Ok(stream),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(AudioError::new(
            "the capture thread ended before the device opened",
        )),
    }
}

struct Client {
    audio: IAudioClient,
    capture: IAudioCaptureClient,
    format: StereoDecoder,
    _com: Com,
}

impl Client {
    fn open(endpoint: &str, role: Role) -> Result<Self, AudioError> {
        let com = Com::enter()?;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| failure("creating the audio endpoint enumerator", error))?;
        let device = unsafe { enumerator.GetDevice(&HSTRING::from(endpoint)) }
            .map_err(|error| failure(&format!("opening endpoint {endpoint}"), error))?;

        let (audio, format) = activate_shared_or_mix(&device, role.stream_flags())?;
        let capture: IAudioCaptureClient = unsafe { audio.GetService() }
            .map_err(|error| failure("requesting the capture service", error))?;

        Ok(Self {
            audio,
            capture,
            format,
            _com: com,
        })
    }

    fn pump(&self, tap: &Tap) -> Result<(), CaptureError> {
        unsafe { self.audio.Start() }.map_err(|error| lost("starting capture", error))?;
        let outcome = self.drain_until_stop(tap);
        let _ = unsafe { self.audio.Stop() };
        outcome
    }

    fn drain_until_stop(&self, tap: &Tap) -> Result<(), CaptureError> {
        while !tap.wait_for_stop(POLL) {
            loop {
                let packet = unsafe { self.capture.GetNextPacketSize() }
                    .map_err(|error| lost("asking for the next capture packet", error))?;
                if packet == 0 || !self.take_packet(tap)? {
                    break;
                }
            }
        }
        Ok(())
    }

    fn take_packet(&self, tap: &Tap) -> Result<bool, CaptureError> {
        let mut data = ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        unsafe {
            self.capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
        }
        .map_err(|error| lost("reading a capture packet", error))?;

        if frames == 0 || data.is_null() {
            let _ = unsafe { self.capture.ReleaseBuffer(frames) };
            return Ok(false);
        }
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
            tap.push_silence(frames as usize);
        } else {
            let bytes =
                unsafe { slice::from_raw_parts(data, frames as usize * self.format.frame_bytes) };
            tap.push(&self.format.front_pair(bytes));
        }
        unsafe { self.capture.ReleaseBuffer(frames) }
            .map_err(|error| lost("releasing a capture packet", error))?;
        Ok(true)
    }
}

fn activate_shared_or_mix(
    device: &IMMDevice,
    flags: u32,
) -> Result<(IAudioClient, StereoDecoder), AudioError> {
    let audio = activate(device)?;
    let desired = stereo_float();
    let attempt = unsafe {
        audio.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            BUFFER_HNS,
            0,
            &desired,
            None,
        )
    };
    match attempt {
        Ok(()) => return Ok((audio, StereoDecoder::STEREO_FLOAT)),
        Err(error) if error.code() != AUDCLNT_E_UNSUPPORTED_FORMAT => {
            return Err(failure("initializing the audio client", error))
        }
        Err(_) => {}
    }

    let audio = activate(device)?;
    let mix = MixFormat::of(&audio)?;
    let format = StereoDecoder::of(mix.as_ptr())?;
    let flags =
        flags & !(AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY);
    unsafe {
        audio.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            BUFFER_HNS,
            0,
            mix.as_ptr(),
            None,
        )
    }
    .map_err(|error| failure("initializing the audio client in its own mix format", error))?;
    Ok((audio, format))
}

fn activate(device: &IMMDevice) -> Result<IAudioClient, AudioError> {
    unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|error| failure("activating the audio client", error))
}

fn stereo_float() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
        nChannels: 2,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * 8,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

struct MixFormat(*mut WAVEFORMATEX);

impl MixFormat {
    fn of(audio: &IAudioClient) -> Result<Self, AudioError> {
        let raw = unsafe { audio.GetMixFormat() }
            .map_err(|error| failure("reading the device mix format", error))?;
        if raw.is_null() {
            return Err(AudioError::new("the device reported no mix format"));
        }
        Ok(Self(raw))
    }

    fn as_ptr(&self) -> *const WAVEFORMATEX {
        self.0
    }
}

impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const c_void)) };
    }
}

struct StereoDecoder {
    sample: Sample,
    channels: usize,
    frame_bytes: usize,
}

#[derive(Clone, Copy)]
enum Sample {
    Float32,
    Int16,
    Int24,
    Int32,
}

impl StereoDecoder {
    const STEREO_FLOAT: Self = Self {
        sample: Sample::Float32,
        channels: 2,
        frame_bytes: 8,
    };

    fn of(wave: *const WAVEFORMATEX) -> Result<Self, AudioError> {
        let header = unsafe { *wave };
        let rate = header.nSamplesPerSec;
        let channels = header.nChannels as usize;
        let frame_bytes = header.nBlockAlign as usize;
        let tag = header.wFormatTag as u32;
        if rate != SAMPLE_RATE {
            return Err(AudioError::new(format!(
                "the device runs at {rate} Hz and this build has no resampler"
            )));
        }
        if channels == 0 || frame_bytes == 0 || !frame_bytes.is_multiple_of(channels) {
            return Err(AudioError::new(format!(
                "the device reported {channels} channels in {frame_bytes} bytes per frame"
            )));
        }
        let float = is_float(wave, tag)?;
        let stride = frame_bytes / channels;
        let sample = match (float, stride) {
            (true, 4) => Sample::Float32,
            (false, 2) => Sample::Int16,
            (false, 3) => Sample::Int24,
            (false, 4) => Sample::Int32,
            _ => {
                return Err(AudioError::new(format!(
                    "the device uses an unsupported {} byte sample",
                    stride
                )))
            }
        };
        Ok(Self {
            sample,
            channels,
            frame_bytes,
        })
    }

    fn front_pair(&self, bytes: &[u8]) -> Vec<[f32; 2]> {
        let stride = self.frame_bytes / self.channels;
        bytes
            .chunks_exact(self.frame_bytes)
            .map(|frame| {
                let left = self.sample.read(&frame[..stride]);
                let right = if self.channels > 1 {
                    self.sample.read(&frame[stride..stride * 2])
                } else {
                    left
                };
                [left, right]
            })
            .collect()
    }
}

impl Sample {
    fn read(self, bytes: &[u8]) -> f32 {
        match self {
            Sample::Float32 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            Sample::Int16 => i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0,
            Sample::Int24 => {
                let sample =
                    ((bytes[2] as i8 as i32) << 16) | ((bytes[1] as i32) << 8) | (bytes[0] as i32);
                sample as f32 / 8_388_608.0
            }
            Sample::Int32 => {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                    / 2_147_483_648.0
            }
        }
    }
}

fn is_float(wave: *const WAVEFORMATEX, tag: u32) -> Result<bool, AudioError> {
    match tag {
        WAVE_FORMAT_IEEE_FLOAT => Ok(true),
        WAVE_FORMAT_PCM => Ok(false),
        WAVE_FORMAT_EXTENSIBLE => {
            let subformat = unsafe { (*(wave as *const WAVEFORMATEXTENSIBLE)).SubFormat };
            if subformat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                Ok(true)
            } else if subformat == KSDATAFORMAT_SUBTYPE_PCM {
                Ok(false)
            } else {
                Err(AudioError::new(format!(
                    "the device uses an unsupported sample subtype {subformat:?}"
                )))
            }
        }
        other => Err(AudioError::new(format!(
            "the device uses an unsupported wave format {other}"
        ))),
    }
}

fn lost(context: &str, error: WindowsError) -> CaptureError {
    CaptureError::device_lost(format!("{context} failed: {error}"))
}

fn collect<Id: Branded, E>(
    enumerator: &IMMDeviceEnumerator,
    flow: EDataFlow,
    parse: fn(String) -> Result<Id, E>,
) -> Result<Vec<Endpoint<Id>>, AudioError> {
    let collection = unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }
        .map_err(|error| failure("enumerating audio endpoints", error))?;
    let count = unsafe { collection.GetCount() }
        .map_err(|error| failure("counting audio endpoints", error))?;
    let mut found = Vec::with_capacity(count as usize);
    for index in 0..count {
        let device = unsafe { collection.Item(index) }
            .map_err(|error| failure("reading an audio endpoint", error))?;
        let Some(id) = endpoint_id(&device).and_then(|raw| parse(raw).ok()) else {
            continue;
        };
        found.push(Endpoint::new(id, friendly_name(&device)));
    }
    Ok(found)
}

fn default_id<Id, E>(
    enumerator: &IMMDeviceEnumerator,
    flow: EDataFlow,
    parse: fn(String) -> Result<Id, E>,
) -> Option<Id> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }.ok()?;
    endpoint_id(&device).and_then(|raw| parse(raw).ok())
}

fn endpoint_id(device: &IMMDevice) -> Option<String> {
    let raw = unsafe { device.GetId() }.ok()?;
    let text = unsafe { raw.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(raw.0 as *const c_void)) };
    text
}

fn friendly_name(device: &IMMDevice) -> String {
    let Ok(store) = (unsafe { device.OpenPropertyStore(STGM_READ) }) else {
        return String::new();
    };
    let Ok(value) = (unsafe { store.GetValue(&PKEY_Device_FriendlyName) }) else {
        return String::new();
    };
    value.to_string()
}

fn failure(context: &str, error: WindowsError) -> AudioError {
    AudioError::new(format!("{context} failed: {error}"))
}

/// One MTA apartment per thread that touches WASAPI. A thread already in an STA keeps it,
/// because enumeration and shared-mode capture work from either apartment.
struct Com {
    owned: bool,
}

impl Com {
    fn enter() -> Result<Self, AudioError> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result == RPC_E_CHANGED_MODE {
            Ok(Self { owned: false })
        } else if result.is_ok() {
            Ok(Self { owned: true })
        } else {
            Err(AudioError::new(format!(
                "COM would not start on this thread: {}",
                result.message()
            )))
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int24_reads_little_endian_with_sign() {
        assert!((Sample::Int24.read(&[0, 0, 0x40]) - 0.5).abs() < 1e-6);
        assert!((Sample::Int24.read(&[0, 0, 0x80]) + 1.0).abs() < 1e-6);
    }
}
