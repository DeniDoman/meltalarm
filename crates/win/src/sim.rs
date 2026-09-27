//! Dev-only simulated source (cargo feature `simulate`): scripted readings, no USB at all.
//! Pick a scenario with `MELTALARM_SIM` = cycle (default) | normal | idle | red | alarm | critical | nodata.

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

impl Driver for SimDriver {
    fn name(&self) -> &'static str {
        "Simulated PSU"
    }
    fn discover(&self, _: &mut HidContext) -> Discovery {
        Discovery::Found(Box::new(SimSource {
            info: SourceInfo {
                id: SourceId("sim:psu".into()),
                vendor: "Simulated".into(),
                model: "MPG Ai1300TS".into(),
                firmware: Some("sim".into()),
                serial: None,
                connectors: (0..2).map(|i| ConnectorInfo { index: i, label: format!("12V-2x6 #{}", i + 1) }).collect(),
                caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: true },
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
        let t = self.start.elapsed().as_secs() % 70;
        let (wires, status, fail) = match self.scenario.as_str() {
            "normal" => (LOAD, 0, false),
            "idle" => (IDLE, 0, false),
            "red" => (BAD, 0, false),
            "alarm" => (BAD, 2, false),
            "critical" => (CRIT, 3, false),
            "nodata" => (LOAD, 0, true),
            _ => match t {
                0..8 => (IDLE, 0, false),
                8..20 => (LOAD, 0, false),
                20..30 => (BAD, 0, false),
                30..55 => (BAD, 2, false),
                55..60 => (LOAD, 0, false),
                60..64 => (LOAD, 0, true),
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
            faults: Some(vec![]),
            protection: Some(Protection {
                enabled: true,
                wire_limit: Some(12.0),
                spread_limit: Some(5.5),
                wire_trigger: Some(Duration::from_secs(20)),
                spread_trigger: Some(Duration::from_secs(20)),
                cutoff_after: Some(Duration::from_secs(180)),
                hard_wire_limit: Some(18.0),
            }),
            diagnostic: (status != 0).then(|| format!("C1 SIMULATED status {status}")),
        }
    }
}
