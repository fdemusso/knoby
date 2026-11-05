//! User settings, read from `%APPDATA%\Knoby\config.toml`.
//!
//! The format is a tiny TOML subset (`key = value`, `#` comments), parsed by
//! hand to avoid pulling in a TOML/serde dependency for seven settings.
//! Invalid or out-of-range values fall back to the default and are reported.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Corner {
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl Corner {
    pub fn is_right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }

    pub fn is_top(self) -> bool {
        matches!(self, Corner::TopRight | Corner::TopLeft)
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "top-right" => Some(Corner::TopRight),
            "top-left" => Some(Corner::TopLeft),
            "bottom-right" => Some(Corner::BottomRight),
            "bottom-left" => Some(Corner::BottomLeft),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Volume change per knob detent, as a fraction (0.03 = 3%).
    pub volume_step: f32,
    /// Knob detents needed to move the overlay selection by one icon (1 = every detent).
    pub select_detents: i32,
    /// Speed limit: detents sooner than this after the last move are ignored.
    pub select_min_ms: u128,
    /// Window in which a second click counts as a double click.
    pub double_click_ms: u32,
    /// Overlay closes after this long without input.
    pub select_timeout_ms: u32,
    pub overlay_corner: Corner,
    /// Distance from the screen edge, in pixels at 100% scaling.
    pub overlay_margin: i32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            volume_step: 0.03,
            select_detents: 1,
            select_min_ms: 90,
            double_click_ms: 250,
            select_timeout_ms: 4000,
            overlay_corner: Corner::TopRight,
            overlay_margin: 24,
        }
    }
}

pub const DEFAULT_FILE: &str = "\
# Knoby settings. Use the tray menu (Reload config) to apply changes.

# Volume change per knob detent, in percent (1-20).
volume_step = 3

# Knob detents needed to move the channel picker by one icon (1-6): 1 means
# every detent moves one icon. select_min_ms is the speed limit: detents
# arriving sooner than this after the last move are ignored (0-500).
select_detents = 1
select_min_ms = 90

# A second click within this many milliseconds is a double click (100-600);
# a single click (mute) fires only after this delay.
double_click_ms = 250

# The channel picker closes after this many milliseconds without input (1000-15000).
select_timeout_ms = 4000

# Where the channel picker appears: top-right, top-left, bottom-right, bottom-left.
overlay_corner = \"top-right\"

# Distance from the screen edge in pixels (0-200).
overlay_margin = 24
";

pub fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Knoby").join("config.toml"))
}

/// Loads the config, creating the default file on first run. Never fails:
/// problems are returned as warnings and the affected setting keeps its default.
pub fn load() -> (Config, Vec<String>) {
    let Some(p) = path() else { return (Config::default(), vec![]) };
    match std::fs::read_to_string(&p) {
        Ok(text) => parse(&text),
        Err(_) => {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&p, DEFAULT_FILE);
            (Config::default(), vec![])
        }
    }
}

pub fn parse(text: &str) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warnings = Vec::new();

    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!("line {}: expected `key = value`", n + 1));
            continue;
        };
        let (key, value) = (key.trim(), value.trim().trim_matches('"'));

        let int = |min: i64, max: i64| -> Result<i64, String> {
            let v: i64 = value.parse().map_err(|_| format!("`{key}` must be a whole number"))?;
            if (min..=max).contains(&v) { Ok(v) } else { Err(format!("`{key}` must be between {min} and {max}")) }
        };
        let result = match key {
            "volume_step" => int(1, 20).map(|v| cfg.volume_step = v as f32 / 100.0),
            "select_detents" => int(1, 6).map(|v| cfg.select_detents = v as i32),
            "select_min_ms" => int(0, 500).map(|v| cfg.select_min_ms = v as u128),
            "double_click_ms" => int(100, 600).map(|v| cfg.double_click_ms = v as u32),
            "select_timeout_ms" => int(1000, 15000).map(|v| cfg.select_timeout_ms = v as u32),
            "overlay_margin" => int(0, 200).map(|v| cfg.overlay_margin = v as i32),
            "overlay_corner" => Corner::parse(value)
                .map(|c| cfg.overlay_corner = c)
                .ok_or_else(|| "`overlay_corner` must be top-right, top-left, bottom-right or bottom-left".to_owned()),
            _ => Err(format!("unknown setting `{key}`")),
        };
        if let Err(e) = result {
            warnings.push(format!("line {}: {e}", n + 1));
        }
    }
    (cfg, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_parses_to_defaults_without_warnings() {
        let (cfg, warnings) = parse(DEFAULT_FILE);
        assert_eq!(cfg, Config::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn overrides_values_and_ignores_comments() {
        let (cfg, w) = parse("volume_step = 5  # louder\noverlay_corner = \"bottom-left\"\n# select_detents = 4\n");
        assert!(w.is_empty());
        assert_eq!(cfg.volume_step, 0.05);
        assert_eq!(cfg.overlay_corner, Corner::BottomLeft);
        assert_eq!(cfg.select_detents, 1);
    }

    #[test]
    fn bad_values_keep_defaults_and_warn() {
        let (cfg, w) = parse("volume_step = 99\nselect_detents = two\nbogus = 1\nno equals sign\noverlay_corner = middle\n");
        assert_eq!(cfg, Config::default());
        assert_eq!(w.len(), 5, "{w:?}");
    }
}
