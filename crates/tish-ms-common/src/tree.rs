//! The retained node tree a host draws: committed vnodes, with fragments flattened, tags made
//! canonical, and a text node's string children joined into its `text`.

use tishlang_core::{PropMap, Value};

use crate::tag::canonical_host_tag;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    /// Canonical tag (`row`, `column`, `text`, …).
    pub tag: String,
    pub props: PropMap,
    /// Joined string and number children (text, button, any leaf).
    pub text: String,
    pub children: Vec<Node>,
    /// Set by [`crate::layout::layout`], in window coordinates (top-left origin, y down).
    pub frame: Rect,
}

impl Node {
    pub fn handler(&self, name: &str) -> Option<Value> {
        match self.props.get(name) {
            Some(f @ Value::Function(_)) => Some(f.clone()),
            _ => None,
        }
    }

    /// Depth-first, deepest first: the node under `(x, y)` that has `handler`.
    pub fn hit(&self, x: f64, y: f64, handler: &str) -> Option<&Node> {
        if !self.frame.contains(x, y) {
            return None;
        }
        for c in self.children.iter().rev() {
            if let Some(n) = c.hit(x, y, handler) {
                return Some(n);
            }
        }
        self.handler(handler).map(|_| self)
    }

    /// Every node, depth first (parents before children).
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Node>) {
        out.push(self);
        for c in &self.children {
            c.walk(out);
        }
    }
}

fn is_fragment(tag: Option<&Value>) -> bool {
    matches!(tag, Some(Value::String(s)) if s.as_str() == tishlang_ui::FRAGMENT_SENTINEL)
}

/// Nodes for one committed vnode (a fragment gives several; null or a bare string gives text).
pub fn from_vnode(v: &Value) -> Vec<Node> {
    let mut out = Vec::new();
    push_vnode(v, &mut out);
    out
}

fn push_vnode(v: &Value, out: &mut Vec<Node>) {
    match v {
        Value::Null | Value::Bool(_) => {}
        Value::String(_) | Value::Number(_) => out.push(Node { tag: "text".into(), text: v.to_display_string(), ..Default::default() }),
        Value::Array(a) => {
            for c in a.borrow().iter() {
                push_vnode(c, out);
            }
        }
        Value::Object(o) => {
            let o = o.borrow();
            let tag = o.strings.get("tag");
            let children: Vec<Value> = match o.strings.get("children") {
                Some(Value::Array(a)) => a.borrow().clone(),
                _ => Vec::new(),
            };
            if is_fragment(tag) {
                for c in &children {
                    push_vnode(c, out);
                }
                return;
            }
            let Some(Value::String(t)) = tag else { return };
            let props = match o.strings.get("props") {
                Some(Value::Object(p)) => p.borrow().strings.clone(),
                _ => PropMap::default(),
            };
            let mut node = Node { tag: canonical_host_tag(t.as_str()).to_string(), props, ..Default::default() };
            for c in &children {
                match c {
                    Value::String(_) | Value::Number(_) => node.text.push_str(&c.to_display_string()),
                    _ => push_vnode(c, &mut node.children),
                }
            }
            out.push(node);
        }
        _ => {}
    }
}
