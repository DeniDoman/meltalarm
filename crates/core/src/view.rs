//! The declarative view: everything a frontend needs to render, including all user-facing
//! text (docs/ARCHITECTURE.md D4, D8). Frontends only render and reconcile this.

use std::time::{Duration, Instant};

use meltalarm_model::{ConnectorKey, DeviceStatus, WIRES};

use crate::levels::Level;
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
    /// The caution strip (Spec §8.8), while it should be on screen.
    pub caution: Option<CautionView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: u32,
    pub title: String,
    pub text: String,
    /// Plays the notification sound; otherwise silent (DESIGN.md "Notifications").
    pub warning: bool,
    /// A click opens this connector's flyout.
    pub connector: Option<ConnectorKey>,
}

/// The caution strip: one line, `place · what · action` (Spec §8.8).
#[derive(Clone, Debug, PartialEq)]
pub struct CautionView {
    /// Changes once per chime: a frontend plays the chime when it sees a new value.
    pub chime: u32,
    /// `#1`, or none for things that aren't about one connector.
    pub place: Option<String>,
    pub what: String,
    pub action: String,
    pub test: bool,
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
    /// An alarm (either judge) on this connector: solid tile, blinking.
    Alarm,
    NoData,
    NotConnected,
}

/// The state words, the same on every surface (Spec §7.1): `OK`, `Caution`, `ALARM`, `No data`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Normal,
    Caution,
    Alarm,
    NoData,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WireView {
    pub amps: Option<f32>,
    pub level: Level,
    /// Flagged by the device, or the peak wire of our overload: knocked out in the alarm tile.
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
    /// The display name: "GPU power cable", numbered only when that helps (Spec §7.0).
    pub label: String,
    /// Always numbered: "GPU power cable 1" (menus, Settings).
    pub full_label: String,
    pub tracked: bool,
    pub present: bool,
    pub stale: bool,
    pub wires: [WireView; WIRES],
    /// The most minus the least loaded wire (Spec §7.0).
    pub imbalance: Option<f32>,
    pub imbalance_level: Level,
    /// Full scale of the bars (the alarm limit) and the dashed line (the rating).
    pub bar_limit: Option<f32>,
    pub caution_line: Option<f32>,
    pub glyph: Glyph,
    /// Amber marker: something to read in the flyout (a cable note, a PSU fault, Safeguard+ off).
    pub attention: bool,
    pub status_kind: StatusKind,
    /// `OK`, `Caution`, `ALARM` or `No data`, for every layout.
    pub status_text: String,
    /// During an alarm: reason line and (from the device) countdown.
    pub alarm_reason: Option<String>,
    pub countdown: Option<String>,
    /// Live lines while true (Spec §7.2).
    pub notes: Vec<Note>,
    /// The connector's cable note (Spec §8.10), shown with *Dismiss* once the connector is back to
    /// normal; while a problem is live, the live line says it.
    pub cable_note: Option<String>,
    /// The device's own verdict in words: `Normal`, `Current imbalance`, `Safeguard+ off`, or `—`.
    pub psu_status: String,
    pub psu_level: Level,
    pub tooltip: String,
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
    /// Where the bars are cut (the rating, DESIGN.md "Cut bars").
    pub bar_rating: Option<f32>,
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

impl Core {
    /// The cable number is shown when more than one cable is tracked, or for an untracked one.
    pub(crate) fn numbered(&self, c: &Conn) -> bool {
        let tracked = self.conns.iter().filter(|k| self.settings.tracked.contains(&k.key)).count();
        tracked > 1 || !self.settings.tracked.contains(&c.key)
    }

    pub(crate) fn name(&self, c: &Conn) -> String {
        if self.numbered(c) { c.label.clone() } else { "GPU power cable".into() }
    }

    /// "Cable 2" where space is tight, or nothing when no number is needed.
    pub(crate) fn short_name(&self, c: &Conn) -> Option<String> {
        self.numbered(c).then(|| format!("Cable {}", c.key.index + 1))
    }

    fn conn_view(&self, c: &Conn, now: Instant) -> ConnectorView {
        let l = self.limits.limits;
        let health = self.health(now);
        let no_data = matches!(health, Health::NoData { .. } | Health::Starting | Health::Connecting { .. });
        let stale = matches!(health, Health::Stale { .. }) || no_data;
        let present = c.last_nonzero.is_some_and(|t| now.duration_since(t) <= crate::PRESENCE_WINDOW);
        let device = c.device_alarm();
        let overload = c.guard.overload();
        let alarm = c.in_alarm();
        let flagged = c.verdict.as_ref().map(|v| v.flagged).unwrap_or_default();
        let wires: [WireView; WIRES] = std::array::from_fn(|i| WireView {
            amps: c.wires.and_then(|w| w[i]),
            level: if flagged[i] { Level::Warning } else { c.eval.wires[i] },
            flagged: flagged[i] || overload.is_some_and(|o| o.peak.0 == i),
        });
        let safeguard_off = self.protection.as_ref().is_some_and(|p| !p.enabled);
        let cable_note = self.state.notes.get(&c.key).map(|n| n.text.clone());
        let attention = !self.faults.is_empty() || safeguard_off || cable_note.is_some();
        let live = c.eval.worst();

        let glyph = if no_data {
            Glyph::NoData
        } else if alarm {
            Glyph::Alarm
        } else if !present {
            Glyph::NotConnected
        } else {
            Glyph::Normal
        };
        let status_kind = if alarm {
            StatusKind::Alarm
        } else if no_data {
            StatusKind::NoData
        } else if live > Level::Normal {
            StatusKind::Caution
        } else {
            StatusKind::Normal
        };
        let status_text = match status_kind {
            StatusKind::Normal => "OK",
            StatusKind::Caution => "Caution",
            StatusKind::Alarm => "ALARM",
            StatusKind::NoData => "No data",
        }
        .to_owned();

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
        if safeguard_off {
            notes.push(Note { kind: NoteKind::Caution, text: "PSU Safeguard+ is OFF: the PSU won't cut power. MeltAlarm still alarms.".into() });
        }
        for f in &self.faults {
            notes.push(Note { kind: NoteKind::Caution, text: format!("PSU fault: {f}") });
        }
        if !alarm && !no_data {
            if let Some((_, a)) = c.eval.max.filter(|&(_, a)| a >= l.rating) {
                let text = if a >= l.alarm {
                    format!("A wire at {}, over the {} alarm limit.", amps(a), amps(l.alarm))
                } else {
                    format!("A wire at {}, above the {} rating.", amps(a), amps(l.rating))
                };
                notes.push(Note { kind: NoteKind::Caution, text });
            }
            if c.eval.spread_level > Level::Normal
                && let (Some((_, lo)), Some((_, hi))) = (c.eval.min, c.eval.max)
            {
                notes.push(Note {
                    kind: NoteKind::Caution,
                    text: format!("Uneven load: one wire carries {}, the others up to {}.", amps(lo), amps(hi)),
                });
            }
        }

        let alarm_reason = match (device, overload) {
            (Some(s), _) => Some(format!("PSU: {}", status_name(s))),
            (None, Some(o)) => {
                let now_a = c.wires.and_then(|w| w[o.peak.0]).unwrap_or(o.peak.1);
                Some(format!("Wire overload · {}", amps(now_a)))
            }
            (None, None) => None,
        };
        let countdown = if device.is_some() { self.countdown(c, now) } else { None };
        let psu_status = match (&self.source, c.verdict.as_ref()) {
            (Some(s), _) if !s.caps.device_verdict => "—".to_owned(),
            (_, Some(v)) if v.status.is_alarm() => status_name(v.status),
            _ if safeguard_off => "Safeguard+ off".to_owned(),
            (_, Some(_)) => "Normal".to_owned(),
            (_, None) => "—".to_owned(),
        };
        let psu_level = if device.is_some() {
            Level::Warning
        } else if safeguard_off {
            Level::Caution
        } else {
            Level::Normal
        };

        // One line: the shell wraps tray tips at about 50 characters.
        let mut tooltip = match self.short_name(c) {
            Some(short) => format!("MeltAlarm · {short} · "),
            None => "MeltAlarm · ".to_owned(),
        };
        let numbers = |t: &mut String| {
            if let (Some((_, max)), Some(spread)) = (c.eval.max, c.eval.spread) {
                t.push_str(&format!(" · max {max:.1}A · Δ {spread:.1}A"));
            }
        };
        if no_data {
            tooltip.push_str("No data");
        } else if alarm {
            let reason = match (device, overload) {
                (Some(s), _) => short_status_name(s),
                (None, Some(_)) => "wire overload".to_owned(),
                _ => String::new(),
            };
            tooltip.push_str(&format!("ALARM: {reason}"));
        } else if !present {
            tooltip.push_str("Not connected");
        } else if live > Level::Normal {
            tooltip.push_str("Caution");
            numbers(&mut tooltip);
        } else if cable_note.is_some() {
            tooltip.push_str("OK · check the cable");
        } else {
            tooltip.push_str("OK");
            numbers(&mut tooltip);
        }

        // Compact summary: the most important thing (Spec §7.4).
        let (summary, summary_level) = if let Some(s) = device {
            let name = short_status_name(s);
            let cd = self.countdown(c, now);
            (cd.map_or(name.clone(), |cd| format!("{name} · cut {cd}")), Level::Warning)
        } else if let Some(o) = overload {
            let now_a = c.wires.and_then(|w| w[o.peak.0]).unwrap_or(o.peak.1);
            (format!("Overload · {} · stop", amps(now_a)), Level::Warning)
        } else if let Health::NoData { since } = health {
            let age = self.last_healthy.map_or(now.duration_since(since), |t| now.duration_since(t));
            (format!("Last reading {} ago", crate::log::secs(age)), Level::Normal)
        } else if matches!(health, Health::Starting | Health::Connecting { .. }) {
            ("Reading the PSU…".to_owned(), Level::Normal)
        } else if let Some((_, a)) = c.eval.max.filter(|&(_, a)| a >= l.rating) {
            (format!("{} · over rating", amps(a)), Level::Caution)
        } else if c.eval.spread_level > Level::Normal {
            (format!("Imbalance {}", amps(c.eval.spread.unwrap_or(0.0))), Level::Caution)
        } else if cable_note.is_some() {
            ("Check the cable".to_owned(), Level::Caution)
        } else if let Some(s) = c.eval.spread {
            (format!("Imbalance {}", amps(s)), Level::Normal)
        } else {
            ("—".to_owned(), Level::Normal)
        };

        ConnectorView {
            key: c.key.clone(),
            label: self.name(c),
            full_label: c.label.clone(),
            tracked: self.settings.tracked.contains(&c.key),
            present,
            stale,
            wires,
            imbalance: c.eval.spread,
            imbalance_level: c.eval.spread_level,
            bar_limit: Some(l.alarm),
            caution_line: Some(l.rating),
            glyph,
            attention,
            status_kind,
            status_text,
            alarm_reason,
            countdown,
            notes,
            cable_note: cable_note.filter(|_| matches!(status_kind, StatusKind::Normal | StatusKind::NoData)),
            psu_status,
            psu_level,
            tooltip,
            summary,
            summary_level,
        }
    }

    fn countdown(&self, c: &Conn, now: Instant) -> Option<String> {
        let status = c.device_alarm()?;
        if status == DeviceStatus::CriticalOverCurrent {
            return Some("any second".into());
        }
        let cutoff = self.protection.as_ref()?.cutoff_after?;
        let since = c.alarm_since?;
        let left = cutoff.saturating_sub(now.duration_since(since));
        Some(if left.is_zero() { "expected now".into() } else { format!("~{}", mmss(left)) })
    }

    fn alarm_view(&self, now: Instant) -> Option<AlarmView> {
        match &self.phase {
            Phase::Idle | Phase::Snoozed { .. } => None,
            Phase::Test { strip_until, .. } if now < *strip_until => None,
            Phase::Test { .. } => {
                let c = self.conns.first()?;
                let mut v = self.alarm_for(c, now, true);
                v.what = "Current imbalance · sample data".into();
                v.numbers = "Lowest wire 2.1 A, the others 9.7–9.9 A. Imbalance 7.8 A.".into();
                v.bars = test_bars();
                v.right_label = "POWER CUT IN".into();
                v.right_value = "~3:00".into();
                Some(v)
            }
            Phase::Active => {
                let worst = self.worst_conn()?;
                let mut v = self.alarm_for(worst, now, false);
                let in_alarm: Vec<String> = self.conns.iter().filter(|c| c.in_alarm()).map(|c| (c.key.index + 1).to_string()).collect();
                if in_alarm.len() > 1 {
                    v.connector = format!("GPU power cables {}", in_alarm.join(" + "));
                }
                Some(v)
            }
            Phase::Cleared { until, lasted, reason, label } => {
                let total = crate::CLEARED_SHOW.as_secs_f32();
                let left = until.saturating_duration_since(now).as_secs_f32();
                let conn = self.conns.iter().find(|c| &self.name(c) == label);
                Some(AlarmView {
                    kind: AlarmKind::Cleared,
                    green: true,
                    test: false,
                    headline: "BACK TO NORMAL".into(),
                    connector: label.clone(),
                    action: "LOAD BACK TO NORMAL".into(),
                    sub: "Inspect the cable before the next session. The note is in MeltAlarm.".into(),
                    what: format!("{reason} · cleared after {}", crate::log::secs(*lasted)),
                    numbers: if reason == "Wire overload" {
                        format!("Every wire is below the {} rating again. Sound stopped.", amps(self.limits.limits.rating))
                    } else {
                        "The PSU reports Normal again. Sound stopped.".into()
                    },
                    right_label: "ALARM LASTED".into(),
                    right_value: mmss(*lasted),
                    bars: conn.map(|c| self.conn_view(c, now).wires).unwrap_or_default(),
                    bar_limit: Some(self.limits.limits.alarm),
                    bar_rating: Some(self.limits.limits.rating),
                    note: None,
                    snooze: false,
                    cleared_progress: Some(((total - left) / total).clamp(0.0, 1.0)),
                })
            }
        }
    }

    fn alarm_for(&self, c: &Conn, now: Instant, test: bool) -> AlarmView {
        let l = self.limits.limits;
        let cv = self.conn_view(c, now);
        let status = if test { Some(DeviceStatus::Imbalance) } else { c.device_alarm() };
        let overload = if test { None } else { c.guard.overload() };
        let critical = status == Some(DeviceStatus::CriticalOverCurrent);
        let data_lost = !test && matches!(self.health(now), Health::NoData { .. });
        let p = self.protection.as_ref();
        let ours = overload.map(|o| {
            let a = c.wires.and_then(|w| w[o.peak.0]).unwrap_or(o.peak.1);
            (o.peak.0, a)
        });
        let our_line = ours.map(|(_, a)| format!("A wire carries {}, rated {}.", amps(a), amps(l.rating)));
        let psu_numbers = |s: DeviceStatus| -> String {
            if s == DeviceStatus::CriticalOverCurrent {
                let (_, a) = c.eval.max.unwrap_or((0, 0.0));
                format!("Highest wire {}. Hard PSU limit {}.", amps(a), trim(p.and_then(|p| p.hard_wire_limit).unwrap_or(18.0)))
            } else if s == DeviceStatus::Imbalance {
                let (lo_i, lo) = c.eval.min.unwrap_or((0, 0.0));
                let others: Vec<f32> = cv.wires.iter().enumerate().filter(|(i, _)| *i != lo_i).filter_map(|(_, w)| w.amps).collect();
                let (omin, omax) = others.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
                let limit = p.and_then(|p| p.spread_limit).map(|l| format!(", PSU limit {}", amps(l))).unwrap_or_default();
                if others.is_empty() {
                    "Wire readings unavailable.".into()
                } else {
                    format!("Lowest wire {}, the others {:.1}–{:.1} A. Imbalance {}{limit}.", amps(lo), omin, omax, amps(c.eval.spread.unwrap_or(0.0)))
                }
            } else {
                let (_, a) = c.eval.max.unwrap_or((0, 0.0));
                let limit = p.and_then(|p| p.wire_limit).map(|l| format!(", PSU limit {}", amps(l))).unwrap_or_default();
                format!("Highest wire {}{limit}.", amps(a))
            }
        };
        let source_word = if data_lost { "last PSU report" } else { "reported by the PSU" };
        let (what, numbers) = match (status, &our_line) {
            (Some(s), Some(ours)) => (format!("{} · {source_word}", status_name(s)), format!("{} {ours}", psu_numbers(s))),
            (Some(s), None) => (format!("{} · {source_word}", status_name(s)), psu_numbers(s)),
            (None, Some(ours)) => ("Wire overload · measured by MeltAlarm".to_owned(), format!("{ours} The PSU hasn't raised an alarm yet.")),
            (None, None) => (String::new(), String::new()),
        };
        let (right_label, right_value) = if critical {
            ("POWER CUT".into(), "ANY SECOND".into())
        } else if let Some(cd) = self.countdown(c, now) {
            ("POWER CUT IN".into(), cd)
        } else if let Some((_, a)) = ours {
            ("HIGHEST WIRE".to_owned(), amps(a))
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
            connector: self.name(c),
            action: "STOP GPU LOAD NOW".into(),
            sub: if test {
                "This is a test. Your cable reports no problem.".into()
            } else if critical {
                "A wire is above the hard 18 A limit. Power can be cut at any moment.".into()
            } else if status.is_some() {
                "Quit the game or render. If the load stays, the PSU cuts power.".into()
            } else {
                "Quit the game or render. A wire carries more than the connector is rated for.".into()
            },
            what,
            numbers,
            right_label,
            right_value,
            bars: cv.wires,
            bar_limit: cv.bar_limit,
            bar_rating: cv.caution_line,
            note: data_lost.then(|| "PSU data lost. MeltAlarm cannot confirm the problem is gone, so the alarm stays.".into()),
            snooze: true,
            cleared_progress: None,
        }
    }

    fn audio_at(&self, now: Instant) -> Option<AudioScript> {
        let (critical, test) = match self.phase {
            Phase::Active => (self.conns.iter().any(|c| c.device_alarm() == Some(DeviceStatus::CriticalOverCurrent)), false),
            Phase::Test { strip_until, .. } if now >= strip_until => (false, true),
            _ => return None,
        };
        let conns: Vec<&Conn> = if test { self.conns.iter().take(1).collect() } else { self.conns.iter().filter(|c| c.in_alarm()).collect() };
        let numbers: Vec<String> = conns.iter().map(|c| (c.key.index + 1).to_string()).collect();
        let which = match conns.as_slice() {
            [c] if self.numbered(c) => format!(" on cable {}", numbers[0]),
            [_] | [] => String::new(),
            _ => format!(" on cables {}", numbers.join(" and ")),
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

    fn caution_view(&self, now: Instant) -> Option<CautionView> {
        if let Phase::Test { strip_until, chime, .. } = self.phase {
            let c = self.conns.first();
            return (now < strip_until).then(|| CautionView {
                chime,
                place: c.and_then(|c| self.short_name(c)),
                what: format!("A wire at 9.9 A, above the {} rating", amps(self.limits.limits.rating)),
                action: "Ease the GPU load".into(),
                test: true,
            });
        }
        self.strip.as_ref().filter(|s| now < s.until).map(|s| s.view.clone())
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
            audio: self.audio_at(now),
            settings: self.settings.clone(),
            connecting: self.source.is_none().then(|| {
                if self.pending.is_some() { "MeltAlarm · connecting to the PSU…".into() } else { "MeltAlarm · looking for the PSU…".into() }
            }),
            notice: self.notice.clone(),
            caution: self.caution_view(now),
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
