//! Compose a complete button before exposing any pixels to its window.
use windows::Win32::Graphics::Gdi::*;

pub(super) struct PaintBuffer {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    width: i32,
    height: i32,
}

impl PaintBuffer {
    pub fn new(target: HDC, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        unsafe {
            let dc = CreateCompatibleDC(target);
            if dc.is_invalid() {
                return None;
            }
            let bitmap = CreateCompatibleBitmap(target, width, height);
            if bitmap.is_invalid() {
                let _ = DeleteDC(dc);
                return None;
            }
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            if previous.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return None;
            }
            Some(Self {
                dc,
                bitmap,
                previous,
                width,
                height,
            })
        }
    }

    pub fn dc(&self) -> HDC {
        self.dc
    }

    pub fn present(&self, target: HDC) -> bool {
        unsafe {
            BitBlt(
                target,
                0,
                0,
                self.width,
                self.height,
                self.dc,
                0,
                0,
                SRCCOPY,
            )
            .is_ok()
        }
    }
}

impl Drop for PaintBuffer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;

    #[test]
    fn intermediate_frames_are_not_visible_until_presented() {
        unsafe {
            let reference = GetDC(None);
            let target = PaintBuffer::new(reference, 32, 32).unwrap();
            let frame = PaintBuffer::new(reference, 32, 32).unwrap();
            let rect = RECT {
                right: 32,
                bottom: 32,
                ..Default::default()
            };
            FillRect(target.dc(), &rect, HBRUSH(GetStockObject(WHITE_BRUSH).0));
            FillRect(frame.dc(), &rect, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            assert_eq!(GetPixel(target.dc(), 16, 16).0, 0xffffff);
            assert_eq!(GetPixel(frame.dc(), 16, 16).0, 0);
            assert!(frame.present(target.dc()));
            assert_eq!(GetPixel(target.dc(), 16, 16).0, 0);
            ReleaseDC(None, reference);
        }
    }
}
