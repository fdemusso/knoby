mod accel;
mod wavelink;

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use accel::Accel;
use wavelink::{Channel, Handle};

const WM_WL: u32 = WM_APP + 1;
const T_CLICK: usize = 1;
const T_SEL: usize = 2;
const CLICK_MS: u32 = 250;
const SEL_MS: u32 = 4000;

const VK_VOLUME_MUTE: u32 = 0xAD;
const VK_VOLUME_DOWN: u32 = 0xAE;
const VK_VOLUME_UP: u32 = 0xAF;
const VK_F13: u32 = 0x7C;
const LLKHF_INJECTED: u32 = 0x10;

const BG: COLORREF = COLORREF(0x001e1e1e);
const TILE_BG: COLORREF = COLORREF(0x002b2b2b);
const ACCENT: COLORREF = COLORREF(0x0000b976);
const TEXT: COLORREF = COLORREF(0x00f0f0f0);

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Up,
    Down,
    Click,
}

struct App {
    hwnd: HWND,
    wl: Handle,
    scale: f32,
    accel: Accel,
    active: Option<String>,
    /// Some(index) while the channel picker overlay is open.
    selecting: Option<usize>,
    click_pending: bool,
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

fn layout(scale: f32, n: usize) -> (i32, i32) {
    let s = |v: f32| (v * scale).round() as i32;
    let n = n as i32;
    (2 * s(20.0) + n * s(56.0) + (n - 1).max(0) * s(12.0), s(20.0) + s(56.0) + s(8.0) + s(28.0) + s(12.0))
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
        if kb.flags.0 & LLKHF_INJECTED == 0 {
            let key = match kb.vkCode {
                VK_VOLUME_UP => Some(Key::Up),
                VK_VOLUME_DOWN => Some(Key::Down),
                VK_VOLUME_MUTE | VK_F13 => Some(Key::Click),
                _ => None,
            };
            if let Some(key) = key {
                if ready() {
                    if matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
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
    match (selecting, key) {
        (None, Key::Click) => click_normal(),
        (None, k) => adjust(k == Key::Up),
        (Some(i), Key::Click) => confirm(i),
        (Some(i), k) => move_selection(i, k == Key::Up),
    }
}

fn adjust(up: bool) {
    with_app(|a| {
        let step = a.accel.step(Instant::now());
        let mut g = a.wl.shared.lock().unwrap();
        let idx = active_index(&a.active, &g.channels);
        let ch = &mut g.channels[idx];
        let d = if up { step } else { -step };
        ch.level = ((ch.level + d).clamp(0.0, 1.0) * 10000.0).round() / 10000.0;
        ch.local_set = Some(Instant::now());
        let (id, level) = (ch.id.clone(), ch.level);
        drop(g);
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
    let (pending, hwnd) = with_app(|a| (std::mem::replace(&mut a.click_pending, false), a.hwnd));
    if pending {
        unsafe {
            let _ = KillTimer(Some(hwnd), T_CLICK);
        }
        open_overlay();
    } else {
        with_app(|a| a.click_pending = true);
        unsafe {
            SetTimer(Some(hwnd), T_CLICK, CLICK_MS, None);
        }
    }
}

// --- overlay ---------------------------------------------------------------

fn open_overlay() {
    let (hwnd, scale, n) = with_app(|a| {
        let g = a.wl.shared.lock().unwrap();
        let idx = active_index(&a.active, &g.channels);
        let n = g.channels.len();
        drop(g);
        a.selecting = Some(idx);
        (a.hwnd, a.scale, n)
    });
    let (w, h) = layout(scale, n);
    let margin = (24.0 * scale).round() as i32;
    let x = unsafe { GetSystemMetrics(SM_CXSCREEN) } - w - margin;
    unsafe {
        SetTimer(Some(hwnd), T_SEL, SEL_MS, None);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, margin, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn move_selection(i: usize, up: bool) {
    let hwnd = with_app(|a| {
        let n = a.wl.shared.lock().unwrap().channels.len();
        a.selecting = Some(if up { (i + 1) % n } else { (i + n - 1) % n });
        a.hwnd
    });
    unsafe {
        SetTimer(Some(hwnd), T_SEL, SEL_MS, None);
        let _ = InvalidateRect(Some(hwnd), None, false);
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

fn close_overlay() {
    let hwnd = with_app(|a| {
        a.selecting = None;
        a.hwnd
    });
    unsafe {
        let _ = KillTimer(Some(hwnd), T_SEL);
        let _ = ShowWindow(hwnd, SW_HIDE);
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
                _ => {}
            }
            LRESULT(0)
        }
        WM_WL => {
            if with_app(|a| a.selecting.is_some()) {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                APP.with(|c| {
                    if let Ok(a) = c.try_borrow() {
                        if let Some(a) = a.as_ref() {
                            paint(hwnd, hdc, a);
                        }
                    }
                });
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn paint(hwnd: HWND, hdc: HDC, a: &App) {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bmp.into());

        let bg = CreateSolidBrush(BG);
        FillRect(mem, &rc, bg);
        let _ = DeleteObject(bg.into());

        let s = |v: f32| (v * a.scale).round() as i32;
        let (pad, tile, gap) = (s(20.0), s(56.0), s(12.0));
        let sel = a.selecting.unwrap_or(0);
        let g = a.wl.shared.lock().unwrap();

        for (i, ch) in g.channels.iter().enumerate() {
            let x = pad + i as i32 * (tile + gap);
            let y = pad;
            let selected = i == sel;
            if selected {
                let r = RECT { left: x - s(4.0), top: y - s(4.0), right: x + tile + s(4.0), bottom: y + tile + s(4.0) };
                let b = CreateSolidBrush(ACCENT);
                FillRect(mem, &r, b);
                let _ = DeleteObject(b.into());
            }
            let r = RECT { left: x, top: y, right: x + tile, bottom: y + tile };
            let b = CreateSolidBrush(TILE_BG);
            FillRect(mem, &r, b);
            let _ = DeleteObject(b.into());
            if let Some(icon) = &ch.icon {
                draw_icon(mem, icon, x, y, tile, if selected { 255 } else { 110 });
            }
        }

        if let Some(ch) = g.channels.get(sel) {
            let status = if ch.muted { "MUTED".to_owned() } else { format!("{}%", (ch.level * 100.0).round()) };
            let mut text: Vec<u16> = format!("{}  {status}", ch.name).encode_utf16().collect();
            let font = CreateFontW(
                -s(18.0), 0, 0, 0, 600, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI"),
            );
            let of = SelectObject(mem, font.into());
            SetBkMode(mem, TRANSPARENT);
            SetTextColor(mem, TEXT);
            let mut tr = RECT { left: 0, top: pad + tile + s(8.0), right: w, bottom: h };
            DrawTextW(mem, &mut text, &mut tr, DT_CENTER | DT_TOP | DT_SINGLELINE);
            SelectObject(mem, of);
            let _ = DeleteObject(font.into());
        }
        drop(g);

        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
    }
}

unsafe fn draw_icon(dst: HDC, icon: &wavelink::Icon, x: i32, y: i32, size: i32, alpha: u8) {
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: icon.w,
                biHeight: -icon.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(dst), &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return;
        };
        std::ptr::copy_nonoverlapping(icon.bgra.as_ptr(), bits as *mut u8, icon.bgra.len());
        let src = CreateCompatibleDC(Some(dst));
        let old = SelectObject(src, dib.into());
        let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        let _ = AlphaBlend(dst, x, y, size, size, src, 0, 0, icon.w, icon.h, bf);
        SelectObject(src, old);
        let _ = DeleteDC(src);
        let _ = DeleteObject(dib.into());
    }
}

fn main() -> windows::core::Result<()> {
    unsafe {
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
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA)?;

        let notify_hwnd = hwnd.0 as isize;
        let wl = wavelink::spawn(move || {
            let _ = PostMessageW(Some(HWND(notify_hwnd as *mut c_void)), WM_WL, WPARAM(0), LPARAM(0));
        });
        let scale = GetDpiForSystem() as f32 / 96.0;
        APP.with(|c| {
            *c.borrow_mut() = Some(App {
                hwnd,
                wl,
                scale,
                accel: Accel::default(),
                active: load_active(),
                selecting: None,
                click_pending: false,
            });
        });

        let _hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(hinst.into()), 0)?;
        println!("knoby running (Ctrl+C to quit)");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
