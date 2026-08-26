//! Runtime-drawn HICON (battery glyph) — no embedded asset needed.

use std::mem::{size_of, zeroed};

use windows::Win32::Foundation::{BOOL, HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, GetDC, HGDIOBJ, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

const S: usize = 32;

/// Build a tray icon for a battery level. `None` → greyed / unknown.
pub fn make_icon(battery: Option<u8>) -> windows::core::Result<HICON> {
    unsafe {
        let dc = GetDC(HWND::default());

        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = S as i32;
        bmi.bmiHeader.biHeight = -(S as i32); // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let hbmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0)?;

        let px = bits.cast::<u32>();
        let (fill_color, fill_h, show_level) = match battery {
            Some(v) => {
                let color = if v <= 20 {
                    0xE5484D
                } else if v <= 50 {
                    0xE8930C
                } else {
                    0x2FA05C
                };
                let h = 4 + (13 * v as usize) / 100;
                (color, h.min(17), true)
            }
            None => (0x808088, 0, false),
        };

        let (bx0, bx1, by0, by1, rx) = (5i32, 26i32, 9i32, 24i32, 3i32);
        let (tx0, ty0, tx1, ty1) = (27i32, 12i32, 30i32, 21i32);

        for y in 0..S as i32 {
            for x in 0..S as i32 {
                let in_body = in_round_rect(x, y, bx0, by0, bx1, by1, rx);
                let in_term = x >= tx0 && x <= tx1 && y >= ty0 && y <= ty1;
                if !in_body && !in_term {
                    *px.add((y * S as i32 + x) as usize) = 0x0000_0000;
                    continue;
                }
                let on_boundary = !in_round_rect(
                    x,
                    y,
                    bx0 + 2,
                    by0 + 2,
                    bx1 - 2,
                    by1 - 2,
                    rx.saturating_sub(1),
                ) && !in_round_rect(x, y, tx0 + 1, ty0 + 1, tx1 - 1, ty1 - 1, 1);
                let rgb = if on_boundary {
                    0x45454A
                } else if in_body {
                    if show_level && y >= by1 - 2 - fill_h as i32 {
                        fill_color
                    } else {
                        0x2A2A2E
                    }
                } else {
                    0x45454A
                };
                *px.add((y * S as i32 + x) as usize) = (0xFFu32 << 24) | rgb;
            }
        }

        let hbm_mask = CreateBitmap(S as i32, S as i32, 1, 1, None);

        let info = ICONINFO {
            fIcon: BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbmp,
        };
        let icon = CreateIconIndirect(&info)?;

        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        let _ = DeleteObject(HGDIOBJ(hbm_mask.0));
        let _ = ReleaseDC(HWND::default(), dc);
        Ok(icon)
    }
}

fn in_round_rect(x: i32, y: i32, x0: i32, y0: i32, x1: i32, y1: i32, r: i32) -> bool {
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let w = x1 - x0;
    let h = y1 - y0;
    if w <= 0 || h <= 0 {
        return false;
    }
    let r = r.min(w.min(h) / 2).max(0);
    if r == 0 {
        return true;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    let dx = (x - cx) as i64;
    let dy = (y - cy) as i64;
    dx * dx + dy * dy <= (r * r) as i64
}
