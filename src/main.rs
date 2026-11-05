// Release builds have no console window; debug builds keep it for logging.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod overlay;
mod tray;
mod wavelink;

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Instant;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use config::Config;
use overlay::Overlay;
use wavelink::{Channel, Handle};

const WM_WL: u32 = WM_APP + 1;
const T_CLICK: usize = 1;
const T_SEL: usize = 2;
const T_ANIM: usize = 3;
const FRAME_MS: u32 = 15;

const VK_VOLUME_MUTE: u32 = 0xAD;
const VK_VOLUME_DOWN: u32 = 0xAE;
const VK_VOLUME_UP: u32 = 0xAF;
const VK_F13: u32 = 0x7C;
const VK_F14: u32 = 0x7D;
const VK_F15: u32 = 0x7E;
const LLKHF_INJECTED: u32 = 0x10;

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Up,
    Down,
    Click,
}

struct App {
    hwnd: HWND,
    wl: Handle,
    overlay: Overlay,
    cfg: Config,
    active: Option<String>,
    /// Some(index) while the channel picker overlay is open.
    selecting: Option<usize>,
    click_pending: bool,
    preview_done: bool,
    /// Net detents accumulated towards the next selection move (sign = direction).
    sel_accum: i32,
    last_sel_move: Option<Instant>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|c| f(c.borrow_mut().as_mut().expect("app initialised")))
}

fn active_index(active: &Option<String>, channels: &[Channel]) -> usize {
    active
        .as_ref()
        .and_then(|id| channels.iter().position(|c| &c.id == id))
        .unwrap_or(0)
}

fn state_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("APPDATA")?).join("Knoby").join("active.txt"))
}

fn load_active() -> Option<String> {
    // Dev/testing override so a test run doesn't touch the saved channel.
    if let Ok(id) = std::env::var("KNOBY_ACTIVE") {
        return Some(id);
    }
    Some(std::fs::read_to_string(state_path()?).ok()?.trim().to_owned())
}

fn save_active(id: &str) {
    if let Some(p) = state_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, id);
    }
}

/// Loads the user config; problems are printed (visible in debug builds) and
/// the affected settings keep their defaults.
fn load_config() -> Config {
    let (cfg, warnings) = config::load();
    for w in warnings {
        eprintln!("config: {w}");
    }
    cfg
}

fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KNOBY_DEBUG").is_some())
}

fn accept_injected() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KNOBY_ACCEPT_INJECTED").is_some())
}

// --- input -----------------------------------------------------------------

fn ready() -> bool {
    APP.with(|c| match c.try_borrow() {
        Ok(a) => a.as_ref().is_some_and(|a| {
            let g = a.wl.shared.lock().unwrap();
            g.connected && !g.channels.is_empty()
        }),
        Err(_) => false,
    })
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        // KNOBY_ACCEPT_INJECTED lets scripted key presses drive Knoby (dev/testing).
        if kb.flags.0 & LLKHF_INJECTED == 0 || accept_injected() {
            let key = match kb.vkCode {
                VK_VOLUME_UP | VK_F15 => Some(Key::Up),
                VK_VOLUME_DOWN | VK_F14 => Some(Key::Down),
                VK_VOLUME_MUTE | VK_F13 => Some(Key::Click),
                _ => None,
            };
            if let Some(key) = key {
                let is_down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
                let ready = ready();
                if debug() {
                    eprintln!("key vk={:#x} down={is_down} ready={ready}", kb.vkCode);
                }
                if ready {
                    if is_down {
                        handle_key(key);
                    }
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn handle_key(key: Key) {
    let selecting = with_app(|a| a.selecting);
    if debug() {
        eprintln!("  handle_key selecting={selecting:?}");
    }
    match (selecting, key) {
        (None, Key::Click) => click_normal(),
        (None, k) => adjust(k == Key::Up),
        (Some(i), Key::Click) => confirm(i),
        (Some(i), k) => move_selection(i, k == Key::Up),
    }
}

fn adjust(up: bool) {
    with_app(|a| {
        let mut g = a.wl.shared.lock().unwrap();
        let idx = active_index(&a.active, &g.channels);
        let ch = &mut g.channels[idx];
        let d = if up { a.cfg.volume_step } else { -a.cfg.volume_step };
        ch.level = ((ch.level + d).clamp(0.0, 1.0) * 10000.0).round() / 10000.0;
        ch.local_set = Some(Instant::now());
        let (id, level) = (ch.id.clone(), ch.level);
        drop(g);
        if debug() {
            eprintln!("  adjust step={d:+.4} -> level={level}");
        }
        a.wl.set_level(&id, level);
    });
}

fn toggle_mute() {
    with_app(|a| {
        let mut g = a.wl.shared.lock().unwrap();
        let idx = active_index(&a.active, &g.channels);
        let ch = &mut g.channels[idx];
        ch.muted = !ch.muted;
        let (id, muted) = (ch.id.clone(), ch.muted);
        drop(g);
        a.wl.set_muted(&id, muted);
    });
}

fn click_normal() {
    let (pending, hwnd, window_ms) =
        with_app(|a| (std::mem::replace(&mut a.click_pending, false), a.hwnd, a.cfg.double_click_ms));
    if pending {
        unsafe {
            let _ = KillTimer(Some(hwnd), T_CLICK);
        }
        open_overlay();
    } else {
        with_app(|a| a.click_pending = true);
        unsafe {
            SetTimer(Some(hwnd), T_CLICK, window_ms, None);
        }
    }
}

// --- overlay ---------------------------------------------------------------

fn open_overlay() {
    let (hwnd, timeout_ms) = with_app(|a| {
        let g = a.wl.shared.lock().unwrap();
        let idx = active_index(&a.active, &g.channels);
        let n = g.channels.len();
        a.selecting = Some(idx);
        a.overlay.open(n, idx, &a.cfg);
        a.overlay.tick(&g.channels);
        (a.hwnd, a.cfg.select_timeout_ms)
    });
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        SetTimer(Some(hwnd), T_SEL, timeout_ms, None);
        SetTimer(Some(hwnd), T_ANIM, FRAME_MS, None);
    }
}

fn move_selection(i: usize, up: bool) {
    let (hwnd, timeout_ms) = with_app(|a| {
        let dir = if up { 1 } else { -1 };
        // A direction change restarts the count; same-direction detents add up.
        a.sel_accum = if a.sel_accum.signum() == -dir { dir } else { a.sel_accum + dir };
        let throttled = a.last_sel_move.is_some_and(|t| t.elapsed().as_millis() < a.cfg.select_min_ms);
        if a.sel_accum.abs() >= a.cfg.select_detents && !throttled {
            a.sel_accum = 0;
            a.last_sel_move = Some(Instant::now());
            let n = a.wl.shared.lock().unwrap().channels.len();
            let next = if up { (i + 1) % n } else { (i + n - 1) % n };
            a.selecting = Some(next);
            a.overlay.set_selection(next);
        }
        (a.hwnd, a.cfg.select_timeout_ms)
    });
    unsafe {
        SetTimer(Some(hwnd), T_SEL, timeout_ms, None);
    }
}

fn confirm(i: usize) {
    with_app(|a| {
        let g = a.wl.shared.lock().unwrap();
        if let Some(ch) = g.channels.get(i) {
            save_active(&ch.id);
            a.active = Some(ch.id.clone());
        }
    });
    close_overlay();
}

/// Leaves selection mode immediately; the window slides out on its own and is
/// hidden by the animation timer once finished.
fn close_overlay() {
    let hwnd = with_app(|a| {
        a.selecting = None;
        a.overlay.begin_close();
        a.hwnd
    });
    unsafe {
        let _ = KillTimer(Some(hwnd), T_SEL);
    }
}

fn animate(hwnd: HWND) {
    let alive = with_app(|a| {
        let g = a.wl.shared.lock().unwrap();
        a.overlay.tick(&g.channels)
    });
    if !alive {
        unsafe {
            let _ = KillTimer(Some(hwnd), T_ANIM);
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        // Wave Link vanished under an open picker: leave selection mode.
        with_app(|a| a.selecting = None);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wparam.0 {
                T_CLICK => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), T_CLICK);
                    }
                    if with_app(|a| std::mem::replace(&mut a.click_pending, false)) {
                        toggle_mute();
                    }
                }
                T_SEL => close_overlay(),
                T_ANIM => animate(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_WL => {
            // Dev aid: KNOBY_PREVIEW=<file.png> opens the overlay once Wave Link
            // is connected and dumps a finished frame to that file.
            if let Some(path) = std::env::var_os("KNOBY_PREVIEW") {
                let go = with_app(|a| {
                    let ready = !a.wl.shared.lock().unwrap().channels.is_empty();
                    if ready && a.selecting.is_none() && !a.preview_done {
                        a.preview_done = true;
                        a.overlay.dump_frame_to(path.into());
                        return true;
                    }
                    false
                });
                if go {
                    open_overlay();
                }
            }
            LRESULT(0)
        }
        tray::WM_TRAY => {
            if matches!((lparam.0 & 0xFFFF) as u32, WM_RBUTTONUP | WM_CONTEXTMENU) {
                let status = with_app(|a| {
                    let g = a.wl.shared.lock().unwrap();
                    if !g.connected {
                        return "Waiting for Wave Link...".to_owned();
                    }
                    let idx = active_index(&a.active, &g.channels);
                    format!("Active channel: {}", g.channels.get(idx).map_or("-", |c| c.name.as_str()))
                });
                match tray::show_menu(hwnd, &status) {
                    Some(tray::Choice::Quit) => {
                        tray::remove(hwnd);
                        unsafe {
                            PostQuitMessage(0);
                        }
                    }
                    Some(tray::Choice::ToggleAutostart) => tray::set_autostart(!tray::autostart_enabled()),
                    Some(tray::Choice::OpenConfig) => {
                        // Make sure the file exists (first run creates it), then open it.
                        let cfg = load_config();
                        with_app(|a| a.cfg = cfg);
                        if let Some(p) = config::path() {
                            tray::open_in_notepad(&p);
                        }
                    }
                    Some(tray::Choice::ReloadConfig) => {
                        let cfg = load_config();
                        with_app(|a| a.cfg = cfg);
                    }
                    None => {}
                }
            }
            LRESULT(0)
        }
        // Explorer restarted: the notification area was wiped, add the icon again.
        m if m == taskbar_created() => {
            tray::add(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn taskbar_created() -> u32 {
    static MSG: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

fn main() -> windows::core::Result<()> {
    unsafe {
        // Two instances would both hook the keyboard and fight over the keys.
        let _single = CreateMutexW(None, true, w!("Local\\KnobySingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = GetModuleHandleW(PCWSTR::null())?;
        let class = w!("KnobyOverlay");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
            class,
            w!("Knoby"),
            WS_POPUP,
            0, 0, 10, 10,
            None, None, Some(hinst.into()), None,
        )?;

        let notify_hwnd = hwnd.0 as isize;
        let wl = wavelink::spawn(move || {
            let _ = PostMessageW(Some(HWND(notify_hwnd as *mut c_void)), WM_WL, WPARAM(0), LPARAM(0));
        });
        let scale = GetDpiForSystem() as f32 / 96.0;
        APP.with(|c| {
            *c.borrow_mut() = Some(App {
                hwnd,
                wl,
                overlay: Overlay::new(hwnd, scale),
                cfg: load_config(),
                active: load_active(),
                selecting: None,
                click_pending: false,
                preview_done: false,
                sel_accum: 0,
                last_sel_move: None,
            });
        });

        let _hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(hinst.into()), 0)?;
        tray::add(hwnd);
        println!("knoby running (tray icon > Quit, or Ctrl+C)");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
