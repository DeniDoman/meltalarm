//! Alarm "notch" on every monitor (docs/DESIGN.md "Alarm overlay"): topmost, never
//! activates; the big snooze button is its only interaction.

use meltalarm_core::{AlarmView, Level};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;
use windows::core::{PCWSTR, w};

use crate::card::draw_cut_bar;
use crate::edge::EdgeWindows;
use crate::gfx::{self, Align, Gfx, Painter, num, ui};
use crate::palette::{ALARM_RED, ALERT_BODY, CAUTION_DARK, CLEARED_GREEN, WARNING_DARK};

pub const CLASS: PCWSTR = w!("MeltAlarmOverlay");

pub struct Overlay {
    windows: EdgeWindows,
    /// The snooze button of each window, in window pixels (same order as the windows).
    buttons: Vec<Option<RECT>>,
    pub hover: Option<HWND>,
    pub hotkey_hint: bool,
}

fn notch_width(monitor_w_dip: f32) -> f32 {
    (monitor_w_dip * 0.32).clamp(560.0, 880.0).min(monitor_w_dip - 24.0)
}

impl Overlay {
    pub fn new() -> Self {
        let windows = EdgeWindows::new(CLASS, WINDOW_EX_STYLE(0), w!("MeltAlarm alarm"));
        Overlay { windows, buttons: vec![], hover: None, hotkey_hint: true }
    }

    pub fn visible(&self) -> bool {
        !self.windows.is_empty()
    }

    /// Show/redraw for `alarm`, or tear down when `None` (frees all rendering resources).
    pub fn sync(&mut self, gfx: &Gfx, alarm: Option<&AlarmView>) {
        let Some(a) = alarm else {
            self.close();
            return;
        };
        self.windows.ensure();
        let hint = self.hotkey_hint;
        self.buttons.clear();
        for win in &self.windows.wins {
            let width = notch_width(win.width_dip());
            let height = height(gfx, a, width);
            let hovered = self.hover == Some(win.hwnd);
            let mut button = None;
            win.present(gfx, width, height, |p| button = draw(p, a, width, height, hovered, hint));
            self.buttons.push(button.map(|(bx, by, bw, bh)| RECT {
                left: (bx * win.scale) as i32,
                top: (by * win.scale) as i32,
                right: ((bx + bw) * win.scale) as i32,
                bottom: ((by + bh) * win.scale) as i32,
            }));
        }
    }

    pub fn hit_button(&self, hwnd: HWND, x: i32, y: i32) -> bool {
        self.windows
            .wins
            .iter()
            .position(|w| w.hwnd == hwnd)
            .and_then(|i| self.buttons.get(i).copied().flatten())
            .is_some_and(|b| x >= b.left && x < b.right && y >= b.top && y < b.bottom)
    }

    pub fn close(&mut self) {
        self.windows.close();
        self.buttons.clear();
        self.hover = None;
    }
}

const BAND: f32 = 44.0;

fn height(gfx: &Gfx, a: &AlarmView, w: f32) -> f32 {
    let mut h = BAND + 16.0 + 34.0 + 3.0 + 20.0 + 14.0 + 1.0 + 14.0 + 50.0 + 18.0;
    if let Some(note) = &a.note {
        h += gfx.text_height(note, ui(13.0, 400), w - 64.0) + 18.0 + 14.0;
    }
    if a.snooze {
        h += 52.0 + 14.0;
    }
    if a.cleared_progress.is_some() {
        h += 4.0 + 14.0;
    }
    h
}

/// Returns the snooze button rect (DIP) if drawn.
fn draw(p: &Painter, a: &AlarmView, w: f32, h: f32, hovered: bool, hint: bool) -> Option<(f32, f32, f32, f32)> {
    let band = if a.green { CLEARED_GREEN } else { ALARM_RED };
    // Frame: flat top fused to the screen edge, 16 px bottom corners, 2 px colored border.
    gfx::shadow(p, 12.0, -20.0, w - 24.0, h + 8.0, 16.0, 10.0, 0.5);
    p.fill_notch(0.0, 0.0, w, h, 16.0, band, 1.0);
    p.fill_notch(2.0, 0.0, w - 4.0, h - 2.0, 14.0, ALERT_BODY, 0.97);
    p.fill_rect(0.0, 0.0, w, BAND, band, 1.0);

    // Band: headline + connector (+ TEST chip).
    let mut right_x = w - 18.0;
    let conn_w = p.gfx().text_width(&a.connector, ui(14.0, 600));
    right_x -= conn_w;
    p.text(&a.connector, ui(14.0, 600), right_x, 0.0, conn_w + 1.0, BAND, 0xFFFFFF, 1.0, Align::Left);
    if a.test {
        let tw = p.gfx().text_width("TEST", num(13.0, 700)) + 16.0;
        right_x -= tw + 10.0;
        p.fill_rrect(right_x, (BAND - 22.0) / 2.0, tw, 22.0, 4.0, 0xFFFFFF, 1.0);
        p.text("TEST", num(13.0, 700), right_x, (BAND - 22.0) / 2.0, tw, 22.0, ALARM_RED, 1.0, Align::Center);
    }
    let mut hx = 18.0;
    if !a.green {
        // Warning triangle.
        let (tx, ty) = (18.0, BAND / 2.0);
        p.line(tx, ty + 8.0, tx + 9.0, ty - 8.0, 0xFFFFFF, 1.0, 2.2);
        p.line(tx + 9.0, ty - 8.0, tx + 18.0, ty + 8.0, 0xFFFFFF, 1.0, 2.2);
        p.line(tx, ty + 8.0, tx + 18.0, ty + 8.0, 0xFFFFFF, 1.0, 2.2);
        p.line(tx + 9.0, ty - 2.5, tx + 9.0, ty + 2.5, 0xFFFFFF, 1.0, 2.2);
        p.circle(tx + 9.0, ty + 5.3, 1.2, 0xFFFFFF, 1.0);
        hx += 28.0;
    }
    p.text(&a.headline, num(22.0, 700), hx, 0.0, right_x - hx - 12.0, BAND, 0xFFFFFF, 1.0, Align::Left);

    let (x, iw) = (20.0, w - 40.0);
    let mut y = BAND + 16.0;
    p.text(&a.action, num(30.0, 700), x, y, iw, 34.0, 0xFFFFFF, 1.0, Align::Left);
    y += 34.0 + 3.0;
    p.text(&a.sub, ui(14.0, 400), x, y, iw, 20.0, 0xC8C8C8, 1.0, Align::Left);
    y += 20.0 + 14.0;
    p.line(x, y + 0.5, x + iw, y + 0.5, 0x2A2A2C, 1.0, 1.0);
    y += 1.0 + 14.0;

    // Detail row: small cut bars | what + numbers | right block.
    for (i, wv) in a.bars.iter().enumerate() {
        let c = match wv.level {
            Level::Warning => WARNING_DARK,
            Level::Caution => CAUTION_DARK,
            Level::Normal => 0x8C8C8C,
        };
        let bx = x + i as f32 * 14.0;
        draw_cut_bar(p, bx, y + 1.0, 10.0, 48.0, wv.amps, a.bar_limit, a.bar_rating, (0x2A2A2C, 1.0), c, 1.0, 0x8C8C8C);
    }
    let right_w = p.gfx().text_width(&a.right_value, num(36.0, 700)).max(p.gfx().text_width(&a.right_label, ui(11.0, 400)) + 10.0) + 4.0;
    let tx = x + 6.0 * 14.0 + 12.0;
    let tw = iw - (tx - x) - right_w - 12.0;
    p.text(&a.what, ui(15.0, 600), tx, y + 2.0, tw, 20.0, 0xFFFFFF, 1.0, Align::Left);
    p.para(&a.numbers, ui(13.0, 400), tx, y + 23.0, tw, 38.0, 0xC8C8C8, 1.0); // two full lines
    let rx = x + iw - right_w;
    p.text(&a.right_label, ui(11.0, 400), rx, y, right_w, 14.0, 0xC8C8C8, 1.0, Align::Right);
    let rc = if a.green { 0xFFFFFF } else { WARNING_DARK };
    p.text(&a.right_value, num(36.0, 700), rx, y + 12.0, right_w, 40.0, rc, 1.0, Align::Right);
    y += 50.0 + 14.0;

    if let Some(note) = &a.note {
        let th = p.gfx().text_height(note, ui(13.0, 400), iw - 24.0);
        p.fill_rrect(x, y, iw, th + 18.0, 6.0, 0x26262A, 1.0);
        p.para(note, ui(13.0, 400), x + 12.0, y + 9.0, iw - 24.0, th + 2.0, 0xE0E0E0, 1.0);
        y += th + 18.0 + 14.0;
    }

    let mut button = None;
    if a.snooze {
        let (bx, bh) = (x, 52.0);
        if hovered {
            p.fill_rrect(bx, y, iw, bh, 8.0, 0xFFFFFF, 0.12);
        }
        p.stroke_rrect(bx + 1.0, y + 1.0, iw - 2.0, bh - 2.0, 8.0, 0xEDEDED, 1.0, 2.0);
        let label = "SNOOZE 30 s";
        let lw = p.gfx().text_width(label, num(18.0, 700));
        let kbd = "Ctrl+Alt+G";
        let kw = if hint { p.gfx().text_width(kbd, ui(12.0, 400)) + 14.0 } else { 0.0 };
        let total = lw + if hint { 14.0 + kw } else { 0.0 };
        let lx = bx + (iw - total) / 2.0;
        p.text(label, num(18.0, 700), lx, y, lw + 2.0, bh, 0xFFFFFF, 1.0, Align::Left);
        if hint {
            let kx = lx + lw + 14.0;
            p.stroke_rrect(kx, y + (bh - 22.0) / 2.0, kw, 22.0, 4.0, 0x555555, 1.0, 1.0);
            p.text(kbd, ui(12.0, 400), kx, y + (bh - 22.0) / 2.0, kw, 22.0, 0xC8C8C8, 1.0, Align::Center);
        }
        button = Some((bx, y, iw, bh));
        y += bh + 14.0;
    }
    if let Some(progress) = a.cleared_progress {
        p.fill_rrect(x, y, iw, 4.0, 2.0, 0x26262A, 1.0);
        p.fill_rrect(x, y, iw * (1.0 - progress), 4.0, 2.0, 0x3FB96F, 1.0);
    }
    button
}
