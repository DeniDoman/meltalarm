//! MeltAlarm for Windows: the frontend and composition root (docs/ARCHITECTURE.md §7).
//!
//! The UI thread owns the portable `Runtime` (core + log + settings). After every call into
//! it, `reconcile()` makes tray, popup, overlay, audio, hotkey and autostart match the view.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Drawing primitives take position, size, color and alpha; a struct would add noise, not clarity.
#![allow(clippy::too_many_arguments)]

mod anim;
mod audio;
mod autostart;
mod card;
mod edge;
mod floating;
mod gfx;
mod glyph;
mod lifecycle;
mod menu;
mod overlay;
mod paint;
mod palette;
mod placement;
mod popup;
mod strip;
mod sys;
mod tray;

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use meltalarm_core::{Glyph, LogEvent, UserAction, ViewModel};
use meltalarm_lifecycle::Autostart;
use meltalarm_model::ConnectorKey;
use meltalarm_runtime::{Host, Lifecycle, LogSink, Paths, Runtime, Update};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const WM_WAKE: u32 = WM_APP + 2;
const NIN_SELECT: u32 = WM_USER;
const NIN_KEYSELECT: u32 = WM_USER | 1;
const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;
const WM_MOUSELEAVE: u32 = 0x02A3;
const PBT_APMSUSPEND: u32 = 0x4;
const PBT_APMRESUMESUSPEND: u32 = 0x7;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x12;
const TIMER_WAKE: usize = 1;
const TIMER_BLINK: usize = 2;
/// Runs only while something moves (DESIGN.md "Motion").
const TIMER_ANIM: usize = 3;
const HOTKEY_SNOOZE: i32 = 1;

#[cfg(feature = "simulate")]
const APP_NAME: &str = "MeltAlarm (simulated)";
#[cfg(not(feature = "simulate"))]
const APP_NAME: &str = "MeltAlarm";

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// What a click on the current notification opens.
enum Click {
    Open(&'static str),
    /// That connector's flyout (or its floating view, located).
    Flyout(ConnectorKey),
}

/// Work that must run *outside* the App borrow (modal loops re-enter the window procedure).
enum Deferred {
    Menu(i32, i32),
    ConfirmExit,
    Quit(Option<String>),
}

struct App {
    hwnd: HWND,
    runtime: Runtime,
    gfx: gfx::Gfx,
    tray: tray::Tray,
    popup: popup::Popup,
    overlay: overlay::Overlay,
    strip: strip::Strips,
    audio: audio::Audio,
    /// The last caution chime played (`CautionView::chime`).
    chimed: Option<u32>,
    view: Arc<ViewModel>,
    light: bool,
    blink_on: bool,
    blinking: bool,
    /// The animation timer runs.
    animating: bool,
    hotkey: bool,
    autostart_applied: Option<bool>,
    taskbar_created: u32,
    popup_on_start: bool,
    notice_shown: Option<u32>,
    /// Not the installed copy (or a dev build): never touches the startup task (Spec L2).
    portable: bool,
    control_msg: u32,
    /// Just installed: explain how to keep the icon visible (Spec §7.1).
    welcome: bool,
    /// What a click on the current notification opens.
    notice_click: Option<Click>,
    floating: floating::Floating,
    /// Mouse press in the flyout: where, and on what (pop-out button or header).
    popup_press: Option<(i32, i32, popup::PopupHit)>,
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

impl App {
    fn after(&mut self, u: Update) -> Option<Deferred> {
        if let Some(t) = u.next_wake {
            let ms = t.saturating_duration_since(Instant::now()).as_millis().clamp(10, 60_000) as u32;
            // SAFETY: our own window.
            unsafe { SetTimer(Some(self.hwnd), TIMER_WAKE, ms, None) };
        }
        let deferred = match u.lifecycle {
            Some(Lifecycle::DiscoveryFailed(m)) => Some(Deferred::Quit(Some(m))),
            Some(Lifecycle::Fatal(m)) => Some(Deferred::Quit(Some(format!("Monitoring stopped: {m}")))),
            None => None,
        };
        self.reconcile();
        deferred
    }

    fn reconcile(&mut self) {
        let view = self.runtime.view(Instant::now());
        let alarm_glyph = view.tracked().any(|c| c.glyph == Glyph::Alarm);
        if alarm_glyph != self.blinking {
            // SAFETY: our own window.
            unsafe {
                if alarm_glyph {
                    SetTimer(Some(self.hwnd), TIMER_BLINK, 500, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_BLINK);
                    self.blink_on = true;
                }
            }
            self.blinking = alarm_glyph;
        }
        self.tray.sync(&view, self.light, self.blink_on);
        // Notifications need an icon to come from; a notice waits until there is one.
        if let Some(n) = &view.notice
            && self.notice_shown != Some(n.id)
            && self.tray.has_icons()
        {
            let kind = if n.warning { tray::Notice::Warning } else { tray::Notice::Info };
            self.tray.notify(kind, &n.title, &n.text);
            self.notice_click = n.connector.clone().map(Click::Flyout);
            self.notice_shown = Some(n.id);
        }
        if self.welcome && self.tray.has_icons() {
            self.welcome = false;
            self.tray.notify(tray::Notice::Info, "MeltAlarm is running", "Click here to keep its icon visible on the taskbar.");
            self.notice_click = Some(Click::Open("ms-settings:taskbar"));
        }
        if self.popup_on_start && !view.connectors.is_empty() {
            self.popup_on_start = false;
            self.popup.toggle(&view.connectors[0].key, None, &self.gfx, &view, self.light);
        }
        self.popup.update(&self.gfx, &view, self.light);
        self.floating.sync(&self.gfx, &view, self.light);
        self.overlay.sync(&self.gfx, view.alarm.as_ref());
        // The alarm takes over at once: the strip doesn't retract under the dropping notch.
        self.strip.sync(&self.gfx, view.caution.as_ref(), view.alarm.is_some());
        self.audio.sync(view.audio.as_ref());
        if let Some(c) = &view.caution
            && self.chimed != Some(c.chime)
        {
            self.chimed = Some(c.chime);
            self.audio.chime();
        }

        let want_hotkey = self.overlay.visible() && view.alarm.as_ref().is_some_and(|a| a.snooze);
        if want_hotkey != self.hotkey {
            // SAFETY: our own window and hotkey id.
            unsafe {
                if want_hotkey {
                    let ok = RegisterHotKey(Some(self.hwnd), HOTKEY_SNOOZE, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, u32::from(b'G')).is_ok();
                    self.overlay.hotkey_hint = ok;
                } else {
                    let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_SNOOZE);
                    self.overlay.hotkey_hint = true;
                }
            }
            self.hotkey = want_hotkey;
        }

        if !self.portable {
            let want = view.settings.run_at_startup;
            if self.autostart_applied != Some(want) {
                let exe = lifecycle::installed_exe();
                let task = autostart::TaskScheduler;
                let actual = task.target().is_some_and(|t| lifecycle::same_file(&t, &exe));
                let result = if actual == want { Ok(()) } else { task.set(want.then_some(exe.as_path())) };
                let applied = if result.is_ok() { want } else { actual };
                self.autostart_applied = Some(applied);
                if let Err(e) = result {
                    self.runtime.log_event(LogEvent::ConfigWarning { text: format!("could not change the startup task: {e}") });
                    self.runtime.user(UserAction::SetRunAtStartup(applied), Instant::now());
                }
            }
        }
        self.view = view;
        self.animate();
    }

    /// Keep the animation timer running exactly while something moves. Called after every
    /// reconcile and at the end of every window procedure, because a transition can start from
    /// any of them (a tray click, a lost focus, Esc).
    fn animate(&mut self) {
        let moving = self.popup.moving() || self.overlay.moving() || self.strip.moving();
        if moving != self.animating {
            // SAFETY: our own window.
            unsafe {
                if moving {
                    SetTimer(Some(self.hwnd), TIMER_ANIM, anim::FRAME_MS, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_ANIM);
                }
            }
            self.animating = moving;
        }
    }

    fn user(&mut self, a: UserAction) -> Option<Deferred> {
        let u = self.runtime.user(a, Instant::now());
        self.after(u)
    }

    fn on_main(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> (Option<LRESULT>, Option<Deferred>) {
        let handled = Some(LRESULT(0));
        match msg {
            WM_WAKE => {
                let u = self.runtime.pump(Instant::now());
                (handled, self.after(u))
            }
            WM_TIMER if wp.0 == TIMER_WAKE => {
                let u = self.runtime.wake(Instant::now());
                (handled, self.after(u))
            }
            WM_TIMER if wp.0 == TIMER_ANIM => {
                self.popup.tick();
                self.overlay.tick();
                self.strip.tick();
                self.animate();
                (handled, None)
            }
            WM_TIMER if wp.0 == TIMER_BLINK => {
                self.blink_on = !self.blink_on;
                self.tray.sync(&self.view.clone(), self.light, self.blink_on);
                (handled, None)
            }
            tray::WM_TRAY => {
                let event = (lp.0 & 0xFFFF) as u32;
                let id = ((lp.0 >> 16) & 0xFFFF) as u32;
                match event {
                    NIN_SELECT | NIN_KEYSELECT => {
                        let view = self.view.clone();
                        match view.connectors.iter().find(|c| tray::uid(&c.key) == id) {
                            // One view per connector: a floating one is located, not doubled (Spec §7.4).
                            Some(c) if self.floating.is_floating(&c.key) => self.floating.locate(&self.gfx, c, self.light),
                            Some(c) => {
                                let rect = self.tray.icon_rect(id);
                                self.popup.toggle(&c.key, rect, &self.gfx, &view, self.light);
                            }
                            None => {}
                        }
                        (handled, None)
                    }
                    NIN_BALLOONUSERCLICK => {
                        match self.notice_click.take() {
                            Some(Click::Open(target)) => sys::open_path(std::path::Path::new(target)),
                            Some(Click::Flyout(key)) => self.open_connector(&key),
                            None => {}
                        }
                        (handled, None)
                    }
                    WM_CONTEXTMENU => {
                        let (x, y) = ((wp.0 & 0xFFFF) as i16 as i32, ((wp.0 >> 16) & 0xFFFF) as i16 as i32);
                        (handled, Some(Deferred::Menu(x, y)))
                    }
                    _ => (handled, None),
                }
            }
            WM_HOTKEY if wp.0 as i32 == HOTKEY_SNOOZE => (handled, self.user(UserAction::Snooze)),
            WM_POWERBROADCAST => {
                match wp.0 as u32 {
                    PBT_APMSUSPEND => self.runtime.suspend(Instant::now()),
                    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                        let u = self.runtime.resume(Instant::now());
                        self.after(u);
                    }
                    _ => {}
                }
                (Some(LRESULT(1)), None)
            }
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
                self.light = sys::system_light();
                self.reconcile();
                (None, None)
            }
            m if m == self.control_msg => match wp.0 {
                lifecycle::SHOW => {
                    self.show();
                    (Some(LRESULT(1)), None)
                }
                lifecycle::ALARM => {
                    let alarm = self.view.alarm.as_ref().is_some_and(|a| !a.test);
                    (Some(LRESULT(if alarm { lifecycle::IN_ALARM } else { lifecycle::NO_ALARM })), None)
                }
                lifecycle::QUIT => (Some(LRESULT(1)), Some(Deferred::Quit(None))),
                _ => (Some(LRESULT(0)), None),
            },
            m if m == self.taskbar_created => {
                self.tray.forget_all();
                self.reconcile();
                (handled, None)
            }
            _ => (None, None),
        }
    }

    /// A second launch (Spec L6): open the popup of the first tracked connector.
    fn show(&mut self) {
        let view = self.view.clone();
        match view.connectors.iter().find(|c| c.tracked) {
            Some(c) if self.floating.is_floating(&c.key) => self.floating.locate(&self.gfx, c, self.light),
            Some(c) if self.popup.connector.as_ref() != Some(&c.key) => {
                let rect = self.tray.icon_rect(tray::uid(&c.key));
                self.popup.toggle(&c.key, rect, &self.gfx, &view, self.light);
            }
            Some(_) => {}
            None => {
                let text = view.connecting.as_deref().and_then(|t| t.strip_prefix("MeltAlarm · ")).unwrap_or("Connecting to the PSU…");
                self.tray.notify(tray::Notice::Info, "MeltAlarm is already running", &capitalize(text));
                self.notice_click = None;
            }
        }
    }

    /// Bring a connector's view up: locate it if floating, otherwise open its flyout.
    fn open_connector(&mut self, key: &ConnectorKey) {
        let view = self.view.clone();
        let Some(c) = view.connectors.iter().find(|c| &c.key == key) else { return };
        if self.floating.is_floating(key) {
            self.floating.locate(&self.gfx, c, self.light);
        } else if self.popup.connector.as_ref() != Some(key) {
            let rect = if c.tracked { self.tray.icon_rect(tray::uid(key)) } else { None };
            self.popup.toggle(key, rect, &self.gfx, &view, self.light);
        }
    }

    /// Flyout → floating view (Spec §7.4). `tear_off`: the header is being dragged.
    fn pop_out(&mut self, tear_off: bool) {
        let view = self.view.clone();
        let Some(c) = self.popup.shown(&view) else { return };
        let origin = self.popup.origin;
        self.popup.hide_now();
        self.floating.pop_out(&self.gfx, c, self.light, origin, tear_off);
    }

    fn shutdown(&mut self) {
        self.floating.close_all();
        self.audio.stop();
        self.overlay.close();
        self.strip.close();
        self.popup.hide_now();
        self.tray.remove_all();
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn run_deferred(hwnd: HWND, d: Deferred) {
    match d {
        Deferred::Menu(x, y) => {
            let Some(Some(menu)) = with_app(|a| a.build_menu()) else { return };
            // SAFETY: standard tray-menu sequence on our own window and menu.
            let cmd = unsafe {
                let _ = SetForegroundWindow(hwnd);
                let r = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, x, y, None, hwnd, None);
                let _ = DestroyMenu(menu);
                let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                r.0 as u32
            };
            if cmd != 0
                && let Some(Some(next)) = with_app(|a| a.on_command(cmd))
            {
                run_deferred(hwnd, next);
            }
        }
        Deferred::ConfirmExit => {
            if sys::confirm(hwnd, APP_NAME, "Exit MeltAlarm?\n\nMonitoring of the GPU power cable will stop until MeltAlarm runs again.") {
                with_app(|a| a.shutdown());
                // SAFETY: ends the message loop.
                unsafe { PostQuitMessage(0) };
            }
        }
        Deferred::Quit(message) => {
            with_app(|a| a.shutdown());
            if let Some(m) = message {
                sys::message(APP_NAME, &m, true);
            }
            // SAFETY: ends the message loop.
            unsafe { PostQuitMessage(0) };
        }
    }
}

extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let r = with_app(|a| a.on_main(msg, wp, lp));
    let (result, deferred) = r.unwrap_or((None, None));
    if let Some(d) = deferred {
        run_deferred(hwnd, d);
    }
    with_app(|a| a.animate());
    // SAFETY: default processing for everything we did not handle.
    result.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

extern "system" fn popup_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let close = (msg == WM_ACTIVATE && (wp.0 & 0xFFFF) as u32 == WA_INACTIVE) || (msg == WM_KEYDOWN && wp.0 as u16 == VK_ESCAPE.0);
    if close {
        with_app(|a| a.popup.hide());
    }
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
    match msg {
        WM_LBUTTONDOWN => {
            with_app(|a| {
                let hit = a.popup.hit(&a.gfx, &a.view, x, y);
                a.popup_press = (hit != popup::PopupHit::Other).then_some((x, y, hit));
            });
        }
        WM_MOUSEMOVE => {
            // Dragging the header tears the view off; a plain click on it does nothing.
            with_app(|a| {
                if let Some((px, py, popup::PopupHit::Header)) = a.popup_press
                    && ((x - px).abs() > 4 || (y - py).abs() > 4)
                {
                    a.popup_press = None;
                    a.pop_out(true);
                }
            });
        }
        WM_LBUTTONUP => {
            with_app(|a| {
                let hit = a.popup.hit(&a.gfx, &a.view, x, y);
                match a.popup_press.take() {
                    Some((_, _, popup::PopupHit::PopOut)) if hit == popup::PopupHit::PopOut => a.pop_out(false),
                    Some((_, _, popup::PopupHit::Dismiss)) if hit == popup::PopupHit::Dismiss => {
                        if let Some(key) = a.popup.connector.clone() {
                            a.user(UserAction::DismissNote(key));
                        }
                    }
                    _ => {}
                }
            });
        }
        _ => {}
    }
    with_app(|a| a.animate());
    // SAFETY: default processing.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
    match msg {
        // Never take focus from the game.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
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
            with_app(|a| {
                let hover = a.overlay.hit_button(hwnd, x, y).then_some(hwnd);
                if hover != a.overlay.hover {
                    a.overlay.hover = hover;
                    let view = a.view.clone();
                    a.overlay.sync(&a.gfx, view.alarm.as_ref());
                }
            });
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                if a.overlay.hover.take().is_some() {
                    let view = a.view.clone();
                    a.overlay.sync(&a.gfx, view.alarm.as_ref());
                }
            });
            return LRESULT(0);
        }
        WM_SETCURSOR => {
            // SAFETY: loading a shared system cursor.
            unsafe {
                if let Ok(c) = LoadCursorW(None, IDC_HAND) {
                    SetCursor(Some(c));
                }
            }
            return LRESULT(1);
        }
        WM_LBUTTONUP => {
            let r = with_app(|a| if a.overlay.hit_button(hwnd, x, y) { a.user(UserAction::Snooze) } else { None });
            match r {
                Some(Some(d)) => run_deferred(hwnd, d),
                Some(None) => {}
                // App busy (re-entrant call): retry once the current handler returns.
                // SAFETY: re-posting our own message.
                None => unsafe {
                    let _ = PostMessageW(Some(hwnd), msg, wp, lp);
                },
            }
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: default processing.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

extern "system" fn float_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Never take focus from the game (Spec §7.4).
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    let action = with_app(|a| {
        let view = a.view.clone();
        (a.hwnd, a.floating.on_message(&a.gfx, &view, a.light, hwnd, msg, wp.0))
    });
    match action {
        Some((main, floating::Action::Menu(x, y))) => run_deferred(main, Deferred::Menu(x, y)),
        Some((_, floating::Action::Dismiss(key))) => {
            with_app(|a| a.user(UserAction::DismissNote(key)));
        }
        _ => {}
    }
    if msg == WM_SETCURSOR {
        return LRESULT(1);
    }
    // SAFETY: default processing.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

extern "system" fn strip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Never take focus; everything else is default (the strip is click-through).
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    // SAFETY: default processing.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn register_class(name: PCWSTR, proc: WNDPROC) {
    // SAFETY: registering a window class with static name and procedure.
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: proc,
            hInstance: hinstance.into(),
            lpszClassName: name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

fn main() {
    #[cfg(feature = "simulate")]
    let (instance_name, dir_name) = (w!("Local\\MeltAlarm.Simulated"), "MeltAlarm-sim");
    #[cfg(not(feature = "simulate"))]
    let (instance_name, dir_name) = (w!("Local\\MeltAlarm.Instance"), "MeltAlarm");

    let paths = Paths {
        config_dir: sys::app_dir("APPDATA").with_file_name(dir_name),
        log_dir: sys::app_dir("LOCALAPPDATA").with_file_name(dir_name),
    };
    // SAFETY: plain Win32 initialization call.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    // Install, update, uninstall, or hand off to a running instance (Spec §4.4).
    #[cfg(not(feature = "simulate"))]
    let portable = match lifecycle::launch(&paths) {
        lifecycle::Start::Monitor { portable } => portable,
        lifecycle::Start::Exit => return,
    };
    #[cfg(feature = "simulate")]
    let portable = true;

    // SAFETY: plain Win32 call; the handle lives until the process exits.
    unsafe {
        let _instance = CreateMutexW(None, true, instance_name);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return; // started at the same moment as another instance
        }
    }
    sys::allow_dark_menus();
    let log_path = paths.log_dir.join("alarms.log");
    let places = placement::Placements::load(paths.config_dir.join("window.toml"));
    std::panic::set_hook(Box::new(move |info| {
        let line = LogEvent::MonitoringStopped { reason: format!("internal error: {info}") }.format(&sys::wall_clock());
        LogSink::new(log_path.clone()).write(&line);
        sys::message(APP_NAME, &format!("MeltAlarm stopped because of an internal error.\n\n{info}"), true);
    }));

    register_class(lifecycle::MAIN_CLASS, Some(main_proc));
    register_class(w!("MeltAlarmPopup"), Some(popup_proc));
    register_class(overlay::CLASS, Some(overlay_proc));
    register_class(floating::CLASS, Some(float_proc));
    register_class(strip::CLASS, Some(strip_proc));

    // SAFETY: creating our hidden main window and the (hidden) popup window.
    let (hwnd, popup_hwnd, taskbar_created) = unsafe {
        let hwnd =
            CreateWindowExW(WINDOW_EX_STYLE(0), lifecycle::MAIN_CLASS, w!("MeltAlarm"), WS_POPUP, 0, 0, 0, 0, None, None, None, None)
                .expect("main window");
        let popup = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("MeltAlarmPopup"),
            w!("MeltAlarm"),
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
        .expect("popup window");
        let tc = RegisterWindowMessageW(w!("TaskbarCreated"));
        // We run elevated: let Explorer's (medium integrity) messages through UIPI.
        let _ = ChangeWindowMessageFilterEx(hwnd, tc, MSGFLT_ALLOW, None);
        let _ = ChangeWindowMessageFilterEx(hwnd, tray::WM_TRAY, MSGFLT_ALLOW, None);
        (hwnd, popup, tc)
    };

    #[cfg(feature = "simulate")]
    let drivers: Vec<Box<dyn meltalarm_source_api::Driver>> = vec![Box::new(meltalarm_source_sim::SimDriver)];
    #[cfg(not(feature = "simulate"))]
    let drivers: Vec<Box<dyn meltalarm_source_api::Driver>> = vec![Box::new(meltalarm_source_msi::MsiDriver)];

    let raw = hwnd.0 as isize;
    let runtime = Runtime::start(Host {
        drivers,
        paths,
        wall_clock: sys::wall_clock,
        waker: Box::new(move || {
            // SAFETY: posting to our window from the acquisition thread is allowed.
            unsafe {
                let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0));
            }
        }),
    });
    let gfx = gfx::Gfx::new().expect("Direct2D/DirectWrite");
    let view = runtime.view(Instant::now());
    let popup_on_start = cfg!(feature = "simulate") && std::env::args().any(|a| a == "--popup");
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            hwnd,
            runtime,
            gfx,
            tray: tray::Tray::new(hwnd),
            popup: popup::Popup::new(popup_hwnd),
            overlay: overlay::Overlay::new(),
            strip: strip::Strips::new(),
            audio: audio::Audio::default(),
            chimed: None,
            view,
            light: sys::system_light(),
            blink_on: true,
            blinking: false,
            animating: false,
            hotkey: false,
            autostart_applied: None,
            taskbar_created,
            popup_on_start,
            notice_shown: None,
            portable,
            control_msg: lifecycle::control_message(),
            welcome: std::env::args().any(|a| a == "--installed"),
            notice_click: None,
            floating: floating::Floating::new(places),
            popup_press: None,
        })
    });
    if cfg!(feature = "simulate") && std::env::args().any(|a| a == "--test-alarm") {
        with_app(|a| a.user(UserAction::TestAlarm));
    }

    let mut msg = MSG::default();
    // SAFETY: standard message loop.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    APP.with(|a| {
        if let Some(mut app) = a.borrow_mut().take() {
            app.shutdown();
        }
    });
}
