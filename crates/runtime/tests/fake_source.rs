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
                imbalance_limit: Some(5.5),
                wire_trigger: Some(Duration::from_secs(20)),
                imbalance_trigger: Some(Duration::from_secs(20)),
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
        connectors: vec![ConnectorInfo { index: 0 }],
        caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: false },
        protection_name: None,
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
    assert!(log.contains("| PSU ALARM     | GPU power cable 1 | status Current imbalance"), "{log}");
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

/// Present but silent for a few polls, then answers (Spec §5.1, T13 shape).
struct SlowDriver(std::sync::atomic::AtomicUsize);

impl Driver for SlowDriver {
    fn name(&self) -> &'static str {
        "Slow PSU"
    }
    fn discover(&self, _: &mut HidContext) -> Discovery {
        if self.0.fetch_add(1, Ordering::SeqCst) < 1 {
            Discovery::NotReady("PSU found but not answering (Timeout)".into())
        } else {
            Discovery::Found(Box::new(FakeSource(info(), 0)))
        }
    }
}

#[test]
fn silent_device_is_retried_instead_of_failing_startup() {
    let dir = std::env::temp_dir().join(format!("meltalarm-rt-slow-{}", std::process::id()));
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let mut rt = Runtime::start(Host {
        drivers: vec![Box::new(SlowDriver(AtomicUsize::new(0)))],
        paths: Paths { config_dir: dir.clone(), log_dir: dir.clone() },
        wall_clock: || "2026-09-28 00:00:00".into(),
        waker: Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
    });
    wait_wakes(&wakes, 1); // SourcePending
    let u = rt.pump(Instant::now());
    assert!(u.lifecycle.is_none(), "a present but silent PSU never ends startup");
    assert_eq!(rt.view(Instant::now()).connecting.as_deref(), Some("MeltAlarm · connecting to the PSU…"));
    wait_wakes(&wakes, 3); // connected (after the 2 s retry) + first report
    rt.pump(Instant::now());
    let v = rt.view(Instant::now());
    assert!(v.connecting.is_none() && !v.connectors.is_empty());
    drop(rt);
    let _ = std::fs::remove_dir_all(dir);
}
