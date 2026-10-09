//! `windows.pasteboard`: the clipboard's text.
//!
//! - `readText()` -> its text, or ""
//! - `writeText(text)` -> whether it was written
//! - `watch(cb)`: `cb({ text, app, appPath })` for each text copied from now on, `app` being the
//!   foreground app's name (where the copy happened) or "" for this app's own `writeText`.
//!   Clipboard updates arrive as WM_CLIPBOARDUPDATE. Copies that password managers mark with
//!   `ExcludeClipboardContentFromMonitorProcessing` or `CanIncludeInClipboardHistory = 0` are
//!   skipped. Returns false if already watching.

use std::cell::{Cell, RefCell};

use tishlang_core::Value;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, MAX_PATH, WPARAM};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, WM_CLIPBOARDUPDATE};

use super::{call, obj, s, str_arg, ui_hwnd};

thread_local! {
    static WATCHER: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// Set while this app writes, so its own copies report `app: ""`.
    static OWN_WRITE: Cell<bool> = const { Cell::new(false) };
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

/// Open the clipboard, retrying briefly: another app may hold it for a moment.
fn open() -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(ui_hwnd()) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

fn read() -> String {
    if !open() {
        return String::new();
    }
    let mut out = String::new();
    unsafe {
        if let Ok(h) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                out = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                let _ = GlobalUnlock(g);
            }
        }
        let _ = CloseClipboard();
    }
    out
}

fn has_format(name: windows::core::PCWSTR) -> bool {
    unsafe {
        let f = RegisterClipboardFormatW(name);
        f != 0 && IsClipboardFormatAvailable(f).is_ok()
    }
}

pub(super) fn read_text(_a: &[Value]) -> Value {
    s(&read())
}

pub(super) fn write_text(args: &[Value]) -> Value {
    let text = str_arg(args, 0);
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    if !open() {
        return Value::Bool(false);
    }
    let ok = unsafe {
        let _ = EmptyClipboard();
        match GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            Ok(g) => {
                let p = GlobalLock(g) as *mut u16;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                    let _ = GlobalUnlock(g);
                }
                OWN_WRITE.with(|o| o.set(true));
                SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(g.0))).is_ok()
            }
            Err(_) => false,
        }
    };
    unsafe {
        let _ = CloseClipboard();
    }
    Value::Bool(ok)
}

pub(super) fn window_ready(hwnd: HWND) {
    if WATCHER.with(|w| w.borrow().is_some()) && !LISTENING.with(|l| l.get()) {
        unsafe {
            let _ = AddClipboardFormatListener(hwnd);
        }
        LISTENING.with(|l| l.set(true));
    }
}

pub(super) fn watch(args: &[Value]) -> Value {
    if WATCHER.with(|w| w.borrow().is_some()) {
        return Value::Bool(false);
    }
    WATCHER.with(|w| *w.borrow_mut() = args.first().cloned());
    if let Some(h) = ui_hwnd() {
        window_ready(h);
    }
    Value::Bool(true)
}

/// The foreground window's app: (file name without extension, full path).
fn foreground_app() -> (String, String) {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return Default::default() };
        let mut buf = [0u16; MAX_PATH as usize * 2];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return Default::default();
        }
        let path = String::from_utf16_lossy(&buf[..n as usize]);
        let name = std::path::Path::new(&path).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        (name, path)
    }
}

pub(super) fn handle(msg: u32, _wp: WPARAM, _lp: LPARAM) -> Option<LRESULT> {
    if msg != WM_CLIPBOARDUPDATE {
        return None;
    }
    let own = OWN_WRITE.with(|o| o.replace(false));
    let Some(cb) = WATCHER.with(|w| w.borrow().clone()) else { return Some(LRESULT(0)) };
    if has_format(w!("ExcludeClipboardContentFromMonitorProcessing")) || has_format(w!("Clipboard Viewer Ignore")) {
        return Some(LRESULT(0));
    }
    let text = read();
    if text.is_empty() {
        return Some(LRESULT(0));
    }
    let (app, path) = if own { (String::new(), String::new()) } else { foreground_app() };
    call(&cb, &[obj(vec![("text", s(&text)), ("app", s(&app)), ("appPath", s(&path))])]);
    Some(LRESULT(0))
}
