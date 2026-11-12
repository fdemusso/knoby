//! The channel picker overlay: software-rendered into a premultiplied BGRA
//! buffer and presented with `UpdateLayeredWindow` (per-pixel alpha), with a
//! slide-in / staggered-icon animation.

use std::time::Instant;

use std::ffi::c_void;

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW, ULW_ALPHA, UpdateLayeredWindow,
};
use windows::core::w;

use crate::config::{Config, Corner};
use crate::wavelink::{Channel, Icon};

const OPEN_MS: f32 = 240.0;
const CLOSE_MS: f32 = 180.0;
const ICON_DELAY_MS: f32 = 150.0;
const ICON_STAGGER_MS: f32 = 45.0;
const ICON_MS: f32 = 200.0;
/// Corner radius of Wave Link icons relative to their size.
const ICON_RADIUS: f32 = 0.12;

const BG: [f32; 3] = [20.0, 22.0, 27.0];
const BORDER: [f32; 3] = [43.0, 46.0, 56.0];
const TEXT: [f32; 3] = [236.0, 238.0, 244.0];
const MUTED: [f32; 3] = [140.0, 147.0, 166.0];
const RED: [f32; 3] = [255.0, 107.0, 107.0];
const FALLBACK_ACCENT: [f32; 3] = [200.0, 205.0, 220.0];

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn ease_out(x: f32) -> f32 {
    1.0 - (1.0 - clamp01(x)).powi(3)
}
fn ease_in(x: f32) -> f32 {
    clamp01(x).powi(3)
}

struct Geo {
    pad: f32,
    pad_top: f32,
    tile: f32,
    gap: f32,
    label_gap: f32,
    label_h: i32,
    w: i32,
    h: i32,
    margin: i32,
}

impl Geo {
    fn new(scale: f32, n: usize, margin: i32) -> Self {
        let s = |v: f32| v * scale;
        let n = n.max(1) as f32;
        let (pad, tile, gap) = (s(20.0), s(52.0), s(14.0));
        let label_h = s(24.0).round() as i32;
        let pad_top = s(16.0);
        let label_gap = s(10.0);
        let w = (2.0 * pad + n * tile + (n - 1.0) * gap).round() as i32;
        let h = (pad_top + tile + label_gap + label_h as f32 + s(12.0)).round() as i32;
        Geo { pad, pad_top, tile, gap, label_gap, label_h, w, h, margin: s(margin as f32).round() as i32 }
    }

    /// The volume bar: one icon with the name, level and a progress bar beside it.
    fn volume(scale: f32, margin: i32) -> Self {
        let s = |v: f32| v * scale;
        let (pad, tile, gap) = (s(20.0), s(52.0), s(14.0));
        let pad_top = s(16.0);
        let w = (2.0 * pad + tile + gap + s(190.0)).round() as i32;
        let h = (2.0 * pad_top + tile).round() as i32;
        Geo {
            pad, pad_top, tile, gap, label_gap: s(10.0), label_h: s(24.0).round() as i32, w, h,
            margin: s(margin as f32).round() as i32,
        }
    }

    fn cell_x(&self, i: usize) -> f32 {
        self.pad + i as f32 * (self.tile + self.gap)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// The channel picker.
    Picker,
    /// The volume bar shown while turning the knob or muting.
    Volume,
}

struct Anim {
    mode: Mode,
    start: Instant,
    closing: Option<Instant>,
    last: Instant,
    sel: usize,
    ring_x: f32,
    amt: Vec<f32>,
    /// Animated level of the volume bar, following the channel's level.
    shown: f32,
}

struct Surface {
    dc: HDC,
    dib: HBITMAP,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Surface {
    fn new(w: i32, h: i32) -> Option<Self> {
        unsafe {
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let dib = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let dc = CreateCompatibleDC(None);
            SelectObject(dc, dib.into());
            Some(Surface { dc, dib, bits: bits as *mut u8, w, h })
        }
    }

    fn bytes(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.dib.into());
        }
    }
}

pub struct Overlay {
    hwnd: HWND,
    scale: f32,
    font_name: HFONT,
    font_val: HFONT,
    geo: Geo,
    corner: Corner,
    canvas: Vec<u8>,
    surface: Option<Surface>,
    /// White-on-black glyph mask used to rasterise text with GDI.
    mask: Option<Surface>,
    anim: Option<Anim>,
    /// Dev aid: write one fully-animated frame to this PNG, then forget it.
    dump_path: Option<std::path::PathBuf>,
}

impl Overlay {
    pub fn new(hwnd: HWND, scale: f32) -> Self {
        let font = |weight: i32| unsafe {
            CreateFontW(
                -((15.0 * scale).round() as i32), 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY, 0, w!("Segoe UI"),
            )
        };
        Overlay {
            hwnd,
            scale,
            font_name: font(600),
            font_val: font(400),
            geo: Geo::new(scale, 1, 0),
            corner: Corner::TopRight,
            canvas: Vec::new(),
            surface: None,
            mask: None,
            anim: None,
            dump_path: None,
        }
    }

    pub fn dump_frame_to(&mut self, path: std::path::PathBuf) {
        self.dump_path = Some(path);
    }

    fn dump(&self, path: &std::path::Path) {
        let (w, h) = (self.geo.w as u32, self.geo.h as u32);
        let Ok(file) = std::fs::File::create(path) else { return };
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let rgba: Vec<u8> = self.canvas.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        if let Ok(mut wr) = enc.write_header() {
            let _ = wr.write_image_data(&rgba);
        }
    }

    fn prepare(&mut self, geo: Geo, cfg: &Config) {
        self.corner = cfg.overlay_corner;
        self.geo = geo;
        let (w, h) = (self.geo.w, self.geo.h);
        if self.surface.as_ref().is_none_or(|s| s.w != w || s.h != h) {
            self.surface = Surface::new(w, h);
            self.mask = Surface::new(w, self.geo.label_h);
        }
        self.canvas = vec![0; (w * h * 4) as usize];
    }

    pub fn open(&mut self, n: usize, sel: usize, cfg: &Config) {
        self.prepare(Geo::new(self.scale, n, cfg.overlay_margin), cfg);
        let now = Instant::now();
        let mut amt = vec![0.0; n];
        if let Some(a) = amt.get_mut(sel) {
            *a = 1.0;
        }
        self.anim = Some(Anim {
            mode: Mode::Picker, start: now, closing: None, last: now, sel,
            ring_x: self.geo.cell_x(sel), amt, shown: 0.0,
        });
    }

    /// Shows the volume bar for channel `sel`, whose level was `from` before the
    /// change, or refreshes it if it is already up. Returns true when the window
    /// was not showing the bar and has to be shown.
    pub fn show_volume(&mut self, sel: usize, from: f32, cfg: &Config) -> bool {
        let now = Instant::now();
        if let Some(a) = self.anim.as_mut()
            && a.mode == Mode::Volume
        {
            a.sel = sel;
            // Turned again while sliding out: slide back in from where it is.
            if let Some(c) = a.closing.take() {
                let p = clamp01(now.duration_since(c).as_secs_f32() * 1000.0 / CLOSE_MS);
                let back = std::time::Duration::from_secs_f32((1.0 - ease_in(p).cbrt()) * OPEN_MS / 1000.0);
                a.start = now.checked_sub(back).unwrap_or(now);
            }
            return false;
        }
        self.prepare(Geo::volume(self.scale, cfg.overlay_margin), cfg);
        self.anim = Some(Anim {
            mode: Mode::Volume, start: now, closing: None, last: now, sel,
            ring_x: 0.0, amt: Vec::new(), shown: from,
        });
        true
    }

    pub fn set_selection(&mut self, sel: usize) {
        if let Some(a) = self.anim.as_mut() {
            a.sel = sel;
        }
    }

    pub fn begin_close(&mut self) {
        if let Some(a) = self.anim.as_mut() {
            a.closing.get_or_insert_with(Instant::now);
        }
    }

    /// Advances the animation, renders and presents one frame. Returns false
    /// when the overlay is finished and the window can be hidden.
    pub fn tick(&mut self, channels: &[Channel]) -> bool {
        let Some(mut an) = self.anim.take() else { return false };
        if channels.is_empty() || self.surface.is_none() {
            return false;
        }
        let now = Instant::now();
        let dt = now.duration_since(an.last).as_secs_f32() * 1000.0;
        an.last = now;
        let k = 1.0 - (-dt / 70.0).exp();
        an.sel = an.sel.min(channels.len() - 1);
        for (i, a) in an.amt.iter_mut().enumerate() {
            *a += ((i == an.sel) as u8 as f32 - *a) * k;
        }
        an.ring_x += (self.geo.cell_x(an.sel) - an.ring_x) * k;
        an.shown += (channels[an.sel].level - an.shown) * k;

        let t = now.duration_since(an.start).as_secs_f32() * 1000.0;
        let (slide, alpha, finished) = match an.closing {
            Some(c) => {
                let p = now.duration_since(c).as_secs_f32() * 1000.0 / CLOSE_MS;
                (ease_in(p), 1.0 - ease_in(p), p >= 1.0)
            }
            None => (1.0 - ease_out(t / OPEN_MS), clamp01(t / (OPEN_MS * 0.7)), false),
        };

        self.render(channels, &an, t);
        if t > 900.0
            && an.closing.is_none()
            && let Some(p) = self.dump_path.take()
        {
            self.dump(&p);
        }
        self.present(slide, alpha);
        if !finished {
            self.anim = Some(an);
        }
        !finished
    }

    fn present(&mut self, slide: f32, alpha: f32) {
        let (w, h) = (self.geo.w, self.geo.h);
        let Some(surface) = self.surface.as_mut() else { return };
        surface.bytes().copy_from_slice(&self.canvas);
        // Work area = screen minus the taskbar, so bottom corners stay visible.
        let mut area = RECT::default();
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA, 0, Some(&mut area as *mut RECT as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
        let m = self.geo.margin;
        // Slides in from the screen edge nearest to the chosen corner.
        let travel = ((w + m) as f32 * slide).round() as i32;
        let x = if self.corner.is_right() { area.right - w - m + travel } else { area.left + m - travel };
        let y = if self.corner.is_top() { area.top + m } else { area.bottom - h - m };
        let pt = POINT { x, y };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (alpha * 0.97 * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            let _ = UpdateLayeredWindow(
                self.hwnd, None, Some(&pt), Some(&size), Some(surface.dc), Some(&src),
                COLORREF(0), Some(&blend), ULW_ALPHA,
            );
        }
    }

    fn render(&mut self, channels: &[Channel], an: &Anim, t: f32) {
        self.fill_panel();
        match an.mode {
            Mode::Picker => self.render_picker(channels, an, t),
            Mode::Volume => self.render_volume(&channels[an.sel], an),
        }
    }

    /// Panel and 1px border.
    fn fill_panel(&mut self) {
        let (w, h) = (self.geo.w, self.geo.h);
        for y in 0..h {
            for x in 0..w {
                let edge = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                let c = if edge { BORDER } else { BG };
                let i = ((y * w + x) * 4) as usize;
                self.canvas[i..i + 4].copy_from_slice(&[c[2] as u8, c[1] as u8, c[0] as u8, 255]);
            }
        }
    }

    fn render_volume(&mut self, ch: &Channel, an: &Anim) {
        let g = &self.geo;
        let (w, h) = (g.w, g.h);
        let accent = ch.icon.as_ref().map_or(FALLBACK_ACCENT, |i| i.accent.map(f32::from));

        if let Some(icon) = &ch.icon {
            let a = if ch.muted { 0.55 } else { 1.0 };
            blit_icon(&mut self.canvas, w, h, icon, g.pad, g.pad_top, g.tile, a);
        }

        // Name and level on one row, the bar below, the block centred on the icon.
        let (x0, x1) = ((g.pad + g.tile + g.gap) as i32, (w as f32 - g.pad) as i32);
        let bar_h = 8.0 * self.scale;
        let block = g.label_h as f32 + g.label_gap + bar_h;
        let top = g.pad_top + (g.tile - block) / 2.0;
        let (text_y, bar_y) = (top as i32, top + g.label_h as f32 + g.label_gap);

        let (val, val_color, fill_color) = if ch.muted {
            ("MUTED".to_owned(), RED, MUTED)
        } else {
            (format!("{}%", (an.shown * 100.0).round()), MUTED, accent)
        };
        let (name_w, val_w) = (wide(&ch.name), wide(&val));
        let vw = self.text_width(self.font_val, &val_w);
        let gap = (8.0 * self.scale) as i32;
        self.draw_text(&name_w, self.font_name, TEXT, x0, text_y, x1 - vw - gap, 1.0);
        self.draw_text(&val_w, self.font_val, val_color, x1 - vw, text_y, x1, 1.0);

        // Track, then the fill clipped to the animated level.
        let (cx, cy) = ((x0 + x1) as f32 / 2.0, bar_y + bar_h / 2.0);
        let (hw, hh) = ((x1 - x0) as f32 / 2.0, bar_h / 2.0);
        let fill_edge = x0 as f32 + (x1 - x0) as f32 * clamp01(an.shown);
        for y in (bar_y.floor() as i32 - 1).max(0)..((bar_y + bar_h).ceil() as i32 + 1).min(h) {
            for x in (x0 - 1).max(0)..(x1 + 1).min(w) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let cov = clamp01(0.5 - sd_round_rect(px - cx, py - cy, hw, hh, hh));
                if cov <= 0.0 {
                    continue;
                }
                let px_i = ((y * w + x) * 4) as usize;
                over(&mut self.canvas[px_i..], BORDER, cov);
                let fill = cov * clamp01(fill_edge - px + 0.5);
                if fill > 0.0 {
                    over(&mut self.canvas[px_i..], fill_color, fill);
                }
            }
        }
    }

    fn render_picker(&mut self, channels: &[Channel], an: &Anim, t: f32) {
        let g = &self.geo;
        let (w, h) = (g.w, g.h);
        let n = channels.len();

        let accent = channels[an.sel].icon.as_ref().map_or(FALLBACK_ACCENT, |i| i.accent.map(f32::from));
        let ring_vis = ease_out((t - (ICON_DELAY_MS + 80.0)) / ICON_MS);

        // Selection ring + glow, in the selected icon's own colour.
        if ring_vis > 0.0 {
            let half = g.tile / 2.0 + 5.0 * self.scale;
            let cx = an.ring_x + g.tile / 2.0;
            let cy = g.pad_top + g.tile / 2.0;
            let radius = ICON_RADIUS * g.tile + 5.0 * self.scale;
            let thick = 2.0 * self.scale;
            let reach = 18.0 * self.scale;
            let (x0, x1) = (((cx - half - reach) as i32).max(0), ((cx + half + reach) as i32 + 1).min(w));
            let (y0, y1) = (((cy - half - reach) as i32).max(0), ((cy + half + reach) as i32 + 1).min(h));
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                    let d = sd_round_rect(px, py, half, half, radius);
                    let ring = clamp01(0.5 - (d.abs() - thick / 2.0));
                    let glow = if d > 0.0 { 0.32 * (-d / (6.0 * self.scale)).exp() } else { 0.0 };
                    let a = ring.max(glow) * ring_vis;
                    if a > 0.002 {
                        over(&mut self.canvas[((y * w + x) * 4) as usize..], accent, a);
                    }
                }
            }
        }

        // Icons, revealed one after the other.
        for (i, ch) in channels.iter().enumerate() {
            let r = ease_out((t - (ICON_DELAY_MS + i as f32 * ICON_STAGGER_MS)) / ICON_MS);
            if r <= 0.0 {
                continue;
            }
            let amt = an.amt.get(i).copied().unwrap_or(0.0);
            let size = g.tile * (0.84 + 0.16 * amt);
            let cx = g.cell_x(i) + g.tile / 2.0;
            let cy = g.pad_top + g.tile / 2.0 + (1.0 - r) * 8.0 * self.scale;
            let a = r * (0.5 + 0.5 * amt) * if ch.muted { 0.55 } else { 1.0 };
            if let Some(icon) = &ch.icon {
                blit_icon(&mut self.canvas, w, h, icon, cx - size / 2.0, cy - size / 2.0, size, a);
            }
        }

        // Label: name + level (or MUTED).
        let label_vis = ease_out((t - (ICON_DELAY_MS + 80.0)) / ICON_MS);
        if label_vis > 0.0 {
            let ch = &channels[an.sel];
            let (val, val_color) = if ch.muted {
                ("MUTED".to_owned(), RED)
            } else {
                (format!("{}%", (ch.level * 100.0).round()), MUTED)
            };
            self.draw_label(&ch.name, &val, val_color, label_vis);
        }
        let _ = n;
    }

    fn draw_label(&mut self, name: &str, val: &str, val_color: [f32; 3], alpha: f32) {
        let y_off = (self.geo.pad_top + self.geo.tile + self.geo.label_gap) as i32;
        let gap = (8.0 * self.scale) as i32;
        let (name_w, val_w) = (wide(name), wide(val));
        let nw = self.text_width(self.font_name, &name_w);
        let vw = self.text_width(self.font_val, &val_w);
        let x = (self.geo.w - (nw + gap + vw)) / 2;
        self.draw_text(&name_w, self.font_name, TEXT, x, y_off, self.geo.w, alpha);
        self.draw_text(&val_w, self.font_val, val_color, x + nw + gap, y_off, self.geo.w, alpha);
    }

    fn text_width(&self, font: HFONT, s: &[u16]) -> i32 {
        let Some(mask) = self.mask.as_ref() else { return 0 };
        unsafe {
            let old = SelectObject(mask.dc, font.into());
            let mut sz = SIZE::default();
            let _ = GetTextExtentPoint32W(mask.dc, s, &mut sz);
            SelectObject(mask.dc, old);
            sz.cx
        }
    }

    /// Draws `text` with its top-left at (`x`, `y_off`), clipped at `clip_right`.
    #[allow(clippy::too_many_arguments)]
    fn draw_text(&mut self, text: &[u16], font: HFONT, color: [f32; 3], x: i32, y_off: i32, clip_right: i32, alpha: f32) {
        let tw = self.text_width(font, text);
        let Some(mask) = self.mask.as_mut() else { return };
        let (w, label_h) = (self.geo.w, self.geo.label_h);
        unsafe {
            let m = mask.bytes();
            m.fill(0);
            let old = SelectObject(mask.dc, font.into());
            SetBkMode(mask.dc, TRANSPARENT);
            SetTextColor(mask.dc, COLORREF(0x00ffffff));
            let _ = TextOutW(mask.dc, 0, 0, text);
            SelectObject(mask.dc, old);
            let _ = GdiFlush();
        }
        let m = mask.bytes();
        for yy in 0..label_h {
            for xx in 0..(tw + 2).min(w) {
                let cov = m[((yy * w + xx) * 4 + 1) as usize] as f32 / 255.0;
                let (dx, dy) = (x + xx, y_off + yy);
                if cov > 0.0 && dx >= 0 && dx < clip_right.min(w) && dy >= 0 && dy < self.geo.h {
                    over(&mut self.canvas[((dy * w + dx) * 4) as usize..], color, cov * alpha);
                }
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Signed distance from (px, py) to a rounded rectangle centred at the origin.
pub fn sd_round_rect(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (qx, qy) = (px.abs() - (hw - r), py.abs() - (hh - r));
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// Composites `color` at coverage `a` over an opaque premultiplied BGRA pixel.
fn over(px: &mut [u8], color: [f32; 3], a: f32) {
    let inv = 1.0 - a;
    px[0] = (color[2] * a + px[0] as f32 * inv + 0.5) as u8;
    px[1] = (color[1] * a + px[1] as f32 * inv + 0.5) as u8;
    px[2] = (color[0] * a + px[2] as f32 * inv + 0.5) as u8;
}

/// Bilinear-scaled blit of a premultiplied BGRA icon onto the canvas.
#[allow(clippy::too_many_arguments)]
fn blit_icon(canvas: &mut [u8], cw: i32, ch: i32, icon: &Icon, x0: f32, y0: f32, size: f32, alpha: f32) {
    let (sx, sy) = (icon.w as f32 / size, icon.h as f32 / size);
    let texel = |x: i32, y: i32| {
        let (x, y) = (x.clamp(0, icon.w - 1), y.clamp(0, icon.h - 1));
        let i = ((y * icon.w + x) * 4) as usize;
        [icon.bgra[i] as f32, icon.bgra[i + 1] as f32, icon.bgra[i + 2] as f32, icon.bgra[i + 3] as f32]
    };
    for y in (y0.floor() as i32).max(0)..((y0 + size).ceil() as i32).min(ch) {
        for x in (x0.floor() as i32).max(0)..((x0 + size).ceil() as i32).min(cw) {
            let u = (x as f32 + 0.5 - x0) * sx - 0.5;
            let v = (y as f32 + 0.5 - y0) * sy - 0.5;
            let (iu, iv) = (u.floor(), v.floor());
            let (fu, fv) = (u - iu, v - iv);
            let (a, b) = (texel(iu as i32, iv as i32), texel(iu as i32 + 1, iv as i32));
            let (c, d) = (texel(iu as i32, iv as i32 + 1), texel(iu as i32 + 1, iv as i32 + 1));
            let mut s = [0.0f32; 4];
            for k in 0..4 {
                s[k] = (a[k] * (1.0 - fu) + b[k] * fu) * (1.0 - fv) + (c[k] * (1.0 - fu) + d[k] * fu) * fv;
            }
            let inv = 1.0 - (s[3] / 255.0) * alpha;
            let i = ((y * cw + x) * 4) as usize;
            for k in 0..3 {
                canvas[i + k] = (s[k] * alpha + canvas[i + k] as f32 * inv + 0.5).min(255.0) as u8;
            }
        }
    }
}
