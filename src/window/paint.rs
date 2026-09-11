use std::ffi::c_void;

use ::windows::core::{w, HSTRING, PCWSTR};
use ::windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, RECT, WPARAM};
use ::windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, CreateSolidBrush, DeleteObject, FillRect, GetStockObject, GetSysColor,
    GetSysColorBrush, InvalidateRect, SetBkMode, SetTextColor, COLOR_BTNFACE, COLOR_WINDOWTEXT,
    DEFAULT_GUI_FONT, HBRUSH, HDC, HFONT, HGDIOBJ, TRANSPARENT,
};
use ::windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetWindowTheme, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SendMessageW, SetWindowTextW, SystemParametersInfoW, CBS_DROPDOWNLIST,
    CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL, HMENU, NONCLIENTMETRICSW,
    SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_SETFONT, WS_CHILD, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::mp3::{ExportQuality, SaveProgress};
use crate::recorder::{Level, Levels, Selector, Status, Tone, View};
use crate::RunError;

pub(crate) const CLIENT_WIDTH: i32 = 460;
pub(crate) const CLIENT_HEIGHT: i32 = 264;

pub(crate) const ID_MICROPHONE: u16 = 101;
pub(crate) const ID_OUTPUT: u16 = 102;
pub(crate) const ID_TOGGLE: u16 = 103;
pub(crate) const ID_SAVE: u16 = 104;
pub(crate) const ID_DISCARD: u16 = 105;
pub(crate) const ID_QUALITY: u16 = 106;

const MARGIN: i32 = 16;
const LABEL_WIDTH: i32 = 92;
const FIELD_X: i32 = MARGIN + LABEL_WIDTH;
const FIELD_WIDTH: i32 = CLIENT_WIDTH - FIELD_X - MARGIN;
const ROW_HEIGHT: i32 = 24;
const LABEL_HEIGHT: i32 = 18;
const DROPPED_HEIGHT: i32 = ROW_HEIGHT + 180;

const COMBO_METER_GAP: i32 = 4;
const GROUP_GAP: i32 = 12;
const METER_HEIGHT: i32 = 16;

const MICROPHONE_Y: i32 = MARGIN;
const MICROPHONE_METER_Y: i32 = MICROPHONE_Y + ROW_HEIGHT + COMBO_METER_GAP;
const OUTPUT_Y: i32 = MICROPHONE_METER_Y + METER_HEIGHT + GROUP_GAP;
const SYSTEM_METER_Y: i32 = OUTPUT_Y + ROW_HEIGHT + COMBO_METER_GAP;
const QUALITY_Y: i32 = SYSTEM_METER_Y + METER_HEIGHT + GROUP_GAP;
const SAVE_BAR_Y: i32 = QUALITY_Y + ROW_HEIGHT + COMBO_METER_GAP;
const BUTTON_Y: i32 = 176;
const BUTTON_HEIGHT: i32 = 30;
const TOGGLE_WIDTH: i32 = 140;
const SMALL_BUTTON_WIDTH: i32 = 90;
const SAVE_X: i32 = MARGIN + TOGGLE_WIDTH + 8;
const DISCARD_X: i32 = SAVE_X + SMALL_BUTTON_WIDTH + 8;
const ELAPSED_X: i32 = DISCARD_X + SMALL_BUTTON_WIDTH + 8;
const ELAPSED_WIDTH: i32 = CLIENT_WIDTH - MARGIN - ELAPSED_X;
const STATUS_Y: i32 = 216;
const STATUS_HEIGHT: i32 = 32;

// SS_RIGHT lives in Win32::System::SystemServices, a feature nothing else here needs.
const SS_RIGHT: u32 = 0x0002;

pub(crate) struct Controls {
    microphones: HWND,
    outputs: HWND,
    quality: HWND,
    toggle: HWND,
    save: HWND,
    discard: HWND,
    elapsed: HWND,
    status: HWND,
    font: HFONT,
    trough: HBRUSH,
    signal: HBRUSH,
    clipping: HBRUSH,
    painted: Painted,
    list_dropped: bool,
}

struct Painted {
    toggle_label: &'static str,
    elapsed: String,
    status: Option<Status>,
    levels: Option<Levels>,
    progress: Option<SaveProgress>,
    saving: bool,
}

impl Controls {
    pub(crate) fn create(root: HWND, instance: HINSTANCE) -> Result<Self, RunError> {
        let common = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        unsafe {
            let _ = InitCommonControlsEx(&common);
        }

        let font = message_font();
        label(root, instance, font, w!("Microphone"), MICROPHONE_Y + 3)?;
        label(root, instance, font, w!("System"), OUTPUT_Y + 3)?;
        label(root, instance, font, w!("MP3 quality"), QUALITY_Y + 3)?;

        let controls = Self {
            microphones: combo(root, instance, ID_MICROPHONE, MICROPHONE_Y)?,
            outputs: combo(root, instance, ID_OUTPUT, OUTPUT_Y)?,
            quality: combo(root, instance, ID_QUALITY, QUALITY_Y)?,
            toggle: button(root, instance, ID_TOGGLE, MARGIN, TOGGLE_WIDTH)?,
            save: button(root, instance, ID_SAVE, SAVE_X, SMALL_BUTTON_WIDTH)?,
            discard: button(root, instance, ID_DISCARD, DISCARD_X, SMALL_BUTTON_WIDTH)?,
            elapsed: text(
                root,
                instance,
                ELAPSED_X,
                BUTTON_Y + (BUTTON_HEIGHT - LABEL_HEIGHT) / 2,
                ELAPSED_WIDTH,
                LABEL_HEIGHT,
                WINDOW_STYLE(SS_RIGHT),
            )?,
            status: text(
                root,
                instance,
                MARGIN,
                STATUS_Y,
                CLIENT_WIDTH - 2 * MARGIN,
                STATUS_HEIGHT,
                WINDOW_STYLE(0),
            )?,
            font,
            trough: unsafe { CreateSolidBrush(rgb(0x21, 0x23, 0x26)) },
            signal: unsafe { CreateSolidBrush(rgb(0x2e, 0xa0, 0x43)) },
            clipping: unsafe { CreateSolidBrush(rgb(0xd1, 0x3b, 0x3b)) },
            painted: Painted {
                toggle_label: "",
                elapsed: String::new(),
                status: None,
                levels: None,
                progress: None,
                saving: false,
            },
            list_dropped: false,
        };
        for control in [
            controls.microphones,
            controls.outputs,
            controls.quality,
            controls.toggle,
            controls.save,
            controls.discard,
            controls.elapsed,
            controls.status,
        ] {
            unsafe { SendMessageW(control, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1)) };
        }
        unsafe {
            let _ = SetWindowTextW(controls.save, w!("Save…"));
            let _ = SetWindowTextW(controls.discard, w!("Discard"));
        }
        refill(
            controls.quality,
            &ExportQuality::ALL.map(|quality| quality.label().to_string()),
        );
        Ok(controls)
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
        if self.list_dropped {
            return;
        }
        if let Some(lists) = &view.endpoints {
            refill(self.microphones, &lists.microphones);
            refill(self.outputs, &lists.outputs);
        }
        choose(self.microphones, view.microphone);
        choose(self.outputs, view.output);
        choose(self.quality, view.quality);

        if self.painted.toggle_label != view.transport.toggle_label {
            self.painted.toggle_label = view.transport.toggle_label;
            let label = HSTRING::from(view.transport.toggle_label);
            unsafe {
                let _ = SetWindowTextW(self.toggle, &label);
            }
        }
        enable(self.toggle, view.transport.toggle_enabled);
        enable(self.save, view.transport.save_enabled);
        enable(self.discard, view.transport.discard_enabled);

        if self.painted.elapsed != view.elapsed {
            self.painted.elapsed.clone_from(&view.elapsed);
            let elapsed = HSTRING::from(&view.elapsed);
            unsafe {
                let _ = SetWindowTextW(self.elapsed, &elapsed);
            }
        }
        if self.painted.status.as_ref() != Some(&view.status) {
            self.painted.status = Some(view.status.clone());
            let status = HSTRING::from(&view.status.text);
            unsafe {
                let _ = SetWindowTextW(self.status, &status);
            }
        }
        let saving = view.progress.is_some();
        if saving != self.painted.saving {
            self.painted.saving = saving;
            unsafe {
                let _ = InvalidateRect(root, Some(&save_bar()), true);
            }
        }
        if self.painted.levels != Some(view.levels) {
            self.painted.levels = Some(view.levels);
            for rect in [meter(MICROPHONE_METER_Y), meter(SYSTEM_METER_Y)] {
                unsafe {
                    let _ = InvalidateRect(root, Some(&rect), false);
                }
            }
        }
        if self.painted.progress != view.progress {
            self.painted.progress = view.progress;
            if saving {
                unsafe {
                    let _ = InvalidateRect(root, Some(&save_bar()), false);
                }
            }
        }
    }

    pub(crate) fn draw_meters(&self, hdc: HDC) {
        if let Some(progress) = self.painted.progress {
            self.fill_linear(hdc, save_bar(), progress.fraction());
        }
        let Some(levels) = self.painted.levels else {
            return;
        };
        self.draw_meter(hdc, meter(MICROPHONE_METER_Y), levels.microphone);
        self.draw_meter(hdc, meter(SYSTEM_METER_Y), levels.system);
    }

    pub(crate) fn color_static(&self, control: HWND, hdc: HDC) -> HBRUSH {
        let tone = if control == self.status {
            self.painted.status.as_ref().map(|status| status.tone)
        } else {
            None
        };
        let color = match tone {
            Some(Tone::Recording) => rgb(0xc0, 0x28, 0x28),
            Some(Tone::Warning) => rgb(0x8a, 0x5a, 0x00),
            Some(Tone::Failure) => rgb(0xb0, 0x00, 0x00),
            Some(Tone::Neutral) | None => COLORREF(unsafe { GetSysColor(COLOR_WINDOWTEXT) }),
        };
        unsafe {
            SetTextColor(hdc, color);
            SetBkMode(hdc, TRANSPARENT);
            GetSysColorBrush(COLOR_BTNFACE)
        }
    }

    fn draw_meter(&self, hdc: HDC, rect: RECT, level: Level) {
        unsafe { FillRect(hdc, &rect, self.trough) };
        let filled = ((rect.right - rect.left) as f32 * bar_fraction(level.peak)).round() as i32;
        if filled <= 0 {
            return;
        }
        let lit = RECT {
            right: rect.left + filled,
            ..rect
        };
        let brush = if level.clipping {
            self.clipping
        } else {
            self.signal
        };
        unsafe { FillRect(hdc, &lit, brush) };
    }

    fn fill_linear(&self, hdc: HDC, rect: RECT, fraction: f32) {
        unsafe { FillRect(hdc, &rect, self.trough) };
        let filled = ((rect.right - rect.left) as f32 * fraction.clamp(0.0, 1.0)).round() as i32;
        if filled <= 0 {
            return;
        }
        let lit = RECT {
            right: rect.left + filled,
            ..rect
        };
        unsafe { FillRect(hdc, &lit, self.signal) };
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.font.0));
            let _ = DeleteObject(HGDIOBJ(self.trough.0));
            let _ = DeleteObject(HGDIOBJ(self.signal.0));
            let _ = DeleteObject(HGDIOBJ(self.clipping.0));
        }
    }
}

const METER_FLOOR_DB: f32 = 60.0;

fn bar_fraction(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * peak.log10() + METER_FLOOR_DB) / METER_FLOOR_DB).clamp(0.0, 1.0)
}

fn meter(top: i32) -> RECT {
    RECT {
        left: FIELD_X,
        top,
        right: FIELD_X + FIELD_WIDTH,
        bottom: top + METER_HEIGHT,
    }
}

fn save_bar() -> RECT {
    meter(SAVE_BAR_Y)
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

fn choose(list: HWND, selector: Selector) {
    let wanted = selector.selected.map_or(-1, |index| index as isize);
    let current = unsafe { SendMessageW(list, CB_GETCURSEL, WPARAM(0), LPARAM(0)) };
    if current.0 != wanted {
        unsafe { SendMessageW(list, CB_SETCURSEL, WPARAM(wanted as usize), LPARAM(0)) };
    }
    enable(list, selector.enabled);
}

fn enable(control: HWND, enabled: bool) {
    unsafe {
        let _ = EnableWindow(control, enabled);
    }
}

fn combo(root: HWND, instance: HINSTANCE, id: u16, top: i32) -> Result<HWND, RunError> {
    child(
        root,
        instance,
        w!("COMBOBOX"),
        WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        FIELD_X,
        top,
        FIELD_WIDTH,
        DROPPED_HEIGHT,
        id,
    )
}

fn button(
    root: HWND,
    instance: HINSTANCE,
    id: u16,
    left: i32,
    width: i32,
) -> Result<HWND, RunError> {
    child(
        root,
        instance,
        w!("BUTTON"),
        WS_TABSTOP,
        left,
        BUTTON_Y,
        width,
        BUTTON_HEIGHT,
        id,
    )
}

fn text(
    root: HWND,
    instance: HINSTANCE,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    style: WINDOW_STYLE,
) -> Result<HWND, RunError> {
    let control = child(
        root,
        instance,
        w!("STATIC"),
        style,
        left,
        top,
        width,
        height,
        0,
    )?;
    unsafe {
        let _ = SetWindowTheme(control, w!(""), w!(""));
    }
    Ok(control)
}

fn label(
    root: HWND,
    instance: HINSTANCE,
    font: HFONT,
    caption: PCWSTR,
    top: i32,
) -> Result<HWND, RunError> {
    let control = text(
        root,
        instance,
        MARGIN,
        top,
        LABEL_WIDTH,
        LABEL_HEIGHT,
        WINDOW_STYLE(0),
    )?;
    unsafe {
        let _ = SetWindowTextW(control, caption);
        SendMessageW(control, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    }
    Ok(control)
}

#[allow(clippy::too_many_arguments)]
fn child(
    root: HWND,
    instance: HINSTANCE,
    class: PCWSTR,
    style: WINDOW_STYLE,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    id: u16,
) -> Result<HWND, RunError> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | style,
            left,
            top,
            width,
            height,
            root,
            HMENU(id as usize as *mut c_void),
            instance,
            None,
        )
    }
    .map_err(|error| RunError::new(format!("creating a window control failed: {error}")))
}

fn message_font() -> HFONT {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some(&mut metrics as *mut _ as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    match read {
        Ok(()) => unsafe { CreateFontIndirectW(&metrics.lfMessageFont) },
        Err(_) => HFONT(unsafe { GetStockObject(DEFAULT_GUI_FONT) }.0),
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | (green as u32) << 8 | (blue as u32) << 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{Level, Levels, Status, Tone, Transport, View};
    use std::sync::Once;
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::{GetComboBoxInfo, COMBOBOXINFO};
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, RegisterClassExW, ShowWindow, CB_GETDROPPEDSTATE,
        CB_SHOWDROPDOWN, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, LB_GETCURSEL, SW_SHOW, WNDCLASSEXW,
        WS_CAPTION, WS_OVERLAPPED, WS_SYSMENU,
    };

    struct Harness {
        root: HWND,
        combo: HWND,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.root);
            }
        }
    }

    unsafe extern "system" fn proc(
        root: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(root, message, wparam, lparam)
    }

    fn harness() -> Harness {
        static REGISTER: Once = Once::new();
        REGISTER.call_once(|| {
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(proc),
                lpszClassName: w!("onerec.combo.test"),
                ..Default::default()
            };
            unsafe { RegisterClassExW(&class) };
        });
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let root = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("onerec.combo.test"),
                w!("combo-test"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CLIENT_WIDTH + 32,
                CLIENT_HEIGHT + 48,
                None,
                None,
                instance,
                None,
            )
        }
        .expect("test window");
        unsafe {
            let _ = ShowWindow(root, SW_SHOW);
        }
        let combo = combo(root, instance, ID_MICROPHONE, MICROPHONE_Y).expect("combo");
        refill(
            combo,
            &[
                "Mic A".into(),
                "Mic B".into(),
                "Mic C".into(),
                "Mic D".into(),
            ],
        );
        Harness { root, combo }
    }

    fn current(combo: HWND) -> isize {
        unsafe { SendMessageW(combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0
    }

    fn list_sel(combo: HWND) -> isize {
        let mut info = COMBOBOXINFO {
            cbSize: std::mem::size_of::<COMBOBOXINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetComboBoxInfo(combo, &mut info) }.expect("combo info");
        unsafe { SendMessageW(info.hwndList, LB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0
    }

    fn dropped(combo: HWND) -> bool {
        unsafe { SendMessageW(combo, CB_GETDROPPEDSTATE, WPARAM(0), LPARAM(0)) }.0 != 0
    }

    fn idle_view(microphone: Option<usize>) -> View {
        View {
            endpoints: None,
            microphone: Selector {
                selected: microphone,
                enabled: true,
            },
            output: Selector {
                selected: Some(0),
                enabled: true,
            },
            quality: Selector {
                selected: Some(ExportQuality::Meeting.index()),
                enabled: true,
            },
            transport: Transport {
                toggle_label: "Start recording",
                toggle_enabled: true,
                save_enabled: false,
                discard_enabled: false,
            },
            elapsed: "00:00".into(),
            levels: Levels {
                microphone: Level::ZERO,
                system: Level::ZERO,
            },
            progress: None,
            status: Status {
                text: String::new(),
                tone: Tone::Neutral,
            },
            ask: None,
        }
    }

    #[test]
    fn show_leaves_hover_highlight_while_the_list_is_dropped() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).expect("controls");
        let combo = controls.microphone_combo();
        refill(
            combo,
            &[
                "Mic A".into(),
                "Mic B".into(),
                "Mic C".into(),
                "Mic D".into(),
            ],
        );
        unsafe {
            SendMessageW(combo, CB_SETCURSEL, WPARAM(2), LPARAM(0));
            SendMessageW(combo, CB_SHOWDROPDOWN, WPARAM(1), LPARAM(0));
            SendMessageW(combo, CB_SETCURSEL, WPARAM(0), LPARAM(0));
        }
        assert_eq!(list_sel(combo), 0);

        controls.set_list_dropped(true);
        controls.show(ui.root, &idle_view(Some(2)));

        assert_eq!(list_sel(combo), 0);
    }

    #[test]
    fn choose_applies_the_committed_row_when_the_list_is_closed() {
        let ui = harness();
        unsafe { SendMessageW(ui.combo, CB_SETCURSEL, WPARAM(2), LPARAM(0)) };
        assert!(!dropped(ui.combo));
        assert_eq!(current(ui.combo), 2);

        choose(
            ui.combo,
            Selector {
                selected: Some(0),
                enabled: true,
            },
        );

        assert_eq!(current(ui.combo), 0);
    }
}
