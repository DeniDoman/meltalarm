//! The cable guard: MeltAlarm's own judge (docs/FUNCTIONAL_SPEC.md §6.3–6.4).
//!
//! Pure and sample-driven: nothing fires because a timer ran out, only when a fresh sample
//! arrives. One `Guard` per connector.

use std::time::{Duration, Instant};

use meltalarm_model::WIRES;

use crate::limits::{Limits, RELEASE_MARGIN, UNEVEN_MIN_AVG};

/// Samples further apart than this break every episode's continuity.
pub const GAP: Duration = Duration::from_secs(3);
/// A caution or uneven-load episode must last this long before the user is told.
pub const QUALIFY: Duration = Duration::from_secs(10);
/// A told episode ends after this long below its release level.
pub const RELEASE: Duration = Duration::from_secs(30);

/// Which overload rule raised the alarm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// One reading at or above the instant limit.
    Instant,
    /// The second reading at or above the fast limit in the episode.
    Fast,
    /// Above the alarm limit for the alarm delay.
    Sustained,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GuardEvent {
    OverloadStart { wire: usize, amps: f32, rule: Rule },
    OverloadEnd { lasted: Duration, peak: (usize, f32) },
    /// A wire above the rating qualified (caution).
    CautionStart { wire: usize, amps: f32 },
    CautionEnd { lasted: Duration, peak: (usize, f32) },
    /// Uneven load qualified (advisory). `low`: the wire carrying least; `high`: the most.
    UnevenStart { spread: f32, avg: f32, low: (usize, f32), high: f32 },
    UnevenEnd { lasted: Duration, peak_spread: f32 },
}

#[derive(Clone, Copy, Debug)]
struct WireEpisode {
    start: Instant,
    fast_readings: u32,
}

/// The connector's active overload: from the triggering reading until every wire is below
/// the rating again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overload {
    pub since: Instant,
    pub peak: (usize, f32),
}

#[derive(Clone, Copy, Debug, Default)]
struct Episode {
    start: Option<Instant>,
    told: bool,
    /// Last reading above the release level, and since when it has been below it.
    last_hot: Option<Instant>,
    cool_since: Option<Instant>,
    peak: (usize, f32),
}

enum Step {
    Qualified,
    Ended { lasted: Duration, peak: (usize, f32) },
}

impl Episode {
    /// `starts`: the condition itself; `hot`: above the release level (hysteresis).
    fn step(&mut self, starts: bool, hot: bool, at: Instant, value: (usize, f32)) -> Option<Step> {
        let Some(start) = self.start else {
            if starts {
                *self = Episode { start: Some(at), told: false, last_hot: Some(at), cool_since: None, peak: value };
            }
            return None;
        };
        if hot {
            self.cool_since = None;
            self.last_hot = Some(at);
            if value.1 > self.peak.1 {
                self.peak = value;
            }
            if !self.told && at.duration_since(start) >= QUALIFY {
                self.told = true;
                return Some(Step::Qualified);
            }
            None
        } else if !self.told {
            // Never told: a dip simply ends it; qualification restarts from scratch.
            *self = Episode::default();
            None
        } else {
            let cool = *self.cool_since.get_or_insert(at);
            if at.duration_since(cool) >= RELEASE {
                let lasted = self.last_hot.unwrap_or(start).duration_since(start);
                let peak = self.peak;
                *self = Episode::default();
                return Some(Step::Ended { lasted, peak });
            }
            None
        }
    }

    /// A gap: an untold episode restarts; a told one continues (it ends only on data).
    fn gap(&mut self) {
        if !self.told {
            *self = Episode::default();
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Guard {
    wires: [Option<WireEpisode>; WIRES],
    last: Option<Instant>,
    overload: Option<Overload>,
    caution: Episode,
    uneven: Episode,
}

impl Guard {
    pub fn overload(&self) -> Option<&Overload> {
        self.overload.as_ref()
    }

    /// The peak of a told caution episode, while it lasts.
    pub fn caution_peak(&self) -> Option<(usize, f32)> {
        self.caution.told.then_some(self.caution.peak)
    }

    /// A failed or missing sample, or NO DATA: continuity is broken. An active overload stays.
    pub fn gap(&mut self) {
        self.wires = [None; WIRES];
        self.last = None;
        self.caution.gap();
        self.uneven.gap();
    }

    /// One valid sample of this connector.
    pub fn sample(&mut self, w: &[Option<f32>; WIRES], at: Instant, l: &Limits) -> Vec<GuardEvent> {
        if self.last.is_none_or(|t| at.saturating_duration_since(t) > GAP) {
            self.gap();
        }
        self.last = Some(at);
        let mut events = Vec::new();

        // Overload (§6.3): per wire, episodes with hysteresis.
        let mut trigger: Option<(usize, f32, Rule)> = None;
        for (i, reading) in w.iter().enumerate() {
            let a = match *reading {
                Some(a) if a >= l.rating => a,
                _ => {
                    self.wires[i] = None; // below the rating, or unmeasured: never read as zero
                    continue;
                }
            };
            if self.wires[i].is_none() && a >= l.alarm {
                self.wires[i] = Some(WireEpisode { start: at, fast_readings: 0 });
            }
            let Some(ep) = &mut self.wires[i] else { continue };
            if a >= l.fast {
                ep.fast_readings += 1;
            }
            let rule = if a >= l.instant {
                Some(Rule::Instant)
            } else if a >= l.fast && ep.fast_readings >= 2 {
                Some(Rule::Fast)
            } else if a >= l.alarm && at.duration_since(ep.start) >= l.alarm_delay {
                Some(Rule::Sustained)
            } else {
                None
            };
            if let Some(r) = rule
                && trigger.is_none_or(|(_, b, _)| a > b)
            {
                trigger = Some((i, a, r));
            }
        }
        let present: Vec<(usize, f32)> = w.iter().enumerate().filter_map(|(i, a)| a.map(|a| (i, a))).collect();
        let max = present.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
        match &mut self.overload {
            None => {
                if let Some((wire, amps, rule)) = trigger {
                    self.overload = Some(Overload { since: at, peak: (wire, amps) });
                    events.push(GuardEvent::OverloadStart { wire, amps, rule });
                }
            }
            Some(o) => {
                if let Some(m) = max
                    && m.1 > o.peak.1
                {
                    o.peak = m;
                }
                let all_below = present.len() == WIRES && present.iter().all(|&(_, a)| a < l.rating);
                if all_below {
                    events.push(GuardEvent::OverloadEnd { lasted: at.duration_since(o.since), peak: o.peak });
                    self.overload = None;
                }
            }
        }

        // Caution (§6.4): a wire above the rating.
        if let Some(m) = max {
            match self.caution.step(m.1 >= l.rating, m.1 >= l.rating - RELEASE_MARGIN, at, m) {
                Some(Step::Qualified) => events.push(GuardEvent::CautionStart { wire: m.0, amps: m.1 }),
                Some(Step::Ended { lasted, peak }) => events.push(GuardEvent::CautionEnd { lasted, peak }),
                None => {}
            }
        }

        // Uneven load (§6.4): advisory only.
        if present.len() >= 2 {
            let low = present.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or_default();
            let high = max.map_or(0.0, |m| m.1);
            let spread = high - low.1;
            let avg = present.iter().map(|p| p.1).sum::<f32>() / present.len() as f32;
            let loaded = avg >= UNEVEN_MIN_AVG;
            let starts = loaded && spread >= l.uneven;
            let hot = loaded && spread >= l.uneven - RELEASE_MARGIN;
            match self.uneven.step(starts, hot, at, (low.0, spread)) {
                Some(Step::Qualified) => events.push(GuardEvent::UnevenStart { spread, avg, low, high }),
                Some(Step::Ended { lasted, peak }) => events.push(GuardEvent::UnevenEnd { lasted, peak_spread: peak.1 }),
                None => {}
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    //! Spec T22 and the research brief's acceptance checks.
    use super::*;

    const LOAD: [f32; 6] = [8.1, 7.8, 7.7, 7.8, 8.0, 8.1];

    struct Feed {
        g: Guard,
        t: Instant,
        events: Vec<(u64, GuardEvent)>,
        n: u64,
    }

    impl Feed {
        fn new() -> Feed {
            Feed { g: Guard::default(), t: Instant::now(), events: vec![], n: 0 }
        }
        fn step(&mut self, secs: u64, w: [f32; 6]) {
            self.n += secs;
            let at = self.t + Duration::from_secs(self.n);
            for e in self.g.sample(&w.map(Some), at, &Limits::V1) {
                self.events.push((self.n, e));
            }
        }
        fn run(&mut self, count: usize, w: [f32; 6]) {
            for _ in 0..count {
                self.step(1, w);
            }
        }
        fn pin1(a: f32) -> [f32; 6] {
            let mut w = LOAD;
            w[0] = a;
            w
        }
        fn overload_at(&self) -> Option<u64> {
            self.events.iter().find(|(_, e)| matches!(e, GuardEvent::OverloadStart { .. })).map(|(t, _)| *t)
        }
    }

    #[test]
    fn healthy_load_never_alarms_or_cautions() {
        let mut f = Feed::new();
        f.run(600, LOAD);
        assert!(f.events.is_empty());
    }

    #[test]
    fn a_single_11a_reading_does_not_alarm() {
        let mut f = Feed::new();
        f.run(5, LOAD);
        f.step(1, Feed::pin1(11.0));
        f.run(10, LOAD);
        assert_eq!(f.overload_at(), None);
    }

    #[test]
    fn two_12_2a_readings_on_one_wire_alarm_on_the_second() {
        let mut f = Feed::new();
        f.run(5, LOAD);
        f.step(1, Feed::pin1(12.2));
        assert_eq!(f.overload_at(), None);
        f.step(1, Feed::pin1(12.2));
        assert_eq!(f.overload_at(), Some(7));
        assert!(matches!(f.events[0].1, GuardEvent::OverloadStart { wire: 0, rule: Rule::Fast, .. }));
    }

    #[test]
    fn one_16_5a_reading_alarms_at_once() {
        let mut f = Feed::new();
        f.run(5, LOAD);
        f.step(1, Feed::pin1(16.5));
        assert_eq!(f.overload_at(), Some(6));
    }

    #[test]
    fn constant_10_7a_alarms_on_the_first_reading_4s_after_the_first() {
        let mut f = Feed::new();
        f.run(5, LOAD);
        f.run(10, Feed::pin1(10.7)); // first at t=6
        assert_eq!(f.overload_at(), Some(10));
    }

    #[test]
    fn noise_around_the_alarm_limit_is_one_episode() {
        let mut f = Feed::new();
        f.run(5, LOAD);
        for i in 0..10 {
            f.step(1, Feed::pin1(if i % 2 == 0 { 10.6 } else { 10.4 }));
        }
        // 10.6 at t=6, 8, 10: the episode (hysteresis down to 9.5) is 4 s old at t=10.
        assert_eq!(f.overload_at(), Some(10));
    }

    #[test]
    fn high_readings_alternating_between_wires_do_not_confirm_each_other() {
        let mut f = Feed::new();
        f.run(5, LOAD);
        for i in 0..20 {
            let mut w = LOAD;
            w[i % 2] = 12.5; // the other one is back at 8 A: its episode ends
            f.step(1, w);
        }
        assert_eq!(f.overload_at(), None);
    }

    #[test]
    fn a_gap_restarts_qualification_but_never_clears_an_active_overload() {
        let mut f = Feed::new();
        f.run(5, LOAD);
        f.run(3, Feed::pin1(10.7));
        f.step(4, Feed::pin1(10.7)); // gap > 3 s
        f.run(3, Feed::pin1(10.7));
        assert_eq!(f.overload_at(), None, "qualification restarted after the gap");
        f.step(1, Feed::pin1(10.7));
        assert!(f.overload_at().is_some());
        f.step(10, Feed::pin1(10.7)); // a long gap during the overload
        assert!(f.g.overload().is_some());
        f.step(1, Feed::pin1(9.6)); // above the rating: not over
        assert!(f.g.overload().is_some());
        f.step(1, LOAD);
        assert!(f.g.overload().is_none());
        assert!(f.events.iter().any(|(_, e)| matches!(e, GuardEvent::OverloadEnd { .. })));
    }

    #[test]
    fn an_unmeasured_wire_never_confirms_the_all_clear() {
        let mut f = Feed::new();
        f.step(1, Feed::pin1(16.0));
        assert!(f.g.overload().is_some());
        let at = f.t + Duration::from_secs(2);
        let mut w = LOAD.map(Some);
        w[0] = None;
        f.g.sample(&w, at, &Limits::V1);
        assert!(f.g.overload().is_some());
    }

    #[test]
    fn caution_is_told_once_after_10s_and_ends_after_30s_below_the_release_level() {
        let mut f = Feed::new();
        f.run(5, LOAD);
        f.run(9, Feed::pin1(9.9)); // t=6..14
        f.step(1, Feed::pin1(9.2)); // t=15: still above 9.0, continuity kept
        f.step(1, Feed::pin1(9.9)); // t=16: 10 s after the start at t=6
        let told: Vec<u64> = f.events.iter().filter(|(_, e)| matches!(e, GuardEvent::CautionStart { .. })).map(|(t, _)| *t).collect();
        assert_eq!(told, [16]);
        // Coming and going within 30 s stays one episode.
        f.run(20, LOAD);
        f.run(5, Feed::pin1(9.9));
        f.run(30, LOAD);
        assert!(!f.events.iter().any(|(_, e)| matches!(e, GuardEvent::CautionEnd { .. })));
        f.step(1, LOAD);
        assert!(f.events.iter().any(|(_, e)| matches!(e, GuardEvent::CautionEnd { .. })));
        assert_eq!(f.events.iter().filter(|(_, e)| matches!(e, GuardEvent::CautionStart { .. })).count(), 1);
    }

    #[test]
    fn a_dip_below_the_release_level_restarts_an_untold_caution() {
        let mut f = Feed::new();
        f.run(8, Feed::pin1(9.9));
        f.step(1, LOAD);
        f.run(10, Feed::pin1(9.9));
        assert!(f.events.is_empty());
        f.step(1, Feed::pin1(9.9));
        assert_eq!(f.events.len(), 1);
    }

    #[test]
    fn uneven_load_is_told_after_10s_but_not_at_idle() {
        let mut f = Feed::new();
        f.run(30, [0.1, 0.1, 0.1, 0.1, 0.1, 4.0]); // spread 3.9 A at idle-ish average: ignored
        assert!(f.events.is_empty());
        f.run(11, [0.5, 8.0, 8.0, 8.0, 8.0, 8.0]);
        assert!(matches!(f.events.as_slice(), [(_, GuardEvent::UnevenStart { low: (0, _), .. })]));
    }
}
