//! User settings and their file format (a tiny `key = value` subset of TOML).
//! Parsing/formatting is pure; the runtime does the file IO.

use std::collections::BTreeSet;

use meltalarm_model::ConnectorKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Connectors that get a tray icon. Empty until first-run initialization.
    pub tracked: BTreeSet<ConnectorKey>,
    /// Master switch for overlay + sound + voice. The log is always written.
    pub alarm_enabled: bool,
    pub run_at_startup: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { tracked: BTreeSet::new(), alarm_enabled: true, run_at_startup: true }
    }
}

impl Settings {
    pub fn to_file(&self) -> String {
        let tracked: Vec<String> = self.tracked.iter().map(|k| format!("\"{k}\"")).collect();
        format!(
            "# MeltAlarm settings\nalarm = {}\nrun_at_startup = {}\ntracked = [{}]\n",
            self.alarm_enabled,
            self.run_at_startup,
            tracked.join(", ")
        )
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
        let odd = Settings::from_file("alarm = maybe\nnonsense\ntracked = [\"bad\"]\n");
        assert_eq!(odd, Settings::default());
    }
}
