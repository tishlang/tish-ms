//! `windows.hotkeys`: system-wide hotkeys (RegisterHotKey on the app's window).
//!
//! - `register(spec, cb)` -> an id, or `{ error }` (taken by another app, or a bad spec);
//!   `cb()` on each press
//! - `unregister(id)`
//! - `check(spec)` -> null when it's free, or why not (registers and releases it)
//! - `display(spec)` -> `Ctrl+Alt+Space`

use std::cell::RefCell;
use std::collections::HashMap;

use tishlang_core::Value;
use tishlang_ms_common::keys;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::WindowsAndMessaging::WM_HOTKEY;

use super::{call, obj, s, str_arg, ui_hwnd};

thread_local! {
    static KEYS: RefCell<HashMap<i32, Value>> = RefCell::new(HashMap::new());
    static NEXT: std::cell::Cell<i32> = const { std::cell::Cell::new(1) };
}

fn try_register(id: i32, spec: &str) -> Result<(), String> {
    let hk = keys::parse(spec)?;
    let hwnd = ui_hwnd().ok_or("no window yet: register hotkeys after windows.run")?;
    unsafe { RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(hk.mods) | MOD_NOREPEAT, hk.vk) }
        .map_err(|_| format!("{} is in use by another app", keys::display(spec)))
}

pub(super) fn register(args: &[Value]) -> Value {
    let spec = str_arg(args, 0);
    let id = NEXT.with(|n| {
        let v = n.get();
        n.set(v + 1);
        v
    });
    match try_register(id, &spec) {
        Ok(()) => {
            KEYS.with(|k| k.borrow_mut().insert(id, args.get(1).cloned().unwrap_or(Value::Null)));
            Value::Number(id as f64)
        }
        Err(e) => obj(vec![("error", s(&e))]),
    }
}

pub(super) fn unregister(args: &[Value]) -> Value {
    let id = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as i32;
    KEYS.with(|k| k.borrow_mut().remove(&id));
    if let Some(h) = ui_hwnd() {
        unsafe {
            let _ = UnregisterHotKey(Some(h), id);
        }
    }
    Value::Null
}

pub(super) fn check(args: &[Value]) -> Value {
    const PROBE: i32 = 0xBFFF;
    match try_register(PROBE, &str_arg(args, 0)) {
        Ok(()) => {
            if let Some(h) = ui_hwnd() {
                unsafe {
                    let _ = UnregisterHotKey(Some(h), PROBE);
                }
            }
            Value::Null
        }
        Err(e) => s(&e),
    }
}

pub(super) fn handle(msg: u32, wp: WPARAM, _lp: LPARAM) -> Option<LRESULT> {
    if msg != WM_HOTKEY {
        return None;
    }
    if let Some(cb) = KEYS.with(|k| k.borrow().get(&(wp.0 as i32)).cloned()) {
        call(&cb, &[]);
    }
    Some(LRESULT(0))
}

pub(super) fn display(args: &[Value]) -> Value {
    s(&keys::display(&str_arg(args, 0)))
}
