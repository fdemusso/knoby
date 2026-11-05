//! Notification-area icon with a small status / quit menu.

use windows::Win32::Foundation::{HINSTANCE, HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

pub const WM_TRAY: u32 = WM_APP + 2;
const TRAY_ID: u32 = 1;
const ID_QUIT: usize = 1;
const ID_AUTOSTART: usize = 2;
const ID_OPEN_CONFIG: usize = 3;
const ID_RELOAD_CONFIG: usize = 4;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The app icon, loaded at the tray's size from the resource `build.rs` embeds (icon resource id 1).
fn make_icon() -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).max(16);
        let hinst = GetModuleHandleW(PCWSTR::null()).ok().map(|h| HINSTANCE(h.0));
        // MAKEINTRESOURCE(1): a resource *id* passed in the pointer's address.
        let resource_id = PCWSTR(std::ptr::without_provenance(1));
        LoadImageW(hinst, resource_id, IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    /// Round-trips a Run value under a throwaway name (never touches "Knoby").
    #[test]
    fn run_key_roundtrip() {
        let name = "KnobyTestEntry";
        let value = r#""C:\x y\knoby.exe""#;
        assert!(super::set_run_value(name, Some(value)));
        assert_eq!(super::run_value(name).as_deref(), Some(value));
        assert!(super::set_run_value(name, None));
        assert_eq!(super::run_value(name), None);
    }
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
        // Create-or-open: the Run key can be missing on a fresh user profile.
        let opened = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if opened.is_err() {
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
