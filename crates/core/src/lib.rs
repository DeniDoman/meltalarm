//! MeltAlarm's brain: pure, clock-injected, no IO (docs/ARCHITECTURE.md §5).
//!
//! `Core::handle(event, now)` updates state and returns log events, settings and state
//! changes, and the next time it wants to be woken. `Core::view(now)` describes what should
//! be on screen. Two judges raise the alarm (Spec §8.1): the cable guard (`guard`, MeltAlarm's
//! own limits) and the device's verdict.
//!
//! This file takes the events in (source, health, user, time). The rules live in modules that
//! follow the spec: `levels` (§6.2), `guard` (§6.3–6.4), `alarm` (§8.1–8.6), `ladder` (§8.8–8.10),
//! `view` (§7), `log` (§9), with the shared wording helpers in `text`.
#![forbid(unsafe_code)]

mod alarm;
pub mod guard;
mod ladder;
pub mod levels;
pub mod limits;
pub mod log;
pub mod settings;
pub mod state;
mod text;
pub mod view;

use std::time::{Duration, Instant};

use meltalarm_model::{ConnectorKey, DeviceStatus, Fault, Protection, Report, SourceInfo, Verdict, WIRES};

pub use guard::{Guard, GuardEvent, Rule};
pub use levels::{Eval, Level};
pub use limits::{LimitOverrides, Limits};
pub use log::LogEvent;
pub use settings::Settings;
pub use state::{CableNote, CoreState, Severity};
pub use view::*;

use alarm::Phase;
use ladder::{LiveNote, Strip};
use text::{cable_label, fault_name, on, protection_summary, status_name, wires_text};

const NO_DATA_AFTER: u32 = 3;
pub(crate) const STALE_AFTER: Duration = Duration::from_millis(2500);
const WATCHDOG: Duration = Duration::from_secs(5);
pub(crate) const PRESENCE_WINDOW: Duration = Duration::from_secs(10);
const RAW_EVERY: Duration = Duration::from_secs(10);
pub const SNOOZE: Duration = Duration::from_secs(30);
pub(crate) const CLEARED_SHOW: Duration = Duration::from_secs(5);
/// Test alarm: the caution strip first (Spec §8.5), then the alarm.
const TEST_STRIP: Duration = Duration::from_secs(3);
const TEST_LENGTH: Duration = Duration::from_secs(30);
/// How long the caution strip stays (Spec §8.8).
pub const STRIP_SHOW: Duration = Duration::from_secs(10);
/// A present-but-silent PSU is reported (log + notice) after this long (Spec §5.1).
pub const NOT_CONNECTED_AFTER: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq)]
pub enum UserAction {
    Snooze,
    TestAlarm,
    SetTracked(ConnectorKey, bool),
    SetAlarmEnabled(bool),
    SetRunAtStartup(bool),
    /// Remove a connector's cable note (Spec §8.10).
    DismissNote(ConnectorKey),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    SourceConnected(SourceInfo),
    /// Before the first connection: a supported device is present but not answering (reason).
    SourcePending(String),
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
    /// Cable notes or the open-alarm flag changed: persist (Spec §8.10).
    pub state_changed: Option<CoreState>,
    pub next_wake: Option<Instant>,
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
    pub guard: Guard,
    pub(crate) live_note: Option<LiveNote>,
}

impl Conn {
    pub(crate) fn device_alarm(&self) -> Option<DeviceStatus> {
        self.verdict.as_ref().map(|v| v.status).filter(|s| s.is_alarm())
    }
    pub(crate) fn in_alarm(&self) -> bool {
        self.device_alarm().is_some() || self.guard.overload().is_some()
    }
}

pub struct Core {
    pub(crate) settings: Settings,
    pub(crate) limits: limits::Resolved,
    pub(crate) source: Option<SourceInfo>,
    pub(crate) protection: Option<Protection>,
    pub(crate) faults: Vec<Fault>,
    pub(crate) conns: Vec<Conn>,
    pub(crate) unhealthy: u32,
    pub(crate) last_healthy: Option<Instant>,
    pub(crate) no_data_since: Option<Instant>,
    pub(crate) suspended: bool,
    pub(crate) phase: Phase,
    pub(crate) episode_start: Option<Instant>,
    /// Reason and connector of the latest alarm, for the green "cleared" notch.
    pub(crate) last_alarm: Option<(String, String)>,
    tracking_pending: bool,
    /// Before the first connection: since when a device has been present but silent, and why.
    pub(crate) pending: Option<(Instant, String)>,
    not_connected_logged: bool,
    pub(crate) notice: Option<Notice>,
    pub(crate) notice_seq: u32,
    pub(crate) state: CoreState,
    pub(crate) state_dirty: bool,
    pub(crate) strip: Option<Strip>,
    pub(crate) chime_seq: u32,
    pub(crate) wall: fn() -> String,
    limits_logged: bool,
}

fn no_wall() -> String {
    String::new()
}

impl Core {
    pub fn new(settings: Settings) -> Self {
        let limits = settings.limits.resolve();
        Core {
            settings,
            limits,
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
            pending: None,
            not_connected_logged: false,
            notice: None,
            notice_seq: 0,
            state: CoreState::default(),
            state_dirty: false,
            strip: None,
            chime_seq: 0,
            wall: no_wall,
            limits_logged: false,
        }
    }

    /// Local wall-clock time, `2026-09-30 18:02:11`, used only to date cable notes.
    pub fn set_wall_clock(&mut self, wall: fn() -> String) {
        self.wall = wall;
    }

    /// Cable notes and the open-alarm flag from the last session (Spec §8.10).
    pub fn restore(&mut self, state: CoreState) {
        self.state = state;
        if let Some(key) = self.state.alarm_open.take() {
            // The last session ended during an alarm; the usual reason is the PSU's power cut.
            self.state_dirty = true;
            if self.settings.alarm_enabled {
                self.push_notice(
                    "Last session ended during a cable alarm",
                    "Inspect the GPU power cable with the PC off before gaming.".into(),
                    true,
                    Some(key),
                );
            }
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The device's name for its protection (MSI: `Safeguard+`); MeltAlarm never names one itself.
    pub(crate) fn protection_name(&self) -> &str {
        self.source.as_ref().and_then(|s| s.protection_name.as_deref()).unwrap_or("Protection")
    }

    pub fn handle(&mut self, ev: Event, now: Instant) -> Output {
        let mut out = Output::default();
        match ev {
            Event::SourceConnected(info) => self.on_connected(info, now, &mut out),
            Event::SourcePending(reason) => {
                if self.source.is_none() {
                    match &mut self.pending {
                        Some((_, r)) => *r = reason,
                        None => self.pending = Some((now, reason)),
                    }
                }
            }
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
        self.update_alarm_open();
        if std::mem::take(&mut self.state_dirty) {
            out.state_changed = Some(self.state.clone());
        }
        out.next_wake = self.next_wake(now);
        out
    }

    fn on_connected(&mut self, info: SourceInfo, now: Instant, out: &mut Output) {
        if !self.limits_logged {
            self.limits_logged = true;
            out.log.push(LogEvent::Limits { text: self.limits.summary() });
            if let Some(w) = &self.limits.warning {
                out.log.push(LogEvent::ConfigWarning { text: w.clone() });
            }
        }
        if let Some((since, _)) = self.pending.take()
            && self.not_connected_logged
        {
            out.log.push(LogEvent::Connected { model: format!("{} {}", info.vendor, info.model), after: now.duration_since(since) });
        }
        self.not_connected_logged = false;
        if self.notice.as_ref().is_some_and(|n| n.connector.is_none()) {
            self.notice = None;
        }
        let same = self.source.as_ref().is_some_and(|s| s.id == info.id);
        if !same {
            self.conns = info
                .connectors
                .iter()
                .map(|c| Conn {
                    key: ConnectorKey { source: info.id.clone(), index: c.index },
                    label: cable_label(c.index),
                    wires: None,
                    eval: Eval::default(),
                    last_nonzero: None,
                    verdict: None,
                    alarm_since: None,
                    last_raw: None,
                    guard: Guard::default(),
                    live_note: None,
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
            for c in &mut self.conns {
                c.guard.gap();
            }
            // Losing the monitor is itself a caution, once we had data (Spec §6.8).
            if self.last_healthy.is_some() {
                self.raise_caution(None, "Monitoring lost".into(), "MeltAlarm can't read the PSU".into(), now);
            }
        }
    }

    fn on_report(&mut self, r: Report, now: Instant, out: &mut Output) {
        if self.suspended {
            return;
        }
        let healthy = r.is_healthy(&self.source.as_ref().map(|s| s.caps).unwrap_or_default());

        if let Some(p) = r.protection
            && self.protection.as_ref() != Some(&p)
        {
            out.log.push(LogEvent::Config { text: protection_summary(&p, self.protection_name()) });
            if !p.enabled {
                out.log.push(LogEvent::ConfigWarning { text: format!("{} is OFF on the PSU", self.protection_name()) });
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

        // The cable guard sees every valid sample; a connector without one this tick has a gap.
        let limits = self.limits.limits;
        for ci in 0..self.conns.len() {
            let reading = r.readings.iter().flatten().find(|x| x.index == self.conns[ci].key.index);
            let Some(reading) = reading else {
                self.conns[ci].guard.gap();
                continue;
            };
            let c = &mut self.conns[ci];
            c.wires = Some(reading.wires);
            if reading.wires.iter().flatten().any(|&a| a > 0.0) {
                c.last_nonzero = Some(now);
            }
            let events = c.guard.sample(&reading.wires, r.at, &limits);
            for e in events {
                self.on_guard(ci, e, now, out);
            }
            self.refresh_live_note(ci);
        }

        for v in r.verdicts.iter().flatten() {
            let Some(ci) = self.conns.iter().position(|c| c.key.index == v.index) else { continue };
            let c = &mut self.conns[ci];
            let old = c.verdict.as_ref().map_or(DeviceStatus::Normal, |o| o.status);
            let mut new_alarm = None;
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
                    new_alarm = Some(v.status);
                }
                c.alarm_since.get_or_insert(now);
                if let Some(d) = &r.diagnostic
                    && c.last_raw.is_none_or(|t| now.duration_since(t) >= RAW_EVERY)
                {
                    out.log.push(LogEvent::PsuRaw { diagnostic: d.clone() });
                    c.last_raw = Some(now);
                }
            } else if old.is_alarm() {
                let lasted = c.alarm_since.map_or(Duration::ZERO, |s| now.duration_since(s));
                out.log.push(LogEvent::PsuClear { conn: c.label.clone(), lasted });
                c.alarm_since = None;
            }
            c.verdict = Some(v.clone());
            if let Some(status) = new_alarm {
                let text = format!(
                    "PSU alarm{}: {}. Inspect the cable and both connectors with the PC off before the next session.",
                    on(&self.when()),
                    status_name(status)
                );
                self.set_note(ci, Severity::Alarm, text);
            }
        }

        for c in &mut self.conns {
            if let Some(w) = &c.wires {
                let imbalance = c.device_alarm() == Some(DeviceStatus::Imbalance);
                c.eval = levels::evaluate(w, &limits, imbalance);
            }
        }

        if let Some(faults) = r.faults {
            let new: Vec<Fault> = faults.iter().filter(|f| !self.faults.contains(f)).cloned().collect();
            for f in &new {
                out.log.push(LogEvent::PsuFlag { fault: fault_name(f), set: true });
            }
            for f in self.faults.iter().filter(|f| !faults.contains(f)) {
                out.log.push(LogEvent::PsuFlag { fault: fault_name(f), set: false });
            }
            self.faults = faults;
            if let Some(f) = new.first() {
                self.raise_caution(None, format!("PSU fault: {}", fault_name(f)), "The PSU may shut down".into(), now);
            }
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
                    self.chime_seq += 1;
                    self.strip = None;
                    self.phase =
                        Phase::Test { until: now + TEST_STRIP + TEST_LENGTH, strip_until: now + TEST_STRIP, chime: self.chime_seq };
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
                if !b {
                    self.strip = None;
                }
                out.settings_changed = Some(self.settings.clone());
            }
            UserAction::SetRunAtStartup(b) => {
                if self.settings.run_at_startup != b {
                    self.settings.run_at_startup = b;
                    out.settings_changed = Some(self.settings.clone());
                }
            }
            UserAction::DismissNote(key) => {
                if self.state.notes.remove(&key).is_some() {
                    self.state_dirty = true;
                }
                if let Some(c) = self.conns.iter_mut().find(|c| c.key == key) {
                    c.live_note = None;
                }
            }
        }
    }

    fn on_time(&mut self, now: Instant, out: &mut Output) {
        if let Some((since, reason)) = &self.pending
            && !self.not_connected_logged
            && now.duration_since(*since) >= NOT_CONNECTED_AFTER
        {
            out.log.push(LogEvent::NotConnected { reason: reason.clone() });
            let text = format!("{reason}. MeltAlarm keeps trying and connects as soon as the PSU answers.");
            self.push_notice("MeltAlarm can't reach the PSU", text, true, None);
            self.not_connected_logged = true;
        }
        if !self.suspended
            && self.no_data_since.is_none()
            && let Some(t) = self.last_healthy
            && now.duration_since(t) >= WATCHDOG
        {
            self.mark_no_data(now, out);
        }
        if self.strip.as_ref().is_some_and(|s| now >= s.until) {
            self.strip = None;
        }
    }

    fn next_wake(&self, now: Instant) -> Option<Instant> {
        let mut t: Vec<Instant> = vec![];
        match &self.phase {
            Phase::Snoozed { until, .. } | Phase::Cleared { until, .. } => t.push(*until),
            Phase::Test { until, strip_until, .. } => t.extend([*until, *strip_until]),
            Phase::Active => t.push(now + Duration::from_secs(1)), // countdown text
            Phase::Idle => {}
        }
        if let Phase::Cleared { .. } = self.phase {
            t.push(now + Duration::from_millis(250)); // green progress bar
        }
        if let Some(s) = &self.strip {
            t.push(s.until);
        }
        if !self.suspended
            && self.no_data_since.is_none()
            && let Some(h) = self.last_healthy
        {
            t.push(h + STALE_AFTER + Duration::from_millis(10));
            t.push(h + WATCHDOG);
        }
        if let Some((since, _)) = &self.pending
            && !self.not_connected_logged
        {
            t.push(*since + NOT_CONNECTED_AFTER);
        }
        t.into_iter().filter(|&x| x > now).min()
    }
}

#[cfg(test)]
mod tests;
