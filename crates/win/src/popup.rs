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

pub(crate) const W: f32 = 360.0;
pub(crate) const PAD: f32 = 20.0;
pub(crate) const MARGIN: f32 = 16.0; // room for the shadow
const BAR_H: f32 = 96.0;
pub(crate) const TOP: f32 = 18.0;

pub(crate) struct Theme {
    pub bg: u32,
    pub border: u32,
    pub fg: u32,
    pub fg3: u32,
    pub track: (u32, f32),
    pub ok: u32,
    pub caution: u32,
    pub warn: u32,
    pub caution_text: u32,
    pub warn_text: u32,
    pub note_caution: (u32, f32, u32),
    pub note_info: (u32, f32, u32),
    /// Links (DESIGN.md "State colors": accent).
    pub accent: u32,
}

/// The one button at the right end of the header (DESIGN.md "Popup").
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderButton {
    None,
    PopOut,
    Close,
}

/// What a click in the flyout hits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PopupHit {
    PopOut,
    /// The header: a drag here tears the view off (Spec §7.4).
    Header,
    /// The cable note's *Dismiss* link (Spec §8.10).
    Dismiss,
    Other,
}

pub(crate) fn theme(light: bool) -> Theme {
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
            accent: 0x005FB8,
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
            accent: 0x4CC2FF,
        }
    }
}

pub struct Popup {
    pub hwnd: HWND,
    pub connector: Option<usize>,
    anchor: Option<RECT>,
    hidden_at: Option<(usize, Instant)>,
    /// Where and at which DPI scale it was last drawn (window origin, px).
    pub origin: (i32, i32),
    pub scale: f32,
}

impl Popup {
    pub fn new(hwnd: HWND) -> Self {
        Popup { hwnd, connector: None, anchor: None, hidden_at: None, origin: (0, 0), scale: 1.0 }
    }

    /// `x`, `y`: client pixels.
    pub fn hit(&self, gfx: &Gfx, view: &ViewModel, x: i32, y: i32) -> PopupHit {
        let (dx, dy) = (x as f32 / self.scale - MARGIN, y as f32 / self.scale - MARGIN);
        let (bx, by, bw, bh) = header_button_rect(PAD, TOP, W - 2.0 * PAD);
        let dismiss = self.connector.and_then(|i| view.connectors.get(i)).and_then(|c| dismiss_rect(gfx, c, W - 2.0 * PAD));
        if (bx..bx + bw).contains(&dx) && (by..by + bh).contains(&dy) {
            PopupHit::PopOut
        } else if let Some((rx, ry, rw, rh)) = dismiss
            && (PAD + rx..PAD + rx + rw).contains(&dx)
            && (TOP + ry..TOP + ry + rh).contains(&dy)
        {
            PopupHit::Dismiss
        } else if (0.0..W).contains(&dx) && (0.0..TOP + 24.0 + 8.0).contains(&dy) {
            PopupHit::Header
        } else {
            PopupHit::Other
        }
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

        self.origin = (x, y);
        self.scale = scale;
        let _ = gfx.present(self.hwnd, x, y, win_w, win_h, scale, |p| {
            let (cx, cy) = (MARGIN, MARGIN);
            draw_card(p, &t, cx, cy, card_w, card_h, t.border);
            draw_content(p, &t, c, cx + PAD, cy + TOP, card_w - 2.0 * PAD, HeaderButton::PopOut);
        });
    }
}

fn notes_height(gfx: &Gfx, c: &ConnectorView, w: f32) -> f32 {
    c.notes.iter().map(|n| gfx.text_height(&n.text, ui(13.0, 400), w - 24.0) + 18.0 + 10.0).sum()
}

/// From the content origin down to the first live note.
fn above_notes(c: &ConnectorView) -> f32 {
    let mut y = 24.0 + 14.0;
    if c.alarm_reason.is_some() {
        y += 36.0 + 14.0;
    }
    y + 16.0 + 10.0 + BAR_H + 6.0 + 20.0 + 14.0
}

const DISMISS: &str = "Dismiss";

/// The cable note block: text, then the *Dismiss* link on its own row.
fn cable_note_height(gfx: &Gfx, text: &str, w: f32) -> f32 {
    gfx.text_height(text, ui(13.0, 400), w - 24.0) + 9.0 + 4.0 + 20.0 + 7.0
}

/// The *Dismiss* hit area relative to the content origin (at least 28 px tall).
pub(crate) fn dismiss_rect(gfx: &Gfx, c: &ConnectorView, w: f32) -> Option<(f32, f32, f32, f32)> {
    let text = c.cable_note.as_ref()?;
    let y = above_notes(c) + notes_height(gfx, c, w);
    let th = gfx.text_height(text, ui(13.0, 400), w - 24.0);
    let lw = gfx.text_width(DISMISS, ui(13.0, 600));
    Some((w - 12.0 - lw - 8.0, y + 9.0 + th, lw + 16.0, 28.0))
}

/// Card with its shadow and a 1 px border.
pub(crate) fn draw_card(p: &Painter, t: &Theme, x: f32, y: f32, w: f32, h: f32, border: u32) {
    gfx::shadow(p, x, y, w, h, 8.0, 12.0, 0.35);
    p.fill_rrect(x, y, w, h, 8.0, t.bg, 1.0);
    p.stroke_rrect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, 8.0, border, 1.0, 1.0);
}

/// The header button's box, relative to the content origin (`x`, `y`) and width `w`.
pub(crate) fn header_button_rect(x: f32, y: f32, w: f32) -> (f32, f32, f32, f32) {
    (x + w - 22.0, y - 2.0, 28.0, 28.0)
}

pub(crate) fn draw_header_button(p: &Painter, t: &Theme, button: HeaderButton, x: f32, y: f32, w: f32) {
    let (bx, by, bw, bh) = header_button_rect(x, y, w);
    let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
    match button {
        HeaderButton::None => {}
        HeaderButton::PopOut => {
            p.fill_rrect(bx, by, bw, bh, 6.0, t.track.0, t.track.1 * 0.7);
            let (ix, iy) = (cx - 8.0, cy - 8.0);
            p.stroke_rrect(ix + 2.5, iy + 4.5, 9.0, 9.0, 1.5, t.fg, 1.0, 1.3);
            p.line(ix + 7.5, iy + 2.5, ix + 13.5, iy + 2.5, t.fg, 1.0, 1.3);
            p.line(ix + 13.5, iy + 2.5, ix + 13.5, iy + 8.5, t.fg, 1.0, 1.3);
            p.line(ix + 13.5, iy + 2.5, ix + 8.0, iy + 8.0, t.fg, 1.0, 1.3);
        }
        HeaderButton::Close => draw_close(p, t, bx, by, bw, bh),
    }
}

/// A close button (x) in the box (`x`, `y`, `w`, `h`).
pub(crate) fn draw_close(p: &Painter, t: &Theme, x: f32, y: f32, w: f32, h: f32) {
    p.fill_rrect(x, y, w, h, 5.0, t.track.0, t.track.1 * 0.9);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    p.line(cx - 4.5, cy - 4.5, cx + 4.5, cy + 4.5, t.fg3, 1.0, 1.3);
    p.line(cx + 4.5, cy - 4.5, cx - 4.5, cy + 4.5, t.fg3, 1.0, 1.3);
}

/// Name, then the status chip right after it (DESIGN.md "Popup", item 1).
pub(crate) fn draw_title(p: &Painter, t: &Theme, c: &ConnectorView, x: f32, y: f32, compact: bool) {
    let (title, size, chip_h, chip_text, chip_font) = if compact {
        (ui(13.0, 600), 20.0, 18.0, c.status_text.as_str(), ui(11.0, 600))
    } else {
        (ui(15.0, 600), 24.0, 24.0, c.status_text.as_str(), ui(12.0, 600))
    };
    let lw = p.gfx().text_width(&c.label, title);
    p.text(&c.label, title, x, y, lw + 2.0, size, t.fg, 1.0, Align::Left);
    let dot_r = if compact { 3.0 } else { 3.5 };
    let pad = if compact { 7.0 } else { 10.0 };
    let chip_w = p.gfx().text_width(chip_text, chip_font) + 2.0 * pad + 2.0 * dot_r + 6.0;
    let (chip_bg, chip_a, chip_fg, dot) = match c.status_kind {
        StatusKind::Alarm => (0xC8102E, 1.0, 0xFFFFFF, Some(0xFFFFFF)),
        StatusKind::Caution => (0xF5A623, 0.18, t.caution_text, Some(t.caution)),
        StatusKind::NoData => (t.track.0, t.track.1, t.fg3, None),
        StatusKind::Normal => (t.track.0, t.track.1, t.fg, Some(t.ok)),
    };
    let (chip_x, chip_y) = (x + lw + 8.0, y + (size - chip_h) / 2.0);
    p.fill_rrect(chip_x, chip_y, chip_w, chip_h, chip_h / 2.0, chip_bg, chip_a);
    if let Some(d) = dot {
        p.circle(chip_x + pad + dot_r, chip_y + chip_h / 2.0, dot_r, d, 1.0);
    }
    let text_x = chip_x + pad + 2.0 * dot_r + 6.0;
    p.text(chip_text, chip_font, text_x, chip_y, chip_w - (text_x - chip_x), chip_h, chip_fg, 1.0, Align::Left);
}

pub(crate) fn content_height(gfx: &Gfx, c: &ConnectorView) -> f32 {
    let w = W - 2.0 * PAD;
    let mut h = TOP + above_notes(c) + notes_height(gfx, c, w);
    if let Some(text) = &c.cable_note {
        h += cable_note_height(gfx, text, w) + 10.0;
    }
    h + 1.0 + 12.0 + 36.0 + 16.0 // divider, stats, bottom pad
}

pub(crate) fn level_colors(t: &Theme, l: Level) -> (u32, u32) {
    match l {
        Level::Normal => (t.ok, t.fg),
        Level::Caution => (t.caution, t.caution_text),
        Level::Warning => (t.warn, t.warn_text),
    }
}

/// The full content (popup, floating Full), starting at the content origin.
pub(crate) fn draw_content(p: &Painter, t: &Theme, c: &ConnectorView, x: f32, mut y: f32, w: f32, button: HeaderButton) {
    draw_title(p, t, c, x, y, false);
    draw_header_button(p, t, button, x, y, w);
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

    // Cut bars, no wire numbers (DESIGN.md "Cut bars").
    let col = w / 6.0;
    let stale = if c.stale { 0.45 } else { 1.0 };
    for (i, wv) in c.wires.iter().enumerate() {
        let cx = x + col * i as f32 + col / 2.0;
        let (bar, txt) = level_colors(t, wv.level);
        draw_cut_bar(p, cx - 7.0, y, 14.0, BAR_H, wv.amps, c.bar_limit, c.caution_line, t.track, bar, stale, t.fg3);
        let value = wv.amps.map_or("—".to_owned(), |a| format!("{a:.1}"));
        let vcolor = if wv.amps.is_some() { txt } else { t.fg3 };
        p.text(&value, num(16.0, 600), cx - col / 2.0, y + BAR_H + 6.0, col, 20.0, vcolor, stale, Align::Center);
    }
    y += BAR_H + 6.0 + 20.0 + 14.0;

    for n in &c.notes {
        let (bg, a, fg) = if n.kind == NoteKind::Caution { t.note_caution } else { t.note_info };
        let th = p.gfx().text_height(&n.text, ui(13.0, 400), w - 24.0);
        p.fill_rrect(x, y, w, th + 18.0, 6.0, bg, a);
        p.para(&n.text, ui(13.0, 400), x + 12.0, y + 9.0, w - 24.0, th + 2.0, fg, 1.0);
        y += th + 18.0 + 10.0;
    }

    if let Some(text) = &c.cable_note {
        let (bg, a, fg) = t.note_caution;
        let bh = cable_note_height(p.gfx(), text, w);
        let th = p.gfx().text_height(text, ui(13.0, 400), w - 24.0);
        p.fill_rrect(x, y, w, bh, 6.0, bg, a);
        p.para(text, ui(13.0, 400), x + 12.0, y + 9.0, w - 24.0, th + 2.0, fg, 1.0);
        p.text(DISMISS, ui(13.0, 600), x + 12.0, y + 9.0 + th + 4.0, w - 24.0, 20.0, t.accent, 1.0, Align::Right);
        y += bh + 10.0;
    }

    p.line(x, y + 0.5, x + w, y + 0.5, t.border, 1.0, 1.0);
    y += 1.0 + 12.0;
    let stats: [(&str, String, u32); 2] = [
        ("Imbalance", c.imbalance.map_or("—".into(), |v| format!("{v:.1} A")), level_colors(t, c.imbalance_level).1),
        ("PSU status", c.psu_status.clone(), level_colors(t, c.psu_level).1),
    ];
    let sw = w / 2.0;
    for (i, (k, v, col)) in stats.iter().enumerate() {
        let sx = x + sw * i as f32;
        p.text(k, ui(11.0, 400), sx, y, sw, 14.0, t.fg3, 1.0, Align::Left);
        p.text(v, num(16.0, 600), sx, y + 15.0, sw, 21.0, *col, stale, Align::Left);
    }
}

/// The knee scale (DESIGN.md "Cut bars"): the share of the bar's height for `a` amps. The
/// bottom 20 % covers 0 A to the knee (6 A, or 60 % of the limit if lower), the rest the
/// decision range up to the alarm limit at the top.
pub(crate) fn bar_share(a: f32, limit: f32) -> f32 {
    let knee = 6.0_f32.min(0.6 * limit);
    let a = a.clamp(0.0, limit);
    if a <= knee { a / knee * 0.2 } else { 0.2 + (a - knee) / (limit - knee) * 0.8 }
}

/// The cut between head and body (DESIGN.md: 2 px, the weight of MeltAlarm's meaningful edges).
const CUT: f32 = 2.0;

/// One cut bar at (`x`, `y`), `w`×`h`: the top is the alarm limit, the bar is cut straight
/// across at the rating (head: round top, flat bottom; body: flat top, round bottom), and the
/// fill has a round top except where it meets the cut. An unmeasured wire is an outline.
pub(crate) fn draw_cut_bar(
    p: &Painter,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    amps: Option<f32>,
    limit: Option<f32>,
    rating: Option<f32>,
    track: (u32, f32),
    fill: u32,
    fill_a: f32,
    outline: u32,
) {
    let r = w / 2.0;
    let Some(a) = amps else {
        p.stroke_rrect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, r, outline, 0.6, 1.0);
        return;
    };
    let limit = limit.unwrap_or(12.0);
    // Head and body heights; no rating (or a rating at the limit) means one uncut bar.
    let cut_at = rating.filter(|&rt| rt < limit).map(|rt| bar_share(rt, limit) * h);
    let (head_h, body_h) = match cut_at {
        Some(c) => (h - c - CUT / 2.0, c - CUT / 2.0),
        None => (0.0, h),
    };
    if head_h > 0.0 {
        p.fill_rounded_ends(x, y, w, head_h, r, true, false, track.0, track.1);
        p.fill_rounded_ends(x, y + h - body_h, w, body_h, r, false, true, track.0, track.1);
    } else {
        p.fill_rrect(x, y, w, h, r, track.0, track.1);
    }
    let f = if a > 0.05 { (bar_share(a, limit) * h).max(3.0_f32.min(h)) } else { 0.0 };
    if f <= 0.0 {
        return;
    }
    let body_fill = f.min(body_h);
    let body_full = head_h > 0.0 && body_fill >= body_h;
    p.fill_rounded_ends(x, y + h - body_fill, w, body_fill, r, !body_full, true, fill, fill_a);
    if head_h > 0.0 {
        let head_fill = (f - body_h - CUT).clamp(0.0, head_h);
        if head_fill > 0.0 {
            p.fill_rounded_ends(x, y + head_h - head_fill, w, head_fill, r, true, false, fill, fill_a);
        }
    }
}
