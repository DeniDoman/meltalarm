//! Formatting shared by MeltAlarm's texts (Spec §7.0, §9): amps, durations, dates, names, the
//! device's verdict in words. The sentences themselves live next to the rule they explain.

use std::time::Duration;

use meltalarm_model::{DeviceStatus, Fault, Protection, WIRES};

/// The name in the log and wherever a number is needed: "GPU power cable 1" (Spec §7.0).
pub(crate) fn cable_label(index: u8) -> String {
    format!("GPU power cable {}", index + 1)
}

pub(crate) fn wires_text(w: &Option<[Option<f32>; WIRES]>) -> String {
    match w {
        Some(w) => w.iter().map(|a| a.map_or("-".into(), |a| format!("{a:.1}"))).collect::<Vec<_>>().join(" "),
        None => "unavailable".into(),
    }
}

/// `2026-09-30 18:02:11` → `Sep 30, 18:02`; anything else → empty (the note then has no time).
pub(crate) fn note_time(wall: &str) -> String {
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
pub(crate) fn on(when: &str) -> String {
    if when.is_empty() { String::new() } else { format!(" on {when}") }
}

pub(crate) fn amps(a: f32) -> String {
    format!("{a:.1} A")
}

pub(crate) fn mmss(d: Duration) -> String {
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
pub(crate) fn short_status_name(s: DeviceStatus) -> String {
    match s {
        DeviceStatus::Normal => "Normal".into(),
        DeviceStatus::OverCurrent => "Over-current".into(),
        DeviceStatus::Imbalance => "Imbalance".into(),
        DeviceStatus::CriticalOverCurrent => "Critical".into(),
        DeviceStatus::Unknown(n) => format!("PSU alert 0x{n:02X}"),
    }
}

pub(crate) fn trim(a: f32) -> String {
    if a.fract() == 0.0 { format!("{a:.0} A") } else { amps(a) }
}

/// The CONFIG log line and the Settings info: `Safeguard+ ON · OCP 12.0 A for 20 s · …`. `name`: the
/// device's name for its protection.
pub(crate) fn protection_summary(p: &Protection, name: &str) -> String {
    if !p.enabled {
        return format!("{name} OFF");
    }
    let mut parts = vec![format!("{name} ON")];
    if let Some(w) = p.wire_limit {
        parts.push(format!("OCP {}{}", amps(w), p.wire_trigger.map(|t| format!(" for {}", crate::log::secs(t))).unwrap_or_default()));
    }
    if let Some(s) = p.imbalance_limit {
        parts.push(format!("imbalance {}{}", amps(s), p.imbalance_trigger.map(|t| format!(" for {}", crate::log::secs(t))).unwrap_or_default()));
    }
    if let Some(c) = p.cutoff_after {
        parts.push(format!("power cut {} after alarm", crate::log::secs(c)));
    }
    parts.join(" · ")
}

/// A device fault in words, for the log, the flyout and the caution strip.
pub(crate) fn fault_name(f: &Fault) -> String {
    match f {
        Fault::OverTemperature => "OTP (PSU over-temperature)".into(),
        Fault::FanFailure => "Fan failure".into(),
        Fault::OverPower => "OPP (PSU over-power)".into(),
        Fault::RailOverCurrent(rail) => format!("OCP on the {rail} rail"),
        Fault::Other(s) => s.clone(),
    }
}
