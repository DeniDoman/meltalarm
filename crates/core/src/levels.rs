//! Live levels (docs/FUNCTIONAL_SPEC.md §6.2): colors only, instant, no debounce. A level
//! alone never interrupts the user; the cable guard (§6.3–6.4) decides that.

use meltalarm_model::WIRES;

use crate::limits::{Limits, UNEVEN_MIN_AVG};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    #[default]
    Normal,
    Caution,
    Warning,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Eval {
    pub wires: [Level; WIRES],
    pub imbalance: Option<f32>,
    pub imbalance_level: Level,
    pub max: Option<(usize, f32)>,
    pub min: Option<(usize, f32)>,
    /// Mean of the measured wires.
    pub avg: Option<f32>,
}

impl Eval {
    /// The highest live level of any wire or the imbalance.
    pub fn worst(&self) -> Level {
        self.wires.iter().copied().chain([self.imbalance_level]).max().unwrap_or_default()
    }
}

fn median(sorted: &[f32]) -> f32 {
    let n = sorted.len();
    if n % 2 == 1 { sorted[n / 2] } else { (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0 }
}

/// Evaluate one connector's wires against MeltAlarm's cable limits. `device_imbalance`: the
/// device reports its own imbalance status for this connector (the imbalance turns red).
pub fn evaluate(wires: &[Option<f32>; WIRES], l: &Limits, device_imbalance: bool) -> Eval {
    let present: Vec<(usize, f32)> = wires.iter().enumerate().filter_map(|(i, w)| w.map(|a| (i, a))).collect();
    let mut e = Eval::default();
    if present.is_empty() {
        return e;
    }
    e.avg = Some(present.iter().map(|(_, a)| a).sum::<f32>() / present.len() as f32);
    e.max = present.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
    e.min = present.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1));
    if present.len() >= 2 {
        e.imbalance = Some(e.max.unwrap().1 - e.min.unwrap().1);
    }

    for &(i, a) in &present {
        e.wires[i] = if a >= l.alarm {
            Level::Warning
        } else if a >= l.rating {
            Level::Caution
        } else {
            Level::Normal
        };
    }

    if let Some(imbalance) = e.imbalance {
        let uneven = imbalance >= l.uneven && e.avg.is_some_and(|a| a >= UNEVEN_MIN_AVG);
        e.imbalance_level = if device_imbalance {
            Level::Warning
        } else if uneven {
            Level::Caution
        } else {
            Level::Normal
        };
        if e.imbalance_level > Level::Normal {
            // Attribute the imbalance to the outlier(s): |I − median| ≥ T/2. At least one wire
            // always qualifies because the median lies between min and max.
            let mut sorted: Vec<f32> = present.iter().map(|(_, a)| *a).collect();
            sorted.sort_by(f32::total_cmp);
            let med = median(&sorted);
            let threshold = if uneven { l.uneven / 2.0 } else { imbalance / 2.0 };
            for &(i, a) in &present {
                if (a - med).abs() >= threshold {
                    e.wires[i] = e.wires[i].max(e.imbalance_level);
                }
            }
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(v: [f32; 6]) -> [Option<f32>; 6] {
        v.map(Some)
    }
    use Level::*;
    const L: Limits = Limits::V1;

    #[test]
    fn healthy_full_load_is_all_normal() {
        // Measured on the reference system at ~575 W (F12).
        let e = evaluate(&w([8.1, 7.8, 7.7, 7.8, 8.0, 8.56]), &L, false);
        assert_eq!(e.wires, [Normal; 6]);
        assert_eq!(e.imbalance_level, Normal);
        assert_eq!(e.worst(), Normal);
    }

    #[test]
    fn wire_levels_follow_the_cable_limits_inclusively() {
        let e = evaluate(&w([9.5, 10.5, 9.49, 9.0, 9.0, 9.0]), &L, false);
        assert_eq!(&e.wires[..3], &[Caution, Warning, Normal]);
    }

    #[test]
    fn uneven_load_attributes_to_the_outlier_only() {
        // imbalance 3.0 at 6.5 A average → caution on the low wire only.
        let e = evaluate(&w([7.0, 7.0, 4.0, 7.0, 7.0, 7.0]), &L, false);
        assert_eq!(e.imbalance_level, Caution);
        assert_eq!(e.wires, [Normal, Normal, Caution, Normal, Normal, Normal]);
    }

    #[test]
    fn uneven_needs_load_idle_noise_is_normal() {
        let e = evaluate(&w([0.1, 0.1, 0.1, 0.1, 0.1, 3.5]), &L, false);
        assert_eq!(e.imbalance_level, Normal);
    }

    #[test]
    fn the_device_imbalance_status_turns_the_imbalance_and_the_outlier_red() {
        let e = evaluate(&w([9.9, 9.8, 2.1, 9.7, 9.8, 9.9]), &L, true);
        assert_eq!(e.imbalance_level, Warning);
        assert_eq!(e.wires, [Caution, Caution, Warning, Caution, Caution, Caution]);
    }

    #[test]
    fn missing_wires_are_ignored() {
        let e = evaluate(&[Some(1.0), None, None, None, None, None], &L, false);
        assert_eq!(e.imbalance, None);
        assert_eq!(e.avg, Some(1.0));
    }
}
