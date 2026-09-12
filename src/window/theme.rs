use std::ffi::c_void;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

pub(super) struct Theme {
    pub background: HBRUSH,
    pub track: HBRUSH,
    pub signal: HBRUSH,
    pub clipping: HBRUSH,
    pub primary: HBRUSH,
    pub hover: HBRUSH,
    pub pressed: HBRUSH,
    pub save: HBRUSH,
    pub save_hover: HBRUSH,
    pub save_pressed: HBRUSH,
    pub disabled: HBRUSH,
    pub disabled_ink: COLORREF,
    pub secondary: HBRUSH,
    pub secondary_hover: HBRUSH,
    pub secondary_pressed: HBRUSH,
    pub danger_hover: HBRUSH,
    pub primary_ink: COLORREF,
    pub divider: HBRUSH,
    pub ink: COLORREF,
    pub muted: COLORREF,
    pub red: COLORREF,
    pub warning: COLORREF,
}

impl Theme {
    pub fn new() -> Self {
        let mut contrast = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                Some(&mut contrast as *mut _ as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
        let hc = contrast.dwFlags.contains(HCF_HIGHCONTRASTON);
        let sys = |index| COLORREF(unsafe { GetSysColor(index) });
        let brush = |color| unsafe { CreateSolidBrush(color) };
        let ink = sys(COLOR_WINDOWTEXT);
        let red = if hc { ink } else { rgb(0xb8, 0x2a, 0x35) };
        Self {
            background: brush(sys(COLOR_WINDOW)),
            track: brush(if hc {
                sys(COLOR_BTNFACE)
            } else {
                rgb(0xe6, 0xea, 0xee)
            }),
            signal: brush(if hc {
                sys(COLOR_HIGHLIGHT)
            } else {
                rgb(0x23, 0x83, 0x49)
            }),
            clipping: brush(red),
            primary: brush(if hc { sys(COLOR_HIGHLIGHT) } else { red }),
            hover: brush(if hc {
                sys(COLOR_HIGHLIGHT)
            } else {
                rgb(0xa2, 0x20, 0x2b)
            }),
            pressed: brush(if hc {
                sys(COLOR_HIGHLIGHT)
            } else {
                rgb(0x88, 0x1b, 0x24)
            }),
            save: brush(if hc {
                sys(COLOR_HIGHLIGHT)
            } else {
                rgb(0x2d, 0x37, 0x45)
            }),
            save_hover: brush(if hc {
                sys(COLOR_HIGHLIGHT)
            } else {
                rgb(0x21, 0x2a, 0x36)
            }),
            save_pressed: brush(if hc {
                sys(COLOR_HIGHLIGHT)
            } else {
                rgb(0x17, 0x1e, 0x28)
            }),
            disabled: brush(if hc {
                sys(COLOR_BTNFACE)
            } else {
                rgb(0xf5, 0xf6, 0xf8)
            }),
            disabled_ink: if hc {
                sys(COLOR_GRAYTEXT)
            } else {
                rgb(0xab, 0xb2, 0xbd)
            },
            secondary: brush(if hc {
                sys(COLOR_BTNFACE)
            } else {
                rgb(0xe7, 0xeb, 0xf0)
            }),
            secondary_hover: brush(if hc {
                sys(COLOR_BTNFACE)
            } else {
                rgb(0xda, 0xe0, 0xe8)
            }),
            secondary_pressed: brush(if hc {
                sys(COLOR_BTNFACE)
            } else {
                rgb(0xcb, 0xd3, 0xde)
            }),
            danger_hover: brush(if hc {
                sys(COLOR_BTNFACE)
            } else {
                rgb(0xfc, 0xeb, 0xed)
            }),
            primary_ink: if hc {
                sys(COLOR_HIGHLIGHTTEXT)
            } else {
                rgb(255, 255, 255)
            },
            divider: brush(if hc { ink } else { rgb(0xe6, 0xea, 0xee) }),
            ink,
            muted: if hc { ink } else { rgb(0x59, 0x63, 0x6e) },
            red,
            warning: if hc { ink } else { rgb(0x85, 0x56, 0x00) },
        }
    }
}

impl Drop for Theme {
    fn drop(&mut self) {
        for brush in [
            self.background,
            self.track,
            self.signal,
            self.clipping,
            self.primary,
            self.hover,
            self.pressed,
            self.save,
            self.save_hover,
            self.save_pressed,
            self.disabled,
            self.secondary,
            self.secondary_hover,
            self.secondary_pressed,
            self.danger_hover,
            self.divider,
        ] {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(brush.0));
            }
        }
    }
}

pub(super) const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | (green as u32) << 8 | (blue as u32) << 16)
}
