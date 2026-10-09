//! The Win32 host: one top-level window per `windows.run`, drawn with Direct2D, with a native
//! EDIT control for each `textinput`. Tish runs on the UI thread only; work from other threads
//! (`whenSettled`) comes back as a posted message.

mod render;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tishlang_core::{ObjectMap, PropMap, Value};
use tishlang_ms_common::layout::{font, layout};
use tishlang_ms_common::style::{props_bool, props_f64, props_string};
use tishlang_ms_common::tree::{from_vnode, Node, Rect};
use tishlang_ui::{install_host_for_root, native_create_root, Host, LEGACY_ROOT_ID};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::Controls::{EM_SETCUEBANNER, EM_SETSEL, MARGINS};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VIRTUAL_KEY};
use windows::Win32::UI::WindowsAndMessaging::*;

use render::Renderer;

const TIMER_ID: usize = 1;
const WM_SETTLED: u32 = WM_APP + 1;
/// A resize that arrived while a commit held the app: lay out again once it's done.
const WM_RELAYOUT: u32 = WM_APP + 2;
const EN_CHANGE_CODE: u32 = 0x0300;

/// One `textinput`: its EDIT control and the node path it belongs to.
struct Edit {
    hwnd: HWND,
    font: HFONT,
    font_px: i32,
}

#[derive(Default)]
struct Stats {
    commits: u64,
    last_commit_ms: f64,
    total_commit_ms: f64,
    last_paint_ms: f64,
    first_paint_since_start_ms: f64,
}

struct App {
    hwnd: HWND,
    renderer: Renderer,
    roots: Vec<Node>,
    edits: Vec<Edit>,
    on_key: Option<Value>,
    stats: Stats,
    painted: bool,
    #[cfg(feature = "winui")]
    xaml: Option<crate::winui::Xaml>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static PENDING: RefCell<HashMap<u64, Value>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = const { Cell::new(1) };
    static EDIT_PROCS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
    /// Kept outside `APP`: edit controls ask for colours while a commit holds `APP`.
    static DARK: Cell<bool> = const { Cell::new(true) };
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = ObjectMap::default();
    for (k, v) in pairs {
        m.insert(Arc::from(k), v);
    }
    Value::object(m)
}

fn call(f: &Value, args: &[Value]) -> Value {
    match f {
        Value::Function(f) => f.call(args),
        _ => Value::Null,
    }
}

/// `o[key]`, cloned out of the object.
fn opt(o: Option<&Value>, key: &str) -> Option<Value> {
    match o {
        Some(Value::Object(m)) => m.borrow().strings.get(key).cloned(),
        _ => None,
    }
}

fn opt_props(o: Option<&Value>) -> PropMap {
    match o {
        Some(Value::Object(m)) => m.borrow().strings.clone(),
        _ => PropMap::default(),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn dpi_scale(hwnd: HWND) -> f64 {
    unsafe { GetDpiForWindow(hwnd) as f64 / 96.0 }
}

// ── The host ────────────────────────────────────────────────────────────────

struct Win32Host;

impl Host for Win32Host {
    fn commit_root(&mut self, vnode: &Value) {
        let t0 = Instant::now();
        APP.with(|a| {
            let mut a = a.borrow_mut();
            let Some(app) = a.as_mut() else { return };
            let roots = from_vnode(vnode);
            app.roots = roots;
            relayout(app);
            #[cfg(feature = "winui")]
            if let Some(x) = app.xaml.as_mut() {
                if let Err(e) = x.commit(&app.roots) {
                    eprintln!("tish-windows: XAML commit failed: {e}");
                }
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                app.stats.commits += 1;
                app.stats.last_commit_ms = ms;
                app.stats.total_commit_ms += ms;
                return;
            }
            sync_edits(app);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            app.stats.commits += 1;
            app.stats.last_commit_ms = ms;
            app.stats.total_commit_ms += ms;
            unsafe {
                let _ = InvalidateRect(Some(app.hwnd), None, false);
            }
        });
    }
}

fn client_size_dip(hwnd: HWND) -> (f64, f64) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let sc = dpi_scale(hwnd);
    ((rc.right - rc.left) as f64 / sc, (rc.bottom - rc.top) as f64 / sc)
}

fn relayout(app: &mut App) {
    let (w, _) = client_size_dip(app.hwnd);
    let mut y = 0.0;
    let renderer = &app.renderer;
    for r in app.roots.iter_mut() {
        y += layout(r, 0.0, y, w, renderer);
    }
}

fn inputs(roots: &[Node]) -> Vec<&Node> {
    let mut all = Vec::new();
    for r in roots {
        r.walk(&mut all);
    }
    all.into_iter().filter(|n| n.tag == "textinput").collect()
}

/// One EDIT per `textinput`, in tree order; position, font and (controlled) value follow the node.
fn sync_edits(app: &mut App) {
    let sc = dpi_scale(app.hwnd);
    let nodes: Vec<(Rect, PropMap)> = inputs(&app.roots).into_iter().map(|n| (n.frame, n.props.clone())).collect();
    while app.edits.len() > nodes.len() {
        if let Some(e) = app.edits.pop() {
            unsafe {
                let _ = DestroyWindow(e.hwnd);
                let _ = DeleteObject(e.font.into());
            }
        }
    }
    for (i, (f, props)) in nodes.iter().enumerate() {
        let fpx = (font(props).size * sc).round() as i32;
        if i >= app.edits.len() {
            app.edits.push(create_edit(app.hwnd, fpx));
        }
        let e = &mut app.edits[i];
        if e.font_px != fpx {
            unsafe {
                let _ = DeleteObject(e.font.into());
            }
            e.font = make_font(fpx);
            e.font_px = fpx;
            unsafe {
                SendMessageW(e.hwnd, WM_SETFONT, Some(WPARAM(e.font.0 as usize)), Some(LPARAM(1)));
            }
        }
        unsafe {
            let _ = MoveWindow(e.hwnd, (f.x * sc) as i32, (f.y * sc) as i32, (f.w * sc) as i32, (f.h * sc) as i32, true);
        }
        if let Some(v) = props_string(props, &["value"]) {
            if edit_text(e.hwnd) != v {
                let w = wide(&v);
                unsafe {
                    let _ = SetWindowTextW(e.hwnd, PCWSTR(w.as_ptr()));
                    // Keep the caret at the end after an outside change.
                    let n = v.encode_utf16().count();
                    SendMessageW(e.hwnd, EM_SETSEL, Some(WPARAM(n)), Some(LPARAM(n as isize)));
                }
            }
        }
        if let Some(p) = props_string(props, &["placeholder"]) {
            let w = wide(&p);
            unsafe {
                SendMessageW(e.hwnd, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(w.as_ptr() as isize)));
            }
        }
    }
}

fn make_font(px: i32) -> HFONT {
    unsafe { CreateFontW(-px, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI Variable Text")) }
}

fn create_edit(parent: HWND, font_px: i32) -> Edit {
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            0,
            0,
            10,
            10,
            Some(parent),
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        )
        .unwrap_or_default();
        let font = make_font(font_px);
        SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, edit_proc as *const () as usize as isize);
        EDIT_PROCS.with(|p| p.borrow_mut().insert(hwnd.0 as isize, old));
        let _ = SetFocus(Some(hwnd));
        Edit { hwnd, font, font_px }
    }
}

fn edit_text(hwnd: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

/// Input `i` (tree order) now holds `text`: call its `onChange` / `onInput`. A change made by a
/// commit itself (setting a controlled value) arrives while the app is borrowed and is ignored.
fn input_changed(i: usize, text: String) {
    let handler = APP.with(|a| {
        let a = a.try_borrow().ok()?;
        let node = inputs(&a.as_ref()?.roots).into_iter().nth(i)?;
        node.handler("onChange").or_else(|| node.handler("onInput"))
    });
    if let Some(f) = handler {
        call(&f, &[s(&text)]);
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

fn key_name(vk: VIRTUAL_KEY) -> String {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let base = match vk {
        VK_RETURN => "enter".to_string(),
        VK_ESCAPE => "escape".into(),
        VK_TAB => "tab".into(),
        VK_UP => "up".into(),
        VK_DOWN => "down".into(),
        VK_LEFT => "left".into(),
        VK_RIGHT => "right".into(),
        VK_BACK => "backspace".into(),
        VK_DELETE => "delete".into(),
        VK_HOME => "home".into(),
        VK_END => "end".into(),
        VK_PRIOR => "pageup".into(),
        VK_NEXT => "pagedown".into(),
        k if (0x30..=0x39).contains(&k.0) || (0x41..=0x5A).contains(&k.0) => ((k.0 as u8) as char).to_ascii_lowercase().to_string(),
        k => format!("vk{}", k.0),
    };
    let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
    let mut name = String::new();
    if down(VK_CONTROL) {
        name.push_str("ctrl+");
    }
    if down(VK_MENU) {
        name.push_str("alt+");
    }
    if down(VK_SHIFT) {
        name.push_str("shift+");
    }
    name + &base
}

/// `onKey(name)` from `run`'s options; true when the app handled it (the key goes no further).
fn route_key(vk: VIRTUAL_KEY) -> bool {
    let Some(cb) = APP.with(|a| a.try_borrow().ok()?.as_ref().and_then(|a| a.on_key.clone())) else { return false };
    matches!(call(&cb, &[s(&key_name(vk))]), Value::Bool(true))
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let old = EDIT_PROCS.with(|p| p.borrow().get(&(hwnd.0 as isize)).copied()).unwrap_or(0);
    if msg == WM_KEYDOWN && route_key(VIRTUAL_KEY(wp.0 as u16)) {
        return LRESULT(0);
    }
    // Enter and Escape would beep in a single-line EDIT.
    if msg == WM_CHAR && (wp.0 == 13 || wp.0 == 27) {
        return LRESULT(0);
    }
    let proc: WNDPROC = std::mem::transmute(old);
    CallWindowProcW(proc, hwnd, msg, wp, lp)
}

// ── Window procedure ────────────────────────────────────────────────────────

fn handler_at(x: f64, y: f64, name: &str) -> Option<Value> {
    APP.with(|a| {
        let a = a.try_borrow().ok()?;
        let app = a.as_ref()?;
        app.roots.iter().rev().find_map(|r| r.hit(x, y, name)).and_then(|n| n.handler(name))
    })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            let t0 = Instant::now();
            let dpi = GetDpiForWindow(hwnd) as f32;
            APP.with(|a| {
                let Ok(mut guard) = a.try_borrow_mut() else {
                    // Mid-commit: the commit invalidates when it's done.
                    return;
                };
                if let Some(app) = guard.as_mut() {
                    // Under a XAML island Direct2D only clears to the backdrop.
                    #[cfg(feature = "winui")]
                    let island = app.xaml.is_some();
                    #[cfg(not(feature = "winui"))]
                    let island = false;
                    let drawn: &[Node] = if island { &[] } else { &app.roots };
                    let _ = app.renderer.paint(hwnd, dpi, drawn);
                    app.stats.last_paint_ms = t0.elapsed().as_secs_f64() * 1000.0;
                    if !app.painted {
                        app.painted = true;
                        app.stats.first_paint_since_start_ms = ms_since_process_start();
                    }
                }
            });
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_SIZE => {
            let (w, h) = ((lp.0 & 0xffff) as u32, ((lp.0 >> 16) & 0xffff) as u32);
            APP.with(|a| {
                let Ok(mut guard) = a.try_borrow_mut() else {
                    let _ = PostMessageW(Some(hwnd), WM_RELAYOUT, WPARAM(0), lp);
                    return;
                };
                if let Some(app) = guard.as_mut() {
                    app.renderer.resize(w, h);
                    relayout(app);
                    #[cfg(feature = "winui")]
                    if let Some(x) = app.xaml.as_mut() {
                        x.resize(w as i32, h as i32);
                        let _ = x.commit(&app.roots);
                        return;
                    }
                    sync_edits(app);
                }
            });
            LRESULT(0)
        }
        WM_RELAYOUT => SendMessageW(hwnd, WM_SIZE, Some(WPARAM(0)), Some(lp)),
        WM_DPICHANGED => {
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let sc = dpi_scale(hwnd);
            let (x, y) = ((lp.0 & 0xffff) as i16 as f64 / sc, ((lp.0 >> 16) & 0xffff) as i16 as f64 / sc);
            if let Some(f) = handler_at(x, y, "onClick") {
                call(&f, &[obj(vec![("x", Value::Number(x)), ("y", Value::Number(y))])]);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            route_key(VIRTUAL_KEY(wp.0 as u16));
            LRESULT(0)
        }
        WM_COMMAND if ((wp.0 >> 16) & 0xffff) as u32 == EN_CHANGE_CODE => {
            let edit = HWND(lp.0 as *mut _);
            let index = APP.with(|a| a.try_borrow().ok()?.as_ref()?.edits.iter().position(|e| e.hwnd == edit));
            if let Some(i) = index {
                input_changed(i, edit_text(edit));
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_ID => {
            tishlang_runtime::drain_timers();
            LRESULT(0)
        }
        WM_SETTLED => {
            let id = wp.0 as u64;
            let payload = Box::from_raw(lp.0 as *mut Settled);
            if let Some(cb) = PENDING.with(|p| p.borrow_mut().remove(&id)) {
                match payload.0 {
                    Ok(v) => call(&cb, &[v, Value::Null]),
                    Err(e) => call(&cb, &[Value::Null, e]),
                };
            }
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            let dark = DARK.with(|d| d.get());
            let hdc = HDC(wp.0 as *mut _);
            if dark {
                SetTextColor(hdc, COLORREF(0x00F2F2F2));
                SetBkColor(hdc, COLORREF(0x00202020));
                return LRESULT(GetStockObject(BLACK_BRUSH).0 as isize);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ── Process timing and memory, for `windows.stats()` ─────────────────────────

fn filetime_ms(f: FILETIME) -> f64 {
    (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 10_000.0
}

fn ms_since_process_start() -> f64 {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return -1.0;
        }
        let now = windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime();
        filetime_ms(now) - filetime_ms(c)
    }
}

fn working_set_mb() -> f64 {
    unsafe {
        let mut pmc = PROCESS_MEMORY_COUNTERS::default();
        let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, cb).is_err() {
            return -1.0;
        }
        pmc.WorkingSetSize as f64 / (1024.0 * 1024.0)
    }
}

// ── `windows.*` ─────────────────────────────────────────────────────────────

struct Settled(Result<Value, Value>);
struct SendPtr(HWND, Value, u64);
unsafe impl Send for SendPtr {}

fn when_settled(args: &[Value]) -> Value {
    let Some(hwnd) = APP.with(|a| a.borrow().as_ref().map(|a| a.hwnd)) else { return Value::Null };
    let id = NEXT.with(|n| {
        let v = n.get();
        n.set(v + 1);
        v
    });
    PENDING.with(|p| p.borrow_mut().insert(id, args.get(1).cloned().unwrap_or(Value::Null)));
    let carry = SendPtr(hwnd, args.first().cloned().unwrap_or(Value::Null), id);
    std::thread::spawn(move || {
        let carry = carry;
        let result = match &carry.1 {
            Value::Promise(p) => p.block_until_settled(),
            other => Ok(other.clone()),
        };
        let payload = Box::into_raw(Box::new(Settled(result)));
        unsafe {
            if PostMessageW(Some(carry.0), WM_SETTLED, WPARAM(carry.2 as usize), LPARAM(payload as isize)).is_err() {
                drop(Box::from_raw(payload));
            }
        }
    });
    Value::Null
}

fn backdrop_type(name: &str) -> DWM_SYSTEMBACKDROP_TYPE {
    match name {
        "none" => DWMSBT_NONE,
        "acrylic" => DWMSBT_TRANSIENTWINDOW,
        "tabbed" => DWMSBT_TABBEDWINDOW,
        _ => DWMSBT_MAINWINDOW,
    }
}

fn create_window(o: &PropMap) -> windows::core::Result<HWND> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class = w!("TishWindowsHost");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide(&props_string(o, &["title"]).unwrap_or_else(|| "Tish".into()));
        let borderless = props_bool(o, &["borderless"], false);
        let style = if borderless { WS_POPUP | WS_THICKFRAME } else { WS_OVERLAPPEDWINDOW };
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(title.as_ptr()),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let sc = dpi_scale(hwnd);
        let (w, h) = (props_f64(o, &["width"], 640.0), props_f64(o, &["height"], 420.0));
        let (sw, shh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let (pw, ph) = ((w * sc) as i32, (h * sc) as i32);
        let _ = SetWindowPos(hwnd, None, (sw - pw) / 2, (shh - ph) / 4, pw, ph, SWP_NOZORDER | SWP_FRAMECHANGED);

        let dark = props_bool(o, &["dark"], true);
        let on: windows::core::BOOL = dark.into();
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const _ as _, 4);
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
        let backdrop = backdrop_type(&props_string(o, &["backdrop"]).unwrap_or_default());
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as _, 4);
        // Let the backdrop show through the whole client area.
        let _ = DwmExtendFrameIntoClientArea(hwnd, &MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 });
        SetTimer(Some(hwnd), TIMER_ID, 16, None);
        Ok(hwnd)
    }
}

fn run_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(feature = "winui")]
static XAML_HOOKS: crate::winui::Hooks = crate::winui::Hooks { on_text: input_changed, on_key: |vk| route_key(VIRTUAL_KEY(vk)) };

fn show(hwnd: HWND) {
    #[cfg(feature = "winui")]
    APP.with(|a| {
        if let Some(x) = a.borrow().as_ref().and_then(|a| a.xaml.as_ref()) {
            x.focus_input();
        }
    });
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        if let Some(e) = APP.with(|a| a.borrow().as_ref().and_then(|a| a.edits.first().map(|e| e.hwnd))) {
            let _ = SetFocus(Some(e));
        }
    }
}

fn run(args: &[Value]) -> Value {
    // The UI thread is a single-threaded apartment: XAML requires it, and so do the clipboard and
    // drag and drop through OLE.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let app_fn = args.first().cloned().unwrap_or(Value::Null);
    let o = opt_props(args.get(1));
    let hwnd = match create_window(&o) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("tish-windows: cannot create the window: {e}");
            return Value::Null;
        }
    };
    let dark = props_bool(&o, &["dark"], true);
    DARK.with(|d| d.set(dark));
    let renderer = match Renderer::new(dark) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("tish-windows: Direct2D is unavailable: {e}");
            return Value::Null;
        }
    };
    let on_key = opt(args.get(1), "onKey").filter(|v| matches!(v, Value::Function(_)));
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            hwnd,
            renderer,
            roots: Vec::new(),
            edits: Vec::new(),
            on_key,
            stats: Stats::default(),
            painted: false,
            #[cfg(feature = "winui")]
            xaml: None,
        })
    });
    #[cfg(feature = "winui")]
    if std::env::var("TISH_WINDOWS_HOST").as_deref() == Ok("winui") {
        match crate::winui::Xaml::attach(hwnd, dark, &XAML_HOOKS) {
            Ok(x) => {
                let mut rc = RECT::default();
                unsafe {
                    let _ = GetClientRect(hwnd, &mut rc);
                }
                x.resize(rc.right - rc.left, rc.bottom - rc.top);
                APP.with(|a| {
                    if let Some(app) = a.borrow_mut().as_mut() {
                        app.xaml = Some(x);
                    }
                });
            }
            Err(e) => eprintln!("tish-windows: WinUI host unavailable ({e}); using Win32"),
        }
    }
    install_host_for_root(LEGACY_ROOT_ID, Box::new(Win32Host));
    if let Value::Object(root) = native_create_root(&[Value::Null]) {
        let render = root.borrow().strings.get("render").cloned();
        if let Some(r) = render {
            call(&r, &[app_fn]);
        }
    }
    if props_bool(&o, &["autoShow"], true) {
        show(hwnd);
    }
    if props_bool(&o, &["autoRunEventLoop"], true) {
        run_loop();
        return Value::Null;
    }
    let h = hwnd.0 as isize;
    obj(vec![
        ("show", Value::native(move |_| {
            show(HWND(h as *mut _));
            Value::Null
        })),
        ("hide", Value::native(move |_| {
            unsafe {
                let _ = ShowWindow(HWND(h as *mut _), SW_HIDE);
            }
            Value::Null
        })),
        ("runEventLoop", Value::native(|_| {
            run_loop();
            Value::Null
        })),
    ])
}

fn host_name(_app: &App) -> &'static str {
    #[cfg(feature = "winui")]
    if _app.xaml.is_some() {
        return "winui";
    }
    "win32"
}

fn stats(_a: &[Value]) -> Value {
    APP.with(|a| {
        let a = a.borrow();
        let Some(app) = a.as_ref() else { return Value::Null };
        let st = &app.stats;
        obj(vec![
            ("host", s(host_name(app))),
            ("commits", Value::Number(st.commits as f64)),
            ("lastCommitMs", Value::Number(st.last_commit_ms)),
            ("avgCommitMs", Value::Number(if st.commits > 0 { st.total_commit_ms / st.commits as f64 } else { 0.0 })),
            ("lastPaintMs", Value::Number(st.last_paint_ms)),
            ("firstPaintSinceStartMs", Value::Number(st.first_paint_since_start_ms)),
            ("workingSetMb", Value::Number(working_set_mb())),
        ])
    })
}

fn quit(_a: &[Value]) -> Value {
    unsafe { PostQuitMessage(0) };
    Value::Null
}

/// Pump pending messages (paints included) once, then return: lets a script measure a paint.
fn pump(_a: &[Value]) -> Value {
    unsafe {
        if let Some(h) = APP.with(|a| a.borrow().as_ref().map(|a| a.hwnd)) {
            let _ = UpdateWindow(h);
        }
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Value::Null
}

fn start_timers(_a: &[Value]) -> Value {
    Value::Null
}

pub(crate) fn windows_object() -> Value {
    let mut w = ObjectMap::default();
    type Native = fn(&[Value]) -> Value;
    let fns: [(&str, Native); 6] = [("run", run), ("whenSettled", when_settled), ("startTimers", start_timers), ("stats", stats), ("quit", quit), ("pump", pump)];
    for (k, f) in fns {
        w.insert(Arc::from(k), Value::native(f));
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
