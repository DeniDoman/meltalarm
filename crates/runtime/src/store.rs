//! Settings, state and alarm log files. Failures never stop monitoring.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use meltalarm_core::{CoreState, Settings};

const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// Write `text` to `path` atomically: a temp file next to it, then a rename over the old one.
/// Creates the folder. Failures are ignored: a file that can't be written never stops monitoring.
pub fn write_atomic(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        SettingsStore { path }
    }

    pub fn load(&self) -> Settings {
        fs::read_to_string(&self.path).map(|t| Settings::from_file(&t)).unwrap_or_default()
    }

    pub fn save(&self, s: &Settings) {
        write_atomic(&self.path, &s.to_file());
    }
}

/// Cable notes and the open-alarm flag (Spec §8.10), next to the settings.
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn new(path: PathBuf) -> Self {
        StateStore { path }
    }

    pub fn load(&self) -> CoreState {
        fs::read_to_string(&self.path).map(|t| CoreState::from_file(&t)).unwrap_or_default()
    }

    pub fn save(&self, s: &CoreState) {
        write_atomic(&self.path, &s.to_file());
    }
}

pub struct LogSink {
    path: PathBuf,
}

impl LogSink {
    pub fn new(path: PathBuf) -> Self {
        LogSink { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, line: &str) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if fs::metadata(&self.path).is_ok_and(|m| m.len() >= LOG_ROTATE_BYTES) {
            let old = self.path.with_extension("old.log");
            let _ = fs::remove_file(&old);
            let _ = fs::rename(&self.path, &old);
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_log_round_trip_in_temp_dir() {
        let dir = std::env::temp_dir().join(format!("meltalarm-test-{}", std::process::id()));
        let store = SettingsStore::new(dir.join("cfg").join("settings.toml"));
        let s = Settings { alarm_enabled: false, ..Settings::default() };
        store.save(&s);
        assert_eq!(store.load(), s);

        let state = StateStore::new(dir.join("cfg").join("state.toml"));
        let st = CoreState { alarm_open: meltalarm_model::ConnectorKey::parse("sim:psu:1"), ..Default::default() };
        state.save(&st);
        assert_eq!(state.load(), st);

        let log = LogSink::new(dir.join("log").join("alarms.log"));
        log.write("one");
        log.write("two");
        assert_eq!(fs::read_to_string(log.path()).unwrap(), "one\ntwo\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
