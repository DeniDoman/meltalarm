//! The declarative view: everything a frontend needs to render, including all user-facing
//! text (docs/ARCHITECTURE.md D4, D8). Frontends only render and reconcile this.

use std::time::{Duration, Instant};

use meltalarm_model::{ConnectorKey, DeviceStatus, WIRES};

use crate::levels::{CAUTION_WIRE_RATIO, Level};
use crate::settings::Settings;
use crate::{Conn, Core, Phase, STALE_AFTER};

#[derive(Clone, Debug, PartialEq)]
pub struct ViewModel {
    pub health: Health,
    pub connectors: Vec<ConnectorView>,
    pub source: Option<SourceView>,
    pub alarm: Option<AlarmView>,
    /// `Some` ⇔ the alarm should be sounding right now.
    pub audio: Option<AudioScript>,
    pub settings: Settings,
    /// While no source has connected yet: tooltip for a single placeholder tray icon.
    pub connecting: Option<String>,
    /// One-shot user notification; frontends show each `id` once.
    pub notice: Option<Notice>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: u32,
    pub title: String,
    pub text: String,
}

impl ViewModel {
    pub fn tracked(&self) -> impl Iterator<Item = &ConnectorView> {
        self.connectors.iter().filter(|c| c.tracked)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Health {
    Starting,
    /// A supported device is present but not answering yet (before the first connection).
    Connecting { since: Instant },
    Live,
    Stale { age: Duration },
    NoData { since: Instant },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Normal,
    /// The device raised an alarm on this connector: solid tile, blinking.
    Alarm,
    NoData,
    NotConnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Normal,
    Alarm,
    NoData,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WireView {
    pub amps: Option<f32>,
    pub level: Level,
    pub flagged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Caution,
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub kind: NoteKind,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorView {
    pub key: ConnectorKey,
    pub label: String,
    pub tracked: bool,
    pub present: bool,
    pub stale: bool,
    pub wires: [WireView; WIRES],
    pub total: Option<f32>,
    pub spread: Option<f32>,
    pub spread_level: Level,
    /// Full-scale value of the bars (the device's wire limit) and the caution line.
    pub bar_limit: Option<f32>,
    pub caution_line: Option<f32>,
    pub glyph: Glyph,
    /// Amber marker: Safeguard+ off or a device fault flag set.
    pub attention: bool,
    pub status_kind: StatusKind,
    pub status_text: String,
    /// During a device alarm: reason line and countdown for the popup.
    pub alarm_reason: Option<String>,
    pub countdown: Option<String>,
    pub limits_text: Option<String>,
    pub notes: Vec<Note>,
    pub tooltip: String,
    /// Compact layouts (floating monitor): `OK`, `PSU ALARM` or `No data`.
    pub short_status: String,
    /// Compact layouts: one line under the bars, and the level that colors it.
    pub summary: String,
    pub summary_level: Level,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceView {
    pub model: String,
    pub firmware: Option<String>,
    pub protection: String,
    pub faults: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmKind {
    Active,
    Critical,
    DataLost,
    Cleared,
    Test,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlarmView {
    pub kind: AlarmKind,
    /// Green band for `Cleared`, red otherwise.
    pub green: bool,
    pub test: bool,
    pub headline: String,
    pub connector: String,
    pub action: String,
    pub sub: String,
    pub what: String,
    pub numbers: String,
    pub right_label: String,
    pub right_value: String,
    pub bars: [WireView; WIRES],
    pub bar_limit: Option<f32>,
    pub note: Option<String>,
    pub snooze: bool,
    /// 0‥1 elapsed share of the green "cleared" display.
    pub cleared_progress: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AudioStep {
    Sound,
    Pause(Duration),
    Speak(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioScript {
    pub steps: Vec<AudioStep>,
    pub repeat: bool,
}

pub(crate) fn amps(a: f32) -> String {
    format!("{a:.1} A")
}

fn mmss(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

pub(crate) fn status_name(s: DeviceStatus) -> String {
    match s {
        DeviceStatus::Normal => "Normal".into(),
        DeviceStatus::OverCurrent => "Over-current".into(),
        DeviceStatus::Imbalance => "Current imbalance".into(),
        DeviceStatus::CriticalOverCurrent => "Critical over-current".into(),
        DeviceStatus::Unknown(n) => format!("Unknown PSU alert (0x{n:02X})"),
    }
}

/// For one-line summaries: "Imbalance · cut ~2:13".
fn short_status_name(s: DeviceStatus) -> String {
    match s {
        DeviceStatus::Normal => "Normal".into(),
        DeviceStatus::OverCurrent => "Over-current".into(),
        DeviceStatus::Imbalance => "Imbalance".into(),
        DeviceStatus::CriticalOverCurrent => "Critical".into(),
        DeviceStatus::Unknown(n) => format!("PSU alert 0x{n:02X}"),
    }
}

fn conn_number(label: &str) -> String {
    label.rsplit('#').next().unwrap_or(label).trim().to_owned()
}

impl Core {
    fn conn_view(&self, c: &Conn, now: Instant) -> ConnectorView {
        let health = self.health(now);
        let no_data = matches!(health, Health::NoData { .. } | Health::Starting | Health::Connecting { .. });
        let stale = matches!(health, Health::Stale { .. }) || no_data;
        let present = c.last_nonzero.is_some_and(|t| now.duration_since(t) <= crate::PRESENCE_WINDOW);
        let flagged = c.verdict.as_ref().map(|v| v.flagged).unwrap_or_default();
        let alarm = c.verdict.as_ref().is_some_and(|v| v.status.is_alarm());
        let wires: [WireView; WIRES] = std::array::from_fn(|i| WireView {
            amps: c.wires.and_then(|w| w[i]),
            level: if flagged[i] { Level::Warning } else { c.eval.wires[i] },
            flagged: flagged[i],
        });
        let wire_limit = self.protection.as_ref().and_then(|p| p.wire_limit);
        let attention = !self.faults.is_empty() || self.protection.as_ref().is_some_and(|p| !p.enabled);

        let glyph = if no_data {
            Glyph::NoData
        } else if alarm {
            Glyph::Alarm
        } else if !present {
            Glyph::NotConnected
        } else {
            Glyph::Normal
        };
        let (status_kind, status_text) = if alarm {
            (StatusKind::Alarm, "PSU ALARM".to_owned())
        } else if no_data {
            (StatusKind::NoData, "No data".to_owned())
        } else {
            (StatusKind::Normal, "PSU · Normal".to_owned())
        };

        let mut notes = Vec::new();
        if let Health::NoData { since } = health {
            notes.push(Note {
                kind: NoteKind::Info,
                text: format!(
                    "Monitoring interrupted. Last valid reading {} ago. Retrying.",
                    crate::log::secs(self.last_healthy.map_or(now.duration_since(since), |t| now.duration_since(t)))
                ),
            });
        } else if matches!(health, Health::Starting | Health::Connecting { .. }) {
            notes.push(Note { kind: NoteKind::Info, text: "Reading the PSU…".into() });
        }
        if self.protection.as_ref().is_some_and(|p| !p.enabled) {
            notes.push(Note { kind: NoteKind::Caution, text: "PSU Safeguard+ is OFF — the PSU will not protect the cable.".into() });
        }
        for f in &self.faults {
            notes.push(Note { kind: NoteKind::Caution, text: format!("PSU fault: {f}") });
        }
        if !alarm && !no_data && c.eval.is_red() {
            notes.push(Note { kind: NoteKind::Caution, text: self.red_note(c, now) });
        }

        let alarm_reason = c.verdict.as_ref().filter(|v| v.status.is_alarm()).map(|v| {
            let wires: Vec<String> = (0..WIRES).filter(|&i| v.flagged[i]).map(|i| (i + 1).to_string()).collect();
            if wires.is_empty() { status_name(v.status) } else { format!("{} · wire {}", status_name(v.status), wires.join(", ")) }
        });
        let countdown = if alarm { self.countdown(c, now) } else { None };
        let limits_text = self.protection.as_ref().and_then(|p| match (p.wire_limit, p.spread_limit) {
            (Some(w), Some(s)) => Some(format!("{} · {}", trim(w), amps(s))),
            (Some(w), None) => Some(amps(w)),
            _ => None,
        });

        // One line: the shell wraps tray tips at about 50 characters. "12V-2x6 #1" → "#1".
        let short = c.label.rsplit(' ').next().unwrap_or(&c.label);
        let mut tooltip = format!("MeltAlarm · {short} · ");
        if no_data {
            tooltip.push_str("No data");
        } else if alarm {
            tooltip.push_str(&format!("PSU ALARM: {}", alarm_reason.clone().unwrap_or_default()));
        } else if !present {
            tooltip.push_str("Not connected");
        } else {
            tooltip.push_str("OK");
            if let (Some((_, max)), Some(spread)) = (c.eval.max, c.eval.spread) {
                tooltip.push_str(&format!(" · max {max:.1}A · Δ {spread:.1}A"));
            }
        }

        let short_status = match status_kind {
            StatusKind::Alarm => "PSU ALARM",
            StatusKind::NoData => "No data",
            StatusKind::Normal => "OK",
        }
        .to_owned();
        let (summary, summary_level) = if let Some(v) = c.verdict.as_ref().filter(|v| alarm && v.status.is_alarm()) {
            let name = short_status_name(v.status);
            (countdown.as_ref().map_or(name.clone(), |cd| format!("{name} · cut {cd}")), Level::Warning)
        } else if let Health::NoData { since } = health {
            let age = self.last_healthy.map_or(now.duration_since(since), |t| now.duration_since(t));
            (format!("Last reading {} ago", crate::log::secs(age)), Level::Normal)
        } else if matches!(health, Health::Starting | Health::Connecting { .. }) {
            ("Reading the PSU…".to_owned(), Level::Normal)
        } else if let (Some(t), Some(s)) = (c.eval.total, c.eval.spread) {
            (format!("Σ {t:.1}A · Δ {s:.1}A"), c.eval.spread_level)
        } else {
            ("—".to_owned(), Level::Normal)
        };

        ConnectorView {
            key: c.key.clone(),
            label: c.label.clone(),
            tracked: self.settings.tracked.contains(&c.key),
            present,
            stale,
            wires,
            total: c.eval.total,
            spread: c.eval.spread,
            spread_level: c.eval.spread_level,
            bar_limit: wire_limit,
            caution_line: wire_limit.map(|l| l * CAUTION_WIRE_RATIO),
            glyph,
            attention,
            status_kind,
            status_text,
            alarm_reason,
            countdown,
            limits_text,
            notes,
            tooltip,
            short_status,
            summary,
            summary_level,
        }
    }

    fn red_note(&self, c: &Conn, now: Instant) -> String {
        let p = self.protection.as_ref();
        if let Some(r) = &c.red
            && r.sustained_logged {
                return format!(
                    "Over the PSU limit for {} — the PSU has not raised an alarm.",
                    crate::log::secs(now.duration_since(r.start))
                );
            }
        let trigger = |t: Option<Duration>| t.map(|t| format!(" The PSU raises an alarm if this lasts {}.", crate::log::secs(t))).unwrap_or_default();
        if c.eval.spread_red {
            format!(
                "Spread {} is over the PSU limit of {}.{}",
                amps(c.eval.spread.unwrap_or(0.0)),
                amps(p.and_then(|p| p.spread_limit).unwrap_or(0.0)),
                trigger(p.and_then(|p| p.spread_trigger))
            )
        } else {
            let (i, a) = c.eval.max.unwrap_or((0, 0.0));
            format!(
                "Wire {} at {} is over the PSU limit of {}.{}",
                i + 1,
                amps(a),
                amps(p.and_then(|p| p.wire_limit).unwrap_or(0.0)),
                trigger(p.and_then(|p| p.wire_trigger))
            )
        }
    }

    fn countdown(&self, c: &Conn, now: Instant) -> Option<String> {
        let v = c.verdict.as_ref()?;
        if v.status == DeviceStatus::CriticalOverCurrent {
            return Some("any second".into());
        }
        let cutoff = self.protection.as_ref()?.cutoff_after?;
        let since = c.alarm_since?;
        let left = cutoff.saturating_sub(now.duration_since(since));
        Some(if left.is_zero() { "expected now".into() } else { format!("~{}", mmss(left)) })
    }

    fn alarm_view(&self, now: Instant) -> Option<AlarmView> {
        let active: Vec<&Conn> = self.conns.iter().filter(|c| c.verdict.as_ref().is_some_and(|v| v.status.is_alarm())).collect();
        match &self.phase {
            Phase::Idle | Phase::Snoozed { .. } => None,
            Phase::Test { .. } => {
                let c = self.conns.first()?;
                let mut v = self.alarm_for(c, DeviceStatus::Imbalance, now, true);
                v.what = "Current imbalance · sample data".into();
                v.numbers = "Wire 3 at 2.1 A, others 9.7–9.9 A. Spread 7.8 A.".into();
                v.bars = test_bars();
                v.right_value = "~3:00".into();
                Some(v)
            }
            Phase::Active => {
                let worst = active
                    .iter()
                    .copied()
                    .max_by_key(|c| c.verdict.as_ref().map(|v| v.status == DeviceStatus::CriticalOverCurrent))?;
                let status = worst.verdict.as_ref()?.status;
                let mut v = self.alarm_for(worst, status, now, false);
                if active.len() > 1 {
                    v.connector = active.iter().map(|c| c.label.as_str()).collect::<Vec<_>>().join(" + ");
                }
                Some(v)
            }
            Phase::Cleared { until, lasted, cause, label } => {
                let total = crate::CLEARED_SHOW.as_secs_f32();
                let left = until.saturating_duration_since(now).as_secs_f32();
                Some(AlarmView {
                    kind: AlarmKind::Cleared,
                    green: true,
                    test: false,
                    headline: "BACK TO NORMAL".into(),
                    connector: label.clone(),
                    action: format!("{} RECOVERED", label.to_uppercase()),
                    sub: format!("{} cleared after {}. The event is in the log.", status_name(*cause), crate::log::secs(*lasted)),
                    what: "PSU status · Normal".into(),
                    numbers: self
                        .conns
                        .iter()
                        .find(|c| &c.label == label)
                        .and_then(|c| c.eval.spread)
                        .map(|s| format!("Spread {}. Sound stopped.", amps(s)))
                        .unwrap_or_else(|| "Sound stopped.".into()),
                    right_label: "ALARM LASTED".into(),
                    right_value: mmss(*lasted),
                    bars: self.conns.iter().find(|c| &c.label == label).map(|c| self.conn_view(c, now).wires).unwrap_or_default(),
                    bar_limit: self.protection.as_ref().and_then(|p| p.wire_limit),
                    note: None,
                    snooze: false,
                    cleared_progress: Some(((total - left) / total).clamp(0.0, 1.0)),
                })
            }
        }
    }

    fn alarm_for(&self, c: &Conn, status: DeviceStatus, now: Instant, test: bool) -> AlarmView {
        let cv = self.conn_view(c, now);
        let critical = status == DeviceStatus::CriticalOverCurrent;
        let data_lost = !test && matches!(self.health(now), Health::NoData { .. });
        let p = self.protection.as_ref();
        let numbers = if critical {
            let (i, a) = c.eval.max.unwrap_or((0, 0.0));
            format!("Wire {} at {}. Hard PSU limit {}.", i + 1, amps(a), trim(p.and_then(|p| p.hard_wire_limit).unwrap_or(18.0)))
        } else if status == DeviceStatus::Imbalance {
            let (lo_i, lo) = c.eval.min.unwrap_or((0, 0.0));
            let others: Vec<f32> = cv.wires.iter().enumerate().filter(|(i, _)| *i != lo_i).filter_map(|(_, w)| w.amps).collect();
            let (omin, omax) = others.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
            let limit = p.and_then(|p| p.spread_limit).map(|l| format!(", PSU limit {}", amps(l))).unwrap_or_default();
            if others.is_empty() {
                "Wire readings unavailable.".into()
            } else {
                format!(
                    "Wire {} at {}, others {:.1}–{:.1} A. Spread {}{limit}.",
                    lo_i + 1,
                    amps(lo),
                    omin,
                    omax,
                    amps(c.eval.spread.unwrap_or(0.0))
                )
            }
        } else {
            let (i, a) = c.eval.max.unwrap_or((0, 0.0));
            let limit = p.and_then(|p| p.wire_limit).map(|l| format!(", PSU limit {}", amps(l))).unwrap_or_default();
            format!("Wire {} at {}{limit}.", i + 1, amps(a))
        };
        let (right_label, right_value) = if critical {
            ("POWER CUT".into(), "ANY SECOND".into())
        } else if let Some(cd) = self.countdown(c, now) {
            ("POWER CUT IN".into(), cd)
        } else {
            ("ALARM FOR".into(), c.alarm_since.map(|s| mmss(now.duration_since(s))).unwrap_or_default())
        };
        AlarmView {
            kind: if test {
                AlarmKind::Test
            } else if data_lost {
                AlarmKind::DataLost
            } else if critical {
                AlarmKind::Critical
            } else {
                AlarmKind::Active
            },
            green: false,
            test,
            headline: if critical { "GPU CABLE CRITICAL OVER-CURRENT".into() } else { "GPU POWER CABLE OVERLOAD".into() },
            connector: c.label.clone(),
            action: "STOP GPU LOAD NOW".into(),
            sub: if test {
                "This is a test. Your PSU reports no problem.".into()
            } else if critical {
                "A wire is above the hard 18 A limit. Power can be cut at any moment.".into()
            } else {
                "Quit the game or render. If the load stays, the PSU cuts power.".into()
            },
            what: format!("{} · {}", status_name(status), if data_lost { "last PSU report" } else { "reported by the PSU" }),
            numbers,
            right_label,
            right_value,
            bars: cv.wires,
            bar_limit: cv.bar_limit,
            note: data_lost.then(|| "PSU data lost. MeltAlarm cannot confirm the problem is gone, so the alarm stays.".into()),
            snooze: true,
            cleared_progress: None,
        }
    }

    fn audio(&self) -> Option<AudioScript> {
        let (critical, test) = match self.phase {
            Phase::Active => (
                self.conns.iter().any(|c| c.verdict.as_ref().is_some_and(|v| v.status == DeviceStatus::CriticalOverCurrent)),
                false,
            ),
            Phase::Test { .. } => (false, true),
            _ => return None,
        };
        let conns: Vec<String> = if test {
            self.conns.first().map(|c| conn_number(&c.label)).into_iter().collect()
        } else {
            self.conns.iter().filter(|c| c.verdict.as_ref().is_some_and(|v| v.status.is_alarm())).map(|c| conn_number(&c.label)).collect()
        };
        let which = match conns.len() {
            0 => String::new(),
            1 => format!(" on connector {}", conns[0]),
            _ => format!(" on connectors {}", conns.join(" and ")),
        };
        let what = if critical { "Critical GPU cable over-current" } else { "GPU power cable overload" };
        let prefix = if test { "This is a test. " } else { "Warning. " };
        let gap = AudioStep::Pause(Duration::from_millis(300));
        Some(AudioScript {
            steps: vec![
                AudioStep::Sound,
                gap.clone(),
                AudioStep::Sound,
                gap.clone(),
                AudioStep::Sound,
                gap,
                AudioStep::Speak(format!("{prefix}{what}{which}. Stop the game now.")),
            ],
            repeat: true,
        })
    }

    pub(crate) fn health(&self, now: Instant) -> Health {
        if self.source.is_none()
            && let Some((since, _)) = &self.pending
        {
            return Health::Connecting { since: *since };
        }
        if let Some(since) = self.no_data_since {
            return Health::NoData { since };
        }
        match self.last_healthy {
            None => Health::Starting,
            Some(t) if !self.suspended && now.duration_since(t) > STALE_AFTER => Health::Stale { age: now.duration_since(t) },
            Some(_) => Health::Live,
        }
    }

    pub fn view(&self, now: Instant) -> ViewModel {
        let source = self.source.as_ref().map(|s| SourceView {
            model: format!("{} {}", s.vendor, s.model),
            firmware: s.firmware.clone(),
            protection: self.protection.as_ref().map_or_else(|| "Reading…".into(), protection_summary),
            faults: self.faults.iter().map(|f| f.to_string()).collect(),
        });
        ViewModel {
            health: self.health(now),
            connectors: self.conns.iter().map(|c| self.conn_view(c, now)).collect(),
            source,
            alarm: self.alarm_view(now),
            audio: self.audio(),
            settings: self.settings.clone(),
            connecting: self.source.is_none().then(|| {
                if self.pending.is_some() { "MeltAlarm · connecting to the PSU…".into() } else { "MeltAlarm · looking for the PSU…".into() }
            }),
            notice: self.notice.clone(),
        }
    }
}

fn trim(a: f32) -> String {
    if a.fract() == 0.0 { format!("{a:.0} A") } else { amps(a) }
}

pub(crate) fn protection_summary(p: &meltalarm_model::Protection) -> String {
    if !p.enabled {
        return "Safeguard+ OFF".into();
    }
    let mut parts = vec!["Safeguard+ ON".to_owned()];
    if let Some(w) = p.wire_limit {
        parts.push(format!("OCP {}{}", amps(w), p.wire_trigger.map(|t| format!(" for {}", crate::log::secs(t))).unwrap_or_default()));
    }
    if let Some(s) = p.spread_limit {
        parts.push(format!("imbalance {}{}", amps(s), p.spread_trigger.map(|t| format!(" for {}", crate::log::secs(t))).unwrap_or_default()));
    }
    if let Some(c) = p.cutoff_after {
        parts.push(format!("power cut {} after alarm", crate::log::secs(c)));
    }
    parts.join(" · ")
}

fn test_bars() -> [WireView; WIRES] {
    let v = [9.9, 9.8, 2.1, 9.7, 9.8, 9.9];
    std::array::from_fn(|i| WireView {
        amps: Some(v[i]),
        level: if i == 2 { Level::Warning } else { Level::Caution },
        flagged: i == 2,
    })
}
