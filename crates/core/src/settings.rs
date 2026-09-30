//! User settings and their file format (a tiny `key = value` subset of TOML).
//! Parsing/formatting is pure; the runtime does the file IO.

use std::collections::BTreeSet;

use meltalarm_model::ConnectorKey;

use crate::limits::LimitOverrides;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Connectors that get a tray icon. Empty until first-run initialization.
    pub tracked: BTreeSet<ConnectorKey>,
    /// Master switch for every interruption: alarm, caution strip, chime, notifications.
    /// Colors, cable notes and the log stay (Spec §7.3). File key: `alarm`.
    pub alarm_enabled: bool,
    pub run_at_startup: bool,
    /// Cable-limit overrides for advanced users (Spec §6.1). Only set values are written.
    pub limits: LimitOverrides,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { tracked: BTreeSet::new(), alarm_enabled: true, run_at_startup: true, limits: LimitOverrides::default() }
    }
}

const LIMIT_KEYS: [&str; 6] = ["limit_rating", "limit_alarm", "limit_alarm_seconds", "limit_fast", "limit_instant", "limit_uneven"];

fn limit_slot<'a>(l: &'a mut LimitOverrides, key: &str) -> Option<&'a mut Option<f32>> {
    Some(match key {
        "limit_rating" => &mut l.rating,
        "limit_alarm" => &mut l.alarm,
        "limit_alarm_seconds" => &mut l.alarm_seconds,
        "limit_fast" => &mut l.fast,
        "limit_instant" => &mut l.instant,
        "limit_uneven" => &mut l.uneven,
        _ => return None,
    })
}

impl Settings {
    pub fn to_file(&self) -> String {
        let tracked: Vec<String> = self.tracked.iter().map(|k| format!("\"{k}\"")).collect();
        let mut out = format!(
            "# MeltAlarm settings\nalarm = {}\nrun_at_startup = {}\ntracked = [{}]\n",
            self.alarm_enabled,
            self.run_at_startup,
            tracked.join(", ")
        );
        let mut limits = self.limits;
        for key in LIMIT_KEYS {
            if let Some(Some(v)) = limit_slot(&mut limits, key).map(|s| *s) {
                out.push_str(&format!("{key} = {v}\n"));
            }
        }
        out
    }

    /// Lenient: unknown keys and malformed lines are ignored, missing keys keep defaults.
    pub fn from_file(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let flag = match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            match (key, flag) {
                ("alarm", Some(b)) => s.alarm_enabled = b,
                ("run_at_startup", Some(b)) => s.run_at_startup = b,
                ("tracked", _) => {
                    s.tracked = value
                        .trim_start_matches('[')
                        .trim_end_matches(']')
                        .split(',')
                        .filter_map(|item| ConnectorKey::parse(item.trim().trim_matches('"')))
                        .collect();
                }
                (k, None) => {
                    if let (Some(slot), Ok(v)) = (limit_slot(&mut s.limits, k), value.parse::<f32>())
                        && v.is_finite()
                    {
                        *slot = Some(v);
                    }
                }
                _ => {}
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
        let mut s = Settings { alarm_enabled: false, ..Settings::default() };
        s.tracked.insert(ConnectorKey::parse("msi:ai1300ts:1").unwrap());
        s.tracked.insert(ConnectorKey::parse("msi:ai1300ts:2").unwrap());
        assert_eq!(Settings::from_file(&s.to_file()), s);
        let odd = Settings::from_file("alarm = maybe\nnonsense\ntracked = [\"bad\"]\nlimit_alarm = lots\n");
        assert_eq!(odd, Settings::default());
    }

    #[test]
    fn only_overridden_limits_are_written() {
        let s = Settings::default();
        assert!(!s.to_file().contains("limit_"));
        let s = Settings { limits: LimitOverrides { alarm: Some(11.0), alarm_seconds: Some(3.0), ..Default::default() }, ..Settings::default() };
        let text = s.to_file();
        assert!(text.contains("limit_alarm = 11\n") && text.contains("limit_alarm_seconds = 3\n"), "{text}");
        assert_eq!(Settings::from_file(&text), s);
    }
}
