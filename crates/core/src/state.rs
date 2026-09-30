//! What must survive a restart besides the settings (docs/FUNCTIONAL_SPEC.md §8.10): the cable
//! notes, and whether an alarm was open (the PSU's power cut also ends MeltAlarm).
//! Same lenient `key = value` format as the settings; the runtime does the file IO.

use std::collections::BTreeMap;

use meltalarm_model::ConnectorKey;

/// How serious the event behind a cable note was. A note is replaced only by one at least
/// as severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Advisory = 1,
    Caution = 2,
    Alarm = 3,
}

impl Severity {
    fn from_u8(n: u8) -> Option<Severity> {
        match n {
            1 => Some(Severity::Advisory),
            2 => Some(Severity::Caution),
            3 => Some(Severity::Alarm),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CableNote {
    pub severity: Severity,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreState {
    pub notes: BTreeMap<ConnectorKey, CableNote>,
    /// An alarm (either judge) was active on this connector when the state was last saved.
    pub alarm_open: Option<ConnectorKey>,
}

impl CoreState {
    pub fn to_file(&self) -> String {
        let mut out = String::from("# MeltAlarm state: cable notes (Dismiss them in MeltAlarm). Deleting this file forgets them.\n");
        if let Some(k) = &self.alarm_open {
            out.push_str(&format!("alarm_open = {k}\n"));
        }
        for (k, n) in &self.notes {
            let text = n.text.replace(['\r', '\n'], " ");
            out.push_str(&format!("note {k} = {} | {text}\n", n.severity as u8));
        }
        out
    }

    /// Lenient: malformed lines are ignored.
    pub fn from_file(text: &str) -> CoreState {
        let mut s = CoreState::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            if key == "alarm_open" {
                s.alarm_open = ConnectorKey::parse(value);
            } else if let Some(k) = key.strip_prefix("note ").and_then(|k| ConnectorKey::parse(k.trim()))
                && let Some((sev, text)) = value.split_once('|')
                && let Some(severity) = sev.trim().parse::<u8>().ok().and_then(Severity::from_u8)
                && !text.trim().is_empty()
            {
                s.notes.insert(k, CableNote { severity, text: text.trim().to_owned() });
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_garbage() {
        let k = ConnectorKey::parse("msi:ai1300ts:1").unwrap();
        let mut s = CoreState { alarm_open: Some(k.clone()), ..Default::default() };
        s.notes.insert(k, CableNote { severity: Severity::Alarm, text: "Alarm on Sep 30, 18:02: wire 3 reached 12.4 A | ok".into() });
        assert_eq!(CoreState::from_file(&s.to_file()), s);
        let odd = CoreState::from_file("alarm_open = nope\nnote bad = 3 | x\nnote msi:x:1 = 9 | y\nnote msi:x:2 = 2 |\n");
        assert_eq!(odd, CoreState::default());
    }
}
