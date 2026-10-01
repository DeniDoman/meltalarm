//! Windows fused to the top edge of every monitor, centered: the alarm notch (Spec §8.2) and its
//! little sibling, the caution strip (§8.8). Topmost and never activating; one window per
//! monitor, recreated when the monitors change.

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
    WINDOW_EX_STYLE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR};

use crate::gfx::{Gfx, Painter};

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
}

impl EdgeWindow {
    /// The monitor's width in DIPs.
    pub fn width_dip(&self) -> f32 {
        (self.monitor.right - self.monitor.left) as f32 / self.scale
    }

    /// Draw `w`×`h` DIPs centered on the monitor's top edge. Games can push windows back, so
    /// every frame re-asserts topmost, never activating.
    pub fn present(&self, gfx: &Gfx, w: f32, h: f32, draw: impl FnOnce(&Painter)) {
        let x = self.monitor.left + (((self.width_dip() - w) / 2.0) * self.scale) as i32;
        let _ = gfx.present(self.hwnd, x, self.monitor.top, w, h, self.scale, draw);
        // SAFETY: our own window.
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }
}

/// One window per monitor of a registered class.
pub(crate) struct EdgeWindows {
    pub wins: Vec<EdgeWindow>,
    class: PCWSTR,
    /// Added to the topmost, no-activate, tool-window, layered base (e.g. click-through).
    extra_style: WINDOW_EX_STYLE,
    title: PCWSTR,
}

impl EdgeWindows {
    pub fn new(class: PCWSTR, extra_style: WINDOW_EX_STYLE, title: PCWSTR) -> Self {
        EdgeWindows { wins: vec![], class, extra_style, title }
    }

    /// Make sure there is exactly one window per current monitor. Returns `true` if the windows
    /// were (re)created.
    pub fn ensure(&mut self) -> bool {
        let mons = monitors();
        let current = self.wins.len() == mons.len() && self.wins.iter().zip(&mons).all(|(w, (r, s))| w.monitor == *r && w.scale == *s);
        if current {
            return false;
        }
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
                self.wins.push(EdgeWindow { hwnd, monitor, scale });
            }
        }
        true
    }

    pub fn is_empty(&self) -> bool {
        self.wins.is_empty()
    }

    /// Destroy every window (and with them all rendering resources).
    pub fn close(&mut self) {
        for w in self.wins.drain(..) {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
        }
    }
}
