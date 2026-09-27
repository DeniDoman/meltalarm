//! Tray icons: one per tracked connector, reconciled from the view.

use std::collections::HashMap;

use meltalarm_core::ViewModel;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON, SM_CXSMICON};

use crate::glyph;

pub const WM_TRAY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

struct Shown {
    pixels: Vec<u32>,
    size: i32,
    tip: String,
    icon: HICON,
}

pub struct Tray {
    hwnd: HWND,
    shown: HashMap<u32, Shown>,
}

/// Tray icon id for a connector position in `ViewModel::connectors`.
pub fn uid(index: usize) -> u32 {
    index as u32 + 1
}

/// The single "connecting…" icon shown before any source has connected. It reuses the first
/// connector's id: Windows remembers "show on taskbar" per id, so a user who pinned MeltAlarm
/// also sees the placeholder.
pub const PLACEHOLDER: u32 = 1;

impl Tray {
    pub fn new(hwnd: HWND) -> Self {
        Tray { hwnd, shown: HashMap::new() }
    }

    fn base(&self, id: u32) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: id,
            ..Default::default()
        }
    }

    fn icon_size(&self) -> i32 {
        // SAFETY: valid window handle.
        unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(self.hwnd)) }.clamp(16, 64)
    }

    pub fn sync(&mut self, view: &ViewModel, light: bool, blink_on: bool) {
        let size = self.icon_size();
        let mut desired: Vec<(u32, Vec<u32>, String)> = view
            .connectors
            .iter()
            .enumerate()
            .filter(|(_, c)| c.tracked)
            .map(|(i, c)| (uid(i), glyph::render(c, size, light, blink_on), c.tooltip.clone()))
            .collect();
        if let (true, Some(tip)) = (view.connectors.is_empty(), &view.connecting) {
            desired.push((PLACEHOLDER, glyph::render_placeholder(size, light), tip.clone()));
        }
        let wanted: Vec<u32> = desired.iter().map(|d| d.0).collect();
        for (id, pixels, tip) in desired {
            let existing = self.shown.get(&id);
            if existing.is_some_and(|s| s.pixels == pixels && s.tip == tip && s.size == size) {
                continue;
            }
            let Some(icon) = glyph::to_hicon(&pixels, size) else { continue };
            let mut nid = self.base(id);
            nid.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
            nid.uCallbackMessage = WM_TRAY;
            nid.hIcon = icon;
            for (d, s) in nid.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
                *d = s;
            }
            // SAFETY: nid is fully initialized; the shell copies the icon.
            unsafe {
                if existing.is_some() {
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
                } else {
                    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
                    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                    let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
                }
            }
            if let Some(old) = self.shown.insert(id, Shown { pixels, size, tip, icon }) {
                // SAFETY: we created this icon and the shell holds its own copy.
                unsafe { let _ = DestroyIcon(old.icon); }
            }
        }
        let stale: Vec<u32> = self.shown.keys().copied().filter(|k| !wanted.contains(k)).collect();
        for id in stale {
            self.remove(id);
        }
    }

    fn remove(&mut self, id: u32) {
        let nid = self.base(id);
        // SAFETY: deleting our own icon id.
        unsafe { let _ = Shell_NotifyIconW(NIM_DELETE, &nid); }
        if let Some(s) = self.shown.remove(&id) {
            // SAFETY: we created this icon.
            unsafe { let _ = DestroyIcon(s.icon); }
        }
    }

    /// Explorer restarted: our icons are gone; forget them so the next sync re-adds.
    pub fn has_icons(&self) -> bool {
        !self.shown.is_empty()
    }

    pub fn forget_all(&mut self) {
        for (_, s) in self.shown.drain() {
            // SAFETY: we created these icons.
            unsafe { let _ = DestroyIcon(s.icon); }
        }
    }

    pub fn remove_all(&mut self) {
        let ids: Vec<u32> = self.shown.keys().copied().collect();
        for id in ids {
            self.remove(id);
        }
    }

    /// A Windows notification balloon from one of our icons (the first shown one).
    pub fn notify(&self, title: &str, text: &str) {
        let Some(&id) = self.shown.keys().min() else { return };
        let mut nid = self.base(id);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        for (d, s) in nid.szInfoTitle.iter_mut().zip(title.encode_utf16().take(63)) {
            *d = s;
        }
        for (d, s) in nid.szInfo.iter_mut().zip(text.encode_utf16().take(255)) {
            *d = s;
        }
        // SAFETY: modifying our own icon with a fully initialized struct.
        unsafe { let _ = Shell_NotifyIconW(NIM_MODIFY, &nid); }
    }

    pub fn icon_rect(&self, id: u32) -> Option<RECT> {
        let ident = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: id,
            ..Default::default()
        };
        // SAFETY: valid identifier struct.
        unsafe { Shell_NotifyIconGetRect(&ident) }.ok()
    }
}
