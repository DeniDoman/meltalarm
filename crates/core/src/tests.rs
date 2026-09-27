//! Scenario tests: synthetic source feeds against the spec's rules (§6, §8, §9).

use super::*;
use meltalarm_model::{Capabilities, ConnectorInfo, ConnectorReading, SourceId};

const IDLE: [f32; 6] = [0.125; 6];
const LOAD: [f32; 6] = [8.1, 7.8, 7.7, 7.8, 8.0, 8.1];
const BAD: [f32; 6] = [9.9, 9.8, 2.1, 9.7, 9.8, 9.9];
const OFF: [f32; 6] = [0.0; 6];

fn info() -> SourceInfo {
    SourceInfo {
        id: SourceId("msi:ai1300ts".into()),
        vendor: "MSI".into(),
        model: "MPG Ai1300TS".into(),
        firmware: Some("10".into()),
        serial: None,
        connectors: (0..2).map(|i| ConnectorInfo { index: i, label: format!("12V-2x6 #{}", i + 1) }).collect(),
        caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: true },
    }
}

fn prot() -> Protection {
    Protection {
        enabled: true,
        wire_limit: Some(12.0),
        spread_limit: Some(5.5),
        wire_trigger: Some(Duration::from_secs(20)),
        spread_trigger: Some(Duration::from_secs(20)),
        cutoff_after: Some(Duration::from_secs(180)),
        hard_wire_limit: Some(18.0),
    }
}

/// Test harness: a Core plus a virtual clock advancing 1 s per tick.
struct H {
    core: Core,
    t0: Instant,
    now: Instant,
    log: Vec<LogEvent>,
}

impl H {
    fn new() -> H {
        let t0 = Instant::now();
        let mut h = H { core: Core::new(Settings::default()), t0, now: t0, log: vec![] };
        h.ev(Event::SourceConnected(info()));
        h
    }
    fn ev(&mut self, e: Event) -> Output {
        let o = self.core.handle(e, self.now);
        self.log.extend(o.log.clone());
        o
    }
    fn report(&mut self, c1: [f32; 6], c2: [f32; 6], status: [u8; 2], protection: bool) -> Output {
        let st = |n: u8| match n {
            0 => DeviceStatus::Normal,
            1 => DeviceStatus::OverCurrent,
            2 => DeviceStatus::Imbalance,
            3 => DeviceStatus::CriticalOverCurrent,
            n => DeviceStatus::Unknown(n),
        };
        let r = Report {
            at: self.now,
            readings: Some(vec![
                ConnectorReading { index: 0, wires: c1.map(Some) },
                ConnectorReading { index: 1, wires: c2.map(Some) },
            ]),
            verdicts: Some(
                (0..2).map(|i| Verdict { index: i as u8, status: st(status[i]), flagged: [false, false, status[i] == 2, false, false, false] }).collect(),
            ),
            faults: Some(vec![]),
            protection: protection.then(prot),
            diagnostic: (status != [0, 0]).then(|| "C1 51C102".into()),
        };
        self.ev(Event::Report(r))
    }
    /// One normal-cadence tick: advance 1 s, report.
    fn tick(&mut self, c1: [f32; 6], status: [u8; 2]) -> Output {
        self.now += Duration::from_secs(1);
        self.report(c1, OFF, status, false)
    }
    fn failed_tick(&mut self) -> Output {
        self.now += Duration::from_secs(1);
        let at = self.now;
        self.ev(Event::Report(Report::empty(at)))
    }
    fn advance(&mut self, d: Duration) -> Output {
        self.now += d;
        self.ev(Event::Wake)
    }
    fn view(&self) -> ViewModel {
        self.core.view(self.now)
    }
    fn tags(&self) -> Vec<String> {
        self.log.iter().map(|e| e.format("T").split(" | ").nth(1).unwrap().trim().to_owned()).collect()
    }
    fn start(mut self) -> H {
        self.report(LOAD, OFF, [0, 0], true);
        self
    }
}

#[test]
fn first_report_tracks_the_connector_in_use_and_logs_config() {
    let mut h = H::new();
    let o = h.report(IDLE, OFF, [0, 0], true);
    let s = o.settings_changed.expect("first run picks connectors");
    assert_eq!(s.tracked.iter().map(|k| k.to_string()).collect::<Vec<_>>(), ["msi:ai1300ts:1"]);
    assert_eq!(h.tags(), ["CONFIG"]);
    let v = h.view();
    assert_eq!(v.health, Health::Live);
    assert_eq!(v.connectors[0].glyph, Glyph::Normal);
    assert_eq!(v.connectors[1].glyph, Glyph::NotConnected);
    assert!(v.alarm.is_none() && v.audio.is_none());
    assert_eq!(v.connectors[0].tooltip, "MeltAlarm · #1 · OK · max 0.1A · Δ 0.0A");
    assert_eq!(v.connectors[1].tooltip, "MeltAlarm · #2 · Not connected");
}

#[test]
fn device_alarm_raises_overlay_audio_countdown_and_logs() {
    let mut h = H::new().start();
    h.tick(BAD, [2, 0]);
    let v = h.view();
    let a = v.alarm.expect("alarm shown");
    assert_eq!(a.kind, AlarmKind::Active);
    assert_eq!(a.headline, "GPU POWER CABLE OVERLOAD");
    assert_eq!((a.right_label.as_str(), a.right_value.as_str()), ("POWER CUT IN", "~3:00"));
    assert!(a.numbers.starts_with("Wire 3 at 2.1 A, others 9.7–9.9 A. Spread 7.8 A"));
    let audio = v.audio.expect("sound");
    assert!(matches!(audio.steps.last(), Some(AudioStep::Speak(t)) if t.contains("connector 1")));
    assert_eq!(v.connectors[0].glyph, Glyph::Alarm);
    assert!(h.tags().ends_with(&["RED START".into(), "PSU ALARM".into(), "PSU RAW".into()]));
    h.advance(Duration::from_secs(47));
    assert_eq!(h.view().alarm.unwrap().right_value, "~2:13");
}

#[test]
fn alarm_fires_for_untracked_connector_too() {
    let mut h = H::new().start();
    h.now += Duration::from_secs(1);
    h.report(LOAD, BAD, [0, 2], false);
    assert!(!h.view().connectors[1].tracked);
    assert!(h.view().alarm.is_some());
}

#[test]
fn snooze_hides_for_30s_then_reshows_while_alarm_persists() {
    let mut h = H::new().start();
    h.tick(BAD, [2, 0]);
    h.ev(Event::User(UserAction::Snooze));
    assert!(h.view().alarm.is_none() && h.view().audio.is_none());
    for _ in 0..29 {
        h.tick(BAD, [2, 0]);
    }
    assert!(h.view().alarm.is_none());
    h.tick(BAD, [2, 0]);
    assert!(h.view().alarm.is_some() && h.view().audio.is_some());
}

#[test]
fn escalation_cancels_snooze_immediately() {
    let mut h = H::new().start();
    h.tick(BAD, [2, 0]);
    h.ev(Event::User(UserAction::Snooze));
    h.tick([7.9, 18.6, 7.6, 7.8, 7.7, 7.9], [3, 0]);
    let a = h.view().alarm.expect("escalated");
    assert_eq!(a.kind, AlarmKind::Critical);
    assert_eq!(a.right_value, "ANY SECOND");
}

#[test]
fn clearing_shows_green_for_5s_even_when_snoozed_then_idle() {
    let mut h = H::new().start();
    h.tick(BAD, [2, 0]);
    for _ in 0..41 {
        h.tick(BAD, [2, 0]);
    }
    h.ev(Event::User(UserAction::Snooze));
    h.tick(LOAD, [0, 0]);
    let a = h.view().alarm.expect("green notch");
    assert_eq!(a.kind, AlarmKind::Cleared);
    assert!(a.green && h.view().audio.is_none());
    assert_eq!(a.right_value, "0:42");
    assert!(h.tags().contains(&"PSU CLEAR".to_string()));
    h.advance(Duration::from_secs(5));
    assert!(h.view().alarm.is_none());
}

#[test]
fn missing_status_never_clears_an_alarm_and_no_data_keeps_it_up() {
    let mut h = H::new().start();
    h.tick(BAD, [2, 0]);
    for _ in 0..3 {
        h.failed_tick();
    }
    let v = h.view();
    assert!(matches!(v.health, Health::NoData { .. }));
    let a = v.alarm.expect("alarm stays without data");
    assert_eq!(a.kind, AlarmKind::DataLost);
    assert!(a.note.is_some());
    assert!(h.tags().contains(&"NO DATA".to_string()));
    h.tick(BAD, [2, 0]);
    assert!(h.tags().contains(&"DATA BACK".to_string()));
}

#[test]
fn three_failed_ticks_mean_no_data_and_grey_glyphs() {
    let mut h = H::new().start();
    h.failed_tick();
    h.failed_tick();
    assert!(!h.tags().contains(&"NO DATA".to_string()));
    h.failed_tick();
    assert_eq!(h.view().connectors[0].glyph, Glyph::NoData);
    assert!(h.tags().contains(&"NO DATA".to_string()));
}

#[test]
fn watchdog_marks_no_data_when_reports_stop() {
    let mut h = H::new().start();
    h.advance(Duration::from_secs(3));
    assert!(matches!(h.view().health, Health::Stale { .. }));
    h.advance(Duration::from_secs(2));
    assert!(matches!(h.view().health, Health::NoData { .. }));
}

#[test]
fn suspend_is_not_no_data() {
    let mut h = H::new().start();
    h.ev(Event::Suspended);
    h.advance(Duration::from_secs(600));
    h.ev(Event::Resumed);
    h.tick(LOAD, [0, 0]);
    assert!(!h.tags().contains(&"NO DATA".to_string()));
    assert_eq!(h.view().health, Health::Live);
}

#[test]
fn red_spike_is_visual_and_logged_once_as_an_episode() {
    let mut h = H::new().start();
    let spike = [12.5, 8.0, 8.0, 8.0, 8.0, 8.0];
    h.tick(spike, [0, 0]);
    assert!(h.view().alarm.is_none());
    assert_eq!(h.view().connectors[0].wires[0].level, Level::Warning);
    h.tick(LOAD, [0, 0]);
    h.tick(spike, [0, 0]); // re-enters within 10 s: same episode
    for _ in 0..11 {
        h.tick(LOAD, [0, 0]);
    }
    assert_eq!(h.tags().iter().filter(|t| t.starts_with("RED")).collect::<Vec<_>>(), ["RED START", "RED END"]);
}

#[test]
fn sustained_red_without_device_alarm_is_noted_after_trigger_plus_10s() {
    let mut h = H::new().start();
    for _ in 0..30 {
        h.tick(BAD, [0, 0]); // red since the first of these ticks: 29 s elapsed
    }
    assert!(!h.tags().contains(&"RED SUSTAINED".to_string()));
    h.tick(BAD, [0, 0]);
    assert!(h.tags().contains(&"RED SUSTAINED".to_string()));
    assert!(h.view().connectors[0].notes.iter().any(|n| n.text.contains("has not raised an alarm")));
    assert!(h.view().alarm.is_none(), "our own reading never raises the alarm");
}

#[test]
fn alarm_switch_off_suppresses_overlay_and_audio_but_not_the_log() {
    let mut h = H::new().start();
    h.ev(Event::User(UserAction::SetAlarmEnabled(false)));
    h.tick(BAD, [2, 0]);
    assert!(h.view().alarm.is_none() && h.view().audio.is_none());
    assert!(h.tags().contains(&"PSU ALARM".to_string()));
    assert_eq!(h.view().connectors[0].glyph, Glyph::Alarm);
}

#[test]
fn test_alarm_runs_until_snoozed_and_real_alarm_preempts() {
    let mut h = H::new().start();
    h.ev(Event::User(UserAction::TestAlarm));
    let a = h.view().alarm.unwrap();
    assert!(a.test && a.kind == AlarmKind::Test);
    assert!(h.view().audio.is_some());
    h.ev(Event::User(UserAction::Snooze));
    assert!(h.view().alarm.is_none());
    h.ev(Event::User(UserAction::TestAlarm));
    h.tick(BAD, [2, 0]);
    assert_eq!(h.view().alarm.unwrap().kind, AlarmKind::Active);
    assert!(!h.tags().iter().any(|t| t == "PSU ALARM") || h.tags().iter().filter(|t| *t == "PSU ALARM").count() == 1);
}

#[test]
fn last_tracked_connector_cannot_be_untracked() {
    let mut h = H::new().start();
    let k1 = h.view().connectors[0].key.clone();
    let k2 = h.view().connectors[1].key.clone();
    assert!(h.ev(Event::User(UserAction::SetTracked(k1.clone(), false))).settings_changed.is_none());
    assert!(h.ev(Event::User(UserAction::SetTracked(k2, true))).settings_changed.is_some());
    assert!(h.ev(Event::User(UserAction::SetTracked(k1, false))).settings_changed.is_some());
}

#[test]
fn protection_off_and_threshold_changes_are_logged_and_flagged() {
    let mut h = H::new().start();
    h.now += Duration::from_secs(1);
    let mut p = prot();
    p.enabled = false;
    let r = Report { protection: Some(p), ..Report::empty(h.now) };
    h.ev(Event::Report(r));
    assert!(h.log.iter().any(|e| matches!(e, LogEvent::ConfigWarning { .. })));
    assert!(h.view().connectors[0].attention);
}

#[test]
fn next_wake_is_always_scheduled_while_live() {
    let mut h = H::new().start();
    let o = h.tick(LOAD, [0, 0]);
    let wake = o.next_wake.expect("watchdog wake");
    assert!(wake > h.now && wake <= h.now + Duration::from_secs(5));
    let _ = h.t0;
}

#[test]
fn silent_psu_is_connecting_then_noticed_once_after_2_min_then_connected_is_logged() {
    let t0 = Instant::now();
    let mut core = Core::new(Settings::default());
    let v = core.view(t0);
    assert_eq!(v.health, Health::Starting);
    assert_eq!(v.connecting.as_deref(), Some("MeltAlarm · looking for the PSU…"));

    let o = core.handle(Event::SourcePending("PSU found but not answering (Timeout)".into()), t0);
    assert_eq!(o.next_wake, Some(t0 + NOT_CONNECTED_AFTER), "wakes up to report after 2 min");
    let v = core.view(t0);
    assert!(matches!(v.health, Health::Connecting { .. }));
    assert_eq!(v.connecting.as_deref(), Some("MeltAlarm · connecting to the PSU…"));
    assert!(v.notice.is_none() && v.connectors.is_empty());

    let o = core.handle(Event::Wake, t0 + NOT_CONNECTED_AFTER - Duration::from_secs(1));
    assert!(o.log.is_empty());
    let o = core.handle(Event::Wake, t0 + NOT_CONNECTED_AFTER);
    assert_eq!(o.log, [LogEvent::NotConnected { reason: "PSU found but not answering (Timeout)".into() }]);
    let n = core.view(t0 + NOT_CONNECTED_AFTER).notice.expect("one notice");
    assert!(n.text.starts_with("PSU found but not answering (Timeout). MeltAlarm keeps trying"));
    // Repeated pending reports and wakes never repeat it.
    core.handle(Event::SourcePending("PSU found but not answering (Timeout)".into()), t0 + Duration::from_secs(200));
    assert!(core.handle(Event::Wake, t0 + Duration::from_secs(300)).log.is_empty());
    assert_eq!(core.view(t0 + Duration::from_secs(300)).notice.map(|n| n.id), Some(n.id));

    let o = core.handle(Event::SourceConnected(info()), t0 + Duration::from_secs(158));
    assert_eq!(o.log, [LogEvent::Connected { model: "MSI MPG Ai1300TS".into(), after: Duration::from_secs(158) }]);
    let v = core.view(t0 + Duration::from_secs(158));
    assert!(v.connecting.is_none() && v.notice.is_none());
}

#[test]
fn quick_connect_after_pending_logs_nothing() {
    let t0 = Instant::now();
    let mut core = Core::new(Settings::default());
    core.handle(Event::SourcePending("busy".into()), t0);
    let o = core.handle(Event::SourceConnected(info()), t0 + Duration::from_secs(4));
    assert!(o.log.is_empty(), "a normal few-second connect at boot is not an event");
}
