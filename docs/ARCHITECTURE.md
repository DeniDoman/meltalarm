# MeltAlarm — Software Architecture

**Status:** v3 · **Date:** 2026-10-01 · Companions: `FUNCTIONAL_SPEC.md` (what), `DESIGN.md` (how it looks) · Changes: §16

This document describes the system as built. Where something is planned but not built, it says so.

---

## 1. Priorities and quality attributes

**Delivery:** v0 (the reference PC) and the cable guard (0.3) are done. Next: **v1**, a GitHub release for Windows (§14). After that: **Linux** and **other sensing devices**. The architecture keeps both open without building them.

**Quality attributes, highest first.** When two conflict, the higher one wins.

| # | Attribute | Meaning here |
|---|---|---|
| Q1 | **Hardware safety** | Nothing but allowlisted reads ever reaches a device. This is structural, not a convention (§9). |
| Q2 | **Alarm reliability** | An alarm (the cable guard or the device's verdict) reaches the user within 2 s, never clears without data, and survives any single component failure (voice, overlay, task…). |
| Q3 | **Footprint** | ≤ 5 MB private idle, ~0 % CPU, no GPU driver in the process |
| Q4 | **Evolvability** | New device family = new source crate. New OS = new frontend crate. `core` untouched in both cases. |
| Q5 | **Simplicity** | No speculative machinery; only the seams Q4 needs |

---

## 2. Architecture at a glance

```
                        ┌──────────────────────────────────────────┐
  frontends (per OS)    │ meltalarm-win (bin)     meltalarm-linux  │   render the view, run platform effects;
                        │ Win32/D2D/SAPI/Tasks    (next iteration) │   composition root (picks the drivers)
                        └────────────────┬─────────────────────────┘
                                         │ ViewModel ▲ │ UserAction ▼
                        ┌────────────────▼─────────────────────────┐
  host (portable)       │ meltalarm-runtime                        │   acquisition thread, discovery,
                        │                                          │   reconnect, settings/state files, log
                        └───────┬─────────────────────────┬────────┘
                                │ Event ▼ / Output,View ▲ │ Driver/Source traits
                        ┌───────▼────────┐       ┌────────▼─────────────────┐
  logic (pure)          │ meltalarm-core │       │ meltalarm-source-api     │  traits + shared HID context
                        └───────┬────────┘       └────────┬─────────────────┘
                                │                         │ implemented by
                        ┌───────▼─────────────────────────▼───┐   ┌───────────────────────────┐
  vocabulary (pure)     │ meltalarm-model                     │◄──│ meltalarm-source-msi      │  MSI Ai1300TS/Ai1600TS
                        └─────────────────────────────────────┘   │ meltalarm-source-sim (dev)│  scripted readings
                                                                  │ (future: source-<vendor>) │
                                                                  └───────────────────────────┘
```

Beside this stack, `meltalarm-lifecycle` (portable, std only) decides what a launch does and runs the install, update and uninstall steps each frontend supplies (§7.1).

**Dependency rule:** arrows point toward stability. `model` depends on nothing. `core` depends only on `model`. Sources know `model` + `source-api`, never `core`. `runtime` knows `core` + `source-api`, never a concrete source. Only a frontend's `main` (the composition root) names concrete drivers.

---

## 3. Crates

| Crate | Kind | Depends on | Portable | Responsibility |
|---|---|---|---|---|
| `meltalarm-model` | lib, pure, `forbid(unsafe)` | — | ✅ | Neutral vocabulary: readings, verdicts, protection, faults, source info, capabilities, the healthy-tick rule (§4.1) |
| `meltalarm-core` | lib, pure, `forbid(unsafe)` | model | ✅ | All decisions: levels, cable guard, alarm, alert ladder, cable notes, log events, settings and state formats, **ViewModel and every user-facing text** (§5) |
| `meltalarm-source-api` | lib | model, hidapi | ✅ | `Driver` / `Source` traits, `HidContext` (one shared `HidApi`), source authoring rules (§4.2) |
| `meltalarm-source-msi` | lib | model, source-api, windows-sys (cfg windows) | ✅ (lock is cfg) | MSI protocol: closed request set, frame decoding, transactions, `Global\MSI_PSU_Mutex` (Windows), mapping to the model (§4.3) |
| `meltalarm-source-sim` | lib, dev only | model, source-api | ✅ | Scripted scenarios, no device traffic; used by `simulate` builds (§4.4) |
| `meltalarm-runtime` | lib, `forbid(unsafe)` | core, source-api | ✅ | Portable host: acquisition thread, discovery and reconnect, settings/state files, log sink, wake-ups (§6) |
| `meltalarm-lifecycle` | lib, std only | — | ✅ | Program lifecycle, OS-free: launch decision (Spec §4.4), version compare, all-or-nothing step runner, the `Autostart` / `Instances` / `Step` traits (§7.1) |
| `meltalarm-win` | bin `meltalarm` | runtime, core, lifecycle, source-msi, source-sim (feature `simulate`), windows | ❌ Windows | Tray, menu, flyout, floating view, notch, caution strip, audio, voice, hotkey, power events; the Windows lifecycle backend (§7) |
| *planned (v1)* `psu-probe` | bin | source-msi | ✅ | Read-only diagnostics, serial redacted, for hardware reports and porting |
| *future* `meltalarm-linux` | bin | runtime, core, sources | Linux | §11.1 |
| *future* `meltalarm-source-<vendor>` | lib | model, source-api | per device | §11.2 |

Names carry the `meltalarm-` prefix. A crate named `core` would shadow Rust's built-in `core`.

---

## 4. The source abstraction

### 4.1 Neutral model (`meltalarm-model`)

A *source* is anything that measures per-wire current on 12V-2x6 connectors: today a PSU; later possibly a GPU or an inline meter.

```rust
pub struct SourceInfo {
    pub id: SourceId,                     // stable, e.g. "msi:ai1300ts" — keys settings, state, placement
    pub vendor: String, pub model: String,
    pub firmware: Option<String>, pub serial: Option<String>,
    pub connectors: Vec<ConnectorInfo>,   // ConnectorInfo { index }: numbered as on the device
    pub caps: Capabilities,
    pub protection_name: Option<String>,  // the device's name for its protection, e.g. "Safeguard+"
}
pub struct Capabilities {
    pub device_verdict: bool,   // device reports its own Normal/alarm status per connector
    pub device_limits: bool,    // device reports the thresholds it enforces
    pub cutoff_timer: bool,     // device cuts power N s after alarm (countdown possible)
    pub wire_flags: bool,       // device flags which wire tripped
}
pub struct Report {                               // one poll (one tick)
    pub at: Instant,
    pub readings: Option<Vec<ConnectorReading>>,  // None = currents unavailable this tick
    pub verdicts: Option<Vec<Verdict>>,           // None = status unavailable this tick (never "Normal")
    pub faults: Option<Vec<Fault>>,               // non-cable device faults (temperature, fan, …)
    pub protection: Option<Protection>,           // Some when (re)read; core keeps the last one
    pub diagnostic: Option<String>,               // raw status for the log during an alarm (MSI: C1 hex)
}
impl Report { pub fn is_healthy(&self, caps: &Capabilities) -> bool; }
pub struct ConnectorReading { pub index: u8, pub wires: [Option<f32>; 6] }   // amps
pub struct Verdict { pub index: u8, pub status: DeviceStatus, pub flagged: [bool; 6] }
pub enum DeviceStatus { Normal, OverCurrent, Imbalance, CriticalOverCurrent, Unknown(u8) }
pub struct Protection {
    pub enabled: bool,
    pub wire_limit: Option<f32>, pub imbalance_limit: Option<f32>,
    pub wire_trigger: Option<Duration>, pub imbalance_trigger: Option<Duration>,
    pub cutoff_after: Option<Duration>, pub hard_wire_limit: Option<f32>,
}
pub enum Fault { OverTemperature, FanFailure, OverPower, RailOverCurrent(&'static str), Other(String) }
pub struct ConnectorKey { pub source: SourceId, pub index: u8 }   // "msi:ai1300ts:1" in files
```

- `Option` everywhere a device may not provide a value. `core` must behave sensibly for every combination, so a future source can declare what it lacks instead of faking it.
- **Tick health (conservative), `Report::is_healthy`:** a tick is healthy only if `readings` is present **and**, for sources with `device_verdict`, `verdicts` is present. A missing verdict never reads as "Normal" (Spec §8.6). The acquisition thread and core use the same function.
- **Names are MeltAlarm's.** A source never names connectors for the user; core does ("GPU power cable 1", Spec §7.0). A vendor's own words reach the user only through `protection_name` and `Fault`.

### 4.2 Traits (`meltalarm-source-api`)

```rust
pub trait Driver: Send + Sync {
    fn name(&self) -> &'static str;                        // "MSI MPG Ai1300TS / Ai1600TS"
    fn discover(&self, hid: &mut HidContext) -> Discovery; // Found(Box<dyn Source>) | NotPresent | NotReady(reason) | Unusable(msg)
}
pub trait Source: Send {
    fn info(&self) -> &SourceInfo;
    fn poll(&mut self, hid: &mut HidContext) -> Report;    // blocking, bounded ≤ 3 s worst case
}
```

- `HidContext` owns the process's single `hidapi::HidApi` (hidapi allows one per process) and is shared by all drivers on the acquisition thread.
- The registry is a **compile-time list** chosen by the frontend's `main`. There are no plugins and no dynamic loading. An elevated process must never load third-party code (Q1).
- **Source authoring rules** (required for every source; reviewed in PRs):
  1. The source defines a closed request enum; one private function builds the bytes.
  2. The source has exactly one device-write call site (§9).
  3. No byte sent to a device may derive from settings, UI, CLI or IPC.
  4. Every transaction is bounded in time.
  5. Coexistence locks, if the vendor has any, are honored.
  6. Anything unavailable is reported as `None`, never as zero.
  7. The source ships golden-frame tests.
  8. **`discover` performs the vendor's complete connect sequence**, e.g. MSI's handshake. `Found` means the device has *answered*. A device that is present but not answering is `NotReady(reason)`, never `Found` or `NotPresent`.

### 4.3 MSI source (`meltalarm-source-msi`)

| MSI (Spec §1, F-numbers) | Model |
|---|---|
| PID `AA6F`/`808C` | Identifies a supported device (what MSI's software uses); `SourceInfo.id` `msi:ai1300ts` / `msi:ai1600ts` |
| `00 FA 51` handshake (F19) | Sent by `discover` on every connect, exactly as MSI Center's `CONNECT_PSU` does. It must be echoed, with byte 3 ≠ `FE`. |
| `0x11` model string | `SourceInfo.model`, **display only** (falls back to the PID's name if garbled) |
| `0x10`, `0x12`, `0x13` | vendor, firmware, serial |
| `0xE0` words 3–14 | `readings[0..2].wires` |
| `0xC1` status bytes | `verdicts[..].status` (0→Normal, 1→OverCurrent, 2→Imbalance, 3→CriticalOverCurrent, n→Unknown(n)) |
| `0xE1` wire flags / other flags | `verdicts[..].flagged` / `faults` |
| `0xC0` | `protection` (OCP→`wire_limit`, Diff→`imbalance_limit`, triggers, Warning_Time→`cutoff_after`); `hard_wire_limit = 18 A` (firmware constant) |
| raw `0xC1` frame | `diagnostic` (hex) |
| Capabilities, protection name | all `true`; `"Safeguard+"` |

**Internals:**
- `protocol` module (pure): `enum Request { Handshake, Read(Reg) }`, the closed set of 9 packets; `packet(Request)`, the only byte builder; reply classification per request (echo, length, busy rule); decoders; LINEAR11.
- `transport` module: the `Transport` trait over `HidDevice` (with a fake for tests), and `transact()` = lock → drain → write → echo-matched read (500 ms) → unlock.
- `lock` module:
  - `cfg(windows)`: `OpenMutexW(SYNCHRONIZE)`, falling back to `CreateMutexW`; 2 s wait; an abandoned mutex counts as acquired.
  - `cfg(not(windows))`: no-op, since MSI software doesn't exist there.
- **Connect** (`discover`): open → handshake → identity (`10`–`13`) → `Found`. An open or handshake failure gives `NotReady(reason)`; a missing lock gives `Unusable`.
- **Poll policy** (source-internal): E0, C1 and E1 every poll; C0 at start and every 60 s. No handshake per tick.
- **Linux note:** hidapi's hidraw backend works, and the frame parser already tolerates a present or stripped report ID. Non-root access needs a udev rule, which is Linux packaging, not code.

### 4.4 Simulated source (`meltalarm-source-sim`)

A `Driver` that replays a scenario chosen with `MELTALARM_SIM` (`cycle`, `normal`, `idle`, `caution`, `overload`, `uneven`, `red`, `alarm`, `critical`, `fault`, `nodata`, `silent`, `slow`). It sends nothing to any device. Frontends use it instead of the real drivers in their `simulate` build, so every alert path can be exercised on any machine. It is also the second implementation of the source API, which keeps the abstraction honest.

---

## 5. `meltalarm-core` — the brain (pure, sans-IO)

```rust
pub enum Event {
    SourceConnected(SourceInfo), SourcePending(String), SourceLost, Report(Report),
    User(UserAction),        // Snooze, TestAlarm, SetTracked(ConnectorKey, bool), SetAlarmEnabled(bool), SetRunAtStartup(bool), DismissNote(ConnectorKey)
    Suspended, Resumed, Wake, // Wake = a scheduled deadline passed
}
impl Core {
    pub fn new(settings: Settings) -> Self;
    pub fn restore(&mut self, state: CoreState);            // cable notes + "an alarm was open" from state.toml
    pub fn set_wall_clock(&mut self, wall: fn() -> String); // only to date-stamp cable notes
    pub fn handle(&mut self, ev: Event, now: Instant) -> Output;
    pub fn view(&self, now: Instant) -> ViewModel;          // pure function of state + now
}
pub struct Output { pub log: Vec<LogEvent>, pub settings_changed: Option<Settings>, pub state_changed: Option<CoreState>, pub next_wake: Option<Instant> }
```

- **No IO, no clock, no threads, no platform types.** Time comes in as `now`. Wall-clock strings for log lines are added by `runtime`. The one exception is an injected `fn() -> String` that date-stamps a cable note when it is created (tests inject a fixed one).
- `ConnectorKey = (SourceId, index)` keys everything per connector (tray icons, settings, notes, placement). Core holds **one source at a time**; a different source replaces the connectors.
- **State:**
  - per source: info, last `Protection`, faults
  - per connector (`Conn`): last reading, live evaluation, presence (last > 0 A), device verdict and its first-seen instant, the cable guard, the episode whose peak keeps the cable note current
  - health: consecutive unhealthy ticks; NO DATA since
  - alarm phase: `Idle | Active | Snoozed{until, snapshot} | Cleared{until, lasted, reason, label} | Test{until, strip_until, chime}`; a cause is `Overload | Device(status)`
  - the caution strip `{view, until}`, the latest notice
  - `CoreState` (persisted): cable notes per connector, the open-alarm flag
  - settings, with the resolved cable limits
- **Modules follow the spec:**

  | Module | Spec | Rule |
  |---|---|---|
  | `lib` | §5, §6.6–6.9 | Event intake: source connect/loss, reports (health, verdicts, faults, protection), user actions, time; NO DATA after 3 unhealthy ticks (after having had data → caution); first-run tracking; the wake schedule |
  | `limits` | §6.1 | The v1 defaults, file overrides, validation (all or nothing), the LIMITS log text |
  | `levels` | §6.2 | Live colors from the cable limits + median attribution; the device only adds `E1` flags and its imbalance status |
  | `guard` | §6.3–6.4 | Pure, sample-driven episodes: overload with hysteresis, caution, uneven load; gaps > 3 s break qualification; nothing fires without a fresh sample. Emits `GuardEvent`s |
  | `alarm` | §8.1–8.6 | Active iff any (connector, cause) is active and Alerts are on (D15); snooze and escalation; the 5 s green clear, never during NO DATA; the test alarm; the notch's content (`AlarmView`), the countdown (only with `cutoff_after`), the sound script |
  | `ladder` | §8.8–8.10 | Guard events → log lines, cable notes (severity order), the caution strip (once per episode, 10 s, one chime id, never during an alarm), advisory and after-alarm notices |
  | `view` | §7 | The `ViewModel`: connector views (names, chip, bars, live lines, tooltip, compact summary), health, the knee scale `bar_share` |
  | `text` | §7.0, §9 | Shared wording helpers: amps, durations, dates, names, the device's verdict and protection in words |
  | `log` | §9 | Log events and their line format |
  | `settings`, `state` | §7.3, §8.10 | The `key = value` file formats (parsing is pure; runtime does the IO) |

  A real alarm pre-empts a Test. A Test ends on Snooze (no re-show) or after 30 s, is never logged as an alarm and leaves no note.
- **ViewModel** is declarative and complete. Frontends only render and reconcile it.

  ```rust
  pub struct ViewModel {
      pub health: Health,                      // Starting | Connecting{since} | Live | Stale{age} | NoData{since}
      pub connectors: Vec<ConnectorView>,      // every connector, tracked or not: name, chip, wires[6]{amps, level, flagged},
                                               // imbalance (+level), bar_limit + bar_rating, glyph, attention marker,
                                               // live lines, cable note, PSU status, tooltip, compact summary
      pub source: Option<SourceView>,          // model, firmware, protection summary line, fault lines
      pub alarm: Option<AlarmView>,            // the notch: kind, band color, headline, action, details, right block, snooze
      pub audio: Option<AudioScript>,          // Some ⇔ the alarm should be sounding now
      pub settings: Settings,                  // for the menu / settings window
      pub connecting: Option<String>,          // tooltip for the placeholder icon while no source has connected yet
      pub notice: Option<Notice>,              // one-shot notification {id, title, text, warning, connector}
      pub caution: Option<CautionView>,        // the caution strip {chime, place, what, action, test}
  }
  ```

- **Every user-facing string is built here** (English): headlines, voice line, tooltip, notes, cable names, log lines. Every frontend (Windows notch, Linux notification) says the same thing, and the text is unit-tested. Vendor words come only from the source (`protection_name`, faults).
- **Presentation rules every frontend must share** live here too: the knee scale of the cut bars (`bar_share`, DESIGN.md "Cut bars"). Geometry, fonts and colors stay in the frontend.

---

## 6. `meltalarm-runtime` — portable host

- **Hosting model:** the frontend owns the `Runtime` on its UI thread and calls it. The runtime never calls UI code, apart from one injected `waker` (D1).

  ```rust
  pub struct Host { pub drivers: Vec<Box<dyn Driver>>, pub paths: Paths, pub wall_clock: fn() -> String, pub waker: Box<dyn Fn() + Send> }
  impl Runtime {
      pub fn start(host: Host) -> Runtime;             // spawns the acquisition thread
      pub fn pump(&mut self, now: Instant) -> Update;  // after a waker call: drain source events → core
      pub fn user(&mut self, a: UserAction, now: Instant) -> Update;
      pub fn wake(&mut self, now: Instant) -> Update;  // at Update.next_wake
      pub fn suspend(&mut self, now: Instant); pub fn resume(&mut self, now: Instant) -> Update;
      pub fn view(&self, now: Instant) -> Arc<ViewModel>;
  }
  pub struct Update { pub next_wake: Option<Instant>, pub lifecycle: Option<Lifecycle> } // DiscoveryFailed(msg) | Fatal(msg)
  ```

- **Acquisition thread:**
  1. **Discovery** (every 2 s until connected):
     - `Found` → `SourceConnected`
     - `NotReady(reason)` → `SourcePending(reason)`, retried **forever**
     - `Unusable(msg)` → `DiscoveryFailed(msg)` (the app exits)
     - Only if **no supported device was seen at all** for 2 min → `DiscoveryFailed` with the supported-device list from the drivers' names.
  2. Poll every 1000 ms on a drift-free deadline.
  3. After 3 unhealthy ticks (`Report::is_healthy`), drop the source and rediscover every 2 s; `discover` repeats the full connect sequence (e.g. the handshake). This is generic for all sources.
  4. Pause and resume on suspend.
  5. `catch_unwind` at the thread boundary → `Fatal`.
- **Watchdog:** `next_wake` is never later than last report + 5 s, so a stalled thread produces a Stale view (Spec §5.4).
- **Files** (`write_atomic`: temp file, then rename; a failure never stops monitoring):
  - `paths.config/settings.toml`: a hand-parsed `key = value` subset. Tracked connectors are saved by `ConnectorKey` (`msi:ai1300ts:1`). Cable-limit overrides (`limit_*`) are written only when set.
  - `paths.config/state.toml`: the cable notes and the open-alarm flag (Spec §8.10). Loaded into `core.restore()` at start, written on `Output.state_changed`. It is state, not settings, so it lives in its own file.
  - `paths.log/alarms.log`: `LogEvent::format(wall)` comes from `core`; the sink appends and rotates at 5 MB (Spec §9).
- `Paths`, the wall clock and the waker are injected, so the crate has no OS dependencies:

  | Injected | Windows | Linux |
  |---|---|---|
  | `Paths` | `%APPDATA%` / `%LOCALAPPDATA%` | XDG |
  | wall clock | `GetLocalTime` | `localtime_r` |
  | waker | `PostMessage` | an eventfd, or the toolkit's wake mechanism |

---

## 7. `meltalarm-win` — Windows frontend

**Main loop:** a Win32 message loop on the UI thread. `WM_WAKE` (from the waker) → `runtime.pump`. A one-shot `SetTimer` at `next_wake` → `runtime.wake`. After every call, **reconcile** the screen with the view:

| Reconciler | Rule (derived only from the ViewModel) |
|---|---|
| Tray | One icon per tracked connector, icon id = the connector's number (Windows remembers "show on taskbar" per id); glyph and tooltip from `ConnectorView`; blink timer (500 ms) while any glyph is `Alarm`. While `connecting` is set and there are no connectors yet: one hollow placeholder icon. |
| Notice | A tray notification the first time each `notice.id` appears (once an icon exists); silent unless `warning`; a click opens `notice.connector`'s view |
| Flyout | If open, redraw |
| Floating | A window exists iff its placement says floating, the connector is tracked and in the view; redraw each |
| Notch | Visible on every monitor iff `alarm.is_some()` |
| Strip | Visible on every monitor iff `caution.is_some()`; redrawn only when its content or the monitors change |
| Chime | Played once when `caution.chime` differs from the last one played |
| Audio | Player running iff `audio.is_some()`; restarted if the script changed |
| Hotkey | Ctrl+Alt+G registered iff the notch is visible and snooze is allowed |
| Autostart | **Installed copy only** (§7.1): the Task Scheduler task made to match `settings.run_at_startup`, also repairing its target. A portable copy never touches it. A failure sends `SetRunAtStartup(<actual state>)` and logs a warning, so the menu always shows reality. |

**Modules:**

| Module | Responsibility |
|---|---|
| `main` | Single instance, composition (`Host` with the MSI driver, or the simulator), the `App` state, `reconcile`, window procedures, deferred work outside the `App` borrow (menus and dialogs re-enter the window procedure) |
| `menu` | The tray menu; until the Settings window exists it holds the settings as checkmarks |
| `tray` + `glyph` + `paint` | `Shell_NotifyIconW` v4, `TaskbarCreated` (allowed through UIPI), DPI-sized ARGB glyphs drawn on the CPU; `paint` is shared with `build.rs` for the app icon |
| `gfx` | Direct2D + DirectWrite in software mode into layered windows; text measurement; a small painter API |
| `palette` | Color tokens (DESIGN.md "State colors"), the ambient theme (light/dark), level colors |
| `card` | The connector card: header and chip, cut bars, live lines, cable note, footer; shared by the flyout, the floating view and (cut bars) the notch |
| `popup` | The flyout window: anchoring above its icon, hit areas |
| `floating` + `placement` | Floating views: own mouse capture for move and uniform scale, hover controls, locate pulse; `window.toml` and display identity (§7.2) |
| `edge` | One window per monitor fused to the top edge, recreated when monitors change; used by `overlay` (the notch) and `strip` (the caution strip) |
| `overlay`, `strip` | The notch (snooze button, hotkey hint) and the click-through caution strip |
| `audio` | The alarm thread: `PlaySoundW` file + SAPI `ISpVoice` executing the `AudioScript`; the caution chime synthesized once in memory (`SND_MEMORY \| SND_ASYNC`) |
| `autostart`, `lifecycle` | Task Scheduler XML; install, update, uninstall and the control window (§7.1) |
| `sys` | Paths, wall clock, theme, message boxes, shell |

**Not built yet (v1):** the Settings window (then the menu shrinks to Settings… / Open alarm log / Exit), motion (DESIGN.md "Motion"), the Windows 10 pass.

**Threads:** UI (frontend + runtime + core), acquisition (runtime), and audio (only while sounding). Memory rule: no render target is kept between frames; SAPI lives only on the audio thread.

### 7.1 Program lifecycle (Spec §4)

The same split as sources: **what** to do is portable and tested everywhere; **how** is per OS.

**`meltalarm-lifecycle` (portable, std only, no OS calls):**

```rust
pub enum Policy { SelfManaged, PackageManaged }          // Spec L1; chosen by the frontend (Windows / Linux)
pub enum Flag { None, Portable, Install, Uninstall }      // command line
pub enum Instance { None, SameFile, OtherFile }           // a MeltAlarm already running, relative to this file
pub struct Facts { pub policy: Policy, pub flag: Flag, pub this_version: Version, pub this_is_installed: bool,
                   pub installed: Option<Version>, pub running: Instance }
pub enum Launch {
    Monitor { portable: bool },            // normal start; portable → no autostart, "Install…" offered
    HandOff,                               // L6: bring the running instance forward, exit
    OfferInstall,                          // §4.5
    OfferUpdate { from: Version, to: Version },
    OfferReplace { from: Version, to: Version },  // downgrade = rollback
    StartInstalled,                        // same version: hand off to the installed copy
    Uninstall, NotInstalled,               // §4.7
}
pub fn decide(f: &Facts) -> Launch;       // Spec §4.4, table-driven tests; PackageManaged → Monitor/HandOff only

pub trait Step { fn name(&self) -> String; fn apply(&mut self) -> Result<(), String>; fn undo(&mut self) {} }
pub fn run_atomic(steps: &mut [Box<dyn Step>]) -> Result<(), Failed>;  // install/update: undo done steps in reverse
pub fn run_best_effort(steps: &mut [Box<dyn Step>]) -> Vec<Failed>;    // uninstall: keep going, report leftovers

pub trait Autostart { fn target(&self) -> Option<PathBuf>; fn set(&self, target: Option<&Path>) -> Result<(), String>; }
pub trait Instances { fn running(&self) -> Option<Box<dyn Running>>; }
pub trait Running { fn path(&self) -> Option<PathBuf>; fn show(&self); fn alarm_active(&self) -> Option<bool>;
                    fn stop(&self, grace: Duration) -> Result<(), String>; }
```

- **Plans** (ordered step lists) are built by the frontend from its own `Step`s; the runner, the ordering rules and the rollback are shared.
- **Log lines:** `LogEvent::Installed`, `Updated` and `MonitoringStopped` (the gap). A process that never starts monitoring (installer, `--uninstall`) writes the same file through `runtime::LogSink`.
- **Version:** `CARGO_PKG_VERSION`, also embedded as the exe's VERSIONINFO (by `build.rs`) so the installed copy's version is read from the file itself, without running it.

**Windows backend (`meltalarm-win::lifecycle`, `Policy::SelfManaged`):**

| Piece | Implementation |
|---|---|
| Installed copy | `FOLDERID_ProgramFiles\MeltAlarm\meltalarm.exe`; version via `GetFileVersionInfoW` |
| Same file? | Final paths (`canonicalize`), so case, 8.3 names and links don't matter |
| Alarm gate | Before install or update: `Running::alarm_active() == Some(true)` refuses ("A cable alarm is active…") |
| Install plan | StopRunning → PlaceProgram (write `meltalarm.exe.new`, rename into place; an existing copy is kept as `.old` until done) → Shortcut (`IShellLinkW`, `FOLDERID_CommonPrograms`) → AppsEntry (`HKLM\…\Uninstall\MeltAlarm`: name, version, publisher, icon, size, `UninstallString = "…\meltalarm.exe" --uninstall`) → Startup (task + the stored setting, together). Then the installed copy is started with `--installed`. |
| Update plan | StopRunning → PlaceProgram → AppsEntry version → start. The next start of the installed copy deletes `.old`. |
| Uninstall plan | StopRunning → task off → shortcut → AppsEntry → program folder → data folders (if asked). The uninstaller *is* the installed exe: it renames itself out of the folder (allowed for a running image on the same volume), marks that file `MoveFileExW(DELAY_UNTIL_REBOOT)`, and removes the folder. |
| Autostart | Task Scheduler XML via `schtasks.exe`, target = the installed exe only, "highest privileges" at logon |
| Instances | Single-instance mutex. The hidden main window (class `MeltAlarmMain`; simulated builds `MeltAlarmSimMain`) doubles as the **control window**: the registered message `MeltAlarm.Control` with SHOW / ALARM / QUIT via `SendMessageTimeoutW`. UIPI lets only elevated processes send it. `stop` = QUIT, wait on the process handle for 5 s, then `TerminateProcess`. |
| Elevation | All of it runs in the already-elevated process (manifest), so Program Files and HKLM need no extra prompt. |
| Flags | `--portable` (dev, CI), `--install` (tray *Install…* spawns this), `--uninstall` (the Installed apps entry), `--installed` (set by the installer: show the pin-the-icon hint once). `simulate` builds skip the lifecycle entirely. |

**Linux backend (next iteration, `Policy::PackageManaged`):** no `Step`s at all: the package installs the binary, the `.desktop` file and the udev rule. `Autostart` = an XDG autostart entry pointing to the packaged binary; `Instances` = a D-Bus name or a socket in `$XDG_RUNTIME_DIR`. `decide` then only ever returns `Monitor { portable: false }` or `HandOff`.

### 7.2 Floating monitor (Spec §7.4)

- **Core supplies the words, the frontend the geometry.** `ConnectorView` carries `status_text`, `summary` and `summary_level` for the compact layout (D8). Where a view sits is frontend state (D14).
- **Placement file** `window.toml` in the config folder, owned by the `placement` module: per `ConnectorKey`: floating, display id, position in DIPs relative to that display's work area, layout, a scale per layout. Written on every change (end of a drag or scale, layout switch, ×), read at start. Unknown or broken lines and unknown connectors are ignored.
- **Display identity:** the monitor's device interface name (`EnumDisplayDevicesW(…, EDD_GET_DEVICE_INTERFACE_NAME)`), stable across reboots, unlike `\\.\DISPLAYn`. Missing → the primary display. Always clamped fully inside a work area.
- **Window:** layered, `WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`; `WM_MOUSEACTIVATE → MA_NOACTIVATE` (as the notch). Drawn with `gfx.present` at `dpi × scale`, using the `card` module for *Full*. Move and scale use our own mouse capture, not the system move/size loop: the loop would activate the window and can't keep the aspect ratio. Hit zones: a 6 DIP edge band scales; the ×, tab and grip are buttons; everything else moves. While scaling, every redraw (the 1 Hz data update too) uses the scale being dragged to, anchored at the opposite corner. Hover by `TrackMouseEvent`.
- **Reconcile:** see §7. The flyout and the floating view of one connector are mutually exclusive; a tray click on a floating connector *locates* it.
- **Tear-off:** a press on the flyout's header hides the flyout, creates the floating view at the same spot and hands it the drag (capture moves to the new window).
- **Footprint:** measured 13–15 MB private with a floating view, the same as an open flyout: the Direct2D/DirectWrite stack, loaded on the first text draw. Target ≤ 20 MB and flat (Spec §10).

---

## 8. Key flows

```
Launch:   main → lifecycle::decide(facts) → Monitor → Startup below | HandOff → Running::show → exit
          | Offer* → dialog → run_atomic(plan) → start installed copy → exit | Uninstall → run_best_effort → exit
Startup:  main → single instance → Runtime::start (settings + state loaded) → acquisition discovers every 2 s
          ├─ found (answered, incl. handshake) → SourceConnected → first run picks tracked connectors → tray icons
          ├─ present, silent → SourcePending → hollow "connecting…" icon; retried forever; notice + log after 2 min
          ├─ unusable (lock inaccessible) → Lifecycle::DiscoveryFailed → message → exit
          └─ no supported device for 2 min → Lifecycle::DiscoveryFailed → message "only MSI MPG Ai1300TS/Ai1600TS…" → exit
Tick:     acquisition poll → channel + waker → pump → core.handle(Report) → view → reconcile (≈1 ms)
Alarm:    guard overload or verdict ≠ Normal → Phase::Active → view.alarm + view.audio → notch shown, player
          started, hotkey registered; Output.log = [OVERLOAD | PSU ALARM, PSU RAW] → sink; note + alarm_open → state.toml
Caution:  guard caution qualifies (or NO DATA after data, or a new fault) → view.caution{chime+1} → strip on every
          monitor + one chime → wake at +10 s → strip gone; note → state.toml
Advisory: guard uneven load qualifies → view.notice{silent, connector} → tray notification (Windows holds it in games)
Restart:  state.toml has alarm_open → core.restore → notice "Last session ended during a cable alarm" → flag cleared
Snooze:   click/hotkey → runtime.user(Snooze) → Snoozed{+30 s}, next_wake → view without alarm/audio
          → wake: still active → Active again; escalation on any tick → Active immediately
Clear:    no cause left (guard: every wire < rating; device: Normal) → Cleared{5 s} (green notch, audio stops) → wake → Idle
Lost:     3 unhealthy ticks → NoData view (grey); alarm, if any, stays with "cannot confirm"
```

---

## 9. Read-only contract — enforcement (Q1)

1. **Per source**, a closed request enum and a single private packet builder. A unit test enumerates every variant. For MSI that is exactly 9 packets: `[00, 51, reg, 0…]` with reg ∈ {10, 11, 12, 13, C0, C1, E0, E1}, plus the connect handshake `[00, FA, 51, 0…]`.
2. **Workspace-wide** `clippy.toml` `disallowed-methods`, every hidapi call that sends bytes to a device:
   - `hidapi::HidDevice::write`
   - `send_output_report`
   - `send_feature_report`
   - `get_feature_report` (its request also travels to the device)
3. Exactly one `#[allow(clippy::disallowed_methods)]` per source crate, at its `transport` write.
4. *Planned with CI (v1):* CI counts those `#[allow]`s and fails if they don't equal the number of hardware source crates.
5. No frontend or runtime crate depends on `hidapi` directly. Only `source-api` (the context) and source crates do.
6. `psu-probe` (v1) uses `source-msi`'s public API only.

---

## 10. Error handling, testing, build

**Failure isolation:**

| Failure | Effect |
|---|---|
| Source error | The tick is unhealthy (a view change, not a crash) |
| Acquisition panic | `Fatal` → logged `STOPPED` + message |
| Audio or voice failure | The notch is still shown |
| Notch failure | Audio still plays |
| Autostart failure | The menu item shows the real state; a warning is logged |
| File write failure (settings, state, placement, log) | Ignored; monitoring continues |
| Install / update step failure | Done steps undone in reverse; error dialog names the step; this copy runs portable (install) or the previous version keeps running (update) |
| UI thread panic | Panic hook logs the same line |

**Testing:**
- `model`/`core`: table-driven synthetic feeds for every Spec §6/§8/§9 rule, including sources with missing capabilities (e.g. no `cutoff_after` → no countdown); the cable guard (T22); the texts users see.
- `source-msi`: golden frames from the reference captures (serial scrubbed), plus the fake `Transport` (foreign frames, stale same-register frames, busy, timeout — F17).
- `runtime`: fake drivers through the real acquisition thread, temp-dir files.
- `lifecycle`: the Spec §4.4 table for both policies, version ordering, and rollback order with fake `Step`s that fail at each position. Portable, so it runs on Linux CI too.
- `win`: the program file replacement step (fresh, update, failure rollback) in temp folders, and the placement file format. Run with `--features simulate`: the real manifest makes the test binary require elevation.
- **On screen:** the `simulate` build (`meltalarm-source-sim`) exercises every surface with no device traffic. Visual changes are checked there at real size (DESIGN.md).
- Acceptance T1–T28 (Spec §11) run manually.

**Build:**
- `x86_64-pc-windows-msvc`, static CRT.
- `build.rs` embeds the manifest (`requireAdministrator` because of the MSI mutex, `asInvoker` for `simulate`; PerMonitorV2, Common Controls v6, Windows 10/11), the app icon (drawn per size with `paint.rs`) and VERSIONINFO.
- *Planned (v1):* CI on Windows (fmt, clippy for both builds, tests, the read-only count, release + SHA256SUMS) **plus Linux** (`cargo test` for model, core, source-api, source-msi, source-sim, runtime, lifecycle), which keeps the portable crates portable before any Linux frontend exists.
- **Repo:** MIT; `docs/`; the research material (PSU.dll, raw captures, original notes) stays local and git-ignored.

---

## 11. Extension paths (designed for, not built)

### 11.1 Linux frontend
- **Reused unchanged:** model, core, source-api, source-msi, source-sim, runtime, lifecycle (`Policy::PackageManaged`).
- **New crate:**

  | Area | Plan |
  |---|---|
  | Tray | StatusNotifierItem (`ksni`) |
  | Alarm | Critical desktop notification; layer-shell overlay where the compositor supports it (KDE, wlroots; not GNOME) |
  | Caution | A normal-urgency notification with the chime |
  | Sound and voice | PipeWire + speech-dispatcher, executing the same `AudioScript` |
  | Bars | The same knee scale (`core::bar_share`) and texts; its own drawing |
  | Autostart | XDG autostart entry (`~/.config/autostart`) via `lifecycle::Autostart` |
  | Install, update, uninstall | Distro package / AppImage (Spec L1); the package ships the udev rule |
  | Single instance | `lifecycle::Instances` over D-Bus or a `$XDG_RUNTIME_DIR` socket |
  | Device access | udev rule, **no root** |
  | Paths | XDG |

- **Open before starting:** verify HID framing on real hardware with `psu-probe`, and check overlap with Railwatch.

### 11.2 A new sensing device
1. Add a `meltalarm-source-<vendor>` crate implementing `Driver`/`Source` under the §4.2 rules, with golden frames.
2. Add it to the frontend's driver list.
3. Nothing else changes. A device **without** a verdict of its own still gets the full ladder from the cable guard (D15); one with a verdict gets both judges. Its protection's name comes from `protection_name`.

Known limits for that day, deliberately not built yet (YAGNI):
- Core holds **one source at a time**; a PC with two sensing devices (e.g. a PSU and a GPU that both measure) would need connectors from several sources in one view.
- The texts say "PSU" for the device ("PSU status", "reported by the PSU"). A GPU-side source would need the device's noun in `SourceInfo`.

---

## 12. Key decisions

| # | Decision | Alternatives rejected | Why |
|---|---|---|---|
| D1 | Sans-IO `core` hosted by a portable `runtime` on the frontend's UI thread | A dedicated engine thread; core inside the frontend | Deterministic and testable; one fewer thread; the same host serves Windows and Linux |
| D2 | Vendor-neutral model + compile-time `Driver`/`Source` traits | MSI types in core; runtime plugins | Q4 without a rewrite. Plugins would load third-party code into an elevated process (Q1). |
| D3 | *(Superseded by D15.)* The device verdict was the only alarm authority | — | Research showed the PSU's policy is too slow for the contact (Spec F20, F21) |
| D4 | Declarative ViewModel; frontends reconcile | Imperative effect commands | No drift between "should" and "is" (notch, audio, hotkey, tray). Idempotent after any hiccup. |
| D5 | Native frontend per OS | Cross-platform UI toolkit | The hard parts (tray, overlay over games, autostart, audio) are OS-specific anyway; toolkits cost the footprint (Q3). |
| D6 | One elevated process on Windows | Service + user helper | Admin is sufficient (F3); half the moving parts |
| D7 | Direct2D/DirectWrite in software mode | GPU rendering | No GPU driver in the process; tiny surfaces |
| D8 | User-facing text built in `core` | Text per frontend | One tested source of wording across OSes and channels (notch, voice, notification) |
| D9 | MSI handshake sent on **every connect**, never per tick | Only after reads time out ("minimal touch") | Mirrors the vendor's own clients exactly; a failure-driven path would be a rarely-exercised branch where bugs hide. Repeated handshakes are routine in MSI's ecosystem. |
| D10 | A present but silent device is retried forever; exit only when no supported device exists | Give up after a timeout | A safety monitor must not quit because the PSU is slow to answer. |
| D11 | Windows: one self-installing exe | MSI/WiX or Inno Setup installer; portable only | No second toolchain; one file to verify; an installer would still need custom steps for the elevated task. |
| D12 | Lifecycle = portable decisions (`meltalarm-lifecycle`) + per-OS steps; Linux delegates to the package manager | Lifecycle code only inside `meltalarm-win`; self-install on Linux too | Same pattern as sources; the decision table is tested on Linux CI; Linux users expect packages. |
| D13 | Autostart only ever targets the protected installed copy | Task follows `current_exe()` | Otherwise a user-writable file starts elevated at logon (privilege escalation). |
| D14 | Window placement is frontend state in its own file (`window.toml`) | In core `Settings` | Where a window sits is a per-OS, per-display detail; core decides what is shown, not where. |
| D15 | **One alarm, two judges:** the cable guard (our limits from the connector's physics) **or** the device's verdict | Device verdict only (D3); a thermal I²t model; the research brief's rules as written | The PSU's 20 s + 180 s is too slow (F21). An I²t model at 1 Hz is either slow or a disguised filter and hard to explain in a log line. The brief's rules restart on every dip; hysteresis fixes it. Works for sources without a verdict. |
| D16 | Cable limits are core constants (versioned "limits v1"), overridable only in `settings.toml`, all or nothing | A Settings-window control; limits from the device (`C0`) | The defaults come from physics, not taste; a control mostly invites loosening a safety alarm. Only overrides are written, so default improvements reach everyone. |
| D17 | The alert ladder is declarative: `ViewModel.alarm`, `.caution` (with a chime id), `.notice`; cable notes and "alarm open" in `CoreState` | Imperative "play chime / show toast" commands | Same as D4. Notes persist because the PSU's power cut also kills MeltAlarm. |
| D18 | **Vendor words come from the source.** Core's texts never name a vendor feature or a vendor's limit; the source supplies `protection_name` and the limits it enforces | MSI wording in core | A second vendor must not need core changes (Q4). |
| D19 | **Presentation rules shared by frontends live in core** (the knee scale), geometry and drawing in the frontend | The rule duplicated per frontend | One tested rule; Linux draws the same bars without re-deriving them. |
| D20 | **Modules follow the spec's sections** (core: `levels`, `guard`, `alarm`, `ladder`, `view`, `text`; win: one module per surface, shared `card`, `palette`, `edge`) | Files grown by history | A spec change lands in one place; reviewers find the code for a spec section by name. |

---

## 13. Risks

| # | Risk | When | Fallback |
|---|---|---|---|
| R1 | Notch or strip not visible over some borderless or flip-model games, or steals focus | T23, real games | Audio is guaranteed; document the titles |
| R2 | Idle memory above 5 MB after windows close | T9 | Release resources; trim the working set |
| R3 | Linux HID framing (64/65 bytes) | Linux iteration | Parser already tolerant; verify with the probe |
| R4 | Defender/SmartScreen flags an unsigned exe that copies itself to Program Files and creates an elevated logon task | v1, test with Defender on | Plain dialog wording, SHA256SUMS; report false positives to Microsoft; code signing later |
| R5 | `E0` is a held peak, not an instantaneous value (Spec §12) | The read-only probe, with the user's go-ahead | Revalidate the one-reading 15 A rule and the persistence rules |
| R6 | The cable-limit defaults raise false alarms in real games | T28 | A later limits version; overrides for advanced users meanwhile |

---

## 14. Status and roadmap

**Done** (0.1–0.3):
- the read-only MSI source with the connect handshake; coexistence with MSI Center, Afterburner and HWiNFO
- core, runtime, tray, flyout, floating view, notch, sound and voice, snooze and test
- the program lifecycle: install, update, uninstall, autostart
- the cable guard and the alert ladder: caution strip and chime, advisories, cable notes
- the simulator crate

**v1 — GitHub release:**
- Settings window (DESIGN.md "Settings"); the menu shrinks
- motion (DESIGN.md "Motion"); a Windows 10 pass
- `psu-probe`; `docs/PROTOCOL.md`; README (with the GPU power cable / 12V-2x6 / 16-pin bridge and the `limit_*` keys)
- CI (Windows + Linux crates), release with SHA256SUMS
- acceptance: T3 coexistence run, T14–T21 lifecycle and floating, T23–T28 on real games

**Next iteration:** the Linux frontend (§11.1); new sources on demand (§11.2).

---

## 15. Self-review

| Criterion | Assessment |
|---|---|
| Consistent with the spec | Every Spec §5–§9 rule has exactly one home (source-msi: §5; core: §6, §8, §9, by module; runtime: §5.4 reconnect/watchdog, §9 file; win: §4, §7, §8 rendering and playback). Spec facts F3, F13, F17 are reflected in lock, audio and transport. |
| Single responsibility, clear boundaries | Each crate has one reason to change: vocabulary, decisions, device family, hosting, lifecycle, OS. Within core and win, modules follow spec sections and surfaces (D20). |
| Dependency direction | Stable → volatile, verified in §2. No cycles. `core` has zero dependencies beyond `model`. |
| Safety invariant is structural | Closed enums + single write site + the full hidapi deny-list + no `hidapi` outside sources (§9). The CI count is still to come (v1). |
| Testability | Everything that decides is pure and clock-injected; every IO edge has a fake (Transport, Driver, Paths, wall clock); the simulator covers the screens. |
| Extensibility without speculation | Two seams (Source, Frontend), both driven by stated plans; vendor words from the source (D18). The known limits for a second device are listed (§11.2), not built. |
| Failure isolation | §10: every non-core failure degrades a channel, never monitoring or the alarm decision. |
| Footprint | Two permanent threads, software rendering, resources scoped to visibility. |
| **Known trade-offs** | (1) Menu-based settings until v1. (2) Frontends must implement reconciliation correctly (mitigated: small and declarative). (3) `requireAdministrator` applies to the whole app because of the MSI mutex. (4) The cable limits are engineering defaults (Spec §8.7). |

---

## 16. Document history

| Version | Date | Changes |
|---|---|---|
| 3 | 2026-10-01 | Rewritten to describe the system as built after the architecture review: core modules by spec section, the simulator crate, the Windows modules (`card`, `palette`, `edge`, `menu`), vendor words from the source (D18–D20), the full hidapi deny-list, status and roadmap |
| 2.4 | 2026-10-01 | The cable guard (D15–D17), the alert ladder, cable notes and `state.toml` |
| 2.3 | 2026-09-30 | Floating monitor (§7.2) |
| 2.2 | 2026-09-28 | Program lifecycle split into a portable crate plus per-OS backends |
| 2.1 | 2026-09-28 | Connect handshake, `Discovery::NotReady`, a startup that never gives up on a present device |
| 2 | 2026-09-27 | Vendor-neutral model, portable runtime, sans-IO core |
