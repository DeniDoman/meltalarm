//! MeltAlarm for Windows: the frontend and composition root (docs/ARCHITECTURE.md §7).
//!
//! The UI thread owns the portable `Runtime` (core + log + settings). After every call into
//! it, `reconcile()` makes tray, popup, overlay, audio, hotkey and autostart match the view.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Drawing primitives take position, size, color and alpha; a struct would add noise, not clarity.
#![allow(clippy::too_many_arguments)]

mod audio;
mod autostart;
mod gfx;
mod glyph;
mod overlay;
mod popup;
#[cfg(feature = "simulate")]
mod sim;
mod sys;
mod tray;

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use meltalarm_core::{Glyph, LogEvent, UserAction, ViewModel};
use meltalarm_runtime::{Host, Lifecycle, LogSink, Paths, Runtime, Update};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey,
    VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const WM_WAKE: u32 = WM_APP + 2;
const NIN_SELECT: u32 = WM_USER;
const NIN_KEYSELECT: u32 = WM_USER | 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const PBT_APMSUSPEND: u32 = 0x4;
const PBT_APMRESUMESUSPEND: u32 = 0x7;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x12;
const TIMER_WAKE: usize = 1;
const TIMER_BLINK: usize = 2;
const HOTKEY_SNOOZE: i32 = 1;
const OVERLAY_CLASS: PCWSTR = w!("MeltAlarmOverlay");

#[cfg(feature = "simulate")]
const APP_NAME: &str = "MeltAlarm (simulated)";
#[cfg(not(feature = "simulate"))]
const APP_NAME: &str = "MeltAlarm";

const CMD_TRACK: u32 = 100;
const CMD_ALARM: u32 = 200;
const CMD_STARTUP: u32 = 201;
const CMD_TEST: u32 = 202;
const CMD_LOG: u32 = 203;
const CMD_EXIT: u32 = 204;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
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
    audio: audio::Audio,
    view: Arc<ViewModel>,
    light: bool,
    blink_on: bool,
    blinking: bool,
    hotkey: bool,
    autostart_applied: Option<bool>,
    taskbar_created: u32,
    popup_on_start: bool,
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
        if self.popup_on_start && !view.connectors.is_empty() {
            self.popup_on_start = false;
            self.popup.toggle(0, None, &self.gfx, &view, self.light);
        }
        self.popup.update(&self.gfx, &view, self.light);
        self.overlay.sync(&self.gfx, view.alarm.as_ref(), OVERLAY_CLASS);
        self.audio.sync(view.audio.as_ref());

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

        if cfg!(not(feature = "simulate")) && !view.connectors.is_empty() {
            let want = view.settings.run_at_startup;
            if self.autostart_applied != Some(want) {
                let actual = autostart::is_enabled();
                let result = if actual == want { Ok(()) } else { autostart::set(want) };
                let applied = if result.is_ok() { want } else { actual };
                self.autostart_applied = Some(applied);
                if let Err(e) = result {
                    self.runtime.log_event(LogEvent::ConfigWarning { text: format!("could not change the startup task: {e}") });
                    self.runtime.user(UserAction::SetRunAtStartup(applied), Instant::now());
                }
            }
        }
        self.view = view;
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
                        let rect = self.tray.icon_rect(id);
                        let view = self.view.clone();
                        self.popup.toggle(id.saturating_sub(1) as usize, rect, &self.gfx, &view, self.light);
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
            m if m == self.taskbar_created => {
                self.tray.forget_all();
                self.reconcile();
                (handled, None)
            }
            _ => (None, None),
        }
    }

    fn build_menu(&self) -> Option<HMENU> {
        let v = &self.view;
        let tracked = v.tracked().count();
        // SAFETY: menu handles are created here and destroyed by the caller.
        unsafe {
            let m = CreatePopupMenu().ok()?;
            let add = |flags: MENU_ITEM_FLAGS, id: u32, text: &str| {
                let _ = AppendMenuW(m, flags, id as usize, &HSTRING::from(text));
            };
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            for (i, c) in v.connectors.iter().enumerate() {
                let grey = if c.tracked && tracked <= 1 { MF_GRAYED } else { MF_ENABLED };
                let text = format!("Track {}  ({})", c.label, if c.present { "in use" } else { "no load" });
                add(MF_STRING | check(c.tracked) | grey, CMD_TRACK + i as u32, &text);
            }
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            add(MF_STRING | check(v.settings.alarm_enabled), CMD_ALARM, "Alarm (overlay, sound, voice)");
            let startup_grey = if cfg!(feature = "simulate") { MF_GRAYED } else { MF_ENABLED };
            add(MF_STRING | check(v.settings.run_at_startup) | startup_grey, CMD_STARTUP, "Run at Windows startup");
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            add(MF_STRING, CMD_TEST, "Test alarm");
            add(MF_STRING, CMD_LOG, "Open alarm log");
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            add(MF_STRING, CMD_EXIT, "Exit");
            Some(m)
        }
    }

    fn on_command(&mut self, cmd: u32) -> Option<Deferred> {
        let v = self.view.clone();
        match cmd {
            CMD_EXIT => Some(Deferred::ConfirmExit),
            CMD_ALARM => self.user(UserAction::SetAlarmEnabled(!v.settings.alarm_enabled)),
            CMD_STARTUP => self.user(UserAction::SetRunAtStartup(!v.settings.run_at_startup)),
            CMD_TEST => self.user(UserAction::TestAlarm),
            CMD_LOG => {
                let path = self.runtime.log_path().to_path_buf();
                if path.exists() {
                    sys::open_path(&path);
                } else if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                    sys::open_path(dir);
                }
                None
            }
            c if (CMD_TRACK..CMD_TRACK + 16).contains(&c) => {
                let c = v.connectors.get((c - CMD_TRACK) as usize)?;
                self.user(UserAction::SetTracked(c.key.clone(), !c.tracked))
            }
            _ => None,
        }
    }

    fn shutdown(&mut self) {
        self.audio.stop();
        self.overlay.close();
        self.popup.hide();
        self.tray.remove_all();
    }
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
                && let Some(Some(next)) = with_app(|a| a.on_command(cmd)) {
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
    // SAFETY: default processing for everything we did not handle.
    result.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

extern "system" fn popup_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let close = (msg == WM_ACTIVATE && (wp.0 & 0xFFFF) as u32 == WA_INACTIVE) || (msg == WM_KEYDOWN && wp.0 as u16 == VK_ESCAPE.0);
    if close {
        with_app(|a| a.popup.hide());
    }
    // SAFETY: default processing.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
    match msg {
        // Never take focus from the game.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            // SAFETY: valid struct for our window.
            unsafe { let _ = TrackMouseEvent(&mut tme); }
            with_app(|a| {
                let hover = a.overlay.hit_button(hwnd, x, y).then_some(hwnd);
                if hover != a.overlay.hover {
                    a.overlay.hover = hover;
                    let view = a.view.clone();
                    a.overlay.sync(&a.gfx, view.alarm.as_ref(), OVERLAY_CLASS);
                }
            });
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                if a.overlay.hover.take().is_some() {
                    let view = a.view.clone();
                    a.overlay.sync(&a.gfx, view.alarm.as_ref(), OVERLAY_CLASS);
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
                None => unsafe { let _ = PostMessageW(Some(hwnd), msg, wp, lp); },
            }
            return LRESULT(0);
        }
        _ => {}
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

    // SAFETY: plain Win32 initialization calls.
    unsafe {
        let _instance = CreateMutexW(None, true, instance_name);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return; // already running
        }
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    sys::allow_dark_menus();

    let paths = Paths {
        config_dir: sys::app_dir("APPDATA").with_file_name(dir_name),
        log_dir: sys::app_dir("LOCALAPPDATA").with_file_name(dir_name),
    };
    let log_path = paths.log_dir.join("alarms.log");
    std::panic::set_hook(Box::new(move |info| {
        let line = LogEvent::MonitoringStopped { reason: format!("internal error: {info}") }.format(&sys::wall_clock());
        LogSink::new(log_path.clone()).write(&line);
        sys::message(APP_NAME, &format!("MeltAlarm stopped because of an internal error.\n\n{info}"), true);
    }));

    register_class(w!("MeltAlarmMain"), Some(main_proc));
    register_class(w!("MeltAlarmPopup"), Some(popup_proc));
    register_class(OVERLAY_CLASS, Some(overlay_proc));

    // SAFETY: creating our hidden main window and the (hidden) popup window.
    let (hwnd, popup_hwnd, taskbar_created) = unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("MeltAlarmMain"), w!("MeltAlarm"), WS_POPUP, 0, 0, 0, 0, None, None, None, None)
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
    let drivers: Vec<Box<dyn meltalarm_source_api::Driver>> = vec![Box::new(sim::SimDriver)];
    #[cfg(not(feature = "simulate"))]
    let drivers: Vec<Box<dyn meltalarm_source_api::Driver>> = vec![Box::new(meltalarm_source_msi::MsiDriver)];

    let raw = hwnd.0 as isize;
    let runtime = Runtime::start(Host {
        drivers,
        paths,
        wall_clock: sys::wall_clock,
        waker: Box::new(move || {
            // SAFETY: posting to our window from the acquisition thread is allowed.
            unsafe { let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0)); }
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
            audio: audio::Audio::default(),
            view,
            light: sys::system_light(),
            blink_on: true,
            blinking: false,
            hotkey: false,
            autostart_applied: None,
            taskbar_created,
            popup_on_start,
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
