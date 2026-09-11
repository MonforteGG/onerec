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
    pub disabled: HBRUSH,
    pub divider: HBRUSH,
    pub ink: COLORREF,
    pub muted: COLORREF,
    pub red: COLORREF,
    pub warning: COLORREF,
    pub high_contrast: bool,
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
            primary: brush(red),
            hover: brush(rgb(0xa2, 0x20, 0x2b)),
            pressed: brush(rgb(0x88, 0x1b, 0x24)),
            disabled: brush(sys(COLOR_BTNFACE)),
            divider: brush(if hc { ink } else { rgb(0xe6, 0xea, 0xee) }),
            ink,
            muted: if hc { ink } else { rgb(0x59, 0x63, 0x6e) },
            red,
            warning: if hc { ink } else { rgb(0x85, 0x56, 0x00) },
            high_contrast: hc,
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
            self.disabled,
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
