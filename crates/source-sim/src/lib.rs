//! Dev-only simulated source: scripted readings, no device traffic at all. A frontend uses it
//! instead of real drivers in its `simulate` build; it also proves the source API has a second
//! implementation.
//!
//! Pick a scenario with `MELTALARM_SIM` = cycle (default) | normal | idle | caution (a wire above the
//! rating: T23) | overload (MeltAlarm's own alarm) | uneven (advisory: T25) | red (bad contact, PSU
//! still Normal) | alarm (the PSU's alarm) | critical | fault | nodata | silent (present, never
//! answers: T13) | slow (answers after 10 s).

use std::time::{Duration, Instant};

use meltalarm_model::*;
use meltalarm_source_api::{Discovery, Driver, HidContext, Source};

pub struct SimDriver;

struct SimSource {
    info: SourceInfo,
    start: Instant,
    scenario: String,
}

const IDLE: [f32; 6] = [0.125; 6];
const LOAD: [f32; 6] = [8.1, 7.8, 7.7, 7.8, 8.0, 8.1];
const BAD: [f32; 6] = [9.9, 9.8, 2.1, 9.7, 9.8, 9.9];
const CRIT: [f32; 6] = [7.9, 18.6, 7.6, 7.8, 7.7, 7.9];
const HOT: [f32; 6] = [8.9, 9.1, 9.9, 9.0, 8.8, 9.0];
const OVER: [f32; 6] = [8.9, 9.1, 12.4, 9.0, 8.8, 9.0];
const UNEVEN: [f32; 6] = [8.2, 8.0, 4.4, 8.1, 8.3, 8.0];

impl Driver for SimDriver {
    fn name(&self) -> &'static str {
        "Simulated PSU"
    }
    fn discover(&self, _: &mut HidContext) -> Discovery {
        static FIRST: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let first = *FIRST.get_or_init(Instant::now);
        let scenario = std::env::var("MELTALARM_SIM").unwrap_or_default();
        if scenario == "silent" || (scenario == "slow" && first.elapsed() < Duration::from_secs(10)) {
            return Discovery::NotReady("PSU found but not answering (Timeout)".into());
        }
        Discovery::Found(Box::new(SimSource {
            info: SourceInfo {
                id: SourceId("sim:psu".into()),
                vendor: "Simulated".into(),
                model: "MPG Ai1300TS".into(),
                firmware: Some("sim".into()),
                serial: None,
                connectors: (0..2).map(|i| ConnectorInfo { index: i }).collect(),
                caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: true },
                protection_name: Some("Safeguard+".into()),
            },
            start: Instant::now(),
            scenario: std::env::var("MELTALARM_SIM").unwrap_or_else(|_| "cycle".into()),
        }))
    }
}

impl Source for SimSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn poll(&mut self, _: &mut HidContext) -> Report {
        let t = self.start.elapsed().as_secs() % 150;
        let (wires, status, fail) = match self.scenario.as_str() {
            "normal" | "fault" => (LOAD, 0, false),
            "idle" => (IDLE, 0, false),
            "caution" => (HOT, 0, false),
            "overload" => (OVER, 0, false),
            "uneven" => (UNEVEN, 0, false),
            "red" => (BAD, 0, false),
            "alarm" => (BAD, 2, false),
            "critical" => (CRIT, 3, false),
            "nodata" => (LOAD, 0, true),
            _ => match t {
                0..8 => (IDLE, 0, false),
                8..20 => (LOAD, 0, false),
                20..34 => (HOT, 0, false),     // caution strip at 30
                34..44 => (OVER, 0, false),    // our alarm at 35
                44..56 => (LOAD, 0, false),    // cleared
                56..70 => (UNEVEN, 0, false),  // advisory at 66
                70..90 => (LOAD, 0, false),
                90..115 => (BAD, 2, false),    // the PSU's alarm
                115..125 => (LOAD, 0, false),
                125..129 => (LOAD, 0, true),   // monitoring lost
                _ => (LOAD, 0, false),
            },
        };
        let at = Instant::now();
        if fail {
            return Report::empty(at);
        }
        let status_of = |s| match s {
            2 => DeviceStatus::Imbalance,
            3 => DeviceStatus::CriticalOverCurrent,
            _ => DeviceStatus::Normal,
        };
        Report {
            at,
            readings: Some(vec![
                ConnectorReading { index: 0, wires: wires.map(Some) },
                ConnectorReading { index: 1, wires: [Some(0.0); 6] },
            ]),
            verdicts: Some(vec![
                Verdict {
                    index: 0,
                    status: status_of(status),
                    flagged: std::array::from_fn(|i| (status == 2 && i == 2) || (status == 3 && i == 1)),
                },
                Verdict { index: 1, status: DeviceStatus::Normal, flagged: [false; 6] },
            ]),
            faults: Some(if self.scenario == "fault" && self.start.elapsed().as_secs() >= 5 { vec![Fault::FanFailure] } else { vec![] }),
            protection: Some(Protection {
                enabled: true,
                wire_limit: Some(12.0),
                imbalance_limit: Some(5.5),
                wire_trigger: Some(Duration::from_secs(20)),
                imbalance_trigger: Some(Duration::from_secs(20)),
                cutoff_after: Some(Duration::from_secs(180)),
                hard_wire_limit: Some(18.0),
            }),
            diagnostic: (status != 0).then(|| format!("C1 SIMULATED status {status}")),
        }
    }
}
