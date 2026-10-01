//! MeltAlarm's own cable limits (docs/FUNCTIONAL_SPEC.md §6.1): physics of the 12V-2x6
//! connector, the same for every PSU and GPU. Versioned defaults, file overrides only.

use std::time::Duration;

/// Version of the default set, logged at start so reports say which defaults were active.
pub const VERSION: &str = "v1";

/// Uneven load counts only while the average wire carries at least this much (idle noise).
pub const UNEVEN_MIN_AVG: f32 = 3.0;
/// Hysteresis for ending caution and uneven-load episodes.
pub const RELEASE_MARGIN: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Contact rating: caution at or above; an overload episode ends below it.
    pub rating: f32,
    /// An overload episode starts here; the alarm follows after `alarm_delay`.
    pub alarm: f32,
    pub alarm_delay: Duration,
    /// Alarm on the second reading at or above it within an episode.
    pub fast: f32,
    /// Alarm on one reading at or above it.
    pub instant: f32,
    /// Imbalance (max − min) that counts as uneven load.
    pub uneven: f32,
}

impl Limits {
    pub const V1: Limits =
        Limits { rating: 9.5, alarm: 10.5, alarm_delay: Duration::from_secs(4), fast: 12.0, instant: 15.0, uneven: 3.0 };
}

impl Default for Limits {
    fn default() -> Self {
        Limits::V1
    }
}

/// Values from `settings.toml`; `None` = the default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LimitOverrides {
    pub rating: Option<f32>,
    pub alarm: Option<f32>,
    pub alarm_seconds: Option<f32>,
    pub fast: Option<f32>,
    pub instant: Option<f32>,
    pub uneven: Option<f32>,
}

/// The limits in effect, and what to say about them.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub limits: Limits,
    pub custom: bool,
    /// The overrides were rejected (the defaults apply).
    pub warning: Option<String>,
}

impl LimitOverrides {
    pub fn is_empty(&self) -> bool {
        *self == LimitOverrides::default()
    }

    /// All or nothing (Spec §6.1): an override set that breaks the order or leaves 5–30 A
    /// is ignored as a whole.
    pub fn resolve(&self) -> Resolved {
        if self.is_empty() {
            return Resolved { limits: Limits::V1, custom: false, warning: None };
        }
        let d = Limits::V1;
        let l = Limits {
            rating: self.rating.unwrap_or(d.rating),
            alarm: self.alarm.unwrap_or(d.alarm),
            alarm_delay: self.alarm_seconds.map_or(d.alarm_delay, |s| Duration::from_secs_f32(s.clamp(0.0, 600.0))),
            fast: self.fast.unwrap_or(d.fast),
            instant: self.instant.unwrap_or(d.instant),
            uneven: self.uneven.unwrap_or(d.uneven),
        };
        let amps_ok = [l.rating, l.alarm, l.fast, l.instant].iter().all(|a| (5.0..=30.0).contains(a));
        let ordered = l.rating < l.alarm && l.alarm < l.fast && l.fast < l.instant;
        let uneven_ok = (0.5..=30.0).contains(&l.uneven);
        let seconds_ok = self.alarm_seconds.is_none_or(|s| (0.0..=60.0).contains(&s));
        if amps_ok && ordered && uneven_ok && seconds_ok {
            Resolved { limits: l, custom: true, warning: None }
        } else {
            Resolved {
                limits: Limits::V1,
                custom: false,
                warning: Some("custom cable limits ignored (need rating < alarm < fast < instant, 5–30 A); using the defaults".into()),
            }
        }
    }
}

fn a(x: f32) -> String {
    if x.fract() == 0.0 { format!("{x:.1} A") } else { format!("{x} A") }
}

impl Resolved {
    /// The LIMITS log line (Spec §9).
    pub fn summary(&self) -> String {
        let l = &self.limits;
        format!(
            "{}{} · rating {} · alarm {} for {} s, {} twice, {} once · uneven {}",
            VERSION,
            if self.custom { " custom" } else { "" },
            a(l.rating),
            a(l.alarm),
            l.alarm_delay.as_secs_f32(),
            a(l.fast),
            a(l.instant),
            a(l.uneven)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_all_or_nothing_overrides() {
        let r = LimitOverrides::default().resolve();
        assert_eq!((r.limits, r.custom, r.warning.is_none()), (Limits::V1, false, true));
        assert_eq!(r.summary(), "v1 · rating 9.5 A · alarm 10.5 A for 4 s, 12.0 A twice, 15.0 A once · uneven 3.0 A");

        let r = LimitOverrides { alarm: Some(11.0), ..Default::default() }.resolve();
        assert_eq!((r.limits.alarm, r.custom), (11.0, true));
        assert!(r.summary().starts_with("v1 custom"));

        // Breaking the order rejects the whole set, not just the bad value.
        let r = LimitOverrides { alarm: Some(11.0), fast: Some(10.0), ..Default::default() }.resolve();
        assert_eq!((r.limits, r.custom), (Limits::V1, false));
        assert!(r.warning.is_some());
        let r = LimitOverrides { instant: Some(40.0), ..Default::default() }.resolve();
        assert!(r.warning.is_some());
    }
}
