use std::cell::{Cell, RefCell};
use windows::core::{w, HSTRING, PWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::theme::Theme;
use crate::recorder::SettingsValues;
use crate::sidecar;

const TAB: i32 = 201;
const SHORTCUT: i32 = 202;
const CLEAR: i32 = 203;
const HINT: i32 = 207;
const LABEL_SHORTCUT: i32 = 209;
const API_KEY: i32 = 220;
const API_URL: i32 = 221;
const API_MODEL: i32 = 222;
const API_HINT: i32 = 224;
const LABEL_KEY: i32 = 225;
const LABEL_URL: i32 = 226;
const LABEL_TRANSCRIBE: i32 = 227;
const API_NOTES_MODEL: i32 = 228;
const LABEL_NOTES_MODEL: i32 = 229;
const NEST: i32 = 230;
const NOTES_PROMPT: i32 = 231;
const NOTES_HINT: i32 = 232;
const PROMPT_LIMIT: usize = 32767;
const RECORDING_IDS: [i32; 3] = [LABEL_SHORTCUT, SHORTCUT, HINT];
const API_IDS: [i32; 10] = [
    LABEL_KEY,
    API_KEY,
    LABEL_URL,
    API_URL,
    LABEL_TRANSCRIBE,
    API_MODEL,
    LABEL_NOTES_MODEL,
    API_NOTES_MODEL,
    NEST,
    API_HINT,
];
const NOTES_IDS: [i32; 2] = [NOTES_PROMPT, NOTES_HINT];
const SHORTCUT_HINT: &str =
    "Press a key combination. Works even in the background.\nClear to disable the shortcut.";
const API_HELP: &str =
    "Works with Groq, OpenAI, and other OpenAI-compatible APIs.\nThe key is stored in Windows Credential Manager, not in onerec.ini.";
const NOTES_HELP: &str = "Leave blank to use the built-in default.";
const PROMPT_TOO_LONG: &str = "This prompt is too long.";

#[derive(Debug)]
struct PromptTooLong;

struct State {
    owner: HWND,
    initial: SettingsValues,
    saved: RefCell<Option<SettingsValues>>,
    error: Cell<bool>,
    api_error: Cell<bool>,
    notes_error: Cell<bool>,
    theme: Theme,
}

impl State {
    fn new(owner: HWND, initial: SettingsValues) -> Self {
        Self {
            owner,
            initial,
            saved: RefCell::new(None),
            error: Cell::new(false),
            api_error: Cell::new(false),
            notes_error: Cell::new(false),
            theme: Theme::new(),
        }
    }
}

pub(crate) fn ask(owner: HWND, initial: SettingsValues) -> Option<SettingsValues> {
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
            insert_tab(hwnd, 0, "Recording");
            insert_tab(hwnd, 1, "API");
            insert_tab(hwnd, 2, "Notes");
            SendDlgItemMessageW(
                hwnd,
                NOTES_PROMPT,
                EM_LIMITTEXT,
                WPARAM(PROMPT_LIMIT),
                LPARAM(0),
            );
            let _ = SetDlgItemTextW(
                hwnd,
                API_KEY,
                &HSTRING::from(state.initial.api_key.as_str()),
            );
            let _ = SetDlgItemTextW(
                hwnd,
                API_URL,
                &HSTRING::from(state.initial.transcribe_url.as_str()),
            );
            let _ = SetDlgItemTextW(
                hwnd,
                API_MODEL,
                &HSTRING::from(state.initial.transcribe_model.as_str()),
            );
            let _ = SetDlgItemTextW(
                hwnd,
                API_NOTES_MODEL,
                &HSTRING::from(state.initial.notes_model.as_str()),
            );
            let _ = SetDlgItemTextW(
                hwnd,
                NOTES_PROMPT,
                &HSTRING::from(state.initial.notes_prompt.as_str()),
            );
            SendDlgItemMessageW(
                hwnd,
                NEST,
                BM_SETCHECK,
                WPARAM(if state.initial.nest {
                    BST_CHECKED.0 as usize
                } else {
                    BST_UNCHECKED.0 as usize
                }),
                LPARAM(0),
            );
            show_tab(hwnd);
            1
        }
        WM_NOTIFY => {
            let header = &*(lparam.0 as *const NMHDR);
            if header.idFrom == TAB as usize && header.code == TCN_SELCHANGE as u32 {
                show_tab(hwnd);
            }
            0
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
                    if !commit(hwnd, state) {
                        return 1;
                    }
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
            } else if id == API_HINT && state.api_error.get() {
                state.theme.red
            } else if id == NOTES_HINT && state.notes_error.get() {
                state.theme.red
            } else if id == HINT || id == API_HINT || id == NOTES_HINT {
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

fn commit(hwnd: HWND, state: &State) -> bool {
    let raw = unsafe { SendDlgItemMessageW(hwnd, SHORTCUT, HKM_GETHOTKEY, WPARAM(0), LPARAM(0)).0 }
        as u16;
    let shortcut = crate::prefs::Shortcut(if raw & 0xff == 0 { 0 } else { raw });
    if super::shell::register_shortcut(state.owner, shortcut).is_err() {
        state.error.set(true);
        select_tab(hwnd, 0);
        let _ = unsafe { SetDlgItemTextW(hwnd, HINT, w!("This shortcut is unavailable. Try another combination or clear it.")) };
        focus(hwnd, SHORTCUT);
        return false;
    }
    let api_key = dlg_text(hwnd, API_KEY).unwrap_or_default();
    let transcribe_url = dlg_text(hwnd, API_URL).unwrap_or_default();
    let transcribe_model = dlg_text(hwnd, API_MODEL).unwrap_or_default();
    let notes_model = dlg_text(hwnd, API_NOTES_MODEL).unwrap_or_default();
    if let Err(message) = sidecar::validate_fields(&transcribe_url, &transcribe_model, &notes_model)
    {
        state.api_error.set(true);
        select_tab(hwnd, 1);
        let _ = unsafe { SetDlgItemTextW(hwnd, API_HINT, &HSTRING::from(message.as_str())) };
        let field = if message.contains("URL") {
            API_URL
        } else if message.contains("transcribe") {
            API_MODEL
        } else {
            API_NOTES_MODEL
        };
        focus(hwnd, field);
        return false;
    }
    let notes_prompt = match dlg_text(hwnd, NOTES_PROMPT) {
        Err(PromptTooLong) => {
            state.notes_error.set(true);
            select_tab(hwnd, 2);
            let _ = unsafe { SetDlgItemTextW(hwnd, NOTES_HINT, &HSTRING::from(PROMPT_TOO_LONG)) };
            focus(hwnd, NOTES_PROMPT);
            return false;
        }
        Ok(text) => text,
    };
    let nest = unsafe { SendDlgItemMessageW(hwnd, NEST, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 }
        == BST_CHECKED.0 as isize;
    *state.saved.borrow_mut() = Some(SettingsValues {
        shortcut,
        api_key,
        transcribe_url,
        transcribe_model,
        notes_model,
        nest,
        notes_prompt,
    });
    let _ = unsafe { EndDialog(hwnd, 1) };
    true
}

fn focus(hwnd: HWND, id: i32) {
    if let Ok(field) = unsafe { GetDlgItem(hwnd, id) } {
        let _ = unsafe { SetFocus(field) };
    }
}

fn insert_tab(hwnd: HWND, index: usize, title: &str) {
    let text = HSTRING::from(title);
    let mut item = TCITEMW {
        mask: TCIF_TEXT,
        pszText: PWSTR(text.as_ptr() as *mut u16),
        ..Default::default()
    };
    unsafe {
        SendDlgItemMessageW(
            hwnd,
            TAB,
            TCM_INSERTITEMW,
            WPARAM(index),
            LPARAM(&mut item as *mut TCITEMW as isize),
        );
    }
}

fn select_tab(hwnd: HWND, index: usize) {
    unsafe {
        SendDlgItemMessageW(hwnd, TAB, TCM_SETCURSEL, WPARAM(index), LPARAM(0));
    }
    show_tab(hwnd);
}

fn show_tab(hwnd: HWND) {
    let sel = unsafe { SendDlgItemMessageW(hwnd, TAB, TCM_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    set_visible(hwnd, &RECORDING_IDS, sel == 0);
    set_visible(hwnd, &API_IDS, sel == 1);
    set_visible(hwnd, &NOTES_IDS, sel == 2);
}

fn set_visible(hwnd: HWND, ids: &[i32], on: bool) {
    let cmd = if on { SW_SHOW } else { SW_HIDE };
    for id in ids {
        if let Ok(control) = unsafe { GetDlgItem(hwnd, *id) } {
            unsafe {
                let _ = ShowWindow(control, cmd);
            }
        }
    }
}

fn dlg_text(hwnd: HWND, id: i32) -> Result<String, PromptTooLong> {
    let Ok(control) = (unsafe { GetDlgItem(hwnd, id) }) else {
        return Ok(String::new());
    };
    let length = unsafe { GetWindowTextLengthW(control) } as usize;
    if id == NOTES_PROMPT && length == PROMPT_LIMIT {
        return Err(PromptTooLong);
    }
    let mut buf = vec![0u16; length + 1];
    let n = unsafe { GetDlgItemTextW(hwnd, id, &mut buf) } as usize;
    Ok(String::from_utf16_lossy(&buf[..n]))
}

fn template() -> Vec<u32> {
    unsafe {
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TAB_CLASSES | ICC_HOTKEY_CLASS,
        });
    }
    let items = [
        (
            TAB,
            "SysTabControl32",
            "",
            WS_TABSTOP.0 | WS_CLIPSIBLINGS.0,
            [12, 8, 296, 274],
        ),
        (
            LABEL_SHORTCUT,
            "STATIC",
            "Start / stop &recording",
            0,
            [20, 36, 280, 12],
        ),
        (
            SHORTCUT,
            "msctls_hotkey32",
            "",
            WS_TABSTOP.0,
            [20, 52, 280, 14],
        ),
        (HINT, "STATIC", SHORTCUT_HINT, 0, [20, 74, 280, 40]),
        (LABEL_KEY, "STATIC", "API &key", 0, [20, 36, 280, 12]),
        (
            API_KEY,
            "EDIT",
            "",
            WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
            [20, 50, 280, 14],
        ),
        (LABEL_URL, "STATIC", "Base &URL", 0, [20, 70, 280, 12]),
        (
            API_URL,
            "EDIT",
            "",
            WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
            [20, 84, 280, 14],
        ),
        (
            LABEL_TRANSCRIBE,
            "STATIC",
            "Transcribe model",
            0,
            [20, 104, 280, 12],
        ),
        (
            API_MODEL,
            "EDIT",
            "",
            WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
            [20, 118, 280, 14],
        ),
        (
            LABEL_NOTES_MODEL,
            "STATIC",
            "Notes model",
            0,
            [20, 138, 280, 12],
        ),
        (
            API_NOTES_MODEL,
            "EDIT",
            "",
            WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
            [20, 152, 280, 14],
        ),
        (
            NEST,
            "BUTTON",
            "Save take in its own folder",
            WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            [20, 174, 280, 14],
        ),
        (API_HINT, "STATIC", API_HELP, 0, [20, 196, 280, 50]),
        (
            NOTES_PROMPT,
            "EDIT",
            "",
            WS_TABSTOP.0
                | WS_BORDER.0
                | WS_VSCROLL.0
                | ES_MULTILINE as u32
                | ES_WANTRETURN as u32
                | ES_AUTOVSCROLL as u32,
            [20, 36, 280, 200],
        ),
        (NOTES_HINT, "STATIC", NOTES_HELP, 0, [20, 242, 280, 26]),
        (
            CLEAR,
            "BUTTON",
            "&Clear",
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            [20, 298, 68, 20],
        ),
        (
            IDCANCEL.0,
            "BUTTON",
            "Cancel",
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            [156, 298, 68, 20],
        ),
        (
            IDOK.0,
            "BUTTON",
            "&Save",
            WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
            [232, 298, 68, 20],
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
    words.extend([items.len() as u16, 0, 0, 320, 326, 0, 0]);
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
    let state = State::new(owner, SettingsValues::default());
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
    use crate::prefs::Shortcut;

    fn run_modal(proc: DLGPROC) -> (Option<SettingsValues>, bool) {
        let state = State::new(HWND::default(), SettingsValues::default());
        let template = template();
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
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

    unsafe extern "system" fn invalid_url_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> isize {
        let result = dialog_proc(hwnd, msg, wp, lp);
        if msg == WM_INITDIALOG {
            SendMessageW(hwnd, WM_COMMAND, WPARAM(CLEAR as usize), LPARAM(0));
            let _ = SetDlgItemTextW(hwnd, API_URL, w!("not-a-url"));
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
                Some(SettingsValues {
                    shortcut: Shortcut(0),
                    ..SettingsValues::default()
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
    fn invalid_transcription_url_is_rejected_without_committing_settings() {
        let (saved, error) = run_modal(Some(invalid_url_proc));
        assert_eq!(saved, None);
        assert!(!error);
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
            assert_eq!(dlg_text(dialog, API_URL).unwrap(), "");
            assert_eq!(dlg_text(dialog, API_MODEL).unwrap(), "");
            assert_eq!(dlg_text(dialog, API_NOTES_MODEL).unwrap(), "");
            assert_eq!(dlg_text(dialog, API_KEY).unwrap(), "");
            assert_eq!(dlg_text(dialog, NOTES_PROMPT).unwrap(), "");
            assert_eq!(
                SendDlgItemMessageW(dialog, NEST, BM_GETCHECK, WPARAM(0), LPARAM(0)).0,
                0
            );
            assert_eq!(dlg_text(dialog, LABEL_TRANSCRIBE).unwrap(), "Transcribe model");
            assert_eq!(dlg_text(dialog, LABEL_NOTES_MODEL).unwrap(), "Notes model");
            let url = dlg_text(dialog, API_URL).unwrap();
            assert!(!url.contains("groq"));
            assert!(!dlg_text(dialog, API_MODEL).unwrap().contains("whisper-large"));
        });
    }

    #[test]
    fn dlg_text_reads_a_prompt_longer_than_2048() {
        with_preview(HWND::default(), |dialog| unsafe {
            let long = "a".repeat(3000);
            let _ = SetDlgItemTextW(dialog, NOTES_PROMPT, &HSTRING::from(long.as_str()));
            assert_eq!(dlg_text(dialog, NOTES_PROMPT).unwrap().len(), 3000);
        });
    }
}
