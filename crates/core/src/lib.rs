//! MeltAlarm's brain: pure, clock-injected, no IO (docs/ARCHITECTURE.md §5).
//!
//! `Core::handle(event, now)` updates state and returns log events, settings changes and
//! the next time it wants to be woken. `Core::view(now)` describes what should be on screen.
#![forbid(unsafe_code)]

pub mod levels;
pub mod log;
pub mod settings;
pub mod view;

use std::time::{Duration, Instant};

use meltalarm_model::{ConnectorKey, DeviceStatus, Fault, Protection, Report, SourceInfo, Verdict, WIRES};

pub use levels::{Eval, Level};
pub use log::LogEvent;
pub use settings::Settings;
pub use view::*;

use view::{amps, protection_summary, status_name};

const NO_DATA_AFTER: u32 = 3;
pub(crate) const STALE_AFTER: Duration = Duration::from_millis(2500);
const WATCHDOG: Duration = Duration::from_secs(5);
pub(crate) const PRESENCE_WINDOW: Duration = Duration::from_secs(10);
const RED_MERGE: Duration = Duration::from_secs(10);
const SUSTAINED_EXTRA: Duration = Duration::from_secs(10);
const RAW_EVERY: Duration = Duration::from_secs(10);
pub const SNOOZE: Duration = Duration::from_secs(30);
pub(crate) const CLEARED_SHOW: Duration = Duration::from_secs(5);
const TEST_LENGTH: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub enum UserAction {
    Snooze,
    TestAlarm,
    SetTracked(ConnectorKey, bool),
    SetAlarmEnabled(bool),
    SetRunAtStartup(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    SourceConnected(SourceInfo),
    SourceLost,
    Report(Report),
    User(UserAction),
    Suspended,
    Resumed,
    /// A deadline from `Output::next_wake` passed (or any periodic nudge).
    Wake,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub log: Vec<LogEvent>,
    pub settings_changed: Option<Settings>,
    pub next_wake: Option<Instant>,
}

pub(crate) struct Red {
    pub start: Instant,
    pub last_red: Instant,
    pub peak_wire: (usize, f32),
    pub peak_spread: f32,
    pub sustained_logged: bool,
}

pub(crate) struct Conn {
    pub key: ConnectorKey,
    pub label: String,
    pub wires: Option<[Option<f32>; WIRES]>,
    pub eval: Eval,
    pub last_nonzero: Option<Instant>,
    pub verdict: Option<Verdict>,
    pub alarm_since: Option<Instant>,
    pub last_raw: Option<Instant>,
    pub red: Option<Red>,
}

pub(crate) enum Phase {
    Idle,
    Active,
    Snoozed { until: Instant, snapshot: Vec<(u8, DeviceStatus)> },
    Cleared { until: Instant, lasted: Duration, cause: DeviceStatus, label: String },
    Test { until: Instant },
}

pub struct Core {
    pub(crate) settings: Settings,
    pub(crate) source: Option<SourceInfo>,
    pub(crate) protection: Option<Protection>,
    pub(crate) faults: Vec<Fault>,
    pub(crate) conns: Vec<Conn>,
    pub(crate) unhealthy: u32,
    pub(crate) last_healthy: Option<Instant>,
    pub(crate) no_data_since: Option<Instant>,
    pub(crate) suspended: bool,
    pub(crate) phase: Phase,
    episode_start: Option<Instant>,
    /// Cause and connector of the latest alarm, for the green "cleared" notch.
    last_alarm: Option<(DeviceStatus, String)>,
    tracking_pending: bool,
}

impl Core {
    pub fn new(settings: Settings) -> Self {
        Core {
            settings,
            source: None,
            protection: None,
            faults: vec![],
            conns: vec![],
            unhealthy: 0,
            last_healthy: None,
            no_data_since: None,
            suspended: false,
            phase: Phase::Idle,
            episode_start: None,
            last_alarm: None,
            tracking_pending: false,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn handle(&mut self, ev: Event, now: Instant) -> Output {
        let mut out = Output::default();
        match ev {
            Event::SourceConnected(info) => self.on_connected(info),
            Event::SourceLost => self.mark_no_data(now, &mut out),
            Event::Report(r) => self.on_report(r, now, &mut out),
            Event::User(a) => self.on_user(a, now, &mut out),
            Event::Suspended => self.suspended = true,
            Event::Resumed => {
                self.suspended = false;
                self.unhealthy = 0;
                self.last_healthy = Some(now); // grace period: no stale/no-data right after resume
            }
            Event::Wake => {}
        }
        self.on_time(now, &mut out);
        self.update_phase(now);
        out.next_wake = self.next_wake(now);
        out
    }

    fn on_connected(&mut self, info: SourceInfo) {
        let same = self.source.as_ref().is_some_and(|s| s.id == info.id);
        if !same {
            self.conns = info
                .connectors
                .iter()
                .map(|c| Conn {
                    key: ConnectorKey { source: info.id.clone(), index: c.index },
                    label: c.label.clone(),
                    wires: None,
                    eval: Eval::default(),
                    last_nonzero: None,
                    verdict: None,
                    alarm_since: None,
                    last_raw: None,
                    red: None,
                })
                .collect();
            self.protection = None;
            self.faults.clear();
        }
        self.tracking_pending = !self.conns.iter().any(|c| self.settings.tracked.contains(&c.key));
        self.source = Some(info);
    }

    fn mark_no_data(&mut self, now: Instant, out: &mut Output) {
        if self.no_data_since.is_none() && !self.suspended {
            self.no_data_since = Some(now);
            out.log.push(LogEvent::NoData);
        }
    }

    fn on_report(&mut self, r: Report, now: Instant, out: &mut Output) {
        if self.suspended {
            return;
        }
        let needs_verdict = self.source.as_ref().is_some_and(|s| s.caps.device_verdict);
        let healthy = r.readings.is_some() && (!needs_verdict || r.verdicts.is_some());

        if let Some(p) = r.protection
            && self.protection.as_ref() != Some(&p) {
                out.log.push(LogEvent::Config { text: protection_summary(&p) });
                if !p.enabled {
                    out.log.push(LogEvent::ConfigWarning { text: "Safeguard+ is OFF on the PSU".into() });
                }
                self.protection = Some(p);
            }

        if healthy {
            if let Some(since) = self.no_data_since.take() {
                out.log.push(LogEvent::DataBack { after: now.duration_since(since) });
            }
            self.unhealthy = 0;
            self.last_healthy = Some(now);
        } else {
            self.unhealthy += 1;
            if self.unhealthy >= NO_DATA_AFTER {
                self.mark_no_data(now, out);
            }
        }

        for reading in r.readings.iter().flatten() {
            let protection = self.protection.clone();
            let Some(c) = self.conns.iter_mut().find(|c| c.key.index == reading.index) else { continue };
            c.wires = Some(reading.wires);
            c.eval = levels::evaluate(&reading.wires, protection.as_ref());
            if reading.wires.iter().flatten().any(|&a| a > 0.0) {
                c.last_nonzero = Some(now);
            }
            track_red(c, protection.as_ref(), now, out);
        }

        for v in r.verdicts.iter().flatten() {
            let Some(c) = self.conns.iter_mut().find(|c| c.key.index == v.index) else { continue };
            let old = c.verdict.as_ref().map_or(DeviceStatus::Normal, |o| o.status);
            if v.status.is_alarm() {
                if old != v.status {
                    let flagged: Vec<String> = (0..WIRES).filter(|&i| v.flagged[i]).map(|i| (i + 1).to_string()).collect();
                    out.log.push(LogEvent::PsuAlarm {
                        conn: c.label.clone(),
                        detail: format!(
                            "status {} | flagged wires: {} | wires {}",
                            status_name(v.status),
                            if flagged.is_empty() { "none".into() } else { flagged.join(" ") },
                            wires_text(&c.wires)
                        ),
                    });
                    c.last_raw = None;
                }
                c.alarm_since.get_or_insert(now);
                if let Some(d) = &r.diagnostic
                    && c.last_raw.is_none_or(|t| now.duration_since(t) >= RAW_EVERY) {
                        out.log.push(LogEvent::PsuRaw { diagnostic: d.clone() });
                        c.last_raw = Some(now);
                    }
            } else if old.is_alarm() {
                let lasted = c.alarm_since.map_or(Duration::ZERO, |s| now.duration_since(s));
                out.log.push(LogEvent::PsuClear { conn: c.label.clone(), lasted });
                c.alarm_since = None;
            }
            c.verdict = Some(v.clone());
        }

        if let Some(faults) = r.faults {
            for f in faults.iter().filter(|f| !self.faults.contains(f)) {
                out.log.push(LogEvent::PsuFlag { fault: f.to_string(), set: true });
            }
            for f in self.faults.iter().filter(|f| !faults.contains(f)) {
                out.log.push(LogEvent::PsuFlag { fault: f.to_string(), set: false });
            }
            self.faults = faults;
        }

        if healthy && self.tracking_pending {
            let present: Vec<ConnectorKey> = self.conns.iter().filter(|c| c.last_nonzero == Some(now)).map(|c| c.key.clone()).collect();
            let chosen = if present.is_empty() { self.conns.iter().take(1).map(|c| c.key.clone()).collect() } else { present };
            self.settings.tracked.extend(chosen);
            self.tracking_pending = false;
            out.settings_changed = Some(self.settings.clone());
        }
    }

    fn on_user(&mut self, a: UserAction, now: Instant, out: &mut Output) {
        match a {
            UserAction::Snooze => match self.phase {
                Phase::Active => self.phase = Phase::Snoozed { until: now + SNOOZE, snapshot: self.active_set() },
                Phase::Test { .. } | Phase::Cleared { .. } => self.phase = Phase::Idle,
                _ => {}
            },
            UserAction::TestAlarm => {
                if matches!(self.phase, Phase::Idle | Phase::Cleared { .. }) {
                    self.phase = Phase::Test { until: now + TEST_LENGTH };
                }
            }
            UserAction::SetTracked(key, on) => {
                let tracked_here = self.conns.iter().filter(|c| self.settings.tracked.contains(&c.key)).count();
                let changed = if on {
                    self.settings.tracked.insert(key)
                } else if tracked_here > 1 || !self.conns.iter().any(|c| c.key == key) {
                    self.settings.tracked.remove(&key)
                } else {
                    false // at least one connector always stays tracked
                };
                if changed {
                    out.settings_changed = Some(self.settings.clone());
                }
            }
            UserAction::SetAlarmEnabled(b) => {
                self.settings.alarm_enabled = b;
                out.settings_changed = Some(self.settings.clone());
            }
            UserAction::SetRunAtStartup(b) => {
                if self.settings.run_at_startup != b {
                    self.settings.run_at_startup = b;
                    out.settings_changed = Some(self.settings.clone());
                }
            }
        }
    }

    fn on_time(&mut self, now: Instant, out: &mut Output) {
        if !self.suspended && self.no_data_since.is_none()
            && let Some(t) = self.last_healthy
                && now.duration_since(t) >= WATCHDOG {
                    self.mark_no_data(now, out);
                }
        for c in &mut self.conns {
            if let Some(r) = &c.red
                && now.duration_since(r.last_red) >= RED_MERGE {
                    out.log.push(LogEvent::RedEnd {
                        conn: c.label.clone(),
                        lasted: r.last_red.duration_since(r.start) + Duration::from_secs(1),
                        detail: format!("peak wire {} = {} | peak spread {}", r.peak_wire.0 + 1, amps(r.peak_wire.1), amps(r.peak_spread)),
                    });
                    c.red = None;
                }
        }
    }

    fn worst_alarm(&self) -> Option<(DeviceStatus, String)> {
        self.conns
            .iter()
            .filter_map(|c| c.verdict.as_ref().filter(|v| v.status.is_alarm()).map(|v| (v.status, c.label.clone())))
            .max_by_key(|(s, _)| *s == DeviceStatus::CriticalOverCurrent)
    }

    fn active_set(&self) -> Vec<(u8, DeviceStatus)> {
        self.conns
            .iter()
            .filter_map(|c| c.verdict.as_ref().filter(|v| v.status.is_alarm()).map(|v| (c.key.index, v.status)))
            .collect()
    }

    fn update_phase(&mut self, now: Instant) {
        let active = self.active_set();
        if !self.settings.alarm_enabled {
            if !matches!(self.phase, Phase::Idle) {
                self.phase = Phase::Idle;
            }
            self.episode_start = if active.is_empty() { None } else { self.episode_start.or(Some(now)) };
            return;
        }
        if !active.is_empty() && self.episode_start.is_none() {
            self.episode_start = self.conns.iter().filter_map(|c| c.alarm_since).min().or(Some(now));
        }
        if let Some(worst) = self.worst_alarm() {
            self.last_alarm = Some(worst);
        }
        let cleared = |core: &Core| {
            let (cause, label) = core.last_alarm.clone().unwrap_or((DeviceStatus::OverCurrent, String::new()));
            Phase::Cleared {
                until: now + CLEARED_SHOW,
                lasted: core.episode_start.map_or(Duration::ZERO, |s| now.duration_since(s)),
                cause,
                label,
            }
        };
        self.phase = match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Idle | Phase::Cleared { .. } | Phase::Test { .. } if !active.is_empty() => Phase::Active,
            Phase::Active | Phase::Snoozed { .. } if active.is_empty() => {
                let p = cleared(self);
                self.episode_start = None;
                p
            }
            Phase::Snoozed { until, snapshot } => {
                let escalated = active.iter().any(|a| !snapshot.contains(a));
                if escalated || now >= until { Phase::Active } else { Phase::Snoozed { until, snapshot } }
            }
            Phase::Cleared { until, .. } | Phase::Test { until } if now >= until => Phase::Idle,
            other => other,
        };
        if active.is_empty() && !matches!(self.phase, Phase::Cleared { .. }) {
            self.episode_start = None;
        }
    }

    fn next_wake(&self, now: Instant) -> Option<Instant> {
        let mut t: Vec<Instant> = vec![];
        match &self.phase {
            Phase::Snoozed { until, .. } | Phase::Cleared { until, .. } | Phase::Test { until } => t.push(*until),
            Phase::Active => t.push(now + Duration::from_secs(1)), // countdown text
            Phase::Idle => {}
        }
        if let Phase::Cleared { .. } = self.phase {
            t.push(now + Duration::from_millis(250)); // green progress bar
        }
        if !self.suspended && self.no_data_since.is_none()
            && let Some(h) = self.last_healthy {
                t.push(h + STALE_AFTER + Duration::from_millis(10));
                t.push(h + WATCHDOG);
            }
        for c in &self.conns {
            if let Some(r) = &c.red {
                t.push(r.last_red + RED_MERGE);
            }
        }
        t.into_iter().filter(|&x| x > now).min()
    }
}

fn wires_text(w: &Option<[Option<f32>; WIRES]>) -> String {
    match w {
        Some(w) => w.iter().map(|a| a.map_or("-".into(), |a| format!("{a:.1}"))).collect::<Vec<_>>().join(" "),
        None => "unavailable".into(),
    }
}

fn track_red(c: &mut Conn, p: Option<&Protection>, now: Instant, out: &mut Output) {
    if !c.eval.is_red() {
        return;
    }
    let (wi, wa) = c.eval.max.unwrap_or((0, 0.0));
    let spread = c.eval.spread.unwrap_or(0.0);
    match &mut c.red {
        Some(r) => {
            r.last_red = now;
            if wa > r.peak_wire.1 {
                r.peak_wire = (wi, wa);
            }
            r.peak_spread = r.peak_spread.max(spread);
        }
        None => {
            let detail = if c.eval.wire_red {
                format!(
                    "wire {} = {} >= {} | wires {} | spread {}",
                    wi + 1,
                    amps(wa),
                    amps(p.and_then(|p| p.wire_limit).unwrap_or(0.0)),
                    wires_text(&c.wires),
                    amps(spread)
                )
            } else {
                format!(
                    "spread {} >= {} | wires {}",
                    amps(spread),
                    amps(p.and_then(|p| p.spread_limit).unwrap_or(0.0)),
                    wires_text(&c.wires)
                )
            };
            out.log.push(LogEvent::RedStart { conn: c.label.clone(), detail });
            c.red = Some(Red { start: now, last_red: now, peak_wire: (wi, wa), peak_spread: spread, sustained_logged: false });
        }
    }
    // Over the device limit for longer than its trigger (+10 s) while it still says Normal.
    let device_normal = c.verdict.as_ref().is_none_or(|v| !v.status.is_alarm());
    let trigger = p.and_then(|p| match (p.wire_trigger, p.spread_trigger) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    });
    if let (Some(r), Some(trigger)) = (&mut c.red, trigger) {
        let lasted = now.duration_since(r.start);
        if device_normal && !r.sustained_logged && lasted >= trigger + SUSTAINED_EXTRA {
            r.sustained_logged = true;
            out.log.push(LogEvent::RedSustained { conn: c.label.clone(), lasted });
        }
    }
}

#[cfg(test)]
mod tests;
