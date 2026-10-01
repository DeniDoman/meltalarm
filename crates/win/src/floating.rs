//! Floating monitor (docs/FUNCTIONAL_SPEC.md §7.4, DESIGN.md "Floating monitor",
//! ARCHITECTURE.md §7.2): a connector's view taken out of the tray, placed and scaled
//! anywhere, always on top and never taking focus.

use std::collections::HashMap;

use meltalarm_core::{ConnectorView, Level, ViewModel};
use meltalarm_model::ConnectorKey;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetCursorPos, HWND_TOPMOST, IDC_ARROW, IDC_HAND, IDC_SIZENESW,
    IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, KillTimer, LoadCursorW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetCursor, SetTimer, SetWindowPos, ShowWindow,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::gfx::{Align, Gfx, Painter, num};
use crate::placement::{self, Layout, Placement, Placements};
use crate::card::{self, HeaderButton, MARGIN, PAD, TOP};
use crate::palette::{self, Theme, level_colors};

pub const CLASS: PCWSTR = w!("MeltAlarmFloat");
/// DESIGN.md "Floating monitor": the longest header ("GPU power cable" + "Not connected") fits
/// beside the hover ×.
const COMPACT: (f32, f32) = (250.0, 152.0);
/// Scale handles: this far inside (and outside) the card edge, in DIPs.
const EDGE: f32 = 6.0;
const MIN_SCALE: f32 = 0.75;
const TIMER_PULSE: usize = 1;
const PULSE_MS: u32 = 600;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    Close,
    Tab,
    /// The cable note's *Dismiss* link (Full layout).
    Dismiss,
    /// Edge band: -1 = left/top, +1 = right/bottom, 0 = not this axis.
    Edge(i8, i8),
    Move,
    Nothing,
}

#[derive(Clone, Copy)]
struct Drag {
    hit: Hit,
    cursor0: POINT,
    pos0: (i32, i32),
    size0: (i32, i32),
    scale0: f32,
    /// While scaling: the user scale the drag has reached. Saved only when the drag ends, so
    /// every redraw meanwhile (the 1 Hz data update too) must use it, not the saved one.
    live: Option<f32>,
}

struct Win {
    hwnd: HWND,
    /// Window origin (px), total scale (DPI × user) and size (px) of the last frame.
    pos: (i32, i32),
    scale: f32,
    size: (i32, i32),
    hover: bool,
    pulse: bool,
    pressed: Option<Hit>,
    drag: Option<Drag>,
}

/// What the frontend must do after a floating-window message.
pub enum Action {
    None,
    /// Show the tray menu at this screen point.
    Menu(i32, i32),
    /// Dismiss the cable note of this connector.
    Dismiss(ConnectorKey),
}

pub struct Floating {
    wins: HashMap<ConnectorKey, Win>,
    places: Placements,
}

fn card_size(gfx: &Gfx, c: &ConnectorView, layout: Layout) -> (f32, f32) {
    match layout {
        Layout::Compact => COMPACT,
        Layout::Full => (card::W, card::content_height(gfx, c)),
    }
}

fn cursor() -> POINT {
    let mut p = POINT::default();
    // SAFETY: valid out pointer.
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    p
}

impl Floating {
    pub fn new(places: Placements) -> Floating {
        Floating { wins: HashMap::new(), places }
    }

    pub fn is_floating(&self, key: &ConnectorKey) -> bool {
        self.wins.contains_key(key)
    }

    fn key_of(&self, hwnd: HWND) -> Option<ConnectorKey> {
        self.wins.iter().find(|(_, w)| w.hwnd == hwnd).map(|(k, _)| k.clone())
    }

    /// Reconcile with the view: a window exists iff its placement says floating and the
    /// connector is tracked and in the view (ARCHITECTURE §7.2). Redraws every window.
    pub fn sync(&mut self, gfx: &Gfx, view: &ViewModel, light: bool) {
        let mut forget = false;
        for c in view.connectors.iter().filter(|c| !c.tracked) {
            self.destroy(&c.key);
            forget |= self.places.map.remove(&c.key).is_some();
        }
        if forget {
            self.places.save();
        }
        let stale: Vec<ConnectorKey> = self
            .wins
            .keys()
            .filter(|k| !view.connectors.iter().any(|c| c.tracked && &c.key == *k))
            .cloned()
            .collect();
        for k in stale {
            self.destroy(&k);
        }
        let wanted: Vec<ConnectorKey> = self.places.map.iter().filter(|(_, p)| p.floating).map(|(k, _)| k.clone()).collect();
        for key in wanted {
            if let Some(c) = view.connectors.iter().find(|c| c.tracked && c.key == key) {
                self.show(gfx, c, light, &key);
            }
        }
    }

    /// Create the window if needed (at its saved placement) and draw it.
    fn show(&mut self, gfx: &Gfx, c: &ConnectorView, light: bool, key: &ConnectorKey) {
        let Some(place) = self.places.map.get(key).cloned() else { return };
        if !self.wins.contains_key(key) {
            let disp = placement::find(&place.display);
            let pos = (disp.work.left + (place.x * disp.dpi) as i32, disp.work.top + (place.y * disp.dpi) as i32);
            // SAFETY: a layered, no-activate, topmost tool window of our registered class.
            let Ok(hwnd) = (unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    CLASS,
                    w!("MeltAlarm"),
                    WS_POPUP,
                    pos.0,
                    pos.1,
                    0,
                    0,
                    None,
                    None,
                    None,
                    None,
                )
            }) else {
                return;
            };
            let win = Win { hwnd, pos, scale: disp.dpi, size: (0, 0), hover: false, pulse: false, pressed: None, drag: None };
            self.wins.insert(key.clone(), win);
            self.render(gfx, c, light, key, &place, None);
            // SAFETY: our own window.
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        } else {
            // Mid-scale: draw at the size being dragged to, anchored like the drag, or the
            // saved (old) size flashes up on every data update.
            let scaling = self.wins.get(key).and_then(|w| w.drag).and_then(|d| match (d.hit, d.live) {
                (Hit::Edge(sx, sy), Some(user)) => Some((sx, sy, d, user)),
                _ => None,
            });
            match scaling {
                Some((sx, sy, d, user)) => {
                    let mut p = place.clone();
                    p.set_scale(user);
                    let r0 = RECT { left: d.pos0.0, top: d.pos0.1, right: d.pos0.0 + d.size0.0, bottom: d.pos0.1 + d.size0.1 };
                    self.render(gfx, c, light, key, &p, Some((sx, sy, r0)));
                }
                None => self.render(gfx, c, light, key, &place, None),
            }
        }
    }

    /// Draw at the window's position, clamped fully onto its display. `anchor`: keep this
    /// corner (px) fixed while scaling (-1/+1 per axis: which edge moves).
    fn render(&mut self, gfx: &Gfx, c: &ConnectorView, light: bool, key: &ConnectorKey, place: &Placement, anchor: Option<(i8, i8, RECT)>) {
        let Some(win) = self.wins.get_mut(key) else { return };
        let (cw, ch) = card_size(gfx, c, place.layout);
        let (ww, wh) = (cw + 2.0 * MARGIN, ch + 2.0 * MARGIN);
        let here = RECT { left: win.pos.0, top: win.pos.1, right: win.pos.0 + win.size.0.max(1), bottom: win.pos.1 + win.size.1.max(1) };
        let disp = placement::display_of(&here);
        let max_user = ((disp.work.right - disp.work.left) as f32 / (ww * disp.dpi))
            .min((disp.work.bottom - disp.work.top) as f32 / (wh * disp.dpi))
            .max(MIN_SCALE);
        let user = place.scale().clamp(MIN_SCALE, max_user);
        let scale = disp.dpi * user;
        let (w, h) = ((ww * scale).ceil() as i32, (wh * scale).ceil() as i32);
        let (mut x, mut y) = win.pos;
        if let Some((sx, sy, r0)) = anchor {
            if sx < 0 {
                x = r0.right - w;
            }
            if sy < 0 {
                y = r0.bottom - h;
            }
        }
        // Keep the card (not its shadow margin) fully on the display.
        let m = (MARGIN * scale) as i32;
        x = x.clamp(disp.work.left - m, (disp.work.right - w + m).max(disp.work.left - m));
        y = y.clamp(disp.work.top - m, (disp.work.bottom - h + m).max(disp.work.top - m));
        win.pos = (x, y);
        win.scale = scale;
        win.size = (w, h);
        let t = palette::theme(light);
        let (hover, pulse) = (win.hover || win.drag.is_some(), win.pulse);
        let hwnd = win.hwnd;
        let _ = gfx.present(hwnd, x, y, ww, wh, scale, |p| paint(p, &t, c, place.layout, cw, ch, hover, pulse));
    }

    /// Flyout → floating (pop-out button, or `tear_off` = dragging its header). The first time,
    /// and when torn off, *Full* at the flyout's spot; otherwise the last floating placement.
    pub fn pop_out(&mut self, gfx: &Gfx, c: &ConnectorView, light: bool, origin: (i32, i32), tear_off: bool) {
        let key = c.key.clone();
        let restore = !tear_off && self.places.map.get(&key).is_some_and(|p| !p.display.is_empty());
        if restore {
            if let Some(p) = self.places.map.get_mut(&key) {
                p.floating = true;
            }
        } else {
            let here = RECT { left: origin.0, top: origin.1, right: origin.0 + 1, bottom: origin.1 + 1 };
            let disp = placement::display_of(&here);
            let scale = self.places.map.get(&key).map_or([1.0, 1.0], |p| p.scale);
            let place = Placement {
                floating: true,
                layout: Layout::Full,
                scale: [scale[0], 1.0],
                display: disp.id,
                x: (origin.0 - disp.work.left) as f32 / disp.dpi,
                y: (origin.1 - disp.work.top) as f32 / disp.dpi,
            };
            self.places.map.insert(key.clone(), place);
        }
        self.places.save();
        self.show(gfx, c, light, &key);
        if tear_off && let Some(win) = self.wins.get_mut(&key) {
            // The mouse button is still down: the new window takes over the drag.
            win.drag = Some(Drag { hit: Hit::Move, cursor0: cursor(), pos0: win.pos, size0: win.size, scale0: 1.0, live: None });
            // SAFETY: our own window, on the thread that owns the pressed mouse button.
            unsafe {
                SetCapture(win.hwnd);
            }
        }
    }

    /// Tray click while floating: bring to front, back onto a display, pulse once.
    pub fn locate(&mut self, gfx: &Gfx, c: &ConnectorView, light: bool) {
        let key = c.key.clone();
        let Some(win) = self.wins.get_mut(&key) else { return };
        win.pulse = true;
        let hwnd = win.hwnd;
        // SAFETY: our own window; no activation.
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            SetTimer(Some(hwnd), TIMER_PULSE, PULSE_MS, None);
        }
        if let Some(place) = self.places.map.get(&key).cloned() {
            self.render(gfx, c, light, &key, &place, None);
        }
    }

    fn destroy(&mut self, key: &ConnectorKey) {
        if let Some(w) = self.wins.remove(key) {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
        }
    }

    /// App exit: close the windows, keep the placements (restored at the next start).
    pub fn close_all(&mut self) {
        let keys: Vec<ConnectorKey> = self.wins.keys().cloned().collect();
        for k in keys {
            self.destroy(&k);
        }
    }

    /// Remember where the window is now.
    fn save_position(&mut self, key: &ConnectorKey) {
        let Some(win) = self.wins.get(key) else { return };
        let r = RECT { left: win.pos.0, top: win.pos.1, right: win.pos.0 + win.size.0, bottom: win.pos.1 + win.size.1 };
        let disp = placement::display_of(&r);
        if let Some(p) = self.places.map.get_mut(key) {
            p.display = disp.id;
            p.x = (win.pos.0 - disp.work.left) as f32 / disp.dpi;
            p.y = (win.pos.1 - disp.work.top) as f32 / disp.dpi;
            p.set_scale(win.scale / disp.dpi);
        }
        self.places.save();
    }

    fn hit(&self, gfx: &Gfx, c: &ConnectorView, key: &ConnectorKey, client: POINT) -> Hit {
        let (Some(win), Some(place)) = (self.wins.get(key), self.places.map.get(key)) else { return Hit::Nothing };
        let (cw, ch) = card_size(gfx, c, place.layout);
        let (x, y) = (client.x as f32 / win.scale - MARGIN, client.y as f32 / win.scale - MARGIN);
        let inside = |r: (f32, f32, f32, f32)| (r.0..r.0 + r.2).contains(&x) && (r.1..r.1 + r.3).contains(&y);
        if inside(close_rect(place.layout, cw)) {
            return Hit::Close;
        }
        if inside(tab_rect(cw, ch)) {
            return Hit::Tab;
        }
        if place.layout == Layout::Full
            && let Some((dx, dy, dw, dh)) = card::dismiss_rect(gfx, c, cw - 2.0 * PAD)
            && inside((PAD + dx, TOP + dy, dw, dh))
        {
            return Hit::Dismiss;
        }
        if !(-EDGE..cw + EDGE).contains(&x) || !(-EDGE..ch + EDGE).contains(&y) {
            return Hit::Nothing;
        }
        let sx = if x < EDGE { -1 } else if x > cw - EDGE { 1 } else { 0 };
        let sy = if y < EDGE { -1 } else if y > ch - EDGE { 1 } else { 0 };
        if sx != 0 || sy != 0 { Hit::Edge(sx, sy) } else { Hit::Move }
    }

    /// Mouse and timer messages of a floating window.
    pub fn on_message(&mut self, gfx: &Gfx, view: &ViewModel, light: bool, hwnd: HWND, msg: u32, wp: usize) -> Action {
        use windows::Win32::UI::WindowsAndMessaging::{
            WM_CAPTURECHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WM_SETCURSOR, WM_TIMER,
        };
        const WM_MOUSELEAVE: u32 = 0x02A3;
        let Some(key) = self.key_of(hwnd) else { return Action::None };
        let Some(c) = view.connectors.iter().find(|c| c.key == key) else { return Action::None };
        let Some(place) = self.places.map.get(&key).cloned() else { return Action::None };
        let Some(win) = self.wins.get(&key) else { return Action::None };
        let (drag, pos, size, scale) = (win.drag, win.pos, win.size, win.scale);
        let screen = cursor();
        let mut client = screen;
        // SAFETY: our own window.
        unsafe {
            let _ = ScreenToClient(hwnd, &mut client);
        }
        let hit = self.hit(gfx, c, &key, client);
        let rect = |pos: (i32, i32), size: (i32, i32)| RECT { left: pos.0, top: pos.1, right: pos.0 + size.0, bottom: pos.1 + size.1 };

        match msg {
            WM_SETCURSOR => {
                let id = match drag.map_or(hit, |d| d.hit) {
                    Hit::Close | Hit::Tab | Hit::Dismiss => IDC_HAND,
                    Hit::Edge(sx, sy) if sx * sy > 0 => IDC_SIZENWSE,
                    Hit::Edge(sx, sy) if sx * sy < 0 => IDC_SIZENESW,
                    Hit::Edge(0, _) => IDC_SIZENS,
                    Hit::Edge(..) => IDC_SIZEWE,
                    _ => IDC_ARROW,
                };
                // SAFETY: shared system cursor.
                unsafe {
                    if let Ok(cur) = LoadCursorW(None, id) {
                        SetCursor(Some(cur));
                    }
                }
            }
            WM_MOUSEMOVE => match drag {
                Some(d) => {
                    let (dx, dy) = (screen.x - d.cursor0.x, screen.y - d.cursor0.y);
                    match d.hit {
                        Hit::Move => {
                            let to = (d.pos0.0 + dx, d.pos0.1 + dy);
                            if let Some(w) = self.wins.get_mut(&key) {
                                w.pos = to;
                            }
                            // SAFETY: our own window; no activation, no z-order change.
                            unsafe {
                                let _ = SetWindowPos(hwnd, None, to.0, to.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                            }
                        }
                        Hit::Edge(sx, sy) => {
                            // Uniform: the larger relative change of the dragged axes wins.
                            let rx = (d.size0.0 + i32::from(sx) * dx) as f32 / d.size0.0.max(1) as f32;
                            let ry = (d.size0.1 + i32::from(sy) * dy) as f32 / d.size0.1.max(1) as f32;
                            let ratio = match (sx, sy) {
                                (0, _) => ry,
                                (_, 0) => rx,
                                _ => rx.max(ry),
                            };
                            let mut p = place.clone();
                            p.set_scale(d.scale0 * ratio);
                            if let Some(w) = self.wins.get_mut(&key) {
                                w.pos = d.pos0;
                                if let Some(drag) = &mut w.drag {
                                    drag.live = Some(p.scale());
                                }
                            }
                            self.render(gfx, c, light, &key, &p, Some((sx, sy, rect(d.pos0, d.size0))));
                        }
                        _ => {}
                    }
                }
                None => {
                    let newly = self.wins.get_mut(&key).is_some_and(|w| !std::mem::replace(&mut w.hover, true));
                    if newly {
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        // SAFETY: valid struct for our window.
                        unsafe {
                            let _ = TrackMouseEvent(&mut tme);
                        }
                        self.render(gfx, c, light, &key, &place, None);
                    }
                }
            },
            WM_MOUSELEAVE => {
                if let Some(w) = self.wins.get_mut(&key) {
                    w.hover = false;
                }
                if drag.is_none() {
                    self.render(gfx, c, light, &key, &place, None);
                }
            }
            WM_LBUTTONDOWN => match hit {
                Hit::Close | Hit::Tab | Hit::Dismiss => {
                    if let Some(w) = self.wins.get_mut(&key) {
                        w.pressed = Some(hit);
                    }
                }
                Hit::Move | Hit::Edge(..) => {
                    let user = scale / placement::display_of(&rect(pos, size)).dpi;
                    if let Some(w) = self.wins.get_mut(&key) {
                        w.drag = Some(Drag { hit, cursor0: screen, pos0: pos, size0: size, scale0: user, live: None });
                    }
                    // SAFETY: our own window.
                    unsafe {
                        SetCapture(hwnd);
                    }
                }
                Hit::Nothing => {}
            },
            WM_LBUTTONUP => {
                let pressed = self.wins.get_mut(&key).and_then(|w| w.pressed.take());
                if drag.is_some() {
                    self.end_drag(gfx, c, light, &key);
                } else if pressed.is_some() && pressed == Some(hit) {
                    match hit {
                        Hit::Close => {
                            if let Some(p) = self.places.map.get_mut(&key) {
                                p.floating = false;
                            }
                            self.places.save();
                            self.destroy(&key);
                        }
                        Hit::Tab => {
                            if let Some(p) = self.places.map.get_mut(&key) {
                                p.layout = if p.layout == Layout::Compact { Layout::Full } else { Layout::Compact };
                            }
                            self.places.save();
                            if let Some(p) = self.places.map.get(&key).cloned() {
                                self.render(gfx, c, light, &key, &p, None);
                            }
                        }
                        Hit::Dismiss => return Action::Dismiss(key),
                        _ => {}
                    }
                }
            }
            WM_CAPTURECHANGED => {
                if drag.is_some() {
                    self.end_drag(gfx, c, light, &key);
                }
            }
            WM_RBUTTONUP => return Action::Menu(screen.x, screen.y),
            WM_TIMER if wp == TIMER_PULSE => {
                if let Some(w) = self.wins.get_mut(&key) {
                    w.pulse = false;
                }
                // SAFETY: our own window and timer.
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_PULSE);
                }
                self.render(gfx, c, light, &key, &place, None);
            }
            _ => {}
        }
        Action::None
    }

    fn end_drag(&mut self, gfx: &Gfx, c: &ConnectorView, light: bool, key: &ConnectorKey) {
        let Some(w) = self.wins.get_mut(key) else { return };
        if w.drag.take().is_none() {
            return;
        }
        // SAFETY: releases our capture (a no-op if it is already gone).
        unsafe {
            let _ = ReleaseCapture();
        }
        self.save_position(key);
        // Redraw at the final display's DPI and without the drag's hover state if the mouse left.
        if let Some(p) = self.places.map.get(key).cloned() {
            self.render(gfx, c, light, key, &p, None);
        }
    }
}

fn close_rect(layout: Layout, cw: f32) -> (f32, f32, f32, f32) {
    match layout {
        Layout::Compact => (cw - 8.0 - 24.0, 8.0, 24.0, 24.0),
        Layout::Full => card::header_button_rect(PAD, TOP, cw - 2.0 * PAD),
    }
}

fn tab_rect(cw: f32, ch: f32) -> (f32, f32, f32, f32) {
    (cw / 2.0 - 18.0, ch - 9.0, 36.0, 18.0)
}

fn paint(p: &Painter, t: &Theme, c: &ConnectorView, layout: Layout, cw: f32, ch: f32, hover: bool, pulse: bool) {
    let (x, y) = (MARGIN, MARGIN);
    let (accent, hover_border, tab_bg) = (t.accent, t.hover_border, t.tab_bg);
    card::draw_card(p, t, x, y, cw, ch, if hover { hover_border } else { t.border });
    match layout {
        Layout::Full => {
            let button = if hover { HeaderButton::Close } else { HeaderButton::None };
            card::draw_content(p, t, c, x + PAD, y + TOP, cw - 2.0 * PAD, button);
        }
        Layout::Compact => {
            draw_compact(p, t, c, x, y);
            if hover {
                let (bx, by, bw, bh) = close_rect(layout, cw);
                card::draw_close(p, t, x + bx, y + by, bw, bh);
            }
        }
    }
    if hover {
        // Tab on the bottom edge: ⌄ = Full, ⌃ = Compact.
        let (tx, ty, tw, th) = tab_rect(cw, ch);
        let (tx, ty) = (x + tx, y + ty);
        p.fill_rrect(tx, ty, tw, th, th / 2.0, tab_bg, 1.0);
        p.stroke_rrect(tx + 0.5, ty + 0.5, tw - 1.0, th - 1.0, th / 2.0, hover_border, 1.0, 1.0);
        let (cx, cy) = (tx + tw / 2.0, ty + th / 2.0);
        let d = if layout == Layout::Compact { 2.0 } else { -2.0 };
        p.line(cx - 4.0, cy - d, cx, cy + d, t.fg3, 1.0, 1.4);
        p.line(cx, cy + d, cx + 4.0, cy - d, t.fg3, 1.0, 1.4);
        // Resize grip.
        let (gx, gy) = (x + cw - 4.0, y + ch - 4.0);
        p.line(gx, gy - 6.0, gx - 6.0, gy, t.fg3, 0.7, 1.2);
        p.line(gx, gy - 3.0, gx - 3.0, gy, t.fg3, 0.7, 1.2);
    }
    if pulse {
        p.stroke_rrect(x - 5.0, y - 5.0, cw + 10.0, ch + 10.0, 13.0, accent, 0.25, 6.0);
        p.stroke_rrect(x - 1.5, y - 1.5, cw + 3.0, ch + 3.0, 9.5, accent, 1.0, 2.0);
    }
}

/// DESIGN.md "Floating monitor", Compact: 250 × 152, fixed in every state.
fn draw_compact(p: &Painter, t: &Theme, c: &ConnectorView, x: f32, y: f32) {
    card::draw_title(p, t, c, x + 14.0, y + 12.0, true);
    let y0 = y + 40.0;
    let bar_h = 56.0;
    let stale = if c.stale { 0.45 } else { 1.0 };
    for (i, wv) in c.wires.iter().enumerate() {
        let cx = x + (COMPACT.0 - 5.0 * 32.0) / 2.0 + i as f32 * 32.0;
        let (bar, txt) = level_colors(t, wv.level);
        card::draw_cut_bar(p, cx - 5.0, y0, 10.0, bar_h, wv.amps, c.bar_limit, c.bar_rating, t.track, bar, stale, t.fg3);
        let value = wv.amps.map_or("—".to_owned(), |a| format!("{a:.1}"));
        let color = if wv.amps.is_some() { txt } else { t.fg3 };
        p.text(&value, num(13.0, 600), cx - 16.0, y0 + bar_h + 4.0, 32.0, 17.0, color, stale, Align::Center);
    }
    let color = if c.summary_level == Level::Normal { t.fg3 } else { level_colors(t, c.summary_level).1 };
    p.text(&c.summary, num(12.0, 400), x + 14.0, y + 125.0, COMPACT.0 - 28.0, 15.0, color, 1.0, Align::Left);
}
