//! Layout with tish-macos's rules, so one Tish UI lands in the same places on Windows:
//!
//! - `column` (and `div`): children stacked top to bottom at the inner width, `gap` apart.
//! - `row`: widths from `columnWidths` (numbers fixed, `null` / `0` / `"flex"` share the rest by
//!   `weights`), else `weights`, else equal; children top-aligned unless `alignItems` says
//!   `center` or `end`.
//! - `padding` / `paddingLeft|Right|Top|Bottom` inset the content; `height` is a minimum.
//! - Leaves: `text` and `button` measure their text; `image` is `height` square (16 by default);
//!   `textinput` fits its font; `rule` is 1 px; `space` and `progress_bar` take `height`.
//!
//! Text size comes from the host through [`Measure`], so this module needs no graphics API.

use tishlang_core::{PropMap, Value};

use crate::style::{props_f64, props_string};
use crate::tree::{Node, Rect};

/// Font of a text-bearing node.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub size: f64,
    /// 100..900.
    pub weight: u16,
}

pub fn font(props: &PropMap) -> Font {
    let weight = match props.get("fontWeight") {
        Some(Value::Number(n)) => *n as u16,
        Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "thin" => 100,
            "light" => 300,
            "medium" => 500,
            "semibold" => 600,
            "bold" => 700,
            "heavy" | "black" => 900,
            _ => 400,
        },
        _ => 400,
    };
    Font { size: props_f64(props, &["fontSize"], 13.0), weight }
}

/// Text measurement, from the host's text engine.
pub trait Measure {
    /// Width and height of `text` in `font`, wrapped at `max_w` when `wrap`.
    fn text(&self, text: &str, font: &Font, max_w: f64, wrap: bool) -> (f64, f64);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

pub fn cross_align(props: &PropMap) -> Align {
    match props_string(props, &["alignItems", "align_items", "verticalAlign"]).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("center" | "middle") => Align::Center,
        Some("end" | "flex-end" | "bottom") => Align::End,
        _ => Align::Start,
    }
}

/// (top, right, bottom, left).
pub fn padding(props: &PropMap) -> (f64, f64, f64, f64) {
    let all = props_f64(props, &["padding"], 0.0);
    (
        props_f64(props, &["paddingTop"], all),
        props_f64(props, &["paddingRight"], all),
        props_f64(props, &["paddingBottom"], all),
        props_f64(props, &["paddingLeft"], all),
    )
}

/// tish-macos's `row_child_widths`.
pub fn row_child_widths(iw: f64, n: usize, props: &PropMap) -> Vec<f64> {
    if n == 0 {
        return vec![];
    }
    let iw = iw.max(0.0);
    let weights = |n: usize| -> Option<Vec<f64>> {
        match props.get("weights") {
            Some(Value::Array(wa)) => {
                let ws: Vec<f64> = wa.borrow().iter().filter_map(|v| v.as_number()).collect();
                (ws.len() == n).then_some(ws)
            }
            _ => None,
        }
    };
    if let Some(Value::Array(cw)) = props.get("columnWidths") {
        let spec = cw.borrow();
        if spec.len() == n {
            let w_src = weights(n).unwrap_or_else(|| vec![1.0; n]);
            let mut out = vec![0.0; n];
            let mut fixed = vec![false; n];
            let mut flex: Vec<(usize, f64)> = Vec::new();
            for i in 0..n {
                let px = spec[i].as_number().filter(|p| *p > 0.0);
                match px {
                    Some(p) => {
                        out[i] = p;
                        fixed[i] = true;
                    }
                    None => flex.push((i, w_src[i].max(0.0))),
                }
            }
            let mut fixed_sum: f64 = out.iter().sum();
            if fixed_sum > iw && fixed_sum > 1e-9 {
                let scale = iw / fixed_sum;
                for j in 0..n {
                    if fixed[j] {
                        out[j] *= scale;
                    }
                }
                fixed_sum = out.iter().sum();
            }
            let rem = (iw - fixed_sum).max(0.0);
            let wtot: f64 = flex.iter().map(|f| f.1).sum();
            for &(i, w) in &flex {
                out[i] = if wtot > 1e-9 { rem * w / wtot } else { rem / flex.len() as f64 };
            }
            return out;
        }
    }
    if let Some(ws) = weights(n) {
        let sum: f64 = ws.iter().sum();
        if sum > 1e-9 && ws.iter().all(|w| *w >= 0.0) {
            return ws.iter().map(|w| iw * w / sum).collect();
        }
    }
    vec![iw / n as f64; n]
}

/// Lay out `node` at `(x, y)`, `w` wide; sets every frame and returns the node's height.
pub fn layout(node: &mut Node, x: f64, y: f64, w: f64, m: &dyn Measure) -> f64 {
    let (pt, pr, pb, pl) = padding(&node.props);
    let min_h = props_f64(&node.props, &["height", "h"], -1.0);
    let ix = x + pl;
    let iw = (w - pl - pr).max(0.0);
    let content_h = match node.tag.as_str() {
        "column" | "scrollable" | "window" | "visual_effect" | "list" => {
            let gap = props_f64(&node.props, &["gap", "spacing"], 0.0);
            let mut cy = y + pt;
            let n = node.children.len();
            for (i, c) in node.children.iter_mut().enumerate() {
                let cw = props_f64(&c.props, &["width", "w"], iw).min(iw);
                cy += layout(c, ix, cy, cw, m);
                if i + 1 < n {
                    cy += gap;
                }
            }
            cy - (y + pt)
        }
        "row" => {
            let widths = row_child_widths(iw, node.children.len(), &node.props);
            let mut cx = ix;
            let mut heights = Vec::with_capacity(widths.len());
            for (c, cw) in node.children.iter_mut().zip(&widths) {
                heights.push(layout(c, cx, y + pt, *cw, m));
                cx += cw;
            }
            let tallest = heights.iter().cloned().fold(0.0, f64::max);
            let inner = tallest.max(min_h - pt - pb);
            let align = cross_align(&node.props);
            if align != Align::Start {
                for (c, h) in node.children.iter_mut().zip(&heights) {
                    let dy = match align {
                        Align::Center => ((inner - h) * 0.5).max(0.0),
                        _ => (inner - h).max(0.0),
                    };
                    shift(c, 0.0, dy.round());
                }
            }
            inner
        }
        "zstack" => {
            let mut tallest: f64 = 0.0;
            for c in node.children.iter_mut() {
                tallest = tallest.max(layout(c, ix, y + pt, iw, m));
            }
            tallest
        }
        "text" | "button" => {
            let wrap = !matches!(node.props.get("wrap"), Some(Value::Bool(false)));
            let (_, th) = m.text(&node.text, &font(&node.props), iw, wrap);
            th
        }
        "textinput" => (font(&node.props).size * 1.25).ceil() + 6.0,
        "image" => props_f64(&node.props, &["height", "h"], 16.0),
        "rule" => props_f64(&node.props, &["height", "h"], 1.0),
        _ => props_f64(&node.props, &["height", "h"], 0.0),
    };
    let h = (pt + content_h + pb).max(min_h);
    node.frame = Rect { x, y, w, h };
    h
}

fn shift(node: &mut Node, dx: f64, dy: f64) {
    node.frame.x += dx;
    node.frame.y += dy;
    for c in node.children.iter_mut() {
        shift(c, dx, dy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tishlang_core::{ObjectMap, Value};

    /// 7 px per character, line height 1.25 × size.
    struct Fixed;
    impl Measure for Fixed {
        fn text(&self, text: &str, font: &Font, _max_w: f64, _wrap: bool) -> (f64, f64) {
            (text.chars().count() as f64 * 7.0, (font.size * 1.25).ceil())
        }
    }

    fn el(tag: &str, props: Vec<(&str, Value)>, children: Vec<Value>) -> Value {
        let mut p = ObjectMap::default();
        for (k, v) in props {
            p.insert(Arc::from(k), v);
        }
        let mut m = ObjectMap::default();
        m.insert(Arc::from("tag"), Value::String(tag.into()));
        m.insert(Arc::from("props"), Value::object(p));
        m.insert(Arc::from("children"), Value::array(children));
        Value::object(m)
    }

    fn n(v: f64) -> Value {
        Value::Number(v)
    }

    #[test]
    fn row_widths_fixed_and_flex() {
        let v = el("row", vec![("columnWidths", Value::array(vec![n(20.0), Value::String("flex".into()), n(40.0)]))], vec![
            el("image", vec![], vec![]),
            el("text", vec![], vec![Value::String("hi".into())]),
            el("space", vec![], vec![]),
        ]);
        let mut root = crate::tree::from_vnode(&v).remove(0);
        layout(&mut root, 0.0, 0.0, 300.0, &Fixed);
        let w: Vec<f64> = root.children.iter().map(|c| c.frame.w).collect();
        assert_eq!(w, vec![20.0, 240.0, 40.0]);
        assert_eq!(root.children[1].frame.x, 20.0);
    }

    #[test]
    fn column_stacks_with_padding_and_min_height() {
        let v = el("div", vec![("paddingLeft", n(8.0)), ("paddingTop", n(4.0))], vec![
            el("text", vec![("fontSize", n(16.0))], vec![Value::String("a".into())]),
            el("row", vec![("height", n(30.0))], vec![]),
        ]);
        let mut root = crate::tree::from_vnode(&v).remove(0);
        let h = layout(&mut root, 0.0, 0.0, 100.0, &Fixed);
        assert_eq!(root.tag, "column");
        assert_eq!(root.children[0].frame, Rect { x: 8.0, y: 4.0, w: 92.0, h: 20.0 });
        assert_eq!(root.children[1].frame.y, 24.0);
        assert_eq!(h, 54.0);
    }

    #[test]
    fn row_centers_children() {
        let v = el("row", vec![("height", n(40.0)), ("alignItems", Value::String("center".into()))], vec![el("image", vec![("height", n(20.0))], vec![])]);
        let mut root = crate::tree::from_vnode(&v).remove(0);
        layout(&mut root, 0.0, 0.0, 100.0, &Fixed);
        assert_eq!(root.children[0].frame.y, 10.0);
    }

    #[test]
    fn fragments_flatten_and_text_joins() {
        let frag = el(tishlang_ui::FRAGMENT_SENTINEL, vec![], vec![el("text", vec![], vec![Value::String("n=".into()), n(3.0)]), Value::String("x".into())]);
        let nodes = crate::tree::from_vnode(&frag);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].text, "n=3");
        assert_eq!(nodes[1].tag, "text");
    }
}
