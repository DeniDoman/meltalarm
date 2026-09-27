//! Local visual levels (docs/FUNCTIONAL_SPEC.md §6.1): instant, no debounce, never alarms.

use meltalarm_model::{Protection, WIRES};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    #[default]
    Normal,
    Caution,
    Warning,
}

pub const CAUTION_WIRE_RATIO: f32 = 0.8;
pub const CAUTION_SPREAD_RATIO: f32 = 0.5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Eval {
    pub wires: [Level; WIRES],
    pub spread: Option<f32>,
    pub spread_level: Level,
    pub total: Option<f32>,
    pub max: Option<(usize, f32)>,
    pub min: Option<(usize, f32)>,
    /// Our reading says a wire or the spread is at/over the device limit.
    pub wire_red: bool,
    pub spread_red: bool,
}

impl Eval {
    pub fn is_red(&self) -> bool {
        self.wire_red || self.spread_red
    }
}

fn median(sorted: &[f32]) -> f32 {
    let n = sorted.len();
    if n % 2 == 1 { sorted[n / 2] } else { (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0 }
}

/// Evaluate one connector's wires against the device's limits. A limit the device does
/// not report produces no color for that metric (never a guessed threshold).
pub fn evaluate(wires: &[Option<f32>; WIRES], p: Option<&Protection>) -> Eval {
    let present: Vec<(usize, f32)> = wires.iter().enumerate().filter_map(|(i, w)| w.map(|a| (i, a))).collect();
    let mut e = Eval::default();
    if present.is_empty() {
        return e;
    }
    e.total = Some(present.iter().map(|(_, a)| a).sum());
    e.max = present.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
    e.min = present.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1));
    if present.len() >= 2 {
        e.spread = Some(e.max.unwrap().1 - e.min.unwrap().1);
    }
    let wire_limit = p.and_then(|p| p.wire_limit);
    let spread_limit = p.and_then(|p| p.spread_limit);

    if let Some(limit) = wire_limit {
        for &(i, a) in &present {
            e.wires[i] = if a >= limit {
                Level::Warning
            } else if a >= CAUTION_WIRE_RATIO * limit {
                Level::Caution
            } else {
                Level::Normal
            };
        }
        e.wire_red = e.wires.contains(&Level::Warning);
    }

    if let (Some(limit), Some(spread)) = (spread_limit, e.spread) {
        let (level, threshold) = if spread >= limit {
            (Level::Warning, limit)
        } else if spread >= CAUTION_SPREAD_RATIO * limit {
            (Level::Caution, CAUTION_SPREAD_RATIO * limit)
        } else {
            (Level::Normal, 0.0)
        };
        e.spread_level = level;
        e.spread_red = level == Level::Warning;
        if level > Level::Normal {
            // Attribute the spread to the outlier(s): |I − median| ≥ T/2. At least one wire
            // always qualifies because the median lies between min and max.
            let mut sorted: Vec<f32> = present.iter().map(|(_, a)| *a).collect();
            sorted.sort_by(f32::total_cmp);
            let med = median(&sorted);
            for &(i, a) in &present {
                if (a - med).abs() >= threshold / 2.0 {
                    e.wires[i] = e.wires[i].max(level);
                }
            }
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    pub fn prot(ocp: f32, diff: f32) -> Protection {
        Protection {
            enabled: true,
            wire_limit: Some(ocp),
            spread_limit: Some(diff),
            wire_trigger: Some(Duration::from_secs(20)),
            spread_trigger: Some(Duration::from_secs(20)),
            cutoff_after: Some(Duration::from_secs(180)),
            hard_wire_limit: Some(18.0),
        }
    }
    fn w(v: [f32; 6]) -> [Option<f32>; 6] {
        v.map(Some)
    }
    use Level::*;

    #[test]
    fn healthy_full_load_is_all_normal() {
        // Measured on the reference system at ~575 W (F12).
        let e = evaluate(&w([8.1, 7.8, 7.7, 7.8, 8.0, 8.1]), Some(&prot(12.0, 5.5)));
        assert_eq!(e.wires, [Normal; 6]);
        assert_eq!(e.spread_level, Normal);
        assert!(!e.is_red());
    }

    #[test]
    fn wire_thresholds_are_inclusive() {
        let e = evaluate(&w([9.6, 12.0, 9.59, 1.0, 1.0, 1.0]), Some(&prot(12.0, 50.0)));
        assert_eq!(&e.wires[..3], &[Caution, Warning, Normal]);
        assert!(e.wire_red);
    }

    #[test]
    fn bad_contact_lights_the_outlier_red_and_loaded_wires_amber() {
        let e = evaluate(&w([9.9, 9.8, 2.1, 9.7, 9.8, 9.9]), Some(&prot(12.0, 5.5)));
        assert_eq!(e.wires, [Caution, Caution, Warning, Caution, Caution, Caution]);
        assert_eq!(e.spread_level, Warning);
        assert!(e.spread_red && !e.wire_red);
    }

    #[test]
    fn spread_caution_attributes_to_outlier_only() {
        // spread 3.0 ≥ 2.75 (50 % of 5.5) → caution on the low wire only.
        let e = evaluate(&w([7.0, 7.0, 4.0, 7.0, 7.0, 7.0]), Some(&prot(12.0, 5.5)));
        assert_eq!(e.spread_level, Caution);
        assert_eq!(e.wires, [Normal, Normal, Caution, Normal, Normal, Normal]);
    }

    #[test]
    fn follows_non_default_device_limits() {
        let e = evaluate(&w([8.5, 8.0, 8.0, 8.0, 8.0, 8.0]), Some(&prot(10.0, 4.0)));
        assert_eq!(e.wires[0], Caution); // 8.5 ≥ 0.8 × 10
    }

    #[test]
    fn no_limits_no_colors_and_missing_wires_ignored() {
        let e = evaluate(&w([30.0, 0.0, 0.0, 0.0, 0.0, 0.0]), None);
        assert_eq!(e.wires, [Normal; 6]);
        let e = evaluate(&[Some(1.0), None, None, None, None, None], Some(&prot(12.0, 5.5)));
        assert_eq!(e.spread, None);
        assert_eq!(e.total, Some(1.0));
    }
}
