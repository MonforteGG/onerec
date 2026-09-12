use std::collections::VecDeque;
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::capture::{CaptureRead, CaptureSource};
use crate::ids::{MicrophoneId, OutputDeviceId};
use crate::mp3::Encode;
use crate::staging::StagingFile;
use crate::timeline::{draw, fold_mix, MAX_BACKLOG_FRAMES, MIX_QUANTUM_FRAMES, MIX_TICK};

pub use crate::mp3::{ExportQuality, SaveProgress};

const SAVE_SLICE: Duration = Duration::from_millis(30);
const STAGING_BUFFER: usize = 64 * 1024;

pub enum Session {
    Idle,
    Recording(ActiveRecording),
    Paused(ActiveRecording),
    AwaitingSave(PendingRecording),
    Saving(ActiveSave),
    Failed(FailedSession),
}

pub struct ActiveSave {
    worker: ExportWorker,
    destination: PathBuf,
    pending: PendingRecording,
}

// Only exists while exporting. The UI observes atomics and never encodes audio.
struct ExportWorker {
    thread: Option<JoinHandle<io::Result<()>>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    total: u64,
}

impl ExportWorker {
    fn start(mut encode: Encode) -> io::Result<Self> {
        let total = encode.progress().total;
        let done = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_done = Arc::clone(&done);
        let worker_cancel = Arc::clone(&cancel);
        let thread = thread::Builder::new()
            .name("onerec-export".into())
            .spawn(move || {
                while !worker_cancel.load(Ordering::Relaxed) {
                    let complete = encode.pump(SAVE_SLICE)?;
                    worker_done.store(encode.progress().done, Ordering::Relaxed);
                    if complete {
                        return Ok(());
                    }
                }
                Ok(()) // Dropping Encode removes any uncommitted output.
            })?;
        Ok(Self {
            thread: Some(thread),
            cancel,
            done,
            total,
        })
    }

    fn progress(&self) -> SaveProgress {
        SaveProgress {
            done: self.done.load(Ordering::Relaxed),
            total: self.total,
        }
    }

    fn poll(&mut self) -> Option<io::Result<()>> {
        if !self.thread.as_ref()?.is_finished() {
            return None;
        }
        Some(
            self.thread
                .take()
                .unwrap()
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("MP3 export worker panicked"))),
        )
    }
}

impl Drop for ExportWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct ActiveRecording {
    microphone: MicrophoneId,
    output: OutputDeviceId,
    recorded: Duration,
    segment_start: Instant,
    stop_tx: Option<Sender<()>>,
    worker: Option<JoinHandle<Result<(), FailedSession>>>,
    degraded: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    staging_file: StagingFile,
    quality: ExportQuality,
}

pub struct PendingRecording {
    staging_file: StagingFile,
    elapsed: Duration,
    degraded: bool,
    quality: ExportQuality,
}

#[derive(Debug)]
pub struct FailedSession {
    detail: String,
    staging: Option<StagingFile>,
}

#[derive(Debug)]
pub enum SaveError {
    NoTake,
    Write(String),
}

#[derive(Debug)]
pub struct DiscardError {
    path: PathBuf,
    source: io::Error,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Session::Idle => write!(f, "Idle"),
            Session::Recording(active) => debug_live(f, "Recording", active),
            Session::Paused(active) => debug_live(f, "Paused", active),
            Session::AwaitingSave(pending) => f
                .debug_struct("AwaitingSave")
                .field("elapsed", &pending.elapsed)
                .field("degraded", &pending.degraded)
                .field("quality", &pending.quality)
                .field("staging_file", &pending.staging_file)
                .finish(),
            Session::Saving(active) => f
                .debug_struct("Saving")
                .field("elapsed", &active.pending.elapsed)
                .field("degraded", &active.pending.degraded)
                .field("quality", &active.pending.quality)
                .field("staging_file", &active.pending.staging_file)
                .field("destination", &active.destination)
                .field("progress", &active.worker.progress())
                .finish(),
            Session::Failed(failed) => f.debug_tuple("Failed").field(&failed.detail).finish(),
        }
    }
}

fn debug_live(
    f: &mut std::fmt::Formatter<'_>,
    name: &str,
    active: &ActiveRecording,
) -> std::fmt::Result {
    f.debug_struct(name)
        .field("microphone", &active.microphone)
        .field("output", &active.output)
        .field("elapsed", &active.elapsed())
        .field("degraded", &active.degraded.load(Ordering::SeqCst))
        .field("quality", &active.quality)
        .field("staging_file", &active.staging_file)
        .finish_non_exhaustive()
}

impl PendingRecording {
    pub fn staging_file(&self) -> &Path {
        self.staging_file.path()
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    pub fn quality(&self) -> ExportQuality {
        self.quality
    }
}

impl FailedSession {
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for FailedSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::NoTake => f.write_str("nothing to save"),
            SaveError::Write(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for SaveError {}

impl DiscardError {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for DiscardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not delete {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for DiscardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl Session {
    pub fn start(
        &mut self,
        microphone: MicrophoneId,
        output: OutputDeviceId,
        microphone_source: impl CaptureSource,
        system: impl CaptureSource,
        staging_file: StagingFile,
        quality: ExportQuality,
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
            quality,
        ) {
            Ok(active) => Session::Recording(active),
            Err(failed) => Session::Failed(failed),
        };
    }

    pub fn stop(&mut self) {
        *self = match std::mem::replace(self, Session::Idle) {
            Session::Recording(active) | Session::Paused(active) => active.seal(),
            other => other,
        };
    }

    pub fn pause(&mut self) {
        *self = match std::mem::replace(self, Session::Idle) {
            Session::Recording(mut active) => {
                active.recorded += active.segment_start.elapsed();
                active.paused.store(true, Ordering::SeqCst);
                Session::Paused(active)
            }
            other => other,
        };
    }

    pub fn resume(&mut self) {
        *self = match std::mem::replace(self, Session::Idle) {
            Session::Paused(mut active) => {
                active.segment_start = Instant::now();
                active.paused.store(false, Ordering::SeqCst);
                Session::Recording(active)
            }
            other => other,
        };
    }

    pub fn cancel_save(&mut self) {
        let Session::AwaitingSave(_) = self else {
            return;
        };
    }

    pub fn save_as(&mut self, destination: &Path) -> Result<(), SaveError> {
        if matches!(self, Session::Saving(_)) {
            return Ok(());
        }
        let (staged, quality) = match self {
            Session::AwaitingSave(pending) => {
                (pending.staging_file().to_path_buf(), pending.quality)
            }
            _ => return Err(SaveError::NoTake),
        };
        let encode = Encode::start(destination, &staged, quality)
            .map_err(|error| SaveError::Write(error.to_string()))?;
        let worker =
            ExportWorker::start(encode).map_err(|error| SaveError::Write(error.to_string()))?;
        let Session::AwaitingSave(pending) = std::mem::replace(self, Session::Idle) else {
            unreachable!()
        };
        *self = Session::Saving(ActiveSave {
            worker,
            destination: destination.to_path_buf(),
            pending,
        });
        Ok(())
    }

    pub fn poll(&mut self) -> Option<Result<PathBuf, SaveError>> {
        if !matches!(self, Session::Saving(_)) {
            return None;
        }
        let outcome = {
            let Session::Saving(active) = self else {
                unreachable!()
            };
            active.worker.poll()
        };
        match outcome {
            None => None,
            Some(Ok(())) => {
                let Session::Saving(active) = std::mem::replace(self, Session::Idle) else {
                    unreachable!()
                };
                Some(Ok(active.destination))
            }
            Some(Err(error)) => {
                let Session::Saving(active) = std::mem::replace(self, Session::Idle) else {
                    unreachable!()
                };
                *self = Session::AwaitingSave(active.pending);
                Some(Err(SaveError::Write(error.to_string())))
            }
        }
    }

    pub fn save_progress(&self) -> Option<SaveProgress> {
        match self {
            Session::Saving(active) => Some(active.worker.progress()),
            _ => None,
        }
    }

    pub fn discard(&mut self) -> Result<(), DiscardError> {
        let pending = match std::mem::replace(self, Session::Idle) {
            Session::AwaitingSave(pending) => pending,
            Session::Saving(active) => active.pending,
            other => {
                *self = other;
                return Ok(());
            }
        };
        let mut pending = pending;
        if let Err(source) = pending.staging_file.unlink() {
            let path = pending.staging_file.path().to_path_buf();
            *self = Session::AwaitingSave(pending);
            return Err(DiscardError { path, source });
        }
        Ok(())
    }

    pub fn dismiss(&mut self) -> Result<(), DiscardError> {
        let Session::Failed(failed) = self else {
            return Ok(());
        };
        if let Some(staging) = &mut failed.staging {
            staging.unlink().map_err(|source| DiscardError {
                path: staging.path().to_path_buf(),
                source,
            })?;
        }
        *self = Session::Idle;
        Ok(())
    }

    pub fn elapsed(&self) -> Option<Duration> {
        match self {
            Session::Recording(active) | Session::Paused(active) => Some(active.elapsed()),
            Session::AwaitingSave(pending) => Some(pending.elapsed),
            Session::Saving(active) => Some(active.pending.elapsed),
            Session::Idle | Session::Failed(_) => None,
        }
    }

    pub fn is_degraded(&self) -> bool {
        match self {
            Session::Recording(active) | Session::Paused(active) => {
                active.degraded.load(Ordering::SeqCst)
            }
            Session::AwaitingSave(pending) => pending.degraded,
            Session::Saving(active) => active.pending.degraded,
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
        staging_file: StagingFile,
        quality: ExportQuality,
    ) -> Result<Self, FailedSession> {
        let file = match File::create(staging_file.path()) {
            Ok(file) => file,
            Err(error) => {
                return Err(FailedSession {
                    detail: error.to_string(),
                    staging: Some(staging_file),
                });
            }
        };
        let (stop_tx, stop_rx) = mpsc::channel();
        let degraded = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let degraded_worker = Arc::clone(&degraded);
        let paused_worker = Arc::clone(&paused);
        let segment_start = Instant::now();
        let worker = match thread::Builder::new()
            .name("onerec-mix".into())
            .spawn(move || {
                mix_loop(
                    Box::new(microphone_source),
                    Box::new(system),
                    file,
                    quality,
                    stop_rx,
                    degraded_worker,
                    paused_worker,
                )
            }) {
            Ok(worker) => worker,
            Err(error) => {
                return Err(FailedSession {
                    detail: error.to_string(),
                    staging: Some(staging_file),
                });
            }
        };
        Ok(Self {
            microphone,
            output,
            recorded: Duration::ZERO,
            segment_start,
            stop_tx: Some(stop_tx),
            worker: Some(worker),
            degraded,
            paused,
            staging_file,
            quality,
        })
    }

    fn elapsed(&self) -> Duration {
        if self.paused.load(Ordering::SeqCst) {
            self.recorded
        } else {
            self.recorded + self.segment_start.elapsed()
        }
    }

    fn seal(mut self) -> Session {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        let staging_file = self.staging_file.take();
        match self.worker.take() {
            Some(worker) => match worker.join() {
                Ok(Ok(())) => Session::AwaitingSave(PendingRecording {
                    staging_file,
                    elapsed: self.elapsed(),
                    degraded: self.degraded.load(Ordering::SeqCst),
                    quality: self.quality,
                }),
                Ok(Err(mut failed)) => {
                    failed.staging = Some(staging_file);
                    Session::Failed(failed)
                }
                Err(_) => Session::Failed(FailedSession {
                    detail: "mix thread panicked".into(),
                    staging: Some(staging_file),
                }),
            },
            None => Session::Failed(FailedSession {
                detail: "mix thread missing".into(),
                staging: Some(staging_file),
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
        staging: None,
    }
}

fn mix_loop(
    mut microphone: Box<dyn CaptureSource>,
    mut system: Box<dyn CaptureSource>,
    file: File,
    quality: ExportQuality,
    stop_rx: Receiver<()>,
    degraded: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<(), FailedSession> {
    let mut file = BufWriter::with_capacity(STAGING_BUFFER, file);
    let mut mic_buf = VecDeque::new();
    let mut sys_buf = VecDeque::new();
    let mut mic_live = true;
    let mut sys_live = true;
    let mut staged = Vec::with_capacity(MIX_QUANTUM_FRAMES * quality.staging_frame_bytes());
    let mut origin = Instant::now();
    let mut quanta = 0u32;
    let mut was_paused = false;

    loop {
        match stop_rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        pull(microphone.as_mut(), &mut mic_buf, &mut mic_live, &degraded);
        pull(system.as_mut(), &mut sys_buf, &mut sys_live, &degraded);

        if paused.load(Ordering::SeqCst) {
            let _ = take_quantum(&mut mic_buf);
            let _ = take_quantum(&mut sys_buf);
            was_paused = true;
            match stop_rx.recv_timeout(MIX_TICK) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            continue;
        }

        if was_paused {
            origin = Instant::now() - MIX_TICK * quanta;
            was_paused = false;
        }

        let mic = take_quantum(&mut mic_buf);
        let sys = take_quantum(&mut sys_buf);
        let block = mix(&mic, &sys);
        fold_mix(
            &block,
            quality.staging_factor(),
            quality.staging_channels(),
            &mut staged,
        );
        file.write_all(&staged).map_err(io_fail)?;
        quanta = quanta.saturating_add(1);

        let due = origin + MIX_TICK * quanta;
        let wait = due.saturating_duration_since(Instant::now());
        if wait.is_zero() {
            continue;
        }
        match stop_rx.recv_timeout(wait) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    file.flush().map_err(io_fail)?;
    file.get_ref().sync_all().map_err(io_fail)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{NoPacketSource, PcmSource};
    use crate::staging::StagingArea;
    use crate::timeline::MIX_SAMPLE_RATE;

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
        staging: StagingFile,
    ) {
        start_quality(session, microphone, system, staging, ExportQuality::Meeting);
    }

    fn start_quality(
        session: &mut Session,
        microphone: impl CaptureSource,
        system: impl CaptureSource,
        staging: StagingFile,
        quality: ExportQuality,
    ) {
        session.start(mic_id(), out_id(), microphone, system, staging, quality);
    }

    fn record_silence() -> (Session, PathBuf) {
        let staging = StagingArea::open().unwrap().next_take().unwrap();
        let path = staging.path().to_path_buf();
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::silence(), NoPacketSource, staging);
        (session, path)
    }

    fn staged_frame_count(path: &Path, quality: ExportQuality) -> u64 {
        std::fs::metadata(path).unwrap().len() / quality.staging_frame_bytes() as u64
    }

    fn staged_samples(path: &Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len() % 4, 0, "staging length {}", bytes.len());
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()
    }

    fn awaiting(
        staging: StagingFile,
        elapsed: Duration,
        quality: ExportQuality,
    ) -> PendingRecording {
        PendingRecording {
            staging_file: staging,
            elapsed,
            degraded: false,
            quality,
        }
    }

    #[test]
    fn pending_system_elapsed_follows_wall_clock() {
        let staging = StagingArea::open().unwrap().next_take().unwrap();
        let mut session = Session::Idle;
        let wall_origin = Instant::now();
        start_with(&mut session, PcmSource::silence(), NoPacketSource, staging);
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
    fn mixed_pcm_duration_tracks_elapsed() {
        let (mut session, _path) = record_silence();
        thread::sleep(Duration::from_millis(200));
        session.stop();
        let Session::AwaitingSave(pending) = &session else {
            panic!("expected AwaitingSave, got {session:?}");
        };
        let mixed = Duration::from_secs_f64(
            staged_frame_count(pending.staging_file(), pending.quality()) as f64
                / f64::from(pending.quality().staging_hz()),
        );
        let elapsed = pending.elapsed();
        assert!(
            mixed + MIX_TICK * 3 >= elapsed,
            "mixed {mixed:?} is shorter than elapsed {elapsed:?}"
        );
        assert!(
            mixed <= elapsed + MIX_TICK,
            "mixed {mixed:?} ran past elapsed {elapsed:?}"
        );
    }

    #[test]
    fn cancel_save_keeps_awaiting_save_and_staging_file() {
        let staging = StagingArea::open().unwrap().next_take().unwrap();
        let path = staging.path().to_path_buf();
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::tone(0.25), NoPacketSource, staging);
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
        let area = StagingArea::open().unwrap();
        let staging = area.next_take().unwrap();
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::tone(0.25), NoPacketSource, staging);
        thread::sleep(Duration::from_millis(40));
        let elapsed = session.elapsed().expect("recording elapsed");
        start_with(
            &mut session,
            PcmSource::silence(),
            NoPacketSource,
            area.next_take().unwrap(),
        );
        assert!(matches!(session, Session::Recording(_)));
        assert!(session.elapsed().unwrap() >= elapsed);
        session.stop();
        let Session::AwaitingSave(pending) = &session else {
            panic!("expected AwaitingSave, got {session:?}");
        };
        let pcm = staged_samples(pending.staging_file());
        assert!(
            pcm.iter().any(|sample| *sample != 0.0),
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
    fn pause_then_resume_does_not_grow_staging() {
        let staging = StagingArea::open().unwrap().next_take().unwrap();
        let path = staging.path().to_path_buf();
        let mut session = Session::Idle;
        start_quality(
            &mut session,
            PcmSource::silence(),
            NoPacketSource,
            staging,
            ExportQuality::High,
        );
        thread::sleep(Duration::from_millis(300));
        session.pause();
        assert!(matches!(session, Session::Paused(_)));
        thread::sleep(MIX_TICK * 3);
        let paused_at = std::fs::metadata(&path).unwrap().len();
        assert!(paused_at > 0, "need bytes on disk before pause");
        thread::sleep(Duration::from_millis(500));
        let still = std::fs::metadata(&path).unwrap().len();
        assert_eq!(still, paused_at, "paused mix wrote {still} after {paused_at}");
        session.resume();
        assert!(matches!(session, Session::Recording(_)));
        session.stop();
    }

    #[test]
    fn stop_from_paused_seals_the_take() {
        let (mut session, path) = record_silence();
        thread::sleep(Duration::from_millis(40));
        session.pause();
        assert!(matches!(session, Session::Paused(_)));
        session.stop();
        let Session::AwaitingSave(pending) = &session else {
            panic!("expected AwaitingSave, got {session:?}");
        };
        assert_eq!(pending.staging_file(), path.as_path());
        assert!(path.exists());
    }

    #[test]
    fn pause_then_resume_pcm_duration_still_tracks_elapsed() {
        let staging = StagingArea::open().unwrap().next_take().unwrap();
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::silence(), NoPacketSource, staging);
        thread::sleep(Duration::from_millis(150));
        session.pause();
        let frozen = session.elapsed().expect("paused elapsed");
        thread::sleep(Duration::from_millis(200));
        assert_eq!(session.elapsed(), Some(frozen));
        session.resume();
        thread::sleep(Duration::from_millis(150));
        session.stop();
        let Session::AwaitingSave(pending) = &session else {
            panic!("expected AwaitingSave, got {session:?}");
        };
        let elapsed = pending.elapsed();
        assert!(
            elapsed >= Duration::from_millis(200),
            "elapsed {elapsed:?} dropped the live segments"
        );
        assert!(
            elapsed < frozen + Duration::from_millis(280),
            "elapsed {elapsed:?} kept paused time after freeze {frozen:?}"
        );
        let mixed = Duration::from_secs_f64(
            staged_frame_count(pending.staging_file(), pending.quality()) as f64
                / f64::from(pending.quality().staging_hz()),
        );
        assert!(
            mixed + MIX_TICK * 3 >= elapsed,
            "mixed {mixed:?} is shorter than elapsed {elapsed:?}"
        );
        assert!(
            mixed <= elapsed + MIX_TICK,
            "mixed {mixed:?} ran past elapsed {elapsed:?}"
        );
    }

    #[test]
    fn unplug_of_one_source_keeps_the_session_and_other_audio() {
        let staging = StagingArea::open().unwrap().next_take().unwrap();
        let plugged = Arc::new(AtomicBool::new(true));
        let system = PcmSource::with_plug([0.0, 0.0], Arc::clone(&plugged));
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::tone(0.5), system, staging);
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
        let pcm = staged_samples(pending.staging_file());
        assert!(
            pcm.iter().any(|sample| (*sample - 0.5).abs() < 1e-6),
            "microphone frames must remain after the system device is lost"
        );
    }

    #[test]
    fn discard_deletes_the_file_and_returns_to_idle() {
        let (mut session, path) = record_silence();
        thread::sleep(Duration::from_millis(30));
        session.stop();
        assert!(path.exists());
        session.discard().unwrap();
        assert!(matches!(session, Session::Idle));
        assert!(!path.exists());
    }

    #[test]
    fn discard_failure_keeps_awaiting_save() {
        let staging = StagingFile::reserved(PathBuf::from("onerec-missing-take.f32"));
        let mut session =
            Session::AwaitingSave(awaiting(staging, Duration::ZERO, ExportQuality::Meeting));
        session.discard().unwrap_err();
        assert!(matches!(session, Session::AwaitingSave(_)));
        let Session::AwaitingSave(pending) = &session else {
            unreachable!();
        };
        assert_eq!(pending.staging_file(), Path::new("onerec-missing-take.f32"));
    }

    fn poll_until_terminal(session: &mut Session) -> Result<PathBuf, SaveError> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "export timed out");
            match session.poll() {
                None => {
                    assert!(
                        matches!(session, Session::Saving(_)),
                        "save left Saving without a terminal poll"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
                Some(result) => return result,
            }
        }
    }

    fn stage_silence(path: &Path, frames: usize, quality: ExportQuality) {
        std::fs::write(path, vec![0u8; frames * quality.staging_frame_bytes()]).unwrap();
    }

    fn leftovers(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("onerec-part"))
            .count()
    }

    #[test]
    fn save_as_writes_mp3_then_goes_idle() {
        let staging = StagingArea::open().unwrap().next_take().unwrap();
        let path = staging.path().to_path_buf();
        let mut session = Session::Idle;
        start_quality(
            &mut session,
            PcmSource::silence(),
            NoPacketSource,
            staging,
            ExportQuality::Standard,
        );
        thread::sleep(Duration::from_millis(40));
        session.stop();
        assert!(path.exists());
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = dest_dir.path().join("take.mp3");
        session.save_as(&dest).unwrap();
        assert!(matches!(session, Session::Saving(_)));
        assert_eq!(poll_until_terminal(&mut session).unwrap(), dest);
        assert!(matches!(session, Session::Idle));
        assert!(!path.exists());
        let mp3 = std::fs::read(&dest).unwrap();
        assert_eq!(mp3[0], 0xFF);
        assert_eq!(mp3[1] & 0xE0, 0xE0);
    }

    #[test]
    fn save_as_then_poll_reports_monotonic_progress_until_idle() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take.mp3");
        let frames = MIX_SAMPLE_RATE as usize * 10;
        stage_silence(&staged, frames, ExportQuality::Compact);
        let mut session = Session::AwaitingSave(awaiting(
            StagingFile::reserved(staged.clone()),
            Duration::from_secs(10),
            ExportQuality::Compact,
        ));
        session.save_as(&dest).unwrap();
        assert!(matches!(session, Session::Saving(_)));
        let start = session.save_progress().unwrap();
        assert!(start.done <= start.total);
        assert_eq!(start.total, frames as u64);

        let mut previous = start.done;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "export timed out");
            match session.poll() {
                None => {
                    let progress = session.save_progress().expect("still saving");
                    assert!(progress.done >= previous && progress.done <= progress.total);
                    previous = progress.done;
                    thread::sleep(Duration::from_millis(1));
                }
                Some(Ok(_)) => break,
                Some(Err(error)) => panic!("{error}"),
            }
        }
        assert!(matches!(session, Session::Idle));
        assert!(!staged.exists());
        assert!(dest.exists());
        let mp3 = std::fs::read(&dest).unwrap();
        let mpeg = crate::mp3::parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert!(mpeg.iter().all(|frame| frame.bitrate_kbps == 128));
        assert_eq!(leftovers(dir.path()), 0);
    }

    #[test]
    fn export_finishes_without_ui_polling() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take.mp3");
        stage_silence(
            &staged,
            MIX_SAMPLE_RATE as usize * 2,
            ExportQuality::Standard,
        );
        let mut session = Session::AwaitingSave(awaiting(
            StagingFile::reserved(staged.clone()),
            Duration::from_secs(2),
            ExportQuality::Standard,
        ));
        session.save_as(&dest).unwrap();
        let Session::Saving(active) = &session else {
            panic!("expected saving");
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !active.worker.thread.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline, "worker requires UI polling");
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            staged.exists(),
            "staging remains owned until completion is consumed"
        );
        assert!(
            dest.exists(),
            "worker commits the file independently of the UI"
        );
        assert_eq!(session.poll().unwrap().unwrap(), dest);
        assert!(!staged.exists());
        assert_eq!(leftovers(dir.path()), 0);
    }

    #[test]
    fn discard_during_export_releases_files_and_removes_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take.mp3");
        File::create(&staged)
            .unwrap()
            .set_len(MIX_SAMPLE_RATE as u64 * 60 * ExportQuality::High.staging_frame_bytes() as u64)
            .unwrap();
        let mut session = Session::AwaitingSave(awaiting(
            StagingFile::reserved(staged.clone()),
            Duration::from_secs(60),
            ExportQuality::High,
        ));
        session.save_as(&dest).unwrap();
        session.discard().unwrap();
        assert!(matches!(session, Session::Idle));
        assert!(!staged.exists());
        assert_eq!(leftovers(dir.path()), 0);
        // A worker that won the completion race may have committed a complete
        // MP3. Cancellation must never leave an incomplete destination.
        if dest.exists() {
            let bytes = std::fs::read(&dest).unwrap();
            let frames = crate::mp3::parse_mpeg1_layer3_cbr(&bytes).unwrap();
            assert!(frames.len() > 2_000);
        }
    }

    #[test]
    fn save_as_unaligned_staging_stays_awaiting_save() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        std::fs::write(&staged, [0u8; 7]).unwrap();
        let mut session = Session::AwaitingSave(awaiting(
            StagingFile::reserved(staged.clone()),
            Duration::ZERO,
            ExportQuality::Standard,
        ));
        let dest = dir.path().join("take.mp3");
        session.save_as(&dest).unwrap_err();
        assert!(matches!(session, Session::AwaitingSave(_)));
        assert!(staged.exists());
        assert!(!dest.exists());
    }

    #[test]
    fn save_as_write_failure_stays_awaiting_save() {
        let (mut session, path) = record_silence();
        thread::sleep(Duration::from_millis(30));
        session.stop();
        let dest = tempfile::tempdir().unwrap();
        session.save_as(dest.path()).unwrap();
        assert!(matches!(session, Session::Saving(_)));
        poll_until_terminal(&mut session).unwrap_err();
        assert!(matches!(session, Session::AwaitingSave(_)));
        assert!(path.exists());
        assert_eq!(leftovers(dest.path()), 0);
    }

    #[test]
    fn discard_never_constructs_encode() {
        let (mut session, path) = record_silence();
        thread::sleep(Duration::from_millis(30));
        session.stop();
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = dest_dir.path().join("take.mp3");
        session.discard().unwrap();
        assert!(matches!(session, Session::Idle));
        assert!(!path.exists());
        assert!(!dest.exists());
        assert_eq!(leftovers(dest_dir.path()), 0);
    }

    #[test]
    fn dropping_recording_unlinks_staging() {
        let (session, path) = record_silence();
        assert!(matches!(session, Session::Recording(_)));
        assert!(path.exists());
        drop(session);
        assert!(!path.exists());
    }

    #[test]
    fn dropping_awaiting_save_unlinks_staging() {
        let (mut session, path) = record_silence();
        thread::sleep(Duration::from_millis(30));
        session.stop();
        assert!(matches!(session, Session::AwaitingSave(_)));
        assert!(path.exists());
        drop(session);
        assert!(!path.exists());
    }

    #[test]
    fn meeting_staging_is_much_smaller_than_high() {
        let area = StagingArea::open().unwrap();
        let meeting_file = area.next_take().unwrap();
        let high_file = area.next_take().unwrap();
        let meeting_path = meeting_file.path().to_path_buf();
        let high_path = high_file.path().to_path_buf();
        let mut meeting = Session::Idle;
        let mut high = Session::Idle;
        start_quality(
            &mut meeting,
            PcmSource::silence(),
            NoPacketSource,
            meeting_file,
            ExportQuality::Meeting,
        );
        start_quality(
            &mut high,
            PcmSource::silence(),
            NoPacketSource,
            high_file,
            ExportQuality::High,
        );
        thread::sleep(Duration::from_millis(200));
        meeting.stop();
        high.stop();
        let meeting_bytes = std::fs::metadata(&meeting_path).unwrap().len();
        let high_bytes = std::fs::metadata(&high_path).unwrap().len();
        assert!(
            meeting_bytes * 8 < high_bytes,
            "meeting {meeting_bytes} B was not far smaller than high {high_bytes} B"
        );
    }

    #[test]
    fn failed_session_detail_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let staging = StagingFile::reserved(dir.path().to_path_buf());
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::silence(), NoPacketSource, staging);
        let Session::Failed(failed) = &session else {
            panic!("expected Failed, got {session:?}");
        };
        assert!(!failed.detail().is_empty());
        assert_eq!(failed.to_string(), failed.detail());
    }
}
