# MeltAlarm — Software Architecture v2

**Status:** v2.1 · **Date:** 2026-09-28 · **Supersedes:** v2 (2026-09-27) · v2.2: program lifecycle (install, update, uninstall, autostart) split into a portable crate plus per-OS backends (Spec v2.2 §4, §7.1) · v2.1: connect handshake, `Discovery::NotReady`, a startup that never gives up on a present device (Spec v2.1, F19) · **Inputs:** `FUNCTIONAL_SPEC.md`, `DESIGN.md`

**What changed from v1:**
- The alarm logic no longer speaks "MSI". It works on a **vendor-neutral model**, so other devices can be added later.
- Everything except the UI shell is **portable** (Windows today, Linux next).
- `core` is hosted "sans-IO" by a portable `runtime`, and frontends only render a declarative view.
- A **v0 tonight** cut is defined.

---

## 1. Priorities and quality attributes

**Delivery order:**
1. **v0, tonight:** a working app on the reference PC.
2. **v1:** a GitHub-ready Windows release.
3. **Next iteration:** Linux, and other sensing devices.

**Quality attributes, highest first.** When two conflict, the higher one wins.

| # | Attribute | Meaning here |
|---|---|---|
| Q1 | **Hardware safety** | Nothing but allowlisted reads ever reaches a device. This is structural, not a convention. |
| Q2 | **Alarm reliability** | A device alarm reaches the user within 2 s, never clears without data, and survives any single component failure (voice, overlay, task…). |
| Q3 | **Footprint** | ≤ 5 MB private idle, ~0 % CPU, no GPU driver in the process |
| Q4 | **Evolvability** | New device family = new source crate. New OS = new frontend crate. `core` untouched in both cases. |
| Q5 | **Delivery speed** | v0 tonight: no speculative machinery, only seams that Q4 needs |

---

## 2. Architecture at a glance

```
                        ┌──────────────────────────────────────────┐
  frontends (per OS)    │ meltalarm-win (bin)     meltalarm-linux  │   render view, run platform effects
                        │ Win32/D2D/SAPI/Tasks    (next iteration) │   composition root (picks drivers)
                        └────────────────┬─────────────────────────┘
                                         │ ViewModel ▲ │ UserAction ▼
                        ┌────────────────▼─────────────────────────┐
  host (portable)       │ meltalarm-runtime                        │   acquisition thread, discovery,
                        │                                          │   reconnect, settings file, alarm log
                        └───────┬─────────────────────────┬────────┘
                                │ Event ▼ / Output,View ▲ │ Driver/Source traits
                        ┌───────▼────────┐       ┌────────▼─────────────────┐
  logic (pure)          │ meltalarm-core │       │ meltalarm-source-api     │  traits + shared HID context
                        └───────┬────────┘       └────────┬─────────────────┘
                                │                         │ implemented by
                        ┌───────▼─────────────────────────▼───┐   ┌───────────────────────────┐
  vocabulary (pure)     │ meltalarm-model                     │◄──│ meltalarm-source-msi      │  MSI Ai1300TS/Ai1600TS
                        └─────────────────────────────────────┘   │ (future: source-<vendor>) │
                                                                  └───────────────────────────┘
```

Beside this stack, `meltalarm-lifecycle` (portable, std only) decides what a launch does and runs install/update/uninstall steps that each frontend supplies (§7.1).

**Dependency rule:** arrows point toward stability. `model` depends on nothing. `core` depends only on `model`. Sources know `model` + `source-api`, never `core`. `runtime` knows `core` + `source-api`, never a concrete source. Only a frontend's `main` (the composition root) names concrete drivers.

---

## 3. Crates

| Crate | Kind | Depends on | Portable | Responsibility |
|---|---|---|---|---|
| `meltalarm-model` | lib, pure, `forbid(unsafe)` | — | ✅ | Neutral vocabulary: readings, verdicts, protection, faults, source info, capabilities (§4.1) |
| `meltalarm-core` | lib, pure, `forbid(unsafe)` | model | ✅ | All decisions: levels, attribution, presence, health, alarm state machine, snooze, countdown, log events, settings model, **ViewModel and all user-facing text** (§5) |
| `meltalarm-source-api` | lib | model, hidapi | ✅ | `Driver` / `Source` traits, `HidContext` (one shared `HidApi`), source authoring rules (§4.2) |
| `meltalarm-source-msi` | lib | model, source-api, hidapi, windows-sys (cfg windows) | ✅ (lock is cfg) | MSI protocol: closed request set, frame decoding, transactions, `Global\MSI_PSU_Mutex` (Windows), mapping to the model (§4.3) |
| `meltalarm-runtime` | lib | core, source-api | ✅ | Portable host: acquisition thread, discovery and reconnect, settings persistence, log sink, timers and wake-ups (§6) |
| `meltalarm-lifecycle` | lib, std only | — | ✅ | Program lifecycle, OS-free: launch decision (Spec §4.4), version compare, all-or-nothing step runner, the `Autostart` / `Instances` / `Step` traits (§7.1) |
| `meltalarm-win` | bin `meltalarm` | runtime, core, lifecycle, source-msi, windows | ❌ Windows | Tray, menu, popup, overlay, audio, voice, hotkey, power events, and the Windows lifecycle backend: install steps, Task Scheduler autostart, single instance + control window (§7, §7.1) |
| `psu-probe` (tools/) | bin | source-msi | ✅ | Read-only diagnostics, serial redacted, for hardware reports and porting (v1) |
| *future* `meltalarm-linux` | bin | runtime, core, sources | Linux | §11.1 |
| *future* `meltalarm-source-<vendor>` | lib | model, source-api | per device | §11.2 |

Names carry the `meltalarm-` prefix. A crate named `core` would shadow Rust's built-in `core`.

---

## 4. The source abstraction

### 4.1 Neutral model (`meltalarm-model`)

A *source* is anything that measures per-wire current on 12V-2x6 connectors: today a PSU; later possibly a GPU, or an inline meter.

```rust
pub struct SourceInfo {
    pub id: SourceId,                 // stable, e.g. "msi:ai1300ts" — keys settings
    pub vendor: String, pub model: String,
    pub firmware: Option<String>, pub serial: Option<String>,
    pub connectors: Vec<ConnectorInfo>,   // index + label, e.g. (0, "12V-2x6 #1")
    pub caps: Capabilities,
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
pub struct ConnectorReading { pub index: u8, pub wires: [Option<f32>; 6] }   // amps
pub struct Verdict { pub index: u8, pub status: DeviceStatus, pub flagged: [bool; 6] }
pub enum DeviceStatus { Normal, OverCurrent, Imbalance, CriticalOverCurrent, Unknown(u8) }
pub struct Protection {
    pub enabled: bool,
    pub wire_limit: Option<f32>, pub spread_limit: Option<f32>,
    pub wire_trigger: Option<Duration>, pub spread_trigger: Option<Duration>,
    pub cutoff_after: Option<Duration>, pub hard_wire_limit: Option<f32>,
}
pub enum Fault { OverTemperature, FanFailure, OverPower, RailOverCurrent(&'static str), Other(String) }
```

- `Option` everywhere a device may not provide a value. `core` must behave sensibly for every combination, so a future source can declare what it lacks instead of faking it.
- **Tick health (conservative):** a tick is *healthy* only if `readings` is present **and**, for sources with `device_verdict`, `verdicts` is present. A missing verdict never reads as "Normal" (Spec §8.6).

### 4.2 Traits (`meltalarm-source-api`)

```rust
pub trait Driver: Send + Sync {
    fn name(&self) -> &'static str;                        // "MSI MPG Ai1300TS / Ai1600TS"
    fn discover(&self, hid: &mut HidContext) -> Discovery; // Found(Box<dyn Source>) | NotPresent | NotReady(reason) | Unusable(msg)| NotPresent | Unsupported(String)
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
| `0xC0` | `protection` (OCP→`wire_limit`, Diff→`spread_limit`, triggers, Warning_Time→`cutoff_after`); `hard_wire_limit = 18 A` (firmware constant) |
| raw `0xC1` frame | `diagnostic` (hex) |
| Capabilities | all `true` |

**Internals:**
- `protocol` module (pure): `enum Request { Handshake, Read(Reg) }`, the closed set of 9 packets; `packet(Request)`, the only byte builder; reply classification per request (echo, length, busy rule); decoders; LINEAR11.
- `transport` module: the `Transport` trait over `HidDevice` (with a fake for tests), and `transact()` = lock → drain → write → echo-matched read (500 ms) → unlock.
- `lock` module:
  - `cfg(windows)`: `OpenMutexW(SYNCHRONIZE)`, falling back to `CreateMutexW`; 2 s wait; an abandoned mutex counts as acquired.
  - `cfg(not(windows))`: no-op, since MSI software doesn't exist there.
- **Connect** (`discover`): open → handshake → identity (`10`–`13`) → `Found`. An open or handshake failure gives `NotReady(reason)`; a missing lock gives `Unusable`.
- **Poll policy** (source-internal): E0, C1 and E1 every poll; C0 at start and every 60 s. No handshake per tick.
- **Linux note:** hidapi's hidraw backend works, and the frame parser already tolerates a present or stripped report ID. Non-root access needs a udev rule, which is Linux packaging, not code.

---

## 5. `meltalarm-core` — the brain (pure, sans-IO)

```rust
pub enum Event {
    SourceConnected(SourceInfo), SourcePending(String), SourceLost, Report(Report),
    User(UserAction),        // Snooze, TestAlarm, SetTracked(ConnectorKey, bool), SetAlarmEnabled(bool), SetRunAtStartup(bool)
    Suspended, Resumed, Wake, // Wake = a scheduled deadline passed
}
impl Core {
    pub fn new(settings: Settings) -> Self;
    pub fn handle(&mut self, ev: Event, now: Instant) -> Output;
    pub fn view(&self, now: Instant) -> ViewModel;   // pure function of state + now
}
pub struct Output { pub log: Vec<LogEvent>, pub settings_changed: Option<Settings>, pub next_wake: Option<Instant> }
```

- **No IO, no clock, no threads, no platform types.** Time comes in as `now`. Wall-clock strings for log lines are added by `runtime`.
- `ConnectorKey = (SourceId, index)` keys everything per connector (tray icons, settings, logs), so multiple sources are possible later without redesign. v0 has one source.
- **State:**
  - per source: info, last `Protection`, faults
  - per connector:
    - last good reading and its age
    - wire levels and spread level
    - presence (last > 0 A)
    - RED episode (start, peaks, last-red)
    - the sustained-notice flag
    - device verdict and its first-seen instant
  - health: consecutive failed ticks; NO DATA since
  - alarm phase: `Idle | Active{since, cause} | Snoozed{until, cause} | Cleared{until, duration} | Test{until}`
  - settings
- **Rules**, each in its own module with its own tests:

  | Spec | Rule |
  |---|---|
  | §6.1 | levels + median attribution (limits from `Protection`; if a limit is absent → no color for that metric) |
  | §6.2 + §8.1 | alarm iff any connector's verdict ≠ Normal and alarms are enabled (**device verdict is the only alarm authority**, D3) |
  | §6.3 | disagreement A/B, RED SUSTAINED after `trigger + 10 s` |
  | §6.5–6.7 | presence, stale, NO DATA after 3 unhealthy ticks, protection disabled |
  | §8.4 | countdown only if `cutoff_after` is known |
  | §8.5 | snooze and escalation (new code, Critical, another connector) |
  | §8.6 | clear → 5 s cleared phase; never clear during NO DATA |
  | §9 | log events, RED merge 10 s, raw diagnostic every 10 s during an alarm |
  | Spec §5.1 | `SourcePending(reason)` before the first connection → `Connecting`. After 2 min: `NotConnected` logged once, plus a `notice`. The first `SourceConnected` after that logs `Connected` (and how long it took). |

  A real alarm pre-empts a Test. A Test ends on Snooze (no re-show) or after 30 s, and is never logged as a PSU alarm.
- **ViewModel** is declarative and complete. Frontends only render and reconcile it.

  ```rust
  pub struct ViewModel {
      pub health: Health,                      // Starting | Connecting{since, reason} | Live | Stale{age} | NoData{since}
      pub connectors: Vec<ConnectorView>,      // all connectors: tracked flag, present, wires[6]{amps, level, flagged},
                                               // total, spread (+level), bar_limit, glyph: Normal|Alarm|NoData|NotConnected, attention marker, notes
      pub source: Option<SourceView>,          // model, firmware, protection summary line, fault lines
      pub alarm: Option<AlarmView>,            // phase, band color role, headline, action, sub, detail lines, right block, snooze allowed
      pub audio: Option<AudioScript>,          // Some ⇔ the alarm should be sounding now
      pub settings: Settings,                  // for the menu / settings window
      pub connecting: Option<String>,          // tooltip for the placeholder icon while no source has connected yet
      pub notice: Option<Notice>,              // one-shot user notification {id, title, text}; frontends show each id once
  }
  pub struct AudioScript { pub steps: Vec<AudioStep>, pub repeat: bool }   // [Sound×3 with 300 ms gaps, Speak(text)]
  ```

- **All user-facing strings are built here** (English): headlines, voice line, tooltip, notes. Every frontend (Windows notch, Linux notification) says the same thing, and the text is unit-tested.

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
      pub fn suspend(&mut self) / resume(&mut self);
      pub fn view(&self, now: Instant) -> Arc<ViewModel>;
  }
  pub struct Update { pub view_changed: bool, pub next_wake: Option<Instant>, pub lifecycle: Option<Lifecycle> } // DiscoveryFailed(msg) | Fatal(msg)
  ```

- **Acquisition thread:**
  1. **Discovery** (every 2 s until connected):
     - `Found` → `SourceConnected`
     - `NotReady(reason)` → `SourcePending(reason)`, retried **forever**
     - `Unusable(msg)` → `DiscoveryFailed(msg)` (the app exits)
     - Only if **no supported device was seen at all** for 2 min → `DiscoveryFailed` with the supported-device list from the drivers' names.
  2. Poll every 1000 ms on a drift-free deadline.
  3. After 3 unhealthy ticks, drop the source and rediscover every 2 s; `discover` repeats the full connect sequence (e.g. the handshake). This is generic for all sources.
  4. Pause and resume on suspend.
  5. `catch_unwind` at the thread boundary → `Fatal`.
- **Watchdog:** `next_wake` is never later than last report + 5 s, so a stalled thread produces a Stale view (Spec §5.4).
- **Settings store:** `paths.config/settings.toml`, a hand-parsed `key = value` subset, written atomically. Tracked connectors are saved by `ConnectorKey` string (`msi:ai1300ts:1`).
- **Log sink:** `paths.log/alarms.log`. `LogEvent::format(wall)` comes from `core`; the sink appends and rotates at 5 MB (Spec §9).
- `Paths`, the wall clock and the waker are injected, so the crate has no OS dependencies:

  | Injected | Windows | Linux |
  |---|---|---|
  | `Paths` | `%APPDATA%` / `%LOCALAPPDATA%` | XDG |
  | wall clock | `GetLocalTime` | `localtime_r` |
  | waker | `PostMessage` | an eventfd, or the toolkit's wake mechanism |

---

## 7. `meltalarm-win` — Windows frontend

**Main loop:** a Win32 message loop on the UI thread. `WM_APP_WAKE` (from the waker) → `runtime.pump`. A one-shot `SetTimer` at `next_wake` → `runtime.wake`. After every call, if the view changed, **reconcile**:

| Reconciler | Rule (derived only from the ViewModel) |
|---|---|
| Tray | One icon per tracked connector (stable `uID`); glyph from `ConnectorView`; blink timer (500 ms) while any glyph is `Alarm`; tooltip. While `connecting` is set and there are no connectors yet: one hollow placeholder icon with that tooltip. |
| Notice | A tray balloon (`NIF_INFO`) the first time each `notice.id` appears |
| Popup | If open, re-render |
| Overlay | Visible on every monitor iff `alarm.is_some()` |
| Audio | Player running iff `audio.is_some()`; restart if the script changed |
| Hotkey | Ctrl+Alt+G registered iff the overlay is visible and snooze is allowed |
| Autostart | **Installed copy only** (`Launch::Monitor { portable: false }`, §7.1): Task Scheduler state made to match `settings.run_at_startup`, also repairing the target path, at start and on change. A portable copy never creates, deletes or repoints the task. If the task operation fails, the frontend sends `SetRunAtStartup(<actual state>)`, so the setting and the menu checkmark always show reality, and it adds a one-time error note. |

**Modules:**

| Module | v0 tonight | Later (v1) |
|---|---|---|
| `main` | Single instance, `Host` composition (drivers = `[msi]`), TaskDialog on `DiscoveryFailed` / `Fatal`, loop | — |
| `tray` + `glyph` | `Shell_NotifyIconW` v4, `TaskbarCreated` (allowed through UIPI), DPI-sized ARGB icons, taskbar theme | — |
| `menu` | **All settings as checkmarks:** track #1/#2 (with presence), alarm, run at startup; Test alarm; Open log; Exit | Shrinks to Settings… / Open log / Exit |
| `render` | D2D + DirectWrite, software targets, widget kit, DESIGN tokens | — |
| `popup` | Solid surfaces, 150 ms rise | Acrylic (R1) |
| `overlay` | Layered, no-activate, per-monitor, topmost re-assert, 200 ms drop | — |
| `audio` | Alarm thread: `PlaySoundW` file + SAPI `ISpVoice` executing the `AudioScript` | — |
| `hotkey`, `autostart` (schtasks XML), `power` | ✓ | `autostart` implements `lifecycle::Autostart`; COM `ITaskService` optional |
| `lifecycle` | — | Windows backend of §7.1: dialogs, steps, control window, `--uninstall` |
| `settings` window | — | Win11 window per DESIGN §Settings |
| Win10 fallback | Solid colors (free, since v0 is solid) | Verified pass |

**Threads:** UI (frontend + runtime + core), acquisition (runtime), and audio (while sounding). Memory rule: render targets exist only while a window is visible; SAPI lives only on the audio thread.

### 7.1 Program lifecycle (Spec §4)

The same split as sources: **what** to do is portable and tested everywhere; **how** is per OS.

**`meltalarm-lifecycle` (portable, std only, no OS calls):**

```rust
pub enum Policy { SelfManaged, PackageManaged }      // Spec L1; chosen by the frontend (Windows / Linux)
pub enum Flag { None, Portable, Uninstall }           // command line
pub struct Copy { pub path: PathBuf, pub version: Version }
pub struct Facts { pub policy: Policy, pub flag: Flag, pub this: Copy,
                   pub installed: Option<Copy>, pub same_file_running: bool }
pub enum Launch {
    Monitor { portable: bool },            // normal start; portable → no autostart, "Install…" offered
    HandOff,                               // L6: bring the running instance forward, exit
    OfferInstall,                          // §4.5
    OfferUpdate { from: Version, to: Version },
    OfferReplace { from: Version, to: Version },  // downgrade = rollback
    StartInstalled,                        // same version: hand off to the installed copy
    Uninstall,                             // §4.7
}
pub fn decide(f: &Facts) -> Launch;       // Spec §4.4, table-driven tests; PackageManaged → Monitor/HandOff only

pub trait Step { fn name(&self) -> &str; fn apply(&mut self) -> Result<(), String>; fn undo(&mut self); }
pub fn run_atomic(steps: Vec<Box<dyn Step>>) -> Result<(), Failed>;   // install/update: undo done steps in reverse
pub fn run_best_effort(steps: Vec<Box<dyn Step>>) -> Vec<Failed>;     // uninstall: keep going, report leftovers

pub trait Autostart { fn target(&self) -> Option<PathBuf>; fn set(&self, target: Option<&Path>) -> Result<(), String>; }
pub trait Instances { fn running(&self) -> Option<Box<dyn Running>>; }
pub trait Running { fn show(&self); fn alarm_active(&self) -> Option<bool>; fn stop(&self, grace: Duration) -> Result<(), String>; }
```

- **Plans** (ordered step lists) are built by the frontend from its own `Step`s; the runner, the ordering rules and the rollback are shared.
- **Log lines:** `core::LogEvent` gains `Installed { version, autostart }` and `Updated { from, to }`; the gaps use the existing `MonitoringStopped`. `runtime` exposes `log::append(paths, wall, event)` so a process that never starts monitoring (installer, `--uninstall`) writes the same file.
- **Version:** `CARGO_PKG_VERSION`, also embedded as the exe's VERSIONINFO so the installed copy's version is read from the file itself, without running it.

**Windows backend (`meltalarm-win::lifecycle`, `Policy::SelfManaged`):**

| Piece | Implementation |
|---|---|
| Installed copy | `FOLDERID_ProgramFilesX64\MeltAlarm\meltalarm.exe`; version via `GetFileVersionInfoW` |
| Same file? | File identity (volume serial + file index), not path strings |
| Install plan | StopRunning → CopyProgram (write `meltalarm.exe.new`, rename into place) → StartMenuShortcut (`IShellLinkW`, `FOLDERID_CommonPrograms`) → AppsEntry (`HKLM\…\Uninstall\MeltAlarm`: name, version, publisher, icon, size, `UninstallString = "…\meltalarm.exe" --uninstall`) → Autostart (if chosen) → start the installed copy |
| Update plan | AlarmGate (`Running::alarm_active`, refuse if `Some(true)`) → StopRunning → ReplaceProgram (rename current → `meltalarm.exe.old`, copy the new file in; undo restores `.old`) → AppsEntry version → start. The next start of the installed copy deletes `.old`. |
| Uninstall plan | StopRunning → Autostart off → shortcut → AppsEntry → program folder → data folders (if asked). The uninstaller *is* the installed exe: it renames itself out of the folder (allowed for a running image on the same volume), marks that file `MoveFileExW(DELAY_UNTIL_REBOOT)`, and removes the now-empty folder. |
| Autostart | Existing Task Scheduler XML, target = the installed exe only |
| Instances | Single-instance mutex (as today) + a message-only **control window** (class `MeltAlarm.Control`): `WM_APP_CONTROL` SHOW / ALARM? / QUIT via `SendMessageTimeoutW`. `stop` = QUIT, wait on the process handle for 5 s, then `TerminateProcess`. Instances without a control window (v0.x, or another session) are found by image name `meltalarm.exe` and stopped the same way. |
| Elevation | All of it runs in the already-elevated process (manifest), so Program Files and HKLM need no extra prompt. |
| Dev builds | `--portable` → `Flag::Portable`; `simulate` builds skip lifecycle entirely. |

**Linux backend (next iteration, `Policy::PackageManaged`):** no `Step`s at all: the package installs the binary, the `.desktop` file and the udev rule. `Autostart` = an XDG autostart entry pointing to the packaged binary; `Instances` = a D-Bus name or a socket in `$XDG_RUNTIME_DIR`. `decide` then only ever returns `Monitor { portable: false }` or `HandOff`.

---

## 8. Key flows

```
Launch:   main → lifecycle::decide(facts) → Monitor → Startup below | HandOff → Running::show → exit
          | Offer* → dialog → run_atomic(plan) → start installed copy → exit | Uninstall → run_best_effort → exit
Startup:  main → single instance → Runtime::start (settings loaded) → acquisition discovers every 2 s
          ├─ found (answered, incl. handshake) → SourceConnected → first run picks tracked connectors → tray icons
          ├─ present, silent → SourcePending → hollow "connecting…" icon; retried forever; notice + log after 2 min
          ├─ unusable (lock inaccessible) → Lifecycle::DiscoveryFailed → message → exit
          └─ no supported device for 2 min → Lifecycle::DiscoveryFailed → message "only MSI MPG Ai1300TS/Ai1600TS…" → exit
Tick:     acquisition poll → channel + waker → pump → core.handle(Report) → view → reconcile (≈1 ms)
Alarm:    verdict ≠ Normal → core Active → view.alarm + view.audio → overlay shown, player started,
          hotkey registered; Output.log = [PSU ALARM, RAW] → sink
Snooze:   click/hotkey → runtime.user(Snooze) → Snoozed{+30 s}, next_wake → view without alarm/audio
          → wake: still ≠ Normal → Active again; escalation on any tick → Active immediately
Clear:    all Normal → Cleared{5 s} (green notch, audio stops) → wake → Idle
Lost:     3 unhealthy ticks → NoData view (grey); alarm, if any, stays with "cannot confirm"
```

---

## 9. Read-only contract — enforcement (Q1)

1. **Per source**, a closed request enum and a single private packet builder. A unit test enumerates every variant. For MSI that is exactly 9 packets: `[00, 51, reg, 0…]` with reg ∈ {10, 11, 12, 13, C0, C1, E0, E1}, plus the connect handshake `[00, FA, 51, 0…]`.
2. **Workspace-wide** `clippy.toml` `disallowed-methods`:
   - `hidapi::HidDevice::write`
   - `send_feature_report`
   - `get_feature_report`
   - `send_output_report`
3. Exactly one `#[allow(clippy::disallowed_methods)]` per source crate, at its `transport` write.
4. CI counts those `#[allow]`s and fails if they don't equal the number of source crates.
5. No frontend or runtime crate depends on `hidapi` directly. Only `source-api` (the context) and source crates do.
6. `psu-probe` uses `source-msi`'s public API only.

---

## 10. Error handling, testing, build

**Failure isolation:**

| Failure | Effect |
|---|---|
| Source error | The tick is unhealthy (a view change, not a crash) |
| Acquisition panic | `Fatal` → logged `MONITORING STOPPED` + TaskDialog |
| Audio or voice failure | Overlay still shown; logged once |
| Overlay failure | Audio still plays |
| Autostart failure | The menu item shows unchecked, with a note |
| Install / update step failure | Done steps undone in reverse; error dialog names the step; this copy runs portable (install) or the previous version is restarted (update) |
| UI thread panic | Panic hook logs the same line |

**Testing:**
- `model`/`core`: table-driven synthetic feeds for every Spec §6/§8/§9 rule, including sources with missing capabilities (e.g. no `cutoff_after` → no countdown).
- `source-msi`: golden frames from the reference captures (serial scrubbed), plus the fake `Transport` (foreign frames, stale same-register frames, busy, timeout, disconnect — F17).
- `runtime`: fake drivers (scripted discovery and failures) and a temp-dir store and log.
- `lifecycle`: the Spec §4.4 table for both policies, version ordering, and rollback order with fake `Step`s that fail at each position. Runs on Linux CI too.
- **Simulated source** (cargo feature `simulate`, never in release builds): a `Driver` that replays a scenario file. The UI and every alarm path can be exercised with **no device traffic at all**. It is also a second implementation of the source API, which proves the abstraction.
- Acceptance T1–T10 run manually.

**Build:**
- `x86_64-pc-windows-msvc`, static CRT.
- `build.rs` embeds the manifest (`requireAdministrator` because of the MSI mutex, PerMonitorV2, Common Controls v6, Windows 10/11), the icon and version info.
- **CI (v1):** Windows (fmt, clippy, test, read-only check, release + SHA256SUMS), **plus Linux** (`cargo test` for model, core, source-api, source-msi, runtime, lifecycle). That keeps the portable crates portable before any Linux frontend exists.
- **Repo:** MIT; `docs/`; the research material (PSU.dll, raw captures, original notes) stays local and git-ignored.

---

## 11. Extension paths (designed-for, not built)

### 11.1 Linux frontend
- **Reused unchanged:** model, core, source-api, source-msi, runtime.
- **New crate:**

  | Area | Plan |
  |---|---|
  | Tray | StatusNotifierItem (`ksni`) |
  | Alarm | Critical desktop notification; layer-shell overlay where the compositor supports it (KDE, wlroots; not GNOME) |
  | Sound and voice | PipeWire + speech-dispatcher, executing the same `AudioScript` |
  | Autostart | XDG autostart entry (`~/.config/autostart`) via `lifecycle::Autostart`; works on every desktop, and systemd generates a unit from it where used |
  | Install, update, uninstall | Distro package / AppImage (`lifecycle::Policy::PackageManaged`, Spec L1); the package ships the udev rule |
  | Single instance | `lifecycle::Instances` over D-Bus or a `$XDG_RUNTIME_DIR` socket |
  | Device access | udev rule, **no root** |
  | Paths | XDG |

- **Open before starting:** verify HID framing on real hardware with `psu-probe`, and check overlap with Railwatch.

### 11.2 A new sensing device
1. Add a `meltalarm-source-<vendor>` crate implementing `Driver`/`Source` under the §4.2 rules, with golden frames.
2. Add it to the frontend's driver list.
3. Nothing else changes, **if** the device reports its own verdict.

**Product decision still pending (D3):** for devices *without* a verdict, MeltAlarm would have to decide alarms itself. The hook is already there: `Capabilities.device_verdict` plus one place in `core` (`alarm::authority`). The policy, e.g. local limits sustained for N s, is deliberately not designed until such a device exists.

---

## 12. Key decisions

| # | Decision | Alternatives rejected | Why |
|---|---|---|---|
| D1 | Sans-IO `core` hosted by a portable `runtime` on the frontend's UI thread | A dedicated engine thread; core inside the frontend (v1) | Deterministic and testable; one fewer thread; the same host serves Windows and Linux |
| D2 | Vendor-neutral model + compile-time `Driver`/`Source` traits | MSI types in core (v1); runtime plugins | Q4 without a rewrite. Plugins would load third-party code into an elevated process (Q1). |
| D3 | The device verdict is the only alarm authority | Local alarm from our own readings | Spec decision (spikes must not alarm). Extension hook exists; policy deferred. |
| D4 | Declarative ViewModel; frontends reconcile | Imperative effect commands | No drift between "should" and "is" (overlay, audio, hotkey, tray). Idempotent after any hiccup. |
| D5 | Native frontend per OS | Cross-platform UI toolkit | The hard parts (tray, overlay over games, autostart, audio) are OS-specific anyway; toolkits cost the footprint (Q3). |
| D6 | One elevated process on Windows | Service + user helper | Admin is sufficient (F3); half the moving parts |
| D7 | Direct2D/DirectWrite in software mode | GPU rendering | No GPU driver in the process; tiny surfaces |
| D8 | User-facing text built in `core` | Text per frontend | One tested source of wording across OSes and channels (notch, voice, notification) |
| D9 | MSI handshake sent on **every connect**, never per tick | Only after reads time out ("minimal touch") | Mirrors the vendor's own clients exactly (deterministic, independent of what other software did since boot); a failure-driven path would be a rarely-exercised branch where bugs hide. Repeated handshakes are routine in MSI's ecosystem (MSI Center + Afterburner). |
| D10 | A present but silent device is retried forever; exit only when no supported device exists | Give up after a timeout | A safety monitor must not quit because the PSU is slow to answer; "unsupported hardware" (exit) and "not answering yet" (wait) are different situations. |
| D11 | Windows: one self-installing exe | MSI/WiX or Inno Setup installer; portable only | No second toolchain; one file to verify; an installer would still need custom steps for the elevated task; unsigned installers get harsher SmartScreen treatment. Portable-only broke autostart and left no uninstall. |
| D12 | Lifecycle = portable decisions (`meltalarm-lifecycle`) + per-OS steps; Linux delegates to the package manager | Lifecycle code only inside `meltalarm-win`; self-install on Linux too | Same pattern as sources; the decision table is tested on Linux CI; Linux users expect packages; nothing Windows-specific reaches the portable crates. |
| D13 | Autostart only ever targets the protected installed copy | Task follows `current_exe()` (v0) | v0 let a user-writable file start elevated at logon (privilege escalation), and let dev builds repoint the user's task. |

---

## 13. Risks and spikes

| # | Risk | When | Fallback |
|---|---|---|---|
| R1 | Acrylic behind software-D2D content | v1 | Solid (the v0 default) |
| R2 | Overlay not visible over some borderless or flip-model games, or steals focus | v0 test with 2–3 games | Audio is guaranteed; document the titles |
| R3 | UIPI blocks tray messages to the elevated process | v0, first thing | `ChangeWindowMessageFilterEx` |
| R4 | Idle memory above 5 MB after windows close | v0 measure | Release resources; trim the working set |
| R5 | Linux HID framing (64/65 bytes) | Linux iteration | Parser already tolerant; verify with the probe |
| R6 | Defender/SmartScreen heuristics flag an unsigned exe that copies itself to Program Files and creates an elevated logon task | v1, test with Defender on | Plain dialog wording, SHA256SUMS; report false positives to Microsoft; code signing later |
| R7 | Self-removal of the running installed exe (rename out of the folder) | v1 spike | Mark the whole folder `DELAY_UNTIL_REBOOT` |

---

## 14. Milestones

**v0 — tonight** (in order; each step leaves something runnable):
1. `git init`, workspace skeleton, `.gitignore` (research material moved to the ignored `research/`), clippy read-only lint.
2. `model` + `source-api` + `source-msi` (port the verified probe code, golden frames).
3. `core`: levels, attribution, health, alarm phases, snooze, clear, log events, ViewModel and text, with tests for the safety-relevant rules.
4. `runtime`: acquisition, discovery, reconnect, settings store, log sink.
5. `win`: tray + glyph + menu (settings as checkmarks) → **first live run** (R3, R4).
6. `win`: popup (solid).
7. `win`: overlay + audio/voice + hotkey + Test alarm → **alarm verified via Test alarm** (R2).
8. `win`: autostart task, power events → install for daily use.

**v0.1 — fix after the first cold boot (2026-09-28, F19):**
- MSI connect handshake + `Request` enum (9-packet contract test).
- `Discovery::NotReady`; identification by USB ID, with the model string display-only.
- Runtime: 2 s discovery loop, retried forever when present; exit only if absent for 2 min; startup failures logged.
- Core: `Connecting` health, `NOT CONNECTED` / `CONNECTED` log events, one-shot `notice`.
- Win: placeholder "connecting…" tray icon and a notice balloon; simulated `silent` scenario for T13.
- Acceptance: T11 (cold boot, no MSI software), T12, T13.

**v1 — GitHub:**
- settings window
- acrylic/Mica
- Win10 pass
- simulated source + scenario files
- `psu-probe` tool
- `docs/PROTOCOL.md`, README
- CI (Windows + Linux crates), release, coexistence run (T3)
- program lifecycle (§7.1): `meltalarm-lifecycle`, Windows backend, VERSIONINFO, control window; acceptance T14–T17. Migrates the reference PC from the hand-copied `%LOCALAPPDATA%\Programs\MeltAlarm`.

**Next iteration:** Linux frontend (§11.1); new sources on demand (§11.2), plus the local-alarm policy decision for sources without a verdict.

---

## 15. Self-review

| Criterion | Assessment |
|---|---|
| Consistent with the spec | Every Spec §5–§9 rule has exactly one home (source-msi: §5; core: §6, §8, §9; runtime: §5.4 reconnect/watchdog, §9 file; win: §4, §7, §8.2/8.3 rendering and playback). Spec facts F3, F13, F17 are reflected in lock, audio and transport. |
| Single responsibility, clear boundaries | Each crate has one reason to change: vocabulary, decisions, device family, hosting, OS. |
| Dependency direction | Stable → volatile, verified in §2. No cycles. `core` has zero dependencies beyond `model`. |
| Safety invariant is structural | Closed enums + single write site + lint + CI count + no `hidapi` outside sources (§9) |
| Testability | Everything that decides is pure and clock-injected; every IO edge has a fake (Transport, Driver, Paths, wall clock). |
| Extensibility without speculation | Only two seams (Source, Frontend), both driven by stated plans. No plugin system, no generic "sensor" beyond 12V-2x6's 6 wires, no local-alarm policy yet (YAGNI). |
| Failure isolation | §10 table: every non-core failure degrades a channel, never monitoring or the alarm decision. |
| Footprint | Two permanent threads, software rendering, resources scoped to visibility; the extra crates cost nothing at runtime. |
| Delivery (tonight) | v0 steps need no v1-only parts. The seams add a few small traits, not frameworks. |
| **Known trade-offs** | (1) Menu-based settings in v0. (2) Frontends must implement reconciliation correctly (mitigated: small and declarative). (3) `requireAdministrator` applies to the whole app because of the MSI source on Windows. (4) A source without a verdict can color but never alarm until D3's pending decision. |
