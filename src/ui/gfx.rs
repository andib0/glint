//! Minimal anti-aliased drawing on top of GDI+ (rounded rects, circles).

use std::ptr::null_mut;
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::Graphics::GdiPlus::*;

pub fn startup() {
    let input = GdiplusStartupInput {
        GdiplusVersion: 1,
        DebugEventCallback: 0,
        SuppressBackgroundThread: 0,
        SuppressExternalCodecs: 0,
    };
    let mut token = 0usize;
    unsafe { GdiplusStartup(&mut token, &input, null_mut()) };
}

/// `0xRRGGBB` + alpha -> GDI+ ARGB.
fn argb(rgb: u32, a: u8) -> u32 {
    (u32::from(a) << 24) | (rgb & 0x00FF_FFFF)
}

pub struct Canvas(*mut GpGraphics);

impl Canvas {
    pub fn new(hdc: HDC) -> Option<Canvas> {
        let mut g = null_mut();
        unsafe {
            if GdipCreateFromHDC(hdc, &mut g) != 0 || g.is_null() {
                return None;
            }
            GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
        }
        Some(Canvas(g))
    }

    fn round_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> *mut GpPath {
        let mut p = null_mut();
        let d = (r * 2.0).min(w).min(h).max(0.1);
        unsafe {
            GdipCreatePath(FillModeAlternate, &mut p);
            GdipAddPathArc(p, x, y, d, d, 180.0, 90.0);
            GdipAddPathArc(p, x + w - d, y, d, d, 270.0, 90.0);
            GdipAddPathArc(p, x + w - d, y + h - d, d, d, 0.0, 90.0);
            GdipAddPathArc(p, x, y + h - d, d, d, 90.0, 90.0);
            GdipClosePathFigure(p);
        }
        p
    }

    pub fn fill_round(&self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32) {
        unsafe {
            let p = Self::round_path(x, y, w, h, r);
            let mut b = null_mut();
            GdipCreateSolidFill(argb(rgb, 255), &mut b);
            GdipFillPath(self.0, b as *mut GpBrush, p);
            GdipDeleteBrush(b as *mut GpBrush);
            GdipDeletePath(p);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stroke_round(&self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32, width: f32) {
        unsafe {
            let half = width / 2.0;
            let p = Self::round_path(x + half, y + half, w - width, h - width, r);
            let mut pen = null_mut();
            GdipCreatePen1(argb(rgb, 255), width, UnitPixel, &mut pen);
            GdipDrawPath(self.0, pen, p);
            GdipDeletePen(pen);
            GdipDeletePath(p);
        }
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, r: f32, rgb: u32) {
        unsafe {
            let mut b = null_mut();
            GdipCreateSolidFill(argb(rgb, 255), &mut b);
            GdipFillEllipse(self.0, b as *mut GpBrush, cx - r, cy - r, r * 2.0, r * 2.0);
            GdipDeleteBrush(b as *mut GpBrush);
        }
    }

    pub fn stroke_circle(&self, cx: f32, cy: f32, r: f32, rgb: u32, width: f32) {
        unsafe {
            let mut pen = null_mut();
            GdipCreatePen1(argb(rgb, 255), width, UnitPixel, &mut pen);
            GdipDrawEllipse(self.0, pen, cx - r, cy - r, r * 2.0, r * 2.0);
            GdipDeletePen(pen);
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe { GdipDeleteGraphics(self.0) };
    }
}
