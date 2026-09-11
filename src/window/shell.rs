use std::cell::RefCell;
use std::ffi::c_void;

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use ::windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetSysColorBrush, UpdateWindow, COLOR_BTNFACE, HDC, PAINTSTRUCT,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, GetWindowLongPtrW, IsDialogMessageW, KillTimer, LoadCursorW, MessageBoxW,
    PostQuitMessage, RegisterClassExW, SendMessageW, SetCursor, SetTimer, SetWindowLongPtrW,
    ShowWindow, TranslateMessage, BN_CLICKED, CBN_DROPDOWN, CBN_SELCHANGE, CB_GETCURSEL,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HCURSOR, IDC_ARROW, IDC_WAIT, IDYES,
    MB_ICONWARNING, MB_YESNO, MSG, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_DEVICECHANGE, WM_HOTKEY, WM_PAINT, WM_TIMER, WNDCLASSEXW,
    WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
};

use super::paint::{
    Controls, CLIENT_HEIGHT, CLIENT_WIDTH, ID_DISCARD, ID_MICROPHONE, ID_OUTPUT, ID_SAVE, ID_TOGGLE,
};
use super::save_dialog;
use crate::recorder::{Ask, Intent, Recorder};
use crate::RunError;

const CLASS_NAME: PCWSTR = w!("onerec.window");
const TITLE: PCWSTR = w!("onerec");
const TOGGLE_HOTKEY: i32 = 1;
const TICK_TIMER: usize = 1;
const TICK_MS: u32 = 50;
const STYLE: WINDOW_STYLE =
    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

pub(crate) fn run(recorder: Recorder) -> Result<(), RunError> {
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
        .map_err(|error| RunError::new(format!("reading the module handle failed: {error}")))?
        .into();
    register_class(instance)?;
    let root = create_window(instance)?;
    let controls = Controls::create(root, instance)?;
    if unsafe { SetTimer(root, TICK_TIMER, TICK_MS, None) } == 0 {
        return Err(RunError::new("the recorder could not start its clock"));
    }

    // The shell outlives the window: the loop only ends after WM_DESTROY posts the quit
    // message, so dropping it here cannot dangle a pointer the window procedure still reads.
    let shell = RefCell::new(Shell { recorder, controls });
    unsafe { SetWindowLongPtrW(root, GWLP_USERDATA, &shell as *const _ as isize) };
    let hotkey = unsafe {
        RegisterHotKey(
            root,
            TOGGLE_HOTKEY,
            MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
            b'R' as u32,
        )
    };
    shell.borrow_mut().dispatch(
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
}

impl Shell {
    /// The single funnel. Every message that means something becomes one intent here.
    fn dispatch(&mut self, root: HWND, intent: Intent) {
        // Stopping a take joins the capture threads and saving streams the whole staging file.
        let blocking = matches!(
            intent,
            Intent::Toggle | Intent::SaveTo(_) | Intent::DiscardAndClose
        );
        let _wait = blocking.then(WaitCursor::show);
        let view = self.recorder.apply(intent);
        self.controls.show(root, &view);
        match view.ask {
            None => {}
            Some(Ask::Close) => unsafe {
                let _ = DestroyWindow(root);
            },
            Some(Ask::ConfirmClose) => {
                if confirmed_close(root) {
                    self.dispatch(root, Intent::DiscardAndClose);
                }
            }
            Some(Ask::SaveDestination(prompt)) => {
                let next = match save_dialog::ask_destination(root, &prompt) {
                    Some(path) => Intent::SaveTo(path),
                    None => Intent::CancelSave,
                };
                self.dispatch(root, next);
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
            MB_YESNO | MB_ICONWARNING,
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
        hbrBackground: unsafe { GetSysColorBrush(COLOR_BTNFACE) },
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

fn create_window(instance: HINSTANCE) -> Result<HWND, RunError> {
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: CLIENT_WIDTH,
        bottom: CLIENT_HEIGHT,
    };
    unsafe { AdjustWindowRectEx(&mut frame, STYLE, false, WINDOW_EX_STYLE(0)) }
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

/// Win32 re-enters the window procedure from a modal dialog's own message pump. A failed
/// borrow is exactly that re-entry, and the message is dropped rather than reordered.
fn with_shell<T>(root: HWND, body: impl FnOnce(&mut Shell) -> T) -> Option<T> {
    let pointer = unsafe { GetWindowLongPtrW(root, GWLP_USERDATA) } as *const RefCell<Shell>;
    let cell = unsafe { pointer.as_ref() }?;
    let mut shell = cell.try_borrow_mut().ok()?;
    Some(body(&mut shell))
}

unsafe extern "system" fn wnd_proc(
    root: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TIMER | WM_HOTKEY | WM_COMMAND | WM_DEVICECHANGE | WM_CLOSE => {
            if let Some(intent) = translate(message, wparam, lparam) {
                with_shell(root, |shell| shell.dispatch(root, intent));
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(root, &mut paint);
            // A repaint during a modal dialog cannot borrow the shell, and the meters read
            // zero for as long as one is up, so nothing visible is lost.
            with_shell(root, |shell| shell.controls.draw_meters(hdc));
            let _ = EndPaint(root, &paint);
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut c_void);
            let control = HWND(lparam.0 as *mut c_void);
            let brush = with_shell(root, |shell| shell.controls.color_static(control, hdc))
                .unwrap_or_else(|| GetSysColorBrush(COLOR_BTNFACE));
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

fn translate(message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<Intent> {
    match message {
        WM_TIMER if wparam.0 == TICK_TIMER => Some(Intent::Tick),
        WM_HOTKEY if wparam.0 as i32 == TOGGLE_HOTKEY => Some(Intent::Toggle),
        WM_DEVICECHANGE => Some(Intent::RefreshEndpoints),
        WM_CLOSE => Some(Intent::Closing),
        WM_COMMAND => command(wparam, lparam),
        _ => None,
    }
}

fn command(wparam: WPARAM, lparam: LPARAM) -> Option<Intent> {
    let id = (wparam.0 & 0xffff) as u16;
    let notification = ((wparam.0 >> 16) & 0xffff) as u32;
    let control = HWND(lparam.0 as *mut c_void);
    match (notification, id) {
        (CBN_SELCHANGE, ID_MICROPHONE) => Some(Intent::ChooseMicrophone(selection(control)?)),
        (CBN_SELCHANGE, ID_OUTPUT) => Some(Intent::ChooseOutput(selection(control)?)),
        // The list is worth refreshing exactly when the user is about to look at it.
        (CBN_DROPDOWN, ID_MICROPHONE | ID_OUTPUT) => Some(Intent::RefreshEndpoints),
        (BN_CLICKED, ID_TOGGLE) => Some(Intent::Toggle),
        (BN_CLICKED, ID_SAVE) => Some(Intent::Save),
        (BN_CLICKED, ID_DISCARD) => Some(Intent::Discard),
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
