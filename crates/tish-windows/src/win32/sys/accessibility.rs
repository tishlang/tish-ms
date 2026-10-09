//! `windows.accessibility` and `windows.screens()`, tish-macos's shapes. Windows needs no
//! permission for any of this, so `trusted()` is always true.
//!
//! - `selectedText()` -> the selection in the foreground app: UI Automation's text pattern, else a
//!   Ctrl+C with the clipboard restored after; null when there's none
//! - `replaceBeforeCursor(typed, text)`: delete `typed` (just typed in the foreground app) with
//!   Backspace and type `text` -> null, or why not
//! - `focusedWindow()` -> the foreground window `{ pid, app, x, y, w, h, key }` (its visible
//!   frame, physical pixels) or `{ error }`
//! - `setFocusedWindowFrame(x, y, w, h)` -> null, or why the window didn't move (restored first
//!   when maximized; the frame is the visible one, invisible resize borders accounted for)
//! - `windowAction(pid, action)`: minimize, unminimize, fullscreen, close or raise (window.rs)
//! - `windows.screens()` -> `[{ visible, frame }]`, each `{ x, y, w, h }`: every display's work
//!   area (taskbar left out) and whole area, primary first, in physical pixels

use tishlang_core::Value;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{arr, obj, s, str_arg};

fn rect(x: i32, y: i32, w: i32, h: i32) -> Value {
    obj(vec![("x", Value::Number(x as f64)), ("y", Value::Number(y as f64)), ("w", Value::Number(w as f64)), ("h", Value::Number(h as f64))])
}

pub(super) fn trusted(_a: &[Value]) -> Value {
    Value::Bool(true)
}

/// The window's visible bounds (without the invisible resize borders Windows 10+ adds).
fn visible_frame(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).is_err() {
            let _ = GetWindowRect(h, &mut r);
        }
    }
    r
}

fn foreground() -> Result<(HWND, u32, String), String> {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            return Err("no window in front".into());
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == std::process::id() {
            return Err("no window to arrange".into());
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(h, &mut buf);
        Ok((h, pid, String::from_utf16_lossy(&buf[..n as usize])))
    }
}

pub(super) fn focused_window(_a: &[Value]) -> Value {
    match foreground() {
        Ok((h, pid, title)) => {
            let r = visible_frame(h);
            obj(vec![
                ("pid", Value::Number(pid as f64)),
                ("app", s(&title)),
                ("x", Value::Number(r.left as f64)),
                ("y", Value::Number(r.top as f64)),
                ("w", Value::Number((r.right - r.left) as f64)),
                ("h", Value::Number((r.bottom - r.top) as f64)),
                ("key", Value::Number(h.0 as usize as f64)),
            ])
        }
        Err(e) => obj(vec![("error", s(&e))]),
    }
}

pub(super) fn set_focused_window_frame(args: &[Value]) -> Value {
    let n = |i: usize| args.get(i).and_then(|v| v.as_number()).unwrap_or(0.0).round() as i32;
    let (x, y, w, h) = (n(0), n(1), n(2), n(3));
    let r = (|| -> Result<(), String> {
        let (hwnd, _, title) = foreground()?;
        unsafe {
            if IsZoomed(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            // SetWindowPos takes the outer rect: add the invisible borders back.
            let mut outer = RECT::default();
            let _ = GetWindowRect(hwnd, &mut outer);
            let vis = visible_frame(hwnd);
            let (l, t) = (vis.left - outer.left, vis.top - outer.top);
            let (rr, bb) = (outer.right - vis.right, outer.bottom - vis.bottom);
            SetWindowPos(hwnd, None, x - l, y - t, w + l + rr, h + t + bb, SWP_NOZORDER | SWP_NOACTIVATE)
                .map_err(|_| format!("{title} didn't let its window move"))
        }
    })();
    match r {
        Ok(()) => Value::Null,
        Err(e) => s(&e),
    }
}

pub(super) fn screens(_a: &[Value]) -> Value {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> windows::core::BOOL {
        let out = &mut *(lp.0 as *mut Vec<(bool, RECT, RECT)>);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(m, &mut mi).as_bool() {
            out.push((mi.dwFlags & MONITORINFOF_PRIMARY != 0, mi.rcWork, mi.rcMonitor));
        }
        TRUE
    }
    let mut list: Vec<(bool, RECT, RECT)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list.sort_by_key(|m| !m.0);
    let r = |r: RECT| rect(r.left, r.top, r.right - r.left, r.bottom - r.top);
    arr(list.into_iter().map(|(_, work, mon)| obj(vec![("visible", r(work)), ("frame", r(mon))])).collect())
}

// ── Selected text ───────────────────────────────────────────────────────────

fn uia_selection() -> Option<String> {
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let el = uia.GetFocusedElement().ok()?;
        let pattern: IUIAutomationTextPattern = el.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let ranges = pattern.GetSelection().ok()?;
        let mut out = String::new();
        for i in 0..ranges.Length().ok()? {
            out.push_str(&ranges.GetElement(i).ok()?.GetText(-1).ok()?.to_string());
        }
        (!out.is_empty()).then_some(out)
    }
}

/// Tags the keystrokes this module synthesizes (`dwExtraInfo`), so a keyboard hook in the same app
/// (Moo's snippet watcher) can skip its own typing and still see other tools'. "MOO\0".
pub(crate) const SYNTHETIC_KEY_TAG: usize = 0x004F_4F4D;

fn press(keys: &[(VIRTUAL_KEY, bool)]) {
    let inputs: Vec<INPUT> = keys
        .iter()
        .map(|(vk, up)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: *vk, dwFlags: if *up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, dwExtraInfo: SYNTHETIC_KEY_TAG, ..Default::default() } },
        })
        .collect();
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Ctrl+C in the foreground app, reading the copy and putting the clipboard back.
fn copy_selection() -> Option<String> {
    let before = super::pasteboard::read_raw();
    let seq = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
    press(&[(VK_CONTROL, false), (VK_C, false), (VK_C, true), (VK_CONTROL, true)]);
    let mut got = None;
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() } != seq {
            got = Some(super::pasteboard::read_raw());
            break;
        }
    }
    if got.is_some() {
        super::pasteboard::write_raw(&before);
    }
    got.filter(|t| !t.is_empty())
}

pub(super) fn selected_text(_a: &[Value]) -> Value {
    uia_selection().or_else(copy_selection).map_or(Value::Null, |t| s(&t))
}

/// Type `text` as Unicode keystrokes (any character, whatever the keyboard layout).
fn type_text(text: &str) {
    let mut inputs = Vec::new();
    for u in text.encode_utf16() {
        for up in [false, true] {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wScan: u, dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, dwExtraInfo: SYNTHETIC_KEY_TAG, ..Default::default() } },
            });
        }
    }
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

pub(super) fn replace_before_cursor(args: &[Value]) -> Value {
    let typed = str_arg(args, 0);
    let text = str_arg(args, 1).replace('\n', "\r");
    let back: Vec<(VIRTUAL_KEY, bool)> = typed.chars().flat_map(|_| [(VK_BACK, false), (VK_BACK, true)]).collect();
    press(&back);
    type_text(&text);
    Value::Null
}

fn pid_windows(pid: u32) -> Vec<HWND> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> windows::core::BOOL {
        let (pid, out) = &mut *(lp.0 as *mut (u32, Vec<HWND>));
        let mut p = 0u32;
        GetWindowThreadProcessId(h, Some(&mut p));
        if p == *pid && IsWindowVisible(h).as_bool() && GetWindow(h, GW_OWNER).map_or(true, |o| o.is_invalid()) && GetWindowTextLengthW(h) > 0 {
            out.push(h);
        }
        TRUE
    }
    let mut st: (u32, Vec<HWND>) = (pid, Vec::new());
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut st as *mut _ as isize));
    }
    st.1
}

/// `windowAction(pid, action)` -> `{ count }` or `{ error }` (the error reads after the app's
/// name): "minimize", "fullscreen" (maximize, toggled), "close" or "raise" on the app's front
/// window; "unminimize" restores all its minimized windows.
pub(super) fn window_action(args: &[Value]) -> Value {
    let pid = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    let action = str_arg(args, 1);
    let wins = pid_windows(pid);
    let r: Result<usize, String> = unsafe {
        if action == "unminimize" {
            let mins: Vec<HWND> = wins.iter().copied().filter(|w| IsIconic(*w).as_bool()).collect();
            for w in &mins {
                let _ = ShowWindow(*w, SW_RESTORE);
            }
            if mins.is_empty() { Err("has no minimized windows".into()) } else { Ok(mins.len()) }
        } else if let Some(&w) = wins.first() {
            match action.as_str() {
                "minimize" => {
                    let _ = ShowWindow(w, SW_MINIMIZE);
                    Ok(1)
                }
                "fullscreen" => {
                    let _ = ShowWindow(w, if IsZoomed(w).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                    Ok(1)
                }
                "close" => PostMessageW(Some(w), WM_CLOSE, WPARAM(0), LPARAM(0)).map(|_| 1).map_err(|_| "would not close".into()),
                "raise" => {
                    if IsIconic(w).as_bool() {
                        let _ = ShowWindow(w, SW_RESTORE);
                    }
                    let _ = SetForegroundWindow(w);
                    Ok(1)
                }
                other => Err(format!("unknown window action `{other}`")),
            }
        } else {
            Err("has no open window".into())
        }
    };
    match r {
        Ok(n) => obj(vec![("count", Value::Number(n as f64))]),
        Err(e) => obj(vec![("error", s(&e))]),
    }
}
