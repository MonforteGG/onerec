use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::capture::{CaptureError, CaptureRead, CaptureSource};

const RELEASE: Duration = Duration::from_millis(300);
const CLIP_HOLD: Duration = Duration::from_millis(1200);
const CLIP_AT: f32 = 0.999;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Level {
    pub peak: f32,
    pub clipping: bool,
}

impl Level {
    pub(crate) const ZERO: Self = Self {
        peak: 0.0,
        clipping: false,
    };
}

pub(crate) struct LevelCell {
    peak_bits: AtomicU32,
}

impl LevelCell {
    fn new() -> Self {
        Self {
            peak_bits: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    fn observe(&self, frames: &[[f32; 2]]) {
        let peak = frames.iter().flatten().fold(0.0f32, |acc, sample| {
            let magnitude = sample.abs();
            if magnitude.is_finite() {
                acc.max(magnitude)
            } else {
                acc
            }
        });
        // Non-negative finite f32 bits sort the same as their values, so fetch_max is a peak hold.
        self.peak_bits.fetch_max(peak.to_bits(), Ordering::Relaxed);
    }

    fn take(&self) -> f32 {
        f32::from_bits(self.peak_bits.swap(0.0f32.to_bits(), Ordering::Relaxed))
    }
}

pub(crate) struct LevelTap<S> {
    inner: S,
    cell: Arc<LevelCell>,
}

impl<S: CaptureSource> CaptureSource for LevelTap<S> {
    fn read(&mut self, max_wait: Duration) -> Result<CaptureRead, CaptureError> {
        let read = self.inner.read(max_wait);
        if let Ok(CaptureRead::Frames(packet)) = &read {
            self.cell.observe(packet.frames());
        }
        read
    }
}

pub(crate) struct Vu {
    cell: Arc<LevelCell>,
    level: f32,
    clip_until: Option<Instant>,
}

impl Vu {
    pub(crate) fn new() -> Self {
        Self {
            cell: Arc::new(LevelCell::new()),
            level: 0.0,
            clip_until: None,
        }
    }

    pub(crate) fn tap<S: CaptureSource>(&self, source: S) -> LevelTap<S> {
        LevelTap {
            inner: source,
            cell: Arc::clone(&self.cell),
        }
    }

    pub(crate) fn advance(&mut self, dt: Duration) -> Level {
        let raw = self.cell.take();
        if raw >= self.level {
            self.level = raw;
        } else {
            let tau = RELEASE.as_secs_f32();
            let decay = if tau > 0.0 {
                1.0 - (-dt.as_secs_f32() / tau).exp()
            } else {
                1.0
            };
            self.level += (raw - self.level) * decay;
        }
        if raw >= CLIP_AT {
            self.clip_until = Some(Instant::now() + CLIP_HOLD);
        }
        self.snapshot()
    }

    pub(crate) fn snapshot(&self) -> Level {
        Level {
            peak: self.level,
            clipping: self.clip_until.is_some_and(|until| until > Instant::now()),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.level = 0.0;
        self.clip_until = None;
        let _ = self.cell.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{SessionFrame, TimedStereoFrames};

    struct Packet(f32);

    impl CaptureSource for Packet {
        fn read(&mut self, _max_wait: Duration) -> Result<CaptureRead, CaptureError> {
            Ok(CaptureRead::Frames(
                TimedStereoFrames::try_new(SessionFrame::from_index(0), vec![[self.0, self.0]])
                    .unwrap(),
            ))
        }
    }

    #[test]
    fn vu_attacks_instantly_and_releases_over_300ms() {
        let mut vu = Vu::new();
        let mut tap = vu.tap(Packet(0.8));
        let _ = tap.read(Duration::ZERO).unwrap();
        let attacked = vu.advance(Duration::ZERO);
        assert_eq!(attacked.peak, 0.8);
        assert!(!attacked.clipping);

        let released = vu.advance(Duration::from_millis(300));
        let expected = 0.8 / std::f32::consts::E;
        assert!(
            (released.peak - expected).abs() < 0.02,
            "released {} expected ~{}",
            released.peak,
            expected
        );
    }
}
