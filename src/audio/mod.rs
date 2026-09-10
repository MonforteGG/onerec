mod stream;

#[cfg(windows)]
mod windows;

pub use stream::CaptureStream;

use std::fmt;
use std::sync::Arc;

use crate::ids::{MicrophoneId, OutputDeviceId};

/// What the audio endpoints looked like when `query` ran. Devices come and go, so a
/// caller refreshes by taking another snapshot rather than holding this one.
#[derive(Clone, Debug)]
pub struct Endpoints {
    microphones: Vec<Microphone>,
    outputs: Vec<OutputDevice>,
    default_microphone: Option<MicrophoneId>,
    default_output: Option<OutputDeviceId>,
}

pub type Microphone = Endpoint<MicrophoneId>;
pub type OutputDevice = Endpoint<OutputDeviceId>;

#[derive(Clone, Debug)]
pub struct Endpoint<Id> {
    id: Id,
    name: String,
}

/// One capture thread with one `IAudioClient`. Opening two devices means two of these,
/// sharing nothing, so losing one leaves the other recording.
pub fn open_microphone(id: &MicrophoneId) -> Result<CaptureStream, AudioError> {
    #[cfg(windows)]
    {
        windows::open_microphone(id)
    }
    #[cfg(not(windows))]
    {
        let _ = id;
        Err(unsupported())
    }
}

pub fn open_loopback(id: &OutputDeviceId) -> Result<CaptureStream, AudioError> {
    #[cfg(windows)]
    {
        windows::open_loopback(id)
    }
    #[cfg(not(windows))]
    {
        let _ = id;
        Err(unsupported())
    }
}

#[cfg(not(windows))]
fn unsupported() -> AudioError {
    AudioError::new("audio capture needs Windows")
}

impl Endpoints {
    pub fn query() -> Result<Self, AudioError> {
        #[cfg(windows)]
        {
            windows::query()
        }
        #[cfg(not(windows))]
        {
            Err(unsupported())
        }
    }

    pub fn microphones(&self) -> &[Microphone] {
        &self.microphones
    }

    pub fn outputs(&self) -> &[OutputDevice] {
        &self.outputs
    }

    pub fn default_microphone(&self) -> Option<&MicrophoneId> {
        self.default_microphone.as_ref()
    }

    pub fn default_output(&self) -> Option<&OutputDeviceId> {
        self.default_output.as_ref()
    }

    /// A device can disappear between the enumeration walk and the default lookup, so a
    /// default that is not in its own list is dropped rather than handed out as a
    /// selection nothing can render.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn new(
        microphones: Vec<Microphone>,
        outputs: Vec<OutputDevice>,
        default_microphone: Option<MicrophoneId>,
        default_output: Option<OutputDeviceId>,
    ) -> Self {
        let default_microphone =
            default_microphone.filter(|id| microphones.iter().any(|found| found.id() == id));
        let default_output =
            default_output.filter(|id| outputs.iter().any(|found| found.id() == id));
        Self {
            microphones,
            outputs,
            default_microphone,
            default_output,
        }
    }
}

impl<Id> Endpoint<Id> {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn new(id: Id, name: String) -> Self
    where
        Id: Branded,
    {
        let name = name.trim();
        let name = if name.is_empty() {
            id.as_str().to_owned()
        } else {
            name.to_owned()
        };
        Self { id, name }
    }

    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Never blank. A device with no friendly name is labelled by its endpoint id.
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub(crate) trait Branded {
    fn as_str(&self) -> &str;
}

impl Branded for MicrophoneId {
    fn as_str(&self) -> &str {
        MicrophoneId::as_str(self)
    }
}

impl Branded for OutputDeviceId {
    fn as_str(&self) -> &str {
        OutputDeviceId::as_str(self)
    }
}

/// Opening or enumerating failed. Mid-stream device loss is `CaptureError`, not this.
#[derive(Clone, Debug)]
pub struct AudioError {
    detail: Arc<str>,
}

impl AudioError {
    pub(crate) fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: Arc::from(detail.into()),
        }
    }
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn microphone(id: &str, name: &str) -> Microphone {
        Endpoint::new(MicrophoneId::parse(id.into()).unwrap(), name.into())
    }

    #[test]
    fn a_blank_friendly_name_falls_back_to_the_endpoint_id() {
        assert_eq!(microphone("{0.0.1}.mic", "   ").name(), "{0.0.1}.mic");
        assert_eq!(microphone("{0.0.1}.mic", " Yeti Nano ").name(), "Yeti Nano");
    }

    #[test]
    fn a_default_missing_from_its_list_is_not_handed_out() {
        let present = MicrophoneId::parse("{0.0.1}.present".into()).unwrap();
        let vanished = MicrophoneId::parse("{0.0.1}.vanished".into()).unwrap();
        let endpoints = Endpoints::new(
            vec![microphone("{0.0.1}.present", "Present")],
            Vec::new(),
            Some(vanished),
            None,
        );
        assert_eq!(endpoints.default_microphone(), None);

        let endpoints = Endpoints::new(
            vec![microphone("{0.0.1}.present", "Present")],
            Vec::new(),
            Some(present.clone()),
            None,
        );
        assert_eq!(endpoints.default_microphone(), Some(&present));
    }
}
