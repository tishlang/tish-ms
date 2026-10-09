//! Windows services that aren't UI, with the names and shapes of tish-macos's `macos.*`
//! services, so an app's platform layer changes little between the two:
//!
//! - `windows.pasteboard`: `readText()`, `writeText(text)`, `watch(cb)`
//! - `windows.workspace`: `open(target)`, `reveal(path)`, `trash(path)`
//! - `windows.shell.run(command, { cwd, args, timeoutMs }, cb)`
//! - `windows.credentials`: `get(account)`, `set(account, secret)`, `delete(account)`, `has(account)`
//! - `windows.hotkeys`: `register(spec, cb)`, `unregister(id)`, `check(spec)`, `display(spec)`
//! - `windows.statusItem(options)`: a notification-area (tray) icon with a menu
//! - `windows.apps`: `installed()` (Start menu), `running()`, `act(pid, action)`, `quitAll()`,
//!   `hideAll()`
//! - `windows.system`: lock, sleep, displays off, screen saver, restart, shut down, log out,
//!   Recycle Bin, dark mode, volume and mute, eject
//! - `windows.timeZones`: `names()`, `local()`, `at(id, unix)`, `byAbbreviation(abbr)`
//! - `windows.systemInfo()`, `windows.screens()`
//! - `windows.accessibility`: `trusted()`, `selectedText()`, `replaceBeforeCursor(typed, text)`,
//!   `focusedWindow()`, `setFocusedWindowFrame(x, y, w, h)`, `windowAction(pid, action)`
//! - `windows.icons`: `file(path)` (the shell's icon), `image(path)` (an image file),
//!   `symbol(name)` ("" for now), `onLoaded(cb)`: names to use as an `<image src>`
//!
//! Callbacks always run on the UI thread. Work done elsewhere (shell commands) comes back as a
//! posted message, like `whenSettled`. The services have their own hidden window, made on first
//! use, so hotkeys, the tray icon and the clipboard watcher work before `windows.run` too.

mod accessibility;
mod apps;
mod credentials;
mod hotkeys;
mod pasteboard;
mod shell;
mod sysinfo;
mod system;
mod timezones;
mod tray;
mod workspace;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::Arc;

use tishlang_core::{ObjectMap, Value};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Results from background threads: `lp` is a `Box<Delivery>`.
pub(super) const WM_DELIVER: u32 = WM_APP + 10;

/// The services' hidden window, which receives this module's messages.
static UI_HWND: AtomicIsize = AtomicIsize::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static PENDING: RefCell<HashMap<u64, Value>> = RefCell::new(HashMap::new());
}

unsafe extern "system" fn services_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match handle(hwnd, msg, wp, lp) {
        Some(r) => r,
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// The services' window: never shown, a tool window so it has no taskbar button. Not a
/// message-only window, because the tray icon needs the shell's broadcasts (TaskbarCreated).
/// Called on the UI thread; the first call makes it.
pub(super) fn ui_hwnd() -> Option<HWND> {
    match UI_HWND.load(Ordering::Relaxed) {
        0 => {}
        h => return Some(HWND(h as *mut _)),
    }
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = w!("TishWindowsServices");
        let wc = WNDCLASSW { lpfnWndProc: Some(services_proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, class, w!(""), WS_POPUP, 0, 0, 0, 0, None, None, Some(instance.into()), None).ok()?;
        UI_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        Some(hwnd)
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
    let _ = ui_hwnd();
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
    /// The value is an argument list (`[value, error]`), not a single argument.
    spread: bool,
}

/// From any thread: on the UI thread, `cb(make(data))` for the callback `id` holds. The window
/// exists already: [`hold`] made it on the UI thread.
pub(crate) fn deliver<T: Send + 'static>(id: u64, data: T, make: fn(T) -> Value) {
    let hwnd = match UI_HWND.load(Ordering::Relaxed) {
        0 => return,
        h => HWND(h as *mut _),
    };
    let d = Box::into_raw(Box::new(Delivery { id, run: Box::new(move || make(data)), spread: false }));
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

/// Carries a Tish promise to the waiting thread. Sound because a `Value::Promise` only exists
/// when the runtime is built with `http` or `promise`, which turn on `send-values`.
struct SendValue(Value);
unsafe impl Send for SendValue {}

/// `whenSettled(promise, cb)`: wait for the promise off the UI thread, then `cb(value, error)` on
/// it (`error` null when it fulfilled, `value` null when it rejected). Works before `run` too.
pub(super) fn when_settled(args: &[Value]) -> Value {
    let promise = SendValue(args.first().cloned().unwrap_or(Value::Null));
    let id = hold(args.get(1));
    std::thread::spawn(move || {
        let p = promise;
        let result = match &p.0 {
            Value::Promise(pr) => pr.block_until_settled(),
            other => Ok(other.clone()),
        };
        deliver_args(id, SendResult(result));
    });
    Value::Null
}

struct SendResult(Result<Value, Value>);
unsafe impl Send for SendResult {}

/// Like [`deliver`], calling `cb(value, error)`.
fn deliver_args(id: u64, r: SendResult) {
    let hwnd = match UI_HWND.load(Ordering::Relaxed) {
        0 => return,
        h => HWND(h as *mut _),
    };
    let run = move || {
        // The whole wrapper moves in (not just its field), so the closure is Send.
        let r = r;
        match r.0 {
            Ok(v) => arr(vec![v, Value::Null]),
            Err(e) => arr(vec![Value::Null, e]),
        }
    };
    let d = Box::into_raw(Box::new(Delivery { id, run: Box::new(run), spread: true }));
    unsafe {
        if PostMessageW(Some(hwnd), WM_DELIVER, WPARAM(0), LPARAM(d as isize)).is_err() {
            drop(Box::from_raw(d));
        }
    }
}

/// This module's window messages; `None` when `msg` isn't one of them.
pub(super) fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if msg == WM_DELIVER {
        let d = unsafe { Box::from_raw(lp.0 as *mut Delivery) };
        let spread = d.spread;
        let value = (d.run)();
        if let Some(cb) = PENDING.with(|p| p.borrow_mut().remove(&d.id)) {
            match (spread, &value) {
                (true, Value::Array(a)) => {
                    let args = a.borrow().clone();
                    call(&cb, &args);
                }
                _ => {
                    call(&cb, &[value]);
                }
            }
        }
        return Some(LRESULT(0));
    }
    pasteboard::handle(msg, wp, lp)
        .or_else(|| hotkeys::handle(msg, wp, lp))
        .or_else(|| tray::handle(hwnd, msg, wp, lp))
}

/// Image names the renderer loads (and caches) when an `<image src>` is drawn.
fn icon_file(args: &[Value]) -> Value {
    let p = str_arg(args, 0);
    if p.is_empty() { s("") } else { s(&format!("file:{p}")) }
}

fn icon_image(args: &[Value]) -> Value {
    let p = str_arg(args, 0);
    if p.is_empty() { s("") } else { s(&format!("image:{p}")) }
}

/// No symbol font mapping yet: rows show their placeholder.
fn icon_symbol(_a: &[Value]) -> Value {
    s("")
}

/// Icons load as they're first drawn, so there's never a later "loaded" moment.
fn icon_on_loaded(_a: &[Value]) -> Value {
    Value::Null
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
    w.insert(
        Arc::from("apps"),
        namespace(vec![
            ("installed", apps::installed),
            ("running", system::running),
            ("act", system::act),
            ("quitAll", system::quit_all),
            ("hideAll", system::hide_all),
        ]),
    );
    w.insert(
        Arc::from("system"),
        namespace(vec![
            ("lockScreen", system::lock),
            ("sleep", system::sleep),
            ("sleepDisplays", system::sleep_displays),
            ("screenSaver", system::screen_saver),
            ("restart", system::restart),
            ("shutDown", system::shut_down),
            ("logOut", system::log_out),
            ("emptyTrash", system::empty_trash),
            ("darkMode", system::dark_mode),
            ("setDarkMode", system::set_dark_mode),
            ("volume", system::volume),
            ("setVolume", system::set_volume),
            ("setMuted", system::set_muted),
            ("ejectAll", system::eject_all),
        ]),
    );
    w.insert(
        Arc::from("timeZones"),
        namespace(vec![("names", timezones::names), ("local", timezones::local), ("at", timezones::at), ("byAbbreviation", timezones::by_abbreviation)]),
    );
    w.insert(Arc::from("systemInfo"), Value::native(sysinfo::system_info));
    w.insert(
        Arc::from("accessibility"),
        namespace(vec![
            ("trusted", accessibility::trusted),
            ("selectedText", accessibility::selected_text),
            ("replaceBeforeCursor", accessibility::replace_before_cursor),
            ("focusedWindow", accessibility::focused_window),
            ("setFocusedWindowFrame", accessibility::set_focused_window_frame),
            ("windowAction", accessibility::window_action),
        ]),
    );
    w.insert(Arc::from("screens"), Value::native(accessibility::screens));
    w.insert(
        Arc::from("icons"),
        namespace(vec![("file", icon_file), ("image", icon_image), ("symbol", icon_symbol), ("onLoaded", icon_on_loaded)]),
    );
}
