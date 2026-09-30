//! Alarm log events (docs/FUNCTIONAL_SPEC.md §9). One line per event, never per tick.

use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum LogEvent {
    /// The cable limits in effect (Spec §6.1), once at start.
    Limits { text: String },
    /// MeltAlarm's overload alarm (Spec §6.3) and its end.
    Overload { conn: String, detail: String },
    OverloadEnd { conn: String, lasted: Duration, detail: String },
    /// A wire-above-rating caution qualified (Spec §6.4), and the episode's end.
    Caution { conn: String, detail: String },
    CautionEnd { conn: String, lasted: Duration, detail: String },
    /// An uneven-load advisory qualified, and the episode's end.
    Uneven { conn: String, detail: String },
    UnevenEnd { conn: String, lasted: Duration, detail: String },
    PsuAlarm { conn: String, detail: String },
    PsuRaw { diagnostic: String },
    PsuClear { conn: String, lasted: Duration },
    PsuFlag { fault: String, set: bool },
    NoData,
    DataBack { after: Duration },
    Config { text: String },
    ConfigWarning { text: String },
    MonitoringStopped { reason: String },
    NotConnected { reason: String },
    Connected { model: String, after: Duration },
    Installed { version: String, autostart: bool },
    /// `downgrade`: an older file replaced a newer one (rollback, Spec §4.6).
    Updated { from: String, to: String, downgrade: bool },
}

pub fn secs(d: Duration) -> String {
    format!("{} s", d.as_secs())
}

impl LogEvent {
    /// `wall` is the local timestamp, e.g. `2026-09-27 18:02:11`.
    pub fn format(&self, wall: &str) -> String {
        let (tag, body) = match self {
            LogEvent::Limits { text } => ("LIMITS", text.clone()),
            LogEvent::Overload { conn, detail } => ("OVERLOAD", format!("{conn} | {detail}")),
            LogEvent::OverloadEnd { conn, lasted, detail } => ("OVERLOAD END", format!("{conn} | {} | {detail}", secs(*lasted))),
            LogEvent::Caution { conn, detail } => ("CAUTION", format!("{conn} | {detail}")),
            LogEvent::CautionEnd { conn, lasted, detail } => ("CAUTION END", format!("{conn} | {} | {detail}", secs(*lasted))),
            LogEvent::Uneven { conn, detail } => ("UNEVEN LOAD", format!("{conn} | {detail}")),
            LogEvent::UnevenEnd { conn, lasted, detail } => ("UNEVEN END", format!("{conn} | {} | {detail}", secs(*lasted))),
            LogEvent::PsuAlarm { conn, detail } => ("PSU ALARM", format!("{conn} | {detail}")),
            LogEvent::PsuRaw { diagnostic } => ("PSU RAW", diagnostic.clone()),
            LogEvent::PsuClear { conn, lasted } => ("PSU CLEAR", format!("{conn} | {}", secs(*lasted))),
            LogEvent::PsuFlag { fault, set } => ("PSU FLAG", format!("{fault} {}", if *set { "set" } else { "cleared" })),
            LogEvent::NoData => ("NO DATA", "PSU stopped answering".into()),
            LogEvent::DataBack { after } => ("DATA BACK", format!("after {}", secs(*after))),
            LogEvent::Config { text } => ("CONFIG", text.clone()),
            LogEvent::ConfigWarning { text } => ("CONFIG", format!("WARNING: {text}")),
            LogEvent::MonitoringStopped { reason } => ("STOPPED", format!("Monitoring stopped: {reason}")),
            LogEvent::NotConnected { reason } => ("NOT CONNECTED", format!("{reason} — still trying")),
            LogEvent::Connected { model, after } => ("CONNECTED", format!("{model} after {}", secs(*after))),
            LogEvent::Installed { version, autostart } => (
                "INSTALL",
                format!("MeltAlarm {version} installed · {}", if *autostart { "starts with Windows" } else { "manual start" }),
            ),
            LogEvent::Updated { from, to, downgrade: false } => ("INSTALL", format!("updated {from} → {to}")),
            LogEvent::Updated { from, to, downgrade: true } => ("INSTALL", format!("replaced {from} with {to}")),
        };
        format!("{wall} | {tag:<13} | {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format_matches_spec() {
        let e = LogEvent::PsuClear { conn: "12V-2x6 #1".into(), lasted: Duration::from_secs(42) };
        assert_eq!(e.format("2026-09-27 18:06:22"), "2026-09-27 18:06:22 | PSU CLEAR     | 12V-2x6 #1 | 42 s");
    }

    #[test]
    fn install_lines_match_spec() {
        let wall = "2026-10-05 18:00:02";
        let e = LogEvent::Installed { version: "0.2.0".into(), autostart: true };
        assert_eq!(e.format(wall), "2026-10-05 18:00:02 | INSTALL       | MeltAlarm 0.2.0 installed · starts with Windows");
        let e = LogEvent::Updated { from: "0.2.0".into(), to: "0.3.0".into(), downgrade: false };
        assert_eq!(e.format(wall), "2026-10-05 18:00:02 | INSTALL       | updated 0.2.0 → 0.3.0");
        let e = LogEvent::Updated { from: "0.3.0".into(), to: "0.2.0".into(), downgrade: true };
        assert_eq!(e.format(wall), "2026-10-05 18:00:02 | INSTALL       | replaced 0.3.0 with 0.2.0");
    }
}
