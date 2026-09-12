use super::theme::{rgb, Theme};
use crate::mp3::ExportQuality;
use crate::recorder::{Phase, Selector, Status, Tone, View};
use crate::RunError;
use std::ffi::c_void;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, RECT, SIZE, WPARAM};
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
pub(crate) const CLIENT_HEIGHT: i32 = 348;
pub(crate) const CLIENT_HUD_HEIGHT: i32 = 112;
pub(crate) const ID_MICROPHONE: u16 = 101;
pub(crate) const ID_OUTPUT: u16 = 102;
pub(crate) const ID_TOGGLE: u16 = 103;
pub(crate) const ID_SAVE: u16 = 104;
pub(crate) const ID_DISCARD: u16 = 105;
pub(crate) const ID_QUALITY: u16 = 106;
pub(crate) const ID_FOLDER: u16 = 107;
pub(crate) const ID_REFRESH: u16 = 108;
pub(crate) const ID_PAUSE: u16 = 110;
const MICROPHONE_Y: i32 = 130;

pub(crate) struct Controls {
    microphones: HWND,
    outputs: HWND,
    quality: HWND,
    microphone_label: HWND,
    output_label: HWND,
    quality_label: HWND,
    microphone_level: HWND,
    output_level: HWND,
    toggle: HWND,
    save: HWND,
    discard: HWND,
    pause: HWND,
    folder: HWND,
    refresh: HWND,
    elapsed: HWND,
    heading: HWND,
    shortcut: HWND,
    status: HWND,
    progress: HWND,
    progress_label: HWND,
    fonts: Fonts,
    theme: Theme,
    dpi: u32,
    units: u32,
    painted: Painted,
    list_dropped: bool,
}

#[derive(Default)]
struct Painted {
    phase: Option<Phase>,
    toggle_label: &'static str,
    elapsed: String,
    heading: String,
    status: Option<Status>,
    meters: [Meter; 2],
    percent: Option<u32>,
    saved_file: bool,
    missing_devices: bool,
    hud: bool,
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
        // Keep each native label immediately before its associated field.
        let microphone_label = static_text(root, instance, w!("&Microphone"), 0)?;
        let microphones = combo(root, instance, ID_MICROPHONE, MICROPHONE_Y)?;
        let output_label = static_text(root, instance, w!("System &audio"), 0)?;
        let outputs = combo(root, instance, ID_OUTPUT, 202)?;
        let quality_label = static_text(root, instance, w!("MP3 &quality"), 0)?;
        let quality = combo(root, instance, ID_QUALITY, 254)?;
        let toggle = button(root, instance, ID_TOGGLE, w!("Start &recording"))?;
        let save = button(root, instance, ID_SAVE, w!("&Save recording…"))?;
        let discard = button(root, instance, ID_DISCARD, w!("&Discard…"))?;
        let pause = button(root, instance, ID_PAUSE, w!("&Pause"))?;
        let folder = button(root, instance, ID_FOLDER, w!("Open &folder"))?;
        let refresh = button(root, instance, ID_REFRESH, w!("Re&fresh devices"))?;
        let controls = Self {
            microphones,
            outputs,
            quality,
            microphone_label,
            output_label,
            quality_label,
            microphone_level: static_text(root, instance, w!(""), 2)?,
            output_level: static_text(root, instance, w!(""), 2)?,
            toggle,
            save,
            discard,
            pause,
            folder,
            refresh,
            elapsed: static_text(root, instance, w!("00:00"), 0)?,
            heading: static_text(root, instance, w!("Ready"), 0)?,
            shortcut: static_text(root, instance, w!("Ctrl+Shift+R"), 2)?,
            status: static_text(
                root,
                instance,
                w!("Levels appear when recording."),
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
            dpi,
            painted: Painted::default(),
            list_dropped: false,
        };
        for control in [
            save,
            discard,
            pause,
            folder,
            refresh,
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
        // Visual styles draw a focus halo after NM_CUSTOMDRAW. Strip them so
        // the command buttons only show the rounded fill we paint.
        for hwnd in [controls.toggle, controls.save] {
            unsafe {
                let _ = SetWindowTheme(hwnd, w!(""), w!(""));
            }
        }
        Ok(controls)
    }

    fn all(&self) -> [HWND; 20] {
        [
            self.microphone_label,
            self.microphones,
            self.output_label,
            self.outputs,
            self.quality_label,
            self.quality,
            self.microphone_level,
            self.output_level,
            self.toggle,
            self.save,
            self.discard,
            self.pause,
            self.folder,
            self.refresh,
            self.elapsed,
            self.heading,
            self.shortcut,
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

    fn layout(&self, root: HWND) {
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
        if self.painted.hud {
            place(self.heading, 20, 12, 220, 20);
            place(self.elapsed, 20, 36, 220, 44);
            place(self.toggle, 256, 36, 204, 40);
            place(self.save, 256, 36, 204, 40);
            place(self.shortcut, 20, 84, 200, 18);
            place(self.pause, 356, 80, 104, 24);
            self.fit_client(root, CLIENT_WIDTH, CLIENT_HUD_HEIGHT);
            return;
        }
        place(self.heading, 20, 16, 440, 20);
        place(self.elapsed, 20, 36, 220, 44);
        place(self.toggle, 256, 36, 204, 40);
        place(self.save, 256, 36, 204, 40);
        place(self.shortcut, 256, 80, 204, 18);
        place(self.discard, 356, 80, 104, 24);
        place(self.pause, 356, 80, 104, 24);
        place(self.microphone_label, 20, 110, 440, 18);
        place(self.microphones, 20, 130, 440, 220);
        place(self.microphone_level, 364, 155, 96, 20);
        place(self.output_label, 20, 182, 440, 18);
        place(self.outputs, 20, 202, 440, 220);
        place(self.output_level, 364, 227, 96, 20);
        place(self.quality_label, 20, 259, 136, 20);
        place(self.quality, 178, 254, 282, 220);
        place(self.progress_label, 20, 254, 440, 20);
        place(self.progress, 20, 278, 440, 10);
        let text_width = if self.painted.saved_file || self.painted.missing_devices {
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
                self.s(298),
                self.s(text_width),
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        place(self.folder, 320, 298, 140, 30);
        place(self.refresh, 320, 298, 140, 30);
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
        self.apply_frame(
            root,
            RECT {
                right: self.s(CLIENT_WIDTH),
                bottom: self.s(314) + height,
                ..Default::default()
            },
        );
    }

    fn fit_client(&self, root: HWND, client_w: i32, client_h: i32) {
        let frame = RECT {
            right: self.s(client_w),
            bottom: self.s(client_h),
            ..Default::default()
        };
        self.apply_frame(root, frame);
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
        let hud = live;
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
        // Preserve an open picker's hovered row without freezing status updates.
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
            self.painted.hud = hud;
            visible(self.toggle, !pending && !saving);
            visible(self.save, pending || saving);
            visible(self.discard, pending);
            visible(self.pause, live);
            visible(self.shortcut, hud || (!pending && !saving && !live));
            visible(self.quality, !hud && !saving);
            visible(self.quality_label, !hud && !saving);
            visible(self.microphone_label, !hud);
            visible(self.microphones, !hud);
            visible(self.microphone_level, !hud);
            visible(self.output_label, !hud);
            visible(self.outputs, !hud);
            visible(self.output_level, !hud);
            visible(self.status, !hud);
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
            set_text(
                self.save,
                if saving {
                    "Saving MP3…"
                } else {
                    "&Save recording…"
                },
            );
            self.layout(root);
        }
        if self.painted.toggle_label != view.transport.toggle_label {
            self.painted.toggle_label = view.transport.toggle_label;
            set_text(
                self.toggle,
                if matches!(view.phase, Phase::Recording | Phase::Paused) {
                    "Stop &recording"
                } else {
                    "Start &recording"
                },
            );
        }
        enable(self.toggle, view.transport.toggle_enabled);
        enable(self.save, view.transport.save_enabled);
        enable(self.discard, view.transport.discard_enabled);
        if phase_changed
            && !saving
            && [self.toggle, self.save, self.discard].contains(&previous_focus)
        {
            unsafe {
                let _ = SetFocus(if pending { self.save } else { self.toggle });
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
            Phase::Recording if hud => "REC",
            Phase::Recording if view.status.tone == Tone::Warning => "Recording · check audio",
            Phase::Recording => "Recording",
            Phase::Paused if hud => "PAUSED",
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
            (Phase::Idle, "Ready. Ctrl+Shift+R starts recording.") => {
                "Levels appear when recording.".into()
            }
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
        {
            self.painted.saved_file = saved_file;
            self.painted.missing_devices = missing;
            visible(self.folder, saved_file);
            visible(self.refresh, missing && !saved_file);
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
        if self.painted.hud {
            return;
        }
        for (index, level) in [view.levels.microphone, view.levels.system]
            .into_iter()
            .enumerate()
        {
            let next = Meter {
                width: (bar_fraction(level.peak) * self.s(332) as f32).round() as i32,
                clipping: level.clipping,
                db: (view.phase == Phase::Recording).then(|| {
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
        let y = if index == 0 { 160 } else { 232 };
        RECT {
            left: self.s(20),
            top: self.s(y),
            right: self.s(352),
            bottom: self.s(y + 8),
        }
    }
    pub(crate) fn background(&self) -> HBRUSH {
        self.theme.background
    }
    pub(crate) fn draw_meters(&self, hdc: HDC) {
        if self.painted.hud {
            return;
        }
        unsafe {
            FillRect(
                hdc,
                &RECT {
                    left: self.s(20),
                    top: self.s(104),
                    right: self.s(460),
                    bottom: self.s(104) + 1,
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
        } else if control == self.shortcut {
            self.theme.muted
        } else {
            self.theme.ink
        };
        unsafe {
            SetTextColor(hdc, color);
            SetBkMode(hdc, TRANSPARENT);
        }
        self.theme.background
    }

    pub(crate) fn command_buttons(&self) -> [HWND; 2] {
        [self.toggle, self.save]
    }

    pub(crate) fn paints_command_button(&self, hwnd: HWND) -> bool {
        [self.toggle, self.save].contains(&hwnd) && !self.theme.high_contrast
    }

    // Native BUTTON keeps keyboard, accessibility and hover/pressed tracking.
    pub(crate) fn draw_button(&self, draw: &NMCUSTOMDRAW) -> Option<u32> {
        if !self.paints_command_button(draw.hdr.hwndFrom) {
            return None;
        }
        match draw.dwDrawStage {
            CDDS_PREPAINT => {
                self.paint_command_button(draw.hdr.hwndFrom, draw.hdc);
                Some(CDRF_SKIPDEFAULT | CDRF_NOTIFYPOSTPAINT)
            }
            CDDS_POSTPAINT => {
                self.paint_command_button(draw.hdr.hwndFrom, draw.hdc);
                Some(CDRF_SKIPDEFAULT)
            }
            _ => Some(CDRF_DODEFAULT),
        }
    }

    pub(crate) fn paint_command_button(&self, hwnd: HWND, hdc: HDC) -> bool {
        if !self.paints_command_button(hwnd) {
            return false;
        }
        let state = unsafe { SendMessageW(hwnd, BM_GETSTATE, WPARAM(0), LPARAM(0)) }.0 as u32;
        let disabled = unsafe { !IsWindowEnabled(hwnd).as_bool() };
        let brush = if disabled {
            self.theme.disabled
        } else if state & BST_PUSHED != 0 {
            self.theme.pressed
        } else if state & BST_HOT != 0 {
            self.theme.hover
        } else {
            self.theme.primary
        };
        let caption = if hwnd == self.save {
            if self.painted.phase == Some(Phase::Saving) {
                "Saving MP3…"
            } else {
                "Save recording…"
            }
        } else {
            self.painted.toggle_label
        };
        let mut text: Vec<u16> = caption.encode_utf16().collect();
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
            let _ = SetROP2(hdc, R2_COPYPEN);
            FillRect(hdc, &rect, self.theme.background);
            let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
            let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
            let _ = RoundRect(
                hdc,
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                self.s(10),
                self.s(10),
            );
            let old_font = SelectObject(hdc, HGDIOBJ(self.fonts.strong.0));
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(
                hdc,
                if disabled {
                    self.theme.muted
                } else {
                    rgb(255, 255, 255)
                },
            );
            let mut text_rect = rect;
            DrawTextW(
                hdc,
                &mut text,
                &mut text_rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(hdc, old_font);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
        }
        true
    }
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
        Self {
            body,
            strong,
            timer,
            units,
        }
    }
}
impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [self.body, self.strong, self.timer] {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
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
    child(root, instance, w!("BUTTON"), caption, WS_TABSTOP, id)
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
