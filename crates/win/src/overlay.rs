//! Alarm "notch" on every monitor (docs/DESIGN.md "Alarm overlay"): topmost, never
//! activates, click-through-free only where drawn, big snooze button.

use meltalarm_core::{AlarmView, Level};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, w};

use crate::gfx::{self, Align, Gfx, Painter, num, ui};

const RED: u32 = 0xC8102E;
const GREEN: u32 = 0x1E7F45;
const BODY: u32 = 0x0F0F10;

struct Notch {
    hwnd: HWND,
    monitor: RECT,
    scale: f32,
    /// Snooze button in window pixels.
    button: Option<RECT>,
}

pub struct Overlay {
    notches: Vec<Notch>,
    pub hover: Option<HWND>,
    pub hotkey_hint: bool,
}

fn monitors() -> Vec<(RECT, f32)> {
    unsafe extern "system" fn cb(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec pointer passed below, valid for the enumeration.
        unsafe {
            let v = &mut *(data.0 as *mut Vec<(RECT, f32)>);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            v.push((mi.rcMonitor, dx as f32 / 96.0));
        }
        true.into()
    }
    let mut v: Vec<(RECT, f32)> = Vec::new();
    // SAFETY: callback only touches `v` during the call.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

fn notch_width(monitor_w_dip: f32) -> f32 {
    (monitor_w_dip * 0.32).clamp(560.0, 880.0).min(monitor_w_dip - 24.0)
}

impl Overlay {
    pub fn new() -> Self {
        Overlay { notches: vec![], hover: None, hotkey_hint: true }
    }

    pub fn visible(&self) -> bool {
        !self.notches.is_empty()
    }

    /// Show/redraw for `alarm`, or tear down when `None` (frees all rendering resources).
    pub fn sync(&mut self, gfx: &Gfx, alarm: Option<&AlarmView>, class: windows::core::PCWSTR) {
        let Some(a) = alarm else {
            self.close();
            return;
        };
        let mons = monitors();
        if self.notches.len() != mons.len() || self.notches.iter().zip(&mons).any(|(n, (r, s))| n.monitor != *r || n.scale != *s) {
            self.close();
            for (r, s) in mons {
                // SAFETY: plain window creation with our registered class.
                let hwnd = unsafe {
                    CreateWindowExW(
                        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                        class,
                        w!("MeltAlarm alarm"),
                        WS_POPUP,
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        None,
                        None,
                    )
                };
                if let Ok(hwnd) = hwnd {
                    self.notches.push(Notch { hwnd, monitor: r, scale: s, button: None });
                }
            }
        }
        let hint = self.hotkey_hint;
        for n in &mut self.notches {
            let mw = (n.monitor.right - n.monitor.left) as f32 / n.scale;
            let width = notch_width(mw);
            let height = height(gfx, a, width);
            let x = n.monitor.left + (((mw - width) / 2.0) * n.scale) as i32;
            let hovered = self.hover == Some(n.hwnd);
            let mut button = None;
            let _ = gfx.present(n.hwnd, x, n.monitor.top, width, height, n.scale, |p| {
                button = draw(p, a, width, height, hovered, hint);
            });
            n.button = button.map(|(bx, by, bw, bh)| RECT {
                left: (bx * n.scale) as i32,
                top: (by * n.scale) as i32,
                right: ((bx + bw) * n.scale) as i32,
                bottom: ((by + bh) * n.scale) as i32,
            });
            // Games can push windows back: re-assert topmost on every redraw, never activate.
            // SAFETY: our own window.
            unsafe {
                let _ = SetWindowPos(n.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
        }
    }

    pub fn hit_button(&self, hwnd: HWND, x: i32, y: i32) -> bool {
        self.notches
            .iter()
            .find(|n| n.hwnd == hwnd)
            .and_then(|n| n.button)
            .is_some_and(|b| x >= b.left && x < b.right && y >= b.top && y < b.bottom)
    }

    pub fn close(&mut self) {
        for n in self.notches.drain(..) {
            // SAFETY: our own window.
            unsafe { let _ = DestroyWindow(n.hwnd); }
        }
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
    let band = if a.green { GREEN } else { RED };
    // Frame: flat top fused to the screen edge, 16 px bottom corners, 2 px colored border.
    gfx::shadow(p, 12.0, -20.0, w - 24.0, h + 8.0, 16.0, 10.0, 0.5);
    p.fill_notch(0.0, 0.0, w, h, 16.0, band, 1.0);
    p.fill_notch(2.0, 0.0, w - 4.0, h - 2.0, 14.0, BODY, 0.97);
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
        p.text("TEST", num(13.0, 700), right_x, (BAND - 22.0) / 2.0, tw, 22.0, RED, 1.0, Align::Center);
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

    // Detail row: mini bars | what + numbers | right block.
    let limit = a.bar_limit.unwrap_or(12.0);
    for (i, wv) in a.bars.iter().enumerate() {
        let bx = x + i as f32 * 14.0;
        p.fill_rrect(bx, y + 1.0, 10.0, 48.0, 5.0, 0x2A2A2C, 1.0);
        if let Some(amps) = wv.amps {
            let fh = (amps / limit).clamp(0.0, 1.0) * 48.0;
            let c = match wv.level {
                Level::Warning => 0xFF4D4F,
                Level::Caution => 0xF5A623,
                Level::Normal => 0x8C8C8C,
            };
            if fh > 0.5 {
                p.fill_rrect(bx, y + 49.0 - fh, 10.0, fh, 5.0_f32.min(fh / 2.0), c, 1.0);
            }
        }
    }
    let right_w = p.gfx().text_width(&a.right_value, num(36.0, 700)).max(p.gfx().text_width(&a.right_label, ui(11.0, 400)) + 10.0) + 4.0;
    let tx = x + 6.0 * 14.0 + 12.0;
    let tw = iw - (tx - x) - right_w - 12.0;
    p.text(&a.what, ui(15.0, 600), tx, y + 2.0, tw, 20.0, 0xFFFFFF, 1.0, Align::Left);
    p.para(&a.numbers, ui(13.0, 400), tx, y + 24.0, tw, 30.0, 0xC8C8C8, 1.0);
    let rx = x + iw - right_w;
    p.text(&a.right_label, ui(11.0, 400), rx, y, right_w, 14.0, 0xC8C8C8, 1.0, Align::Right);
    let rc = if a.green { 0xFFFFFF } else { 0xFF4D4F };
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
