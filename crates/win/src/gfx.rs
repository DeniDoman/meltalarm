//! Direct2D + DirectWrite in **software** mode (no GPU driver in the process) drawing into
//! per-pixel-alpha layered windows. Coordinates are DIPs; the render target DPI scales them.

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};
use windows::core::{Result, w};
use windows_numerics::Vector2;

use crate::sys::utf16;

pub const UI_FONT: &str = "Segoe UI Variable Text";
pub const NUM_FONT: &str = "Bahnschrift";

#[derive(Clone, Copy)]
pub struct Font {
    pub family: &'static str,
    pub size: f32,
    pub weight: i32,
}

pub const fn ui(size: f32, weight: i32) -> Font {
    Font { family: UI_FONT, size, weight }
}

pub const fn num(size: f32, weight: i32) -> Font {
    Font { family: NUM_FONT, size, weight }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        // SAFETY: standard factory creation.
        unsafe {
            Ok(Gfx {
                d2d: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                dw: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
            })
        }
    }

    fn format(&self, f: Font, align: Align, wrap: bool, vcenter: bool) -> Result<IDWriteTextFormat> {
        let family = crate::sys::wide(f.family);
        // SAFETY: valid NUL-terminated strings.
        unsafe {
            let tf = self.dw.CreateTextFormat(
                windows::core::PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT(f.weight),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                f.size,
                w!("en-us"),
            )?;
            tf.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            })?;
            tf.SetParagraphAlignment(if vcenter { DWRITE_PARAGRAPH_ALIGNMENT_CENTER } else { DWRITE_PARAGRAPH_ALIGNMENT_NEAR })?;
            tf.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP })?;
            Ok(tf)
        }
    }

    /// Height (DIP) of wrapped text at `max_w` DIP.
    pub fn text_height(&self, s: &str, f: Font, max_w: f32) -> f32 {
        let text = utf16(s);
        // SAFETY: plain DirectWrite layout measurement.
        unsafe {
            let Ok(tf) = self.format(f, Align::Left, true, false) else { return f.size * 1.4 };
            let Ok(layout) = self.dw.CreateTextLayout(&text, &tf, max_w, 10_000.0) else { return f.size * 1.4 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() { m.height } else { f.size * 1.4 }
        }
    }

    /// Width (DIP) of single-line text.
    pub fn text_width(&self, s: &str, f: Font) -> f32 {
        let text = utf16(s);
        // SAFETY: as above.
        unsafe {
            let Ok(tf) = self.format(f, Align::Left, false, false) else { return 0.0 };
            let Ok(layout) = self.dw.CreateTextLayout(&text, &tf, 10_000.0, 1_000.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() { m.widthIncludingTrailingWhitespace } else { 0.0 }
        }
    }

    /// Render `w_dip × h_dip` at `scale` into the layered window `hwnd` at screen position (x, y) px.
    pub fn present(&self, hwnd: HWND, x: i32, y: i32, w_dip: f32, h_dip: f32, scale: f32, draw: impl FnOnce(&Painter)) -> Result<()> {
        let frame = self.render(w_dip, h_dip, scale, draw)?;
        frame.show(hwnd, (x, y), (0, 0, frame.w, frame.h), 255)
    }

    /// Render `w_dip × h_dip` at `scale` into a frame that can be shown, in part or translucent,
    /// as often as needed (DESIGN.md "Motion": a transition shows one frame, never redraws it).
    pub fn render(&self, w_dip: f32, h_dip: f32, scale: f32, draw: impl FnOnce(&Painter)) -> Result<Frame> {
        let (w, h) = ((w_dip * scale).ceil() as i32, (h_dip * scale).ceil() as i32);
        // SAFETY: GDI objects are created here and owned by the frame, which releases them on drop.
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let dib = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(dib) => dib,
                Err(e) => {
                    let _ = DeleteDC(mem);
                    return Err(e);
                }
            };
            let old = SelectObject(mem, dib.into());
            let frame = Frame { mem, dib, old, w, h };
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt: ID2D1DCRenderTarget = self.d2d.CreateDCRenderTarget(&props)?;
            rt.BindDC(mem, &RECT { left: 0, top: 0, right: w, bottom: h })?;
            rt.SetDpi(96.0 * scale, 96.0 * scale);
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            draw(&Painter { gfx: self, rt: &rt });
            rt.EndDraw(None, None)?;
            Ok(frame)
        }
    }
}

/// A rendered picture for a layered window (premultiplied BGRA), kept while it is on screen or
/// moving, released on drop.
pub struct Frame {
    mem: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    /// Size in pixels.
    pub w: i32,
    pub h: i32,
}

impl Frame {
    /// Show the part `src` (x, y, w, h in frame pixels) of this frame as the whole layered window
    /// `hwnd`, its top-left at screen pixel `dst`, with a constant `alpha` on top of the per-pixel one.
    pub fn show(&self, hwnd: HWND, dst: (i32, i32), src: (i32, i32, i32, i32), alpha: u8) -> Result<()> {
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        // SAFETY: the memory DC and its bitmap live as long as `self`.
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&POINT { x: dst.0, y: dst.1 }),
                Some(&SIZE { cx: src.2.max(1), cy: src.3.max(1) }),
                Some(self.mem),
                Some(&POINT { x: src.0, y: src.1 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: we created both objects; the bitmap is deselected before it is deleted.
        unsafe {
            SelectObject(self.mem, self.old);
            let _ = DeleteObject(self.dib.into());
            let _ = DeleteDC(self.mem);
        }
    }
}

pub struct Painter<'a> {
    gfx: &'a Gfx,
    rt: &'a ID2D1DCRenderTarget,
}

fn color(rgb: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((rgb >> 16) & 255) as f32 / 255.0,
        g: ((rgb >> 8) & 255) as f32 / 255.0,
        b: (rgb & 255) as f32 / 255.0,
        a,
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

impl Painter<'_> {
    fn brush(&self, rgb: u32, a: f32) -> Option<ID2D1SolidColorBrush> {
        // SAFETY: valid render target.
        unsafe { self.rt.CreateSolidColorBrush(&color(rgb, a), None).ok() }
    }

    pub fn fill_rrect(&self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32, a: f32) {
        if let Some(b) = self.brush(rgb, a) {
            let rr = D2D1_ROUNDED_RECT { rect: rect(x, y, w, h), radiusX: r, radiusY: r };
            // SAFETY: drawing between BeginDraw/EndDraw.
            unsafe { self.rt.FillRoundedRectangle(&rr, &b) };
        }
    }

    /// Rounded only at the bottom corners (the alarm notch fused to the top edge).
    pub fn fill_notch(&self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32, a: f32) {
        self.fill_rrect(x, y, w, h, r, rgb, a);
        self.fill_rect(x, y, w, r.min(h), rgb, a);
    }

    /// A rectangle rounded only at the chosen ends (top and/or bottom), drawn once (no
    /// overlapping halves, so translucent fills stay even): a taller rounded rect, clipped.
    pub fn fill_rounded_ends(&self, x: f32, y: f32, w: f32, h: f32, r: f32, top: bool, bottom: bool, rgb: u32, a: f32) {
        if h <= 0.0 {
            return;
        }
        let ext_top = if top { 0.0 } else { r };
        let ext_bottom = if bottom { 0.0 } else { r };
        // SAFETY: a clip pushed and popped within the same draw.
        unsafe { self.rt.PushAxisAlignedClip(&rect(x, y, w, h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
        self.fill_rrect(x, y - ext_top, w, h + ext_top + ext_bottom, r.min((h + ext_top + ext_bottom) / 2.0), rgb, a);
        // SAFETY: matches the push above.
        unsafe { self.rt.PopAxisAlignedClip() };
    }

    pub fn stroke_rrect(&self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32, a: f32, width: f32) {
        if let Some(b) = self.brush(rgb, a) {
            let rr = D2D1_ROUNDED_RECT { rect: rect(x, y, w, h), radiusX: r, radiusY: r };
            // SAFETY: as above.
            unsafe { self.rt.DrawRoundedRectangle(&rr, &b, width, None) };
        }
    }

    pub fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32, rgb: u32, a: f32) {
        if let Some(b) = self.brush(rgb, a) {
            // SAFETY: as above.
            unsafe { self.rt.FillRectangle(&rect(x, y, w, h), &b) };
        }
    }

    pub fn circle(&self, cx: f32, cy: f32, r: f32, rgb: u32, a: f32) {
        if let Some(b) = self.brush(rgb, a) {
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
            // SAFETY: as above.
            unsafe { self.rt.FillEllipse(&e, &b) };
        }
    }

    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, rgb: u32, a: f32, width: f32) {
        if let Some(b) = self.brush(rgb, a) {
            // SAFETY: as above.
            unsafe { self.rt.DrawLine(Vector2 { X: x1, Y: y1 }, Vector2 { X: x2, Y: y2 }, &b, width, None) };
        }
    }

    pub fn text(&self, s: &str, f: Font, x: f32, y: f32, w: f32, h: f32, rgb: u32, a: f32, align: Align) {
        self.text_impl(s, f, x, y, w, h, rgb, a, align, false, true);
    }

    /// Wrapped text from the top of the box.
    pub fn para(&self, s: &str, f: Font, x: f32, y: f32, w: f32, h: f32, rgb: u32, a: f32) {
        self.text_impl(s, f, x, y, w, h, rgb, a, Align::Left, true, false);
    }

    #[allow(clippy::too_many_arguments)]
    fn text_impl(&self, s: &str, f: Font, x: f32, y: f32, w: f32, h: f32, rgb: u32, a: f32, align: Align, wrap: bool, vcenter: bool) {
        let (Ok(tf), Some(b)) = (self.gfx.format(f, align, wrap, vcenter), self.brush(rgb, a)) else { return };
        let text = utf16(s);
        // SAFETY: as above.
        unsafe {
            self.rt.DrawText(&text, &tf, &rect(x, y, w, h), &b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    pub fn gfx(&self) -> &Gfx {
        self.gfx
    }
}

/// Soft drop shadow approximated by stacked translucent rounded rects.
pub fn shadow(p: &Painter, x: f32, y: f32, w: f32, h: f32, r: f32, spread: f32, strength: f32) {
    let steps = 8;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let grow = spread * (1.0 - t);
        p.fill_rrect(x - grow, y - grow + spread * 0.35, w + 2.0 * grow, h + 2.0 * grow, r + grow, 0x000000, strength / steps as f32);
    }
}
