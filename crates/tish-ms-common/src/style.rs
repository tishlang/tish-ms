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

/// `#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`, or `transparent` / `clear`.
pub fn parse_color(s: &str) -> Option<Rgba> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("transparent") || t.eq_ignore_ascii_case("clear") {
        return Some(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 });
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
