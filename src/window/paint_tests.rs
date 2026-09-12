#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{Level, Levels, Status, Tone, Transport, View};
    use std::sync::Once;
    use windows::Win32::Foundation::{LRESULT, RECT};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::{GetComboBoxInfo, COMBOBOXINFO};
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, GetClientRect, RegisterClassExW, ShowWindow,
        CB_GETDROPPEDSTATE, CB_SHOWDROPDOWN, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
        LB_GETCURSEL, SW_SHOW, WNDCLASSEXW, WS_CAPTION, WS_OVERLAPPED, WS_SYSMENU,
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
            save_direct: false,
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
    fn pending_take_enables_save_and_disables_record() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        let mut view = idle_view(Some(0));
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.toggle).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.save).as_bool() });
        assert!(!unsafe { IsWindowEnabled(controls.save).as_bool() });
        unsafe { SetFocus(controls.toggle).unwrap(); }
        view.phase = Phase::AwaitingSave;
        view.microphone.enabled = false;
        view.output.enabled = false;
        view.quality.enabled = false;
        view.transport.toggle_enabled = false;
        view.transport.save_enabled = true;
        view.transport.discard_enabled = true;
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.toggle).as_bool() });
        assert!(!unsafe { IsWindowEnabled(controls.toggle).as_bool() });
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
        assert!(unsafe { IsWindowVisible(controls.discard).as_bool() });
        assert!(!unsafe { IsWindowEnabled(controls.discard).as_bool() });
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
    fn pause_is_always_visible_and_enabled_only_while_live() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        let mut view = idle_view(Some(0));
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.pause).as_bool() });
        assert!(!unsafe { IsWindowEnabled(controls.pause).as_bool() });
        view.phase = Phase::Recording;
        view.microphone.enabled = false;
        view.output.enabled = false;
        view.quality.enabled = false;
        view.transport.toggle_label = "Stop recording";
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.pause).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.stop).as_bool() });
        assert!(!unsafe { IsWindowEnabled(controls.quality).as_bool() });
        assert_eq!(caption(controls.pause), "&Pause");
        view.phase = Phase::Paused;
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.pause).as_bool() });
        assert_eq!(caption(controls.pause), "&Resume");
        assert!(!unsafe { IsWindowEnabled(controls.quality).as_bool() });
        view.phase = Phase::AwaitingSave;
        view.transport.toggle_enabled = false;
        view.transport.save_enabled = true;
        view.transport.discard_enabled = true;
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.pause).as_bool() });
        assert!(!unsafe { IsWindowEnabled(controls.pause).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.discard).as_bool() });
    }

    #[test]
    fn settings_remains_visible_in_full_and_compact_views() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        let mut view = idle_view(Some(0));
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.settings).as_bool() });
        assert!(!unsafe { IsWindowVisible(controls.save_as).as_bool() });
        view.phase = Phase::Recording;
        view.microphone.enabled = false;
        view.output.enabled = false;
        view.quality.enabled = false;
        view.transport.toggle_label = "Stop recording";
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.settings).as_bool() });
        assert!(unsafe { IsWindowEnabled(controls.settings).as_bool() });
        view.phase = Phase::AwaitingSave;
        view.save_direct = true;
        view.transport.toggle_enabled = false;
        view.transport.save_enabled = true;
        view.transport.discard_enabled = true;
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.settings).as_bool() });
        assert!(unsafe { IsWindowEnabled(controls.settings).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.save_as).as_bool() });
        assert_eq!(caption(controls.save), "&Save");
    }

    fn client_h(root: HWND) -> i32 {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(root, &mut rect);
        }
        rect.bottom
    }

    #[test]
    fn closed_device_combos_sit_above_their_vu_row() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        let mut view = idle_view(Some(0));
        view.saved_path = Some(r"C:\Meetings\2026-09-12 15-52 Meeting.mp3".into());
        view.status.text = "Saved: 2026-09-12 15-52 Meeting.mp3".into();
        controls.show(ui.root, &view);

        let mic = window_rect_in_parent(ui.root, controls.microphones);
        let mic_level = window_rect_in_parent(ui.root, controls.microphone_level);
        let mic_meter = controls.meter_rect(0);
        assert!(
            mic.bottom <= mic_level.top,
            "mic combo {mic:?} overlaps level {mic_level:?}"
        );
        assert!(
            mic.bottom <= mic_meter.top,
            "mic combo {mic:?} overlaps meter {mic_meter:?}"
        );

        let output = window_rect_in_parent(ui.root, controls.outputs);
        let output_level = window_rect_in_parent(ui.root, controls.output_level);
        let output_meter = controls.meter_rect(1);
        assert!(
            output.bottom <= output_level.top,
            "output combo {output:?} overlaps level {output_level:?}"
        );
        assert!(
            output.bottom <= output_meter.top,
            "output combo {output:?} overlaps meter {output_meter:?}"
        );
    }

    #[test]
    fn recording_hud_hides_device_combos() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        let mut view = idle_view(Some(0));
        controls.show(ui.root, &view);
        let idle_h = client_h(ui.root);
        assert!(unsafe { IsWindowVisible(controls.microphones).as_bool() });
        view.phase = Phase::Recording;
        view.microphone.enabled = false;
        view.output.enabled = false;
        view.quality.enabled = false;
        view.transport.toggle_label = "Stop recording";
        controls.show(ui.root, &view);
        assert!(!unsafe { IsWindowVisible(controls.microphones).as_bool() });
        assert!(!unsafe { IsWindowVisible(controls.outputs).as_bool() });
        assert!(!unsafe { IsWindowVisible(controls.quality).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.pause).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.stop).as_bool() });
        assert!(client_h(ui.root) < idle_h);
    }

    #[test]
    fn stop_restores_full_layout() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        let mut view = idle_view(Some(0));
        view.phase = Phase::Recording;
        view.microphone.enabled = false;
        view.output.enabled = false;
        view.quality.enabled = false;
        view.transport.toggle_label = "Stop recording";
        controls.show(ui.root, &view);
        let hud_h = client_h(ui.root);
        view.phase = Phase::AwaitingSave;
        view.transport.toggle_enabled = false;
        view.transport.save_enabled = true;
        view.transport.discard_enabled = true;
        controls.show(ui.root, &view);
        assert!(unsafe { IsWindowVisible(controls.microphones).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.save).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.discard).as_bool() });
        assert!(unsafe { IsWindowVisible(controls.pause).as_bool() });
        assert!(!unsafe { IsWindowEnabled(controls.pause).as_bool() });
        assert!(client_h(ui.root) > hud_h);
    }

    fn caption(hwnd: HWND) -> String {
        let mut buf = [0u16; 64];
        let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..n as usize])
    }

    #[test]
    fn idle_hint_is_empty_but_actionable_status_is_preserved() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        assert_eq!(caption(controls.status), "");
        let mut view = idle_view(Some(0));
        view.status.text = "Ready. Ctrl+Shift+R starts recording.".into();
        controls.show(ui.root, &view);
        assert_eq!(caption(controls.status), "");
        view.status.text = "Connect a microphone to record.".into();
        controls.show(ui.root, &view);
        assert_eq!(caption(controls.status), view.status.text);
    }

    #[test]
    fn transport_and_settings_keep_their_positions_and_valid_actions_in_every_state() {
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let ui = harness();
        let mut controls = Controls::create(ui.root, instance).unwrap();
        for dpi in [96, 120, 144, 192] {
            controls.refresh_theme(ui.root, dpi);
            let mut view = idle_view(Some(0));
            controls.show(ui.root, &view);
            let buttons = [controls.toggle, controls.pause, controls.stop, controls.save,
                controls.discard, controls.settings];
            let bounds = |button| {
                let r = window_rect_in_parent(ui.root, button);
                (r.left, r.top, r.right, r.bottom)
            };
            let positions = buttons.map(bounds);
            for phase in [Phase::Idle, Phase::Recording, Phase::Paused, Phase::AwaitingSave,
                Phase::Saving, Phase::Failed] {
                let live = matches!(phase, Phase::Recording | Phase::Paused);
                let ready = matches!(phase, Phase::Idle | Phase::Failed);
                let pending = phase == Phase::AwaitingSave;
                view.phase = phase;
                view.transport.toggle_enabled = ready || live;
                view.transport.save_enabled = pending;
                view.transport.discard_enabled = pending;
                controls.show(ui.root, &view);
                assert_eq!(buttons.map(bounds), positions, "geometry changed for {phase:?} at {dpi}");
                let elapsed = window_rect_in_parent(ui.root, controls.elapsed);
                let record = window_rect_in_parent(ui.root, controls.toggle);
                assert!(record.top - elapsed.bottom >= controls.s(28) - 1,
                    "timer needs breathing room above transport at {dpi} DPI");
                assert!(client_h(ui.root) - record.bottom >= controls.s(16) - 1);
                let enabled = [ready, live, live, pending, pending, true];
                for (button, expected) in buttons.into_iter().zip(enabled) {
                    assert!(unsafe { IsWindowVisible(button).as_bool() });
                    assert_eq!(unsafe { IsWindowEnabled(button).as_bool() }, expected,
                        "invalid enabled state for {} in {phase:?}", caption(button));
                }
                assert_eq!(caption(controls.toggle), "&Record");
                assert_eq!(caption(controls.stop), "S&top");
                assert_eq!(caption(controls.save), "&Save");
                for control in controls.all() {
                    assert!(!["&Microphone", "System &audio", "Ctrl+Shift+R"].contains(&caption(control).as_str()));
                }
            }
        }
    }

    #[test]
    fn embedded_lucide_font_resolves_every_button_icon_at_each_dpi() {
        for dpi in [96, 120, 144, 192] {
            let fonts = Fonts::new(dpi);
            assert!(!fonts.icon_resource.is_invalid(), "private icon font did not load");
            unsafe {
                let dc = CreateCompatibleDC(None);
                let previous = SelectObject(dc, HGDIOBJ(fonts.icons.0));
                let mut face = [0u16; 64];
                let length = GetTextFaceW(dc, Some(&mut face));
                let actual = String::from_utf16_lossy(&face[..length as usize]);
                assert_eq!(actual.trim_end_matches('\0'), "lucide", "font fallback at {dpi} DPI");
                let chars = [ButtonIcon::Record, ButtonIcon::Stop, ButtonIcon::Pause,
                    ButtonIcon::Resume, ButtonIcon::Discard, ButtonIcon::Settings, ButtonIcon::Save]
                    .map(ButtonIcon::glyph);
                let mut indices = [0u16; 7];
                let result = GetGlyphIndicesW(dc, PCWSTR(chars.as_ptr()), chars.len() as i32,
                    indices.as_mut_ptr(), GGI_MARK_NONEXISTING_GLYPHS);
                SelectObject(dc, previous);
                let _ = DeleteDC(dc);
                assert_ne!(result, GDI_ERROR as u32);
                assert!(indices.iter().all(|&index| index != 0 && index != u16::MAX),
                    "missing icon at {dpi} DPI: {indices:?}");
            }
        }
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

