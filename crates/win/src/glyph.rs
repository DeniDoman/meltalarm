//! The tray glyph (docs/DESIGN.md "Tray icon"): 6 squares, 2 rows of 3, drawn on the CPU
//! into an ARGB buffer at the exact tray icon size.

use meltalarm_core::{ConnectorView, Glyph, Level};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

pub const RED_TILE: u32 = 0xC8102E;
const KNOCKOUT: u32 = 0x5A0010;

pub struct Palette {
    pub neutral: u32,
    pub caution: u32,
    pub warning: u32,
}

pub fn palette(light_taskbar: bool) -> Palette {
    if light_taskbar {
        Palette { neutral: 0x1B1B1B, caution: 0xB86E00, warning: 0xD1242F }
    } else {
        Palette { neutral: 0xF2F2F2, caution: 0xF5A623, warning: 0xFF4D4F }
    }
}

struct Canvas {
    size: i32,
    px: Vec<[f32; 4]>, // straight-alpha RGBA, composited "over"
}

impl Canvas {
    fn new(size: i32) -> Self {
        Canvas { size, px: vec![[0.0; 4]; (size * size) as usize] }
    }

    /// Rounded rectangle with 4×4 supersampled coverage. `hollow` draws a `stroke`-wide outline.
    fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32, alpha: f32, hollow: Option<f32>) {
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

    fn argb(&self) -> Vec<u32> {
        self.px
            .iter()
            .map(|p| {
                let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
                (b(p[3]) << 24) | (b(p[0]) << 16) | (b(p[1]) << 8) | b(p[2])
            })
            .collect()
    }
}

/// Render the glyph for one connector. `blink_on` selects frame A (red tile) of the alarm blink.
pub fn render(view: &ConnectorView, size: i32, light: bool, blink_on: bool) -> Vec<u32> {
    let p = palette(light);
    let s = size as f32;
    let cell = (size * 4 / 16) as f32;
    let gap = (size / 12).max(1) as f32;
    let glyph_w = 3.0 * cell + 2.0 * gap;
    let glyph_h = 2.0 * cell + gap;
    let marker_h = (size / 8).max(2) as f32;
    let ox = ((s - glyph_w) / 2.0).floor();
    let tile = view.glyph == Glyph::Alarm && blink_on;
    let oy = if tile { ((s - glyph_h) / 2.0).floor() } else { ((s - glyph_h - marker_h - 1.0) / 2.0).floor() };
    let radius = (size as f32 / 16.0) * 0.75;

    let mut cv = Canvas::new(size);
    if tile {
        cv.rrect(0.0, 0.0, s, s, s / 5.0, RED_TILE, 1.0, None);
    }
    for (i, w) in view.wires.iter().enumerate() {
        let x = ox + (i % 3) as f32 * (cell + gap);
        let y = oy + (i / 3) as f32 * (cell + gap);
        let level_color = match w.level {
            Level::Normal => p.neutral,
            Level::Caution => p.caution,
            Level::Warning => p.warning,
        };
        match view.glyph {
            _ if tile => cv.rrect(x, y, cell, cell, radius, if w.flagged { KNOCKOUT } else { 0xFFFFFF }, 1.0, None),
            Glyph::NoData => cv.rrect(x, y, cell, cell, radius, p.neutral, 0.7, Some((s / 16.0).max(1.0))),
            Glyph::NotConnected => cv.rrect(x, y, cell, cell, radius, p.neutral, 0.28, None),
            _ => cv.rrect(x, y, cell, cell, radius, level_color, 1.0, None),
        }
    }
    if view.attention && !tile {
        let mw = 2.0 * cell;
        cv.rrect(((s - mw) / 2.0).floor(), oy + glyph_h + gap + 1.0, mw, marker_h, marker_h / 2.0, p.caution, 1.0, None);
    }
    cv.argb()
}

pub fn to_hicon(argb: &[u32], size: i32) -> Option<HICON> {
    // SAFETY: buffers are sized size×size×4 bytes (color) and size×size bits (mask) and
    // outlive the calls; bitmaps are deleted after the icon copies them.
    unsafe {
        let color = CreateBitmap(size, size, 1, 32, Some(argb.as_ptr() as *const _));
        let mask_bits = vec![0u8; ((size + 15) / 16 * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const _));
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
