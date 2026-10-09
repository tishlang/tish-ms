//! Canonical host tag names (legacy and PascalCase aliases map to one dispatch key). Matches
//! tish-apple's `canonical_host_tag`, so a UI written for tish-macos uses the same tags here.

pub fn canonical_host_tag(tag: &str) -> &str {
    match tag {
        "Window" | "macos_window" | "window" => "window",
        "ScrollView" | "scrollable" => "scrollable",
        "Text" | "text" | "p" | "span" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "title"
        | "heading" => "text",
        "Row" | "HStack" | "row" | "hstack" | "horizontal" => "row",
        "Column" | "column" | "VStack" | "vstack" | "div" => "column",
        "ZStack" | "zstack" => "zstack",
        "VisualEffect" | "visual_effect" => "visual_effect",
        "TextInput" | "textinput" | "text-input" | "input" => "textinput",
        "ProgressBar" | "progress_bar" | "progressbar" => "progress_bar",
        "Image" | "image" => "image",
        "Space" | "space" => "space",
        "Button" | "button" => "button",
        "Rule" | "rule" | "separator" | "divider" => "rule",
        "List" | "list" | "table" => "list",
        _ => tag,
    }
}
