//! The alert ladder below the alarm (Spec §8.8–8.10): the caution strip, advisory notices, and
//! the cable note every episode leaves behind. Also turns the cable guard's events into log
//! lines.

use std::time::Instant;

use meltalarm_model::ConnectorKey;

use crate::alarm::Phase;
use crate::guard::{self, GuardEvent, Rule};
use crate::log::LogEvent;
use crate::state::{CableNote, Severity};
use crate::text::{amps, note_time, on, wires_text};
use crate::view::{CautionView, Notice};
use crate::{Core, Output, STRIP_SHOW};

/// The episode whose peak keeps the connector's cable note up to date while it lasts.
#[derive(Clone, Debug)]
pub(crate) enum LiveNote {
    Overload { when: String },
    Caution { when: String },
}

pub(crate) struct Strip {
    pub view: CautionView,
    pub until: Instant,
}

impl Core {
    pub(crate) fn push_notice(&mut self, title: &str, text: String, warning: bool, connector: Option<ConnectorKey>) {
        self.notice_seq += 1;
        self.notice = Some(Notice { id: self.notice_seq, title: title.into(), text, warning, connector });
    }

    /// Turn a cable-guard event into log lines, notes, the strip or a notice (Spec §8).
    pub(crate) fn on_guard(&mut self, ci: usize, e: GuardEvent, now: Instant, out: &mut Output) {
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
                let imbalance = self.conns[ci].eval.imbalance.map(|s| format!(" | imbalance {}", amps(s))).unwrap_or_default();
                out.log.push(LogEvent::Overload {
                    conn: label,
                    detail: format!("wire {} = {} · {rule} | wires {wires}{imbalance}", wire + 1, amps(a)),
                });
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
                    detail: format!(
                        "wire {} = {} above the {} rating for {} s | wires {wires}",
                        wire + 1,
                        amps(a),
                        amps(l.rating),
                        guard::QUALIFY.as_secs()
                    ),
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
            GuardEvent::UnevenStart { imbalance, avg, low, high } => {
                out.log.push(LogEvent::Uneven {
                    conn: label.clone(),
                    detail: format!(
                        "imbalance {} at {} average for {} s | wires {wires}",
                        amps(imbalance),
                        amps(avg),
                        guard::QUALIFY.as_secs()
                    ),
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
            GuardEvent::UnevenEnd { lasted, peak_imbalance } => {
                out.log.push(LogEvent::UnevenEnd { conn: label, lasted, detail: format!("peak imbalance {}", amps(peak_imbalance)) });
            }
        }
    }

    /// Keep the note of a running overload or caution episode at its peak.
    pub(crate) fn refresh_live_note(&mut self, ci: usize) {
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
    pub(crate) fn set_note(&mut self, ci: usize, severity: Severity, text: String) {
        let key = self.conns[ci].key.clone();
        if self.state.notes.get(&key).is_some_and(|n| n.severity > severity || (n.severity == severity && n.text == text)) {
            return;
        }
        self.state.notes.insert(key, CableNote { severity, text });
        self.state_dirty = true;
    }

    pub(crate) fn when(&self) -> String {
        note_time(&(self.wall)())
    }

    /// Show the caution strip (Spec §8.8), unless alerts are off or an alarm is active. A new
    /// caution while one is visible replaces its text without a second chime.
    pub(crate) fn raise_caution(&mut self, place: Option<String>, what: String, action: String, now: Instant) {
        if !self.settings.alarm_enabled || !self.active_set().is_empty() {
            return;
        }
        let visible = self.strip.as_ref().is_some_and(|s| s.until > now);
        if !visible {
            self.chime_seq += 1;
        }
        self.strip = Some(Strip { view: CautionView { chime: self.chime_seq, place, what, action, test: false }, until: now + STRIP_SHOW });
    }

    pub(crate) fn caution_view(&self, now: Instant) -> Option<CautionView> {
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
}
