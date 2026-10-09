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
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

pub struct Renderer {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    formats: RefCell<HashMap<(u32, u16), IDWriteTextFormat>>,
    pub dark: bool,
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
            Ok(Self { d2d, dw, target: None, formats: RefCell::new(HashMap::new()), dark })
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

    pub fn paint(&mut self, hwnd: HWND, dpi: f32, roots: &[Node]) -> Result<()> {
        let t = self.ensure_target(hwnd, dpi)?;
        unsafe {
            t.BeginDraw();
            t.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            for n in roots {
                self.draw(&t, n)?;
            }
            if let Err(e) = t.EndDraw(None, None) {
                if e.code() == D2DERR_RECREATE_TARGET {
                    self.target = None;
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
                    let tl = self.text_layout(&n.text, &font(&n.props), f.w - pl - pr, true)?;
                    let c = props_color(&n.props, &["color"]).unwrap_or_else(|| self.default_text());
                    let b = t.CreateSolidColorBrush(&color(c), None)?;
                    t.DrawTextLayout(Vector2 { X: (f.x + pl) as f32, Y: (f.y + pt) as f32 }, &tl, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP);
                }
                "rule" => {
                    let c = props_color(&n.props, &["color"]).unwrap_or(Rgba { r: 0.5, g: 0.5, b: 0.5, a: 0.3 });
                    let b = t.CreateSolidColorBrush(&color(c), None)?;
                    t.FillRectangle(&rect(Rect { h: 1.0, ..f }), &b);
                }
                "image" if f.h > 1.0 => {
                    // Proof of concept: a placeholder until WIC icon loading lands.
                    let b = t.CreateSolidColorBrush(&color(Rgba { r: 0.5, g: 0.5, b: 0.5, a: 0.35 }), None)?;
                    let s = f.h.min(f.w) as f32;
                    let r = D2D_RECT_F { left: f.x as f32, top: f.y as f32, right: f.x as f32 + s, bottom: f.y as f32 + s };
                    t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: s / 5.0, radiusY: s / 5.0 }, &b);
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
