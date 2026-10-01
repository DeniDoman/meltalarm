//! The declarative view (Spec §7): everything a frontend needs to render, including all
//! user-facing text (docs/ARCHITECTURE.md D4, D8). Frontends only render and reconcile this.

use std::time::{Duration, Instant};

use meltalarm_model::{ConnectorKey, WIRES};

use crate::levels::Level;
use crate::settings::Settings;
use crate::text::{amps, protection_summary, short_status_name, status_name};
use crate::{Conn, Core, STALE_AFTER};

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

/// The knee scale of the cut bars (DESIGN.md "Cut bars"), the same for every frontend: the share
/// of a bar's height for `amps`. The bottom 20 % covers 0 A to the knee (6 A, or 60 % of the
/// limit if lower), the rest the decision range up to the alarm limit at the top.
pub fn bar_share(amps: f32, limit: f32) -> f32 {
    let knee = 6.0_f32.min(0.6 * limit);
    let a = amps.clamp(0.0, limit);
    if a <= knee { a / knee * 0.2 } else { 0.2 + (a - knee) / (limit - knee) * 0.8 }
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
    /// The cut bars (DESIGN.md "Cut bars"): the top is the alarm limit, the cut is the rating.
    pub bar_limit: Option<f32>,
    pub bar_rating: Option<f32>,
    pub glyph: Glyph,
    /// Amber marker: something to read in the flyout (a cable note, a PSU fault, the PSU's protection off).
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

    pub(crate) fn conn_view(&self, c: &Conn, now: Instant) -> ConnectorView {
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
            notes.push(Note { kind: NoteKind::Caution, text: format!("PSU {} is OFF: the PSU won't cut power. MeltAlarm still alarms.", self.protection_name()) });
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
            if c.eval.imbalance_level > Level::Normal
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
            _ if safeguard_off => format!("{} off", self.protection_name()),
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
            if let (Some((_, max)), Some(imbalance)) = (c.eval.max, c.eval.imbalance) {
                t.push_str(&format!(" · max {max:.1}A · Δ {imbalance:.1}A"));
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
        } else if c.eval.imbalance_level > Level::Normal {
            (format!("Imbalance {}", amps(c.eval.imbalance.unwrap_or(0.0))), Level::Caution)
        } else if cable_note.is_some() {
            ("Check the cable".to_owned(), Level::Caution)
        } else if let Some(s) = c.eval.imbalance {
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
            imbalance: c.eval.imbalance,
            imbalance_level: c.eval.imbalance_level,
            bar_limit: Some(l.alarm),
            bar_rating: Some(l.rating),
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
            protection: self.protection.as_ref().map_or_else(|| "Reading…".into(), |p| protection_summary(p, self.protection_name())),
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
