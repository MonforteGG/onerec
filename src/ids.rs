use std::fmt;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MicrophoneId(Arc<str>);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OutputDeviceId(Arc<str>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceIdError;

impl MicrophoneId {
    pub fn parse(raw: String) -> Result<Self, DeviceIdError> {
        parse_id(raw).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl OutputDeviceId {
    pub fn parse(raw: String) -> Result<Self, DeviceIdError> {
        parse_id(raw).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn parse_id(raw: String) -> Result<Arc<str>, DeviceIdError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Err(DeviceIdError)
    } else {
        Ok(Arc::from(trimmed))
    }
}

impl fmt::Display for DeviceIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("device id is empty")
    }
}

impl std::error::Error for DeviceIdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_blank_ids() {
        assert_eq!(
            MicrophoneId::parse(String::new()).unwrap_err(),
            DeviceIdError
        );
        assert_eq!(
            OutputDeviceId::parse("   ".into()).unwrap_err(),
            DeviceIdError
        );
    }

    #[test]
    fn parse_keeps_the_trimmed_endpoint() {
        let mic = MicrophoneId::parse("  {0.0.1.00000000}.mic  ".into()).unwrap();
        let out = OutputDeviceId::parse("  {0.0.0.00000000}.speakers  ".into()).unwrap();
        assert_eq!(mic.as_str(), "{0.0.1.00000000}.mic");
        assert_eq!(out.as_str(), "{0.0.0.00000000}.speakers");
    }
}
