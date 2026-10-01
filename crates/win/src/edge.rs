//! Windows fused to the top edge of every monitor, centered: the alarm notch (Spec §8.2) and its
//! little sibling, the caution strip (§8.8). Topmost and never activating; one window per
//! monitor, recreated when the monitors change. They drop out of the top edge and retract into it
//! (DESIGN.md "Motion"): the frame is drawn once and revealed from its bottom up, so it never
//! spills onto a monitor stacked above.

use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
    WINDOW_EX_STYLE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR};

use crate::anim::{self, Motion, Transition};
use crate::gfx::{Frame, Gfx, Painter};

/// Every monitor: its rectangle (px) and DPI scale.
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

pub(crate) struct EdgeWindow {
    pub hwnd: HWND,
    pub monitor: RECT,
    pub scale: f32,
    /// The last drawn picture and its left edge (px); kept while the window is up.
    frame: Option<(Frame, i32)>,
}

impl EdgeWindow {
    /// The monitor's width in DIPs.
    pub fn width_dip(&self) -> f32 {
        (self.monitor.right - self.monitor.left) as f32 / self.scale
    }

    /// Draw `w`×`h` DIPs, centered on the monitor's top edge. Shown by `EdgeWindows::show`.
    pub fn render(&mut self, gfx: &Gfx, w: f32, h: f32, draw: impl FnOnce(&Painter)) {
        let x = self.monitor.left + (((self.width_dip() - w) / 2.0) * self.scale) as i32;
        if let Ok(frame) = gfx.render(w, h, self.scale, draw) {
            self.frame = Some((frame, x));
        }
    }

    /// Show the bottom `shown` share (0‥1) of the frame at the top edge: it slides out of the edge.
    /// Games can push windows back, so every frame re-asserts topmost, never activating.
    fn show(&self, shown: f32) {
        let Some((frame, x)) = &self.frame else { return };
        let visible = ((frame.h as f32 * shown).round() as i32).clamp(1, frame.h);
        let _ = frame.show(self.hwnd, (*x, self.monitor.top), (0, frame.h - visible, frame.w, visible), 255);
        // SAFETY: our own window.
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }
}

/// One window per monitor of a registered class, and their motion.
pub(crate) struct EdgeWindows {
    pub wins: Vec<EdgeWindow>,
    class: PCWSTR,
    /// Added to the topmost, no-activate, tool-window, layered base (e.g. click-through).
    extra_style: WINDOW_EX_STYLE,
    title: PCWSTR,
    motion: Motion,
}

impl EdgeWindows {
    pub fn new(class: PCWSTR, extra_style: WINDOW_EX_STYLE, title: PCWSTR) -> Self {
        EdgeWindows { wins: vec![], class, extra_style, title, motion: Motion::Still }
    }

    /// Make sure there is exactly one window per current monitor, and start the drop if the
    /// surface wasn't up (or was leaving). Returns `true` if the windows were (re)created.
    pub fn ensure(&mut self) -> bool {
        let mons = monitors();
        let current = self.wins.len() == mons.len() && self.wins.iter().zip(&mons).all(|(w, (r, s))| w.monitor == *r && w.scale == *s);
        if current {
            if self.motion.leaving() {
                self.enter();
            }
            return false;
        }
        let was_up = !self.wins.is_empty() && !self.motion.leaving();
        self.close();
        for (monitor, scale) in mons {
            // SAFETY: plain window creation with a registered class.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | self.extra_style,
                    self.class,
                    self.title,
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
                self.wins.push(EdgeWindow { hwnd, monitor, scale, frame: None });
            }
        }
        // A display change while up redraws in place; a new surface drops in.
        if !was_up {
            self.enter();
        }
        true
    }

    fn enter(&mut self) {
        self.motion = if anim::enabled() { Motion::In(Transition::new(anim::DROP)) } else { Motion::Still };
    }

    /// Show every window as far as its motion has come.
    pub fn show(&self) {
        let shown = self.motion.shown(Instant::now());
        for w in &self.wins {
            w.show(shown);
        }
    }

    /// The surface is up and not leaving.
    pub fn is_up(&self) -> bool {
        !self.wins.is_empty() && !self.motion.leaving()
    }

    /// At rest: hit areas match what is on screen.
    pub fn at_rest(&self) -> bool {
        !self.motion.moving()
    }

    pub fn moving(&self) -> bool {
        self.motion.moving()
    }

    /// Retract into the top edge, then close; `instant`, or with motion off: close now.
    pub fn leave(&mut self, instant: bool) {
        if self.wins.is_empty() || (self.motion.leaving() && !instant) {
            return;
        }
        if instant || !anim::enabled() {
            self.close();
        } else {
            self.motion = Motion::Out(Transition::new(anim::RETRACT));
        }
    }

    /// One animation frame. Returns `true` while still moving.
    pub fn tick(&mut self) -> bool {
        if !self.motion.moving() {
            return false;
        }
        self.show();
        if self.motion.settle(Instant::now()) {
            self.close();
        } else if !self.motion.moving() {
            self.show();
        }
        self.motion.moving()
    }

    /// Destroy every window (and with them all rendering resources).
    pub fn close(&mut self) {
        for w in self.wins.drain(..) {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
        }
        self.motion = Motion::Still;
    }
}
