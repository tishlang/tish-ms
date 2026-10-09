//! `windows.systemInfo()` -> `{ os, model, chip, cores, memory, uptime, diskTotal, diskFree,
//! battery }`, tish-macos's shape: bytes and seconds; `battery` is `{ percent, charging, onAc,
//! minutesToEmpty, minutesToFull }` (minutes -1 while Windows estimates) or null without one.

use tishlang_core::Value;
use windows::core::{w, PCWSTR};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};

use super::{obj, s};

fn reg_string(key: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut n = (buf.len() * 2) as u32;
    unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut n)) }.ok().ok()?;
    let len = (n as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len]).trim().to_string())
}

fn os_name() -> String {
    let cv = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let product = reg_string(cv, w!("ProductName")).unwrap_or_else(|| "Windows".into());
    let build: u32 = reg_string(cv, w!("CurrentBuildNumber")).and_then(|b| b.parse().ok()).unwrap_or(0);
    // Windows 11 still reports "Windows 10" in ProductName; its builds start at 22000.
    let product = if build >= 22000 { product.replace("Windows 10", "Windows 11") } else { product };
    match reg_string(cv, w!("DisplayVersion")) {
        Some(v) => format!("{product} {v} ({build})"),
        None => format!("{product} ({build})"),
    }
}

pub(super) fn system_info(_a: &[Value]) -> Value {
    let model = reg_string(w!("HARDWARE\\DESCRIPTION\\System\\BIOS"), w!("SystemProductName")).unwrap_or_default();
    let chip = reg_string(w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"), w!("ProcessorNameString")).unwrap_or_default();
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    let mut mem = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    let memory = if unsafe { GlobalMemoryStatusEx(&mut mem) }.is_ok() { mem.ullTotalPhys } else { 0 };
    let uptime = unsafe { GetTickCount64() } / 1000;
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\";
    let d: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    unsafe {
        let _ = GetDiskFreeSpaceExW(PCWSTR(d.as_ptr()), Some(&mut free), Some(&mut total), None);
    }
    let mut ps = SYSTEM_POWER_STATUS::default();
    let battery = if unsafe { GetSystemPowerStatus(&mut ps) }.is_ok() && ps.BatteryFlag != 128 && ps.BatteryFlag != 255 && ps.BatteryLifePercent <= 100 {
        let secs = |v: u32| if v == u32::MAX { -1.0 } else { (v / 60) as f64 };
        obj(vec![
            ("percent", Value::Number(ps.BatteryLifePercent as f64)),
            ("charging", Value::Bool(ps.BatteryFlag & 8 != 0)),
            ("onAc", Value::Bool(ps.ACLineStatus == 1)),
            ("minutesToEmpty", Value::Number(if ps.ACLineStatus == 1 { -1.0 } else { secs(ps.BatteryLifeTime) })),
            ("minutesToFull", Value::Number(if ps.ACLineStatus == 1 { secs(ps.BatteryFullLifeTime) } else { -1.0 })),
        ])
    } else {
        Value::Null
    };
    obj(vec![
        ("os", s(&os_name())),
        ("model", s(&model)),
        ("chip", s(&chip)),
        ("cores", Value::Number(cores as f64)),
        ("memory", Value::Number(memory as f64)),
        ("uptime", Value::Number(uptime as f64)),
        ("diskTotal", Value::Number(total as f64)),
        ("diskFree", Value::Number(free as f64)),
        ("battery", battery),
    ])
}
