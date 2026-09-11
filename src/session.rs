use std::collections::VecDeque;
use std::fmt;
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
use crate::mp3::Encode;
use crate::staging::StagingFile;
use crate::timeline::{draw, MAX_BACKLOG_FRAMES, MIX_QUANTUM_FRAMES, MIX_TICK};

pub use crate::mp3::{ExportQuality, SaveProgress};

const SAVE_SLICE: Duration = Duration::from_millis(30);

pub enum Session {
    Idle,
    Recording(ActiveRecording),
    AwaitingSave(PendingRecording),
    Saving(ActiveSave),
    Failed(FailedSession),
}

pub struct ActiveSave {
    encode: Encode,
    destination: PathBuf,
    pending: PendingRecording,
}

pub struct ActiveRecording {
    microphone: MicrophoneId,
    output: OutputDeviceId,
    started_at: Instant,
    stop_tx: Option<Sender<()>>,
    worker: Option<JoinHandle<Result<(), FailedSession>>>,
    degraded: Arc<AtomicBool>,
    staging_file: StagingFile,
}

pub struct PendingRecording {
    staging_file: StagingFile,
    elapsed: Duration,
    degraded: bool,
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
            Session::Saving(active) => f
                .debug_struct("Saving")
                .field("elapsed", &active.pending.elapsed)
                .field("degraded", &active.pending.degraded)
                .field("staging_file", &active.pending.staging_file)
                .field("destination", &active.destination)
                .field("progress", &active.encode.progress())
                .finish(),
            Session::Failed(failed) => f.debug_tuple("Failed").field(&failed.detail).finish(),
        }
    }
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

    pub fn save_as(&mut self, destination: &Path, quality: ExportQuality) -> Result<(), SaveError> {
        if matches!(self, Session::Saving(_)) {
            return Ok(());
        }
        let staged = match self {
            Session::AwaitingSave(pending) => pending.staging_file().to_path_buf(),
            _ => return Err(SaveError::NoTake),
        };
        let encode = Encode::start(destination, &staged, quality)
            .map_err(|error| SaveError::Write(error.to_string()))?;
        let Session::AwaitingSave(pending) = std::mem::replace(self, Session::Idle) else {
            unreachable!()
        };
        *self = Session::Saving(ActiveSave {
            encode,
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
            active.encode.pump(SAVE_SLICE)
        };
        match outcome {
            Ok(false) => None,
            Ok(true) => {
                let Session::Saving(active) = std::mem::replace(self, Session::Idle) else {
                    unreachable!()
                };
                Some(Ok(active.destination))
            }
            Err(error) => {
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
            Session::Saving(active) => Some(active.encode.progress()),
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
            Session::Recording(active) => Some(active.started_at.elapsed()),
            Session::AwaitingSave(pending) => Some(pending.elapsed),
            Session::Saving(active) => Some(active.pending.elapsed),
            Session::Idle | Session::Failed(_) => None,
        }
    }

    pub fn is_degraded(&self) -> bool {
        match self {
            Session::Recording(active) => active.degraded.load(Ordering::SeqCst),
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
        let degraded_worker = Arc::clone(&degraded);
        let started_at = Instant::now();
        let worker = match thread::Builder::new()
            .name("onerec-mix".into())
            .spawn(move || {
                mix_loop(
                    Box::new(microphone_source),
                    Box::new(system),
                    file,
                    stop_rx,
                    degraded_worker,
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
        let staging_file = self.staging_file.take();
        match self.worker.take() {
            Some(worker) => match worker.join() {
                Ok(Ok(())) => Session::AwaitingSave(PendingRecording {
                    staging_file,
                    elapsed: self.started_at.elapsed(),
                    degraded: self.degraded.load(Ordering::SeqCst),
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
    mut file: File,
    stop_rx: Receiver<()>,
    degraded: Arc<AtomicBool>,
) -> Result<(), FailedSession> {
    let mut mic_buf = VecDeque::new();
    let mut sys_buf = VecDeque::new();
    let mut mic_live = true;
    let mut sys_live = true;
    let origin = Instant::now();
    let mut quanta = 0u32;

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

    file.sync_all().map_err(io_fail)?;
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
        session.start(mic_id(), out_id(), microphone, system, staging);
    }

    fn record_silence() -> (Session, PathBuf) {
        let staging = StagingArea::open().unwrap().next_take().unwrap();
        let path = staging.path().to_path_buf();
        let mut session = Session::Idle;
        start_with(&mut session, PcmSource::silence(), NoPacketSource, staging);
        (session, path)
    }

    fn staged_frame_count(path: &Path) -> u64 {
        std::fs::metadata(path).unwrap().len() / 8
    }

    fn staged_interleaved_f32(path: &Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len() % 8, 0, "staging length {}", bytes.len());
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()
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
            staged_frame_count(pending.staging_file()) as f64 / f64::from(MIX_SAMPLE_RATE),
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
        let pcm = staged_interleaved_f32(pending.staging_file());
        assert!(
            pcm.chunks_exact(2)
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
        let pcm = staged_interleaved_f32(pending.staging_file());
        assert!(
            pcm.chunks_exact(2)
                .any(|frame| (frame[0] - 0.5).abs() < 1e-6 && (frame[1] - 0.5).abs() < 1e-6),
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
        let mut session = Session::AwaitingSave(PendingRecording {
            staging_file: staging,
            elapsed: Duration::ZERO,
            degraded: false,
        });
        session.discard().unwrap_err();
        assert!(matches!(session, Session::AwaitingSave(_)));
        let Session::AwaitingSave(pending) = &session else {
            unreachable!();
        };
        assert_eq!(pending.staging_file(), Path::new("onerec-missing-take.f32"));
    }

    fn poll_until_terminal(session: &mut Session) -> Result<PathBuf, SaveError> {
        loop {
            match session.poll() {
                None => {
                    assert!(
                        matches!(session, Session::Saving(_)),
                        "save left Saving without a terminal poll"
                    );
                }
                Some(result) => return result,
            }
        }
    }

    fn stage_silence(path: &Path, frames: usize) {
        std::fs::write(path, vec![0u8; frames * 8]).unwrap();
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
        let (mut session, path) = record_silence();
        thread::sleep(Duration::from_millis(40));
        session.stop();
        assert!(path.exists());
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = dest_dir.path().join("take.mp3");
        session.save_as(&dest, ExportQuality::Standard).unwrap();
        assert!(matches!(session, Session::Saving(_)));
        assert_eq!(poll_until_terminal(&mut session).unwrap(), dest);
        assert!(matches!(session, Session::Idle));
        assert!(!path.exists());
        let mp3 = std::fs::read(&dest).unwrap();
        assert_eq!(mp3[0], 0xFF);
        assert_eq!(mp3[1] & 0xE0, 0xE0);
    }

    #[test]
    fn save_as_then_poll_sees_progress_before_idle() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        let dest = dir.path().join("take.mp3");
        let frames = MIX_SAMPLE_RATE as usize * 10;
        stage_silence(&staged, frames);
        let mut session = Session::AwaitingSave(PendingRecording {
            staging_file: StagingFile::reserved(staged.clone()),
            elapsed: Duration::from_secs(10),
            degraded: false,
        });
        session.save_as(&dest, ExportQuality::Compact).unwrap();
        assert!(matches!(session, Session::Saving(_)));
        let start = session.save_progress().unwrap();
        assert_eq!(start.done, 0);
        assert_eq!(start.total, frames as u64);

        let mut saw_partial = false;
        loop {
            match session.poll() {
                None => {
                    let progress = session.save_progress().expect("still saving");
                    if progress.done > 0 && progress.done < progress.total {
                        saw_partial = true;
                    }
                }
                Some(Ok(_)) => break,
                Some(Err(error)) => panic!("{error}"),
            }
        }
        assert!(saw_partial, "done never rose below total before Idle");
        assert!(matches!(session, Session::Idle));
        assert!(!staged.exists());
        assert!(dest.exists());
        let mp3 = std::fs::read(&dest).unwrap();
        let mpeg = crate::mp3::parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert!(mpeg.iter().all(|frame| frame.bitrate_kbps == 128));
        assert_eq!(leftovers(dir.path()), 0);
    }

    #[test]
    fn save_as_unaligned_staging_stays_awaiting_save() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("take.f32");
        std::fs::write(&staged, [0u8; 7]).unwrap();
        let mut session = Session::AwaitingSave(PendingRecording {
            staging_file: StagingFile::reserved(staged.clone()),
            elapsed: Duration::ZERO,
            degraded: false,
        });
        let dest = dir.path().join("take.mp3");
        session.save_as(&dest, ExportQuality::Standard).unwrap_err();
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
        session
            .save_as(dest.path(), ExportQuality::Standard)
            .unwrap();
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
