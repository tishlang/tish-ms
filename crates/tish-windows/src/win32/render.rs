//! Direct2D drawing and DirectWrite text for the node tree, in device-independent pixels.

use std::cell::RefCell;
use std::collections::HashMap;

use tishlang_ms_common::layout::{font, Font, Measure};
use tishlang_ms_common::style::{props_color, Rgba};
use tishlang_ms_common::tree::{Node, Rect};
use windows::core::{Result, PCWSTR};
use windows_numerics::Vector2;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::Interface;

pub struct Renderer {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    formats: RefCell<HashMap<(u32, u16), IDWriteTextFormat>>,
    /// Decoded images by (src, pixel size), as WIC bitmaps: they outlive render targets.
    images: RefCell<HashMap<(String, u32), Option<IWICBitmapSource>>>,
    /// The same as Direct2D bitmaps for the current render target.
    bitmaps: RefCell<HashMap<(String, u32), ID2D1Bitmap>>,
    wic: Option<IWICImagingFactory>,
    pub dark: bool,
    /// Drawn over the backdrop before everything else (the panel's tint), by dark mode.
    pub tint: Option<(Rgba, Rgba)>,
    /// Over the tint: how much the panel dims what is behind it (the Mac glass's scrim).
    pub scrim: Option<(Rgba, Rgba)>,
    /// The window's corner radius (DIPs) and the hairline drawn along its edge.
    pub radius: f32,
    pub edge: Option<(Rgba, Rgba)>,
    /// What is behind the window, blurred: drawn first, so the panel reads as frosted glass on
    /// any machine (DWM's acrylic needs a GPU).
    backdrop: Option<Backdrop>,
    backdrop_bitmap: RefCell<Option<ID2D1Bitmap>>,
    /// The text inputs' state (their EDIT controls are kept off-screen; the field is drawn here).
    fields: RefCell<Vec<Field>>,
    field_at: std::cell::Cell<usize>,
}

/// The screen behind the window, blurred, at a fraction of its pixel size.
pub struct Backdrop {
    /// Premultiplied BGRA, `w` × `h`, opaque.
    pub pixels: Vec<u8>,
    pub w: u32,
    pub h: u32,
    /// The screen rectangle it covers, in physical pixels.
    pub screen: RECT,
}

/// One `textinput`'s text and selection, in tree order.
#[derive(Clone, Default)]
pub struct Field {
    pub text: String,
    /// UTF-16 offsets, start <= end.
    pub sel: (u32, u32),
    pub focused: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn color(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}

fn rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x as f32, top: r.y as f32, right: (r.x + r.w) as f32, bottom: (r.y + r.h) as f32 }
}

impl Renderer {
    pub fn new(dark: bool) -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            Ok(Self { d2d, dw, target: None, formats: RefCell::new(HashMap::new()), images: RefCell::new(HashMap::new()), bitmaps: RefCell::new(HashMap::new()), wic, dark, tint: None, scrim: None, radius: 0.0, edge: None, backdrop: None, backdrop_bitmap: RefCell::new(None), fields: RefCell::new(Vec::new()), field_at: std::cell::Cell::new(0) })
        }
    }

    fn format(&self, f: &Font) -> Result<IDWriteTextFormat> {
        let key = ((f.size * 100.0) as u32, f.weight);
        if let Some(tf) = self.formats.borrow().get(&key) {
            return Ok(tf.clone());
        }
        let family = wide("Segoe UI Variable Text\0");
        let locale = wide("en-us\0");
        let tf = unsafe {
            self.dw.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT(f.weight as i32),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                f.size as f32,
                PCWSTR(locale.as_ptr()),
            )?
        };
        self.formats.borrow_mut().insert(key, tf.clone());
        Ok(tf)
    }

    fn text_layout(&self, text: &str, f: &Font, max_w: f64, wrap: bool) -> Result<IDWriteTextLayout> {
        let tf = self.format(f)?;
        let w = wide(text);
        unsafe {
            let tl = self.dw.CreateTextLayout(&w, &tf, max_w.max(1.0) as f32, 100_000.0)?;
            if !wrap {
                tl.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            }
            Ok(tl)
        }
    }

    fn ensure_target(&mut self, hwnd: HWND, dpi: f32) -> Result<ID2D1HwndRenderTarget> {
        if let Some(t) = &self.target {
            return Ok(t.clone());
        }
        let mut rc = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rc)? };
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: dpi,
            dpiY: dpi,
            ..Default::default()
        };
        let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U { width: (rc.right - rc.left) as u32, height: (rc.bottom - rc.top) as u32 },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let t = unsafe { self.d2d.CreateHwndRenderTarget(&props, &hprops)? };
        self.target = Some(t.clone());
        Ok(t)
    }

    pub fn set_backdrop(&mut self, b: Option<Backdrop>) {
        self.backdrop = b;
        *self.backdrop_bitmap.borrow_mut() = None;
    }

    /// The blurred backdrop, scaled up over the part of the screen the window covers.
    fn draw_backdrop(&self, t: &ID2D1HwndRenderTarget, hwnd: HWND, dpi: f32) -> Result<()> {
        let Some(b) = &self.backdrop else { return Ok(()) };
        if self.backdrop_bitmap.borrow().is_none() {
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            let bmp = unsafe { t.CreateBitmap(D2D_SIZE_U { width: b.w, height: b.h }, Some(b.pixels.as_ptr() as _), b.w * 4, &props)? };
            *self.backdrop_bitmap.borrow_mut() = Some(bmp);
        }
        let Some(bmp) = self.backdrop_bitmap.borrow().clone() else { return Ok(()) };
        let mut win = RECT::default();
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut win)? };
        let sc = dpi / 96.0;
        let dest = D2D_RECT_F {
            left: (b.screen.left - win.left) as f32 / sc,
            top: (b.screen.top - win.top) as f32 / sc,
            right: (b.screen.right - win.left) as f32 / sc,
            bottom: (b.screen.bottom - win.top) as f32 / sc,
        };
        unsafe { t.DrawBitmap(&bmp, Some(&dest), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
        Ok(())
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if let Some(t) = &self.target {
            unsafe {
                let _ = t.Resize(&D2D_SIZE_U { width: w, height: h });
            }
        }
    }

    fn default_text(&self) -> Rgba {
        if self.dark {
            Rgba { r: 0.95, g: 0.95, b: 0.95, a: 1.0 }
        } else {
            Rgba { r: 0.1, g: 0.1, b: 0.1, a: 1.0 }
        }
    }

    pub fn paint(&mut self, hwnd: HWND, dpi: f32, roots: &[Node], fields: Vec<Field>) -> Result<()> {
        let t = self.ensure_target(hwnd, dpi)?;
        *self.fields.borrow_mut() = fields;
        self.field_at.set(0);
        let clear = match self.tint {
            Some((dark, light)) => color(if self.dark { dark } else { light }),
            None => D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
        };
        let pick = |c: Option<(Rgba, Rgba)>| c.map(|(d, l)| if self.dark { d } else { l });
        unsafe {
            t.BeginDraw();
            if self.backdrop.is_some() {
                t.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
                self.draw_backdrop(&t, hwnd, dpi)?;
                let size = t.GetSize();
                let all = D2D_RECT_F { left: 0.0, top: 0.0, right: size.width, bottom: size.height };
                for c in [pick(self.tint), pick(self.scrim)].into_iter().flatten() {
                    let b = t.CreateSolidColorBrush(&color(c), None)?;
                    t.FillRectangle(&all, &b);
                }
            } else {
                t.Clear(Some(&clear));
            }
            for n in roots {
                self.draw(&t, n)?;
            }
            if let Some(e) = pick(self.edge) {
                // The hairline just inside the window's rounded edge.
                let size = t.GetSize();
                let b = t.CreateSolidColorBrush(&color(e), None)?;
                let r = D2D_RECT_F { left: 0.5, top: 0.5, right: size.width - 0.5, bottom: size.height - 0.5 };
                t.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: self.radius, radiusY: self.radius }, &b, 1.0, None);
            }
            if let Err(e) = t.EndDraw(None, None) {
                if e.code() == D2DERR_RECREATE_TARGET {
                    self.target = None;
                    self.bitmaps.borrow_mut().clear();
                    *self.backdrop_bitmap.borrow_mut() = None;
                } else {
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    fn draw(&self, t: &ID2D1HwndRenderTarget, n: &Node) -> Result<()> {
        let f = n.frame;
        unsafe {
            if let Some(bg) = props_color(&n.props, &["backgroundColor", "background"]).filter(|c| c.a > 0.0) {
                let b = t.CreateSolidColorBrush(&color(bg), None)?;
                let r = tishlang_ms_common::style::props_f64(&n.props, &["borderRadius", "cornerRadius"], 0.0) as f32;
                if r > 0.0 {
                    t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(f), radiusX: r, radiusY: r }, &b);
                } else {
                    t.FillRectangle(&rect(f), &b);
                }
            }
            match n.tag.as_str() {
                "text" | "button" if !n.text.is_empty() => {
                    let (pt, pr, _, pl) = tishlang_ms_common::layout::padding(&n.props);
                    let wrap = tishlang_ms_common::style::props_bool(&n.props, &["wrap", "wrapping"], false);
                    let tl = self.text_layout(&n.text, &font(&n.props), f.w - pl - pr, wrap)?;
                    let align = match tishlang_ms_common::style::props_string(&n.props, &["textAlign", "align", "textAlignment"]).as_deref() {
                        Some("center") => DWRITE_TEXT_ALIGNMENT_CENTER,
                        Some("right") | Some("end") => DWRITE_TEXT_ALIGNMENT_TRAILING,
                        _ => DWRITE_TEXT_ALIGNMENT_LEADING,
                    };
                    tl.SetTextAlignment(align)?;
                    // Clipped to its frame (DRAW_TEXT_OPTIONS_CLIP), as a view's bounds.
                    tl.SetMaxHeight(((f.h - pt) as f32).max(0.0))?;
                    if !wrap {
                        // Cut with "…" when it doesn't fit, as the Mac's labels.
                        let tf = self.format(&font(&n.props))?;
                        let sign = self.dw.CreateEllipsisTrimmingSign(&tf)?;
                        tl.SetTrimming(&DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 }, &sign)?;
                    }
                    let c = props_color(&n.props, &["color"]).unwrap_or_else(|| self.default_text());
                    let b = t.CreateSolidColorBrush(&color(c), None)?;
                    t.DrawTextLayout(Vector2 { X: (f.x + pl) as f32, Y: (f.y + pt) as f32 }, &tl, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP);
                }
                "rule" => {
                    let c = props_color(&n.props, &["color"]).unwrap_or(Rgba { r: 0.5, g: 0.5, b: 0.5, a: 0.3 });
                    let b = t.CreateSolidColorBrush(&color(c), None)?;
                    t.FillRectangle(&rect(Rect { h: 1.0, ..f }), &b);
                }
                // No source draws nothing (Moo's collapsed buttons are 1 px images without one).
                "image" if f.h > 1.0 && f.w > 1.0 && tishlang_ms_common::style::props_string(&n.props, &["src"]).is_some_and(|s| !s.is_empty()) => {
                    let s = f.h.min(f.w) as f32;
                    // Centred in its frame (an icon column is wider than the icon).
                    let left = f.x as f32 + ((f.w as f32 - s) / 2.0).max(0.0);
                    let r = D2D_RECT_F { left, top: f.y as f32, right: left + s, bottom: f.y as f32 + s };
                    let mut src = tishlang_ms_common::style::props_string(&n.props, &["src"]).unwrap_or_default();
                    // `symbol={true}`: `src` is an SF Symbol name, as tish-macos takes it.
                    // `symbol={true}`, or an AppKit named image ("NSActionTemplate").
                    if tishlang_ms_common::style::props_bool(&n.props, &["symbol", "sfSymbol"], false) || (src.starts_with("NS") && !src.contains(':')) {
                        src = super::sys::symbols::glyph(&src).unwrap_or_default();
                    }
                    if let Some(text) = src.strip_prefix("glyph:").or_else(|| src.strip_prefix("label:")) {
                        // A symbol: the glyph centred in the icon's square, in its tint.
                        let family = if src.starts_with("glyph:") { "Segoe Fluent Icons" } else { "Segoe UI Variable Text" };
                        let c = props_color(&n.props, &["tint", "color"]).unwrap_or_else(|| self.default_text());
                        let b = t.CreateSolidColorBrush(&color(c), None)?;
                        let tl = self.symbol_layout(text, family, s * 0.78, s)?;
                        t.DrawTextLayout(Vector2 { X: r.left, Y: r.top }, &tl, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    } else {
                    let tint = props_color(&n.props, &["tint"]);
                    // Bitmaps keep their aspect ratio: fit in the frame's height, centred.
                    let fit = |bmp: &ID2D1Bitmap| -> D2D_RECT_F {
                        let sz = bmp.GetSize();
                        if sz.width <= 0.0 || sz.height <= 0.0 {
                            return r;
                        }
                        let hh = s.min(f.h as f32);
                        let ww = (hh * sz.width / sz.height).min(f.w as f32);
                        let hh = ww * sz.height / sz.width;
                        let left = f.x as f32 + ((f.w as f32 - ww) / 2.0).max(0.0);
                        let top = f.y as f32 + ((f.h as f32 - hh) / 2.0).max(0.0);
                        D2D_RECT_F { left, top, right: left + ww, bottom: top + hh }
                    };
                    match self.bitmap(t, &src, s) {
                        // A tinted image is a template: its alpha, filled with the tint.
                        Some(bmp) if tint.is_some() && src.starts_with("image:") => {
                            let b = t.CreateSolidColorBrush(&color(tint.unwrap_or(self.default_text())), None)?;
                            t.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
                            t.FillOpacityMask(&bmp, &b, D2D1_OPACITY_MASK_CONTENT_GRAPHICS, Some(&fit(&bmp)), None);
                            t.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                        }
                        Some(bmp) => t.DrawBitmap(&bmp, Some(&fit(&bmp)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                        None => {
                            let b = t.CreateSolidColorBrush(&color(Rgba { r: 0.5, g: 0.5, b: 0.5, a: 0.35 }), None)?;
                            t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: s / 5.0, radiusY: s / 5.0 }, &b);
                        }
                    }
                    }
                }
                "textinput" => {
                    let i = self.field_at.get();
                    self.field_at.set(i + 1);
                    let field = self.fields.borrow().get(i).cloned().unwrap_or_default();
                    self.draw_field(t, n, &field)?;
                }
                _ => {}
            }
            for c in &n.children {
                self.draw(t, c)?;
            }
        }
        Ok(())
    }
}

impl Renderer {
    /// A borderless text field, as tish-macos's `bezeled={false}`: the text (or placeholder) on the
    /// panel, the selection, and a caret while focused. Scrolls to keep the caret in view.
    fn draw_field(&self, t: &ID2D1HwndRenderTarget, n: &Node, field: &Field) -> Result<()> {
        let f = n.frame;
        let (pt, pr, pb, pl) = tishlang_ms_common::layout::padding(&n.props);
        let fnt = font(&n.props);
        let fg = props_color(&n.props, &["color"]).unwrap_or_else(|| self.default_text());
        let empty = field.text.is_empty();
        let shown = if empty { tishlang_ms_common::style::props_string(&n.props, &["placeholder"]).unwrap_or_default() } else { field.text.clone() };
        let tl = self.text_layout(if shown.is_empty() { " " } else { &shown }, &fnt, 100_000.0, false)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { tl.GetMetrics(&mut m)? };
        let inner_w = (f.w - pl - pr).max(1.0) as f32;
        let y = (f.y + pt) as f32 + (((f.h - pt - pb) as f32 - m.height) / 2.0).max(0.0);
        let caret_x = |pos: u32| -> f32 {
            if empty {
                return 0.0;
            }
            let (mut x, mut yy) = (0.0f32, 0.0f32);
            let mut hm = DWRITE_HIT_TEST_METRICS::default();
            unsafe {
                let _ = tl.HitTestTextPosition(pos, false, &mut x, &mut yy, &mut hm);
            }
            x
        };
        let cx = caret_x(field.sel.1);
        let scroll = (cx - inner_w + 2.0).max(0.0);
        let x0 = (f.x + pl) as f32 - scroll;
        unsafe {
            t.PushAxisAlignedClip(&D2D_RECT_F { left: (f.x + pl) as f32 - 1.0, top: f.y as f32, right: (f.x + f.w - pr) as f32 + 2.0, bottom: (f.y + f.h) as f32 }, D2D1_ANTIALIAS_MODE_ALIASED);
            if field.focused && !empty && field.sel.0 != field.sel.1 {
                let (a, b) = (caret_x(field.sel.0), cx);
                let sel = Rgba { r: 0.35, g: 0.55, b: 1.0, a: 0.45 };
                let br = t.CreateSolidColorBrush(&color(sel), None)?;
                t.FillRectangle(&D2D_RECT_F { left: x0 + a.min(b), top: y, right: x0 + a.max(b), bottom: y + m.height }, &br);
            }
            let c = if empty { Rgba { a: fg.a * 0.45, ..fg } } else { fg };
            let br = t.CreateSolidColorBrush(&color(c), None)?;
            t.DrawTextLayout(Vector2 { X: x0, Y: y }, &tl, &br, D2D1_DRAW_TEXT_OPTIONS_NONE);
            if field.focused {
                let cc = props_color(&n.props, &["caretColor", "tint", "accentColor"]).unwrap_or(fg);
                let br = t.CreateSolidColorBrush(&color(cc), None)?;
                t.FillRectangle(&D2D_RECT_F { left: x0 + cx, top: y + 1.0, right: x0 + cx + 2.0, bottom: y + m.height - 1.0 }, &br);
            }
            t.PopAxisAlignedClip();
        }
        Ok(())
    }

    /// The image for `src` at about `dip` DIPs (drawn at twice that for sharp high-DPI edges):
    /// `file:<path>` is the shell's icon for a file, folder, program or shortcut;
    /// `image:<path>` an image file (PNG, ICO, JPEG, …). None when it can't be loaded.
    fn bitmap(&self, t: &ID2D1HwndRenderTarget, src: &str, dip: f32) -> Option<ID2D1Bitmap> {
        if src.is_empty() {
            return None;
        }
        let px = ((dip * 2.0).ceil() as u32).clamp(16, 256);
        let key = (src.to_string(), px);
        if let Some(b) = self.bitmaps.borrow().get(&key) {
            return Some(b.clone());
        }
        let source = {
            let cached = self.images.borrow().get(&key).cloned();
            match cached {
                Some(s) => s,
                None => {
                    let s = self.decode(src, px);
                    self.images.borrow_mut().insert(key.clone(), s.clone());
                    s
                }
            }
        }?;
        let b = unsafe { t.CreateBitmapFromWicBitmap(&source, None) }.ok()?;
        self.bitmaps.borrow_mut().insert(key, b.clone());
        Some(b)
    }

    /// `text` in `family` at `size`, centred in a `box_` × `box_` square. Segoe Fluent Icons falls
    /// back to Segoe MDL2 Assets (Windows 10) through DirectWrite's font fallback.
    fn symbol_layout(&self, text: &str, family: &str, size: f32, box_: f32) -> Result<IDWriteTextLayout> {
        let fam: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
        let locale = wide("en-us\0");
        unsafe {
            let tf = self.dw.CreateTextFormat(PCWSTR(fam.as_ptr()), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, PCWSTR(locale.as_ptr()))?;
            tf.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            tf.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let w = wide(text);
            self.dw.CreateTextLayout(&w, &tf, box_, box_)
        }
    }

    #[allow(clippy::question_mark)] // one branch per source kind reads clearer than `?` chains
    fn decode(&self, src: &str, px: u32) -> Option<IWICBitmapSource> {
        let wic = self.wic.as_ref()?;
        let raw: IWICBitmapSource = if let Some(path) = src.strip_prefix("file:") {
            unsafe {
                let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
                let item: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
                let hbmp = item.GetImage(windows::Win32::Foundation::SIZE { cx: px as i32, cy: px as i32 }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()?;
                let bmp = wic.CreateBitmapFromHBITMAP(hbmp, windows::Win32::Graphics::Gdi::HPALETTE::default(), WICBitmapUsePremultipliedAlpha);
                let _ = DeleteObject(hbmp.into());
                bmp.ok()?.cast().ok()?
            }
        } else if let Some(path) = src.strip_prefix("image:") {
            unsafe {
                let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
                let dec = wic.CreateDecoderFromFilename(PCWSTR(w.as_ptr()), None, windows::Win32::Foundation::GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
                dec.GetFrame(0).ok()?.cast().ok()?
            }
        } else {
            return None;
        };
        // Direct2D wants premultiplied BGRA.
        unsafe {
            let conv = wic.CreateFormatConverter().ok()?;
            conv.Initialize(&raw, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut).ok()?;
            conv.cast().ok()
        }
    }
}

impl Measure for Renderer {
    fn text(&self, text: &str, font: &Font, max_w: f64, wrap: bool) -> (f64, f64) {
        let probe = if text.is_empty() { " " } else { text };
        let Ok(tl) = self.text_layout(probe, font, max_w, wrap) else { return (0.0, font.size * 1.3) };
        let mut m = DWRITE_TEXT_METRICS::default();
        if unsafe { tl.GetMetrics(&mut m) }.is_err() {
            return (0.0, font.size * 1.3);
        }
        (m.widthIncludingTrailingWhitespace as f64, m.height.ceil() as f64)
    }
}
