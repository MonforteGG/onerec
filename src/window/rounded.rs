//! Antialiased button backgrounds. Text and icons keep their native GDI renderer.
use std::ptr::null_mut;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{GetObjectW, HBRUSH, HDC, HGDIOBJ, LOGBRUSH};
use windows::Win32::Graphics::GdiPlus::*;

pub(super) struct RoundedButtons {
    token: Option<usize>,
}

impl RoundedButtons {
    pub fn new() -> Self {
        let mut token = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let status = unsafe { GdiplusStartup(&mut token, &input, null_mut()) };
        Self {
            token: (status == Ok).then_some(token),
        }
    }

    // Return false on failure so the caller can still draw a usable GDI button.
    pub fn fill(&self, dc: HDC, rect: RECT, diameter: i32, brush: HBRUSH) -> bool {
        if self.token.is_none() || rect.right <= rect.left || rect.bottom <= rect.top {
            return false;
        }
        unsafe {
            let mut color = LOGBRUSH::default();
            if GetObjectW(
                HGDIOBJ(brush.0),
                std::mem::size_of::<LOGBRUSH>() as i32,
                Some((&mut color as *mut LOGBRUSH).cast()),
            ) == 0
            {
                return false;
            }
            let rgb = color.lbColor.0;
            let argb = 0xff00_0000 | ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff);
            let mut drawing = Drawing::default();
            if GdipCreateFromHDC(dc, &mut drawing.graphics) != Ok
                || GdipCreatePath(FillModeWinding, &mut drawing.path) != Ok
                || GdipCreateSolidFill(argb, &mut drawing.brush) != Ok
                || GdipSetSmoothingMode(drawing.graphics, SmoothingModeAntiAlias8x8) != Ok
                || GdipSetPixelOffsetMode(drawing.graphics, PixelOffsetModeHalf) != Ok
            {
                return false;
            }
            let x = rect.left as f32;
            let y = rect.top as f32;
            let w = (rect.right - rect.left) as f32;
            let h = (rect.bottom - rect.top) as f32;
            let d = (diameter.max(1) as f32).min(w).min(h);
            for (left, top, angle) in [
                (x, y, 180.0),
                (x + w - d, y, 270.0),
                (x + w - d, y + h - d, 0.0),
                (x, y + h - d, 90.0),
            ] {
                if GdipAddPathArc(drawing.path, left, top, d, d, angle, 90.0) != Ok {
                    return false;
                }
            }
            GdipClosePathFigure(drawing.path) == Ok
                && GdipFillPath(drawing.graphics, drawing.brush.cast(), drawing.path) == Ok
            // Drawing drops here, before the caller draws text on this HDC.
        }
    }
}

impl Drop for RoundedButtons {
    fn drop(&mut self) {
        if let Some(token) = self.token {
            unsafe {
                GdiplusShutdown(token);
            }
        }
    }
}

#[derive(Default)]
struct Drawing {
    graphics: *mut GpGraphics,
    path: *mut GpPath,
    brush: *mut GpSolidFill,
}

impl Drop for Drawing {
    fn drop(&mut self) {
        unsafe {
            if !self.brush.is_null() {
                GdipDeleteBrush(self.brush.cast());
            }
            if !self.path.is_null() {
                GdipDeletePath(self.path);
            }
            if !self.graphics.is_null() {
                GdipDeleteGraphics(self.graphics);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::*;

    #[test]
    fn corners_blend_without_changing_the_solid_fill_at_each_dpi() {
        let renderer = RoundedButtons::new();
        assert!(renderer.token.is_some(), "GDI+ did not start");
        for dpi in [96, 120, 144, 192] {
            let scale = |value| super::super::paint::scale(value, dpi);
            let width = scale(96);
            let height = scale(40);
            unsafe {
                let dc = CreateCompatibleDC(None);
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = null_mut();
                let bitmap =
                    CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
                let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
                std::ptr::write_bytes(bits.cast::<u8>(), 255, (width * height * 4) as usize);
                let color = super::super::theme::rgb(184, 42, 53);
                let brush = CreateSolidBrush(color);
                let painted = renderer.fill(
                    dc,
                    RECT {
                        right: width,
                        bottom: height,
                        ..Default::default()
                    },
                    scale(12),
                    brush,
                );
                let _ = GdiFlush();
                let pixels =
                    std::slice::from_raw_parts(bits.cast::<u32>(), (width * height) as usize);
                let pixel = |x: i32, y: i32| pixels[(y * width + x) as usize] & 0x00ff_ffff;
                let corner = pixel(0, 0);
                let center = pixel(width / 2, height / 2);
                let top = pixel(width / 2, 0);
                let mut blended = 0;
                for y in 0..scale(6) {
                    for x in 0..scale(6) {
                        let p = pixel(x, y);
                        if p != 0xffffff && p != 0xb82a35 {
                            blended += 1;
                        }
                    }
                }
                SelectObject(dc, previous);
                let _ = DeleteObject(HGDIOBJ(brush.0));
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                assert!(painted, "smooth fill failed at {dpi} DPI");
                assert_eq!(corner, 0xffffff);
                assert_eq!(center, 0xb82a35);
                assert_eq!(top, 0xb82a35, "straight edge is blurred at {dpi} DPI");
                assert!(blended > 0, "corner has no antialiasing at {dpi} DPI");
            }
        }
    }
}
