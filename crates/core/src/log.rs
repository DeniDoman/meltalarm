//! Alarm log events (docs/FUNCTIONAL_SPEC.md §9). One line per event, never per tick.

use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum LogEvent {
    RedStart { conn: String, detail: String },
    RedEnd { conn: String, lasted: Duration, detail: String },
    RedSustained { conn: String, lasted: Duration },
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
}

pub fn secs(d: Duration) -> String {
    format!("{} s", d.as_secs())
}

impl LogEvent {
    /// `wall` is the local timestamp, e.g. `2026-09-27 18:02:11`.
    pub fn format(&self, wall: &str) -> String {
        let (tag, body) = match self {
            LogEvent::RedStart { conn, detail } => ("RED START", format!("{conn} | {detail}")),
            LogEvent::RedEnd { conn, lasted, detail } => ("RED END", format!("{conn} | {} | {detail}", secs(*lasted))),
            LogEvent::RedSustained { conn, lasted } => {
                ("RED SUSTAINED", format!("{conn} | {} over PSU limit, PSU still Normal", secs(*lasted)))
            }
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
}
