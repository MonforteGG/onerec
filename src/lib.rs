mod audio;
mod capture;
mod ids;
mod mp3;
mod prefs;
mod recorder;
mod save_path;
mod sidecar;
mod session;
mod staging;
mod timeline;
mod vault;
#[cfg(windows)]
mod window;

pub use audio::{
    open_loopback, open_microphone, AudioError, CaptureStream, Endpoint, Endpoints, Microphone,
    OutputDevice,
};
pub use capture::{
    CaptureError, CaptureRead, CaptureSource, InvalidFrames, NoPacketSource, PcmSource,
    SessionFrame, TimedStereoFrames,
};
pub use ids::{DeviceIdError, MicrophoneId, OutputDeviceId};
pub use session::{
    DiscardError, ExportQuality, FailedSession, PendingRecording, SaveError, SaveProgress, Session,
};
pub use staging::{StagingArea, StagingFile};
pub use timeline::{draw, Draw, MAX_BACKLOG_FRAMES, MIX_QUANTUM_FRAMES, MIX_SAMPLE_RATE, MIX_TICK};

use std::fmt;

pub fn run() -> Result<(), RunError> {
    #[cfg(windows)]
    {
        window::run()
    }
    #[cfg(not(windows))]
    {
        Err(RunError::new("v1 is Windows-only"))
    }
}

#[derive(Debug)]
pub struct RunError {
    message: String,
}

impl RunError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RunError {}

#[cfg(all(test, not(windows)))]
mod tests {
    #[test]
    fn run_reports_windows_only_off_windows() {
        assert_eq!(crate::run().unwrap_err().to_string(), "v1 is Windows-only");
    }
}
