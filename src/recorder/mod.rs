mod level;
#[cfg(windows)]
mod wasapi;

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::{AudioError, Endpoint, Endpoints};
use crate::capture::{CaptureRead, CaptureSource};
use crate::ids::{MicrophoneId, OutputDeviceId};
use crate::mp3::{ExportQuality, SaveProgress};
#[cfg(windows)]
use crate::prefs::prefs_path;
use crate::prefs::{optional_text, optional_url, Prefs, Shortcut};
use crate::save_path::{save_plan, SavePlan};
use crate::session::{SaveError, Session};
use crate::sidecar::{self, Job, JobKind, StartError};
use crate::staging::StagingArea;
use crate::update::{self, Release};
use crate::vault::{self, SecretStore};

pub(crate) use self::level::Level;
use self::level::Vu;
const HOTKEY_UNAVAILABLE: &str =
    "The recording shortcut is unavailable. Change it in Settings or use Record / Stop.";
const PREFS_UNSAVED: &str = "This change applies for this session, but could not be saved. Check that the onerec folder is writable.";

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
    SaveTo(PathBuf),
    CancelSave,
    RequestDiscard,
    Discard,
    OpenSettings,
    SetSettings(SettingsValues),
    ConfirmNestedSave,
    Sidecar(JobKind),
    RequestUpdate,
    InstallUpdate,
    Closing,
    DiscardAndClose,
    HotkeyUnavailable,
    Minimized(bool),
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
    pub job_busy: bool,
    pub update: Option<String>,
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
    pub(crate) fn timer_ms(self, minimized: bool, job_busy: bool) -> Option<u32> {
        if job_busy {
            return Some(50);
        }
        match self {
            Self::Saving => Some(50),
            Self::AwaitingSave => None,
            _ if minimized => None,
            Self::Idle | Self::Recording | Self::Paused | Self::Failed => Some(50),
        }
    }

    pub(crate) fn meters_live(self) -> bool {
        matches!(
            self,
            Self::Idle | Self::Recording | Self::Paused | Self::Failed
        )
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

#[derive(PartialEq, Eq)]
pub(crate) enum Ask {
    SaveDestination(SavePrompt),
    ConfirmClose,
    ConfirmDiscard,
    Close,
    Settings(SettingsValues),
    OverwriteNested { path: PathBuf },
    ConfirmUpdate { version: String },
    Relaunch,
}

impl std::fmt::Debug for Ask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SaveDestination(prompt) => {
                f.debug_tuple("SaveDestination").field(prompt).finish()
            }
            Self::ConfirmClose => f.debug_tuple("ConfirmClose").finish(),
            Self::ConfirmDiscard => f.debug_tuple("ConfirmDiscard").finish(),
            Self::Close => f.debug_tuple("Close").finish(),
            Self::Settings(values) => f.debug_tuple("Settings").field(values).finish(),
            Self::OverwriteNested { path } => f
                .debug_struct("OverwriteNested")
                .field("path", path)
                .finish(),
            Self::ConfirmUpdate { version } => {
                f.debug_tuple("ConfirmUpdate").field(version).finish()
            }
            Self::Relaunch => f.debug_tuple("Relaunch").finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SettingsValues {
    pub shortcut: Shortcut,
    pub api_key: String,
    pub transcribe_url: String,
    pub transcribe_model: String,
    pub notes_model: String,
    pub nest: bool,
    pub notes_prompt: String,
}

impl Default for SettingsValues {
    fn default() -> Self {
        Self {
            shortcut: Shortcut::default(),
            api_key: String::new(),
            transcribe_url: String::new(),
            transcribe_model: String::new(),
            notes_model: String::new(),
            nest: false,
            notes_prompt: String::new(),
        }
    }
}

impl SettingsValues {
    pub(crate) fn from_prefs_and_vault(prefs: &Prefs, vault: &dyn SecretStore) -> Self {
        Self {
            shortcut: prefs.shortcut,
            api_key: vault.load().ok().flatten().unwrap_or_default(),
            transcribe_url: prefs.transcribe_url.clone().unwrap_or_default(),
            transcribe_model: prefs.transcribe_model.clone().unwrap_or_default(),
            notes_model: prefs.notes_model.clone().unwrap_or_default(),
            nest: prefs.nest,
            notes_prompt: prefs.notes_prompt.clone().unwrap_or_default(),
        }
    }
}

impl std::fmt::Debug for SettingsValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsValues")
            .field("shortcut", &self.shortcut)
            .field(
                "api_key",
                &if self.api_key.is_empty() { "" } else { "****" },
            )
            .field("transcribe_url", &self.transcribe_url)
            .field("transcribe_model", &self.transcribe_model)
            .field("notes_model", &self.notes_model)
            .field("nest", &self.nest)
            .field("notes_prompt", &self.notes_prompt)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavePrompt {
    pub file_name: String,
    pub folder: Option<PathBuf>,
    pub filter_label: &'static str,
    pub extension: &'static str,
    pub warn_if_exists: bool,
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
    mic_monitor: Option<Box<dyn CaptureSource>>,
    sys_monitor: Option<Box<dyn CaptureSource>>,
    mic_monitor_id: Option<MicrophoneId>,
    sys_monitor_id: Option<OutputDeviceId>,
    notice: Option<Status>,
    last_tick: Instant,
    pending_ask: Option<Ask>,
    saved_path: Option<PathBuf>,
    prefs: Prefs,
    store: Option<PathBuf>,
    vault: Box<dyn SecretStore>,
    minimized: bool,
    sidecar_job: Option<(JobKind, JoinHandle<Result<PathBuf, String>>)>,
    pub(crate) sidecar: fn(&Job) -> Result<PathBuf, String>,
    save_plan: Option<SavePlan>,
    update_check: Option<JoinHandle<Result<Option<Release>, String>>>,
    update_install: Option<JoinHandle<Result<(), String>>>,
    available_update: Option<Release>,
    pub(crate) check_update: fn() -> Result<Option<Release>, String>,
    pub(crate) install_update: fn(&Release) -> Result<(), String>,
}

impl Recorder {
    #[cfg(windows)]
    pub(crate) fn new(staging: StagingArea) -> Self {
        let path = prefs_path();
        let vault: Box<dyn SecretStore> = Box::new(vault::CredentialManager);
        let prefs = load_stored_prefs(&path, &*vault);
        let mut recorder =
            Self::with_prefs(Box::new(wasapi::Wasapi), staging, prefs, Some(path), vault);
        recorder.start_update_check();
        recorder
    }

    #[cfg(test)]
    pub(crate) fn with_devices(devices: Box<dyn Devices>, staging: StagingArea) -> Self {
        Self::with_prefs(
            devices,
            staging,
            Prefs::default(),
            None,
            Box::new(vault::MemoryVault::default()),
        )
    }

    pub(crate) fn with_prefs(
        devices: Box<dyn Devices>,
        staging: StagingArea,
        prefs: Prefs,
        store: Option<PathBuf>,
        vault: Box<dyn SecretStore>,
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
        let mut recorder = Self {
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
            mic_monitor: None,
            sys_monitor: None,
            mic_monitor_id: None,
            sys_monitor_id: None,
            notice,
            last_tick: Instant::now(),
            pending_ask: None,
            saved_path: None,
            prefs,
            store,
            vault,
            minimized: false,
            sidecar_job: None,
            sidecar: sidecar::run,
            save_plan: None,
            update_check: None,
            update_install: None,
            available_update: None,
            check_update: update::check,
            install_update: update::install,
        };
        recorder.ensure_monitors();
        recorder
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
                if matches!(self.session, Session::Idle | Session::Failed(_)) {
                    self.toggle();
                }
            }
            Intent::Stop => {
                if matches!(self.session, Session::Recording(_) | Session::Paused(_)) {
                    self.toggle();
                }
            }
            Intent::Pause => self.pause(),
            Intent::Save => self.request_destination(),
            Intent::SaveTo(path) => self.save_to(&path),
            Intent::CancelSave => self.cancel_save(),
            Intent::RequestDiscard => {
                if matches!(self.session, Session::AwaitingSave(_)) {
                    self.pending_ask = Some(Ask::ConfirmDiscard);
                }
            }
            Intent::Discard => self.discard(),
            Intent::OpenSettings => self.open_settings(),
            Intent::SetSettings(next) => {
                self.prefs.shortcut = next.shortcut;
                if let Err(detail) = self.store_secret(&next.api_key) {
                    self.notice = Some(warn(detail));
                }
                self.prefs.transcribe_url = optional_url(&next.transcribe_url);
                self.prefs.transcribe_model = optional_text(&next.transcribe_model);
                self.prefs.notes_model = optional_text(&next.notes_model);
                self.prefs.nest = next.nest;
                self.prefs.notes_prompt = optional_text(&next.notes_prompt);
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.text == HOTKEY_UNAVAILABLE)
                {
                    self.notice = None;
                }
                self.persist_or_notice();
            }
            Intent::ConfirmNestedSave => self.begin_encode(),
            Intent::Sidecar(kind) => self.request_sidecar(kind),
            Intent::RequestUpdate => self.request_update(),
            Intent::InstallUpdate => self.begin_update(),
            Intent::Closing => self.consider_closing(),
            Intent::DiscardAndClose => self.abandon_and_close(),
            Intent::HotkeyUnavailable => self.notice = Some(warn(HOTKEY_UNAVAILABLE)),
            Intent::Minimized(minimized) => self.set_minimized(minimized),
        }
        self.view()
    }

    fn toggle(&mut self) {
        if self.update_install.is_some() {
            self.notice = Some(warn("Wait for the update to finish."));
            return;
        }
        if matches!(self.session, Session::Idle) {
            self.begin();
        } else if matches!(self.session, Session::Recording(_) | Session::Paused(_)) {
            self.session.stop();
            self.microphone_vu.reset();
            self.system_vu.reset();
            self.ensure_monitors();
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
        let (mon_mic, mon_sys) = self.take_monitors();
        let mic = match mon_mic {
            Some(source) => source,
            None => match self.devices.open_microphone(&mic_id) {
                Ok(source) => Box::new(self.microphone_vu.tap(source)),
                Err(error) => {
                    self.notice = Some(warn(error.to_string()));
                    self.ensure_monitors();
                    return;
                }
            },
        };
        let sys = match mon_sys {
            Some(source) => source,
            None => match self.devices.open_loopback(&out_id) {
                Ok(source) => Box::new(self.system_vu.tap(source)),
                Err(error) => {
                    self.notice = Some(warn(error.to_string()));
                    drop(mic);
                    self.ensure_monitors();
                    return;
                }
            },
        };
        let staging = match self.staging.next_take() {
            Ok(file) => file,
            Err(error) => {
                self.notice = Some(warn(error.to_string()));
                drop(mic);
                drop(sys);
                self.ensure_monitors();
                return;
            }
        };
        self.microphone_vu.reset();
        self.system_vu.reset();
        self.notice = None;
        self.saved_path = None;
        self.last_tick = Instant::now();
        self.session
            .start(mic_id, out_id, mic, sys, staging, self.quality);
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
                self.ensure_monitors();
            }
            Err(error) => self.notice = Some(warn(error.to_string())),
        }
    }

    fn request_destination(&mut self) {
        if !matches!(self.session, Session::AwaitingSave(_)) {
            return;
        }
        self.ask_save_dialog();
    }

    fn ask_save_dialog(&mut self) {
        self.pending_ask = Some(Ask::SaveDestination(SavePrompt {
            file_name: self
                .prefs
                .last_file_name
                .clone()
                .unwrap_or_else(|| "Recording.mp3".into()),
            folder: self.prefs.folder.clone().filter(|path| path.is_dir()),
            filter_label: "MP3 audio",
            extension: "mp3",
            warn_if_exists: !self.prefs.nest,
        }));
    }

    fn open_settings(&mut self) {
        self.pending_ask = Some(Ask::Settings(SettingsValues::from_prefs_and_vault(
            &self.prefs,
            &*self.vault,
        )));
    }

    fn store_secret(&mut self, api_key: &str) -> Result<(), String> {
        let key = api_key.trim();
        if key.is_empty() {
            self.vault.delete()
        } else {
            self.vault.save(key)
        }
    }

    fn request_sidecar(&mut self, kind: JobKind) {
        self.poll_sidecar();
        if self.sidecar_job.is_some() {
            return;
        }
        let Some(audio) = self.saved_path.clone() else {
            return;
        };
        match sidecar::prepare_job(&*self.vault, &self.prefs, kind, &audio) {
            Ok(job) => {
                self.notice = Some(neutral(kind.progress_message()));
                let run = self.sidecar;
                self.sidecar_job = Some((kind, std::thread::spawn(move || run(&job))));
            }
            Err(StartError::Gap(gap)) => {
                self.notice = Some(warn(gap.status()));
                self.open_settings();
            }
            Err(StartError::Vault(detail)) => {
                self.notice = Some(warn(detail));
            }
        }
    }

    fn poll_sidecar(&mut self) {
        let Some((kind, job)) = self.sidecar_job.take() else {
            return;
        };
        if !job.is_finished() {
            self.sidecar_job = Some((kind, job));
            return;
        }
        match job.join() {
            Ok(Ok(path)) => {
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                self.notice = Some(neutral(format!("{}: {name}", kind.success_prefix())));
            }
            Ok(Err(detail)) => self.notice = Some(warn(detail)),
            Err(_) => self.notice = Some(warn("The request stopped unexpectedly.")),
        }
    }

    pub(crate) fn start_update_check(&mut self) {
        if self.update_check.is_some() || self.available_update.is_some() {
            return;
        }
        let check = self.check_update;
        self.update_check = Some(std::thread::spawn(move || check()));
    }

    fn request_update(&mut self) {
        self.poll_update();
        if self.update_install.is_some() {
            return;
        }
        let Some(release) = &self.available_update else {
            return;
        };
        if !matches!(self.session, Session::Idle | Session::Failed(_)) {
            self.notice = Some(warn("Save or discard this take before updating."));
            return;
        }
        self.pending_ask = Some(Ask::ConfirmUpdate {
            version: release.version.to_string(),
        });
    }

    fn begin_update(&mut self) {
        self.poll_update();
        if self.update_install.is_some() {
            return;
        }
        if !matches!(self.session, Session::Idle | Session::Failed(_)) {
            self.notice = Some(warn("Save or discard this take before updating."));
            return;
        }
        let Some(release) = self.available_update.clone() else {
            return;
        };
        self.notice = Some(neutral(format!("Downloading onerec {}…", release.version)));
        let install = self.install_update;
        self.update_install = Some(std::thread::spawn(move || install(&release)));
    }

    fn poll_update(&mut self) {
        if let Some(job) = self.update_install.take() {
            if !job.is_finished() {
                self.update_install = Some(job);
                return;
            }
            match job.join() {
                Ok(Ok(())) => {
                    self.available_update = None;
                    self.notice = Some(neutral("Updated. Restarting…"));
                    self.pending_ask = Some(Ask::Relaunch);
                }
                Ok(Err(detail)) => self.notice = Some(warn(detail)),
                Err(_) => self.notice = Some(warn("The update stopped unexpectedly.")),
            }
            return;
        }
        let Some(job) = self.update_check.take() else {
            return;
        };
        if !job.is_finished() {
            self.update_check = Some(job);
            return;
        }
        match job.join() {
            Ok(Ok(Some(release))) => {
                self.available_update = Some(release.clone());
                if self.notice.is_none()
                    && matches!(self.session, Session::Idle | Session::Failed(_))
                {
                    self.notice =
                        Some(neutral(format!("onerec {} is available.", release.version)));
                }
            }
            Ok(Ok(None) | Err(_)) | Err(_) => {}
        }
    }

    pub(crate) fn shortcut(&self) -> Shortcut {
        self.prefs.shortcut
    }

    fn save_to(&mut self, dialog: &std::path::Path) {
        let plan = save_plan(dialog, self.prefs.nest);
        self.save_plan = Some(plan.clone());
        if plan.needs_overwrite_confirm(dialog) {
            self.pending_ask = Some(Ask::OverwriteNested {
                path: plan.encode.clone(),
            });
            return;
        }
        self.begin_encode();
    }

    fn begin_encode(&mut self) {
        let Some(plan) = self.save_plan.clone() else {
            return;
        };
        if let Some(parent) = plan
            .encode
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.notice = Some(warn(error.to_string()));
                self.save_plan = None;
                return;
            }
        }
        match self.session.save_as(&plan.encode) {
            Ok(()) => {
                if matches!(self.session, Session::Saving(_)) {
                    self.notice = None;
                }
            }
            Err(SaveError::NoTake) => {
                self.save_plan = None;
            }
            Err(SaveError::Write(detail)) => {
                self.notice = Some(warn(detail));
                self.save_plan = None;
            }
        }
    }

    fn cancel_save(&mut self) {
        if !matches!(self.session, Session::AwaitingSave(_)) {
            return;
        }
        self.save_plan = None;
        self.session.cancel_save();
        self.notice = Some(neutral("Save cancelled. The take is kept."));
    }

    fn discard(&mut self) {
        match self.session.discard() {
            Ok(()) => {
                self.microphone_vu.reset();
                self.system_vu.reset();
                self.notice = None;
                self.ensure_monitors();
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
                self.ensure_monitors();
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
        self.persist_or_notice();
        self.ensure_monitors();
    }

    fn select_output(&mut self, index: usize) {
        if !matches!(self.session, Session::Idle | Session::Failed(_)) {
            return;
        }
        if index >= self.endpoints.outputs().len() {
            return;
        }
        self.output = Some(index);
        self.persist_or_notice();
        self.ensure_monitors();
    }

    fn select_quality(&mut self, index: usize) {
        if !matches!(self.session, Session::Idle | Session::Failed(_)) {
            return;
        }
        let Some(quality) = ExportQuality::from_index(index) else {
            return;
        };
        self.quality = quality;
        self.persist_or_notice();
    }

    fn advance_meters(&mut self) {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        if matches!(self.session, Session::Idle | Session::Failed(_)) {
            self.drain_monitors();
            self.microphone_vu.advance(dt);
            self.system_vu.advance(dt);
        } else if matches!(self.session, Session::Recording(_) | Session::Paused(_)) {
            self.microphone_vu.advance(dt);
            self.system_vu.advance(dt);
        }
        self.poll_sidecar();
        self.poll_update();
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
                self.prefs.last_file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                if let Some(plan) = self.save_plan.take() {
                    self.prefs.folder = Some(plan.folder);
                }
                if !self.persist() {
                    self.notice = Some(warn(format!("Saved: {name}. {PREFS_UNSAVED}")));
                }
                self.saved_path = Some(path);
                self.microphone_vu.reset();
                self.system_vu.reset();
                self.ensure_monitors();
            }
            Some(Err(SaveError::NoTake)) => {
                self.save_plan = None;
            }
            Some(Err(SaveError::Write(detail))) => {
                self.save_plan = None;
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
        let phase = self.phase();
        View {
            phase,
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
            levels: if phase.meters_live() {
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
            job_busy: self.sidecar_job.is_some() || self.update_install.is_some(),
            update: self
                .available_update
                .as_ref()
                .map(|release| release.version.to_string()),
        }
    }

    fn transport(&self) -> Transport {
        match &self.session {
            Session::Idle => Transport {
                toggle_label: "Start recording",
                toggle_enabled: self.microphone.is_some()
                    && self.output.is_some()
                    && self.update_install.is_none(),
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
                toggle_enabled: self.update_install.is_none(),
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
            Session::Idle => neutral(""),
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

    fn persist(&mut self) -> bool {
        self.prefs.microphone = self
            .selected_microphone_id()
            .map(|id| id.as_str().to_owned());
        self.prefs.output = self.selected_output_id().map(|id| id.as_str().to_owned());
        self.prefs.quality = self.quality;
        let Some(path) = &self.store else {
            return true;
        };
        self.prefs.write(path).is_ok()
    }

    fn persist_or_notice(&mut self) {
        if !self.persist() {
            self.notice = Some(warn(PREFS_UNSAVED));
        }
    }

    fn phase(&self) -> Phase {
        match self.session {
            Session::Idle => Phase::Idle,
            Session::Recording(_) => Phase::Recording,
            Session::Paused(_) => Phase::Paused,
            Session::AwaitingSave(_) => Phase::AwaitingSave,
            Session::Saving(_) => Phase::Saving,
            Session::Failed(_) => Phase::Failed,
        }
    }

    fn ensure_monitors(&mut self) {
        if self.minimized || !matches!(self.session, Session::Idle | Session::Failed(_)) {
            self.drop_monitors();
            return;
        }
        let mic_id = self.selected_microphone_id();
        if self.mic_monitor_id != mic_id {
            self.mic_monitor = None;
            self.mic_monitor_id = None;
            self.microphone_vu.reset();
            if let Some(id) = &mic_id {
                if let Ok(source) = self.devices.open_microphone(id) {
                    self.mic_monitor = Some(Box::new(self.microphone_vu.tap(source)));
                    self.mic_monitor_id = Some(id.clone());
                }
            }
        }
        let out_id = self.selected_output_id();
        if self.sys_monitor_id != out_id {
            self.sys_monitor = None;
            self.sys_monitor_id = None;
            self.system_vu.reset();
            if let Some(id) = &out_id {
                if let Ok(source) = self.devices.open_loopback(id) {
                    self.sys_monitor = Some(Box::new(self.system_vu.tap(source)));
                    self.sys_monitor_id = Some(id.clone());
                }
            }
        }
    }

    fn take_monitors(
        &mut self,
    ) -> (
        Option<Box<dyn CaptureSource>>,
        Option<Box<dyn CaptureSource>>,
    ) {
        self.mic_monitor_id = None;
        self.sys_monitor_id = None;
        (self.mic_monitor.take(), self.sys_monitor.take())
    }

    fn drop_monitors(&mut self) {
        self.mic_monitor = None;
        self.sys_monitor = None;
        self.mic_monitor_id = None;
        self.sys_monitor_id = None;
    }

    fn drain_monitors(&mut self) {
        drain_source(&mut self.mic_monitor, &mut self.mic_monitor_id);
        drain_source(&mut self.sys_monitor, &mut self.sys_monitor_id);
    }

    fn set_minimized(&mut self, minimized: bool) {
        let restored = self.minimized && !minimized;
        self.minimized = minimized;
        if minimized {
            if matches!(self.session, Session::Idle | Session::Failed(_)) {
                self.drop_monitors();
                self.microphone_vu.reset();
                self.system_vu.reset();
            }
        } else if restored {
            self.ensure_monitors();
            self.advance_meters();
        }
    }
}

fn load_stored_prefs(path: &std::path::Path, vault: &dyn SecretStore) -> Prefs {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let prefs = Prefs::parse(&text);
    if vault::adopt_plaintext(vault, Prefs::legacy_api_key(&text).as_deref()).unwrap_or(false) {
        let _ = prefs.write(path);
    }
    prefs
}

fn drain_source<Id>(source: &mut Option<Box<dyn CaptureSource>>, id: &mut Option<Id>) {
    let Some(capture) = source else {
        return;
    };
    match capture.read(Duration::ZERO) {
        Ok(CaptureRead::Frames(_) | CaptureRead::NoPacket) => {}
        Err(_) => {
            *source = None;
            *id = None;
        }
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
        recorder_with_vault(prefs, store, Box::new(crate::vault::MemoryVault::default()))
    }

    fn recorder_with_vault(
        prefs: Prefs,
        store: Option<PathBuf>,
        vault: Box<dyn SecretStore>,
    ) -> Recorder {
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
            vault,
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

    fn set_settings(shortcut: Shortcut) -> Intent {
        Intent::SetSettings(SettingsValues {
            shortcut,
            ..SettingsValues::default()
        })
    }

    fn ready_prefs() -> Prefs {
        Prefs {
            transcribe_url: Some("https://api.example.com/v1".into()),
            transcribe_model: Some("whisper-1".into()),
            notes_model: Some("notes-1".into()),
            ..Prefs::default()
        }
    }

    #[test]
    fn idle_meters_follow_the_selected_microphone_without_starting() {
        let (mut recorder, opens) = recorder(MicKind::Tone(0.25), false);
        assert_eq!(opens.load(Ordering::SeqCst), 2);
        let view = recorder.apply(Intent::Tick);
        assert_eq!(view.phase, Phase::Idle);
        assert_eq!(view.transport.toggle_label, "Start recording");
        assert!(
            view.levels.microphone.peak >= 0.2,
            "idle mic peak was {}",
            view.levels.microphone.peak
        );
        assert_eq!(view.levels.system.peak, 0.0);
        recorder.apply(Intent::ChooseMicrophone(1));
        assert_eq!(opens.load(Ordering::SeqCst), 3);
        recorder.apply(Intent::ChooseOutput(1));
        assert_eq!(opens.load(Ordering::SeqCst), 4);
        assert_eq!(recorder.apply(Intent::Tick).phase, Phase::Idle);
        stop_take(&mut recorder);
        let stopped = recorder.apply(Intent::Tick);
        assert_eq!(stopped.phase, Phase::AwaitingSave);
        assert_eq!(stopped.levels.microphone.peak, 0.0);
        recorder.apply(Intent::Discard);
        let idle = recorder.apply(Intent::Tick);
        assert_eq!(idle.phase, Phase::Idle);
        assert!(
            idle.levels.microphone.peak >= 0.2,
            "meters should return after discard, peak was {}",
            idle.levels.microphone.peak
        );
        let parked = recorder.apply(Intent::Minimized(true));
        assert_eq!(parked.levels.microphone.peak, 0.0);
        assert_eq!(parked.levels.system.peak, 0.0);
        assert_eq!(recorder.apply(Intent::Tick).levels.microphone.peak, 0.0);
        let opens_while_hidden = opens.load(Ordering::SeqCst);
        recorder.apply(Intent::Tick);
        assert_eq!(opens.load(Ordering::SeqCst), opens_while_hidden);
        let restored = recorder.apply(Intent::Minimized(false));
        assert_eq!(restored.phase, Phase::Idle);
        assert!(
            restored.levels.microphone.peak >= 0.2,
            "meters should resume after restore, peak was {}",
            restored.levels.microphone.peak
        );
        assert!(opens.load(Ordering::SeqCst) > opens_while_hidden);
    }

    #[test]
    fn picker_change_does_not_start_recording() {
        let (mut recorder, opens) = recorder(MicKind::Tone(0.25), false);
        let view = recorder.apply(Intent::ChooseMicrophone(1));
        assert_eq!(view.microphone.selected, Some(1));
        assert_eq!(view.transport.toggle_label, "Start recording");
        assert_eq!(view.phase, Phase::Idle);
        assert!(opens.load(Ordering::SeqCst) >= 2);
        let view = recorder.apply(Intent::ChooseOutput(1));
        assert_eq!(view.output.selected, Some(1));
        assert_eq!(opens.load(Ordering::SeqCst), 4);
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
        assert!(opens.load(Ordering::SeqCst) >= 1);
        assert_eq!(view.phase, Phase::Idle);
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
            prompt.file_name.ends_with("Recording.mp3"),
            "{}",
            prompt.file_name
        );
        assert_eq!(prompt.filter_label, "MP3 audio");
        assert_eq!(prompt.extension, "mp3");
        assert_eq!(prompt.folder, None);
        assert!(prompt.warn_if_exists);
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
                shortcut: Shortcut::default(),
                last_file_name: None,
                ..Prefs::default()
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
                shortcut: Shortcut::default(),
                last_file_name: None,
                ..Prefs::default()
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
    fn save_prompt_uses_recording_name_and_last_folder() {
        let folder = tempfile::tempdir().unwrap();
        let mut recorder = recorder_with_prefs(
            Prefs {
                microphone: None,
                output: None,
                quality: ExportQuality::High,
                folder: Some(folder.path().to_path_buf()),
                shortcut: Shortcut::default(),
                last_file_name: None,
                ..Prefs::default()
            },
            None,
        );
        stop_take(&mut recorder);
        let asked = recorder.apply(Intent::Save);
        let Ask::SaveDestination(prompt) = asked.ask.expect("save prompt") else {
            panic!("expected a save prompt");
        };
        assert!(
            prompt.file_name.ends_with("Recording.mp3"),
            "{}",
            prompt.file_name
        );
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
        assert_eq!(opens.load(Ordering::SeqCst), 2);
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
    fn settings_report_persistence_failure_and_clear_stale_hotkey_warning() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(dir.path().to_path_buf()));
        recorder.apply(Intent::HotkeyUnavailable);
        let view = recorder.apply(set_settings(Shortcut(0)));
        assert!(view.status.text.contains("could not be saved"));
        assert_eq!(recorder.shortcut(), Shortcut(0));
        recorder.store = None;
        recorder.apply(Intent::HotkeyUnavailable);
        let view = recorder.apply(set_settings(Shortcut(0)));
        assert!(view.status.text.is_empty());
    }

    #[test]
    fn choosing_quality_reports_persistence_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(dir.path().to_path_buf()));
        let view = recorder.apply(Intent::ChooseQuality(ExportQuality::High.index()));
        assert!(view.status.text.contains("could not be saved"));
        assert_eq!(view.quality.selected, Some(ExportQuality::High.index()));
    }

    #[test]
    fn successful_save_keeps_the_file_notice_when_prefs_cannot_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(dir.path().to_path_buf()));
        stop_take(&mut recorder);
        let path = dir.path().join("meeting.mp3");
        recorder.apply(Intent::SaveTo(path.clone()));
        let saved = finish_save(&mut recorder);
        assert_eq!(saved.phase, Phase::Idle);
        assert_eq!(saved.saved_path, Some(path));
        assert!(
            saved.status.text.starts_with("Saved: meeting.mp3"),
            "{}",
            saved.status.text
        );
        assert!(
            saved.status.text.contains("could not be saved"),
            "{}",
            saved.status.text
        );
    }

    #[test]
    fn settings_persist_shortcut_changes_and_disabled_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onerec.ini");
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(path.clone()));
        for shortcut in [Shortcut(0x0346), Shortcut(0)] {
            recorder.apply(set_settings(shortcut));
            let loaded = Prefs::read(&path);
            assert_eq!(loaded.shortcut, shortcut);
            assert_eq!(recorder.shortcut(), shortcut);
            let Ask::Settings(asked) = recorder.apply(Intent::OpenSettings).ask.expect("settings")
            else {
                panic!("expected settings");
            };
            assert_eq!(asked.shortcut, shortcut);
            assert!(asked.api_key.is_empty());
        }
    }

    #[test]
    fn settings_can_be_opened_and_changed_without_interrupting_a_take() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Start);
        wait_mix();
        for (intent, expected) in [
            (Intent::Tick, Phase::Recording),
            (Intent::Pause, Phase::Paused),
            (Intent::Stop, Phase::AwaitingSave),
        ] {
            recorder.apply(intent);
            let view = recorder.apply(Intent::OpenSettings);
            assert_eq!(view.phase, expected);
            assert!(matches!(view.ask, Some(Ask::Settings(_))));
            let updated = recorder.apply(set_settings(Shortcut(0)));
            assert_eq!(updated.phase, expected);
            assert_eq!(recorder.shortcut(), Shortcut(0));
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
            "The recording shortcut is unavailable. Change it in Settings or use Record / Stop."
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
    fn timer_stops_meters_when_minimized_but_keeps_export_polling() {
        for phase in [Phase::Idle, Phase::Recording, Phase::Paused, Phase::Failed] {
            assert_eq!(phase.timer_ms(false, false), Some(50));
            assert_eq!(phase.timer_ms(true, false), None);
            assert_eq!(phase.timer_ms(true, true), Some(50));
        }
        assert_eq!(Phase::AwaitingSave.timer_ms(false, false), None);
        assert_eq!(Phase::AwaitingSave.timer_ms(true, false), None);
        assert_eq!(Phase::Saving.timer_ms(true, false), Some(50));
        assert_eq!(Phase::AwaitingSave.timer_ms(true, true), Some(50));
    }

    #[test]
    fn successful_save_remembers_exact_name_and_folder_across_restart() {
        let folder = tempfile::tempdir().unwrap();
        let store = folder.path().join("onerec.ini");
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(store.clone()));
        stop_take(&mut recorder);
        let path = folder.path().join("Reunión de equipo 01.mp3");
        recorder.apply(Intent::SaveTo(path.clone()));
        finish_save(&mut recorder);
        let loaded = Prefs::read(&store);
        assert_eq!(
            loaded.last_file_name.as_deref(),
            Some("Reunión de equipo 01.mp3")
        );
        assert_eq!(loaded.folder.as_deref(), Some(folder.path()));
        let mut recorder = recorder_with_prefs(loaded, Some(store));
        stop_take(&mut recorder);
        let asked = recorder.apply(Intent::Save);
        let Some(Ask::SaveDestination(prompt)) = asked.ask else {
            panic!("Save must ask");
        };
        assert_eq!(prompt.file_name, "Reunión de equipo 01.mp3");
        assert_eq!(prompt.folder.as_deref(), Some(folder.path()));
        assert_eq!(asked.phase, Phase::AwaitingSave);
        let original = std::fs::read(&path).unwrap();
        recorder.apply(Intent::CancelSave);
        assert_eq!(
            recorder.prefs.last_file_name.as_deref(),
            Some("Reunión de equipo 01.mp3")
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn failed_save_does_not_replace_the_remembered_name() {
        let folder = tempfile::tempdir().unwrap();
        let prefs = Prefs {
            last_file_name: Some("Successful.mp3".into()),
            ..Prefs::default()
        };
        let mut recorder = recorder_with_prefs(prefs, None);
        stop_take(&mut recorder);
        let blocker = folder.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap();
        recorder.apply(Intent::SaveTo(blocker.join("Failed.mp3")));
        for _ in 0..100 {
            let view = recorder.apply(Intent::Tick);
            if view.phase != Phase::Saving {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            recorder.prefs.last_file_name.as_deref(),
            Some("Successful.mp3")
        );
        let Some(Ask::SaveDestination(prompt)) = recorder.apply(Intent::Save).ask else {
            panic!("retry dialog");
        };
        assert_eq!(prompt.file_name, "Successful.mp3");
    }

    #[test]
    fn transcribe_without_a_key_opens_settings_after_save() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        stop_take(&mut recorder);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("take.mp3");
        recorder.apply(Intent::SaveTo(dest));
        let saved = finish_save(&mut recorder);
        assert!(!saved.job_busy);
        let asked = recorder.apply(Intent::Sidecar(JobKind::Transcript));
        assert!(matches!(asked.ask, Some(Ask::Settings(_))));
        assert_eq!(asked.status.text, "No API key");
        assert!(!asked.job_busy);
    }

    #[test]
    fn transcribe_writes_a_markdown_sidecar_from_the_saved_mp3() {
        let folder = tempfile::tempdir().unwrap();
        let vault = crate::vault::MemoryVault::from_key("gsk_test");
        let mut recorder = recorder_with_vault(ready_prefs(), None, Box::new(vault));
        recorder.sidecar = |job| {
            assert_eq!(job.kind, JobKind::Transcript);
            let dest = sidecar::path(&job.audio, sidecar::SidecarKind::Transcript);
            std::fs::write(&dest, "# Reunión\n\nhola reunión\n").unwrap();
            Ok(dest)
        };
        stop_take(&mut recorder);
        let path = folder.path().join("Reunión.mp3");
        recorder.apply(Intent::SaveTo(path.clone()));
        finish_save(&mut recorder);
        let started = recorder.apply(Intent::Sidecar(JobKind::Transcript));
        assert!(started.job_busy);
        assert!(started.status.text.contains("Transcribing"));
        let mut view = started;
        for _ in 0..100 {
            if !view.job_busy {
                break;
            }
            thread::sleep(Duration::from_millis(5));
            view = recorder.apply(Intent::Tick);
        }
        assert!(!view.job_busy);
        assert_eq!(view.status.text, "Transcribed: Reunión.md");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("md")).unwrap(),
            "# Reunión\n\nhola reunión\n"
        );
    }

    #[test]
    fn notes_write_a_sidecar_next_to_the_mp3() {
        let folder = tempfile::tempdir().unwrap();
        let vault = crate::vault::MemoryVault::from_key("gsk_test");
        let mut recorder = recorder_with_vault(ready_prefs(), None, Box::new(vault));
        recorder.sidecar = |job| {
            assert_eq!(job.kind, JobKind::Notes);
            let dest = sidecar::path(&job.audio, sidecar::SidecarKind::Notes);
            std::fs::write(&dest, "## Summary\n\nDone.\n").unwrap();
            Ok(dest)
        };
        stop_take(&mut recorder);
        let path = folder.path().join("Meeting.mp3");
        recorder.apply(Intent::SaveTo(path.clone()));
        finish_save(&mut recorder);
        let started = recorder.apply(Intent::Sidecar(JobKind::Notes));
        assert!(started.job_busy);
        assert_eq!(started.status.text, "Writing notes…");
        let mut view = started;
        for _ in 0..100 {
            if !view.job_busy {
                break;
            }
            thread::sleep(Duration::from_millis(5));
            view = recorder.apply(Intent::Tick);
        }
        assert!(!view.job_busy);
        assert_eq!(view.status.text, "Notes: Meeting.notes.md");
        assert_eq!(
            std::fs::read_to_string(folder.path().join("Meeting.notes.md")).unwrap(),
            "## Summary\n\nDone.\n"
        );
    }

    #[test]
    fn settings_persist_transcription_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onerec.ini");
        let vault = crate::vault::MemoryVault::default();
        let inspect = vault.clone();
        let mut recorder =
            recorder_with_vault(Prefs::default(), Some(path.clone()), Box::new(vault));
        recorder.apply(Intent::SetSettings(SettingsValues {
            shortcut: Shortcut::default(),
            api_key: " gsk_live ".into(),
            transcribe_url: "https://api.openai.com/v1/".into(),
            transcribe_model: "whisper-1".into(),
            notes_model: "llama-3.1-8b-instant".into(),
            nest: false,
            notes_prompt: String::new(),
        }));
        let loaded = Prefs::read(&path);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("api_key="));
        assert_eq!(inspect.load().unwrap().as_deref(), Some("gsk_live"));
        assert_eq!(
            loaded.transcribe_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(loaded.transcribe_model.as_deref(), Some("whisper-1"));
        assert_eq!(loaded.notes_model.as_deref(), Some("llama-3.1-8b-instant"));
        recorder.apply(Intent::SetSettings(SettingsValues {
            shortcut: Shortcut::default(),
            api_key: String::new(),
            transcribe_url: "https://api.openai.com/v1".into(),
            transcribe_model: "whisper-1".into(),
            notes_model: "llama-3.1-8b-instant".into(),
            nest: false,
            notes_prompt: String::new(),
        }));
        assert_eq!(inspect.load().unwrap(), None);
    }

    #[test]
    fn empty_settings_save_omits_endpoint_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onerec.ini");
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(path.clone()));
        recorder.apply(Intent::SetSettings(SettingsValues {
            shortcut: Shortcut::default(),
            api_key: "gsk_live".into(),
            transcribe_url: String::new(),
            transcribe_model: String::new(),
            notes_model: String::new(),
            nest: false,
            notes_prompt: String::new(),
        }));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("api_key="));
        assert!(!text.contains("transcribe_url="));
        assert!(!text.contains("transcribe_model="));
        assert!(!text.contains("notes_model="));
        assert!(!text.contains("chat_model="));
        assert!(text.lines().any(|line| line == "nest=0"));
    }

    #[test]
    fn typed_groq_url_is_written_as_that_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onerec.ini");
        let mut recorder = recorder_with_prefs(Prefs::default(), Some(path.clone()));
        recorder.apply(Intent::SetSettings(SettingsValues {
            shortcut: Shortcut::default(),
            api_key: String::new(),
            transcribe_url: "https://api.groq.com/openai/v1".into(),
            transcribe_model: String::new(),
            notes_model: String::new(),
            nest: false,
            notes_prompt: String::new(),
        }));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("transcribe_url=https://api.groq.com/openai/v1"));
        assert!(!text.contains("transcribe_model="));
        assert!(!text.contains("notes_model="));
        assert_eq!(
            Prefs::read(&path).transcribe_url.as_deref(),
            Some("https://api.groq.com/openai/v1")
        );
    }

    #[test]
    fn leftover_ini_key_migrates_into_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onerec.ini");
        std::fs::write(&path, "quality=Voice\napi_key=gsk_legacy\n").unwrap();
        let vault = crate::vault::MemoryVault::default();
        let inspect = vault.clone();
        let prefs = load_stored_prefs(&path, &vault);
        let _recorder = recorder_with_vault(prefs, Some(path.clone()), Box::new(vault));
        assert_eq!(inspect.load().unwrap().as_deref(), Some("gsk_legacy"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("api_key="));
        assert!(!Prefs::read(&path).render().contains("api_key="));
    }

    #[test]
    fn busy_sidecar_click_is_a_silent_noop() {
        let folder = tempfile::tempdir().unwrap();
        let vault = crate::vault::MemoryVault::from_key("gsk_test");
        let mut recorder = recorder_with_vault(ready_prefs(), None, Box::new(vault));
        recorder.sidecar = |_| {
            thread::sleep(Duration::from_millis(80));
            Err("should not run twice".into())
        };
        stop_take(&mut recorder);
        recorder.apply(Intent::SaveTo(folder.path().join("take.mp3")));
        finish_save(&mut recorder);
        let started = recorder.apply(Intent::Sidecar(JobKind::Transcript));
        assert!(started.job_busy);
        let ignored = recorder.apply(Intent::Sidecar(JobKind::Notes));
        assert!(ignored.job_busy);
        assert_eq!(ignored.status.text, started.status.text);
        for _ in 0..40 {
            if !recorder.apply(Intent::Tick).job_busy {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn nest_save_writes_the_take_folder_and_remembers_the_dialog_parent() {
        let dest_dir = tempfile::tempdir().unwrap();
        let dialog = dest_dir.path().join("stem.mp3");
        let nested = dest_dir.path().join("stem").join("stem.mp3");
        let mut recorder = recorder_with_prefs(
            Prefs {
                nest: true,
                ..Prefs::default()
            },
            None,
        );
        stop_take(&mut recorder);
        let asked = recorder.apply(Intent::Save);
        let Ask::SaveDestination(prompt) = asked.ask.expect("save prompt") else {
            panic!("expected a save prompt");
        };
        assert!(!prompt.warn_if_exists);
        recorder.apply(Intent::SaveTo(dialog.clone()));
        let saved = finish_save(&mut recorder);
        assert!(nested.exists());
        assert!(!dialog.exists());
        assert_eq!(saved.saved_path.as_deref(), Some(nested.as_path()));
        assert_eq!(recorder.prefs.folder.as_deref(), Some(dest_dir.path()));
        assert_eq!(recorder.prefs.last_file_name.as_deref(), Some("stem.mp3"));
    }

    #[test]
    fn nest_overwrite_asks_then_confirm_does_not_replan() {
        let dest_dir = tempfile::tempdir().unwrap();
        let dialog = dest_dir.path().join("stem.mp3");
        let nested = dest_dir.path().join("stem").join("stem.mp3");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, b"old").unwrap();
        let mut recorder = recorder_with_prefs(
            Prefs {
                nest: true,
                ..Prefs::default()
            },
            None,
        );
        stop_take(&mut recorder);
        let asked = recorder.apply(Intent::SaveTo(dialog));
        match asked.ask {
            Some(Ask::OverwriteNested { path }) => assert_eq!(path, nested),
            other => panic!("expected nested overwrite, got {other:?}"),
        }
        recorder.apply(Intent::ConfirmNestedSave);
        let saved = finish_save(&mut recorder);
        assert_eq!(saved.saved_path.as_deref(), Some(nested.as_path()));
        assert_eq!(recorder.prefs.folder.as_deref(), Some(dest_dir.path()));
        assert_ne!(std::fs::read(&nested).unwrap(), b"old");
    }

    fn newer_release() -> crate::update::Release {
        crate::update::Release {
            version: crate::update::Version::parse("9.9.9").unwrap(),
            download_url: "https://example.invalid/onerec.zip".into(),
            zip: true,
        }
    }

    fn wait_offer(recorder: &mut Recorder) -> View {
        let mut view = recorder.apply(Intent::Tick);
        for _ in 0..100 {
            if view.update.is_some() {
                return view;
            }
            thread::sleep(Duration::from_millis(5));
            view = recorder.apply(Intent::Tick);
        }
        panic!("update was not offered: {}", view.status.text)
    }

    #[test]
    fn a_newer_release_is_offered_on_the_idle_status_line() {
        let mut recorder = recorder_with_prefs(Prefs::default(), None);
        recorder.check_update = || Ok(Some(newer_release()));
        recorder.start_update_check();
        let view = wait_offer(&mut recorder);
        assert_eq!(view.update.as_deref(), Some("9.9.9"));
        assert_eq!(view.status.text, "onerec 9.9.9 is available.");
        assert!(!view.job_busy);
    }

    #[test]
    fn a_current_release_is_ignored_silently() {
        let mut recorder = recorder_with_prefs(Prefs::default(), None);
        recorder.check_update = || Ok(None);
        recorder.start_update_check();
        let mut view = recorder.apply(Intent::Tick);
        for _ in 0..20 {
            thread::sleep(Duration::from_millis(5));
            view = recorder.apply(Intent::Tick);
        }
        assert_eq!(view.update, None);
        assert_eq!(view.status.text, "");
    }

    #[test]
    fn update_asks_to_confirm_when_idle_and_refuses_during_a_take() {
        let mut recorder = recorder_with_prefs(Prefs::default(), None);
        recorder.check_update = || Ok(Some(newer_release()));
        recorder.start_update_check();
        wait_offer(&mut recorder);
        let asked = recorder.apply(Intent::RequestUpdate);
        match asked.ask {
            Some(Ask::ConfirmUpdate { version }) => assert_eq!(version, "9.9.9"),
            other => panic!("expected confirm, got {other:?}"),
        }
        recorder.apply(Intent::Toggle);
        wait_mix();
        let refused = recorder.apply(Intent::RequestUpdate);
        assert_eq!(refused.phase, Phase::Recording);
        assert!(refused.ask.is_none());
        assert_eq!(refused.update.as_deref(), Some("9.9.9"));
    }

    #[test]
    fn a_successful_update_relaunches_and_a_failed_one_can_be_retried() {
        let mut recorder = recorder_with_prefs(Prefs::default(), None);
        recorder.check_update = || Ok(Some(newer_release()));
        recorder.install_update = |_| Err("the onerec folder is not writable".into());
        recorder.start_update_check();
        wait_offer(&mut recorder);
        let started = recorder.apply(Intent::InstallUpdate);
        assert!(started.job_busy);
        assert!(started.status.text.contains("Downloading"));
        let mut view = started;
        for _ in 0..100 {
            if !view.job_busy {
                break;
            }
            thread::sleep(Duration::from_millis(5));
            view = recorder.apply(Intent::Tick);
        }
        assert!(!view.job_busy);
        assert!(view.status.text.contains("not writable"));
        assert_eq!(view.update.as_deref(), Some("9.9.9"));
        assert!(view.ask.is_none());

        recorder.install_update = |_| Ok(());
        let started = recorder.apply(Intent::InstallUpdate);
        assert!(started.job_busy);
        let mut view = started;
        for _ in 0..100 {
            if matches!(view.ask, Some(Ask::Relaunch)) || view.update.is_none() && !view.job_busy {
                break;
            }
            thread::sleep(Duration::from_millis(5));
            view = recorder.apply(Intent::Tick);
        }
        assert!(matches!(view.ask, Some(Ask::Relaunch)));
        assert_eq!(view.update, None);
        assert_eq!(view.status.text, "Updated. Restarting…");
    }

    #[test]
    fn recording_is_blocked_while_an_update_downloads() {
        let mut recorder = recorder_with_prefs(Prefs::default(), None);
        recorder.check_update = || Ok(Some(newer_release()));
        recorder.install_update = |_| {
            thread::sleep(Duration::from_millis(200));
            Ok(())
        };
        recorder.start_update_check();
        wait_offer(&mut recorder);
        let started = recorder.apply(Intent::InstallUpdate);
        assert!(started.job_busy);
        assert!(!started.transport.toggle_enabled);
        let refused = recorder.apply(Intent::Toggle);
        assert_eq!(refused.phase, Phase::Idle);
        assert!(refused.status.text.contains("Wait for the update"));
    }
}
