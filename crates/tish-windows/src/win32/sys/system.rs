//! `windows.system` and `windows.apps`: system actions and the running apps, with tish-macos's
//! `macos.system` / `macos.apps` shapes.
//!
//! `windows.system` (each action returns null, or why it failed):
//! - `lockScreen()`, `sleep()`, `sleepDisplays()`, `screenSaver()`
//! - `restart()`, `shutDown()`, `logOut()`
//! - `emptyTrash(cb)`: `cb(error)` once the Recycle Bin is empty
//! - `darkMode()` -> true/false (null when it can't tell), `setDarkMode(on)`
//! - `volume()` -> `{ level, muted }` (0–100) or `{ error }`; `setVolume(percent)`, `setMuted(on)`
//! - `ejectAll(cb)`: `cb({ ejected, failed })`, drive names and "name: why"
//!
//! `windows.apps`:
//! - `running()` -> `[{ pid, name, path, bundleId, active, hidden, memory }]`, apps with a
//!   taskbar window, most memory first; `bundleId` is the executable path, `hidden` that all of
//!   its windows are minimized, `memory` the private working set in bytes
//! - `act(pid, action)`: `switch`, `hide`, `unhide`, `quit` or `force-quit` -> `{ ok, message, error }`
//! - `quitAll()` (all but Explorer and this app), `hideAll()` -> how many apps
//! - `installed()` -> Start menu shortcuts (apps.rs)

use std::collections::HashMap;

use tishlang_core::Value;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Shutdown::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{arr, in_background, obj, s, str_arg, wide};

fn outcome(r: Result<(), String>) -> Value {
    match r {
        Ok(()) => Value::Null,
        Err(e) => s(&e),
    }
}

// ── Power and session ───────────────────────────────────────────────────────

pub(super) fn lock(_a: &[Value]) -> Value {
    outcome(unsafe { windows::Win32::System::Shutdown::LockWorkStation() }.map_err(|e| e.message()))
}

pub(super) fn sleep(_a: &[Value]) -> Value {
    // Suspend (not hibernate), wake events allowed.
    let ok = unsafe { windows::Win32::System::Power::SetSuspendState(false, false, false) };
    outcome(if ok { Ok(()) } else { Err("Windows wouldn't sleep".into()) })
}

pub(super) fn sleep_displays(_a: &[Value]) -> Value {
    unsafe {
        let _ = PostMessageW(Some(HWND_BROADCAST), WM_SYSCOMMAND, WPARAM(SC_MONITORPOWER as usize), LPARAM(2));
    }
    Value::Null
}

pub(super) fn screen_saver(_a: &[Value]) -> Value {
    unsafe {
        let _ = PostMessageW(Some(GetDesktopWindow()), WM_SYSCOMMAND, WPARAM(0xF140) /* SC_SCREENSAVE */, LPARAM(0));
    }
    Value::Null
}

/// Restarting and shutting down need the shutdown privilege turned on in our token.
fn enable_shutdown_privilege() -> Result<(), String> {
    use windows::Win32::Security::*;
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).map_err(|e| e.message())?;
        let mut luid = LUID::default();
        LookupPrivilegeValueW(None, w!("SeShutdownPrivilege"), &mut luid).map_err(|e| e.message())?;
        let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
        let r = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None).map_err(|e| e.message());
        let _ = CloseHandle(token);
        r
    }
}

fn exit_windows(flags: EXIT_WINDOWS_FLAGS, privileged: bool) -> Value {
    if privileged {
        if let Err(e) = enable_shutdown_privilege() {
            return s(&e);
        }
    }
    outcome(unsafe { ExitWindowsEx(flags, SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED) }.map_err(|e| e.message()))
}

pub(super) fn restart(_a: &[Value]) -> Value {
    exit_windows(EWX_REBOOT, true)
}

pub(super) fn shut_down(_a: &[Value]) -> Value {
    exit_windows(EWX_POWEROFF, true)
}

pub(super) fn log_out(_a: &[Value]) -> Value {
    exit_windows(EWX_LOGOFF, false)
}

pub(super) fn empty_trash(args: &[Value]) -> Value {
    in_background(
        args.first(),
        || unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND) }
            .map_err(|e| e.message()),
        |r: Result<(), String>| match r {
            Ok(()) => Value::Null,
            Err(e) => s(&e),
        },
    );
    Value::Null
}

// ── Appearance ──────────────────────────────────────────────────────────────

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

fn reg_dword(name: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut n = 4u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, PERSONALIZE, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut _), Some(&mut n)) }
        .is_ok()
        .then_some(v)
}

pub(super) fn dark_mode(_a: &[Value]) -> Value {
    reg_dword(w!("AppsUseLightTheme")).map_or(Value::Null, |v| Value::Bool(v == 0))
}

pub(super) fn set_dark_mode(args: &[Value]) -> Value {
    let light: u32 = if matches!(args.first(), Some(Value::Bool(true))) { 0 } else { 1 };
    let bytes = light.to_le_bytes();
    let r = unsafe {
        for name in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
            let e = RegSetKeyValueW(HKEY_CURRENT_USER, PERSONALIZE, name, REG_DWORD.0, Some(bytes.as_ptr() as *const _), 4);
            if e.is_err() {
                return s(&format!("couldn't change the theme ({})", e.0));
            }
        }
        // Tell running apps (and the taskbar) the colours changed.
        let topic = wide("ImmersiveColorSet");
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(topic.as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None)
    };
    let _ = r;
    Value::Null
}

// ── Volume ──────────────────────────────────────────────────────────────────

fn endpoint() -> Result<IAudioEndpointVolume, String> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.message())?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|_| "no audio output".to_string())?;
        device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).map_err(|e| e.message())
    }
}

pub(super) fn volume(_a: &[Value]) -> Value {
    match endpoint().and_then(|ep| unsafe {
        let level = ep.GetMasterVolumeLevelScalar().map_err(|e| e.message())?;
        let muted = ep.GetMute().map_err(|e| e.message())?.as_bool();
        Ok((level, muted))
    }) {
        Ok((level, muted)) => obj(vec![("level", Value::Number((level * 100.0).round() as f64)), ("muted", Value::Bool(muted))]),
        Err(e) => obj(vec![("error", s(&e))]),
    }
}

pub(super) fn set_volume(args: &[Value]) -> Value {
    let pct = args.first().and_then(|v| v.as_number()).unwrap_or(0.0).clamp(0.0, 100.0);
    outcome(endpoint().and_then(|ep| unsafe { ep.SetMasterVolumeLevelScalar((pct / 100.0) as f32, std::ptr::null()).map_err(|e| e.message()) }))
}

pub(super) fn set_muted(args: &[Value]) -> Value {
    let on = matches!(args.first(), Some(Value::Bool(true)));
    outcome(endpoint().and_then(|ep| unsafe { ep.SetMute(on, std::ptr::null()).map_err(|e| e.message()) }))
}

// ── Removable drives ────────────────────────────────────────────────────────

pub(super) fn eject_all(args: &[Value]) -> Value {
    // The shell's own Eject verb, per removable drive, through PowerShell (off the UI thread).
    let script = r#"$s = New-Object -ComObject Shell.Application
$ok = @(); $bad = @()
Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=2' | ForEach-Object {
  $name = $_.DeviceID + ' ' + $_.VolumeName
  try { $s.Namespace(17).ParseName($_.DeviceID + '\').InvokeVerb('Eject'); $ok += $name.Trim() } catch { $bad += ($name.Trim() + ': ' + $_.Exception.Message) }
}
$ok -join "`n"; '---'; $bad -join "`n""#;
    let script = script.to_string();
    in_background(
        args.first(),
        move || super::shell::run_blocking(&script, "", &[], std::time::Duration::from_secs(30)),
        |o| {
            let (ok, bad) = o.stdout.split_once("---").unwrap_or((&o.stdout, ""));
            let list = |t: &str| arr(t.lines().map(str::trim).filter(|l| !l.is_empty()).map(s).collect());
            obj(vec![("ejected", list(ok)), ("failed", list(bad))])
        },
    );
    Value::Null
}

// ── Running apps ────────────────────────────────────────────────────────────

/// A window that shows on the taskbar: visible, unowned, not a tool window, with a title.
fn is_app_window(h: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(h).as_bool() || GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return false;
        }
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
            return false;
        }
        GetWindowTextLengthW(h) > 0
    }
}

fn app_windows() -> HashMap<u32, Vec<HWND>> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> windows::core::BOOL {
        let out = &mut *(lp.0 as *mut HashMap<u32, Vec<HWND>>);
        if is_app_window(h) {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            out.entry(pid).or_default().push(h);
        }
        TRUE
    }
    let mut out: HashMap<u32, Vec<HWND>> = HashMap::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

fn process_path(pid: u32) -> Option<(String, usize)> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false, pid)
            .or_else(|_| OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid))
            .ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let path = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n)
            .ok()
            .map(|_| String::from_utf16_lossy(&buf[..n as usize]));
        let mut pmc = PROCESS_MEMORY_COUNTERS_EX::default();
        let mem = GetProcessMemoryInfo(h, &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS, std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32)
            .ok()
            .map_or(0, |_| pmc.PrivateUsage);
        let _ = CloseHandle(h);
        Some((path?, mem))
    }
}

struct App {
    pid: u32,
    name: String,
    path: String,
    memory: usize,
    active: bool,
    hidden: bool,
}

fn running_apps() -> Vec<App> {
    let me = std::process::id();
    let fg = unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        pid
    };
    let mut apps: Vec<App> = app_windows()
        .into_iter()
        .filter(|(pid, _)| *pid != me)
        .filter_map(|(pid, wins)| {
            let (path, memory) = process_path(pid)?;
            let name = std::path::Path::new(&path).file_stem()?.to_string_lossy().into_owned();
            let hidden = wins.iter().all(|w| unsafe { IsIconic(*w) }.as_bool());
            Some(App { pid, name, path, memory, active: pid == fg, hidden })
        })
        .collect();
    apps.sort_by_key(|a| std::cmp::Reverse(a.memory));
    apps
}

pub(super) fn running(_a: &[Value]) -> Value {
    arr(running_apps()
        .into_iter()
        .map(|a| {
            obj(vec![
                ("pid", Value::Number(a.pid as f64)),
                ("name", s(&a.name)),
                ("path", s(&a.path)),
                ("bundleId", s(&a.path)),
                ("active", Value::Bool(a.active)),
                ("hidden", Value::Bool(a.hidden)),
                ("memory", Value::Number(a.memory as f64)),
            ])
        })
        .collect())
}

fn act_on(pid: u32, action: &str) -> Result<String, String> {
    let wins = app_windows().remove(&pid).unwrap_or_default();
    let name = process_path(pid).and_then(|(p, _)| std::path::Path::new(&p).file_stem().map(|s| s.to_string_lossy().into_owned())).unwrap_or_else(|| format!("process {pid}"));
    if wins.is_empty() && action != "force-quit" {
        return Err(format!("{name} has no window"));
    }
    unsafe {
        match action {
            "switch" | "unhide" => {
                for w in &wins {
                    if IsIconic(*w).as_bool() {
                        let _ = ShowWindow(*w, SW_RESTORE);
                    }
                }
                let _ = SetForegroundWindow(wins[0]);
                Ok(if action == "switch" { format!("Switched to {name}") } else { format!("Showed {name}") })
            }
            "hide" => {
                for w in &wins {
                    let _ = ShowWindow(*w, SW_MINIMIZE);
                }
                Ok(format!("Hid {name}"))
            }
            "quit" => {
                for w in &wins {
                    let _ = PostMessageW(Some(*w), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                Ok(format!("Asked {name} to quit"))
            }
            "force-quit" => {
                let h = OpenProcess(PROCESS_TERMINATE, false, pid).map_err(|e| e.message())?;
                let r = TerminateProcess(h, 1).map_err(|e| e.message());
                let _ = CloseHandle(h);
                r.map(|_| format!("Force quit {name}"))
            }
            other => Err(format!("unknown action `{other}`")),
        }
    }
}

pub(super) fn act(args: &[Value]) -> Value {
    let pid = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    match act_on(pid, &str_arg(args, 1)) {
        Ok(m) => obj(vec![("ok", Value::Bool(true)), ("message", s(&m)), ("error", s(""))]),
        Err(e) => obj(vec![("ok", Value::Bool(false)), ("message", s("")), ("error", s(&e))]),
    }
}

pub(super) fn quit_all(_a: &[Value]) -> Value {
    let apps: Vec<App> = running_apps().into_iter().filter(|a| !a.name.eq_ignore_ascii_case("explorer")).collect();
    for a in &apps {
        let _ = act_on(a.pid, "quit");
    }
    Value::Number(apps.len() as f64)
}

pub(super) fn hide_all(_a: &[Value]) -> Value {
    let apps = running_apps();
    for a in &apps {
        let _ = act_on(a.pid, "hide");
    }
    Value::Number(apps.len() as f64)
}
