#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::audio::AudioError;
use crate::capture::{CaptureError, CaptureRead, CaptureSource, SessionFrame, TimedStereoFrames};

pub struct CaptureStream {
    tap: Arc<Tap>,
    next_frame: u64,
    worker: Option<JoinHandle<()>>,
}

pub(crate) struct Tap {
    state: Mutex<TapState>,
    ready: Condvar,
    stop: Mutex<bool>,
    stop_changed: Condvar,
}

struct TapState {
    samples: VecDeque<[f32; 2]>,
    lost: Option<CaptureError>,
}

impl CaptureStream {
    pub(crate) fn spawn<F>(thread_name: &str, produce: F) -> Result<Self, AudioError>
    where
        F: FnOnce(&Tap) -> Result<(), CaptureError> + Send + 'static,
    {
        let tap = Arc::new(Tap {
            state: Mutex::new(TapState {
                samples: VecDeque::new(),
                lost: None,
            }),
            ready: Condvar::new(),
            stop: Mutex::new(false),
            stop_changed: Condvar::new(),
        });
        let producer = Arc::clone(&tap);
        let worker = thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                if let Err(error) = produce(&producer) {
                    producer.fail(error);
                }
            })
            .map_err(|error| AudioError::new(format!("could not start {thread_name}: {error}")))?;
        Ok(Self {
            tap,
            next_frame: 0,
            worker: Some(worker),
        })
    }
}

impl CaptureSource for CaptureStream {
    fn read(&mut self, max_wait: Duration) -> Result<CaptureRead, CaptureError> {
        let mut state = ignore_poison(&self.tap.state);
        if !max_wait.is_zero() && state.samples.is_empty() && state.lost.is_none() {
            state = self
                .tap
                .ready
                .wait_timeout_while(state, max_wait, |state| {
                    state.samples.is_empty() && state.lost.is_none()
                })
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        let samples: Vec<[f32; 2]> = state.samples.drain(..).collect();
        if samples.is_empty() {
            return match state.lost.take() {
                Some(error) => Err(error),
                None => Ok(CaptureRead::NoPacket),
            };
        }
        drop(state);
        let first = SessionFrame::from_index(self.next_frame);
        self.next_frame += samples.len() as u64;
        let frames = TimedStereoFrames::try_new(first, samples)
            .expect("from_device_sample already clamped every sample and this drain is not empty");
        Ok(CaptureRead::Frames(frames))
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        self.tap.request_stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Tap {
    pub(crate) fn push(&self, frames: &[[f32; 2]]) {
        if frames.is_empty() {
            return;
        }
        let mut state = ignore_poison(&self.state);
        state.samples.extend(
            frames
                .iter()
                .map(|[left, right]| [from_device_sample(*left), from_device_sample(*right)]),
        );
        drop(state);
        self.ready.notify_one();
    }

    pub(crate) fn push_silence(&self, frames: usize) {
        if frames == 0 {
            return;
        }
        let mut state = ignore_poison(&self.state);
        state
            .samples
            .extend(std::iter::repeat_n([0.0, 0.0], frames));
        drop(state);
        self.ready.notify_one();
    }

    pub(crate) fn wait_for_stop(&self, timeout: Duration) -> bool {
        let stop = ignore_poison(&self.stop);
        if *stop || timeout.is_zero() {
            return *stop;
        }
        let (stop, _) = self
            .stop_changed
            .wait_timeout_while(stop, timeout, |stop| !*stop)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *stop
    }

    fn request_stop(&self) {
        *ignore_poison(&self.stop) = true;
        self.stop_changed.notify_all();
    }

    fn fail(&self, error: CaptureError) {
        let mut state = ignore_poison(&self.state);
        if state.lost.is_none() {
            state.lost = Some(error);
        }
        drop(state);
        self.ready.notify_one();
    }
}

fn from_device_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    fn frames_of(read: CaptureRead) -> TimedStereoFrames {
        match read {
            CaptureRead::Frames(frames) => frames,
            CaptureRead::NoPacket => panic!("expected frames, got NoPacket"),
        }
    }

    fn park_until_stop(tap: &Tap) {
        while !tap.wait_for_stop(Duration::from_millis(5)) {}
    }

    #[test]
    fn read_coalesces_every_buffered_push_into_one_packet() {
        let (pushed_tx, pushed_rx) = mpsc::channel();
        let mut stream = CaptureStream::spawn("test-coalesce", move |tap| {
            tap.push(&[[0.1, 0.2]]);
            tap.push(&[[0.3, 0.4], [0.5, 0.6]]);
            pushed_tx.send(()).unwrap();
            park_until_stop(tap);
            Ok(())
        })
        .unwrap();

        pushed_rx.recv().unwrap();
        let frames = frames_of(stream.read(Duration::ZERO).unwrap());
        assert_eq!(frames.first_frame().index(), 0);
        assert_eq!(frames.frames(), &[[0.1, 0.2], [0.3, 0.4], [0.5, 0.6]]);
    }

    #[test]
    fn consecutive_reads_stamp_contiguous_frame_indices() {
        let (pushed_tx, pushed_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel::<()>();
        let mut stream = CaptureStream::spawn("test-cursor", move |tap| {
            tap.push(&[[0.1, 0.1], [0.2, 0.2]]);
            pushed_tx.send(()).unwrap();
            resume_rx.recv().unwrap();
            tap.push(&[[0.3, 0.3]]);
            pushed_tx.send(()).unwrap();
            park_until_stop(tap);
            Ok(())
        })
        .unwrap();

        pushed_rx.recv().unwrap();
        let first = frames_of(stream.read(Duration::ZERO).unwrap());
        assert_eq!(first.first_frame().index(), 0);
        assert_eq!(first.frames().len(), 2);

        resume_tx.send(()).unwrap();
        pushed_rx.recv().unwrap();
        let second = frames_of(stream.read(Duration::ZERO).unwrap());
        assert_eq!(second.first_frame().index(), 2);
        assert_eq!(second.frames(), &[[0.3, 0.3]]);
    }

    #[test]
    fn empty_live_queue_reads_as_no_packet() {
        let (ready_tx, ready_rx) = mpsc::channel();
        let mut stream = CaptureStream::spawn("test-idle", move |tap| {
            ready_tx.send(()).unwrap();
            park_until_stop(tap);
            Ok(())
        })
        .unwrap();

        ready_rx.recv().unwrap();
        assert!(matches!(
            stream.read(Duration::ZERO).unwrap(),
            CaptureRead::NoPacket
        ));
    }

    #[test]
    fn buffered_frames_arrive_before_the_latched_device_loss() {
        let mut stream = CaptureStream::spawn("test-unplug", |tap| {
            tap.push(&[[0.25, -0.25]]);
            Err(CaptureError::device_lost("cable pulled"))
        })
        .unwrap();

        let frames = frames_of(stream.read(Duration::from_secs(5)).unwrap());
        assert_eq!(frames.frames(), &[[0.25, -0.25]]);
        let error = stream.read(Duration::from_secs(5)).unwrap_err();
        assert_eq!(error.to_string(), "cable pulled");
    }

    #[test]
    fn push_clamps_out_of_range_and_zeroes_non_finite_samples() {
        let (pushed_tx, pushed_rx) = mpsc::channel();
        let mut stream = CaptureStream::spawn("test-clamp", move |tap| {
            tap.push(&[[2.5, -3.0], [f32::NAN, f32::INFINITY]]);
            tap.push_silence(1);
            pushed_tx.send(()).unwrap();
            park_until_stop(tap);
            Ok(())
        })
        .unwrap();

        pushed_rx.recv().unwrap();
        let frames = frames_of(stream.read(Duration::ZERO).unwrap());
        assert_eq!(frames.frames(), &[[1.0, -1.0], [0.0, 0.0], [0.0, 0.0]]);
    }

    #[test]
    fn dropping_the_stream_stops_and_joins_the_producer() {
        let exited = Arc::new(AtomicBool::new(false));
        let producer_exited = Arc::clone(&exited);
        let stream = CaptureStream::spawn("test-join", move |tap| {
            park_until_stop(tap);
            producer_exited.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();

        drop(stream);
        assert!(exited.load(Ordering::SeqCst));
    }
}
