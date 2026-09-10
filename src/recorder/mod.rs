// Window wiring in unit 3 is the first non-test caller.
#![allow(dead_code)]

mod level;
mod wasapi;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::audio::{AudioError, Endpoint, Endpoints};
use crate::capture::CaptureSource;
use crate::ids::{MicrophoneId, OutputDeviceId};
use crate::session::{SaveError, Session};
use crate::staging::StagingArea;

use self::level::{Level, Vu};

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
    Toggle,
    Save,
    SaveTo(PathBuf),
    CancelSave,
    Discard,
    Closing,
}

#[derive(Debug)]
pub(crate) struct View {
    pub endpoints: Option<EndpointLists>,
    pub microphone: Selector,
    pub output: Selector,
    pub transport: Transport,
    pub elapsed: String,
    pub levels: Levels,
    pub status: Status,
    pub ask: Option<Ask>,
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
    Close,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SavePrompt {
    pub suggested_file_name: String,
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
    session: Session,
    microphone_vu: Vu,
    system_vu: Vu,
    notice: Option<Status>,
    last_tick: Instant,
    pending_ask: Option<Ask>,
}

impl Recorder {
    pub(crate) fn new(staging: StagingArea) -> Self {
        Self::with_devices(Box::new(wasapi::Wasapi), staging)
    }

    pub(crate) fn with_devices(devices: Box<dyn Devices>, staging: StagingArea) -> Self {
        let mut notice = None;
        let endpoints = match devices.survey() {
            Ok(endpoints) => endpoints,
            Err(error) => {
                notice = Some(warn(error.to_string()));
                Endpoints::from_enumerated(Vec::new(), Vec::new(), None, None)
            }
        };
        let microphone = pick_index(
            endpoints.microphones(),
            None,
            endpoints.default_microphone(),
        );
        let output = pick_index(endpoints.outputs(), None, endpoints.default_output());
        Self {
            devices,
            staging,
            endpoints,
            endpoints_dirty: true,
            microphone,
            output,
            session: Session::Idle,
            microphone_vu: Vu::new(),
            system_vu: Vu::new(),
            notice,
            last_tick: Instant::now(),
            pending_ask: None,
        }
    }

    pub(crate) fn apply(&mut self, intent: Intent) -> View {
        match intent {
            Intent::Tick => self.advance_meters(),
            Intent::RefreshEndpoints => self.refresh(),
            Intent::ChooseMicrophone(index) => self.select_microphone(index),
            Intent::ChooseOutput(index) => self.select_output(index),
            Intent::Toggle => self.toggle(),
            Intent::Save => self.request_destination(),
            Intent::SaveTo(path) => self.save_to(&path),
            Intent::CancelSave => self.cancel_save(),
            Intent::Discard => self.discard(),
            Intent::Closing => self.consider_closing(),
        }
        self.view()
    }

    fn toggle(&mut self) {
        if matches!(self.session, Session::Idle) {
            self.begin();
        } else if matches!(self.session, Session::Recording(_)) {
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
        self.session.start(
            mic_id,
            out_id,
            self.microphone_vu.tap(mic),
            self.system_vu.tap(sys),
            staging,
        );
        self.dismiss_failed_start();
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
        self.pending_ask = Some(Ask::SaveDestination(SavePrompt {
            suggested_file_name: "onerec.wav".into(),
            filter_label: "Waveform audio",
            extension: "wav",
        }));
    }

    fn save_to(&mut self, destination: &std::path::Path) {
        match self.session.save_as(destination) {
            Ok(()) => {
                self.notice = Some(neutral(format!("Saved to {}.", destination.display())));
                self.microphone_vu.reset();
                self.system_vu.reset();
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
        if matches!(self.session, Session::Recording(_)) {
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
        if !matches!(self.session, Session::Idle) {
            return;
        }
        if index >= self.endpoints.microphones().len() {
            return;
        }
        self.microphone = Some(index);
    }

    fn select_output(&mut self, index: usize) {
        if !matches!(self.session, Session::Idle) {
            return;
        }
        if index >= self.endpoints.outputs().len() {
            return;
        }
        self.output = Some(index);
    }

    fn advance_meters(&mut self) {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        self.microphone_vu.advance(dt);
        self.system_vu.advance(dt);
    }

    fn consider_closing(&mut self) {
        match &self.session {
            Session::Idle | Session::Failed(_) => {
                self.pending_ask = Some(Ask::Close);
            }
            Session::Recording(_) | Session::AwaitingSave(_) => {
                self.notice = Some(warn("Save or discard this take before closing."));
            }
        }
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
            endpoints,
            microphone: Selector {
                selected: self.microphone,
                enabled: pickers_enabled,
            },
            output: Selector {
                selected: self.output,
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
            status: self.notice.clone().unwrap_or_else(|| self.derived_status()),
            ask: self.pending_ask.take(),
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
            Session::Recording(_) => Transport {
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
            Session::AwaitingSave(_) => {
                neutral("Take ready. Save or discard it before the next one.")
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

    fn wait_mix() {
        thread::sleep(Duration::from_millis(50));
    }

    fn stop_take(recorder: &mut Recorder) -> View {
        recorder.apply(Intent::Toggle);
        wait_mix();
        recorder.apply(Intent::Toggle)
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
        assert_eq!(
            asked.ask,
            Some(Ask::SaveDestination(SavePrompt {
                suggested_file_name: "onerec.wav".into(),
                filter_label: "Waveform audio",
                extension: "wav",
            }))
        );
        assert_eq!(recorder.apply(Intent::Tick).ask, None);
        let dest = tempfile::tempdir().unwrap();
        let path = dest.path().join("take.wav");
        let saved = recorder.apply(Intent::SaveTo(path.clone()));
        assert_eq!(saved.transport.toggle_label, "Start recording");
        assert!(saved.transport.toggle_enabled);
        assert!(!saved.transport.save_enabled);
        assert!(path.exists());
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
    fn closing_while_recording_warns_without_destroying() {
        let (mut recorder, _) = recorder(MicKind::Tone(0.25), false);
        recorder.apply(Intent::Toggle);
        let view = recorder.apply(Intent::Closing);
        assert_eq!(view.ask, None);
        assert_eq!(view.transport.toggle_label, "Stop recording");
        assert_eq!(
            view.status.text,
            "Save or discard this take before closing."
        );
        assert_eq!(view.status.tone, Tone::Warning);
        recorder.apply(Intent::Toggle);
    }
}
