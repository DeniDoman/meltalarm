//! MeltAlarm's brain: pure, clock-injected, no IO (docs/ARCHITECTURE.md §5).
//!
//! `Core::handle(event, now)` updates state and returns log events, settings and state
//! changes, and the next time it wants to be woken. `Core::view(now)` describes what should
//! be on screen. Two judges raise the alarm (Spec §8.1): the cable guard (`guard`, MeltAlarm's
//! own limits) and the device's verdict.
#![forbid(unsafe_code)]

pub mod guard;
pub mod levels;
pub mod limits;
pub mod log;
pub mod settings;
pub mod state;
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

use view::{amps, protection_summary, status_name};

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

/// Why a connector is in alarm. Each (connector, cause) pair is tracked separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    /// MeltAlarm's overload rule (Spec §6.3).
    Overload,
    /// The device's own status (Spec §6.5).
    Device(DeviceStatus),
}

/// The episode whose peak keeps the connector's cable note up to date while it lasts.
#[derive(Clone, Debug)]
enum LiveNote {
    Overload { when: String },
    Caution { when: String },
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
    live_note: Option<LiveNote>,
}

impl Conn {
    pub(crate) fn device_alarm(&self) -> Option<DeviceStatus> {
        self.verdict.as_ref().map(|v| v.status).filter(|s| s.is_alarm())
    }
    pub(crate) fn in_alarm(&self) -> bool {
        self.device_alarm().is_some() || self.guard.overload().is_some()
    }
}

/// The name in the log and wherever a number is needed: "GPU power cable 1" (Spec §7.0).
fn cable_label(index: u8) -> String {
    format!("GPU power cable {}", index + 1)
}

pub(crate) enum Phase {
    Idle,
    Active,
    Snoozed { until: Instant, snapshot: Vec<(u8, Cause)> },
    Cleared { until: Instant, lasted: Duration, reason: String, label: String },
    /// The strip part runs until `strip_until`, then the alarm part (Spec §8.5).
    Test { until: Instant, strip_until: Instant, chime: u32 },
}

pub(crate) struct Strip {
    pub view: CautionView,
    pub until: Instant,
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
    episode_start: Option<Instant>,
    /// Reason and connector of the latest alarm, for the green "cleared" notch.
    last_alarm: Option<(String, String)>,
    tracking_pending: bool,
    /// Before the first connection: since when a device has been present but silent, and why.
    pub(crate) pending: Option<(Instant, String)>,
    not_connected_logged: bool,
    pub(crate) notice: Option<Notice>,
    notice_seq: u32,
    pub(crate) state: CoreState,
    state_dirty: bool,
    pub(crate) strip: Option<Strip>,
    chime_seq: u32,
    wall: fn() -> String,
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

    fn push_notice(&mut self, title: &str, text: String, warning: bool, connector: Option<ConnectorKey>) {
        self.notice_seq += 1;
        self.notice = Some(Notice { id: self.notice_seq, title: title.into(), text, warning, connector });
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
        let needs_verdict = self.source.as_ref().is_some_and(|s| s.caps.device_verdict);
        let healthy = r.readings.is_some() && (!needs_verdict || r.verdicts.is_some());

        if let Some(p) = r.protection
            && self.protection.as_ref() != Some(&p)
        {
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
                out.log.push(LogEvent::PsuFlag { fault: f.to_string(), set: true });
            }
            for f in self.faults.iter().filter(|f| !faults.contains(f)) {
                out.log.push(LogEvent::PsuFlag { fault: f.to_string(), set: false });
            }
            self.faults = faults;
            if let Some(f) = new.first() {
                self.raise_caution(None, format!("PSU fault: {f}"), "The PSU may shut down".into(), now);
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

    /// Turn a cable-guard event into log lines, notes, the strip or a notice (Spec §8).
    fn on_guard(&mut self, ci: usize, e: GuardEvent, now: Instant, out: &mut Output) {
        let l = self.limits.limits;
        let c = &self.conns[ci];
        let (label, wires, place, name) = (c.label.clone(), wires_text(&c.wires), self.short_name(c), self.name(c));
        match e {
            GuardEvent::OverloadStart { wire, amps: a, rule } => {
                let rule = match rule {
                    Rule::Instant => format!("1 reading >= {}", amps(l.instant)),
                    Rule::Fast => format!("2 readings >= {}", amps(l.fast)),
                    Rule::Sustained => format!(">= {} for {} s", amps(l.alarm), l.alarm_delay.as_secs()),
                };
                let spread = self.conns[ci].eval.spread.map(|s| format!(" | imbalance {}", amps(s))).unwrap_or_default();
                out.log.push(LogEvent::Overload { conn: label, detail: format!("wire {} = {} · {rule} | wires {wires}{spread}", wire + 1, amps(a)) });
                self.conns[ci].live_note = Some(LiveNote::Overload { when: self.when() });
            }
            GuardEvent::OverloadEnd { lasted, peak } => {
                out.log.push(LogEvent::OverloadEnd { conn: label, lasted, detail: format!("peak wire {} = {}", peak.0 + 1, amps(peak.1)) });
                if matches!(self.conns[ci].live_note, Some(LiveNote::Overload { .. })) {
                    self.conns[ci].live_note = None;
                }
            }
            GuardEvent::CautionStart { wire, amps: a } => {
                out.log.push(LogEvent::Caution {
                    conn: label,
                    detail: format!("wire {} = {} above the {} rating for {} s | wires {wires}", wire + 1, amps(a), amps(l.rating), guard::QUALIFY.as_secs()),
                });
                if !matches!(self.conns[ci].live_note, Some(LiveNote::Overload { .. })) {
                    self.conns[ci].live_note = Some(LiveNote::Caution { when: self.when() });
                }
                self.raise_caution(
                    place,
                    format!("A wire at {}, above the {} rating", amps(a), amps(l.rating)),
                    "Ease the GPU load".into(),
                    now,
                );
            }
            GuardEvent::CautionEnd { lasted, peak } => {
                out.log.push(LogEvent::CautionEnd { conn: label, lasted, detail: format!("peak wire {} = {}", peak.0 + 1, amps(peak.1)) });
                if matches!(self.conns[ci].live_note, Some(LiveNote::Caution { .. })) {
                    self.conns[ci].live_note = None;
                }
            }
            GuardEvent::UnevenStart { spread, avg, low, high } => {
                out.log.push(LogEvent::Uneven {
                    conn: label.clone(),
                    detail: format!("imbalance {} at {} average for {} s | wires {wires}", amps(spread), amps(avg), guard::QUALIFY.as_secs()),
                });
                let text = format!(
                    "Uneven load{}: one wire carried {} while the others carried up to {}. Check that the cable is fully seated at both ends.",
                    on(&self.when()),
                    amps(low.1),
                    amps(high)
                );
                self.set_note(ci, Severity::Advisory, text);
                // Higher levels supersede: no advice while an alarm is on screen.
                if self.settings.alarm_enabled && self.active_set().is_empty() {
                    let key = self.conns[ci].key.clone();
                    let on_what = if name == "GPU power cable" { "the GPU power cable".to_owned() } else { name };
                    self.push_notice(
                        &format!("Uneven load on {on_what}"),
                        "Check that the cable is fully seated at both ends. Details are in MeltAlarm.".into(),
                        false,
                        Some(key),
                    );
                }
            }
            GuardEvent::UnevenEnd { lasted, peak_spread } => {
                out.log.push(LogEvent::UnevenEnd { conn: label, lasted, detail: format!("peak imbalance {}", amps(peak_spread)) });
            }
        }
    }

    /// Keep the note of a running overload or caution episode at its peak.
    fn refresh_live_note(&mut self, ci: usize) {
        let l = self.limits.limits;
        let c = &self.conns[ci];
        let note = match &c.live_note {
            Some(LiveNote::Overload { when }) => c.guard.overload().map(|o| {
                (
                    Severity::Alarm,
                    format!(
                        "Alarm{}: a wire reached {}. Inspect the cable and both connectors with the PC off before the next session.",
                        on(when),
                        amps(o.peak.1)
                    ),
                )
            }),
            Some(LiveNote::Caution { when }) => c.guard.caution_peak().map(|(_, a)| {
                (
                    Severity::Caution,
                    format!(
                        "A wire ran above the {} rating{} (peak {}). Check that the cable is fully seated, or lower the GPU power limit.",
                        amps(l.rating),
                        on(when),
                        amps(a)
                    ),
                )
            }),
            None => None,
        };
        if let Some((severity, text)) = note {
            self.set_note(ci, severity, text);
        }
    }

    /// A note replaces the connector's note only if it is at least as severe (Spec §8.10).
    fn set_note(&mut self, ci: usize, severity: Severity, text: String) {
        let key = self.conns[ci].key.clone();
        if self.state.notes.get(&key).is_some_and(|n| n.severity > severity || (n.severity == severity && n.text == text)) {
            return;
        }
        self.state.notes.insert(key, CableNote { severity, text });
        self.state_dirty = true;
    }

    fn when(&self) -> String {
        note_time(&(self.wall)())
    }

    /// Show the caution strip (Spec §8.8), unless alerts are off or an alarm is active. A new
    /// caution while one is visible replaces its text without a second chime.
    fn raise_caution(&mut self, place: Option<String>, what: String, action: String, now: Instant) {
        if !self.settings.alarm_enabled || !self.active_set().is_empty() {
            return;
        }
        let visible = self.strip.as_ref().is_some_and(|s| s.until > now);
        if !visible {
            self.chime_seq += 1;
        }
        self.strip = Some(Strip { view: CautionView { chime: self.chime_seq, place, what, action, test: false }, until: now + STRIP_SHOW });
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
                    self.phase = Phase::Test { until: now + TEST_STRIP + TEST_LENGTH, strip_until: now + TEST_STRIP, chime: self.chime_seq };
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

    pub(crate) fn active_set(&self) -> Vec<(u8, Cause)> {
        let mut v = Vec::new();
        for c in &self.conns {
            if c.guard.overload().is_some() {
                v.push((c.key.index, Cause::Overload));
            }
            if let Some(s) = c.device_alarm() {
                v.push((c.key.index, Cause::Device(s)));
            }
        }
        v
    }

    /// The connector whose alarm the notch shows: the device's critical status first, then
    /// any device status (it has the countdown), then our overload.
    pub(crate) fn worst_conn(&self) -> Option<&Conn> {
        self.conns.iter().filter(|c| c.in_alarm()).max_by_key(|c| match c.device_alarm() {
            Some(DeviceStatus::CriticalOverCurrent) => 2,
            Some(_) => 1,
            None => 0,
        })
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
            self.episode_start = self.conns.iter().flat_map(|c| [c.alarm_since, c.guard.overload().map(|o| o.since)]).flatten().min().or(Some(now));
        }
        if let Some(c) = self.worst_conn() {
            let reason = match c.device_alarm() {
                Some(s) => status_name(s),
                None => "Wire overload".into(),
            };
            self.last_alarm = Some((reason, self.name(c)));
        }
        let cleared = |core: &Core| {
            let (reason, label) = core.last_alarm.clone().unwrap_or_default();
            Phase::Cleared { until: now + CLEARED_SHOW, lasted: core.episode_start.map_or(Duration::ZERO, |s| now.duration_since(s)), reason, label }
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
            Phase::Cleared { until, .. } | Phase::Test { until, .. } if now >= until => Phase::Idle,
            other => other,
        };
        if active.is_empty() && !matches!(self.phase, Phase::Cleared { .. }) {
            self.episode_start = None;
        }
        // Higher levels supersede: no caution strip while an alarm is active (Spec §8).
        if !active.is_empty() {
            self.strip = None;
        }
    }

    /// Remember whether an alarm is open, so a power cut is noticed at the next start.
    fn update_alarm_open(&mut self) {
        let open = self.active_set().first().and_then(|(i, _)| self.conns.iter().find(|c| c.key.index == *i)).map(|c| c.key.clone());
        if open.is_some() != self.state.alarm_open.is_some() {
            self.state.alarm_open = open;
            self.state_dirty = true;
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

fn wires_text(w: &Option<[Option<f32>; WIRES]>) -> String {
    match w {
        Some(w) => w.iter().map(|a| a.map_or("-".into(), |a| format!("{a:.1}"))).collect::<Vec<_>>().join(" "),
        None => "unavailable".into(),
    }
}

/// `2026-09-30 18:02:11` → `Sep 30, 18:02`; anything else → empty (the note then has no time).
fn note_time(wall: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (Some(date), Some(time)) = (wall.get(0..10), wall.get(11..16)) else { return String::new() };
    let mut parts = date.split('-').skip(1);
    let (Some(m), Some(d)) = (parts.next().and_then(|m| m.parse::<usize>().ok()), parts.next().and_then(|d| d.parse::<u32>().ok())) else {
        return String::new();
    };
    match MONTHS.get(m.wrapping_sub(1)) {
        Some(name) => format!("{name} {d}, {time}"),
        None => String::new(),
    }
}

/// " on Sep 30, 18:02", or nothing without a time.
fn on(when: &str) -> String {
    if when.is_empty() { String::new() } else { format!(" on {when}") }
}

#[cfg(test)]
mod tests;
