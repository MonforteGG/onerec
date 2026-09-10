use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionFrame(u64);

impl SessionFrame {
    pub fn index(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub struct TimedStereoFrames {
    first_frame: SessionFrame,
    frames: Box<[[f32; 2]]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidFrames;

impl TimedStereoFrames {
    pub fn try_new(
        first_frame: SessionFrame,
        frames: Vec<[f32; 2]>,
    ) -> Result<Self, InvalidFrames> {
        if frames.is_empty() {
            return Err(InvalidFrames);
        }
        for [left, right] in &frames {
            if !left.is_finite() || !right.is_finite() {
                return Err(InvalidFrames);
            }
            if left.abs() > 1.0 || right.abs() > 1.0 {
                return Err(InvalidFrames);
            }
        }
        Ok(Self {
            first_frame,
            frames: frames.into_boxed_slice(),
        })
    }

    pub fn first_frame(&self) -> SessionFrame {
        self.first_frame
    }

    pub fn frames(&self) -> &[[f32; 2]] {
        &self.frames
    }
}

#[derive(Debug)]
pub enum CaptureRead {
    Frames(TimedStereoFrames),
    Pending,
}

pub trait CaptureSource: Send + 'static {
    fn read(&mut self, max_wait: Duration) -> Result<CaptureRead, CaptureError>;
}

#[derive(Debug)]
pub struct CaptureError {
    detail: Arc<str>,
}

impl CaptureError {
    pub fn device_lost(detail: impl Into<String>) -> Self {
        Self {
            detail: Arc::from(detail.into()),
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for CaptureError {}

pub struct PendingSource;

impl CaptureSource for PendingSource {
    fn read(&mut self, _max_wait: Duration) -> Result<CaptureRead, CaptureError> {
        Ok(CaptureRead::Pending)
    }
}

const SYNTH_PACKET_FRAMES: usize = 480;

pub struct PcmSource {
    sample: [f32; 2],
    next_frame: u64,
    plugged: Arc<AtomicBool>,
}

impl PcmSource {
    pub fn silence() -> Self {
        Self::with_plug([0.0, 0.0], Arc::new(AtomicBool::new(true)))
    }

    pub fn tone(amplitude: f32) -> Self {
        Self::with_plug([amplitude, amplitude], Arc::new(AtomicBool::new(true)))
    }

    pub fn with_plug(sample: [f32; 2], plugged: Arc<AtomicBool>) -> Self {
        Self {
            sample,
            next_frame: 0,
            plugged,
        }
    }
}

impl CaptureSource for PcmSource {
    fn read(&mut self, _max_wait: Duration) -> Result<CaptureRead, CaptureError> {
        if !self.plugged.load(Ordering::SeqCst) {
            return Err(CaptureError::device_lost("source unplugged"));
        }
        let first = self.next_frame;
        self.next_frame += SYNTH_PACKET_FRAMES as u64;
        let frames = vec![self.sample; SYNTH_PACKET_FRAMES];
        Ok(CaptureRead::Frames(TimedStereoFrames {
            first_frame: SessionFrame(first),
            frames: frames.into_boxed_slice(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_is_not_frames() {
        let mut source = PendingSource;
        assert!(matches!(
            source.read(Duration::ZERO).unwrap(),
            CaptureRead::Pending
        ));
    }

    #[test]
    fn try_new_rejects_empty_and_non_finite() {
        assert!(TimedStereoFrames::try_new(SessionFrame(0), Vec::new()).is_err());
        assert!(TimedStereoFrames::try_new(SessionFrame(0), vec![[f32::NAN, 0.0]]).is_err());
        assert!(TimedStereoFrames::try_new(SessionFrame(0), vec![[1.5, 0.0]]).is_err());
        let ok = TimedStereoFrames::try_new(SessionFrame(3), vec![[0.25, -1.0]]).unwrap();
        assert_eq!(ok.first_frame().index(), 3);
        assert_eq!(ok.frames(), &[[0.25, -1.0]]);
    }
}
