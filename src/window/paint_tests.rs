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
            phase: Phase::Idle,
            saved_path: None,
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
                selected: Some(ExportQuality::Standard.index()),
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
    fn pending_take_exposes_enabled_save_and_hides_start() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        let mut view = idle_view(Some(0));
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.toggle).as_bool() });
        assert!(!unsafe { IsWindowVisible(controls.save).as_bool() });
        unsafe { SetFocus(controls.toggle).unwrap(); }
        view.phase = Phase::AwaitingSave;
        view.microphone.enabled = false;
        view.output.enabled = false;
        view.quality.enabled = false;
        view.transport.toggle_enabled = false;
        view.transport.save_enabled = true;
        view.transport.discard_enabled = true;
        controls.show(ui.root, &view);
        assert!(!unsafe { IsWindowVisible(controls.toggle).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.save).as_bool() });
        assert!(unsafe { IsWindowEnabled(controls.save).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.discard).as_bool() });
        assert!(!unsafe { IsWindowEnabled(controls.quality).as_bool() });
        assert_eq!(unsafe { GetFocus() }, controls.save);
        view.phase = Phase::Saving;
        view.transport.save_enabled = false;
        view.transport.discard_enabled = false;
        controls.show(ui.root, &view);
        assert!(!unsafe { IsWindowEnabled(controls.save).as_bool() });
        assert!(!unsafe { IsWindowVisible(controls.discard).as_bool() });
        assert!(!unsafe { IsWindowVisible(controls.quality).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.progress).as_bool() });
    }

    #[test]
    fn awaiting_save_closes_and_disables_quality_if_the_list_was_open() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        let mut view = idle_view(Some(0));
        controls.show(ui.root, &view);
        unsafe {
            SendMessageW(controls.quality, CB_SHOWDROPDOWN, WPARAM(1), LPARAM(0));
        }
        controls.set_list_dropped(true);
        view.phase = Phase::AwaitingSave;
        view.microphone.enabled = false;
        view.output.enabled = false;
        view.quality.enabled = false;
        view.transport.toggle_enabled = false;
        view.transport.save_enabled = true;
        view.transport.discard_enabled = true;
        controls.show(ui.root, &view);
        assert!(!dropped(controls.quality));
        assert!(!unsafe { IsWindowEnabled(controls.quality).as_bool() });
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

