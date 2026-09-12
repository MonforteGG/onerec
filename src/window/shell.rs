use std::cell::RefCell;
use std::ffi::c_void;

use ::windows::core::{w, HSTRING, PCWSTR};
use ::windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use ::windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, FillRect, GetSysColorBrush, InvalidateRect, UpdateWindow, COLOR_WINDOW,
    HDC, PAINTSTRUCT,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::Controls::{DRAWITEMSTRUCT, WM_MOUSELEAVE};
use ::windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    IsWindowEnabled, RegisterHotKey, TrackMouseEvent, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, TME_LEAVE, TRACKMOUSEEVENT,
};
use ::windows::Win32::UI::Shell::ShellExecuteW;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetParent, GetWindowLongPtrW, IsDialogMessageW, KillTimer, LoadCursorW, LoadImageW,
    MessageBoxW, PostQuitMessage, RegisterClassExW, SendMessageW, SetCursor, SetTimer,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, BM_SETSTATE, BN_CLICKED, CBN_CLOSEUP,
    CBN_DROPDOWN, CBN_SELCHANGE, CBN_SELENDOK, CB_GETCURSEL, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    GWLP_USERDATA, GWLP_WNDPROC, HCURSOR, HICON, IDC_ARROW, IDC_WAIT, IDYES, IMAGE_ICON,
    LR_DEFAULTCOLOR, MB_ICONWARNING, MB_YESNO, MSG, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON,
    SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CANCELMODE, WM_CAPTURECHANGED, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_DEVICECHANGE, WM_DRAWITEM, WM_ENABLE, WM_ERASEBKGND,
    WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_NCDESTROY, WM_PAINT, WM_PRINTCLIENT, WM_SETFOCUS, WM_SETTEXT, WM_SHOWWINDOW, WM_TIMER,
    WM_UPDATEUISTATE, WNDCLASSEXW, WNDPROC, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
};

use super::paint::{
    Controls, CLIENT_HEIGHT, CLIENT_WIDTH, ID_DISCARD, ID_FOLDER, ID_MICROPHONE, ID_OUTPUT,
    ID_PAUSE, ID_QUALITY, ID_REFRESH, ID_SAVE, ID_SETTINGS, ID_STOP, ID_TOGGLE,
};
use super::save_dialog;
use crate::recorder::{Ask, Intent, Phase, Recorder};
use crate::RunError;
use ::windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetClientRect, IsIconic, SetWindowPos, MB_DEFBUTTON2, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_SHOWNORMAL, WM_DPICHANGED, WM_SETICON, WM_SETTINGCHANGE, WM_SIZE,
    WM_SYSCOLORCHANGE, WM_THEMECHANGED, WS_CLIPCHILDREN,
};

const CLASS_NAME: PCWSTR = w!("onerec.window");
const TITLE: PCWSTR = w!("onerec");
const TOGGLE_HOTKEY: i32 = 1;
const TICK_TIMER: usize = 1;
const STYLE: WINDOW_STYLE = WINDOW_STYLE(
    WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_CLIPCHILDREN.0,
);

pub(crate) fn run(recorder: Recorder) -> Result<(), RunError> {
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
        .map_err(|error| RunError::new(format!("reading the module handle failed: {error}")))?
        .into();
    register_class(instance)?;
    let root = create_window(instance)?;
    let controls = Controls::create(root, instance)?;
    let icons = Icons::load(root, instance);

    // The shell lives on this stack frame until GetMessageW returns WM_QUIT, so the
    // GWLP_USERDATA pointer cannot outlive it.
    let shell = RefCell::new(Shell {
        recorder,
        controls,
        icons,
        timer_ms: None,
        phase: Phase::Idle,
        saved_path: None,
        modal: false,
        refresh_after_picker: false,
    });
    unsafe { SetWindowLongPtrW(root, GWLP_USERDATA, &shell as *const _ as isize) };
    install_command_button_subclasses(&shell.borrow().controls);
    let hotkey = register_shortcut(root, shell.borrow().recorder.shortcut());
    dispatch(
        root,
        match hotkey {
            Ok(()) => Intent::Tick,
            Err(_) => Intent::HotkeyUnavailable,
        },
    );
    unsafe {
        let _ = ShowWindow(root, SW_SHOW);
        let _ = UpdateWindow(root);
    }

    pump(root)
}

fn pump(root: HWND) -> Result<(), RunError> {
    let mut message = MSG::default();
    loop {
        let more = unsafe { GetMessageW(&mut message, None, 0, 0) };
        match more.0 {
            0 => return Ok(()),
            -1 => {
                return Err(RunError::new(format!(
                    "the message loop failed: {}",
                    ::windows::core::Error::from_win32()
                )))
            }
            _ => unsafe {
                if !IsDialogMessageW(root, &message).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            },
        }
    }
}

struct Shell {
    recorder: Recorder,
    controls: Controls,
    icons: Icons,
    timer_ms: Option<u32>,
    phase: Phase,
    saved_path: Option<std::path::PathBuf>,
    modal: bool,
    refresh_after_picker: bool,
}

// Release the borrow before opening a native modal dialog: its nested message
// loop must still be able to paint the window and observe the export worker.
fn dispatch(root: HWND, intent: Intent) {
    let Some(Some(view)) = with_shell(root, |shell| {
        if shell.modal && !matches!(intent, Intent::Tick) {
            return None;
        }
        let blocking = matches!(
            intent,
            Intent::Toggle | Intent::Start | Intent::Stop | Intent::DiscardAndClose
        );
        let _wait = blocking.then(WaitCursor::show);
        let view = shell.recorder.apply(intent);
        shell.phase = view.phase;
        shell.saved_path.clone_from(&view.saved_path);
        shell.controls.show(root, &view);
        shell.sync_timer(root);
        Some(view)
    }) else {
        return;
    };
    match view.ask {
        None => {}
        Some(Ask::Close) => unsafe {
            let _ = DestroyWindow(root);
        },
        Some(Ask::ConfirmClose) => {
            if modal(root, || confirmed_close(root)) {
                dispatch(root, Intent::DiscardAndClose);
            }
        }
        Some(Ask::ConfirmDiscard) => {
            let question = HSTRING::from(format!(
                "Discard this {} recording? It has not been saved.",
                view.elapsed
            ));
            if modal(root, || unsafe {
                MessageBoxW(
                    root,
                    &question,
                    TITLE,
                    MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                )
            }) == IDYES
            {
                dispatch(root, Intent::Discard);
            }
        }
        Some(Ask::SaveDestination(prompt)) => {
            let next = match modal(root, || save_dialog::ask_destination(root, &prompt)) {
                Some(path) => Intent::SaveTo(path),
                None => Intent::CancelSave,
            };
            dispatch(root, next);
        }
        Some(Ask::Settings { shortcut }) => {
            // Release the current shortcut so the field can capture that same
            // combination. Save registers the new one; Cancel restores this one.
            unsafe { let _ = UnregisterHotKey(root, TOGGLE_HOTKEY); }
            let initial = super::settings::Settings { shortcut };
            if let Some(next) = modal(root, || super::settings::ask(root, initial)) {
                dispatch(root, Intent::SetSettings { shortcut: next.shortcut });
            } else if register_shortcut(root, shortcut).is_err() {
                dispatch(root, Intent::HotkeyUnavailable);
            }
        }
    }
}

pub(super) fn register_shortcut(root: HWND, shortcut: crate::prefs::Shortcut) -> ::windows::core::Result<()> {
    if shortcut.0 == 0 { return Ok(()); }
    let flags = shortcut.0 >> 8;
    let mut modifiers = MOD_NOREPEAT;
    if flags & 1 != 0 { modifiers |= MOD_SHIFT; }
    if flags & 2 != 0 { modifiers |= MOD_CONTROL; }
    if flags & 4 != 0 { modifiers |= MOD_ALT; }
    unsafe { RegisterHotKey(root, TOGGLE_HOTKEY, modifiers, u32::from(shortcut.0 & 0xff)) }
}

fn modal<T>(root: HWND, dialog: impl FnOnce() -> T) -> T {
    with_shell(root, |shell| shell.modal = true);
    let result = dialog();
    with_shell(root, |shell| shell.modal = false);
    result
}

impl Shell {
    fn sync_timer(&mut self, root: HWND) {
        let wanted = self.phase.timer_ms(unsafe { IsIconic(root) }.as_bool());
        if wanted == self.timer_ms {
            return;
        }
        unsafe {
            let _ = KillTimer(root, TICK_TIMER);
            if let Some(interval) = wanted {
                if SetTimer(root, TICK_TIMER, interval, None) == 0 {
                    // Keep the window usable so the take can still be stopped/saved.
                    MessageBoxW(root, w!("The display timer could not start. Close other applications and try again."), TITLE, MB_ICONWARNING);
                    self.timer_ms = None;
                    return;
                }
            }
        }
        self.timer_ms = wanted;
    }

    fn open_folder(&self, root: HWND) {
        if let Some(folder) = self.saved_path.as_deref().and_then(std::path::Path::parent) {
            unsafe {
                ShellExecuteW(
                    root,
                    w!("open"),
                    &HSTRING::from(folder.as_os_str()),
                    None,
                    None,
                    SW_SHOWNORMAL,
                );
            }
        }
    }
}

fn confirmed_close(root: HWND) -> bool {
    let answer = unsafe {
        MessageBoxW(
            root,
            w!("This take has not been saved. Close onerec and discard it?"),
            TITLE,
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    answer == IDYES
}

fn register_class(instance: HINSTANCE) -> Result<(), RunError> {
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(RunError::new(format!(
            "registering the window class failed: {}",
            ::windows::core::Error::from_win32()
        )));
    }
    Ok(())
}

// MAKEINTRESOURCEW(1) is an integer resource identifier, never dereferenced.
#[allow(clippy::manual_dangling_ptr)]
fn app_icon(instance: HINSTANCE, small: bool, dpi: u32) -> HICON {
    let (width, height) = if small {
        (SM_CXSMICON, SM_CYSMICON)
    } else {
        (SM_CXICON, SM_CYICON)
    };
    // Each DPI-specific handle is owned by Icons; avoid the LR_SHARED size cache.
    unsafe {
        LoadImageW(
            instance,
            PCWSTR(1usize as *const u16),
            IMAGE_ICON,
            GetSystemMetricsForDpi(width, dpi),
            GetSystemMetricsForDpi(height, dpi),
            LR_DEFAULTCOLOR,
        )
    }
    .map(|handle| HICON(handle.0))
    .unwrap_or_default()
}

struct Icons {
    large: HICON,
    small: HICON,
}
impl Icons {
    fn load(root: HWND, instance: HINSTANCE) -> Self {
        let dpi = unsafe { GetDpiForWindow(root) }.max(96);
        let icons = Self {
            large: app_icon(instance, false, dpi),
            small: app_icon(instance, true, dpi),
        };
        unsafe {
            SendMessageW(root, WM_SETICON, WPARAM(1), LPARAM(icons.large.0 as isize));
            SendMessageW(root, WM_SETICON, WPARAM(0), LPARAM(icons.small.0 as isize));
        }
        icons
    }
}
impl Drop for Icons {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.large);
            let _ = DestroyIcon(self.small);
        }
    }
}

fn create_window(instance: HINSTANCE) -> Result<HWND, RunError> {
    let dpi = unsafe { GetDpiForSystem() }.max(96);
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: super::paint::scale(CLIENT_WIDTH, dpi),
        bottom: super::paint::scale(CLIENT_HEIGHT, dpi),
    };
    unsafe { AdjustWindowRectExForDpi(&mut frame, STYLE, false, WINDOW_EX_STYLE(0), dpi) }
        .map_err(|error| RunError::new(format!("sizing the window failed: {error}")))?;
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            TITLE,
            STYLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            frame.right - frame.left,
            frame.bottom - frame.top,
            None,
            None,
            instance,
            None,
        )
    }
    .map_err(|error| RunError::new(format!("opening the recorder window failed: {error}")))
}

/// Win32 re-enters the window procedure from a modal dialog's own message pump.
fn with_shell<T>(root: HWND, body: impl FnOnce(&mut Shell) -> T) -> Option<T> {
    let pointer = unsafe { GetWindowLongPtrW(root, GWLP_USERDATA) } as *const RefCell<Shell>;
    let cell = unsafe { pointer.as_ref() }?;
    let mut shell = cell.try_borrow_mut().ok()?;
    Some(body(&mut shell))
}

fn with_shell_read<T>(root: HWND, body: impl FnOnce(&Shell) -> T) -> Option<T> {
    let pointer = unsafe { GetWindowLongPtrW(root, GWLP_USERDATA) } as *const RefCell<Shell>;
    let cell = unsafe { pointer.as_ref() }?;
    if let Ok(shell) = cell.try_borrow() {
        return Some(body(&shell));
    }
    // WM_PAINT can nest inside show()'s exclusive borrow. The shell is not
    // moved; painting only reads the command-button brushes and caption.
    Some(body(unsafe { &*cell.as_ptr() }))
}

fn install_command_button_subclasses(controls: &Controls) {
    unsafe {
        for hwnd in controls.command_buttons() {
            let previous = SetWindowLongPtrW(
                hwnd,
                GWLP_WNDPROC,
                command_button_proc as *const () as isize,
            );
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, previous);
        }
    }
}

unsafe fn original_button_proc(hwnd: HWND) -> WNDPROC {
    std::mem::transmute(GetWindowLongPtrW(hwnd, GWLP_USERDATA))
}

unsafe extern "system" fn command_button_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let original = original_button_proc(hwnd);
    match message {
        WM_PAINT => {
            let paints = parent_of(hwnd)
                .and_then(|root| {
                    with_shell_read(root, |shell| shell.controls.paints_command_button(hwnd))
                })
                .unwrap_or(false);
            if !paints {
                return CallWindowProcW(original, hwnd, message, wparam, lparam);
            }
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            parent_of(hwnd).and_then(|root| {
                with_shell_read(root, |shell| shell.controls.paint_command_button(hwnd, hdc))
            });
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            let hdc = HDC(wparam.0 as *mut c_void);
            if parent_of(hwnd)
                .and_then(|root| {
                    with_shell_read(root, |shell| shell.controls.paint_command_button(hwnd, hdc))
                })
                .unwrap_or(false)
            {
                LRESULT(0)
            } else {
                CallWindowProcW(original, hwnd, message, wparam, lparam)
            }
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let x = lparam.0 as i16 as i32;
            let y = (lparam.0 >> 16) as i16 as i32;
            let hovered = IsWindowEnabled(hwnd).as_bool()
                && x >= 0
                && x < rect.right
                && y >= 0
                && y < rect.bottom;
            if update_button_hover(hwnd, hovered) && hovered {
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    ..Default::default()
                };
                let _ = TrackMouseEvent(&mut tracking);
            }
            CallWindowProcW(original, hwnd, message, wparam, lparam)
        }
        WM_MOUSELEAVE => {
            update_button_hover(hwnd, false);
            CallWindowProcW(original, hwnd, message, wparam, lparam)
        }
        WM_SETFOCUS | WM_KILLFOCUS | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_KEYDOWN | WM_KEYUP
        | WM_ENABLE | WM_SETTEXT | WM_UPDATEUISTATE | BM_SETSTATE | WM_CAPTURECHANGED
        | WM_CANCELMODE | WM_SHOWWINDOW => {
            if (message == WM_ENABLE || message == WM_SHOWWINDOW) && wparam.0 == 0
                || message == WM_CANCELMODE
            {
                update_button_hover(hwnd, false);
            }
            let result = CallWindowProcW(original, hwnd, message, wparam, lparam);
            let _ = InvalidateRect(hwnd, None, false);
            result
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, GetWindowLongPtrW(hwnd, GWLP_USERDATA));
            CallWindowProcW(original, hwnd, message, wparam, lparam)
        }
        _ => CallWindowProcW(original, hwnd, message, wparam, lparam),
    }
}

fn parent_of(hwnd: HWND) -> Option<HWND> {
    let parent = unsafe { GetParent(hwnd).ok()? };
    if parent.0.is_null() {
        None
    } else {
        Some(parent)
    }
}

fn update_button_hover(hwnd: HWND, hovered: bool) -> bool {
    let changed = parent_of(hwnd)
        .and_then(|root| {
            with_shell_read(root, |shell| shell.controls.set_button_hover(hwnd, hovered))
        })
        .unwrap_or(false);
    if changed {
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
    }
    changed
}

unsafe extern "system" fn wnd_proc(
    root: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COMMAND => {
            if (wparam.0 & 0xffff) as u16 == ID_FOLDER {
                with_shell(root, |shell| shell.open_folder(root));
                return LRESULT(0);
            }
            let notification = ((wparam.0 >> 16) & 0xffff) as u32;
            let list_dropped =
                with_shell(root, |shell| shell.controls.list_dropped()).unwrap_or(false);
            if let Some(intent) = command(wparam, lparam, list_dropped) {
                dispatch(root, intent);
            }
            match notification {
                CBN_DROPDOWN => {
                    with_shell(root, |shell| shell.controls.set_list_dropped(true));
                }
                CBN_CLOSEUP => {
                    let refresh = with_shell(root, |shell| {
                        shell.controls.set_list_dropped(false);
                        std::mem::take(&mut shell.refresh_after_picker)
                    })
                    .unwrap_or(false);
                    dispatch(
                        root,
                        if refresh {
                            Intent::RefreshEndpoints
                        } else {
                            Intent::Tick
                        },
                    );
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER | WM_HOTKEY | WM_DEVICECHANGE | WM_CLOSE => {
            // Keep combo row indices tied to the currently displayed endpoint
            // list until the user commits or cancels the open picker.
            if message == WM_DEVICECHANGE
                && with_shell(root, |shell| {
                    if shell.controls.list_dropped() {
                        shell.refresh_after_picker = true;
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false)
            {
                return LRESULT(0);
            }
            if let Some(intent) = translate(message, wparam, lparam) {
                dispatch(root, intent);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(root, &mut paint);
            with_shell(root, |shell| shell.controls.draw_meters(hdc));
            let _ = EndPaint(root, &paint);
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            with_shell(root, |shell| {
                shell.controls.draw_meters(HDC(wparam.0 as *mut c_void))
            });
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let mut rect = RECT::default();
            let _ = GetClientRect(root, &mut rect);
            let brush = with_shell(root, |shell| shell.controls.background())
                .unwrap_or_else(|| GetSysColorBrush(COLOR_WINDOW));
            FillRect(HDC(wparam.0 as *mut c_void), &rect, brush);
            LRESULT(1)
        }
        WM_DRAWITEM => {
            if let Some(draw) = (lparam.0 as *const DRAWITEMSTRUCT).as_ref() {
                if with_shell_read(root, |shell| {
                    shell.controls.paint_command_button(draw.hwndItem, draw.hDC)
                })
                .unwrap_or(false)
                {
                    return LRESULT(1);
                }
            }
            DefWindowProcW(root, message, wparam, lparam)
        }
        WM_DPICHANGED => {
            let suggested = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(
                root,
                None,
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            with_shell(root, |shell| {
                shell
                    .controls
                    .refresh_theme(root, (wparam.0 & 0xffff) as u32);
                if let Ok(module) = GetModuleHandleW(None) {
                    shell.icons = Icons::load(root, module.into());
                }
            });
            dispatch(root, Intent::Tick);
            LRESULT(0)
        }
        WM_THEMECHANGED | WM_SYSCOLORCHANGE | WM_SETTINGCHANGE => {
            with_shell(root, |shell| {
                shell.controls.refresh_theme(root, GetDpiForWindow(root));
            });
            dispatch(root, Intent::Tick);
            DefWindowProcW(root, message, wparam, lparam)
        }
        WM_SIZE => {
            with_shell(root, |shell| shell.sync_timer(root));
            DefWindowProcW(root, message, wparam, lparam)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut c_void);
            let control = HWND(lparam.0 as *mut c_void);
            let brush = with_shell(root, |shell| shell.controls.color_static(control, hdc))
                .unwrap_or_else(|| GetSysColorBrush(COLOR_WINDOW));
            LRESULT(brush.0 as isize)
        }
        WM_DESTROY => {
            let _ = UnregisterHotKey(root, TOGGLE_HOTKEY);
            let _ = KillTimer(root, TICK_TIMER);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(root, message, wparam, lparam),
    }
}

fn translate(message: u32, wparam: WPARAM, _lparam: LPARAM) -> Option<Intent> {
    match message {
        WM_TIMER if wparam.0 == TICK_TIMER => Some(Intent::Tick),
        WM_HOTKEY if wparam.0 as i32 == TOGGLE_HOTKEY => Some(Intent::Toggle),
        WM_DEVICECHANGE => Some(Intent::RefreshEndpoints),
        WM_CLOSE => Some(Intent::Closing),
        _ => None,
    }
}

fn command(wparam: WPARAM, lparam: LPARAM, list_dropped: bool) -> Option<Intent> {
    let id = (wparam.0 & 0xffff) as u16;
    let notification = ((wparam.0 >> 16) & 0xffff) as u32;
    let control = HWND(lparam.0 as *mut c_void);
    match (notification, id) {
        (CBN_SELENDOK, ID_MICROPHONE) => Some(Intent::ChooseMicrophone(selection(control)?)),
        (CBN_SELENDOK, ID_OUTPUT) => Some(Intent::ChooseOutput(selection(control)?)),
        (CBN_SELENDOK, ID_QUALITY) => Some(Intent::ChooseQuality(selection(control)?)),
        (CBN_SELCHANGE, ID_MICROPHONE) if !list_dropped => {
            Some(Intent::ChooseMicrophone(selection(control)?))
        }
        (CBN_SELCHANGE, ID_OUTPUT) if !list_dropped => {
            Some(Intent::ChooseOutput(selection(control)?))
        }
        (CBN_SELCHANGE, ID_QUALITY) if !list_dropped => {
            Some(Intent::ChooseQuality(selection(control)?))
        }
        (CBN_DROPDOWN, ID_MICROPHONE | ID_OUTPUT) => Some(Intent::RefreshEndpoints),
        (BN_CLICKED, ID_TOGGLE) => Some(Intent::Start),
        (BN_CLICKED, ID_STOP) => Some(Intent::Stop),
        (BN_CLICKED, ID_PAUSE) => Some(Intent::Pause),
        (BN_CLICKED, ID_SAVE) => Some(Intent::Save),
        (BN_CLICKED, ID_SETTINGS) => Some(Intent::OpenSettings),
        (BN_CLICKED, ID_DISCARD) => Some(Intent::RequestDiscard),
        (BN_CLICKED, ID_REFRESH) => Some(Intent::RefreshEndpoints),
        _ => None,
    }
}

fn selection(combo: HWND) -> Option<usize> {
    let index = unsafe { SendMessageW(combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)) };
    usize::try_from(index.0).ok()
}

struct WaitCursor(Option<HCURSOR>);

impl WaitCursor {
    fn show() -> Self {
        let Ok(wait) = (unsafe { LoadCursorW(None, IDC_WAIT) }) else {
            return Self(None);
        };
        Self(Some(unsafe { SetCursor(wait) }))
    }
}

impl Drop for WaitCursor {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            unsafe { SetCursor(previous) };
        }
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod preview_tests;
