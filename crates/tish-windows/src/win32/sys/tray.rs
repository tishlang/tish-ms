//! `windows.statusItem(options)`: an icon in the notification area (tray) with a menu, the
//! menu bar item's counterpart. Calling it again updates the tooltip and menu.
//!
//! `options`: `{ image, tooltip, menu, onClick, onMenu }`
//! - `image`: an `.ico` file; without one, the app's own icon
//! - `menu`: `[{ title, id, enabled } | { separator: true }]`; `onMenu(id)` when one is picked
//! - `onClick()`: with it, a left click calls `onClick` and a right click opens the menu; without
//!   it any click opens the menu

use std::cell::RefCell;

use tishlang_core::Value;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{call, field, wide};

const WM_TRAY: u32 = WM_APP + 11;
const TRAY_ID: u32 = 1;

#[derive(Default)]
struct Item {
    title: String,
    id: String,
    enabled: bool,
    separator: bool,
}

#[derive(Default)]
struct Tray {
    added: bool,
    menu: Vec<Item>,
    on_click: Option<Value>,
    on_menu: Option<Value>,
}

thread_local! {
    static TRAY: RefCell<Tray> = RefCell::new(Tray::default());
}

fn text_field(o: &Value, key: &str) -> String {
    match field(Some(o), key) {
        Some(Value::String(s)) => s.to_string(),
        Some(v @ Value::Number(_)) => v.to_display_string(),
        _ => String::new(),
    }
}

fn copy_into(dst: &mut [u16], text: &str) {
    for (d, c) in dst.iter_mut().zip(text.encode_utf16().chain(std::iter::once(0))) {
        *d = c;
    }
    if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

fn load_icon(path: &str) -> HICON {
    unsafe {
        if !path.is_empty() {
            let p = wide(path);
            if let Ok(h) = LoadImageW(None, PCWSTR(p.as_ptr()), IMAGE_ICON, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE) {
                return HICON(h.0);
            }
        }
        // The executable's first icon, else the stock application icon.
        let module = GetModuleHandleW(None).unwrap_or_default();
        LoadIconW(Some(module.into()), PCWSTR(std::ptr::without_provenance(1))) // MAKEINTRESOURCE(1)
            .or_else(|_| LoadIconW(None, IDI_APPLICATION)).unwrap_or_default()
    }
}

pub(super) fn status_item(args: &[Value]) -> Value {
    let Some(hwnd) = super::ui_hwnd() else { return Value::Bool(false) };
    let o = args.first().cloned().unwrap_or(Value::Null);
    let menu = match field(Some(&o), "menu") {
        Some(Value::Array(a)) => a
            .borrow()
            .iter()
            .map(|e| Item {
                title: text_field(e, "title"),
                id: text_field(e, "id"),
                enabled: !matches!(field(Some(e), "enabled"), Some(Value::Bool(false))),
                separator: matches!(field(Some(e), "separator"), Some(Value::Bool(true))),
            })
            .collect(),
        _ => Vec::new(),
    };
    let fnv = |k: &str| field(Some(&o), k).filter(|v| matches!(v, Value::Function(_)));
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_TIP | NIF_ICON,
        uCallbackMessage: WM_TRAY,
        hIcon: load_icon(&text_field(&o, "image")),
        ..Default::default()
    };
    copy_into(&mut data.szTip, &text_field(&o, "tooltip"));
    let ok = TRAY.with(|t| {
        let mut t = t.borrow_mut();
        t.menu = menu;
        t.on_click = fnv("onClick");
        t.on_menu = fnv("onMenu");
        let ok = unsafe { Shell_NotifyIconW(if t.added { NIM_MODIFY } else { NIM_ADD }, &data) }.as_bool();
        t.added |= ok;
        ok
    });
    Value::Bool(ok)
}

fn show_menu(hwnd: HWND) {
    let picked = TRAY.with(|t| {
        let t = t.borrow();
        if t.menu.is_empty() {
            return None;
        }
        unsafe {
            let m = CreatePopupMenu().ok()?;
            for (i, it) in t.menu.iter().enumerate() {
                if it.separator {
                    let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
                } else {
                    let title = wide(&it.title);
                    let flags = if it.enabled { MF_STRING } else { MF_STRING | MF_GRAYED };
                    let _ = AppendMenuW(m, flags, i + 1, PCWSTR(title.as_ptr()));
                }
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Required for the menu to close when the user clicks elsewhere.
            let _ = SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, Some(0), hwnd, None);
            let _ = DestroyMenu(m);
            let i = cmd.0 as usize;
            (i > 0).then(|| (t.menu[i - 1].id.clone(), t.on_menu.clone()))
        }
    });
    if let Some((id, Some(cb))) = picked {
        call(&cb, &[Value::String(id.into())]);
    }
}

pub(super) fn handle(hwnd: HWND, msg: u32, _wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if msg != WM_TRAY {
        return None;
    }
    let event = (lp.0 & 0xffff) as u32;
    match event {
        WM_LBUTTONUP => match TRAY.with(|t| t.borrow().on_click.clone()) {
            Some(cb) => {
                call(&cb, &[]);
            }
            None => show_menu(hwnd),
        },
        WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
        _ => {}
    }
    Some(LRESULT(0))
}

/// Remove the icon (on exit), so it doesn't linger in the tray until hovered.
pub(crate) fn remove() {
    let Some(hwnd) = super::ui_hwnd() else { return };
    let data = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: TRAY_ID, ..Default::default() };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}
