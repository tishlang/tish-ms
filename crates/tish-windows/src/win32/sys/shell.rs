//! `windows.shell.run(command, { cwd, args, timeoutMs }, cb)` -> id: runs `command` with Windows
//! PowerShell on a background thread; `cb({ id, code, stdout, stderr, ms, timedOut })` on the UI
//! thread.
//!
//! - The command is written to a temporary `.ps1` and run with `-File`, so `args` arrive as
//!   `$args[0]`, `$args[1]`, … and are never spliced into the script.
//! - `cwd` defaults to the home folder. No console window opens.
//! - stdout and stderr are capped at 1 MB each. A command still running after `timeoutMs`
//!   (default 60 s) is killed with everything it started.

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tishlang_core::Value;

use super::{field, in_background, obj, s, str_arg};

const MAX_OUTPUT: usize = 1 << 20;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub ms: f64,
    pub timed_out: bool,
}

fn read_capped(mut r: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut r).take(MAX_OUTPUT as u64).read_to_end(&mut buf);
        let _ = std::io::copy(&mut r, &mut std::io::sink());
        String::from_utf8_lossy(&buf).into_owned()
    })
}

pub(crate) fn run_blocking(cmd: &str, cwd: &str, args: &[String], timeout: Duration) -> Output {
    let t0 = Instant::now();
    let fail = |stderr: String| Output { code: 127, stdout: String::new(), stderr, ms: 0.0, timed_out: false };
    let script = std::env::temp_dir().join(format!("tish-shell-{}-{}.ps1", std::process::id(), t0.elapsed().as_nanos()));
    // A UTF-8 BOM so Windows PowerShell reads non-ASCII scripts correctly.
    match std::fs::File::create(&script).and_then(|mut f| f.write_all(b"\xEF\xBB\xBF").and_then(|_| f.write_all(cmd.as_bytes()))) {
        Ok(()) => {}
        Err(e) => return fail(format!("cannot write the script: {e}")),
    }
    let mut c = Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&script).args(args);
    c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let dir = if !cwd.is_empty() && std::path::Path::new(cwd).is_dir() { Some(cwd.into()) } else { std::env::var_os("USERPROFILE") };
    if let Some(d) = dir {
        c.current_dir(d);
    }
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            let _ = std::fs::remove_file(&script);
            return fail(format!("cannot run PowerShell: {e}"));
        }
    };
    let out = read_capped(child.stdout.take().expect("piped"));
    let err = read_capped(child.stderr.take().expect("piped"));
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) if t0.elapsed() > timeout => {
                // Kill the whole tree: PowerShell and anything it started.
                let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).creation_flags(CREATE_NO_WINDOW).status();
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break -1;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break -1,
        }
    };
    let _ = std::fs::remove_file(&script);
    let mut stderr = err.join().unwrap_or_default();
    if timed_out {
        stderr.push_str(&format!("killed after {} ms\n", timeout.as_millis()));
    }
    Output { code, stdout: out.join().unwrap_or_default(), stderr, ms: t0.elapsed().as_secs_f64() * 1000.0, timed_out }
}

fn value(o: Output) -> Value {
    obj(vec![
        ("code", Value::Number(o.code as f64)),
        ("stdout", s(&o.stdout)),
        ("stderr", s(&o.stderr)),
        ("ms", Value::Number(o.ms)),
        ("timedOut", Value::Bool(o.timed_out)),
    ])
}

pub(super) fn run(args: &[Value]) -> Value {
    let cmd = str_arg(args, 0);
    let opts = args.get(1);
    let cwd = match field(opts, "cwd") {
        Some(Value::String(c)) => c.to_string(),
        _ => String::new(),
    };
    let values: Vec<String> = match field(opts, "args") {
        Some(Value::Array(a)) => a.borrow().iter().map(|v| v.to_display_string()).collect(),
        _ => Vec::new(),
    };
    let ms = field(opts, "timeoutMs").and_then(|v| v.as_number()).unwrap_or(60_000.0).max(1.0);
    let id = in_background(args.get(2), move || run_blocking(&cmd, &cwd, &values, Duration::from_millis(ms as u64)), value);
    Value::Number(id as f64)
}
