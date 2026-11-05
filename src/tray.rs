//! Notification-area icon with a small status / quit menu.

use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use crate::overlay::sd_round_rect;

pub const WM_TRAY: u32 = WM_APP + 2;
const TRAY_ID: u32 = 1;
const ID_QUIT: usize = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A small knob glyph: rounded violet square, white dial, dark indicator notch.
fn make_icon() -> HICON {
    static ICON: OnceLock<isize> = OnceLock::new();
    let raw = *ICON.get_or_init(|| unsafe {
        let s = GetSystemMetrics(SM_CXSMICON).max(16);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: s,
                biHeight: -s,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return 0;
        };
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (s * s * 4) as usize);
        px.copy_from_slice(&icon_pixels(s));
        let mask = CreateBitmap(s, s, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).map_or(0, |i| i.0 as isize);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    });
    HICON(raw as *mut _)
}

/// Premultiplied BGRA pixels of the s×s tray icon.
fn icon_pixels(s: i32) -> Vec<u8> {
    let mut px = vec![0u8; (s * s * 4) as usize];
    let sf = s as f32;
    let (cx, cy) = (sf / 2.0, sf / 2.0);
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let body = (0.5 - sd_round_rect(fx, fy, sf / 2.0, sf / 2.0, sf * 0.26)).clamp(0.0, 1.0);
            let dial = (0.5 - ((fx * fx + fy * fy).sqrt() - sf * 0.31)).clamp(0.0, 1.0);
            // Indicator: vertical capsule from just above the centre to the dial's rim.
            let t = ((fy + sf * 0.05) / (sf * 0.19)).clamp(0.0, 1.0);
            let seg = ((fx * fx + (fy + sf * 0.05 - t * sf * 0.19).powi(2)).sqrt() - sf * 0.05).max(-1.0);
            let notch = (0.5 - seg).clamp(0.0, 1.0);

            // violet body -> white dial -> dark notch, all premultiplied.
            let mut c = [109.0, 124.0, 255.0]; // r, g, b of #6D7CFF
            c = mix(c, [245.0, 246.0, 250.0], dial);
            c = mix(c, [43.0, 47.0, 85.0], notch * dial);
            let i = ((y * s + x) * 4) as usize;
            px[i] = (c[2] * body) as u8;
            px[i + 1] = (c[1] * body) as u8;
            px[i + 2] = (c[0] * body) as u8;
            px[i + 3] = (255.0 * body) as u8;
        }
    }
    px
}

#[cfg(test)]
mod tests {
    /// Dev aid: KNOBY_ICON_DUMP=<file.png> writes the icon, nearest-neighbour
    /// enlarged 8x, over a mid-grey background so the shape can be eyeballed.
    #[test]
    fn dump_icon() {
        let Some(path) = std::env::var_os("KNOBY_ICON_DUMP") else { return };
        let (s, k) = (32usize, 8usize);
        let px = super::icon_pixels(s as i32);
        let mut out = Vec::with_capacity(s * k * s * k * 4);
        for y in 0..s * k {
            for x in 0..s * k {
                let i = ((y / k) * s + x / k) * 4;
                let a = px[i + 3] as f32 / 255.0;
                let bg = 90.0;
                // px is premultiplied: out = src + bg * (1 - a)
                out.extend([px[i + 2], px[i + 1], px[i]].map(|c| (c as f32 + bg * (1.0 - a)) as u8));
                out.push(255);
            }
        }
        let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), (s * k) as u32, (s * k) as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&out).unwrap();
    }
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
    }
}

pub fn add(hwnd: HWND) {
    let mut d = nid(hwnd);
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    d.uCallbackMessage = WM_TRAY;
    d.hIcon = make_icon();
    for (dst, src) in d.szTip.iter_mut().zip(wide("Knoby")) {
        *dst = src;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &d);
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid(hwnd));
    }
}

/// Shows the context menu at the cursor. Returns true if "Quit" was chosen.
/// Runs a nested message loop, so callers must not hold any borrow.
pub fn show_menu(hwnd: HWND, status: &str) -> bool {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return false };
        let title = wide("Knoby");
        let status = wide(status);
        let quit = wide("Quit");
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(title.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(status.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, PCWSTR(quit.as_ptr()));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(menu);
        cmd.0 as usize == ID_QUIT
    }
}
