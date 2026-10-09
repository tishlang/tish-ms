//! Windows services that aren't UI, with the names and shapes of tish-macos's `macos.*`
//! services, so an app's platform layer changes little between the two:
//!
//! - `windows.pasteboard`: `readText()`, `writeText(text)`, `watch(cb)`
//! - `windows.workspace`: `open(target)`, `reveal(path)`, `trash(path)`
//! - `windows.shell.run(command, { cwd, args, timeoutMs }, cb)`
//! - `windows.credentials`: `get(account)`, `set(account, secret)`, `delete(account)`, `has(account)`
//! - `windows.hotkeys`: `register(spec, cb)`, `unregister(id)`, `check(spec)`, `display(spec)`
//! - `windows.statusItem(options)`: a notification-area (tray) icon with a menu
//! - `windows.apps.installed()`: Start menu shortcuts
//!
//! Callbacks always run on the UI thread. Work done elsewhere (shell commands) comes back as a
//! posted message, like `whenSettled`.

mod apps;
mod credentials;
mod hotkeys;
mod pasteboard;
mod shell;
mod tray;
mod workspace;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::Arc;

use tishlang_core::{ObjectMap, Value};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Results from background threads: `lp` is a `Box<Delivery>`.
pub(super) const WM_DELIVER: u32 = WM_APP + 10;

/// The window that receives this module's messages (the app's window).
static UI_HWND: AtomicIsize = AtomicIsize::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static PENDING: RefCell<HashMap<u64, Value>> = RefCell::new(HashMap::new());
}

pub(super) fn set_window(hwnd: HWND) {
    UI_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    pasteboard::window_ready(hwnd);
}

pub(super) fn ui_hwnd() -> Option<HWND> {
    match UI_HWND.load(Ordering::Relaxed) {
        0 => None,
        h => Some(HWND(h as *mut _)),
    }
}

pub(crate) fn s(v: &str) -> Value {
    Value::String(v.into())
}

pub(crate) fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = ObjectMap::default();
    for (k, v) in pairs {
        m.insert(Arc::from(k), v);
    }
    Value::object(m)
}

pub(crate) fn arr(items: Vec<Value>) -> Value {
    Value::Array(tishlang_core::VmRef::new(items))
}

pub(crate) fn str_arg(args: &[Value], i: usize) -> String {
    match args.get(i) {
        Some(Value::String(x)) => x.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn field(o: Option<&Value>, key: &str) -> Option<Value> {
    match o {
        Some(Value::Object(m)) => m.borrow().strings.get(key).cloned(),
        _ => None,
    }
}

pub(crate) fn call(cb: &Value, args: &[Value]) -> Value {
    match cb {
        Value::Function(f) => f.call(args),
        _ => Value::Null,
    }
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Keep `cb` (UI thread) until [`deliver`] answers it; returns its id.
pub(crate) fn hold(cb: Option<&Value>) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if let Some(cb) = cb {
        PENDING.with(|p| p.borrow_mut().insert(id, cb.clone()));
    }
    id
}

/// A result on its way to the UI thread: plain data plus how to make it a `Value` there.
pub(super) struct Delivery {
    id: u64,
    run: Box<dyn FnOnce() -> Value + Send>,
}

/// From any thread: on the UI thread, `cb(make(data))` for the callback `id` holds.
pub(crate) fn deliver<T: Send + 'static>(id: u64, data: T, make: fn(T) -> Value) {
    let Some(hwnd) = ui_hwnd() else { return };
    let d = Box::into_raw(Box::new(Delivery { id, run: Box::new(move || make(data)) }));
    unsafe {
        if PostMessageW(Some(hwnd), WM_DELIVER, WPARAM(0), LPARAM(d as isize)).is_err() {
            drop(Box::from_raw(d));
        }
    }
}

/// Run `work` on a background thread, then `cb(make(result))` on the UI thread.
pub(crate) fn in_background<T: Send + 'static>(cb: Option<&Value>, work: impl FnOnce() -> T + Send + 'static, make: fn(T) -> Value) -> u64 {
    let id = hold(cb);
    std::thread::spawn(move || deliver(id, work(), make));
    id
}

/// This module's window messages; `None` when `msg` isn't one of them.
pub(super) fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if msg == WM_DELIVER {
        let d = unsafe { Box::from_raw(lp.0 as *mut Delivery) };
        let value = (d.run)();
        if let Some(cb) = PENDING.with(|p| p.borrow_mut().remove(&d.id)) {
            call(&cb, &[value]);
        }
        return Some(LRESULT(0));
    }
    pasteboard::handle(msg, wp, lp)
        .or_else(|| hotkeys::handle(msg, wp, lp))
        .or_else(|| tray::handle(hwnd, msg, wp, lp))
}

pub(super) fn remove_tray() {
    tray::remove();
}

type Native = fn(&[Value]) -> Value;

fn namespace(fns: Vec<(&str, Native)>) -> Value {
    let mut m = ObjectMap::default();
    for (k, f) in fns {
        m.insert(Arc::from(k), Value::native(f));
    }
    Value::object(m)
}

/// Add the namespaces to the `windows` object.
pub(super) fn install(w: &mut ObjectMap) {
    w.insert(Arc::from("pasteboard"), namespace(vec![("readText", pasteboard::read_text), ("writeText", pasteboard::write_text), ("watch", pasteboard::watch)]));
    w.insert(Arc::from("workspace"), namespace(vec![("open", workspace::open), ("reveal", workspace::reveal), ("trash", workspace::trash)]));
    w.insert(Arc::from("shell"), namespace(vec![("run", shell::run)]));
    w.insert(
        Arc::from("credentials"),
        namespace(vec![("get", credentials::get), ("set", credentials::set), ("delete", credentials::delete), ("has", credentials::has)]),
    );
    w.insert(Arc::from("hotkeys"), namespace(vec![("register", hotkeys::register), ("unregister", hotkeys::unregister), ("check", hotkeys::check), ("display", hotkeys::display)]));
    w.insert(Arc::from("statusItem"), Value::native(tray::status_item));
    w.insert(Arc::from("apps"), namespace(vec![("installed", apps::installed)]));
}
