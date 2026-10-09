//! `tish:windows`: a native Windows UI host for Tish JSX plus system services, the Windows
//! counterpart of tish-macos.
//!
//! ```tish
//! import { windows, useState } from 'tish:windows'
//! windows.run(App, { width: 750, height: 480 })
//! ```
//!
//! - `windows.run(App, opts)`: render `App` into a window and run the message loop. Options:
//!   `title`, `width`, `height`, `borderless`, `backdrop` (`"mica"`, `"acrylic"`, `"none"`),
//!   `autoShow` (default true), `autoRunEventLoop` (default true; false returns
//!   `{ show, hide, runEventLoop }`), `onKey(name)` (true: handled). A launcher panel adds
//!   `toolWindow` (no taskbar button), `topmost`, `hideOnBlur` and `onBlur()`.
//! - `windows.window`: `show()`, `hide()`, `toggle()`, `visible()`, `setSize(w, h)`.
//! - Services, shaped like tish-macos's: `pasteboard`, `workspace`, `shell`, `credentials`,
//!   `hotkeys`, `statusItem`, `apps` (see `win32/sys`).
//! - `windows.whenSettled(promise, cb)`: wait for a Tish promise off the UI thread, then
//!   `cb(value, error)` on it.
//! - `windows.startTimers()`: drive `setTimeout` / `setInterval` from the message loop (`run`
//!   already does).
//! - `useState`, `useMemo`, `useEffect`, `useRef`, `h`, `Fragment`.
//!
//! Hosts: Win32 (Direct2D and DirectWrite on a DWM-backdrop window) by default; with the `winui`
//! feature, `TISH_WINDOWS_HOST=winui` uses WinUI 3 instead. Off Windows every call is a no-op so
//! other platforms can type-check apps that import it.

use tishlang_core::Value;

#[cfg(windows)]
mod win32;
#[cfg(all(windows, feature = "winui"))]
mod winui;

/// The `windows` object `tish:windows` exports.
pub fn windows_object() -> Value {
    #[cfg(windows)]
    {
        win32::windows_object()
    }
    #[cfg(not(windows))]
    {
        stub_object()
    }
}

#[cfg(not(windows))]
fn stub_object() -> Value {
    use std::sync::Arc;
    use tishlang_core::ObjectMap;
    let noop = || Value::native(|_| Value::Null);
    let mut w = ObjectMap::default();
    for k in ["run", "whenSettled", "startTimers"] {
        w.insert(Arc::from(k), noop());
    }
    let mut m = ObjectMap::default();
    m.insert(Arc::from("windows"), Value::object(w));
    m.insert(Arc::from("useState"), Value::native(tishlang_ui::native_use_state));
    m.insert(Arc::from("useMemo"), Value::native(tishlang_ui::native_use_memo));
    m.insert(Arc::from("useEffect"), Value::native(tishlang_ui::native_use_effect));
    m.insert(Arc::from("useRef"), Value::native(tishlang_ui::native_use_ref));
    m.insert(Arc::from("h"), Value::native(tishlang_ui::ui_h));
    m.insert(Arc::from("Fragment"), tishlang_ui::fragment_value());
    Value::object(m)
}
