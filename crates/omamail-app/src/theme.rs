//! Omarchy `colors.toml` → the handful of colours the window actually paints.
//!
//! The live theme name is `~/.local/state/omarchy/current/theme.name`. Older
//! Omarchy installs also used `~/.config/omarchy/current/theme`.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Palette {
    pub bg: String,
    pub surface: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub selection: String,
    pub red: String,
    pub on_accent: String,
    pub is_dark: bool,
}

impl Palette {
    pub fn fallback() -> Self {
        Self {
            bg: "#1a1b26".into(),
            surface: "#24283b".into(),
            text: "#c0caf5".into(),
            muted: "#9aa5ce".into(),
            accent: "#7aa2f7".into(),
            selection: "#283457".into(),
            red: "#f7768e".into(),
            on_accent: "#1a1b26".into(),
            is_dark: true,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct ColorsFile {
    mode: Option<String>,
    accent: Option<String>,
    selection: Option<String>,
    muted: Option<String>,
    background: Option<String>,
    lighter_background: Option<String>,
    foreground: Option<String>,
    light_foreground: Option<String>,
    red: Option<String>,
    blue: Option<String>,
}

pub fn current_palette() -> Palette {
    colors_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|src| parse_colors(&src))
        .unwrap_or_else(Palette::fallback)
}

pub fn parse_colors(src: &str) -> Option<Palette> {
    let file: ColorsFile = toml::from_str(src).ok()?;
    let bg = normalize_hex(file.background.as_deref()?)?;
    let text = normalize_hex(file.foreground.as_deref()?)?;
    let is_dark = match file.mode.as_deref() {
        Some("dark") => true,
        Some("light") => false,
        _ => luminance(&bg) < 0.5,
    };
    let surface = file
        .lighter_background
        .as_deref()
        .and_then(normalize_hex)
        .unwrap_or_else(|| bg.clone());
    let muted = file
        .light_foreground
        .as_deref()
        .and_then(normalize_hex)
        .or_else(|| file.muted.as_deref().and_then(normalize_hex))
        .unwrap_or_else(|| text.clone());
    let accent = file
        .accent
        .as_deref()
        .and_then(normalize_hex)
        .or_else(|| file.blue.as_deref().and_then(normalize_hex))
        .unwrap_or_else(|| text.clone());
    let selection = file
        .selection
        .as_deref()
        .and_then(normalize_hex)
        .unwrap_or_else(|| surface.clone());
    let red = file
        .red
        .as_deref()
        .and_then(normalize_hex)
        .unwrap_or_else(|| "#f7768e".into());
    let on_accent = if luminance(&accent) > 0.6 {
        "#1a1b26".into()
    } else {
        "#f7f7fb".into()
    };
    Some(Palette {
        bg,
        surface,
        text,
        muted,
        accent,
        selection,
        red,
        on_accent,
        is_dark,
    })
}

pub fn colors_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    for rel in [
        ".local/state/omarchy/current/theme",
        ".config/omarchy/current/theme",
    ] {
        if let Some(path) = colors_in(&home.join(rel)) {
            return Some(path);
        }
    }
    let name =
        std::fs::read_to_string(home.join(".local/state/omarchy/current/theme.name")).ok()?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let omarchy = std::env::var_os("OMARCHY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/share/omarchy"));
    [
        home.join(".config/omarchy/themes").join(name),
        omarchy.join("themes").join(name),
    ]
    .into_iter()
    .find_map(|dir| colors_in(&dir))
}

fn colors_in(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let file = path.join("colors.toml");
    file.is_file().then_some(file)
}

fn normalize_hex(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let hex = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("0x"))
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", hex.to_ascii_lowercase()))
}

fn luminance(hex: &str) -> f32 {
    let bytes = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&bytes[0..2], 16).unwrap_or(0) as f32 / 255.0;
    let g = u8::from_str_radix(&bytes[2..4], 16).unwrap_or(0) as f32 / 255.0;
    let b = u8::from_str_radix(&bytes[4..6], 16).unwrap_or(0) as f32 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omarchy_colors_toml_parses() {
        let src = r##"
mode = "dark"
accent = "#89b4fa"
selection = "#45475a"
muted = "#585b70"
background = "#1e1e2e"
lighter_background = "#313244"
foreground = "#cdd6f4"
light_foreground = "#bac2de"
red = "#f38ba8"
"##;
        let palette = parse_colors(src).unwrap();
        assert_eq!(palette.bg, "#1e1e2e");
        assert_eq!(palette.surface, "#313244");
        assert_eq!(palette.text, "#cdd6f4");
        assert_eq!(palette.muted, "#bac2de");
        assert_eq!(palette.accent, "#89b4fa");
        assert_eq!(palette.selection, "#45475a");
        assert_eq!(palette.red, "#f38ba8");
        assert!(palette.is_dark);
    }

    #[test]
    fn garbage_is_not_a_palette() {
        assert!(parse_colors("this is not toml {{{").is_none());
        assert!(parse_colors("accent = \"#ffffff\"\n").is_none());
    }

    #[test]
    fn light_mode_is_honoured() {
        let src = r##"
mode = "light"
background = "#eff1f5"
foreground = "#4c4f69"
accent = "#1e66f5"
"##;
        let palette = parse_colors(src).unwrap();
        assert!(!palette.is_dark);
        assert_eq!(palette.on_accent, "#f7f7fb");
    }
}
