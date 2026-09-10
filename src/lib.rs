mod audio;
mod capture;
mod ids;
mod session;
mod timeline;

pub use audio::{
    open_loopback, open_microphone, AudioError, CaptureStream, Endpoint, Endpoints, Microphone,
    OutputDevice,
};
pub use capture::{
    CaptureError, CaptureRead, CaptureSource, InvalidFrames, NoPacketSource, PcmSource,
    SessionFrame, TimedStereoFrames,
};
pub use ids::{DeviceIdError, MicrophoneId, OutputDeviceId};
pub use session::{FailedSession, PendingRecording, Session};
pub use timeline::{draw, Draw, MAX_BACKLOG_FRAMES, MIX_QUANTUM_FRAMES, MIX_TICK};

use std::fmt;

pub fn run() -> Result<(), RunError> {
    #[cfg(windows)]
    {
        Err(RunError {
            message: "the recorder window is not in this build".into(),
        })
    }
    #[cfg(not(windows))]
    {
        Err(RunError {
            message: "v1 is Windows-only".into(),
        })
    }
}

#[derive(Debug)]
pub struct RunError {
    message: String,
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RunError {}

#[cfg(test)]
mod tests {
    #[test]
    fn run_reports_windows_only_off_windows() {
        #[cfg(not(windows))]
        {
            assert_eq!(crate::run().unwrap_err().to_string(), "v1 is Windows-only");
        }
        #[cfg(windows)]
        {
            assert_eq!(
                crate::run().unwrap_err().to_string(),
                "the recorder window is not in this build"
            );
        }
    }
}
