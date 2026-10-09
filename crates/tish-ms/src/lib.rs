//! Windows native host (`tish-ms`): real OS window via `winit` + desktop notifications.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(windows)]
use std::thread;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{json, Value};

static ATTACHED: AtomicBool = AtomicBool::new(false);
static WINDOW_COUNT: AtomicU64 = AtomicU64::new(0);
static LAST_TITLE: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new("Tish".into()));

/// Open (or ensure) a native OS window. Off-Windows records attach for CI cross-checks.
pub fn attach_native(title: &str) -> Result<Value, String> {
    *LAST_TITLE.lock() = title.to_string();
    let id = WINDOW_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    ATTACHED.store(true, Ordering::SeqCst);

    #[cfg(windows)]
    {
        let title = title.to_string();
        thread::Builder::new()
            .name("tish-ms-window".into())
            .spawn(move || open_window(&title))
            .map_err(|e| e.to_string())?;
    }

    Ok(json!({
        "ok": true,
        "attached": true,
        "platform": "ms",
        "windowId": id,
        "title": LAST_TITLE.lock().clone(),
        "nativeWindow": cfg!(windows),
    }))
}

pub fn is_attached() -> bool {
    ATTACHED.load(Ordering::SeqCst)
}

pub fn notification_show(title: &str, body: &str) -> Result<Value, String> {
    let mut n = notify_rust::Notification::new();
    n.summary(title);
    if !body.is_empty() {
        n.body(body);
    }
    n.show().map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "title": title, "body": body }))
}

pub fn notification_permission_state() -> Value {
    json!({ "state": "granted" })
}

pub fn notification_request_permission() -> Value {
    json!({ "state": "granted" })
}

#[cfg(windows)]
fn open_window(title: &str) {
    use winit::event::{Event, WindowEvent};
    use winit::event_loop::{ControlFlow, EventLoop};
    use winit::window::WindowBuilder;

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("tish-ms: EventLoop::new failed: {e}");
            return;
        }
    };
    let _window = match WindowBuilder::new()
        .with_title(title)
        .with_inner_size(winit::dpi::LogicalSize::new(480.0, 320.0))
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("tish-ms: WindowBuilder failed: {e}");
            return;
        }
    };

    event_loop.run(move |event, elwt| {
        elwt.set_control_flow(ControlFlow::Wait);
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            elwt.exit();
        }
    });
}
