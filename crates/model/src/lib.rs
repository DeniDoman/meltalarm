//! Vendor-neutral vocabulary shared by sources, the core logic and frontends.
//!
//! A *source* is anything that measures per-wire current on 12V-2x6 connectors
//! (today: MSI MPG Ai1x00TS PSUs). Every value a device may not provide is an
//! `Option`: sources declare what they lack instead of faking it.
#![forbid(unsafe_code)]

use std::fmt;
use std::time::{Duration, Instant};

/// Number of +12 V power wires in a 12V-2x6 connector.
pub const WIRES: usize = 6;

/// Stable identifier of a source, e.g. `msi:ai1300ts`. Keys settings and logs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub String);

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A connector of a specific source.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectorKey {
    pub source: SourceId,
    pub index: u8,
}

impl fmt::Display for ConnectorKey {
    /// Settings-file form: `msi:ai1300ts:1` (1-based, like the labels).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.source, self.index + 1)
    }
}

impl ConnectorKey {
    pub fn parse(s: &str) -> Option<Self> {
        let (source, n) = s.rsplit_once(':')?;
        let n: u8 = n.parse().ok()?;
        (n >= 1 && !source.is_empty()).then(|| ConnectorKey { source: SourceId(source.to_owned()), index: n - 1 })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    pub id: SourceId,
    pub vendor: String,
    pub model: String,
    pub firmware: Option<String>,
    pub serial: Option<String>,
    pub connectors: Vec<ConnectorInfo>,
    pub caps: Capabilities,
    /// The device's own name for its protection feature, e.g. `Safeguard+` (MSI). MeltAlarm's
    /// texts never name a vendor feature themselves.
    pub protection_name: Option<String>,
}

/// A connector the source measures. MeltAlarm names connectors itself ("GPU power cable 1"),
/// numbered by `index` as on the device.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorInfo {
    pub index: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// The device reports its own Normal/alarm status per connector.
    pub device_verdict: bool,
    /// The device reports the thresholds it enforces.
    pub device_limits: bool,
    /// The device cuts power a known time after raising an alarm.
    pub cutoff_timer: bool,
    /// The device flags which wire tripped.
    pub wire_flags: bool,
}

/// The result of one poll (one tick).
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub at: Instant,
    /// `None`: currents unavailable this tick. Never report zeros instead.
    pub readings: Option<Vec<ConnectorReading>>,
    /// `None`: status unavailable this tick. Never treated as Normal.
    pub verdicts: Option<Vec<Verdict>>,
    /// Active non-cable device faults; `None` when unavailable this tick.
    pub faults: Option<Vec<Fault>>,
    /// `Some` whenever the device's protection settings were (re)read.
    pub protection: Option<Protection>,
    /// Raw status for the alarm log (MSI: the C1 frame in hex).
    pub diagnostic: Option<String>,
}

impl Report {
    pub fn empty(at: Instant) -> Self {
        Report { at, readings: None, verdicts: None, faults: None, protection: None, diagnostic: None }
    }

    /// A tick is healthy only with currents and, for a source that has a verdict of its own,
    /// that verdict: a missing verdict never reads as Normal.
    pub fn is_healthy(&self, caps: &Capabilities) -> bool {
        self.readings.is_some() && (!caps.device_verdict || self.verdicts.is_some())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorReading {
    pub index: u8,
    /// Amps per wire; `None` for a wire the device could not measure.
    pub wires: [Option<f32>; WIRES],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub index: u8,
    pub status: DeviceStatus,
    /// Wires the device flagged (all false when the device has no wire flags).
    pub flagged: [bool; WIRES],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceStatus {
    Normal,
    /// A wire stayed above the device's wire limit for its trigger time.
    OverCurrent,
    /// The imbalance between wires stayed above the device's limit for its trigger time.
    Imbalance,
    /// A wire exceeded the device's hard limit; power may be cut immediately.
    CriticalOverCurrent,
    /// A non-normal status code this build does not know.
    Unknown(u8),
}

impl DeviceStatus {
    pub fn is_alarm(self) -> bool {
        self != DeviceStatus::Normal
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Protection {
    pub enabled: bool,
    pub wire_limit: Option<f32>,
    pub imbalance_limit: Option<f32>,
    pub wire_trigger: Option<Duration>,
    pub imbalance_trigger: Option<Duration>,
    pub cutoff_after: Option<Duration>,
    pub hard_wire_limit: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
/// A non-cable device fault. `Other` carries the device's own words.
pub enum Fault {
    OverTemperature,
    FanFailure,
    OverPower,
    RailOverCurrent(&'static str),
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connector_key_round_trips() {
        let k = ConnectorKey { source: SourceId("msi:ai1300ts".into()), index: 1 };
        assert_eq!(k.to_string(), "msi:ai1300ts:2");
        assert_eq!(ConnectorKey::parse("msi:ai1300ts:2"), Some(k));
        assert_eq!(ConnectorKey::parse("msi:ai1300ts:0"), None);
        assert_eq!(ConnectorKey::parse("nocolon"), None);
    }
}
