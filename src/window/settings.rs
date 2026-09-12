use std::cell::{Cell, RefCell};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::theme::Theme;
use crate::prefs::Shortcut;

const SHORTCUT: i32 = 202;
const CLEAR: i32 = 203;
const KEYBOARD_HEADING: i32 = 205;
const HINT: i32 = 207;
const SS_ETCHEDHORZ: u32 = 0x10;
const SHORTCUT_HINT: &str =
    "Press a key combination. Works even in the background.\nClear to disable the shortcut.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    pub shortcut: Shortcut,
}

struct State {
    owner: HWND,
    initial: Settings,
    saved: RefCell<Option<Settings>>,
    error: Cell<bool>,
    bold: Cell<HFONT>,
    theme: Theme,
}

impl State {
    fn new(owner: HWND, initial: Settings) -> Self {
        Self {
            owner,
            initial,
            saved: RefCell::new(None),
            error: Cell::new(false),
            bold: Cell::new(HFONT::default()),
            theme: Theme::new(),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if !self.bold.get().is_invalid() {
            unsafe {
                let _ = DeleteObject(self.bold.get());
            }
        }
    }
}

/// Native modal dialog: Windows supplies DPI scaling, focus traversal, default
/// buttons, Escape/cancel and the nested message loop used by recording timers.
pub(crate) fn ask(owner: HWND, initial: Settings) -> Option<Settings> {
    let state = State::new(owner, initial);
    let template = template();
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.ok()?.into();
    let result = unsafe {
        DialogBoxIndirectParamW(
            instance,
            template.as_ptr().cast(),
            owner,
            Some(dialog_proc),
            LPARAM(&state as *const State as isize),
        )
    };
    if result == -1 {
        unsafe {
            MessageBoxW(
                owner,
                w!("Settings could not be opened. Please try again."),
                w!("onerec"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    let saved = state.saved.borrow_mut().take();
    saved
}

unsafe extern "system" fn dialog_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if message == WM_INITDIALOG {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
    let Some(state) = pointer.as_ref() else {
        return 0;
    };
    match message {
        WM_INITDIALOG => {
            SendDlgItemMessageW(
                hwnd,
                SHORTCUT,
                HKM_SETHOTKEY,
                WPARAM(state.initial.shortcut.0 as usize),
                LPARAM(0),
            );
            SendDlgItemMessageW(hwnd, SHORTCUT, HKM_SETRULES, WPARAM(0), LPARAM(0));
            let font = HFONT(SendMessageW(hwnd, WM_GETFONT, WPARAM(0), LPARAM(0)).0 as *mut _);
            let mut logfont = LOGFONTW::default();
            if GetObjectW(
                font,
                std::mem::size_of::<LOGFONTW>() as i32,
                Some(&mut logfont as *mut _ as *mut _),
            ) != 0
            {
                logfont.lfWeight = FW_SEMIBOLD.0 as i32;
                state.bold.set(CreateFontIndirectW(&logfont));
                for id in [KEYBOARD_HEADING] {
                    SendDlgItemMessageW(
                        hwnd,
                        id,
                        WM_SETFONT,
                        WPARAM(state.bold.get().0 as usize),
                        LPARAM(1),
                    );
                }
            }
            1
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            match id {
                id if id == IDCANCEL.0 => {
                    let _ = EndDialog(hwnd, 0);
                }
                CLEAR => {
                    SendDlgItemMessageW(hwnd, SHORTCUT, HKM_SETHOTKEY, WPARAM(0), LPARAM(0));
                    state.error.set(false);
                    let _ = SetDlgItemTextW(hwnd, HINT, &HSTRING::from(SHORTCUT_HINT));
                    if let Ok(field) = GetDlgItem(hwnd, SHORTCUT) {
                        let _ = SetFocus(field);
                    }
                }
                id if id == IDOK.0 => {
                    let raw =
                        SendDlgItemMessageW(hwnd, SHORTCUT, HKM_GETHOTKEY, WPARAM(0), LPARAM(0)).0
                            as u16;
                    let shortcut = Shortcut(if raw & 0xff == 0 { 0 } else { raw });
                    if super::shell::register_shortcut(state.owner, shortcut).is_err() {
                        state.error.set(true);
                        let _ = SetDlgItemTextW(hwnd, HINT, w!("This shortcut is unavailable. Try another combination or clear it."));
                        if let Ok(field) = GetDlgItem(hwnd, SHORTCUT) {
                            let _ = SetFocus(field);
                        }
                        return 1;
                    }
                    *state.saved.borrow_mut() = Some(Settings { shortcut });
                    let _ = EndDialog(hwnd, 1);
                }
                SHORTCUT => {
                    if state.error.replace(false) {
                        let _ = SetDlgItemTextW(hwnd, HINT, &HSTRING::from(SHORTCUT_HINT));
                    }
                }
                _ => return 0,
            }
            1
        }
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let dc = HDC(wparam.0 as *mut _);
            let id = GetDlgCtrlID(HWND(lparam.0 as *mut _));
            let color = if id == HINT && state.error.get() {
                state.theme.red
            } else if id == HINT {
                state.theme.muted
            } else {
                state.theme.ink
            };
            SetTextColor(dc, color);
            SetBkMode(dc, TRANSPARENT);
            state.theme.background.0 as isize
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            0
        }
        _ => 0,
    }
}

/// Standard dialog template in dialog units, DWORD-aligned for Win32. Kept in
/// code so the same real controls are available in the native preview tests.
fn template() -> Vec<u32> {
    unsafe {
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_HOTKEY_CLASS,
        });
    }
    let items = [
        (
            KEYBOARD_HEADING,
            "STATIC",
            "Keyboard shortcut",
            0,
            [20, 16, 280, 12],
        ),
        (
            209,
            "STATIC",
            "Start / stop &recording",
            0,
            [20, 38, 280, 12],
        ),
        (
            SHORTCUT,
            "msctls_hotkey32",
            "",
            WS_TABSTOP.0,
            [20, 57, 206, 14],
        ),
        (
            CLEAR,
            "BUTTON",
            "&Clear",
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            [234, 54, 66, 20],
        ),
        (HINT, "STATIC", SHORTCUT_HINT, 0, [20, 82, 280, 26]),
        (210, "STATIC", "", SS_ETCHEDHORZ, [0, 114, 320, 1]),
        (
            IDCANCEL.0,
            "BUTTON",
            "Cancel",
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            [156, 126, 68, 20],
        ),
        (
            IDOK.0,
            "BUTTON",
            "&Save",
            WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
            [232, 126, 68, 20],
        ),
    ];
    let mut words = Vec::<u16>::new();
    let dword = |words: &mut Vec<u16>, value: u32| {
        words.extend([value as u16, (value >> 16) as u16]);
    };
    let string = |words: &mut Vec<u16>, value: &str| {
        words.extend(value.encode_utf16());
        words.push(0);
    };
    dword(
        &mut words,
        WS_POPUP.0
            | WS_CAPTION.0
            | WS_SYSMENU.0
            | DS_MODALFRAME as u32
            | DS_SETFONT as u32
            | DS_CENTER as u32,
    );
    dword(&mut words, 0);
    words.extend([items.len() as u16, 0, 0, 320, 158, 0, 0]);
    string(&mut words, "Settings");
    words.push(9);
    string(&mut words, "Segoe UI");
    for (id, class, title, style, rect) in items {
        if words.len() % 2 != 0 {
            words.push(0);
        }
        dword(&mut words, WS_CHILD.0 | WS_VISIBLE.0 | style);
        dword(&mut words, 0);
        words.extend(rect);
        words.push(id as u16);
        string(&mut words, class);
        string(&mut words, title);
        words.push(0);
    }
    if words.len() % 2 != 0 {
        words.push(0);
    }
    words
        .chunks_exact(2)
        .map(|pair| u32::from(pair[0]) | (u32::from(pair[1]) << 16))
        .collect()
}

#[cfg(test)]
pub(super) fn with_preview(owner: HWND, render: impl FnOnce(HWND)) {
    let state = State::new(
        owner,
        Settings {
            shortcut: Shortcut::default(),
        },
    );
    let template = template();
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let dialog = CreateDialogIndirectParamW(
            instance,
            template.as_ptr().cast(),
            owner,
            Some(dialog_proc),
            LPARAM(&state as *const State as isize),
        )
        .unwrap();
        SetWindowPos(
            dialog,
            None,
            -10000,
            -10000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
        let _ = ShowWindow(dialog, SW_SHOWNA);
        render(dialog);
        DestroyWindow(dialog).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_modal(proc: DLGPROC) -> (Option<Settings>, bool) {
        let state = State::new(
            HWND::default(),
            Settings {
                shortcut: Shortcut::default(),
            },
        );
        let template = template();
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
            // Test procedures finish during initialization, before the dialog is
            // shown. They don't inject input or affect the user's running app.
            let result = DialogBoxIndirectParamW(
                instance,
                template.as_ptr().cast(),
                None,
                proc,
                LPARAM(&state as *const State as isize),
            );
            assert_ne!(result, -1);
        }
        let saved = state.saved.borrow_mut().take();
        (saved, state.error.get())
    }

    unsafe extern "system" fn cancel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
        let result = dialog_proc(hwnd, msg, wp, lp);
        if msg == WM_INITDIALOG {
            SendMessageW(hwnd, WM_COMMAND, WPARAM(CLEAR as usize), LPARAM(0));
            SendMessageW(hwnd, WM_COMMAND, WPARAM(IDCANCEL.0 as usize), LPARAM(0));
        }
        result
    }

    unsafe extern "system" fn clear_save_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> isize {
        let result = dialog_proc(hwnd, msg, wp, lp);
        if msg == WM_INITDIALOG {
            SendMessageW(hwnd, WM_COMMAND, WPARAM(CLEAR as usize), LPARAM(0));
            SendMessageW(hwnd, WM_COMMAND, WPARAM(IDOK.0 as usize), LPARAM(0));
        }
        result
    }

    unsafe extern "system" fn reserved_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
        let result = dialog_proc(hwnd, msg, wp, lp);
        if msg == WM_INITDIALOG {
            // F12 is permanently reserved for the Windows debugger.
            SendDlgItemMessageW(hwnd, SHORTCUT, HKM_SETHOTKEY, WPARAM(0x7b), LPARAM(0));
            SendMessageW(hwnd, WM_COMMAND, WPARAM(IDOK.0 as usize), LPARAM(0));
            SendMessageW(hwnd, WM_COMMAND, WPARAM(IDCANCEL.0 as usize), LPARAM(0));
        }
        result
    }

    #[test]
    fn cancel_discards_edits_and_save_can_disable_the_shortcut() {
        assert_eq!(run_modal(Some(cancel_proc)).0, None);
        assert_eq!(
            run_modal(Some(clear_save_proc)),
            (
                Some(Settings {
                    shortcut: Shortcut(0),
                }),
                false
            )
        );
    }

    #[test]
    fn reserved_shortcut_is_rejected_without_committing_settings() {
        assert_eq!(run_modal(Some(reserved_proc)), (None, true));
    }

    #[test]
    fn native_settings_loads_current_values_and_clear_does_not_save() {
        with_preview(HWND::default(), |dialog| unsafe {
            assert_eq!(
                SendDlgItemMessageW(dialog, SHORTCUT, HKM_GETHOTKEY, WPARAM(0), LPARAM(0)).0,
                0x0552
            );
            SendMessageW(dialog, WM_COMMAND, WPARAM(CLEAR as usize), LPARAM(0));
            assert_eq!(
                SendDlgItemMessageW(dialog, SHORTCUT, HKM_GETHOTKEY, WPARAM(0), LPARAM(0)).0,
                0
            );
            let state = &*(GetWindowLongPtrW(dialog, GWLP_USERDATA) as *const State);
            assert_eq!(*state.saved.borrow(), None);
            assert_eq!(state.initial.shortcut, Shortcut::default());
        });
    }
}
