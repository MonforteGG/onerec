use crate::audio::{open_loopback, open_microphone, AudioError, Endpoints};
use crate::capture::CaptureSource;
use crate::ids::{MicrophoneId, OutputDeviceId};

use super::Devices;

pub(crate) struct Wasapi;

impl Devices for Wasapi {
    fn survey(&self) -> Result<Endpoints, AudioError> {
        Endpoints::query()
    }

    fn open_microphone(&self, id: &MicrophoneId) -> Result<Box<dyn CaptureSource>, AudioError> {
        Ok(Box::new(open_microphone(id)?))
    }

    fn open_loopback(&self, id: &OutputDeviceId) -> Result<Box<dyn CaptureSource>, AudioError> {
        Ok(Box::new(open_loopback(id)?))
    }
}
