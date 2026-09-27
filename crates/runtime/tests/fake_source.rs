//! End-to-end through the real acquisition thread with a fake driver (no hardware).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use meltalarm_model::*;
use meltalarm_runtime::core::{AlarmKind, Glyph};
use meltalarm_runtime::{Host, Lifecycle, Paths, Runtime};
use meltalarm_source_api::{Discovery, Driver, HidContext, Source};

struct FakeDriver(Option<u8>);
struct FakeSource(SourceInfo, u8);

impl Driver for FakeDriver {
    fn name(&self) -> &'static str {
        "Fake PSU"
    }
    fn discover(&self, _: &mut HidContext) -> Discovery {
        match self.0 {
            Some(status) => Discovery::Found(Box::new(FakeSource(info(), status))),
            None => Discovery::Unusable("fake: unusable".into()),
        }
    }
}

impl Source for FakeSource {
    fn info(&self) -> &SourceInfo {
        &self.0
    }
    fn poll(&mut self, _: &mut HidContext) -> Report {
        let status = if self.1 == 2 { DeviceStatus::Imbalance } else { DeviceStatus::Normal };
        Report {
            at: Instant::now(),
            readings: Some(vec![ConnectorReading { index: 0, wires: [9.9, 9.8, 2.1, 9.7, 9.8, 9.9].map(Some) }]),
            verdicts: Some(vec![Verdict { index: 0, status, flagged: [false; 6] }]),
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
            diagnostic: None,
        }
    }
}

fn info() -> SourceInfo {
    SourceInfo {
        id: SourceId("fake:psu".into()),
        vendor: "Fake".into(),
        model: "PSU".into(),
        firmware: None,
        serial: None,
        connectors: vec![ConnectorInfo { index: 0, label: "12V-2x6 #1".into() }],
        caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: false },
    }
}

fn start(driver: FakeDriver, tag: &str) -> (Runtime, Arc<AtomicUsize>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("meltalarm-rt-{tag}-{}", std::process::id()));
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let rt = Runtime::start(Host {
        drivers: vec![Box::new(driver)],
        paths: Paths { config_dir: dir.clone(), log_dir: dir.clone() },
        wall_clock: || "2026-09-27 00:00:00".into(),
        waker: Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
    });
    (rt, wakes, dir)
}

fn wait_wakes(w: &AtomicUsize, n: usize) {
    let t = Instant::now();
    while w.load(Ordering::SeqCst) < n && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn device_alarm_flows_to_view_log_and_settings() {
    let (mut rt, wakes, dir) = start(FakeDriver(Some(2)), "alarm");
    wait_wakes(&wakes, 2); // connected + first report
    let u = rt.pump(Instant::now());
    assert!(u.lifecycle.is_none());
    let v = rt.view(Instant::now());
    assert_eq!(v.connectors[0].glyph, Glyph::Alarm);
    assert_eq!(v.alarm.as_ref().unwrap().kind, AlarmKind::Active);
    assert!(v.connectors[0].tracked, "first run tracks the loaded connector");
    drop(rt);
    let log = std::fs::read_to_string(dir.join("alarms.log")).unwrap();
    assert!(log.contains("| PSU ALARM     | 12V-2x6 #1 | status Current imbalance"));
    assert!(std::fs::read_to_string(dir.join("settings.toml")).unwrap().contains("fake:psu:1"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unusable_device_ends_startup_with_its_message() {
    let (mut rt, wakes, dir) = start(FakeDriver(None), "unusable");
    wait_wakes(&wakes, 1);
    let u = rt.pump(Instant::now());
    assert_eq!(u.lifecycle, Some(Lifecycle::DiscoveryFailed("fake: unusable".into())));
    drop(rt);
    let _ = std::fs::remove_dir_all(dir);
}
