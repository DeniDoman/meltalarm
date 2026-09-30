//! Scenario tests: synthetic source feeds against the spec's rules (§6, §8, §9).

use super::*;
use meltalarm_model::{Capabilities, ConnectorInfo, ConnectorReading, SourceId};

const IDLE: [f32; 6] = [0.125; 6];
const LOAD: [f32; 6] = [8.1, 7.8, 7.7, 7.8, 8.0, 8.1];
/// A bad contact on wire 3: the PSU's imbalance case. Below our alarm limit everywhere.
const BAD: [f32; 6] = [9.9, 9.8, 2.1, 9.7, 9.8, 9.9];
const OFF: [f32; 6] = [0.0; 6];

fn pin1(a: f32) -> [f32; 6] {
    let mut w = LOAD;
    w[0] = a;
    w
}

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

fn wall() -> String {
    "2026-09-30 18:02:11".into()
}

/// Test harness: a Core plus a virtual clock advancing 1 s per tick.
struct H {
    core: Core,
    now: Instant,
    log: Vec<LogEvent>,
    state: Option<CoreState>,
}

impl H {
    fn with(settings: Settings, state: Option<CoreState>) -> H {
        let mut core = Core::new(settings);
        core.set_wall_clock(wall);
        if let Some(s) = state {
            core.restore(s);
        }
        let mut h = H { core, now: Instant::now(), log: vec![], state: None };
        h.ev(Event::SourceConnected(info()));
        h
    }
    fn new() -> H {
        H::with(Settings::default(), None)
    }
    fn ev(&mut self, e: Event) -> Output {
        let o = self.core.handle(e, self.now);
        self.log.extend(o.log.clone());
        if let Some(s) = &o.state_changed {
            self.state = Some(s.clone());
        }
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
    fn ticks(&mut self, n: usize, c1: [f32; 6]) {
        for _ in 0..n {
            self.tick(c1, [0, 0]);
        }
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
    fn count(&self, tag: &str) -> usize {
        self.tags().iter().filter(|t| *t == tag).count()
    }
    fn note(&self) -> Option<CableNote> {
        self.state.as_ref().and_then(|s| s.notes.values().next().cloned())
    }
    fn start(mut self) -> H {
        self.report(LOAD, OFF, [0, 0], true);
        self
    }
}

#[test]
fn first_report_tracks_the_connector_in_use_and_logs_limits_and_config() {
    let mut h = H::new();
    let o = h.report(IDLE, OFF, [0, 0], true);
    let s = o.settings_changed.expect("first run picks connectors");
    assert_eq!(s.tracked.iter().map(|k| k.to_string()).collect::<Vec<_>>(), ["msi:ai1300ts:1"]);
    assert_eq!(h.tags(), ["LIMITS", "CONFIG"]);
    let v = h.view();
    assert_eq!(v.health, Health::Live);
    assert_eq!(v.connectors[0].glyph, Glyph::Normal);
    assert_eq!(v.connectors[1].glyph, Glyph::NotConnected);
    assert!(v.alarm.is_none() && v.audio.is_none() && v.caution.is_none());
    assert_eq!(v.connectors[0].tooltip, "MeltAlarm · #1 · OK · max 0.1A · Δ 0.0A");
    assert_eq!(v.connectors[1].tooltip, "MeltAlarm · #2 · Not connected");
    assert_eq!(v.connectors[0].psu_status, "Normal");
    assert_eq!((v.connectors[0].bar_limit, v.connectors[0].caution_line), (Some(10.5), Some(9.5)));
}

#[test]
fn device_alarm_raises_overlay_audio_countdown_note_and_logs() {
    let mut h = H::new().start();
    h.tick(BAD, [2, 0]);
    let v = h.view();
    let a = v.alarm.expect("alarm shown");
    assert_eq!(a.kind, AlarmKind::Active);
    assert_eq!(a.headline, "GPU POWER CABLE OVERLOAD");
    assert_eq!(a.what, "Current imbalance · reported by the PSU");
    assert_eq!((a.right_label.as_str(), a.right_value.as_str()), ("POWER CUT IN", "~3:00"));
    assert!(a.numbers.starts_with("Wire 3 at 2.1 A, others 9.7–9.9 A. Spread 7.8 A"));
    let audio = v.audio.expect("sound");
    assert!(matches!(audio.steps.last(), Some(AudioStep::Speak(t)) if t.contains("connector 1")));
    assert_eq!(v.connectors[0].glyph, Glyph::Alarm);
    assert_eq!(v.connectors[0].alarm_reason.as_deref(), Some("PSU: Current imbalance · wire 3"));
    assert!(h.tags().ends_with(&["PSU ALARM".into(), "PSU RAW".into()]));
    let note = h.note().expect("cable note");
    assert_eq!(note.severity, Severity::Alarm);
    assert!(note.text.starts_with("PSU alarm on Sep 30, 18:02: Current imbalance. Inspect"), "{}", note.text);
    assert!(h.state.as_ref().unwrap().alarm_open.is_some(), "a power cut would be noticed at the next start");
    h.advance(Duration::from_secs(47));
    assert_eq!(h.view().alarm.unwrap().right_value, "~2:13");
    let c = &h.view().connectors[0];
    assert_eq!((c.status_text.as_str(), c.summary.as_str()), ("ALARM", "Imbalance · cut ~2:13"));
    assert_eq!(c.summary_level, Level::Warning);
    assert_eq!(c.psu_status, "Current imbalance");
}

#[test]
fn our_overload_alarms_without_the_psu_and_says_so() {
    let mut h = H::new().start();
    h.tick(pin1(12.5), [0, 0]); // one reading: red, no alarm
    assert!(h.view().alarm.is_none());
    assert_eq!(h.view().connectors[0].wires[0].level, Level::Warning);
    h.tick(pin1(12.6), [0, 0]); // the second reading at >= 12 A
    let v = h.view();
    let a = v.alarm.expect("MeltAlarm's own alarm");
    assert_eq!(a.what, "Wire overload · measured by MeltAlarm");
    assert_eq!(a.numbers, "Wire 1 at 12.6 A, rated 9.5 A. The PSU hasn't raised an alarm yet.");
    assert_eq!((a.right_label.as_str(), a.right_value.as_str()), ("WIRE 1", "12.6 A"));
    assert!(v.audio.is_some());
    let c = &v.connectors[0];
    assert_eq!((c.glyph, c.status_text.as_str()), (Glyph::Alarm, "ALARM"));
    assert_eq!(c.alarm_reason.as_deref(), Some("Wire 1 overload · 12.6 A"));
    assert_eq!(c.summary, "Wire 1 · 12.6 A · stop");
    assert!(c.wires[0].flagged, "the overloaded wire is knocked out in the alarm tile");
    assert_eq!(c.tooltip, "MeltAlarm · #1 · ALARM: wire 1 overload");
    assert_eq!(h.count("OVERLOAD"), 1);
    assert!(h.log.iter().any(|e| e.format("T").contains("wire 1 = 12.6 A · 2 readings >= 12.0 A")));

    // The note follows the peak; the alarm clears when every wire is below the rating.
    h.tick(pin1(13.1), [0, 0]);
    h.tick(pin1(9.8), [0, 0]);
    assert!(h.view().alarm.as_ref().is_some_and(|a| !a.green), "9.8 A is still above the rating");
    h.tick(LOAD, [0, 0]);
    let a = h.view().alarm.expect("green notch");
    assert!(a.green && h.view().audio.is_none());
    assert_eq!(a.sub, "Inspect the cable before the next session. The note is in MeltAlarm.");
    assert_eq!(h.count("OVERLOAD END"), 1);
    let note = h.note().unwrap();
    assert!(note.text.starts_with("Alarm on Sep 30, 18:02: wire 1 reached 13.1 A."), "{}", note.text);
    assert!(h.state.as_ref().unwrap().alarm_open.is_none());
    assert_eq!(h.view().connectors[0].tooltip, "MeltAlarm · #1 · OK · check the cable");
    assert!(h.view().connectors[0].attention);
}

#[test]
fn the_psu_joining_our_overload_escalates_a_snooze() {
    let mut h = H::new().start();
    h.tick(pin1(16.0), [0, 0]);
    assert!(h.view().alarm.is_some());
    h.ev(Event::User(UserAction::Snooze));
    h.tick(pin1(16.0), [0, 0]);
    assert!(h.view().alarm.is_none());
    h.tick(pin1(16.0), [1, 0]);
    let a = h.view().alarm.expect("a new cause cancels the snooze");
    assert_eq!(a.what, "Over-current · reported by the PSU");
    assert!(a.numbers.ends_with("Wire 1 at 16.0 A, rated 9.5 A."), "{}", a.numbers);
}

#[test]
fn a_caution_shows_the_strip_once_with_one_chime_and_leaves_a_note() {
    let mut h = H::new().start();
    h.ticks(10, pin1(9.9));
    assert!(h.view().caution.is_none(), "not yet 10 s");
    h.ticks(1, pin1(9.9));
    let s = h.view().caution.expect("strip");
    assert_eq!((s.place.as_deref(), s.what.as_str(), s.action.as_str()), (Some("#1"), "Wire 1 at 9.9 A, above the 9.5 A rating", "Ease the GPU load"));
    assert_eq!(h.count("CAUTION"), 1);
    assert_eq!(h.note().unwrap().severity, Severity::Caution);
    let c = &h.view().connectors[0];
    assert_eq!((c.status_text.as_str(), c.summary.as_str()), ("Caution", "Wire 1 · 9.9 A · over rating"));
    assert!(c.tooltip.starts_with("MeltAlarm · #1 · Caution · max 9.9A"));
    h.ticks(9, pin1(9.9));
    assert!(h.view().caution.is_some());
    h.ticks(1, pin1(9.9));
    assert!(h.view().caution.is_none(), "gone after 10 s");
    h.ticks(60, pin1(9.9));
    assert!(h.view().caution.is_none(), "once per episode");
    assert!(h.view().alarm.is_none(), "a caution never becomes an alarm below the alarm limit");
    assert!(h.view().connectors[0].cable_note.is_none(), "while live, the live line says it");
    h.ticks(1, LOAD);
    assert!(h.view().connectors[0].cable_note.is_some(), "back to OK: the note shows");
    let note = h.note().unwrap();
    assert!(note.text.starts_with("Wire 1 ran above the 9.5 A rating on Sep 30, 18:02 (peak 9.9 A)."), "{}", note.text);
}

#[test]
fn an_alarm_replaces_the_strip_and_the_caution_note() {
    let mut h = H::new().start();
    h.ticks(11, pin1(9.9));
    let chime = h.view().caution.unwrap().chime;
    h.ticks(4, pin1(10.8));
    assert!(h.view().caution.is_some() && h.view().alarm.is_none());
    h.ticks(1, pin1(10.8)); // 4 s above 10.5 A
    assert!(h.view().caution.is_none() && h.view().alarm.is_some());
    assert_eq!(h.note().unwrap().severity, Severity::Alarm);
    let _ = chime;
}

#[test]
fn uneven_load_is_a_silent_notification_and_a_note_never_a_strip() {
    let mut h = H::new().start();
    h.ticks(11, [0.5, 8.0, 8.0, 8.0, 8.0, 8.0]);
    let v = h.view();
    assert!(v.caution.is_none() && v.alarm.is_none());
    let n = v.notice.expect("advisory");
    assert_eq!((n.title.as_str(), n.warning), ("Uneven load on 12V-2x6 #1", false));
    assert_eq!(n.connector, Some(v.connectors[0].key.clone()));
    assert_eq!(h.count("UNEVEN LOAD"), 1);
    let note = h.note().unwrap();
    assert_eq!(note.severity, Severity::Advisory);
    assert!(note.text.starts_with("Uneven load on Sep 30, 18:02: wire 1 carried 0.5 A while the others carried up to 8.0 A."));
    assert_eq!(v.connectors[0].summary, "Uneven · Δ 7.5A");
}

#[test]
fn a_less_severe_event_never_replaces_a_note_and_dismiss_removes_it() {
    let mut h = H::new().start();
    h.ticks(11, pin1(9.9)); // caution note
    h.ticks(40, LOAD);
    h.ticks(11, [0.5, 8.0, 8.0, 8.0, 8.0, 8.0]); // advisory: less severe
    assert_eq!(h.note().unwrap().severity, Severity::Caution);
    let key = h.view().connectors[0].key.clone();
    h.ev(Event::User(UserAction::DismissNote(key)));
    assert!(h.note().is_none());
    assert!(h.view().connectors[0].cable_note.is_none());
}

#[test]
fn losing_monitoring_after_having_data_is_a_caution() {
    let mut h = H::new().start();
    h.failed_tick();
    h.failed_tick();
    assert!(h.view().caution.is_none());
    h.failed_tick();
    let s = h.view().caution.expect("monitoring lost");
    assert_eq!((s.place, s.what.as_str()), (None, "Monitoring lost"));
}

#[test]
fn restart_after_a_session_that_ended_in_an_alarm_notifies_once() {
    let mut h = H::new().start();
    h.tick(pin1(16.0), [0, 0]);
    let saved = h.state.clone().expect("state saved");
    assert!(saved.alarm_open.is_some());

    let mut h = H::with(Settings::default(), Some(saved));
    let n = h.view().notice.expect("after-alarm notification");
    assert_eq!((n.title.as_str(), n.warning), ("Last session ended during a cable alarm", true));
    assert!(n.connector.is_some());
    assert!(h.state.as_ref().is_some_and(|s| s.alarm_open.is_none()), "the flag is cleared");
    h.report(LOAD, OFF, [0, 0], true);
    assert!(h.view().connectors[0].cable_note.is_some(), "the note survived the restart");
}

#[test]
fn compact_texts_for_normal_and_no_data() {
    let mut h = H::new().start();
    let c = &h.view().connectors[0];
    assert_eq!((c.status_text.as_str(), c.summary.as_str()), ("OK", "Σ 47.5A · Δ 0.4A"));
    h.failed_tick();
    h.failed_tick();
    h.failed_tick();
    let c = &h.view().connectors[0];
    assert_eq!(c.status_text, "No data");
    assert!(c.summary.starts_with("Last reading ") && c.summary.ends_with(" s ago"), "{}", c.summary);
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
    assert_eq!(a.what, "Current imbalance · cleared after 42 s");
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
    assert!(v.caution.is_none(), "no strip while an alarm is active");
    assert!(h.tags().contains(&"NO DATA".to_string()));
    h.tick(BAD, [2, 0]);
    assert!(h.tags().contains(&"DATA BACK".to_string()));
}

#[test]
fn no_data_never_clears_our_overload() {
    let mut h = H::new().start();
    h.tick(pin1(16.0), [0, 0]);
    for _ in 0..5 {
        h.failed_tick();
    }
    assert_eq!(h.view().alarm.map(|a| a.kind), Some(AlarmKind::DataLost));
    h.tick(LOAD, [0, 0]);
    assert!(h.view().alarm.is_some_and(|a| a.green));
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
    assert!(h.view().caution.is_none());
}

#[test]
fn alerts_off_suppresses_every_interruption_but_not_the_log_or_notes() {
    let mut h = H::new().start();
    h.ev(Event::User(UserAction::SetAlarmEnabled(false)));
    h.tick(BAD, [2, 0]);
    assert!(h.view().alarm.is_none() && h.view().audio.is_none());
    assert!(h.tags().contains(&"PSU ALARM".to_string()));
    assert_eq!(h.view().connectors[0].glyph, Glyph::Alarm);
    assert!(h.note().is_some());
    h.tick(LOAD, [0, 0]);
    h.ticks(11, pin1(9.9));
    assert!(h.view().caution.is_none());
    assert_eq!(h.count("CAUTION"), 1);
}

#[test]
fn test_alarm_runs_the_ladder_and_a_real_alarm_preempts_it() {
    let mut h = H::new().start();
    h.ev(Event::User(UserAction::TestAlarm));
    let s = h.view().caution.expect("the strip first");
    assert!(s.test);
    assert!(h.view().alarm.is_none() && h.view().audio.is_none());
    h.advance(Duration::from_secs(3));
    assert!(h.view().caution.is_none());
    let a = h.view().alarm.unwrap();
    assert!(a.test && a.kind == AlarmKind::Test);
    assert!(h.view().audio.is_some());
    h.ev(Event::User(UserAction::Snooze));
    assert!(h.view().alarm.is_none());
    h.ev(Event::User(UserAction::TestAlarm));
    h.tick(BAD, [2, 0]);
    assert_eq!(h.view().alarm.unwrap().kind, AlarmKind::Active);
    assert!(h.note().is_some_and(|n| n.text.starts_with("PSU alarm")), "a test never leaves a note");
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
fn protection_off_is_logged_and_flagged_and_c0_never_changes_colors() {
    let mut h = H::new().start();
    h.now += Duration::from_secs(1);
    let mut p = prot();
    p.enabled = false;
    p.wire_limit = Some(10.0);
    let r = Report { protection: Some(p), ..Report::empty(h.now) };
    h.ev(Event::Report(r));
    assert!(h.log.iter().any(|e| matches!(e, LogEvent::ConfigWarning { .. })));
    h.tick(LOAD, [0, 0]);
    let c = &h.view().connectors[0];
    assert!(c.attention);
    assert_eq!(c.psu_status, "Safeguard+ off");
    assert_eq!(c.wires[0].level, Level::Normal);
}

#[test]
fn custom_limits_apply_and_are_logged() {
    let settings = Settings { limits: LimitOverrides { alarm: Some(10.0), ..Default::default() }, ..Settings::default() };
    let mut h = H::with(settings, None).start();
    assert!(h.log.iter().any(|e| e.format("T").contains("LIMITS        | v1 custom")));
    h.tick(pin1(10.2), [0, 0]);
    assert_eq!(h.view().connectors[0].wires[0].level, Level::Warning);
}

#[test]
fn a_new_psu_fault_is_a_caution() {
    let mut h = H::new().start();
    h.now += Duration::from_secs(1);
    let mut r = Report::empty(h.now);
    r.readings = Some(vec![ConnectorReading { index: 0, wires: LOAD.map(Some) }, ConnectorReading { index: 1, wires: OFF.map(Some) }]);
    r.verdicts = Some((0..2).map(|i| Verdict { index: i, status: DeviceStatus::Normal, flagged: [false; 6] }).collect());
    r.faults = Some(vec![Fault::FanFailure]);
    h.ev(Event::Report(r));
    let s = h.view().caution.expect("strip");
    assert_eq!(s.what, "PSU fault: Fan failure");
}

#[test]
fn next_wake_is_always_scheduled_while_live() {
    let mut h = H::new().start();
    let o = h.tick(LOAD, [0, 0]);
    let wake = o.next_wake.expect("watchdog wake");
    assert!(wake > h.now && wake <= h.now + Duration::from_secs(5));
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
    assert!(n.warning);
    assert!(n.text.starts_with("PSU found but not answering (Timeout). MeltAlarm keeps trying"));
    // Repeated pending reports and wakes never repeat it.
    core.handle(Event::SourcePending("PSU found but not answering (Timeout)".into()), t0 + Duration::from_secs(200));
    assert!(core.handle(Event::Wake, t0 + Duration::from_secs(300)).log.is_empty());
    assert_eq!(core.view(t0 + Duration::from_secs(300)).notice.map(|n| n.id), Some(n.id));

    let o = core.handle(Event::SourceConnected(info()), t0 + Duration::from_secs(158));
    assert!(o.log.contains(&LogEvent::Connected { model: "MSI MPG Ai1300TS".into(), after: Duration::from_secs(158) }));
    let v = core.view(t0 + Duration::from_secs(158));
    assert!(v.connecting.is_none() && v.notice.is_none());
}

#[test]
fn quick_connect_after_pending_logs_nothing_but_the_limits() {
    let t0 = Instant::now();
    let mut core = Core::new(Settings::default());
    core.handle(Event::SourcePending("busy".into()), t0);
    let o = core.handle(Event::SourceConnected(info()), t0 + Duration::from_secs(4));
    assert!(matches!(o.log.as_slice(), [LogEvent::Limits { .. }]), "a normal few-second connect at boot is not an event");
}

#[test]
fn note_time_formats_the_wall_clock() {
    assert_eq!(note_time("2026-09-30 18:02:11"), "Sep 30, 18:02");
    assert_eq!(note_time("2026-01-05 07:00:00"), "Jan 5, 07:00");
    assert_eq!(note_time(""), "");
}

#[test]
fn no_advisory_notification_while_an_alarm_is_on_screen() {
    let mut h = H::new().start();
    h.ticks(12, [8.9, 9.1, 12.4, 9.0, 8.8, 9.0]); // overload + uneven load together
    assert!(h.view().alarm.is_some());
    assert_eq!(h.count("UNEVEN LOAD"), 1, "still logged");
    assert!(h.view().notice.is_none());
}
