//! The caution strip (docs/FUNCTIONAL_SPEC.md §8.8, DESIGN.md "Caution strip"): the notch's
//! little amber sibling at the top edge of every monitor. Click-through, never activates.

use meltalarm_core::CautionView;
use windows::Win32::UI::WindowsAndMessaging::WS_EX_TRANSPARENT;
use windows::core::{PCWSTR, w};

use crate::edge::EdgeWindows;
use crate::gfx::{self, Align, Gfx, Painter, num, ui};
use crate::palette::{ALERT_BODY as BODY, CAUTION_DARK as AMBER};

pub const CLASS: PCWSTR = w!("MeltAlarmStrip");
const H: f32 = 40.0;
const SIDE: f32 = 24.0;
const ICON: f32 = 18.0;

pub struct Strips {
    windows: EdgeWindows,
    shown: Option<CautionView>,
}

struct Parts {
    what: String,
    sep: &'static str,
    action: String,
}

fn parts(c: &CautionView) -> Parts {
    let what = match &c.place {
        Some(p) => format!("{p} · {}", c.what),
        None => c.what.clone(),
    };
    Parts { what, sep: "  ·  ", action: c.action.clone() }
}

fn width(gfx: &Gfx, c: &CautionView, monitor_w: f32) -> f32 {
    let p = parts(c);
    let mut w = SIDE + ICON + 10.0 + gfx.text_width(&p.what, num(15.0, 600)) + gfx.text_width(p.sep, ui(14.0, 400));
    w += gfx.text_width(&p.action, ui(14.0, 400)) + SIDE;
    if c.test {
        w += 10.0 + gfx.text_width("TEST", num(12.0, 700)) + 14.0;
    }
    w.clamp(360.0, 720.0).min(monitor_w - 24.0)
}

impl Strips {
    pub fn new() -> Self {
        // Click-through: the strip never catches the mouse (Spec §8.8).
        Strips { windows: EdgeWindows::new(CLASS, WS_EX_TRANSPARENT, w!("MeltAlarm caution")), shown: None }
    }

    /// Show/redraw for `caution`, or tear down when `None`.
    pub fn sync(&mut self, gfx: &Gfx, caution: Option<&CautionView>) {
        let Some(c) = caution else {
            self.close();
            return;
        };
        let recreated = self.windows.ensure();
        if !recreated && self.shown.as_ref() == Some(c) {
            return; // nothing changed: no redraw every second
        }
        for win in &self.windows.wins {
            let w = width(gfx, c, win.width_dip());
            win.present(gfx, w, H, |p| draw(p, c, w));
        }
        self.shown = Some(c.clone());
    }

    pub fn close(&mut self) {
        self.windows.close();
        self.shown = None;
    }
}

fn draw(p: &Painter, c: &CautionView, w: f32) {
    // The notch shape at a smaller size: flat top on the screen edge, 12 px bottom corners.
    gfx::shadow(p, 10.0, -16.0, w - 20.0, H + 6.0, 12.0, 8.0, 0.45);
    p.fill_notch(0.0, 0.0, w, H, 12.0, AMBER, 1.0);
    p.fill_notch(2.0, 0.0, w - 4.0, H - 2.0, 10.0, BODY, 0.97);

    // Amber caution triangle.
    let (tx, ty) = (SIDE, H / 2.0);
    p.line(tx, ty + 7.5, tx + 9.0, ty - 8.0, AMBER, 1.0, 2.0);
    p.line(tx + 9.0, ty - 8.0, tx + ICON, ty + 7.5, AMBER, 1.0, 2.0);
    p.line(tx, ty + 7.5, tx + ICON, ty + 7.5, AMBER, 1.0, 2.0);
    p.line(tx + 9.0, ty - 2.5, tx + 9.0, ty + 2.0, AMBER, 1.0, 2.0);
    p.circle(tx + 9.0, ty + 4.8, 1.1, AMBER, 1.0);

    let parts = parts(c);
    let mut right = w - SIDE;
    if c.test {
        let cw = p.gfx().text_width("TEST", num(12.0, 700)) + 14.0;
        right -= cw;
        p.fill_rrect(right, (H - 20.0) / 2.0, cw, 20.0, 4.0, AMBER, 1.0);
        p.text("TEST", num(12.0, 700), right, (H - 20.0) / 2.0, cw, 20.0, BODY, 1.0, Align::Center);
        right -= 10.0;
    }
    let mut x = SIDE + ICON + 10.0;
    let what_w = p.gfx().text_width(&parts.what, num(15.0, 600));
    p.text(&parts.what, num(15.0, 600), x, 0.0, (right - x).max(0.0), H - 2.0, 0xFFFFFF, 1.0, Align::Left);
    x += what_w;
    let rest = format!("{}{}", parts.sep, parts.action);
    if x < right {
        p.text(&rest, ui(14.0, 400), x, 0.0, right - x, H - 2.0, AMBER, 1.0, Align::Left);
    }
}
