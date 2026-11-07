//! Tray icon drawn at runtime (anti-aliased sun), so no resource compiler is needed.

use std::ptr::null_mut;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
}

/// Premultiplied BGRA pixels of a white sun, `n`x`n`.
pub fn sun_pixels(n: i32) -> Vec<u32> {
    let s = n as f32;
    let c = s / 2.0;
    let r = s * 0.22;
    let (r0, r1, half_w) = (s * 0.34, s * 0.47, (s * 0.06).max(0.75));
    let mut px = vec![0u32; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
            let mut a = (r + 0.5 - d).clamp(0.0, 1.0);
            for k in 0..8 {
                let ang = k as f32 * std::f32::consts::FRAC_PI_4;
                let (ca, sa) = (ang.cos(), ang.sin());
                let dist = seg_dist(fx, fy, c + ca * r0, c + sa * r0, c + ca * r1, c + sa * r1);
                a = a.max((half_w + 0.5 - dist).clamp(0.0, 1.0));
            }
            let v = (a * 255.0).round() as u32;
            px[(y * n + x) as usize] = v << 24 | v << 16 | v << 8 | v;
        }
    }
    px
}

pub fn make(n: i32) -> HICON {
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: n,
            biHeight: -n,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = null_mut();
        let color = CreateDIBSection(null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if color.is_null() || bits.is_null() {
            return null_mut();
        }
        let px = sun_pixels(n);
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        let mask_bytes = vec![0u8; (((n + 15) / 16 * 2) * n) as usize];
        let mask = CreateBitmap(n, n, 1, 1, mask_bytes.as_ptr() as *const _);
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sun_is_centered_and_transparent_at_corners() {
        let n = 32;
        let px = super::sun_pixels(n);
        assert_eq!(px[0], 0);
        assert_eq!(px[(n * n - 1) as usize], 0);
        assert_eq!(px[(16 * n + 16) as usize] >> 24, 255);
    }
}
