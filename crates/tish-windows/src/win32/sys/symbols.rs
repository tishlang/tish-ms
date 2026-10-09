//! SF Symbol names (what tish-macos apps pass to `icons.symbol`) to Segoe Fluent Icons glyphs, so
//! the same app shows a Windows icon for each. `glyph:<char>` draws in the icon font; `label:<text>`
//! (numbered circles) in the UI font. Unknown names have no glyph and keep their placeholder.

pub(crate) fn glyph(name: &str) -> Option<String> {
    if let Some(d) = name.strip_suffix(".circle").filter(|d| d.len() == 1 && d.as_bytes()[0].is_ascii_digit()) {
        return Some(format!("label:{d}"));
    }
    let code: u32 = match name {
        "keyboard" | "keyboard.badge.ellipsis" => 0xE765,
        "doc.on.clipboard" | "clipboard" => 0xE77F,
        "exclamationmark.triangle" => 0xE7BA,
        "exclamationmark.octagon" => 0xEA39,
        "xmark" | "xmark.app" | "xmark.circle" | "xmark.rectangle" => 0xE711,
        "eye" => 0xE890,
        "eye.slash" => 0xED1A,
        "arrow.uturn.backward" => 0xE7A7,
        "trash" => 0xE74D,
        "square.on.square" | "rectangle.on.rectangle" => 0xE8C8,
        "square.grid.2x2" | "square.grid.3x2" => 0xECA5,
        "folder" | "public.folder" | "folder.badge.gearshape" => 0xE8B7,
        "character.cursor.ibeam" | "textformat" | "text.insert" | "text.quote" => 0xE8D2,
        "arrow.up.forward.app" => 0xE8A7,
        "square.2.layers.3d" | "square.stack.3d.up" => 0xE81E,
        "macwindow" | "macwindow.on.rectangle" => 0xE7C4,
        "rectangle.portrait.and.arrow.right" => 0xF3B1,
        "arrow.up.left.and.arrow.down.right" | "rectangle.inset.filled" | "rectangle.center.inset.filled" => 0xE740,
        n if n.starts_with("rectangle.") => 0xE7C4,
        "plus.square" | "plus.rectangle" | "plus.circle" | "plus" => 0xE710,
        "person.badge.plus" => 0xE8FA,
        "minus.rectangle" => 0xE738,
        "pin" => 0xE718,
        "pin.slash" => 0xE77A,
        "person.crop.circle" | "person.crop.circle.fill" | "person.crop.square" | "person" => 0xE77B,
        "info.circle" => 0xE946,
        "display" | "display.2" => 0xE7F4,
        "command" | "command.square" | "terminal" => 0xE756,
        "speaker.wave.1.fill" | "speaker.wave.2.fill" | "speaker.wave.3.fill" => 0xE767,
        "speaker.slash.fill" => 0xE74F,
        "sparkles" | "sparkles.tv" | "hypery.ai" | "bolt" => 0xE945,
        "restart" | "arrow.clockwise" => 0xE72C,
        "clock.arrow.circlepath" => 0xE81C,
        "clock" => 0xE823,
        "power" => 0xE7E8,
        "moon.zzz.fill" | "moon" => 0xE708,
        "lock" | "lock.fill" => 0xE72E,
        "globe" => 0xE774,
        "gear" | "gearshape" | "gearshape.2" => 0xE713,
        "equal.circle.fill" => 0xE8EF,
        "eject.fill" => 0xE7E7,
        "magnifyingglass" | "doc.text.magnifyingglass" => 0xE721,
        "doc" => 0xE8A5,
        "circle.lefthalf.filled" => 0xE706,
        "checkmark.circle" | "checkmark" => 0xE73E,
        "character.book.closed.fill" | "book" => 0xE82D,
        "building.2" => 0xE80F,
        "arrow.up.and.down" => 0xE8CB,
        "arrow.right" => 0xE72A,
        "arrow.left" => 0xE72B,
        "calendar" => 0xE787,
        "star" => 0xE734,
        "photo" => 0xEB9F,
        "music.note" => 0xEC4F,
        "film" => 0xE714,
        "paperplane" => 0xE724,
        "bubble.left" => 0xE8BD,
        _ => return None,
    };
    char::from_u32(code).map(|c| format!("glyph:{c}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn maps_names() {
        assert_eq!(super::glyph("trash").as_deref(), Some("glyph:\u{E74D}"));
        assert_eq!(super::glyph("3.circle").as_deref(), Some("label:3"));
        assert_eq!(super::glyph("rectangle.lefthalf.filled").as_deref(), Some("glyph:\u{E7C4}"));
        assert_eq!(super::glyph("no.such.symbol"), None);
    }
}
