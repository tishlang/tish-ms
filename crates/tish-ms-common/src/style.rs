//! React-style prop helpers.

use tishlang_core::{PropMap, Value};

pub fn props_string(props: &PropMap, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match props.get(k) {
        Some(Value::String(s)) => Some(s.to_string()),
        _ => None,
    })
}

pub fn props_f64(props: &PropMap, keys: &[&str], default: f64) -> f64 {
    keys.iter().find_map(|k| props.get(k).and_then(|v| v.as_number())).unwrap_or(default)
}

pub fn props_bool(props: &PropMap, keys: &[&str], default: bool) -> bool {
    for k in keys {
        if let Some(v) = props.get(k) {
            return match v {
                Value::Bool(b) => *b,
                Value::Number(n) => *n != 0.0,
                Value::Null => false,
                _ => default,
            };
        }
    }
    default
}

/// sRGB colour, components 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// The system accent colour as 0xRRGGBB (macOS's default blue until the host sets it).
static ACCENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x0A84FF);

/// Light or dark: which values the semantic colours take.
pub fn set_dark(dark: bool) {
    DARK.store(dark, std::sync::atomic::Ordering::Relaxed);
}

/// The system accent colour, 0xRRGGBB, for `controlAccent`.
pub fn set_accent(rgb: u32) {
    ACCENT.store(rgb, std::sync::atomic::Ordering::Relaxed);
}

/// AppKit's semantic colour names, so a theme written for tish-macos looks the same: the label
/// colours are white or black at AppKit's opacities, `controlAccent` the system accent.
fn semantic(name: &str) -> Option<Rgba> {
    let dark = DARK.load(std::sync::atomic::Ordering::Relaxed);
    let ink = |a: f32| Some(if dark { Rgba { r: 1.0, g: 1.0, b: 1.0, a } } else { Rgba { r: 0.0, g: 0.0, b: 0.0, a } });
    match name {
        "label" | "labelColor" | "textColor" | "controlTextColor" => ink(if dark { 0.85 } else { 0.85 }),
        "secondaryLabel" | "secondaryLabelColor" => ink(if dark { 0.55 } else { 0.5 }),
        "tertiaryLabel" | "tertiaryLabelColor" => ink(if dark { 0.25 } else { 0.26 }),
        "quaternaryLabel" | "quaternaryLabelColor" => ink(if dark { 0.1 } else { 0.1 }),
        "separator" | "separatorColor" | "gridColor" => ink(0.1),
        "controlAccent" | "controlAccentColor" | "accent" | "selectedContentBackgroundColor" => {
            let c = ACCENT.load(std::sync::atomic::Ordering::Relaxed);
            let f = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
            Some(Rgba { r: f(16), g: f(8), b: f(0), a: 1.0 })
        }
        "systemInfo" => Some(Rgba { r: 0.04, g: 0.52, b: 1.0, a: 1.0 }),
        _ => None,
    }
}

/// `#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`, `transparent` / `clear`, or an AppKit semantic name
/// (`label`, `secondaryLabel`, `controlAccent`, `separator`, …) in the current light / dark mode.
pub fn parse_color(s: &str) -> Option<Rgba> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("transparent") || t.eq_ignore_ascii_case("clear") {
        return Some(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 });
    }
    if let Some(c) = semantic(t) {
        return Some(c);
    }
    let hex = t.strip_prefix('#')?;
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = hex.as_bytes();
    let bytes: Vec<u8> = match b.len() {
        3 | 4 => b.iter().map(|&c| digit(c).map(|d| d * 17)).collect::<Option<_>>()?,
        6 | 8 => b.chunks(2).map(|p| Some(digit(p[0])? * 16 + digit(p[1])?)).collect::<Option<_>>()?,
        _ => return None,
    };
    let f = |i: usize| bytes.get(i).map_or(1.0, |&v| v as f32 / 255.0);
    Some(Rgba { r: f(0), g: f(1), b: f(2), a: f(3) })
}

pub fn props_color(props: &PropMap, keys: &[&str]) -> Option<Rgba> {
    props_string(props, keys).and_then(|s| parse_color(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#fff"), Some(Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }));
        assert_eq!(parse_color("#00000080").map(|c| (c.a * 255.0).round()), Some(128.0));
        assert_eq!(parse_color("transparent").map(|c| c.a), Some(0.0));
        assert_eq!(parse_color("red"), None);
    }
}
