//! `windows.credentials`: secrets in Windows Credential Manager (generic credentials, per user).
//! The Keychain counterpart; `account` is the credential's target name.
//!
//! - `get(account)` -> the secret, or null
//! - `set(account, secret)` -> null, or why it couldn't be saved
//! - `delete(account)` -> whether one was removed
//! - `has(account)` -> whether one is saved

use tishlang_core::Value;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::*;

use super::{s, str_arg, wide};

fn read(account: &str) -> Option<String> {
    let target = wide(account);
    unsafe {
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut p).ok()?;
        let c = &*p;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let secret = String::from_utf8_lossy(bytes).into_owned();
        CredFree(p as *const _);
        Some(secret)
    }
}

pub(super) fn get(args: &[Value]) -> Value {
    read(&str_arg(args, 0)).map_or(Value::Null, |v| s(&v))
}

pub(super) fn has(args: &[Value]) -> Value {
    Value::Bool(read(&str_arg(args, 0)).is_some())
}

pub(super) fn set(args: &[Value]) -> Value {
    let account = str_arg(args, 0);
    let mut secret = str_arg(args, 1).into_bytes();
    let mut target = wide(&account);
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        CredentialBlobSize: secret.len() as u32,
        CredentialBlob: secret.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    match unsafe { CredWriteW(&cred, 0) } {
        Ok(()) => Value::Null,
        Err(e) => s(&e.message()),
    }
}

pub(super) fn delete(args: &[Value]) -> Value {
    let target = wide(&str_arg(args, 0));
    Value::Bool(unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) }.is_ok())
}
