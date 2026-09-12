mod level;
#[cfg(windows)]
mod wasapi;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::audio::{AudioError, Endpoint, Endpoints};
use crate::capture::CaptureSource;
use crate::ids::{MicrophoneId, OutputDeviceId};
use crate::mp3::{ExportQuality, SaveProgress};
#[cfg(windows)]
use crate::prefs::prefs_path;
use crate::prefs::{dated_file_name, unique_mp3_path, CivilTime, Prefs};
use crate::session::{SaveError, Session};
use crate::staging::StagingArea;

pub(crate) use self::level::Level;
use self::level::Vu;

pub(crate) trait Devices: 'static {
    fn survey(&self) -> Result<Endpoints, AudioError>;
    fn open_microphone(&self, id: &MicrophoneId) -> Result<Box<dyn CaptureSource>, AudioError>;
    fn open_loopback(&self, id: &OutputDeviceId) -> Result<Box<dyn CaptureSource>, AudioError>;
}

pub(crate) enum Intent {
    Tick,
    RefreshEndpoints,
    ChooseMicrophone(usize),
    ChooseOutput(usize),
    ChooseQuality(usize),
    Toggle,
    Start,
    Stop,
    Pause,
    Save,
    SaveAs,
    SaveTo(PathBuf),
    CancelSave,
    RequestDiscard,
    Discard,
    OpenSettings,
    SetSaveDirect(bool),
    Closing,
    DiscardAndClose,
    HotkeyUnavailable,
}

#[derive(Debug)]
pub(crate) struct View {
    pub phase: Phase,
    pub endpoints: Option<EndpointLists>,
    pub microphone: Selector,
    pub output: Selector,
    pub quality: Selector,
    pub transport: Transport,
    pub elapsed: String,
    pub levels: Levels,
    pub progress: Option<SaveProgress>,
    pub status: Status,
    pub ask: Option<Ask>,
    pub saved_path: Option<PathBuf>,
    pub save_direct: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Recording,
    Paused,
    AwaitingSave,
    Saving,
    Failed,
}

impl Phase {
    pub(crate) fn timer_ms(self, minimized: bool) -> Option<u32> {
        match self {
            Self::Recording => Some(if minimized { 500 } else { 50 }),
            Self::Saving => Some(50),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EndpointLists {
    pub microphones: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selector {
    pub selected: Option<usize>,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Transport {
    pub toggle_label: &'static str,
    pub toggle_enabled: bool,
    pub save_enabled: bool,
    pub discard_enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Levels {
    pub microphone: Level,
    pub system: Level,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub text: String,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Neutral,
    Recording,
    Warning,
    Failure,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ask {
    SaveDestination(SavePrompt),
    ConfirmClose,
    ConfirmDiscard,
    Close,
    Settings { save_direct: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavePrompt {
    pub file_name: String,
    pub folder: Option<PathBuf>,
    pub filter_label: &'static str,
    pub extension: &'static str,
}

pub(crate) struct Recorder {
    devices: Box<dyn Devices>,
    staging: StagingArea,
    endpoints: Endpoints,
    endpoints_dirty: bool,
    microphone: Option<usize>,
    output: Option<usize>,
    quality: ExportQuality,
    session: Session,
    microphone_vu: Vu,
    system_vu: Vu,
    notice: Option<Status>,
    last_tick: Instant,
    pending_ask: Option<Ask>,
    saved_path: Option<PathBuf>,
    prefs: Prefs,
    store: Option<PathBuf>,
}

impl Recorder {
    #[cfg(windows)]
    pub(crate) fn new(staging: StagingArea) -> Self {
        let path = prefs_path();
        let prefs = Prefs::read(&path);
        Self::with_prefs(Box::new(wasapi::Wasapi), staging, prefs, Some(path))
    }

    #[cfg(test)]
    pub(crate) fn with_devices(devices: Box<dyn Devices>, staging: StagingArea) -> Self {
        Self::with_prefs(devices, staging, Prefs::default(), None)
    }

    pub(crate) fn with_prefs(
        devices: Box<dyn Devices>,
        staging: StagingArea,
        prefs: Prefs,
        store: Option<PathBuf>,
    ) -> Self {
        let mut notice = None;
        let endpoints = match devices.survey() {
            Ok(endpoints) => endpoints,
            Err(error) => {
                notice = Some(warn(error.to_string()));
                Endpoints::from_enumerated(Vec::new(), Vec::new(), None, None)
            }
        };
        let remembered_mic = prefs
            .microphone
            .as_ref()
            .and_then(|raw| MicrophoneId::parse(raw.clone()).ok());
        let remembered_out = prefs
            .output
            .as_ref()
            .and_then(|raw| OutputDeviceId::parse(raw.clone()).ok());
        let microphone = pick_index(
            endpoints.microphones(),
            remembered_mic.as_ref(),
            endpoints.default_microphone(),
        );
        let output = pick_index(
            endpoints.outputs(),
            remembered_out.as_ref(),
            endpoints.default_output(),
        );
        Self {
            devices,
            staging,
            endpoints,
            endpoints_dirty: true,
            microphone,
            output,
            quality: prefs.quality,
            session: Session::Idle,
            microphone_vu: Vu::new(),
            system_vu: Vu::new(),
            notice,
            last_tick: Instant::now(),
            pending_ask: None,
            saved_path: None,
            prefs,
            store,
        }
    }

    pub(crate) fn apply(&mut self, intent: Intent) -> View {
        match intent {
            Intent::Tick => self.advance_meters(),
            Intent::RefreshEndpoints => self.refresh(),
            Intent::ChooseMicrophone(index) => self.select_microphone(index),
            Intent::ChooseOutput(index) => self.select_output(index),
            Intent::ChooseQuality(index) => self.select_quality(index),
            Intent::Toggle => self.toggle(),
            Intent::Start => {
                if matches!(self.session, Session::Idle | Session::Failed(_)) { self.toggle(); }
            }
            Intent::Stop => {
                if matches!(self.session, Session::Recording(_) | Session::Paused(_)) { self.toggle(); }
            }
            Intent::Pause => self.pause(),
            Intent::Save => self.request_destination(),
            Intent::SaveAs => self.request_save_as(),
            Intent::SaveTo(path) => self.save_to(&path),
            Intent::CancelSave => self.cancel_save(),
            Intent::RequestDiscard => {
                if matches!(self.session, Session::AwaitingSave(_)) {
                    self.pending_ask = Some(Ask::ConfirmDiscard);
                }
            }
            Intent::Discard => self.discard(),
            Intent::OpenSettings => self.open_settings(),
            Intent::SetSaveDirect(on) => self.set_save_direct(on),
            Intent::Closing => self.consider_closing(),
            Intent::DiscardAndClose => self.abandon_and_close(),
            Intent::HotkeyUnavailable => {
                self.notice = Some(warn(
                    "Another app holds Ctrl+Shift+R. Use the Start recording button.",
                ))
            }
        }
        self.view()
    }

    fn toggle(&mut self) {
        if matches!(self.session, Session::Idle) {
            self.begin();
        } else if matches!(self.session, Session::Recording(_) | Session::Paused(_)) {
            self.session.stop();
            self.microphone_vu.reset();
            self.system_vu.reset();
        } else if matches!(self.session, Session::AwaitingSave(_)) {
            self.notice = Some(warn("Save or discard this take before starting another."));
        } else if matches!(self.session, Session::Failed(_)) {
            if let Err(error) = self.session.dismiss() {
                self.notice = Some(warn(error.to_string()));
                return;
            }
            self.begin();
        }
    }

    fn begin(&mut self) {
        let (Some(mic_index), Some(out_index)) = (self.microphone, self.output) else {
            self.notice = Some(warn("Pick a microphone and an output device first."));
            return;
        };
        let Some(microphone) = self.endpoints.microphones().get(mic_index) else {
            self.notice = Some(warn("Pick a microphone and an output device first."));
            return;
        };
        let Some(output) = self.endpoints.outputs().get(out_index) else {
            self.notice = Some(warn("Pick a microphone and an output device first."));
            return;
        };
        let mic_id = microphone.id().clone();
        let out_id = output.id().clone();
        let mic = match self.devices.open_microphone(&mic_id) {
            Ok(source) => source,
            Err(error) => {
                self.notice = Some(warn(error.to_string()));
                return;
            }
        };
        let sys = match self.devices.open_loopback(&out_id) {
            Ok(source) => source,
            Err(error) => {
                self.notice = Some(warn(error.to_string()));
                return;
            }
        };
        let staging = match self.staging.next_take() {
            Ok(file) => file,
            Err(error) => {
                self.notice = Some(warn(error.to_string()));
                return;
            }
        };
        self.microphone_vu.reset();
        self.system_vu.reset();
        self.notice = None;
        self.saved_path = None;
        self.last_tick = Instant::now();
        self.session.start(
            mic_id,
            out_id,
            self.microphone_vu.tap(mic),
            self.system_vu.tap(sys),
            staging,
            self.quality,
        );
        self.dismiss_failed_start();
    }

    fn pause(&mut self) {
        match &self.session {
            Session::Recording(_) => self.session.pause(),
            Session::Paused(_) => self.session.resume(),
            _ => {}
        }
    }

    fn dismiss_failed_start(&mut self) {
        let detail = match &self.session {
            Session::Failed(failed) => Some(failed.detail().to_string()),
            _ => None,
        };
        let Some(detail) = detail else {
            return;
        };
        match self.session.dismiss() {
            Ok(()) => {
                self.notice = Some(Status {
                    text: detail,
                    tone: Tone::Failure,
                });
            }
            Err(error) => self.notice = Some(warn(error.to_string())),
        }
    }

    fn request_destination(&mut self) {
        if !matches!(self.session, Session::AwaitingSave(_)) {
            return;
        }
        if self.prefs.save_direct {
            if let Some(path) = self.direct_save_path() {
                self.save_to(&path);
                return;
            }
        }
        self.ask_save_dialog();
    }

    fn request_save_as(&mut self) {
        if !matches!(self.session, Session::AwaitingSave(_)) {
            return;
        }
        self.ask_save_dialog();
    }

    fn direct_save_path(&self) -> Option<PathBuf> {
        let folder = self.prefs.folder.as_ref().filter(|path| path.is_dir())?;
        unique_mp3_path(folder, &dated_file_name(self.quality, CivilTime::local()))
    }

    fn ask_save_dialog(&mut self) {
        self.pending_ask = Some(Ask::SaveDestination(SavePrompt {
            file_name: dated_file_name(self.quality, CivilTime::local()),
            folder: self.prefs.folder.clone().filter(|path| path.is_dir()),
            filter_label: "MP3 audio",
            extension: "mp3",
        }));
    }

    fn open_settings(&mut self) {
        self.pending_ask = Some(Ask::Settings {
            save_direct: self.prefs.save_direct,
        });
    }

    fn set_save_direct(&mut self, on: bool) {
        self.prefs.save_direct = on;
        self.persist();
    }

    fn save_to(&mut self, destination: &std::path::Path) {
        match self.session.save_as(destination) {
            Ok(()) => {
                if matches!(self.session, Session::Saving(_)) {
                    self.notice = None;
                }
            }
            Err(SaveError::NoTake) => {}
            Err(SaveError::Write(detail)) => self.notice = Some(warn(detail)),
        }
    }

    fn cancel_save(&mut self) {
        if !matches!(self.session, Session::AwaitingSave(_)) {
            return;
        }
        self.session.cancel_save();
        self.notice = Some(neutral("Save cancelled. The take is kept."));
    }

    fn discard(&mut self) {
        match self.session.discard() {
            Ok(()) => {
                self.microphone_vu.reset();
                self.system_vu.reset();
                self.notice = None;
            }
            Err(error) => self.notice = Some(warn(error.to_string())),
        }
    }

    fn refresh(&mut self) {
        if !matches!(self.session, Session::Idle | Session::Failed(_)) {
            return;
        }
        match self.devices.survey() {
            Ok(endpoints) => {
                let mic_id = self.selected_microphone_id();
                let out_id = self.selected_output_id();
                self.endpoints = endpoints;
                self.microphone = pick_index(
                    self.endpoints.microphones(),
                    mic_id.as_ref(),
                    self.endpoints.default_microphone(),
                );
                self.output = pick_index(
                    self.endpoints.outputs(),
                    out_id.as_ref(),
                    self.endpoints.default_output(),
                );
                self.endpoints_dirty = true;
            }
            Err(error) => self.notice = Some(warn(error.to_string())),
        }
    }

    fn select_microphone(&mut self, index: usize) {
        if !matches!(self.session, Session::Idle | Session::Failed(_)) {
            return;
        }
        if index >= self.endpoints.microphones().len() {
            return;
        }
        self.microphone = Some(index);
        self.persist();
    }

    fn select_output(&mut self, index: usize) {
        if !matches!(self.session, Session::Idle | Session::Failed(_)) {
            return;
        }
        if index >= self.endpoints.outputs().len() {
            return;
        }
        self.output = Some(index);
        self.persist();
    }

    fn select_quality(&mut self, index: usize) {
        if !matches!(self.session, Session::Idle | Session::Failed(_)) {
            return;
        }
        let Some(quality) = ExportQuality::from_index(index) else {
            return;
        };
        self.quality = quality;
        self.persist();
    }

    fn advance_meters(&mut self) {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        if matches!(self.session, Session::Recording(_)) {
            self.microphone_vu.advance(dt);
            self.system_vu.advance(dt);
        }
        self.drive_save();
    }

    fn drive_save(&mut self) {
        match self.session.poll() {
            None => {}
            Some(Ok(path)) => {
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                self.notice = Some(neutral(format!("Saved: {name}")));
                if let Some(folder) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                    self.prefs.folder = Some(folder.to_path_buf());
                    self.persist();
                }
                self.saved_path = Some(path);
                self.microphone_vu.reset();
                self.system_vu.reset();
            }
            Some(Err(SaveError::NoTake)) => {}
            Some(Err(SaveError::Write(detail))) => {
                self.notice = Some(warn(detail));
            }
        }
    }

    fn consider_closing(&mut self) {
        self.pending_ask = Some(match &self.session {
            Session::Idle | Session::Failed(_) => Ask::Close,
            Session::Recording(_)
            | Session::Paused(_)
            | Session::AwaitingSave(_)
            | Session::Saving(_) => Ask::ConfirmClose,
        });
    }

    fn abandon_and_close(&mut self) {
        if matches!(self.session, Session::Recording(_) | Session::Paused(_)) {
            self.session.stop();
        }
        let _ = self.session.discard();
        self.microphone_vu.reset();
        self.system_vu.reset();
        self.pending_ask = Some(Ask::Close);
    }

    fn view(&mut self) -> View {
        let endpoints = if self.endpoints_dirty {
            self.endpoints_dirty = false;
            Some(self.endpoint_lists())
        } else {
            None
        };
        let pickers_enabled = matches!(self.session, Session::Idle | Session::Failed(_));
        let recording = matches!(self.session, Session::Recording(_));
        View {
            phase: match self.session {
                Session::Idle => Phase::Idle,
                Session::Recording(_) => Phase::Recording,
                Session::Paused(_) => Phase::Paused,
                Session::AwaitingSave(_) => Phase::AwaitingSave,
                Session::Saving(_) => Phase::Saving,
                Session::Failed(_) => Phase::Failed,
            },
            endpoints,
            microphone: Selector {
                selected: self.microphone,
                enabled: pickers_enabled,
            },
            output: Selector {
                selected: self.output,
                enabled: pickers_enabled,
            },
            quality: Selector {
                selected: Some(self.quality.index()),
                enabled: pickers_enabled,
            },
            transport: self.transport(),
            elapsed: format_elapsed(self.session.elapsed().unwrap_or_default()),
            levels: if recording {
                Levels {
                    microphone: self.microphone_vu.snapshot(),
                    system: self.system_vu.snapshot(),
                }
            } else {
                Levels {
                    microphone: Level::ZERO,
                    system: Level::ZERO,
                }
            },
            progress: self.session.save_progress(),
            status: if matches!(
                self.session,
                Session::Recording(_) | Session::Paused(_) | Session::Saving(_)
            ) {
                self.derived_status()
            } else {
                self.notice.clone().unwrap_or_else(|| self.derived_status())
            },
            ask: self.pending_ask.take(),
            saved_path: self.saved_path.clone(),
            save_direct: self.prefs.save_direct,
        }
    }

    fn transport(&self) -> Transport {
        match &self.session {
            Session::Idle => Transport {
                toggle_label: "Start recording",
                toggle_enabled: self.microphone.is_some() && self.output.is_some(),
                save_enabled: false,
                discard_enabled: false,
            },
            Session::Recording(_) | Session::Paused(_) => Transport {
                toggle_label: "Stop recording",
                toggle_enabled: true,
                save_enabled: false,
                discard_enabled: false,
            },
            Session::AwaitingSave(_) => Transport {
                toggle_label: "Start recording",
                toggle_enabled: false,
                save_enabled: true,
                discard_enabled: true,
            },
            Session::Failed(_) => Transport {
                toggle_label: "Start recording",
                toggle_enabled: true,
                save_enabled: false,
                discard_enabled: false,
            },
            Session::Saving(_) => Transport {
                toggle_label: "Start recording",
                toggle_enabled: false,
                save_enabled: false,
                discard_enabled: false,
            },
        }
    }

    fn derived_status(&self) -> Status {
        match &self.session {
            Session::Idle if self.microphone.is_none() => warn("Connect a microphone to record."),
            Session::Idle if self.output.is_none() => warn("No output device found."),
            Session::Idle => neutral("Ready. Ctrl+Shift+R starts recording."),
            Session::Recording(_) if self.session.is_degraded() => {
                warn("Recording. One device dropped out; the other is still being recorded.")
            }
            Session::Recording(_) => Status {
                text: "Recording.".into(),
                tone: Tone::Recording,
            },
            Session::Paused(_) if self.session.is_degraded() => {
                warn("Recording paused. One device dropped out; the other is still being recorded.")
            }
            Session::Paused(_) => neutral("Recording paused."),
            Session::AwaitingSave(_) => {
                neutral("Take ready. Save or discard it before the next one.")
            }
            Session::Saving(_) => {
                let percent = self
                    .session
                    .save_progress()
                    .map(SaveProgress::percent)
                    .unwrap_or(0);
                neutral(format!(
                    "Saving {} MP3, {percent}%",
                    self.quality.short_name()
                ))
            }
            Session::Failed(failed) => Status {
                text: failed.to_string(),
                tone: Tone::Failure,
            },
        }
    }

    fn endpoint_lists(&self) -> EndpointLists {
        EndpointLists {
            microphones: self
                .endpoints
                .microphones()
                .iter()
                .map(|endpoint| endpoint.name().to_string())
                .collect(),
            outputs: self
                .endpoints
                .outputs()
                .iter()
                .map(|endpoint| endpoint.name().to_string())
                .collect(),
        }
    }

    fn selected_microphone_id(&self) -> Option<MicrophoneId> {
        self.microphone
            .and_then(|index| self.endpoints.microphones().get(index))
            .map(|endpoint| endpoint.id().clone())
    }

    fn selected_output_id(&self) -> Option<OutputDeviceId> {
        self.output
            .and_then(|index| self.endpoints.outputs().get(index))
            .map(|endpoint| endpoint.id().clone())
    }

    fn persist(&mut self) {
        self.prefs.microphone = self
            .selected_microphone_id()
            .map(|id| id.as_str().to_owned());
        self.prefs.output = self.selected_output_id().map(|id| id.as_str().to_owned());
        self.prefs.quality = self.quality;
        let Some(path) = &self.store else {
            return;
        };
        let _ = self.prefs.write(path);
    }
}

fn pick_index<Id: PartialEq>(
    list: &[Endpoint<Id>],
    current: Option<&Id>,
    default: Option<&Id>,
) -> Option<usize> {
    current
        .and_then(|id| list.iter().position(|item| item.id() == id))
        .or_else(|| default.and_then(|id| list.iter().position(|item| item.id() == id)))
        .or_else(|| (!list.is_empty()).then_some(0))
}

fn format_elapsed(elapsed: Duration) -> String {
    let total_seconds = elapsed.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds / 60) % 60;
    let seconds = total_seconds % 60;
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
    }
}

fn warn(text: impl Into<String>) -> Status {
    Status {
        text: text.into(),
        tone: Tone::Warning,
    }
}

fn neutral(text: impl Into<String>) -> Status {
    Status {
        text: text.into(),
        tone: Tone::Neutral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Endpoint;
    use crate::capture::SessionFrame;
    use crate::capture::{CaptureError, CaptureRead, NoPacketSource, PcmSource, TimedStereoFrames};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    enum MicKind {
        Tone(f32),
        Burst { packets: u32, amp: f32 },
    }

    struct FakeDevices {
        opens: Arc<AtomicUsize>,
        endpoints: Endpoints,
        fail_mic: bool,
        mic: MicKind,
    }

    struct BurstSource {
        left: u32,
        sample: [f32; 2],
    }

    impl CaptureSource for BurstSource {
        fn read(&mut self, _max_wait: Duration) -> Result<CaptureRead, CaptureError> {
            if self.left == 0 {
                return Ok(CaptureRead::NoPacket);
            }
            self.left -= 1;
            Ok(CaptureRead::Frames(
                TimedStereoFrames::try_new(SessionFrame::from_index(0), vec![self.sample; 8])
                    .unwrap(),
            ))
        }
    }

    impl Devices for FakeDevices {
        fn survey(&self) -> Result<Endpoints, AudioError> {
            Ok(self.endpoints.clone())
        }

        fn open_microphone(
            &self,
            _id: &MicrophoneId,
        ) -> Result<Box<dyn CaptureSource>, AudioError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            if self.fail_mic {
                return Err(AudioError::new("microphone open failed"));
            }
            let source: Box<dyn CaptureSource> = match self.mic {
                MicKind::Tone(amp) => Box::new(PcmSource::tone(amp)),
                MicKind::Burst { packets, amp } => Box::new(BurstSource {
                    left: packets,
                    sample: [amp, amp],
                }),
            };
            Ok(source)
        }

        fn open_loopback(
            &self,
            _id: &OutputDeviceId,
        ) -> Result<Box<dyn CaptureSource>, AudioError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(NoPacketSource))
        }
    }

    fn microphone(id: &str, name: &str) -> crate::audio::Microphone {
        Endpoint::new(MicrophoneId::parse(id.into()).unwrap(), name.into())
    }

    fn output(id: &str, name: &str) -> crate::audio::OutputDevice {
        Endpoint::new(OutputDeviceId::parse(id.into()).unwrap(), name.into())
    }

    fn test_endpoints() -> Endpoints {
        Endpoints::from_enumerated(
            vec![microphone("mic-a", "Mic A"), microphone("mic-b", "Mic B")],
            vec![output("out-a", "Speakers"), output("out-b", "Headphones")],
            Some(MicrophoneId::parse("mic-a".into()).unwrap()),
            Some(OutputDeviceId::parse("out-a".into()).unwrap()),
        )
    }

    fn recorder(mic: MicKind, fail_mic: bool) -> (Recorder, Arc<AtomicUsize>) {
        let opens = Arc::new(AtomicUsize::new(0));
        let devices = FakeDevices {
            opens: Arc::clone(&opens),
            endpoints: test_endpoints(),
            fail_mic,
            mic,
        };
        let recorder = Recorder::with_devices(Box::new(devices), StagingArea::open().unwrap());
        (recorder, opens)
    }

    fn recorder_with_prefs(prefs: Prefs, store: Option<PathBuf>) -> Recorder {
        Recorder::with_prefs(
            Box::new(FakeDevices {
                opens: Arc::new(AtomicUsize::new(0)),
                endpoints: test_endpoints(),
                fail_mic: false,
                mic: MicKind::Tone(0.25),
            }),
            StagingArea::open().unwrap(),
            prefs,
            store,
        )
    }

    fn wait_mix() {
        thread::sleep(Duration::from_millis(50));
    }

    fn stop_take(recorder: &mut Recorder) -> View {
        recorder.apply(Intent::Toggle);
        wait_mix();
        recorder.apply(Intent::Toggle)
    }

    fn finish_save(recorder: &mut Recorder) -> View {
        let mut view = recorder.apply(Intent::Tick);
        for _ in 0..10_000 {
            if view.progress.is_none() {
                return view;
            }
            view = recorder.apply(Intent::Tick);
            thread::sleep(Duration::from_millis(1));
        }
        panic!("save did not reach Idle")
    }

    #[test]
    fn picker_change_opens_nothing() {
        let (mut recorder, opens) = recorder(MicKind::Tone(0.25), false);
        let view = recorder.apply(Intent::ChooseMicrophone(1));
        assert_eq!(view.microphone.selected, Some(1));
        assert_eq!(view.transport.toggle_label, "Start recording");
        assert_eq!(opens.load(Ordering::SeqCst), 0);
        let view = recorder.apply(Intent::ChooseOutput(1));
        assert_eq!(view.output.selected, Some(1));
        assert_eq!(opens.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn toggle_from_idle_opens_both_streams_and_starts() {
        let (mut recorder, opens) = recorder(MicKind::Tone(0.25), false);
        let view = recorder.apply(Intent::Toggle);
        assert_eq!(view.transport.toggle_label, "Stop recording");
        assert!(view.transport.toggle_enabled);
        assert!(!view.transport.save_enabled);
        assert_eq!(opens.load(Ordering::SeqCst), 2);
        recorder.apply(Intent::Toggle);
    }

    #[test]
    fn toggle_while_awaiting_save_refuses_and_opens_nothing_more() {
        let (mut recorder, opens) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Toggle);
        wait_mix();
        let stopped = recorder.apply(Intent::Toggle);
        assert!(stopped.transport.save_enabled);
        assert!(!stopped.transport.toggle_enabled);
        let opens_after_stop = opens.load(Ordering::SeqCst);
        assert_eq!(opens_after_stop, 2);
        let refused = recorder.apply(Intent::Toggle);
        assert!(!refused.transport.toggle_enabled);
        assert!(refused.transport.save_enabled);
        assert_eq!(refused.transport.toggle_label, "Start recording");
        assert_eq!(
            refused.status.text,
            "Save or discard this take before starting another."
        );
        assert_eq!(refused.status.tone, Tone::Warning);
        assert_eq!(opens.load(Ordering::SeqCst), opens_after_stop);
    }

    #[test]
    fn discard_returns_to_idle_and_unlinks_staging() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        let stopped = stop_take(&mut recorder);
        assert!(stopped.transport.save_enabled);
        let view = recorder.apply(Intent::Discard);
        assert_eq!(view.transport.toggle_label, "Start recording");
        assert!(view.transport.toggle_enabled);
        assert!(!view.transport.save_enabled);
        assert!(!view.transport.discard_enabled);
        let again = recorder.apply(Intent::Toggle);
        assert_eq!(again.transport.toggle_label, "Stop recording");
        recorder.apply(Intent::Toggle);
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn cancel_save_keeps_the_take() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        stop_take(&mut recorder);
        let view = recorder.apply(Intent::CancelSave);
        assert!(view.transport.save_enabled);
        assert!(!view.transport.toggle_enabled);
        assert_eq!(view.status.text, "Save cancelled. The take is kept.");
        let still_pending = recorder.apply(Intent::Tick);
        assert!(still_pending.transport.save_enabled);
        let discarded = recorder.apply(Intent::Discard);
        assert!(!discarded.transport.save_enabled);
        assert_eq!(discarded.transport.toggle_label, "Start recording");
    }

    #[test]
    fn failed_open_leaves_idle_and_does_not_start() {
        let (mut recorder, opens) = recorder(MicKind::Tone(0.25), true);
        let view = recorder.apply(Intent::Toggle);
        assert_eq!(view.transport.toggle_label, "Start recording");
        assert!(view.transport.toggle_enabled);
        assert!(!view.transport.save_enabled);
        assert_eq!(view.status.text, "microphone open failed");
        assert_eq!(view.status.tone, Tone::Warning);
        assert_eq!(opens.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn endpoint_lists_omitted_when_nothing_changed() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        let first = recorder.apply(Intent::Tick);
        assert_eq!(first.elapsed, "00:00");
        assert_eq!(
            first
                .endpoints
                .as_ref()
                .map(|lists| lists.microphones.as_slice()),
            Some(["Mic A".to_string(), "Mic B".to_string()].as_slice())
        );
        let second = recorder.apply(Intent::Tick);
        assert_eq!(second.endpoints, None);
        let picked = recorder.apply(Intent::ChooseMicrophone(1));
        assert_eq!(picked.endpoints, None);
        assert_eq!(picked.microphone.selected, Some(1));
        let refreshed = recorder.apply(Intent::RefreshEndpoints);
        assert!(refreshed.endpoints.is_some());
        let after_refresh = recorder.apply(Intent::Tick);
        assert_eq!(after_refresh.endpoints, None);
    }

    #[test]
    fn save_asks_once_then_save_to_returns_idle() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        stop_take(&mut recorder);
        let asked = recorder.apply(Intent::Save);
        let Ask::SaveDestination(prompt) = asked.ask.expect("save prompt") else {
            panic!("expected a save prompt");
        };
        assert!(
            prompt.file_name.ends_with(" Meeting"),
            "{}",
            prompt.file_name
        );
        assert_eq!(prompt.filter_label, "MP3 audio");
        assert_eq!(prompt.extension, "mp3");
        assert_eq!(prompt.folder, None);
        assert_eq!(recorder.apply(Intent::Tick).ask, None);
        let dest = tempfile::tempdir().unwrap();
        let path = dest.path().join("take.mp3");
        let started = recorder.apply(Intent::SaveTo(path.clone()));
        let progress = started.progress.expect("save started");
        assert!(progress.done <= progress.total);
        assert!(progress.total > 0);
        assert!(!started.transport.save_enabled);
        assert!(!started.transport.toggle_enabled);
        assert!(!started.quality.enabled);
        assert_eq!(started.status.text, "Saving Meeting MP3, 0%");
        let saved = finish_save(&mut recorder);
        assert_eq!(saved.transport.toggle_label, "Start recording");
        assert!(saved.transport.toggle_enabled);
        assert!(!saved.transport.save_enabled);
        assert!(saved.progress.is_none());
        assert!(path.exists());
    }

    #[test]
    fn save_to_after_cancel_still_shows_saving_status() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        stop_take(&mut recorder);
        recorder.apply(Intent::CancelSave);
        let dest = tempfile::tempdir().unwrap();
        let path = dest.path().join("take.mp3");
        let started = recorder.apply(Intent::SaveTo(path.clone()));
        assert!(
            started.status.text.starts_with("Saving Meeting MP3, "),
            "{}",
            started.status.text
        );
        assert!(
            started.status.text.ends_with('%'),
            "{}",
            started.status.text
        );
        let percent = started.progress.expect("save started").percent();
        assert_eq!(
            started.status.text,
            format!("Saving Meeting MP3, {percent}%")
        );
        finish_save(&mut recorder);
    }

    #[test]
    fn quality_defaults_to_meeting_and_changes_while_idle() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        let view = recorder.apply(Intent::Tick);
        assert_eq!(view.quality.selected, Some(ExportQuality::Meeting.index()));
        assert!(view.quality.enabled);
        let high = recorder.apply(Intent::ChooseQuality(ExportQuality::High.index()));
        assert_eq!(high.quality.selected, Some(ExportQuality::High.index()));
    }

    #[test]
    fn remembered_devices_beat_windows_defaults() {
        let mut recorder = recorder_with_prefs(
            Prefs {
                microphone: Some("mic-b".into()),
                output: Some("out-b".into()),
                quality: ExportQuality::Voice,
                folder: None,
                save_direct: false,
            },
            None,
        );
        let view = recorder.apply(Intent::Tick);
        assert_eq!(view.microphone.selected, Some(1));
        assert_eq!(view.output.selected, Some(1));
        assert_eq!(view.quality.selected, Some(ExportQuality::Voice.index()));
    }

    #[test]
    fn missing_remembered_device_falls_back_to_default() {
        let mut recorder = recorder_with_prefs(
            Prefs {
                microphone: Some("mic-gone".into()),
                output: None,
                quality: ExportQuality::Meeting,
                folder: None,
                save_direct: false,
            },
            None,
        );
        let view = recorder.apply(Intent::Tick);
        assert_eq!(view.microphone.selected, Some(0));
        assert_eq!(view.output.selected, Some(0));
    }

    #[test]
    fn choosing_quality_writes_prefs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onerec.ini");
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(path.clone()));
        recorder.apply(Intent::ChooseMicrophone(1));
        recorder.apply(Intent::ChooseOutput(1));
        recorder.apply(Intent::ChooseQuality(ExportQuality::High.index()));
        let loaded = Prefs::read(&path);
        assert_eq!(loaded.microphone.as_deref(), Some("mic-b"));
        assert_eq!(loaded.output.as_deref(), Some("out-b"));
        assert_eq!(loaded.quality, ExportQuality::High);
    }

    #[test]
    fn save_prompt_uses_quality_name_and_last_folder() {
        let folder = tempfile::tempdir().unwrap();
        let mut recorder = recorder_with_prefs(
            Prefs {
                microphone: None,
                output: None,
                quality: ExportQuality::High,
                folder: Some(folder.path().to_path_buf()),
                save_direct: false,
            },
            None,
        );
        stop_take(&mut recorder);
        let asked = recorder.apply(Intent::Save);
        let Ask::SaveDestination(prompt) = asked.ask.expect("save prompt") else {
            panic!("expected a save prompt");
        };
        assert!(prompt.file_name.ends_with(" High"), "{}", prompt.file_name);
        assert_eq!(prompt.folder.as_deref(), Some(folder.path()));
    }

    #[test]
    fn successful_save_remembers_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("onerec.ini");
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = dest_dir.path().join("take.mp3");
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(store.clone()));
        stop_take(&mut recorder);
        recorder.apply(Intent::SaveTo(dest));
        finish_save(&mut recorder);
        let loaded = Prefs::read(&store);
        assert_eq!(loaded.folder.as_deref(), Some(dest_dir.path()));
    }

    #[test]
    fn quality_is_ignored_while_recording() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::ChooseQuality(ExportQuality::Compact.index()));
        recorder.apply(Intent::Toggle);
        let ignored = recorder.apply(Intent::ChooseQuality(ExportQuality::High.index()));
        assert_eq!(
            ignored.quality.selected,
            Some(ExportQuality::Compact.index())
        );
        assert!(!ignored.quality.enabled);
        recorder.apply(Intent::Toggle);
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn quality_is_locked_while_awaiting_save() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        let stopped = stop_take(&mut recorder);
        assert!(!stopped.quality.enabled);
        let ignored = recorder.apply(Intent::ChooseQuality(ExportQuality::High.index()));
        assert_eq!(
            ignored.quality.selected,
            Some(ExportQuality::Meeting.index())
        );
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn choose_quality_before_recording_sets_the_file_bitrate() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::ChooseQuality(ExportQuality::High.index()));
        stop_take(&mut recorder);
        let dest = tempfile::tempdir().unwrap();
        let path = dest.path().join("take.mp3");
        recorder.apply(Intent::SaveTo(path.clone()));
        finish_save(&mut recorder);
        let mp3 = std::fs::read(&path).unwrap();
        let frames = crate::mp3::parse_mpeg1_layer3_cbr(&mp3).unwrap();
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|frame| frame.bitrate_kbps == 320));
    }

    #[test]
    fn vu_attacks_instantly_and_decays_over_tick() {
        let (mut recorder, _) = recorder(
            MicKind::Burst {
                packets: 2,
                amp: 0.8,
            },
            false,
        );
        recorder.apply(Intent::Toggle);
        wait_mix();
        let attacked = recorder.apply(Intent::Tick);
        assert!(
            attacked.levels.microphone.peak >= 0.7,
            "attacked peak was {}",
            attacked.levels.microphone.peak
        );
        thread::sleep(Duration::from_millis(250));
        let decayed = recorder.apply(Intent::Tick);
        assert!(
            decayed.levels.microphone.peak < attacked.levels.microphone.peak,
            "decayed {} was not below attacked {}",
            decayed.levels.microphone.peak,
            attacked.levels.microphone.peak
        );
        recorder.apply(Intent::Toggle);
    }

    #[test]
    fn closing_while_recording_asks_for_confirmation_first() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Toggle);
        let asked = recorder.apply(Intent::Closing);
        assert_eq!(asked.ask, Some(Ask::ConfirmClose));
        assert_eq!(asked.transport.toggle_label, "Stop recording");
        let kept = recorder.apply(Intent::Tick);
        assert_eq!(kept.ask, None);
        assert_eq!(kept.transport.toggle_label, "Stop recording");
        recorder.apply(Intent::Toggle);
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn closing_when_idle_asks_to_close_outright() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        assert_eq!(recorder.apply(Intent::Closing).ask, Some(Ask::Close));
    }

    #[test]
    fn discard_and_close_unlinks_the_pending_take() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        stop_take(&mut recorder);
        let Session::AwaitingSave(pending) = &recorder.session else {
            panic!("the take should be waiting to be saved");
        };
        let staged = pending.staging_file().to_path_buf();
        assert!(staged.exists());
        let closing = recorder.apply(Intent::DiscardAndClose);
        assert_eq!(closing.ask, Some(Ask::Close));
        assert!(!staged.exists(), "{} was left behind", staged.display());
    }

    #[test]
    fn discard_and_close_stops_a_running_take_first() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Toggle);
        wait_mix();
        let closing = recorder.apply(Intent::DiscardAndClose);
        assert_eq!(closing.ask, Some(Ask::Close));
        assert_eq!(closing.transport.toggle_label, "Start recording");
        assert!(!closing.transport.save_enabled);
        assert!(!closing.transport.discard_enabled);
    }

    #[test]
    fn toggle_while_paused_stops() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Toggle);
        wait_mix();
        let paused = recorder.apply(Intent::Pause);
        assert_eq!(paused.phase, Phase::Paused);
        assert_eq!(paused.transport.toggle_label, "Stop recording");
        let stopped = recorder.apply(Intent::Toggle);
        assert_eq!(stopped.phase, Phase::AwaitingSave);
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn explicit_record_and_stop_never_toggle_the_opposite_action() {
        let (mut recorder, opens) = recorder(MicKind::Tone(0.25), false);
        assert_eq!(recorder.apply(Intent::Stop).phase, Phase::Idle);
        assert_eq!(opens.load(Ordering::SeqCst), 0);
        assert_eq!(recorder.apply(Intent::Start).phase, Phase::Recording);
        assert_eq!(recorder.apply(Intent::Start).phase, Phase::Recording);
        assert_eq!(opens.load(Ordering::SeqCst), 2);
        wait_mix();
        assert_eq!(recorder.apply(Intent::Pause).phase, Phase::Paused);
        assert_eq!(recorder.apply(Intent::Start).phase, Phase::Paused);
        assert_eq!(recorder.apply(Intent::Stop).phase, Phase::AwaitingSave);
        assert_eq!(recorder.apply(Intent::Stop).phase, Phase::AwaitingSave);
        assert_eq!(recorder.apply(Intent::Start).phase, Phase::AwaitingSave);
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn settings_can_be_opened_and_changed_without_interrupting_a_take() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Start);
        wait_mix();
        for (intent, expected) in [(Intent::Tick, Phase::Recording),
            (Intent::Pause, Phase::Paused), (Intent::Stop, Phase::AwaitingSave)] {
            recorder.apply(intent);
            let view = recorder.apply(Intent::OpenSettings);
            assert_eq!(view.phase, expected);
            assert!(matches!(view.ask, Some(Ask::Settings { .. })));
            let updated = recorder.apply(Intent::SetSaveDirect(true));
            assert_eq!(updated.phase, expected);
            assert!(updated.save_direct);
        }
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn pause_locks_quality() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::ChooseQuality(ExportQuality::Compact.index()));
        recorder.apply(Intent::Toggle);
        wait_mix();
        recorder.apply(Intent::Pause);
        let ignored = recorder.apply(Intent::ChooseQuality(ExportQuality::High.index()));
        assert_eq!(ignored.phase, Phase::Paused);
        assert!(!ignored.quality.enabled);
        assert_eq!(
            ignored.quality.selected,
            Some(ExportQuality::Compact.index())
        );
        recorder.apply(Intent::Toggle);
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn elapsed_does_not_advance_while_paused() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Toggle);
        wait_mix();
        recorder.apply(Intent::Pause);
        let frozen = recorder.session.elapsed().expect("paused elapsed");
        thread::sleep(Duration::from_millis(150));
        assert_eq!(recorder.session.elapsed(), Some(frozen));
        recorder.apply(Intent::Toggle);
        recorder.apply(Intent::Discard);
    }

    #[test]
    fn discard_and_close_stops_a_paused_take_first() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Toggle);
        wait_mix();
        recorder.apply(Intent::Pause);
        let closing = recorder.apply(Intent::DiscardAndClose);
        assert_eq!(closing.ask, Some(Ask::Close));
        assert_eq!(closing.transport.toggle_label, "Start recording");
        assert!(!closing.transport.save_enabled);
        assert_eq!(closing.phase, Phase::Idle);
    }

    #[test]
    fn a_taken_hotkey_says_so_and_leaves_the_button() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        let view = recorder.apply(Intent::HotkeyUnavailable);
        assert_eq!(
            view.status.text,
            "Another app holds Ctrl+Shift+R. Use the Start recording button."
        );
        assert_eq!(view.status.tone, Tone::Warning);
        assert!(view.transport.toggle_enabled);
    }

    #[test]
    fn discard_request_keeps_take_until_confirmed() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        stop_take(&mut recorder);
        let asked = recorder.apply(Intent::RequestDiscard);
        assert_eq!(asked.ask, Some(Ask::ConfirmDiscard));
        assert_eq!(asked.phase, Phase::AwaitingSave);
        let cancelled = recorder.apply(Intent::Tick);
        assert!(cancelled.ask.is_none());
        assert!(cancelled.transport.save_enabled);
        assert_eq!(recorder.apply(Intent::Discard).phase, Phase::Idle);
    }

    #[test]
    fn retry_save_replaces_cancel_notice_and_provides_saved_path() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        stop_take(&mut recorder);
        recorder.apply(Intent::CancelSave);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("meeting.mp3");
        let saving = recorder.apply(Intent::SaveTo(path.clone()));
        assert_eq!(saving.phase, Phase::Saving);
        assert!(saving.status.text.starts_with("Saving Meeting MP3, "));
        let saved = finish_save(&mut recorder);
        assert_eq!(saved.phase, Phase::Idle);
        assert_eq!(saved.saved_path, Some(path));
        assert_eq!(saved.status.text, "Saved: meeting.mp3");
    }

    #[test]
    fn timer_sleeps_when_idle_without_stalling_export_when_minimized() {
        for phase in [Phase::Idle, Phase::Paused, Phase::AwaitingSave, Phase::Failed] {
            assert_eq!(phase.timer_ms(false), None);
            assert_eq!(phase.timer_ms(true), None);
        }
        assert_eq!(Phase::Recording.timer_ms(false), Some(50));
        assert_eq!(Phase::Recording.timer_ms(true), Some(500));
        assert_eq!(Phase::Saving.timer_ms(true), Some(50));
    }

    #[test]
    fn save_direct_skips_ask_when_folder_known() {
        let folder = tempfile::tempdir().unwrap();
        let mut recorder = recorder_with_prefs(
            Prefs {
                microphone: None,
                output: None,
                quality: ExportQuality::Meeting,
                folder: Some(folder.path().to_path_buf()),
                save_direct: true,
            },
            None,
        );
        stop_take(&mut recorder);
        let started = recorder.apply(Intent::Save);
        assert!(started.ask.is_none());
        assert_eq!(started.phase, Phase::Saving);
        let saved = finish_save(&mut recorder);
        assert_eq!(saved.phase, Phase::Idle);
        let written: Vec<_> = std::fs::read_dir(folder.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("mp3"))
            .collect();
        assert_eq!(written.len(), 1);
    }

    #[test]
    fn save_direct_asks_when_folder_missing() {
        let mut recorder = recorder_with_prefs(
            Prefs {
                microphone: None,
                output: None,
                quality: ExportQuality::Meeting,
                folder: None,
                save_direct: true,
            },
            None,
        );
        stop_take(&mut recorder);
        let asked = recorder.apply(Intent::Save);
        assert!(matches!(asked.ask, Some(Ask::SaveDestination(_))));
        assert_eq!(asked.phase, Phase::AwaitingSave);
    }
}
