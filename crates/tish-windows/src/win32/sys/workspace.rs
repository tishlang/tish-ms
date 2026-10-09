//! `windows.workspace`: opening things through the shell, and the Recycle Bin.
//!
//! - `open(target)`: a URL (`https://…`, `mailto:…`, any registered scheme) or a file, folder,
//!   program or shortcut path -> whether Windows took it
//! - `reveal(path)`: select it in an Explorer window -> false when it doesn't exist
//! - `trash(path)` -> `{ ok, path, error }`; `path` is "" (the Recycle Bin doesn't say where)

use std::path::Path;

use tishlang_core::Value;
use windows::core::{w, PCWSTR};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use super::{obj, s, str_arg, wide};

pub(super) fn open(args: &[Value]) -> Value {
    let target = str_arg(args, 0);
    if target.is_empty() {
        return Value::Bool(false);
    }
    let file = wide(&target);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    Value::Bool(unsafe { ShellExecuteExW(&mut info) }.is_ok())
}

pub(super) fn reveal(args: &[Value]) -> Value {
    let path = str_arg(args, 0);
    if !Path::new(&path).exists() {
        return Value::Bool(false);
    }
    let params = wide(&format!("/select,\"{path}\""));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        lpFile: w!("explorer.exe"),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    Value::Bool(unsafe { ShellExecuteExW(&mut info) }.is_ok())
}

pub(super) fn trash(args: &[Value]) -> Value {
    let path = str_arg(args, 0);
    if !Path::new(&path).exists() {
        return obj(vec![("ok", Value::Bool(false)), ("path", s("")), ("error", s(&format!("no such file: {path}")))]);
    }
    // SHFileOperation wants a list ending in two NULs.
    let mut from = wide(&path);
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r == 0 && !op.fAnyOperationsAborted.as_bool() {
        obj(vec![("ok", Value::Bool(true)), ("path", s("")), ("error", s(""))])
    } else {
        obj(vec![("ok", Value::Bool(false)), ("path", s("")), ("error", s(&format!("couldn't move it to the Recycle Bin (error {r})")))])
    }
}
