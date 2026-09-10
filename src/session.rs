use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::capture::{CaptureRead, CaptureSource};
use crate::ids::{MicrophoneId, OutputDeviceId};
use crate::timeline::{draw, MAX_BACKLOG_FRAMES, MIX_QUANTUM_FRAMES, MIX_TICK};

pub enum Session {
    Idle,
    Recording(ActiveRecording),
    AwaitingSave(PendingRecording),
    Failed(FailedSession),
}

pub struct ActiveRecording {
    microphone: MicrophoneId,
    output: OutputDeviceId,
    started_at: Instant,
    stop_tx: Option<Sender<()>>,
    worker: Option<JoinHandle<Result<Vec<[f32; 2]>, FailedSession>>>,
    degraded: Arc<AtomicBool>,
    staging_file: PathBuf,
}

pub struct PendingRecording {
    staging_file: PathBuf,
    elapsed: Duration,
    mixed: Vec<[f32; 2]>,
    degraded: bool,
}

#[derive(Debug)]
pub struct FailedSession {
    detail: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Session::Idle => write!(f, "Idle"),
            Session::Recording(active) => f
                .debug_struct("Recording")
                .field("microphone", &active.microphone)
                .field("output", &active.output)
                .field("elapsed", &active.started_at.elapsed())
                .field("degraded", &active.degraded.load(Ordering::SeqCst))
                .field("staging_file", &active.staging_file)
                .finish_non_exhaustive(),
            Session::AwaitingSave(pending) => f
                .debug_struct("AwaitingSave")
                .field("elapsed", &pending.elapsed)
                .field("degraded", &pending.degraded)
                .field("staging_file", &pending.staging_file)
                .finish(),
            Session::Failed(failed) => f.debug_tuple("Failed").field(&failed.detail).finish(),
        }
    }
}

impl PendingRecording {
    pub fn staging_file(&self) -> &Path {
        &self.staging_file
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn mixed_frames(&self) -> &[[f32; 2]] {
        &self.mixed
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded
    }
}

impl Session {
    pub fn start(
        &mut self,
        microphone: MicrophoneId,
        output: OutputDeviceId,
        microphone_source: impl CaptureSource,
        system: impl CaptureSource,
        staging_file: PathBuf,
    ) {
        if !matches!(self, Session::Idle) {
            return;
        }
        *self = match ActiveRecording::spawn(
            microphone,
            output,
            microphone_source,
            system,
            staging_file,
        ) {
            Ok(active) => Session::Recording(active),
            Err(failed) => Session::Failed(failed),
        };
    }

    pub fn stop(&mut self) {
        if !matches!(self, Session::Recording(_)) {
            return;
        }
        let Session::Recording(active) = std::mem::replace(self, Session::Idle) else {
            return;
        };
        *self = active.seal();
    }

    pub fn cancel_save(&mut self) {
        let Session::AwaitingSave(_) = self else {
            return;
        };
    }

    pub fn elapsed(&self) -> Option<Duration> {
        match self {
            Session::Recording(active) => Some(active.started_at.elapsed()),
            Session::AwaitingSave(pending) => Some(pending.elapsed),
            Session::Idle | Session::Failed(_) => None,
        }
    }

    pub fn is_degraded(&self) -> bool {
        match self {
            Session::Recording(active) => active.degraded.load(Ordering::SeqCst),
            Session::AwaitingSave(pending) => pending.degraded,
            Session::Idle | Session::Failed(_) => false,
        }
    }
}

impl ActiveRecording {
    fn spawn(
        microphone: MicrophoneId,
        output: OutputDeviceId,
        microphone_source: impl CaptureSource,
        system: impl CaptureSource,
        staging_file: PathBuf,
    ) -> Result<Self, FailedSession> {
        let file = File::create(&staging_file).map_err(io_fail)?;
        let (stop_tx, stop_rx) = mpsc::channel();
        let degraded = Arc::new(AtomicBool::new(false));
        let degraded_worker = Arc::clone(&degraded);
        let started_at = Instant::now();
        let worker = thread::Builder::new()
            .name("onerec-mix".into())
            .spawn(move || {
                mix_loop(
                    Box::new(microphone_source),
                    Box::new(system),
                    file,
                    stop_rx,
                    degraded_worker,
                )
            })
            .map_err(|error| FailedSession {
                detail: error.to_string(),
            })?;
        Ok(Self {
            microphone,
            output,
            started_at,
            stop_tx: Some(stop_tx),
            worker: Some(worker),
            degraded,
            staging_file,
        })
    }

    fn seal(mut self) -> Session {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        match self.worker.take() {
            Some(worker) => match worker.join() {
                Ok(Ok(mixed)) => Session::AwaitingSave(PendingRecording {
                    staging_file: self.staging_file.clone(),
                    elapsed: self.started_at.elapsed(),
                    mixed,
                    degraded: self.degraded.load(Ordering::SeqCst),
                }),
                Ok(Err(failed)) => Session::Failed(failed),
                Err(_) => Session::Failed(FailedSession {
                    detail: "mix thread panicked".into(),
                }),
            },
            None => Session::Failed(FailedSession {
                detail: "mix thread missing".into(),
            }),
        }
    }
}

impl Drop for ActiveRecording {
    fn drop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn io_fail(error: io::Error) -> FailedSession {
    FailedSession {
        detail: error.to_string(),
    }
}

fn mix_loop(
    mut microphone: Box<dyn CaptureSource>,
    mut system: Box<dyn CaptureSource>,
    mut file: File,
    stop_rx: Receiver<()>,
    degraded: Arc<AtomicBool>,
) -> Result<Vec<[f32; 2]>, FailedSession> {
    let mut mic_buf = VecDeque::new();
    let mut sys_buf = VecDeque::new();
    let mut mic_live = true;
    let mut sys_live = true;
    let mut mixed = Vec::new();

    loop {
        match stop_rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        pull(microphone.as_mut(), &mut mic_buf, &mut mic_live, &degraded);
        pull(system.as_mut(), &mut sys_buf, &mut sys_live, &degraded);

        let mic = take_quantum(&mut mic_buf);
        let sys = take_quantum(&mut sys_buf);
        let block = mix(&mic, &sys);
        write_block(&mut file, &block)?;
        mixed.extend_from_slice(&block);

        match stop_rx.recv_timeout(MIX_TICK) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    file.sync_all().map_err(io_fail)?;
    Ok(mixed)
}

fn pull(
    source: &mut dyn CaptureSource,
    buf: &mut VecDeque<[f32; 2]>,
    live: &mut bool,
    degraded: &AtomicBool,
) {
    if !*live {
        return;
    }
    match source.read(Duration::ZERO) {
        Ok(CaptureRead::Frames(frames)) => {
            buf.extend(frames.frames().iter().copied());
        }
        Ok(CaptureRead::NoPacket) => {}
        Err(_) => {
            *live = false;
            degraded.store(true, Ordering::SeqCst);
        }
    }
}

fn take_quantum(buf: &mut VecDeque<[f32; 2]>) -> Vec<[f32; 2]> {
    let drawn = draw(buf.len(), MIX_QUANTUM_FRAMES, MAX_BACKLOG_FRAMES);
    let discard = drawn.discard.min(buf.len());
    buf.drain(..discard);
    let mut out = Vec::with_capacity(MIX_QUANTUM_FRAMES);
    for _ in 0..drawn.take {
        out.push(buf.pop_front().unwrap_or([0.0, 0.0]));
    }
    out.resize(drawn.take + drawn.pad, [0.0, 0.0]);
    out
}

fn mix(mic: &[[f32; 2]], sys: &[[f32; 2]]) -> Vec<[f32; 2]> {
    mic.iter()
        .zip(sys)
        .map(|([ml, mr], [sl, sr])| [clamp(*ml + *sl), clamp(*mr + *sr)])
        .collect()
}

fn clamp(sample: f32) -> f32 {
    sample.clamp(-1.0, 1.0)
}

fn write_block(file: &mut File, block: &[[f32; 2]]) -> Result<(), FailedSession> {
    for [left, right] in block {
        file.write_all(&left.to_le_bytes()).map_err(io_fail)?;
        file.write_all(&right.to_le_bytes()).map_err(io_fail)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{NoPacketSource, PcmSource};

    fn mic_id() -> MicrophoneId {
        MicrophoneId::parse("mic".into()).unwrap()
    }

    fn out_id() -> OutputDeviceId {
        OutputDeviceId::parse("out".into()).unwrap()
    }

    fn start_with(
        session: &mut Session,
        microphone: impl CaptureSource,
        system: impl CaptureSource,
        path: PathBuf,
    ) {
        session.start(mic_id(), out_id(), microphone, system, path);
    }

    #[test]
    fn pending_system_elapsed_follows_wall_clock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.part");
        let mut session = Session::Idle;
        let wall_origin = Instant::now();
        start_with(&mut session, PcmSource::silence(), NoPacketSource, path);
        assert!(matches!(session, Session::Recording(_)));
        thread::sleep(Duration::from_millis(120));
        session.stop();
        let wall = wall_origin.elapsed();
        let Session::AwaitingSave(pending) = &session else {
            panic!("expected AwaitingSave, got {session:?}");
        };
        let elapsed = pending.elapsed();
        assert!(
            elapsed >= Duration::from_millis(100),
            "elapsed {elapsed:?} came from packet count or the clock stalled"
        );
        assert!(
            elapsed <= wall + MIX_TICK,
            "elapsed {elapsed:?} drifted from wall {wall:?}"
        );
    }

    #[test]
    fn cancel_save_keeps_awaiting_save_and_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.part");
        let mut session = Session::Idle;
        start_with(
            &mut session,
            PcmSource::tone(0.25),
            NoPacketSource,
            path.clone(),
        );
        thread::sleep(Duration::from_millis(30));
        session.stop();
        let Session::AwaitingSave(pending) = &session else {
            panic!("expected AwaitingSave, got {session:?}");
        };
        assert_eq!(pending.staging_file(), path.as_path());
        assert!(path.exists());
        session.cancel_save();
        assert!(matches!(session, Session::AwaitingSave(_)));
        assert!(path.exists());
    }

    #[test]
    fn start_while_recording_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.part");
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::tone(0.25), NoPacketSource, path);
        thread::sleep(Duration::from_millis(40));
        let elapsed = session.elapsed().expect("recording elapsed");
        start_with(
            &mut session,
            PcmSource::silence(),
            NoPacketSource,
            dir.path().join("other.part"),
        );
        assert!(matches!(session, Session::Recording(_)));
        assert!(session.elapsed().unwrap() >= elapsed);
        session.stop();
        let Session::AwaitingSave(pending) = &session else {
            panic!("expected AwaitingSave, got {session:?}");
        };
        assert!(
            pending
                .mixed_frames()
                .iter()
                .any(|frame| frame[0] != 0.0 || frame[1] != 0.0),
            "second start must not replace the live tone source"
        );
    }

    #[test]
    fn stop_while_idle_is_a_noop() {
        let mut session = Session::Idle;
        session.stop();
        assert!(matches!(session, Session::Idle));
    }

    #[test]
    fn unplug_of_one_source_keeps_the_session_and_other_audio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.part");
        let plugged = Arc::new(AtomicBool::new(true));
        let system = PcmSource::with_plug([0.0, 0.0], Arc::clone(&plugged));
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::tone(0.5), system, path);
        thread::sleep(Duration::from_millis(40));
        plugged.store(false, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(40));
        assert!(matches!(session, Session::Recording(_)));
        assert!(session.is_degraded());
        session.stop();
        let Session::AwaitingSave(pending) = &session else {
            panic!("expected AwaitingSave, got {session:?}");
        };
        assert!(pending.is_degraded());
        assert!(
            pending
                .mixed_frames()
                .iter()
                .any(|frame| (frame[0] - 0.5).abs() < 1e-6 && (frame[1] - 0.5).abs() < 1e-6),
            "microphone frames must remain after the system device is lost"
        );
    }
}
