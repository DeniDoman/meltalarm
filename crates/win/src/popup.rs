//! The status flyout above the tray icon (docs/DESIGN.md "Popup"): one connector's card,
//! anchored to its icon, closed by any click elsewhere.

use std::time::{Duration, Instant};

use meltalarm_core::{ConnectorView, ViewModel};
use meltalarm_model::ConnectorKey;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, HWND_TOPMOST, SW_HIDE, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos, ShowWindow,
};

use crate::anim::{self, Motion, Transition};
use crate::card::{HeaderButton, MARGIN, PAD, TOP, W, content_height, dismiss_rect, draw_card, draw_content, header_button_rect};
use crate::gfx::{Frame, Gfx};
use crate::palette::theme;

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

pub struct Popup {
    pub hwnd: HWND,
    pub connector: Option<ConnectorKey>,
    anchor: Option<RECT>,
    hidden_at: Option<(ConnectorKey, Instant)>,
    /// Where and at which DPI scale it was last drawn (window origin, px).
    pub origin: (i32, i32),
    pub scale: f32,
    /// The last drawn picture, kept while it is open or fading (DESIGN.md "Motion").
    frame: Option<Frame>,
    motion: Motion,
    /// The entry starts with the next shown frame.
    fresh: bool,
}

impl Popup {
    pub fn new(hwnd: HWND) -> Self {
        Popup {
            hwnd,
            connector: None,
            anchor: None,
            hidden_at: None,
            origin: (0, 0),
            scale: 1.0,
            frame: None,
            motion: Motion::Still,
            fresh: false,
        }
    }

    /// `x`, `y`: client pixels.
    pub fn hit(&self, gfx: &Gfx, view: &ViewModel, x: i32, y: i32) -> PopupHit {
        let (dx, dy) = (x as f32 / self.scale - MARGIN, y as f32 / self.scale - MARGIN);
        let (bx, by, bw, bh) = header_button_rect(PAD, TOP, W - 2.0 * PAD);
        let dismiss = self.shown(view).and_then(|c| dismiss_rect(gfx, c, W - 2.0 * PAD));
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

    /// The connector it shows, if open.
    pub fn shown<'v>(&self, view: &'v ViewModel) -> Option<&'v ConnectorView> {
        let key = self.connector.as_ref()?;
        view.connectors.iter().find(|c| &c.key == key)
    }

    /// Tray click: open for `key`, or close if it is already open for it.
    pub fn toggle(&mut self, key: &ConnectorKey, anchor: Option<RECT>, gfx: &Gfx, view: &ViewModel, light: bool) {
        // A click on the icon first deactivates (hides) the popup; don't reopen it right away.
        if self.hidden_at.as_ref().is_some_and(|(k, t)| k == key && t.elapsed() < Duration::from_millis(400)) {
            self.hidden_at = None;
            return;
        }
        if self.connector.as_ref() == Some(key) {
            self.hide();
            return;
        }
        self.connector = Some(key.clone());
        // It rises from its icon and fades in (DESIGN.md "Motion").
        self.motion = if anim::enabled() { Motion::In(Transition::new(anim::RISE)) } else { Motion::Still };
        self.fresh = true;
        // Freeze the anchor at open time (the cursor may move while the popup stays).
        self.anchor = anchor.or_else(|| {
            let mut p = POINT::default();
            // SAFETY: valid out pointer.
            unsafe {
                let _ = GetCursorPos(&mut p);
            }
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

    /// Close: it fades out (or goes at once with Windows' animations off).
    pub fn hide(&mut self) {
        if let Some(k) = self.connector.take() {
            self.hidden_at = Some((k, Instant::now()));
            if anim::enabled() && self.frame.is_some() {
                self.motion = Motion::Out(Transition::new(anim::FADE));
                self.show_frame();
            } else {
                self.gone();
            }
        }
    }

    /// Close at once: it turns into the floating view, or the app exits.
    pub fn hide_now(&mut self) {
        if let Some(k) = self.connector.take() {
            self.hidden_at = Some((k, Instant::now()));
        }
        self.gone();
    }

    fn gone(&mut self) {
        self.motion = Motion::Still;
        self.frame = None;
        // SAFETY: our own window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// One animation frame; `true` while it still moves.
    pub fn tick(&mut self) -> bool {
        if !self.motion.moving() {
            return false;
        }
        self.show_frame();
        if self.motion.settle(Instant::now()) {
            self.gone();
        } else if !self.motion.moving() {
            self.show_frame();
        }
        self.motion.moving()
    }

    pub fn moving(&self) -> bool {
        self.motion.moving()
    }

    /// Show the frame as far as the motion has come: rising 8 DIP and fading in, or fading out.
    fn show_frame(&self) {
        let Some(frame) = &self.frame else { return };
        let now = Instant::now();
        let shown = self.motion.shown(now);
        let rise = match self.motion {
            Motion::In(_) => ((1.0 - shown) * anim::RISE_DIP * self.scale).round() as i32,
            _ => 0,
        };
        let alpha = (shown * 255.0).round().clamp(0.0, 255.0) as u8;
        let _ = frame.show(self.hwnd, (self.origin.0, self.origin.1 + rise), (0, 0, frame.w, frame.h), alpha);
    }

    pub fn update(&mut self, gfx: &Gfx, view: &ViewModel, light: bool) {
        if self.connector.is_some() {
            self.draw(gfx, view, light);
        }
    }

    fn draw(&mut self, gfx: &Gfx, view: &ViewModel, light: bool) {
        let Some(c) = self.shown(view) else {
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
            unsafe {
                let _ = GetCursorPos(&mut p);
            }
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
        let frame = gfx.render(win_w, win_h, scale, |p| {
            let (cx, cy) = (MARGIN, MARGIN);
            draw_card(p, &t, cx, cy, card_w, card_h, t.border);
            draw_content(p, &t, c, cx + PAD, cy + TOP, card_w - 2.0 * PAD, HeaderButton::PopOut);
        });
        if let Ok(frame) = frame {
            self.frame = Some(frame);
            if std::mem::take(&mut self.fresh) {
                self.motion.restart();
            }
            self.show_frame();
        }
    }
}
