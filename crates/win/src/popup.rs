//! Status popup above the tray icon (docs/DESIGN.md "Popup").

use std::time::{Duration, Instant};

use meltalarm_core::{ConnectorView, Level, NoteKind, StatusKind, ViewModel};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, HWND_TOPMOST, SW_HIDE, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos, ShowWindow,
};

use crate::gfx::{self, Align, Gfx, Painter, num, ui};

const W: f32 = 360.0;
const PAD: f32 = 20.0;
const MARGIN: f32 = 16.0; // room for the shadow
const BAR_H: f32 = 96.0;

struct Theme {
    bg: u32,
    border: u32,
    fg: u32,
    fg3: u32,
    track: (u32, f32),
    ok: u32,
    caution: u32,
    warn: u32,
    caution_text: u32,
    warn_text: u32,
    note_caution: (u32, f32, u32),
    note_info: (u32, f32, u32),
}

fn theme(light: bool) -> Theme {
    if light {
        Theme {
            bg: 0xF9F9F9,
            border: 0xE0E0E0,
            fg: 0x1A1A1A,
            fg3: 0x5E5E5E,
            track: (0x000000, 0.08),
            ok: 0x3A3A3A,
            caution: 0xB86E00,
            warn: 0xD1242F,
            caution_text: 0x8F5600,
            warn_text: 0xB81F29,
            note_caution: (0xB86E00, 0.12, 0x6E4200),
            note_info: (0x000000, 0.05, 0x3A3A3A),
        }
    } else {
        Theme {
            bg: 0x2B2B2B,
            border: 0x3A3A3A,
            fg: 0xF2F2F2,
            fg3: 0xA8A8A8,
            track: (0xFFFFFF, 0.09),
            ok: 0xE6E6E6,
            caution: 0xF5A623,
            warn: 0xFF4D4F,
            caution_text: 0xF5A623,
            warn_text: 0xFF6B6D,
            note_caution: (0xF5A623, 0.14, 0xF7C878),
            note_info: (0xFFFFFF, 0.07, 0xD0D0D0),
        }
    }
}

pub struct Popup {
    pub hwnd: HWND,
    pub connector: Option<usize>,
    anchor: Option<RECT>,
    hidden_at: Option<(usize, Instant)>,
}

impl Popup {
    pub fn new(hwnd: HWND) -> Self {
        Popup { hwnd, connector: None, anchor: None, hidden_at: None }
    }

    /// Tray click: open for `index`, or close if it is already open for it.
    pub fn toggle(&mut self, index: usize, anchor: Option<RECT>, gfx: &Gfx, view: &ViewModel, light: bool) {
        // A click on the icon first deactivates (hides) the popup; don't reopen it right away.
        if self.hidden_at.is_some_and(|(i, t)| i == index && t.elapsed() < Duration::from_millis(400)) {
            self.hidden_at = None;
            return;
        }
        if self.connector == Some(index) {
            self.hide();
            return;
        }
        self.connector = Some(index);
        // Freeze the anchor at open time (the cursor may move while the popup stays).
        self.anchor = anchor.or_else(|| {
            let mut p = POINT::default();
            // SAFETY: valid out pointer.
            unsafe { let _ = GetCursorPos(&mut p); }
            Some(RECT { left: p.x, top: p.y, right: p.x, bottom: p.y })
        });
        self.draw(gfx, view, light);
        // SAFETY: our own window; tray clicks grant foreground rights.
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            let _ = ShowWindow(self.hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn hide(&mut self) {
        if let Some(i) = self.connector.take() {
            self.hidden_at = Some((i, Instant::now()));
            // SAFETY: our own window.
            unsafe { let _ = ShowWindow(self.hwnd, SW_HIDE); }
        }
    }

    pub fn update(&mut self, gfx: &Gfx, view: &ViewModel, light: bool) {
        if self.connector.is_some() {
            self.draw(gfx, view, light);
        }
    }

    fn draw(&mut self, gfx: &Gfx, view: &ViewModel, light: bool) {
        let Some(c) = self.connector.and_then(|i| view.connectors.get(i)) else {
            self.hide();
            return;
        };
        let t = theme(light);
        let h = content_height(gfx, c);
        let (card_w, card_h) = (W, h);
        let (win_w, win_h) = (card_w + 2.0 * MARGIN, card_h + 2.0 * MARGIN);

        // Position: above the icon, inside the work area of its monitor.
        let anchor = self.anchor.unwrap_or_else(|| {
            let mut p = POINT::default();
            // SAFETY: valid out pointer.
            unsafe { let _ = GetCursorPos(&mut p); }
            RECT { left: p.x, top: p.y, right: p.x, bottom: p.y }
        });
        let center = POINT { x: (anchor.left + anchor.right) / 2, y: (anchor.top + anchor.bottom) / 2 };
        // SAFETY: plain monitor queries with valid out structs.
        let (work, scale) = unsafe {
            let mon = MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            (mi.rcWork, dx as f32 / 96.0)
        };
        let (pw, ph) = ((win_w * scale) as i32, (win_h * scale) as i32);
        let gap = (4.0 * scale) as i32;
        let x = (center.x - pw / 2).clamp(work.left, (work.right - pw).max(work.left));
        let y = if anchor.top >= work.bottom - 2 {
            work.bottom - ph - gap // taskbar at the bottom
        } else if anchor.bottom <= work.top + 2 {
            work.top + gap // taskbar at the top
        } else {
            (anchor.top - ph - gap).clamp(work.top, (work.bottom - ph).max(work.top))
        };

        let _ = gfx.present(self.hwnd, x, y, win_w, win_h, scale, |p| {
            let (cx, cy) = (MARGIN, MARGIN);
            gfx::shadow(p, cx, cy, card_w, card_h, 8.0, 12.0, 0.35);
            p.fill_rrect(cx, cy, card_w, card_h, 8.0, t.bg, 1.0);
            p.stroke_rrect(cx + 0.5, cy + 0.5, card_w - 1.0, card_h - 1.0, 8.0, t.border, 1.0, 1.0);
            draw_content(p, &t, c, cx + PAD, cy + 18.0, card_w - 2.0 * PAD);
        });
    }
}

fn notes_height(gfx: &Gfx, c: &ConnectorView) -> f32 {
    c.notes.iter().map(|n| gfx.text_height(&n.text, ui(13.0, 400), W - 2.0 * PAD - 24.0) + 18.0 + 10.0).sum()
}

fn content_height(gfx: &Gfx, c: &ConnectorView) -> f32 {
    let mut h = 18.0 + 24.0 + 14.0; // top pad, header, gap
    if c.alarm_reason.is_some() {
        h += 36.0 + 14.0;
    }
    h += 16.0 + 10.0 + BAR_H + 6.0 + 20.0 + 16.0 + 14.0; // label, bars, values, wire numbers
    h += notes_height(gfx, c);
    h + 1.0 + 12.0 + 36.0 + 16.0 // divider, stats, bottom pad
}

fn level_colors(t: &Theme, l: Level) -> (u32, u32) {
    match l {
        Level::Normal => (t.ok, t.fg),
        Level::Caution => (t.caution, t.caution_text),
        Level::Warning => (t.warn, t.warn_text),
    }
}

fn draw_content(p: &Painter, t: &Theme, c: &ConnectorView, x: f32, mut y: f32, w: f32) {
    // Header: connector name + status chip.
    p.text(&c.label, ui(15.0, 600), x, y, w, 24.0, t.fg, 1.0, Align::Left);
    let chip_font = ui(12.0, 600);
    let chip_w = p.gfx().text_width(&c.status_text, chip_font) + 20.0 + 13.0;
    let (chip_bg, chip_a, chip_fg, dot) = match c.status_kind {
        StatusKind::Alarm => (0xC8102E, 1.0, 0xFFFFFF, Some(0xFFFFFF)),
        StatusKind::NoData => (t.track.0, t.track.1, t.fg3, None),
        StatusKind::Normal => (t.track.0, t.track.1, t.fg, Some(t.ok)),
    };
    let chip_x = x + w - chip_w;
    p.fill_rrect(chip_x, y, chip_w, 24.0, 12.0, chip_bg, chip_a);
    if let Some(d) = dot {
        p.circle(chip_x + 13.5, y + 12.0, 3.5, d, 1.0);
    }
    p.text(&c.status_text, chip_font, chip_x + 22.0, y, chip_w - 26.0, 24.0, chip_fg, 1.0, Align::Left);
    y += 24.0 + 14.0;

    if let Some(reason) = &c.alarm_reason {
        p.fill_rrect(x, y, w, 36.0, 6.0, 0xC8102E, 1.0);
        p.text(reason, ui(13.0, 600), x + 12.0, y, w - 24.0, 36.0, 0xFFFFFF, 1.0, Align::Left);
        if let Some(cd) = &c.countdown {
            p.text(&format!("Power cut in {cd}"), num(15.0, 600), x + 12.0, y, w - 24.0, 36.0, 0xFFFFFF, 1.0, Align::Right);
        }
        y += 36.0 + 14.0;
    }

    p.text("Current per wire, A", ui(12.0, 400), x, y, w, 16.0, t.fg3, 1.0, Align::Left);
    y += 16.0 + 10.0;

    // Bars: full scale = the PSU's wire limit; dashed line at the caution level.
    let label_w = 30.0;
    let area_w = w - label_w - 6.0;
    let col = area_w / 6.0;
    let stale = if c.stale { 0.45 } else { 1.0 };
    if let Some(limit) = c.bar_limit {
        p.line(x, y, x + area_w, y, t.warn, 0.8, 1.0);
        p.text(&trim(limit), num(10.0, 400), x + area_w + 6.0, y - 7.0, label_w, 14.0, t.warn, 1.0, Align::Left);
        if let Some(cl) = c.caution_line {
            let ly = y + BAR_H * (1.0 - cl / limit);
            p.dashed_hline(x, x + area_w, ly, t.caution, 0.7);
            p.text(&trim(cl), num(10.0, 400), x + area_w + 6.0, ly - 7.0, label_w, 14.0, t.caution, 1.0, Align::Left);
        }
    }
    for (i, wv) in c.wires.iter().enumerate() {
        let cx = x + col * i as f32 + col / 2.0;
        let (bar, txt) = level_colors(t, wv.level);
        if wv.amps.is_some() {
            p.fill_rrect(cx - 7.0, y, 14.0, BAR_H, 7.0, t.track.0, t.track.1);
        } else {
            p.stroke_rrect(cx - 7.0, y + 0.5, 14.0, BAR_H - 1.0, 7.0, t.fg3, 0.6, 1.0);
        }
        if let (Some(a), Some(limit)) = (wv.amps, c.bar_limit) {
            let fill = (a / limit).clamp(0.0, 1.0) * BAR_H;
            if fill > 0.5 {
                p.fill_rrect(cx - 7.0, y + BAR_H - fill, 14.0, fill, 7.0_f32.min(fill / 2.0), bar, stale);
            }
        }
        let value = wv.amps.map_or("—".to_owned(), |a| format!("{a:.1}"));
        let vcolor = if wv.amps.is_some() { txt } else { t.fg3 };
        p.text(&value, num(16.0, 600), cx - col / 2.0, y + BAR_H + 6.0, col, 20.0, vcolor, stale, Align::Center);
        p.text(&(i + 1).to_string(), ui(11.0, 400), cx - col / 2.0, y + BAR_H + 26.0, col, 16.0, t.fg3, 1.0, Align::Center);
    }
    y += BAR_H + 6.0 + 20.0 + 16.0 + 14.0;

    for n in &c.notes {
        let (bg, a, fg) = if n.kind == NoteKind::Caution { t.note_caution } else { t.note_info };
        let th = p.gfx().text_height(&n.text, ui(13.0, 400), w - 24.0);
        p.fill_rrect(x, y, w, th + 18.0, 6.0, bg, a);
        p.para(&n.text, ui(13.0, 400), x + 12.0, y + 9.0, w - 24.0, th + 2.0, fg, 1.0);
        y += th + 18.0 + 10.0;
    }

    p.line(x, y + 0.5, x + w, y + 0.5, t.border, 1.0, 1.0);
    y += 1.0 + 12.0;
    let stats: [(&str, String, u32); 3] = [
        ("Total (sum of wires)", c.total.map_or("—".into(), |v| format!("{v:.1} A")), t.fg),
        ("Spread", c.spread.map_or("—".into(), |v| format!("{v:.1} A")), level_colors(t, c.spread_level).1),
        ("PSU limits", c.limits_text.clone().unwrap_or_else(|| "—".into()), t.fg),
    ];
    let sw = w / 3.0;
    for (i, (k, v, col)) in stats.iter().enumerate() {
        let sx = x + sw * i as f32;
        p.text(k, ui(11.0, 400), sx, y, sw, 14.0, t.fg3, 1.0, Align::Left);
        p.text(v, num(16.0, 600), sx, y + 15.0, sw, 21.0, *col, stale, Align::Left);
    }
}

fn trim(a: f32) -> String {
    if a.fract() == 0.0 { format!("{a:.0}") } else { format!("{a:.1}") }
}
