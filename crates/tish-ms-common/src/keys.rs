//! Hotkey specs (`"ctrl+alt+space"`, `"win+shift+k"`) to Windows modifier flags and virtual-key
//! codes. Pure, so it's tested anywhere.

/// `MOD_ALT`, `MOD_CONTROL`, `MOD_SHIFT`, `MOD_WIN`, as RegisterHotKey takes them.
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

fn vk(key: &str) -> Option<u32> {
    let k = key.to_ascii_lowercase();
    let b = k.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x6F + n);
    }
    Some(match k.as_str() {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" => 0x2E,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "`" | "backquote" => 0xC0,
        "-" | "minus" => 0xBD,
        "=" | "equal" => 0xBB,
        "," | "comma" => 0xBC,
        "." | "period" => 0xBE,
        "/" | "slash" => 0xBF,
        ";" | "semicolon" => 0xBA,
        _ => return None,
    })
}

/// Parse a spec; `cmd` and `meta` mean the Windows key, `option` means Alt, so macOS-style specs
/// carry over.
pub fn parse(spec: &str) -> Result<Hotkey, String> {
    let parts: Vec<&str> = spec.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    let Some((key, mods)) = parts.split_last() else { return Err("empty hotkey".into()) };
    let mut m = 0;
    for p in mods {
        m |= match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => MOD_CONTROL,
            "alt" | "option" | "opt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            "win" | "super" | "cmd" | "command" | "meta" => MOD_WIN,
            other => return Err(format!("unknown modifier `{other}`")),
        };
    }
    if m == 0 {
        return Err("a hotkey needs a modifier (ctrl, alt, shift or win)".into());
    }
    let vk = vk(key).ok_or_else(|| format!("unknown key `{key}`"))?;
    Ok(Hotkey { mods: m, vk })
}

/// How Windows writes it: `Ctrl+Alt+Space`.
pub fn display(spec: &str) -> String {
    spec.split('+')
        .map(|p| match p.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "Ctrl".to_string(),
            "alt" | "option" | "opt" => "Alt".into(),
            "shift" => "Shift".into(),
            "win" | "super" | "cmd" | "command" | "meta" => "Win".into(),
            k => {
                let mut c = k.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse("ctrl+alt+space"), Ok(Hotkey { mods: MOD_CONTROL | MOD_ALT, vk: 0x20 }));
        assert_eq!(parse("cmd+shift+k"), Ok(Hotkey { mods: MOD_WIN | MOD_SHIFT, vk: b'K' as u32 }));
        assert_eq!(parse("alt+f4").map(|h| h.vk), Ok(0x73));
        assert!(parse("space").is_err());
        assert!(parse("ctrl+nope").is_err());
        assert_eq!(display("cmd+shift+space"), "Win+Shift+Space");
    }
}
