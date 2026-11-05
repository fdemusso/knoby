//! Notification-area icon with a small status / quit menu.

use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::overlay::sd_round_rect;

pub const WM_TRAY: u32 = WM_APP + 2;
const TRAY_ID: u32 = 1;
const ID_QUIT: usize = 1;
const ID_AUTOSTART: usize = 2;
const ID_OPEN_CONFIG: usize = 3;
const ID_RELOAD_CONFIG: usize = 4;

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
    /// Round-trips a Run value under a throwaway name (never touches "Knoby").
    #[test]
    fn run_key_roundtrip() {
        let name = "KnobyTestEntry";
        assert!(super::set_run_value(name, Some("\"C:\\x y\\knoby.exe\"")));
        assert_eq!(super::run_value(name).as_deref(), Some("\"C:\\x y\\knoby.exe\""));
        assert!(super::set_run_value(name, None));
        assert_eq!(super::run_value(name), None);
    }

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

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Choice {
    ToggleAutostart,
    OpenConfig,
    ReloadConfig,
    Quit,
}

/// Opens a file in Notepad (always present, unlike a `.toml` association).
pub fn open_in_notepad(path: &std::path::Path) {
    let file = wide(&format!("\"{}\"", path.display()));
    unsafe {
        ShellExecuteW(None, w!("open"), w!("notepad.exe"), PCWSTR(file.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Shows the context menu at the cursor and returns the chosen item, if any.
/// Runs a nested message loop, so callers must not hold any borrow.
pub fn show_menu(hwnd: HWND, status: &str) -> Option<Choice> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let title = wide("Knoby");
        let status = wide(status);
        let autostart = wide("Start with Windows");
        let open_cfg = wide("Open config file");
        let reload_cfg = wide("Reload config");
        let quit = wide("Quit");
        let checked = if autostart_enabled() { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(title.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(status.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING | checked, ID_AUTOSTART, PCWSTR(autostart.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_CONFIG, PCWSTR(open_cfg.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD_CONFIG, PCWSTR(reload_cfg.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, PCWSTR(quit.as_ptr()));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            ID_QUIT => Some(Choice::Quit),
            ID_AUTOSTART => Some(Choice::ToggleAutostart),
            ID_OPEN_CONFIG => Some(Choice::OpenConfig),
            ID_RELOAD_CONFIG => Some(Choice::ReloadConfig),
            _ => None,
        }
    }
}

// --- start with Windows (HKCU Run key) --------------------------------------

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "Knoby";

/// The value we register: the quoted path of the running executable.
fn run_command() -> Option<String> {
    Some(format!("\"{}\"", std::env::current_exe().ok()?.display()))
}

fn run_value(name: &str) -> Option<String> {
    unsafe {
        let mut key = HKEY::default();
        let key_name = wide(RUN_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr()), None, KEY_QUERY_VALUE, &mut key).is_err() {
            return None;
        }
        let name = wide(name);
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let res = RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, Some(buf.as_mut_ptr() as *mut u8), Some(&mut len));
        let _ = RegCloseKey(key);
        if res.is_err() {
            return None;
        }
        let n = (len as usize / 2).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_owned())
    }
}

fn set_run_value(name: &str, command: Option<&str>) -> bool {
    unsafe {
        let mut key = HKEY::default();
        let key_name = wide(RUN_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr()), None, KEY_SET_VALUE, &mut key).is_err() {
            return false;
        }
        let name = wide(name);
        let ok = match command {
            Some(cmd) => {
                let data: Vec<u8> = wide(cmd).iter().flat_map(|c| c.to_le_bytes()).collect();
                RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&data)).is_ok()
            }
            None => RegDeleteValueW(key, PCWSTR(name.as_ptr())).is_ok(),
        };
        let _ = RegCloseKey(key);
        ok
    }
}

/// True when the Run entry exists and points at this very executable.
pub fn autostart_enabled() -> bool {
    run_value(RUN_VALUE).is_some_and(|v| Some(v) == run_command())
}

pub fn set_autostart(on: bool) {
    let cmd = run_command();
    set_run_value(RUN_VALUE, if on { cmd.as_deref() } else { None });
}
