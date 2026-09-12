//! Render the real Win32 controls into bitmaps with synthetic states. No audio
//! capture, foreground input, or changes to the user's running instance.
use super::*;
use crate::recorder::{EndpointLists, Level, Levels, Selector, Status, Tone, Transport, View};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::ApplicationInstallationAndServicing::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[test]
#[ignore = "writes native UI snapshots to target/ui-preview"]
fn render_native_states() {
    unsafe {
        let old_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let manifest = HSTRING::from(concat!(env!("OUT_DIR"), "/onerec.manifest"));
        let context = CreateActCtxW(&ACTCTXW {
            cbSize: std::mem::size_of::<ACTCTXW>() as u32,
            lpSource: PCWSTR(manifest.as_ptr()),
            ..Default::default()
        })
        .unwrap();
        let mut cookie = 0;
        ActivateActCtx(context, &mut cookie).unwrap();
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        register_class(instance).unwrap();
        let _apartment = super::super::Apartment::enter().unwrap();
        let root = create_window(instance).unwrap();
        let shell = RefCell::new(Shell {
            recorder: Recorder::new(crate::staging::StagingArea::open().unwrap()),
            controls: Controls::create(root, instance).unwrap(),
            icons: Icons::load(root, instance),
            timer_ms: None,
            phase: Phase::Idle,
            saved_path: None,
            modal: false,
            refresh_after_picker: false,
        });
        SetWindowLongPtrW(root, GWLP_USERDATA, &shell as *const _ as isize);
        super::install_command_button_subclasses(&shell.borrow().controls);
        SetWindowPos(
            root,
            None,
            -10000,
            -10000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .unwrap();
        let _ = ShowWindow(root, SW_SHOWNA);
        let directory = std::path::Path::new("target/ui-preview");
        std::fs::create_dir_all(directory).unwrap();
        for dpi in [96, 120, 144, 192] {
            shell.borrow_mut().controls.refresh_theme(root, dpi);
            for phase in [
                Phase::Idle,
                Phase::Recording,
                Phase::Paused,
                Phase::AwaitingSave,
                Phase::Saving,
                Phase::Failed,
            ] {
                let view = fixture(phase);
                shell.borrow_mut().controls.show(root, &view);
                settle(root);
                snapshot(root, &directory.join(format!("{phase:?}-{dpi}.bmp")));
            }
            for saved in [true, false] {
                let mut view = fixture(Phase::Idle);
                let name = if saved { "Saved" } else { "MissingDevice" };
                if saved {
                    view.saved_path = Some("C:/Meetings/team meeting.mp3".into());
                    view.status.text = "Saved: team meeting.mp3".into();
                } else {
                    view.microphone.selected = None;
                    view.endpoints.as_mut().unwrap().microphones.clear();
                    view.transport.toggle_enabled = false;
                    view.status.text = "Connect a microphone to record.".into();
                    view.status.tone = Tone::Warning;
                }
                shell.borrow_mut().controls.show(root, &view);
                settle(root);
                snapshot(root, &directory.join(format!("{name}-{dpi}.bmp")));
            }
        }
        SetWindowLongPtrW(root, GWLP_USERDATA, 0);
        DestroyWindow(root).unwrap();
        drop(shell);
        DeactivateActCtx(0, cookie).unwrap();
        ReleaseActCtx(context);
        SetThreadDpiAwarenessContext(old_dpi);
    }
}

unsafe fn settle(root: HWND) {
    let _ = RedrawWindow(
        root,
        None,
        None,
        RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
    );
    // Common Controls animates state transitions; sample their final rendering.
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(800) {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn fixture(phase: Phase) -> View {
    let recording = phase == Phase::Recording;
    let paused = phase == Phase::Paused;
    let pending = phase == Phase::AwaitingSave;
    let saving = phase == Phase::Saving;
    let enabled = phase == Phase::Idle || phase == Phase::Failed;
    View {
        phase,
        endpoints: Some(EndpointLists {
            microphones: vec!["Microphone (HyperX Cloud Flight Wireless)".into()],
            outputs: vec!["Speakers (HyperX Cloud Flight Wireless)".into()],
        }),
        microphone: Selector { selected: Some(0), enabled },
        output: Selector { selected: Some(0), enabled },
        quality: Selector { selected: Some(1), enabled },
        transport: Transport {
            toggle_label: if recording || paused { "Stop recording" } else { "Start recording" },
            toggle_enabled: recording || paused || enabled,
            save_enabled: pending, discard_enabled: pending,
        },
        elapsed: if phase == Phase::Idle { "00:00" } else { "1:23:45" }.into(),
        levels: Levels {
            microphone: if recording { Level { peak: 0.16, clipping: false } } else { Level::ZERO },
            system: if recording { Level { peak: 1.0, clipping: true } } else { Level::ZERO },
        },
        progress: saving.then_some(crate::mp3::SaveProgress { done: 42, total: 100 }),
        status: Status {
            text: match phase {
                Phase::Idle => "Ready. Ctrl+Shift+R starts recording.",
                Phase::Recording => "Recording.",
                Phase::Paused => "Recording paused.",
                Phase::AwaitingSave => "Save cancelled. The take is kept.",
                Phase::Saving => "Saving MP3…",
                Phase::Failed => "The output device disconnected. Connect it again and choose a device before starting a new recording.",
            }.into(),
            tone: if phase == Phase::Failed { Tone::Failure } else if recording { Tone::Recording } else { Tone::Neutral },
        },
        ask: None, saved_path: None,
    }
}

unsafe fn snapshot(root: HWND, path: &std::path::Path) {
    let mut rect = RECT::default();
    GetClientRect(root, &mut rect).unwrap();
    let screen = GetDC(root);
    let dc = CreateCompatibleDC(screen);
    let width = rect.right;
    let height = rect.bottom;
    let header = BITMAPINFOHEADER {
        biSize: 40,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let mut pixels = std::ptr::null_mut();
    let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut pixels, None, 0).unwrap();
    let old = SelectObject(dc, HGDIOBJ(bitmap.0));
    assert!(PrintWindow(root, dc, PRINT_WINDOW_FLAGS(3)).as_bool());
    let _ = GdiFlush();
    let bytes = std::slice::from_raw_parts(pixels as *const u8, (width * height * 4) as usize);
    let mut file = Vec::with_capacity(bytes.len() + 54);
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&((bytes.len() + 54) as u32).to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&54u32.to_le_bytes());
    file.extend_from_slice(std::slice::from_raw_parts(
        &header as *const _ as *const u8,
        40,
    ));
    file.extend_from_slice(bytes);
    std::fs::write(path, file).unwrap();
    SelectObject(dc, old);
    let _ = DeleteObject(HGDIOBJ(bitmap.0));
    let _ = DeleteDC(dc);
    ReleaseDC(root, screen);
}
