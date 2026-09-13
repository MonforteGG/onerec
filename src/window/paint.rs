use super::buffered::PaintBuffer;
use super::rounded::RoundedButtons;
use super::theme::Theme;
use crate::mp3::ExportQuality;
use crate::recorder::{Phase, Selector, Status, Tone, View};
use crate::RunError;
use std::cell::Cell;
use std::ffi::c_void;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForWindow, SystemParametersInfoForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, IsWindowEnabled, SetFocus,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) const CLIENT_WIDTH: i32 = 480;
pub(crate) const CLIENT_HEIGHT: i32 = 372;
const TRANSPORT_DIVIDER_Y: i32 = 164;
pub(crate) const ID_MICROPHONE: u16 = 101;
pub(crate) const ID_OUTPUT: u16 = 102;
pub(crate) const ID_TOGGLE: u16 = 103;
pub(crate) const ID_SAVE: u16 = 104;
pub(crate) const ID_DISCARD: u16 = 105;
pub(crate) const ID_QUALITY: u16 = 106;
pub(crate) const ID_FOLDER: u16 = 107;
pub(crate) const ID_REFRESH: u16 = 108;
pub(crate) const ID_PAUSE: u16 = 110;
pub(crate) const ID_SETTINGS: u16 = 111;
pub(crate) const ID_STOP: u16 = 113;
pub(crate) const ID_TRANSCRIBE: u16 = 114;
pub(crate) const ID_NOTES: u16 = 115;
const MICROPHONE_Y: i32 = 176;

pub(crate) struct Controls {
    microphones: HWND,
    outputs: HWND,
    quality: HWND,
    quality_label: HWND,
    microphone_level: HWND,
    output_level: HWND,
    toggle: HWND,
    stop: HWND,
    save: HWND,
    discard: HWND,
    pause: HWND,
    settings: HWND,
    folder: HWND,
    refresh: HWND,
    transcribe: HWND,
    notes: HWND,
    elapsed: HWND,
    heading: HWND,
    status: HWND,
    progress: HWND,
    progress_label: HWND,
    fonts: Fonts,
    theme: Theme,
    rounded: RoundedButtons,
    hovered_button: Cell<Option<HWND>>,
    dpi: u32,
    units: u32,
    painted: Painted,
    list_dropped: bool,
    meter_top: [i32; 2],
}

#[derive(Default)]
struct Painted {
    phase: Option<Phase>,
    elapsed: String,
    heading: String,
    status: Option<Status>,
    meters: [Meter; 2],
    percent: Option<u32>,
    saved_file: bool,
    missing_devices: bool,
    job_busy: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Meter {
    width: i32,
    clipping: bool,
    db: Option<i32>,
}
struct Fonts {
    body: HFONT,
    strong: HFONT,
    timer: HFONT,
    icons: HFONT,
    icon_resource: HANDLE,
    units: u32,
}

impl Controls {
    pub(crate) fn create(root: HWND, instance: HINSTANCE) -> Result<Self, RunError> {
        unsafe {
            let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_STANDARD_CLASSES | ICC_PROGRESS_CLASS,
            });
        }
        let dpi = unsafe { GetDpiForWindow(root) }.max(96);
        let fonts = Fonts::new(dpi);
        let microphones = combo(root, instance, ID_MICROPHONE, MICROPHONE_Y)?;
        let outputs = combo(root, instance, ID_OUTPUT, 202)?;
        name_device_controls(microphones, outputs);
        control_tooltip(root, instance, microphones, w!("Microphone"))?;
        control_tooltip(root, instance, outputs, w!("System audio"))?;
        let quality_label = static_text(root, instance, w!("MP3 &quality"), 0)?;
        let quality = combo(root, instance, ID_QUALITY, 254)?;
        let toggle = button(root, instance, ID_TOGGLE, w!("&Record"))?;
        let stop = button(root, instance, ID_STOP, w!("S&top"))?;
        let save = button(root, instance, ID_SAVE, w!("&Save"))?;
        let discard = button(root, instance, ID_DISCARD, w!("&Discard…"))?;
        let pause = button(root, instance, ID_PAUSE, w!("&Pause"))?;
        let settings = button(root, instance, ID_SETTINGS, w!("Settin&gs"))?;
        control_tooltip(root, instance, settings, w!("Settings (Alt+G)"))?;
        control_tooltip(root, instance, discard, w!("Discard recording (Alt+D)"))?;
        let folder = button(root, instance, ID_FOLDER, w!("Open &folder"))?;
        control_tooltip(root, instance, folder, w!("Open folder (Alt+F)"))?;
        let refresh = button(root, instance, ID_REFRESH, w!("Re&fresh devices"))?;
        let transcribe = button(root, instance, ID_TRANSCRIBE, w!("Transcri&be"))?;
        control_tooltip(
            root,
            instance,
            transcribe,
            w!("Transcribe with your API key (Alt+B)"),
        )?;
        let notes = button(root, instance, ID_NOTES, w!("&Notes"))?;
        control_tooltip(
            root,
            instance,
            notes,
            w!("Write notes from the recording (Alt+N)"),
        )?;
        let mut controls = Self {
            microphones,
            outputs,
            quality,
            quality_label,
            microphone_level: static_text(root, instance, w!(""), 2)?,
            output_level: static_text(root, instance, w!(""), 2)?,
            toggle,
            stop,
            save,
            discard,
            pause,
            settings,
            folder,
            refresh,
            transcribe,
            notes,
            elapsed: static_text(root, instance, w!("00:00"), 0)?,
            heading: static_text(root, instance, w!("Ready"), 0)?,
            status: static_text(
                root,
                instance,
                w!(""),
                0x0080 | 0x4000, // SS_EDITCONTROL | SS_ENDELLIPSIS
            )?,
            progress: child(
                root,
                instance,
                PROGRESS_CLASSW,
                w!("MP3 export progress"),
                WINDOW_STYLE(0),
                109,
            )?,
            progress_label: static_text(root, instance, w!(""), 0)?,
            units: fonts.units,
            fonts,
            theme: Theme::new(),
            rounded: RoundedButtons::new(),
            hovered_button: Cell::new(None),
            dpi,
            painted: Painted::default(),
            list_dropped: false,
            meter_top: [0; 2],
        };
        for control in [
            folder,
            refresh,
            transcribe,
            notes,
            controls.progress,
            controls.progress_label,
        ] {
            visible(control, false);
        }
        controls.apply_fonts();
        refill(
            controls.quality,
            &ExportQuality::ALL.map(|q| q.label().to_owned()),
        );
        controls.layout(root);
        Ok(controls)
    }

    fn all(&self) -> [HWND; 21] {
        [
            self.microphones,
            self.outputs,
            self.quality_label,
            self.quality,
            self.microphone_level,
            self.output_level,
            self.toggle,
            self.stop,
            self.save,
            self.discard,
            self.pause,
            self.settings,
            self.folder,
            self.refresh,
            self.transcribe,
            self.notes,
            self.elapsed,
            self.heading,
            self.status,
            self.progress,
            self.progress_label,
        ]
    }

    fn apply_fonts(&self) {
        for control in self.all() {
            let font = if control == self.elapsed {
                self.fonts.timer
            } else if [self.toggle, self.save, self.heading].contains(&control) {
                self.fonts.strong
            } else {
                self.fonts.body
            };
            unsafe {
                SendMessageW(control, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            }
        }
    }

    pub(crate) fn refresh_theme(&mut self, root: HWND, dpi: u32) {
        self.dpi = dpi.max(96);
        let old_fonts = std::mem::replace(&mut self.fonts, Fonts::new(self.dpi));
        self.units = self.fonts.units;
        self.theme = Theme::new();
        self.apply_fonts();
        drop(old_fonts);
        self.painted.meters = [Meter::default(); 2];
        self.layout(root);
        unsafe {
            let _ = InvalidateRect(root, None, true);
        }
    }

    fn s(&self, value: i32) -> i32 {
        scale(value, self.units)
    }

    fn layout(&mut self, root: HWND) {
        let place = |control, x, y, width, height| unsafe {
            let _ = SetWindowPos(
                control,
                None,
                self.s(x),
                self.s(y),
                self.s(width),
                self.s(height),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        };
        let place_combo = |control, x, y, width, height| unsafe {
            let _ = SetWindowPos(
                control,
                None,
                self.s(x),
                self.s(y),
                self.s(width),
                self.s(height),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
            );
        };
        place(self.heading, 20, 12, 392, 20);
        place(self.elapsed, 20, 36, 392, 44);
        place(self.settings, 428, 12, 32, 32);
        place(self.toggle, 20, 108, 96, 40);
        place(self.pause, 124, 108, 96, 40);
        place(self.stop, 228, 108, 88, 40);
        place(self.save, 324, 108, 88, 40);
        place(self.discard, 420, 108, 40, 40);
        place_combo(self.microphones, 20, MICROPHONE_Y, 440, 220);
        place_combo(self.outputs, 20, 230, 440, 220);
        place(self.quality_label, 20, 287, 136, 20);
        place_combo(self.quality, 178, 282, 282, 220);
        place(self.progress_label, 20, 282, 440, 20);
        place(self.progress, 20, 306, 440, 10);
        let text_width = if self.painted.saved_file {
            192
        } else if self.painted.missing_devices {
            288
        } else {
            440
        };
        let height = self.status_height(root, self.s(text_width)).max(self.s(34));
        unsafe {
            let _ = SetWindowPos(
                self.status,
                None,
                self.s(20),
                self.s(326),
                self.s(text_width),
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        place(self.transcribe, 218, 326, 110, 32);
        place(self.notes, 334, 326, 80, 32);
        place(self.folder, 420, 326, 40, 32);
        place(self.refresh, 320, 326, 140, 32);
        for list in [self.microphones, self.outputs, self.quality] {
            unsafe {
                SendMessageW(
                    list,
                    CB_SETITEMHEIGHT,
                    WPARAM(usize::MAX),
                    LPARAM(self.s(22) as isize),
                );
                SendMessageW(
                    list,
                    CB_SETITEMHEIGHT,
                    WPARAM(0),
                    LPARAM(self.s(24) as isize),
                );
            }
            dropdown_width(list, self.fonts.body, self.s(440), self.s(32));
        }
        let mic_bottom = window_rect_in_parent(root, self.microphones).bottom;
        let out_bottom = window_rect_in_parent(root, self.outputs).bottom;
        unsafe {
            let _ = SetWindowPos(
                self.microphone_level,
                None,
                self.s(364),
                mic_bottom,
                self.s(96),
                self.s(20),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = SetWindowPos(
                self.output_level,
                None,
                self.s(364),
                out_bottom,
                self.s(96),
                self.s(20),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.meter_top = [mic_bottom + self.s(2), out_bottom + self.s(2)];
        for list in [self.microphones, self.outputs, self.quality] {
            paint_combo_chrome(list);
        }
        self.apply_frame(
            root,
            RECT {
                right: self.s(CLIENT_WIDTH),
                bottom: self.s(342) + height,
                ..Default::default()
            },
        );
    }

    fn apply_frame(&self, root: HWND, mut frame: RECT) {
        unsafe {
            let style = WINDOW_STYLE(GetWindowLongW(root, GWL_STYLE) as u32);
            let ex_style = WINDOW_EX_STYLE(GetWindowLongW(root, GWL_EXSTYLE) as u32);
            let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, self.dpi);
            let mut existing = RECT::default();
            let _ = GetWindowRect(root, &mut existing);
            let width = frame.right - frame.left;
            let height = frame.bottom - frame.top;
            if existing.right - existing.left != width || existing.bottom - existing.top != height {
                let _ = SetWindowPos(
                    root,
                    None,
                    0,
                    0,
                    width,
                    height,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn status_height(&self, root: HWND, width: i32) -> i32 {
        let text = self.painted.status.as_ref().map_or("", |s| s.text.as_str());
        if text.is_empty() {
            return 0;
        }
        let mut text: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            right: width,
            ..Default::default()
        };
        unsafe {
            let dc = GetDC(root);
            let old = SelectObject(dc, HGDIOBJ(self.fonts.body.0));
            DrawTextW(
                dc,
                &mut text,
                &mut rect,
                DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
            );
            SelectObject(dc, old);
            ReleaseDC(root, dc);
        }
        rect.bottom
    }

    pub(crate) fn set_list_dropped(&mut self, dropped: bool) {
        self.list_dropped = dropped;
    }
    pub(crate) fn list_dropped(&self) -> bool {
        self.list_dropped
    }
    #[cfg(test)]
    fn microphone_combo(&self) -> HWND {
        self.microphones
    }

    pub(crate) fn show(&mut self, root: HWND, view: &View) {
        let pending = view.phase == Phase::AwaitingSave;
        let saving = view.phase == Phase::Saving;
        let live = matches!(view.phase, Phase::Recording | Phase::Paused);
        let phase_changed = self.painted.phase != Some(view.phase);
        if phase_changed && !matches!(view.phase, Phase::Idle | Phase::Failed) {
            for list in [self.microphones, self.outputs, self.quality] {
                unsafe {
                    SendMessageW(list, CB_SHOWDROPDOWN, WPARAM(0), LPARAM(0));
                }
            }
            self.list_dropped = false;
        }
        enable(self.microphones, view.microphone.enabled);
        enable(self.outputs, view.output.enabled);
        enable(self.quality, view.quality.enabled);
        if self.list_dropped && view.phase == Phase::Idle {
            return;
        }
        if !self.list_dropped {
            if let Some(lists) = &view.endpoints {
                refill(self.microphones, &lists.microphones);
                refill(self.outputs, &lists.outputs);
                for list in [self.microphones, self.outputs] {
                    dropdown_width(list, self.fonts.body, self.s(440), self.s(32));
                }
            }
            choose(self.microphones, view.microphone);
            choose(self.outputs, view.output);
            choose(self.quality, view.quality);
        }
        let previous_focus = unsafe { GetFocus() };
        if phase_changed {
            self.painted.phase = Some(view.phase);
            visible(self.quality, !saving);
            visible(self.quality_label, !saving);
            visible(self.microphones, true);
            visible(self.microphone_level, true);
            visible(self.outputs, true);
            visible(self.output_level, true);
            visible(self.status, true);
            visible(self.progress, saving);
            visible(self.progress_label, saving);
            set_text(
                self.pause,
                if view.phase == Phase::Paused {
                    "&Resume"
                } else {
                    "&Pause"
                },
            );
        }
        if phase_changed {
            self.layout(root);
        }
        enable(
            self.toggle,
            matches!(view.phase, Phase::Idle | Phase::Failed) && view.transport.toggle_enabled,
        );
        enable(self.stop, live && view.transport.toggle_enabled);
        enable(self.pause, live);
        enable(self.save, view.transport.save_enabled);
        enable(self.discard, view.transport.discard_enabled);
        if phase_changed
            && self.command_buttons().contains(&previous_focus)
            && unsafe { !IsWindowEnabled(previous_focus).as_bool() }
        {
            let next = if live {
                self.stop
            } else if pending {
                self.save
            } else if saving {
                self.settings
            } else {
                self.toggle
            };
            unsafe {
                let _ = SetFocus(if IsWindowEnabled(next).as_bool() {
                    next
                } else {
                    self.settings
                });
            }
        }
        if self.painted.elapsed != view.elapsed {
            self.painted.elapsed.clone_from(&view.elapsed);
            set_text(self.elapsed, &view.elapsed);
        }
        let heading = match view.phase {
            Phase::Idle if view.status.tone == Tone::Failure => "Needs attention",
            Phase::Idle if view.microphone.selected.is_none() || view.output.selected.is_none() => {
                "Check devices"
            }
            Phase::Idle if view.saved_path.is_some() => "Recording saved",
            Phase::Idle => "Ready to record",
            Phase::Recording if view.status.tone == Tone::Warning => "Recording · check audio",
            Phase::Recording => "Recording",
            Phase::Paused => "Recording paused",
            Phase::AwaitingSave => "Recording not saved",
            Phase::Saving => "Saving MP3",
            Phase::Failed => "Needs attention",
        };
        if self.painted.heading != heading {
            self.painted.heading = heading.into();
            set_text(self.heading, heading);
        }
        let saved_file = view.phase == Phase::Idle && view.saved_path.is_some();
        let missing = view.phase == Phase::Idle
            && (view.microphone.selected.is_none() || view.output.selected.is_none());
        let mut status = view.status.clone();
        status.text = match (view.phase, status.text.as_str()) {
            (Phase::Recording, "Recording.") => "Recording microphone and system audio.".into(),
            (Phase::Paused, "Recording paused.") => {
                "Recording paused. Stop still ends the take.".into()
            }
            (Phase::Saving, "Saving MP3…") => {
                "You can keep working while the MP3 is saved.".into()
            }
            _ => status.text,
        };
        if self.painted.status.as_ref() != Some(&status)
            || saved_file != self.painted.saved_file
            || missing != self.painted.missing_devices
            || view.job_busy != self.painted.job_busy
        {
            self.painted.saved_file = saved_file;
            self.painted.missing_devices = missing;
            self.painted.job_busy = view.job_busy;
            visible(self.folder, saved_file);
            visible(self.transcribe, saved_file);
            visible(self.notes, saved_file);
            visible(self.refresh, missing && !saved_file);
            enable(self.transcribe, saved_file && !view.job_busy);
            enable(self.notes, saved_file && !view.job_busy);
            set_text(self.status, &status.text);
            self.painted.status = Some(status);
            self.layout(root);
        }
        let percent = view.progress.map(|p| {
            if p.total == 0 {
                0
            } else {
                ((p.done.min(p.total) as f64 / p.total as f64) * 100.0).floor() as u32
            }
        });
        if percent != self.painted.percent {
            self.painted.percent = percent;
            if let Some(percent) = percent {
                unsafe {
                    SendMessageW(
                        self.progress,
                        PBM_SETPOS,
                        WPARAM(percent as usize),
                        LPARAM(0),
                    );
                }
                set_text(self.progress_label, &format!("Exporting MP3 · {percent}%"));
            }
        }
        for (index, level) in [view.levels.microphone, view.levels.system]
            .into_iter()
            .enumerate()
        {
            let next = Meter {
                width: (bar_fraction(level.peak) * self.s(332) as f32).round() as i32,
                clipping: level.clipping,
                db: view.phase.meters_live().then(|| {
                    if level.peak > 0.001 {
                        (20.0 * level.peak.log10()).round().min(0.0) as i32
                    } else {
                        -60
                    }
                }),
            };
            if next != self.painted.meters[index] || phase_changed {
                let old = self.painted.meters[index];
                self.painted.meters[index] = next;
                if old.width != next.width || old.clipping != next.clipping || phase_changed {
                    unsafe {
                        let _ = InvalidateRect(root, Some(&self.meter_rect(index)), false);
                    }
                    paint_combo_chrome(if index == 0 {
                        self.microphones
                    } else {
                        self.outputs
                    });
                }
                if old.clipping != next.clipping || old.db != next.db || phase_changed {
                    let label = if next.clipping {
                        "Clipping".into()
                    } else {
                        match next.db {
                            Some(db) if db > -60 => format!("{db} dBFS"),
                            Some(_) => "Silence".into(),
                            None => "Inactive".into(),
                        }
                    };
                    set_text(
                        if index == 0 {
                            self.microphone_level
                        } else {
                            self.output_level
                        },
                        &label,
                    );
                }
            }
        }
    }

    fn meter_rect(&self, index: usize) -> RECT {
        let y = self.meter_top[index];
        RECT {
            left: self.s(20),
            top: y,
            right: self.s(352),
            bottom: y + self.s(8),
        }
    }
    pub(crate) fn background(&self) -> HBRUSH {
        self.theme.background
    }
    pub(crate) fn draw_meters(&self, hdc: HDC) {
        unsafe {
            FillRect(
                hdc,
                &RECT {
                    left: self.s(20),
                    top: self.s(TRANSPORT_DIVIDER_Y),
                    right: self.s(460),
                    bottom: self.s(TRANSPORT_DIVIDER_Y) + 1,
                },
                self.theme.divider,
            );
        }
        for index in 0..2 {
            let rect = self.meter_rect(index);
            let meter = self.painted.meters[index];
            unsafe {
                FillRect(hdc, &rect, self.theme.track);
                if meter.width > 0 {
                    FillRect(
                        hdc,
                        &RECT {
                            right: rect.left + meter.width,
                            ..rect
                        },
                        if meter.clipping {
                            self.theme.clipping
                        } else {
                            self.theme.signal
                        },
                    );
                }
            }
        }
    }

    pub(crate) fn color_static(&self, control: HWND, hdc: HDC) -> HBRUSH {
        let tone = self.painted.status.as_ref().map(|s| s.tone);
        let color = if control == self.heading && self.painted.phase == Some(Phase::Recording) {
            if tone == Some(Tone::Warning) {
                self.theme.warning
            } else {
                self.theme.red
            }
        } else if control == self.heading && self.painted.phase == Some(Phase::Paused) {
            self.theme.warning
        } else if control == self.status {
            match tone {
                Some(Tone::Failure) => self.theme.red,
                Some(Tone::Warning) => self.theme.warning,
                _ => self.theme.muted,
            }
        } else if control == self.microphone_level || control == self.output_level {
            let index = usize::from(control == self.output_level);
            if self.painted.meters[index].clipping {
                self.theme.red
            } else {
                self.theme.muted
            }
        } else {
            self.theme.ink
        };
        unsafe {
            SetTextColor(hdc, color);
            SetBkMode(hdc, TRANSPARENT);
        }
        self.theme.background
    }

    pub(crate) fn command_buttons(&self) -> [HWND; 10] {
        [
            self.toggle,
            self.stop,
            self.save,
            self.pause,
            self.discard,
            self.settings,
            self.notes,
            self.folder,
            self.refresh,
            self.transcribe,
        ]
    }

    pub(crate) fn paints_command_button(&self, hwnd: HWND) -> bool {
        self.command_buttons().contains(&hwnd)
    }

    pub(crate) fn set_button_hover(&self, hwnd: HWND, hovered: bool) -> bool {
        let previous = self.hovered_button.get();
        let next = if hovered {
            Some(hwnd)
        } else if previous == Some(hwnd) {
            None
        } else {
            previous
        };
        if previous == next {
            return false;
        }
        self.hovered_button.set(next);
        true
    }

    pub(crate) fn paint_command_button(&self, hwnd: HWND, target: HDC) -> bool {
        if !self.paints_command_button(hwnd) {
            return false;
        }
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
        }
        if let Some(buffer) = PaintBuffer::new(target, rect.right, rect.bottom) {
            self.paint_button_contents(hwnd, buffer.dc());
            if buffer.present(target) {
                return true;
            }
        }
        self.paint_button_contents(hwnd, target)
    }

    fn paint_button_contents(&self, hwnd: HWND, hdc: HDC) -> bool {
        let state = unsafe { SendMessageW(hwnd, BM_GETSTATE, WPARAM(0), LPARAM(0)) }.0 as u32;
        let disabled = unsafe { !IsWindowEnabled(hwnd).as_bool() };
        let pressed = state & BST_PUSHED != 0;
        let primary = hwnd == self.toggle || hwnd == self.save || hwnd == self.stop;
        let destructive = hwnd == self.discard;
        let settings = hwnd == self.settings;
        let folder = hwnd == self.folder;
        let icon_only = settings || destructive || folder;
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
        }
        let hovered = self.hovered_button.get() == Some(hwnd);
        let brush = if disabled {
            self.theme.disabled
        } else if hwnd == self.save {
            if pressed {
                self.theme.save_pressed
            } else if hovered {
                self.theme.save_hover
            } else {
                self.theme.save
            }
        } else if primary {
            if pressed {
                self.theme.pressed
            } else if hovered {
                self.theme.hover
            } else {
                self.theme.primary
            }
        } else if pressed {
            self.theme.secondary_pressed
        } else if hovered && destructive {
            self.theme.danger_hover
        } else if hovered {
            self.theme.secondary_hover
        } else {
            self.theme.secondary
        };
        let ink = if disabled {
            self.theme.disabled_ink
        } else if primary {
            self.theme.primary_ink
        } else if destructive && (hovered || pressed) {
            self.theme.red
        } else {
            self.theme.ink
        };
        let mut caption = [0u16; 128];
        let count = unsafe { GetWindowTextW(hwnd, &mut caption) } as usize;
        let mut text: Vec<u16> = String::from_utf16_lossy(&caption[..count])
            .replace('&', "")
            .encode_utf16()
            .collect();
        let icon = if settings {
            Some(ButtonIcon::Settings)
        } else if hwnd == self.toggle {
            Some(ButtonIcon::Record)
        } else if hwnd == self.stop {
            Some(ButtonIcon::Stop)
        } else if hwnd == self.pause {
            Some(if self.painted.phase == Some(Phase::Paused) {
                ButtonIcon::Resume
            } else {
                ButtonIcon::Pause
            })
        } else if destructive {
            Some(ButtonIcon::Discard)
        } else if hwnd == self.save {
            Some(ButtonIcon::Save)
        } else {
            None
        };
        unsafe {
            let saved_dc = SaveDC(hdc);
            let _ = SetROP2(hdc, R2_COPYPEN);
            FillRect(hdc, &rect, self.theme.background);
            if !self.rounded.fill(hdc, rect, self.s(12), brush) {
                SelectObject(hdc, GetStockObject(NULL_PEN));
                SelectObject(hdc, HGDIOBJ(brush.0));
                let _ = RoundRect(hdc, 0, 0, rect.right, rect.bottom, self.s(12), self.s(12));
            }
            let font = if primary {
                self.fonts.strong
            } else {
                self.fonts.body
            };
            SelectObject(hdc, HGDIOBJ(font.0));
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, ink);
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, &text, &mut size);
            let icon_size = self.s(20);
            let gap = if icon.is_some() && !icon_only {
                self.s(8)
            } else {
                0
            };
            let width = if icon_only {
                icon_size
            } else {
                size.cx + gap + if icon.is_some() { icon_size } else { 0 }
            };
            let offset = if pressed && !disabled { self.s(1) } else { 0 };
            let left = (rect.right - width) / 2 + offset;
            if folder {
                draw_folder_glyph(
                    hdc,
                    left,
                    (rect.bottom - icon_size) / 2 + offset,
                    icon_size,
                    ink,
                );
            } else if let Some(icon) = icon {
                SelectObject(hdc, HGDIOBJ(self.fonts.icons.0));
                let transform = MAT2 {
                    eM11: FIXED { value: 1, fract: 0 },
                    eM22: FIXED { value: 1, fract: 0 },
                    ..Default::default()
                };
                let mut bounds = GLYPHMETRICS::default();
                GetGlyphOutlineW(
                    hdc,
                    u32::from(icon.glyph()),
                    GGO_METRICS,
                    &mut bounds,
                    0,
                    None,
                    &transform,
                );
                // Center the actual outline, not the font's asymmetric padding.
                let alignment = SetTextAlign(hdc, TA_LEFT | TA_BASELINE);
                let _ = TextOutW(
                    hdc,
                    left + (icon_size - bounds.gmBlackBoxX as i32) / 2 - bounds.gmptGlyphOrigin.x,
                    (rect.bottom - bounds.gmBlackBoxY as i32) / 2
                        + bounds.gmptGlyphOrigin.y
                        + offset,
                    &[icon.glyph()],
                );
                SetTextAlign(hdc, TEXT_ALIGN_OPTIONS(alignment));
                SelectObject(hdc, HGDIOBJ(font.0));
            }
            if !icon_only {
                let mut text_rect = RECT {
                    left: left + if icon.is_some() { icon_size + gap } else { 0 },
                    top: offset,
                    right: rect.right - self.s(8) + offset,
                    bottom: rect.bottom + offset,
                };
                DrawTextW(
                    hdc,
                    &mut text,
                    &mut text_rect,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
            let show_focus = SendMessageW(hwnd, WM_QUERYUISTATE, WPARAM(0), LPARAM(0)).0
                & UISF_HIDEFOCUS as isize
                == 0;
            if GetFocus() == hwnd && !disabled && show_focus {
                let inset = self.s(4);
                let focus = RECT {
                    left: inset,
                    top: inset,
                    right: rect.right - inset,
                    bottom: rect.bottom - inset,
                };
                let _ = DrawFocusRect(hdc, &focus);
            }
            let _ = RestoreDC(hdc, saved_dc);
        }
        true
    }
}

fn draw_folder_glyph(hdc: HDC, left: i32, top: i32, size: i32, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let pen = CreatePen(PS_SOLID, 1.max(size / 16), color);
        let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
        let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        let tab_h = (size / 5).max(2);
        let tab_w = size / 2;
        let body_top = top + tab_h;
        let _ = Rectangle(hdc, left, top + size / 10, left + tab_w, body_top + 1);
        let _ = Rectangle(hdc, left, body_top, left + size, top + size);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

#[derive(Clone, Copy)]
enum ButtonIcon {
    Record,
    Stop,
    Pause,
    Resume,
    Discard,
    Settings,
    Save,
}

impl ButtonIcon {
    // Codepoints from lucide-static 0.468.0; see assets/lucide-SOURCE.txt.
    fn glyph(self) -> u16 {
        match self {
            Self::Record => 0xe07a,
            Self::Stop => 0xe16a,
            Self::Pause => 0xe131,
            Self::Resume => 0xe13f,
            Self::Discard => 0xe18d,
            Self::Settings => 0xe157,
            Self::Save => 0xe150,
        }
    }
}

fn name_device_controls(microphones: HWND, outputs: HWND) {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Accessibility::{CAccPropServices, IAccPropServices, PROPID_ACC_NAME};
    unsafe {
        if let Ok(names) =
            CoCreateInstance::<_, IAccPropServices>(&CAccPropServices, None, CLSCTX_INPROC_SERVER)
        {
            for (control, name) in [
                (microphones, w!("Microphone")),
                (outputs, w!("System audio")),
            ] {
                let _ =
                    names.SetHwndPropStr(control, OBJID_CLIENT.0 as u32, 0, PROPID_ACC_NAME, name);
            }
        }
    }
}

fn control_tooltip(
    root: HWND,
    instance: HINSTANCE,
    control: HWND,
    text: PCWSTR,
) -> Result<(), RunError> {
    unsafe {
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            w!(""),
            WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            root,
            None,
            instance,
            None,
        )
        .map_err(|e| RunError::new(format!("creating control tooltip failed: {e}")))?;
        let info = TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd: root,
            uId: control.0 as usize,
            lpszText: windows::core::PWSTR(text.as_ptr() as *mut u16),
            ..Default::default()
        };
        SendMessageW(
            tooltip,
            TTM_ADDTOOLW,
            WPARAM(0),
            LPARAM(&info as *const _ as isize),
        );
    }
    Ok(())
}

impl Fonts {
    fn new(dpi: u32) -> Self {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some(&mut metrics as *mut _ as *mut c_void),
                0,
                dpi,
            );
        }
        let mut base = metrics.lfMessageFont;
        if base.lfFaceName[0] == 0 {
            for (to, from) in base.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) {
                *to = from;
            }
        }
        base.lfHeight = -base.lfHeight.abs().max(scale(14, dpi));
        let units = dpi.max((base.lfHeight.unsigned_abs() * 96).div_ceil(14));
        let body = unsafe { CreateFontIndirectW(&base) };
        let body_face = base.lfFaceName;
        base.lfWeight = 600;
        if String::from_utf16_lossy(&base.lfFaceName).trim_end_matches('\0') == "Segoe UI" {
            base.lfFaceName.fill(0);
            for (to, from) in base
                .lfFaceName
                .iter_mut()
                .zip("Segoe UI Semibold".encode_utf16())
            {
                *to = from;
            }
        }
        let strong = unsafe { CreateFontIndirectW(&base) };
        base.lfFaceName = body_face;
        base.lfWeight = 400;
        base.lfHeight = -scale(34, units);
        let timer = unsafe { CreateFontIndirectW(&base) };
        // Private to this process: no installed font or runtime download needed.
        let icon_bytes = include_bytes!("../../assets/onerec-lucide.ttf");
        let mut count = 0u32;
        let icon_resource = unsafe {
            AddFontMemResourceEx(
                icon_bytes.as_ptr().cast(),
                icon_bytes.len() as u32,
                None,
                &mut count,
            )
        };
        let mut icon_face = LOGFONTW {
            lfHeight: -scale(20, units),
            lfWeight: 400,
            lfCharSet: DEFAULT_CHARSET,
            lfQuality: ANTIALIASED_QUALITY,
            ..Default::default()
        };
        for (to, from) in icon_face.lfFaceName.iter_mut().zip("lucide".encode_utf16()) {
            *to = from;
        }
        let icons = unsafe { CreateFontIndirectW(&icon_face) };
        Self {
            body,
            strong,
            timer,
            icons,
            icon_resource,
            units,
        }
    }
}
impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [self.body, self.strong, self.timer, self.icons] {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }
        if !self.icon_resource.is_invalid() {
            unsafe {
                let _ = RemoveFontMemResourceEx(self.icon_resource);
            }
        }
    }
}
pub(super) fn scale(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}
fn bar_fraction(peak: f32) -> f32 {
    if peak <= 0.0 {
        0.0
    } else {
        ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
    }
}
fn window_rect_in_parent(parent: HWND, window: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetWindowRect(window, &mut rect);
    }
    let mut points = [
        POINT {
            x: rect.left,
            y: rect.top,
        },
        POINT {
            x: rect.right,
            y: rect.bottom,
        },
    ];
    unsafe {
        MapWindowPoints(HWND::default(), parent, &mut points);
    }
    RECT {
        left: points[0].x,
        top: points[0].y,
        right: points[1].x,
        bottom: points[1].y,
    }
}

fn paint_combo_chrome(list: HWND) {
    unsafe {
        let _ = SetWindowPos(
            list,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = RedrawWindow(
            list,
            None,
            None,
            RDW_INVALIDATE | RDW_FRAME | RDW_NOCHILDREN,
        );
    }
}

fn refill(list: HWND, names: &[String]) {
    unsafe {
        SendMessageW(list, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for name in names {
            let wide = HSTRING::from(name);
            SendMessageW(
                list,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide.as_ptr() as isize),
            );
        }
    }
}
fn dropdown_width(list: HWND, font: HFONT, minimum: i32, padding: i32) {
    unsafe {
        let dc = GetDC(list);
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let count = SendMessageW(list, CB_GETCOUNT, WPARAM(0), LPARAM(0))
            .0
            .max(0) as usize;
        let mut width = minimum;
        for index in 0..count {
            let length = SendMessageW(list, CB_GETLBTEXTLEN, WPARAM(index), LPARAM(0)).0;
            if !(0..=32768).contains(&length) {
                continue;
            }
            let mut text = vec![0u16; length as usize + 1];
            SendMessageW(
                list,
                CB_GETLBTEXT,
                WPARAM(index),
                LPARAM(text.as_mut_ptr() as isize),
            );
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &text[..length as usize], &mut size);
            width = width.max(size.cx + padding);
        }
        SelectObject(dc, old);
        ReleaseDC(list, dc);
        let monitor = MonitorFromWindow(list, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            width = width.min(info.rcWork.right - info.rcWork.left - padding);
        }
        SendMessageW(
            list,
            CB_SETDROPPEDWIDTH,
            WPARAM(width.max(1) as usize),
            LPARAM(0),
        );
    }
}
fn choose(list: HWND, selector: Selector) {
    let wanted = selector.selected.map_or(-1, |index| index as isize);
    unsafe {
        if SendMessageW(list, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 != wanted {
            SendMessageW(list, CB_SETCURSEL, WPARAM(wanted as usize), LPARAM(0));
        }
    }
    enable(list, selector.enabled);
}
fn enable(control: HWND, enabled: bool) {
    unsafe {
        if IsWindowEnabled(control).as_bool() != enabled {
            let _ = EnableWindow(control, enabled);
        }
    }
}
fn visible(control: HWND, show: bool) {
    unsafe {
        if (GetWindowLongW(control, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0) != show {
            let _ = ShowWindow(control, if show { SW_SHOWNA } else { SW_HIDE });
        }
    }
}
fn set_text(control: HWND, text: &str) {
    unsafe {
        let _ = SetWindowTextW(control, &HSTRING::from(text));
    }
}
fn combo(root: HWND, instance: HINSTANCE, id: u16, _top: i32) -> Result<HWND, RunError> {
    child(
        root,
        instance,
        w!("COMBOBOX"),
        w!(""),
        WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        id,
    )
}
fn button(root: HWND, instance: HINSTANCE, id: u16, caption: PCWSTR) -> Result<HWND, RunError> {
    child(
        root,
        instance,
        w!("BUTTON"),
        caption,
        WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        id,
    )
}
fn static_text(
    root: HWND,
    instance: HINSTANCE,
    caption: PCWSTR,
    style: u32,
) -> Result<HWND, RunError> {
    child(
        root,
        instance,
        w!("STATIC"),
        caption,
        WINDOW_STYLE(style),
        0,
    )
}
fn child(
    root: HWND,
    instance: HINSTANCE,
    class: PCWSTR,
    caption: PCWSTR,
    style: WINDOW_STYLE,
    id: u16,
) -> Result<HWND, RunError> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            caption,
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            100,
            24,
            root,
            HMENU(id as usize as *mut c_void),
            instance,
            None,
        )
    }
    .map_err(|e| RunError::new(format!("creating a window control failed: {e}")))
}
#[cfg(test)]
include!("paint_tests.rs");
