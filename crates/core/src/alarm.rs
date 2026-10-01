//! The alarm (Spec §8.1–8.6): either judge raises it, per (connector, cause); snooze,
//! escalation, the green clear, the test alarm; the notch's content and the sound script.

use std::time::{Duration, Instant};

use meltalarm_model::{DeviceStatus, WIRES};

use crate::levels::Level;
use crate::text::{amps, mmss, status_name, trim};
use crate::view::{AlarmKind, AlarmView, AudioScript, AudioStep, Health, WireView};
use crate::{CLEARED_SHOW, Conn, Core};

/// Why a connector is in alarm. Each (connector, cause) pair is tracked separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    /// MeltAlarm's overload rule (Spec §6.3).
    Overload,
    /// The device's own status (Spec §6.5).
    Device(DeviceStatus),
}

/// What the alarm does now: the notch and the sound follow from it.
pub(crate) enum Phase {
    Idle,
    Active,
    Snoozed { until: Instant, snapshot: Vec<(u8, Cause)> },
    Cleared { until: Instant, lasted: Duration, reason: String, label: String },
    /// The strip part runs until `strip_until`, then the alarm part (Spec §8.5).
    Test { until: Instant, strip_until: Instant, chime: u32 },
}

impl Core {
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

    pub(crate) fn update_phase(&mut self, now: Instant) {
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
    pub(crate) fn update_alarm_open(&mut self) {
        let open = self.active_set().first().and_then(|(i, _)| self.conns.iter().find(|c| c.key.index == *i)).map(|c| c.key.clone());
        if open.is_some() != self.state.alarm_open.is_some() {
            self.state.alarm_open = open;
            self.state_dirty = true;
        }
    }

    pub(crate) fn countdown(&self, c: &Conn, now: Instant) -> Option<String> {
        let status = c.device_alarm()?;
        if status == DeviceStatus::CriticalOverCurrent {
            return Some("any second".into());
        }
        let cutoff = self.protection.as_ref()?.cutoff_after?;
        let since = c.alarm_since?;
        let left = cutoff.saturating_sub(now.duration_since(since));
        Some(if left.is_zero() { "expected now".into() } else { format!("~{}", mmss(left)) })
    }

    pub(crate) fn alarm_view(&self, now: Instant) -> Option<AlarmView> {
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

    pub(crate) fn alarm_for(&self, c: &Conn, now: Instant, test: bool) -> AlarmView {
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
                let hard = p.and_then(|p| p.hard_wire_limit).map(|l| format!(" Hard PSU limit {}.", trim(l))).unwrap_or_default();
                format!("Highest wire {}.{hard}", amps(a))
            } else if s == DeviceStatus::Imbalance {
                let (lo_i, lo) = c.eval.min.unwrap_or((0, 0.0));
                let others: Vec<f32> = cv.wires.iter().enumerate().filter(|(i, _)| *i != lo_i).filter_map(|(_, w)| w.amps).collect();
                let (omin, omax) = others.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
                let limit = p.and_then(|p| p.imbalance_limit).map(|l| format!(", PSU limit {}", amps(l))).unwrap_or_default();
                if others.is_empty() {
                    "Wire readings unavailable.".into()
                } else {
                    format!("Lowest wire {}, the others {:.1}–{:.1} A. Imbalance {}{limit}.", amps(lo), omin, omax, amps(c.eval.imbalance.unwrap_or(0.0)))
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
                match p.and_then(|p| p.hard_wire_limit) {
                    Some(l) => format!("A wire is above the PSU's hard {} limit. Power can be cut at any moment.", trim(l)),
                    None => "A wire is above the PSU's hard limit. Power can be cut at any moment.".into(),
                }
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
            bar_rating: cv.bar_rating,
            note: data_lost.then(|| "PSU data lost. MeltAlarm cannot confirm the problem is gone, so the alarm stays.".into()),
            snooze: true,
            cleared_progress: None,
        }
    }

    pub(crate) fn audio_at(&self, now: Instant) -> Option<AudioScript> {
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
}

/// The test alarm's sample bars: a bad contact on one wire.
fn test_bars() -> [WireView; WIRES] {
    let v = [9.9, 9.8, 2.1, 9.7, 9.8, 9.9];
    std::array::from_fn(|i| WireView {
        amps: Some(v[i]),
        level: if i == 2 { Level::Warning } else { Level::Caution },
        flagged: i == 2,
    })
}
