//! The tray glyph (docs/DESIGN.md "Tray icon"): 6 squares, 2 rows of 3, drawn on the CPU
//! into an ARGB buffer at the exact tray icon size.

use meltalarm_core::{ConnectorView, Glyph, Level, WireView};

use crate::paint::Canvas;
use crate::palette::{ALARM_RED, CAUTION_DARK, CAUTION_LIGHT, WARNING_DARK, WARNING_LIGHT};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

/// The overloaded or flagged wire, knocked out of the white squares on the red alarm tile.
const KNOCKOUT: u32 = 0x5A0010;

pub struct Palette {
    pub neutral: u32,
    pub caution: u32,
    pub warning: u32,
}

pub fn palette(light_taskbar: bool) -> Palette {
    if light_taskbar {
        Palette { neutral: 0x1B1B1B, caution: CAUTION_LIGHT, warning: WARNING_LIGHT }
    } else {
        Palette { neutral: 0xF2F2F2, caution: CAUTION_DARK, warning: WARNING_DARK }
    }
}

/// Render the glyph for one connector. `blink_on` selects frame A (red tile) of the alarm blink.
pub fn render(view: &ConnectorView, size: i32, light: bool, blink_on: bool) -> Vec<u32> {
    draw(view.glyph, &view.wires, view.attention, size, light, blink_on)
}

/// The hollow placeholder shown while MeltAlarm is still connecting to the PSU (Spec §5.1).
pub fn render_placeholder(size: i32, light: bool) -> Vec<u32> {
    draw(Glyph::NoData, &[WireView::default(); 6], false, size, light, false)
}

fn draw(glyph: Glyph, wires: &[WireView; 6], attention: bool, size: i32, light: bool, blink_on: bool) -> Vec<u32> {
    let p = palette(light);
    let s = size as f32;
    let cell = (size * 4 / 16) as f32;
    let gap = (size / 12).max(1) as f32;
    let glyph_w = 3.0 * cell + 2.0 * gap;
    let glyph_h = 2.0 * cell + gap;
    let marker_h = (size / 8).max(2) as f32;
    let ox = ((s - glyph_w) / 2.0).floor();
    let tile = glyph == Glyph::Alarm && blink_on;
    let oy = if tile { ((s - glyph_h) / 2.0).floor() } else { ((s - glyph_h - marker_h - 1.0) / 2.0).floor() };
    let radius = (size as f32 / 16.0) * 0.75;

    let mut cv = Canvas::new(size);
    if tile {
        cv.rrect(0.0, 0.0, s, s, s / 5.0, ALARM_RED, 1.0, None);
    }
    for (i, w) in wires.iter().enumerate() {
        let x = ox + (i % 3) as f32 * (cell + gap);
        let y = oy + (i / 3) as f32 * (cell + gap);
        let level_color = match w.level {
            Level::Normal => p.neutral,
            Level::Caution => p.caution,
            Level::Warning => p.warning,
        };
        match glyph {
            _ if tile => cv.rrect(x, y, cell, cell, radius, if w.flagged { KNOCKOUT } else { 0xFFFFFF }, 1.0, None),
            Glyph::NoData => cv.rrect(x, y, cell, cell, radius, p.neutral, 0.7, Some((s / 16.0).max(1.0))),
            Glyph::NotConnected => cv.rrect(x, y, cell, cell, radius, p.neutral, 0.28, None),
            _ => cv.rrect(x, y, cell, cell, radius, level_color, 1.0, None),
        }
    }
    if attention && !tile {
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
