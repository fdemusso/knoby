//! Generates the Knoby app icon (rounded violet square, white dial, dark notch).
//!
//! `cargo run --example make_icon` rewrites `assets/knoby.ico` (16-256 px, PNG-compressed entries) and the
//! `assets/icon-256.png` / `assets/icon-512.png` images used by the README and as a repository avatar.
//! The .ico is embedded into the executable by `build.rs`.

use std::fs;

/// Signed distance from (px, py) to a rounded rectangle centred at the origin.
fn sd_round_rect(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (qx, qy) = (px.abs() - (hw - r), py.abs() - (hh - r));
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Straight (non-premultiplied) RGBA pixels of the s x s icon.
fn pixels(s: u32) -> Vec<u8> {
    let sf = s as f32;
    let mut out = Vec::with_capacity((s * s * 4) as usize);
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = (x as f32 + 0.5 - sf / 2.0, y as f32 + 0.5 - sf / 2.0);
            let body = (0.5 - sd_round_rect(fx, fy, sf / 2.0, sf / 2.0, sf * 0.26)).clamp(0.0, 1.0);
            let dial = (0.5 - ((fx * fx + fy * fy).sqrt() - sf * 0.31)).clamp(0.0, 1.0);
            // Indicator notch: a vertical capsule from just above the centre to the dial's rim.
            let t = ((fy + sf * 0.05) / (sf * 0.19)).clamp(0.0, 1.0);
            let seg = ((fx * fx + (fy + sf * 0.05 - t * sf * 0.19).powi(2)).sqrt() - sf * 0.05).max(-1.0);
            let notch = (0.5 - seg).clamp(0.0, 1.0);

            let mut c = [109.0, 124.0, 255.0]; // #6D7CFF
            c = mix(c, [245.0, 246.0, 250.0], dial);
            c = mix(c, [43.0, 47.0, 85.0], notch * dial);
            out.extend([c[0] as u8, c[1] as u8, c[2] as u8, (255.0 * body) as u8]);
        }
    }
    out
}

fn png(s: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, s, s);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&pixels(s)).unwrap();
    out
}

fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| png(s)).collect();
    let mut out = Vec::new();
    out.extend([0u8, 0, 1, 0]); // reserved, type = icon
    out.extend((sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        out.extend([(s % 256) as u8, (s % 256) as u8, 0, 0]); // 0 means 256
        out.extend(1u16.to_le_bytes()); // planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((img.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend(img);
    }
    out
}

fn main() {
    fs::create_dir_all("assets").unwrap();
    fs::write("assets/knoby.ico", ico(&[16, 24, 32, 48, 64, 128, 256])).unwrap();
    fs::write("assets/icon-256.png", png(256)).unwrap();
    fs::write("assets/icon-512.png", png(512)).unwrap();
    println!("wrote assets/knoby.ico, assets/icon-256.png, assets/icon-512.png");
}
