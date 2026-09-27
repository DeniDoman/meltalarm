//! Pure CPU painting shared by the tray glyph (runtime) and the app icon (build.rs, which
//! includes this file). No dependencies beyond `std`.
#![allow(clippy::too_many_arguments)]

pub struct Canvas {
    size: i32,
    px: Vec<[f32; 4]>, // straight-alpha RGBA, composited "over"
}

impl Canvas {
    pub fn new(size: i32) -> Self {
        Canvas { size, px: vec![[0.0; 4]; (size * size) as usize] }
    }

    /// Rounded rectangle with 4×4 supersampled coverage. `hollow` draws a `stroke`-wide outline.
    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32, alpha: f32, hollow: Option<f32>) {
        let inside = |px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32| -> bool {
            if px < x || py < y || px > x + w || py > y + h {
                return false;
            }
            let cx = px.clamp(x + r, x + w - r);
            let cy = py.clamp(y + r, y + h - r);
            (px - cx).powi(2) + (py - cy).powi(2) <= r * r
        };
        let c = [((rgb >> 16) & 255) as f32 / 255.0, ((rgb >> 8) & 255) as f32 / 255.0, (rgb & 255) as f32 / 255.0];
        for py in 0..self.size {
            for px in 0..self.size {
                let mut cov = 0.0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let (fx, fy) = (px as f32 + (sx as f32 + 0.5) / 4.0, py as f32 + (sy as f32 + 0.5) / 4.0);
                        let mut on = inside(fx, fy, x, y, w, h, r);
                        if let (true, Some(s)) = (on, hollow) {
                            on = !inside(fx, fy, x + s, y + s, w - 2.0 * s, h - 2.0 * s, (r - s).max(0.0));
                        }
                        if on {
                            cov += 1.0 / 16.0;
                        }
                    }
                }
                let a = cov * alpha;
                if a <= 0.0 {
                    continue;
                }
                let d = &mut self.px[(py * self.size + px) as usize];
                let out_a = a + d[3] * (1.0 - a);
                for i in 0..3 {
                    d[i] = (c[i] * a + d[i] * d[3] * (1.0 - a)) / out_a;
                }
                d[3] = out_a;
            }
        }
    }

    pub fn argb(&self) -> Vec<u32> {
        self.px
            .iter()
            .map(|p| {
                let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
                (b(p[3]) << 24) | (b(p[0]) << 16) | (b(p[1]) << 8) | b(p[2])
            })
            .collect()
    }
}

#[allow(dead_code)] // used by build.rs
/// The app icon (docs/DESIGN.md "App icon"): the face of a 12V-2x6 plug (housing, latch,
/// six pins) in the neutral tone on a dark rounded tile. Below 24 px only the six pins
/// remain, like the tray glyph. Laid out per size so edges land on whole pixels.
pub fn app_icon(size: i32) -> Vec<u32> {
    const TILE: u32 = 0x1F1F1F;
    const INK: u32 = 0xF2F2F2;
    let s = size as f32;
    let px = |f: f32| (s * f).round().max(1.0);
    let mut cv = Canvas::new(size);
    cv.rrect(0.0, 0.0, s, s, s * 0.2, TILE, 1.0, None);
    if size < 24 {
        let (cell, gap) = (px(0.19).max(3.0), px(0.06));
        let (w, h) = (3.0 * cell + 2.0 * gap, 2.0 * cell + gap);
        let (ox, oy) = (((s - w) / 2.0).floor(), ((s - h) / 2.0).floor());
        for i in 0..6 {
            let (x, y) = (ox + (i % 3) as f32 * (cell + gap), oy + (i / 3) as f32 * (cell + gap));
            cv.rrect(x, y, cell, cell, 0.75, INK, 1.0, None);
        }
        return cv.argb();
    }
    cv.rrect(0.0, 0.0, s, s, s * 0.2, 0xFFFFFF, 0.12, Some(1.0));
    let stroke = px(0.055);
    let (pad, gap) = (px(0.05), px(0.05));
    let cell = px(0.13);
    let inner_w = 3.0 * cell + 2.0 * gap;
    let inner_h = 2.0 * cell + gap;
    let (hw, hh) = (inner_w + 2.0 * (pad + stroke), inner_h + 2.0 * (pad + stroke));
    let (lw, lh) = (px(0.24), px(0.08));
    let hx = ((s - hw) / 2.0).floor();
    let hy = ((s - hh - lh + stroke) / 2.0).floor() + lh - stroke;
    // Latch on top, then the housing outline, then the pins.
    cv.rrect(((s - lw) / 2.0).floor(), hy - lh + stroke, lw, lh, (lh * 0.3).max(0.5), INK, 1.0, None);
    cv.rrect(hx, hy, hw, hh, (s * 0.07).max(1.5), INK, 1.0, Some(stroke));
    let (ox, oy) = (hx + stroke + pad, hy + stroke + pad);
    for i in 0..6 {
        let (x, y) = (ox + (i % 3) as f32 * (cell + gap), oy + (i / 3) as f32 * (cell + gap));
        cv.rrect(x, y, cell, cell, (cell * 0.18).max(0.75), INK, 1.0, None);
    }
    cv.argb()
}
