//! The tray icon's right-click menu (Spec §7.1). Until the Settings window exists (v1), the
//! settings live here as checkmark items (Spec §7.3).

use meltalarm_core::UserAction;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, HMENU, MENU_ITEM_FLAGS, MF_CHECKED, MF_ENABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
};
use windows::core::{HSTRING, PCWSTR};

use crate::{APP_NAME, App, Deferred, lifecycle, sys};

const CMD_TRACK: u32 = 100;
const CMD_ALARM: u32 = 200;
const CMD_STARTUP: u32 = 201;
const CMD_TEST: u32 = 202;
const CMD_LOG: u32 = 203;
const CMD_EXIT: u32 = 204;
const CMD_INSTALL: u32 = 205;

impl App {
    pub(crate) fn build_menu(&self) -> Option<HMENU> {
        let v = &self.view;
        let tracked = v.tracked().count();
        // SAFETY: menu handles are created here and destroyed by the caller.
        unsafe {
            let m = CreatePopupMenu().ok()?;
            let add = |flags: MENU_ITEM_FLAGS, id: u32, text: &str| {
                let _ = AppendMenuW(m, flags, id as usize, &HSTRING::from(text));
            };
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            let edition = if cfg!(feature = "simulate") {
                " (simulated)"
            } else if self.portable {
                " (not installed)"
            } else {
                ""
            };
            add(MF_STRING | MF_GRAYED, 0, &format!("MeltAlarm {}{edition}", env!("CARGO_PKG_VERSION")));
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            for (i, c) in v.connectors.iter().enumerate() {
                let grey = if c.tracked && tracked <= 1 { MF_GRAYED } else { MF_ENABLED };
                let text = format!("Track {}  ({})", c.full_label, if c.present { "in use" } else { "not connected" });
                add(MF_STRING | check(c.tracked) | grey, CMD_TRACK + i as u32, &text);
            }
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            add(MF_STRING | check(v.settings.alarm_enabled), CMD_ALARM, "Alerts (on screen, sound, voice)");
            if !self.portable {
                add(MF_STRING | check(v.settings.run_at_startup), CMD_STARTUP, "Run at Windows startup");
            } else if cfg!(not(feature = "simulate")) {
                add(MF_STRING, CMD_INSTALL, "Install…");
            }
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            add(MF_STRING, CMD_TEST, "Test alarm");
            add(MF_STRING, CMD_LOG, "Open alarm log");
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            add(MF_STRING, CMD_EXIT, "Exit");
            Some(m)
        }
    }

    pub(crate) fn on_command(&mut self, cmd: u32) -> Option<Deferred> {
        let v = self.view.clone();
        match cmd {
            CMD_EXIT => Some(Deferred::ConfirmExit),
            CMD_ALARM => self.user(UserAction::SetAlarmEnabled(!v.settings.alarm_enabled)),
            CMD_STARTUP => self.user(UserAction::SetRunAtStartup(!v.settings.run_at_startup)),
            CMD_TEST => self.user(UserAction::TestAlarm),
            CMD_INSTALL => {
                if let Err(e) = lifecycle::request_install() {
                    sys::message(APP_NAME, &format!("Could not start the installer: {e}"), true);
                }
                None
            }
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
}
