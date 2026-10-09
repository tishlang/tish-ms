//! `windows.apps.installed()` -> `[{ name, path }]`: the Start menu's programs (shortcuts in the
//! all-users and per-user Start Menu\Programs folders), sorted by name, without uninstallers and
//! help/readme links. Opening `path` with `workspace.open` starts the app.

use std::path::{Path, PathBuf};

use tishlang_core::Value;

use super::{arr, obj, s};

fn start_menu_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = std::env::var_os("ProgramData") {
        dirs.push(Path::new(&p).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(p) = std::env::var_os("APPDATA") {
        dirs.push(Path::new(&p).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    dirs
}

/// Shortcuts that aren't apps to launch.
fn skipped(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["uninstall", "readme", "read me", "help", "documentation", "release notes", "license", "website"].iter().any(|w| n.contains(w))
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 4 {
                walk(&p, depth + 1, out);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk") || x.eq_ignore_ascii_case("url") || x.eq_ignore_ascii_case("appref-ms")) {
            let name = p.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            if !skipped(&name) {
                out.push((name, p.to_string_lossy().into_owned()));
            }
        }
    }
}

pub(crate) fn list() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for d in start_menu_dirs() {
        walk(&d, 0, &mut out);
    }
    out.sort_by_key(|a| a.0.to_lowercase());
    // The same app in both folders: keep the first.
    out.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    out
}

pub(super) fn installed(_a: &[Value]) -> Value {
    arr(list().into_iter().map(|(n, p)| obj(vec![("name", s(&n)), ("path", s(&p))])).collect())
}
