# MeltAlarm — Functional Specification

**Status:** v2.1 · **Date:** 2026-09-28 · v2.1: the FA 51 connect handshake (F19), device identification by USB ID, startup that never gives up on a present PSU

**Supported hardware:**
- MSI **MPG Ai1300TS** and **MPG Ai1600TS** PSUs, connected by USB.
- **Any GPU** powered through the PSU's 12V-2x6 connectors.
- **Both connectors** (#1 and #2): one GPU on either connector, or two GPUs.

**Distribution:** public GitHub project. The app must not assume anything about the user's machine beyond the list above.

MeltAlarm is a lightweight Windows tray app. It watches the per-wire currents that MSI MPG Ai1x00TS PSUs measure on their two 12V-2x6 connectors, shows them at a glance, and raises an impossible-to-miss alarm when the PSU's own **GPU Safeguard+** protection reports a problem.

MeltAlarm is a **notifier, not a protector**. The PSU firmware decides that something is wrong, sounds its buzzer and cuts power. MeltAlarm makes sure the user *knows* in time to react.

---

## 1. Research findings

Measured on 2026-09-27 on a **reference system** (Ai1300TS, fw `MFR_Version 1.0`, rev `10`, RTX 5090 on connector #1) with a read-only R&D probe (the `psu-probe` tool is its successor). Raw captures are kept privately because they contain the unit serial.

The concrete values below describe that one system. The app treats every one of them as **read at runtime or handled generically**, never hardcoded.

| # | Finding | Evidence | Consequence for the app |
|---|---|---|---|
| F1 | PSU enumerates as HID `VID_0DB0&PID_AA6F`, one interface, usage page `FF00`, product string `"MPG Ai1300TS"`. | `psu-probe open-only` | Discovery by VID/PID, then model check (§5.1) |
| F2 | A **non-admin** process can open the HID device. | unelevated `open-only` | — |
| F3 | `Global\MSI_PSU_Mutex` is inaccessible to non-admin (error 5 for every access right). An **elevated admin gets only `SYNCHRONIZE` + `READ_CONTROL`**, and `MUTEX_ALL_ACCESS` is denied, so `CreateMutex` fails even elevated. `OpenMutex(SYNCHRONIZE)` works and is sufficient to wait and release. | probe, unelevated vs elevated | App runs elevated; open with `SYNCHRONIZE`; create only if absent (§5.2) |
| F4 | All needed registers are readable with opcode `0x51`, **but only after a host has connected with the `FA 51` handshake** since the PSU powered up. *Corrected 2026-09-28:* the first measurement ("handshake not needed") was taken while MSI Center or HWiNFO had already done it (see F19). | probe; F19 | Handshake on every connect (§5.1) |
| F5 | Identity registers: `0x10`="MSI", `0x11`=model ("MPG Ai1300TS"), `0x12`=revision, `0x13`=serial (length-prefixed). | probe | Model string is the compatibility check |
| F6 | `0xC0` holds the Safeguard+ config. Reference unit: `IsEnable=1, OCP_Point=12.0 A, Current_Diff=5.5 A, OCP_TriggerTime=20, Diff_TriggerTime=20, Warning_Time=180`. MSI Center allows the user to change it (OCP 10–14 A, Diff 4–8 A, triggers 5–30 s, warning 30–240 s). | probe; reverse-engineering of MSI Center | **Read at start and every 60 s**; all thresholds are derived from it (§5.3, §6) |
| F7 | `0xC1` gives a per-connector status byte (0 = Normal). In Normal the RunTime/TotalTime/current fields are all zero, even under 575 W, so they aren't live data. | 300 samples, idle + load | Status is used; other fields are only logged during an alarm |
| F8 | `0xE0` gives 12 live wire currents (LINEAR11: signed 5-bit exponent, unsigned mantissa). Resolution 0.0625 A in the observed range. | probe; consistent with power/voltage | — |
| F9 | `0xE1` gives 18 protection flag bytes; all 0 in normal operation. | probe | — |
| F10 | One transaction takes **0.6–0.9 ms**. 1170/1170 transactions succeeded across all runs. | watch runs | 1 Hz polling is negligible load |
| F11 | An unused connector reads **exactly 0.000 A**. The used connector at idle reads 0.06–0.19 A per wire. | idle watch | Presence detection (§6.5) |
| F12 | Healthy cable at full load (2× FurMark, ~575 W, 47–50 A): hottest wire **max 8.56 A**, spread (max−min) **max 0.69 A**, relative deviation max 5.2 %. At idle, relative deviation reaches ~40 % from quantization noise. | load watch, 180 loaded samples | **Deviation is measured in amps, not percent**; yellow thresholds validated (§6.1) |
| F13 | A Windows sound scheme can remap "Critical Stop" (`SystemHand`); on the reference system it plays `Windows Foreground.wav`. The file `C:\Windows\Media\Windows Critical Stop.wav` (0.9 s) is standard on Windows 10/11. | registry, filesystem | Play the file, not the alias (§8.3) |
| F14 | English SAPI voices ship with Windows (David, Zira, Mark…). | registry | Voice needs no install |
| F15 | Ctrl+Alt+G was free on the reference system. | `RegisterHotKey` test | Hotkey; a registration failure is handled (§8.5) |
| F16 | **Other PSU readers vary per user.** Seen on the reference system: MSI Center service (running, then stopped), Afterburner PSU plugin (installed, disabled), HWiNFO64 (portable). The mutex existed even with the MSI Center service stopped. | processes, configs | App must work with any combination (§5.2, test T3) |
| F17 | **With HWiNFO64 running:** our mutex waits reached **258 ms**, versus ≤ 2 ms with MSI Center alone, so HWiNFO evidently uses the same mutex. On 109 of 180 transactions our input queue held *foreign* replies. Draining the queue plus the echo check gave **0 failures and 0 wrong frames**. | 60 s watch with HWiNFO sensors open | Mutex timeout 2 s; drain + echo check are mandatory (§5.2) |
| F18 | A minimal Rust prototype (tray icon with 6 squares redrawn at 1 Hz, HID handle open) uses **1.5 MB private memory**, 9.4 MB working set (mostly shared system DLLs), and 16 ms CPU per 15 s. | footprint prototype | Footprint budget (§10) |
| F19 | **Cold boot without MSI software:** the PSU enumerates on USB but does **not answer** `51` reads (every attempt timed out for 2 min). MSI Center's own `CONNECT_PSU` sends `00 FA 51` once per connection, under the mutex, and requires the echo `FA 51` with byte 3 ≠ `FE`. Its per-tick reads (`Get`) send only `51 <reg>`. The Afterburner plugin sends the same handshake on its connect, so this PSU routinely receives it from several clients. What `FA 51` does inside the firmware is not documented. | MeltAlarm log 2026-09-28 00:06; decompiled MSI Center `cPSU.CONNECT_PSU` and `cPSU.Get` | Handshake on every connect, never per tick (§3, §5.1) |

**Not verified, and not provoked on purpose (hardware safety):**
- PSU behavior during a real alarm
- the firmware's exact spread formula
- whether TriggerTime is in seconds
- what RunTime in `C1` means during an alarm
- how fast status 3 cuts power
- mapping of wire numbers to physical pins
- whether the PSU protects without any software running
- **Ai1600TS on real hardware.** Same PIDs and code paths in both MSI Center and the Afterburner plugin, and a public Railwatch capture confirms its `E0` layout; `C0`/`C1`/`E1` are unconfirmed.

MeltAlarm logs enough on the first real event to settle these (§9).

---

## 2. Scope

**In v1:**
- background tray monitoring
- per-wire visualization for either or both connectors
- PSU-status alarm (overlay + sound + voice)
- settings
- event log
- compatibility check
- autostart

**Not in v1:**
- **VR-specific visuals** (SteamVR overlay, OpenXR layer). They are future work, after v1 feedback. In v1, VR users get the alarm through sound and voice.
- Changing the GPU power limit.
- Automatic OS shutdown.
- History charts.
- Any PSU configuration.
- MSI PSUs without Safeguard+ (non-TS models).

---

## 3. Read-only contract (hard requirement)

1. The app can build exactly **nine** packets (65 bytes each), from a closed, compile-time set:
   - **reads** `00 51 <reg> 00…`, with `<reg>` ∈ `0x10 0x11 0x12 0x13 0xC0 0xC1 0xE0 0xE1`
   - **the connect handshake** `00 FA 51 00…`, exactly as MSI Center and Afterburner send it (F19). It is sent only when a connection is opened (startup, reconnect), never per tick.
2. No code path sends opcode `0x50` (write), register `0xF1` (save), `0xC2` (buzzer), or any byte derived from settings, UI, CLI or IPC.
3. A unit test asserts every constructible packet. A code-review checklist item confirms `HidDevice::write` has a single call site.
4. The app never modifies MSI Center, Afterburner, HWiNFO or their configs.

---

## 4. Process model, privileges, autostart

- **Single executable** (Rust), no installer, and no external DLLs (hidapi is linked through its `windows-native` backend).
- **Runs elevated** (manifest `requireAdministrator`). Elevation is needed because of F3: any other PSU reader may own the mutex, and admin is the minimum that can use it. A SYSTEM service is not needed.
- **Autostart** is a Task Scheduler task `MeltAlarm` that runs at logon of the current user with "Run with highest privileges". It starts elevated **without a UAC prompt**. The *Run at startup* setting creates or deletes this task.
- A manual launch shows one UAC prompt (expected).
- **Single instance.** A second launch exits silently. If the first instance has a window open, the second launch brings it forward.
- Side benefit of elevation: the alarm overlay can also appear above elevated apps.

---

## 5. Device communication

### 5.1 Startup and connecting

**Supported hardware is identified by USB ID**: VID `0x0DB0` with PID `0xAA6F` (Ai1300TS) or `0x808C` (Ai1600TS). MSI's own software selects the Safeguard+ protocol by these IDs alone. The model string the PSU reports (`0x11`) is **display-only** and can never make MeltAlarm refuse a device.

**Connecting** (at startup, and on every reconnect, §5.4) — same sequence as MSI Center's `CONNECT_PSU`:
1. Open the HID device.
2. Send the handshake `00 FA 51` under the mutex. The reply must echo `FA 51`, with byte 3 ≠ `FE`.
3. Read `0x10`–`0x13` (identity, for display) and `0xC0` (config), then start the 1 Hz poll (§5.3).

A connection counts as established only after the PSU has answered. Opening the device alone is not enough.

**Startup outcomes:**

| Situation | Behavior |
|---|---|
| No device with a supported USB ID for **2 min** after start | Message *"MeltAlarm supports only MSI MPG Ai1300TS / Ai1600TS power supplies connected by USB. No supported PSU was found."*, logged, then **exit**. The 2 min allow for USB enumeration at logon. |
| Supported device present, **not answering** (handshake or reads fail) | **Never exit.** One hollow tray icon with the tooltip *"MeltAlarm · connecting to the PSU…"*. Retry every 2 s, indefinitely. After 2 min without success: one Windows notification *"MeltAlarm can't reach the PSU (reason). It keeps trying."* and one log line. The first success is logged too (§9). |
| Present but unusable (MeltAlarm not elevated, so the lock is inaccessible) | Message with the reason, logged, then exit. |
| Connected | Tray icons per tracked connector (§7.1). |

### 5.2 Mutex and coexistence
The app must work with **any combination** of MSI Center (running, stopped, or not installed), the Afterburner PSU plugin (on or off) and HWiNFO64 (running or not).
- Open `Global\MSI_PSU_Mutex` with `SYNCHRONIZE`. If it doesn't exist, create it. The default admin DACL still lets MSI's SYSTEM service and elevated tools open it later.
- Each transaction:
  1. acquire the mutex (wait at most **2 s**; on timeout, skip this read)
  2. **drain** stale input reports (other clients' replies arrive in our queue too, F17)
  3. write the request
  4. read until the reply echoes the request (`51 <reg>` or `FA 51`; deadline 500 ms)
  5. release the mutex
- A mutex returned as abandoned counts as acquired.
- Our hold time is about 1 ms per transaction. The mutex is never held across the sleep.
- Other tools that *ignore* the mutex (unknown today) may cause occasional failed samples. §5.4 absorbs these without a false NO DATA.

### 5.3 Poll cycle (every 1000 ms, drift-free)
| Register | When |
|---|---|
| `0xE0` telemetry | every tick |
| `0xC1` status | every tick |
| `0xE1` flags | every tick |
| `0xC0` config | at start, then **every 60 s** |

Why 60 s for `0xC0`: it costs under 1 ms per minute, and thresholds changed in MSI Center are picked up within a minute.

Total: about 3 ms of bus time per second.

### 5.4 Error handling
- A mutex timeout, a transaction timeout, a wrong or short echo, or the busy pattern (`FE` followed by all zeros) makes a **failed sample**. A failed sample is never shown as zero current. The display keeps the last valid values, marked stale.
- **3 consecutive failed ticks** puts the app into **NO DATA**:
  - icons go grey
  - the popup says *"Monitoring interrupted"*
  - the event is logged
  - the app closes the device and **reconnects** (§5.1, including the handshake) every 2 s until a tick succeeds (logged as *DATA BACK*)
- USB unplug while running goes through the same NO DATA path. The app keeps running; the only exits are the startup cases of §5.1.
- Sleep/hibernate: polling pauses on suspend and resumes on resume. That is not logged as lost or restored.
- If the poll thread stalls for more than 5 s (watchdog), icons go grey.

---

## 6. Evaluation model

Per connector *c* (1 or 2), from the latest `E0` sample: wires `I1..I6`, `max`, `min`, `spread = max − min`, `median`. Thresholds come **live from `C0`**:
- `OCP = OCP_Point` (12.0 A on the reference unit)
- `D = Current_Diff` (5.5 A on the reference unit)

### 6.1 Local visual level (instant, no debounce, never triggers the alarm)
| Level | Wire current | Connector spread |
|---|---|---|
| RED | `I ≥ OCP` | `spread ≥ D` |
| YELLOW | `I ≥ 0.8·OCP` | `spread ≥ 0.5·D` |
| GREEN | otherwise | otherwise |

On the reference unit this gives yellow at 9.6 A per wire and 2.75 A spread, against a healthy peak of 8.56 A and 0.69 A. If a user tightens OCP in MSI Center (e.g. to 10 A → yellow at 8 A), yellow becomes common under full load. That is intended: it means "close to the limit **you** set on the PSU".

**Attribution of a spread level to wires.** The PSU judges the connector as a whole; the UI has to point at *which* wire. When spread reaches level *L* with threshold *T*, every wire with `|I − median| ≥ T/2` gets level *L*. Math guarantees at least one wire: the median lies between min and max. This highlights the outlier, not an innocent neighbor. Example: a single bad contact at 2 A among five wires at 9.8 A lights only that wire.

**Wire color** = the highest of:
- the wire's current level
- the attributed spread level
- RED if the PSU `E1` flag for that wire is set

Colors follow any threshold change within 60 s, and each change is logged.

### 6.2 PSU status (the only alarm trigger)
The `C1` status byte per connector:

| Code | Name shown to user | Meaning |
|---|---|---|
| 0 | Normal | — |
| 1 | Over-current | A wire has been above OCP for OCP_TriggerTime |
| 2 | Current imbalance | Spread has been above Diff for Diff_TriggerTime |
| 3 | Critical over-current (>18 A) | Hard firmware limit; power cut may be immediate |
| other | Unknown PSU alert (0xNN) | Treated as an alarm |

### 6.3 When our view and the PSU's view disagree
The two views are computed independently, so they **can and will** disagree. The app never overrides one with the other; each drives its own channel.

**A. Local RED, PSU says Normal.** This is possible and expected:
1. **Trigger delay.** The PSU raises status only after the anomaly has lasted `TriggerTime` (20 s on the reference unit, 5–30 s configurable). Every real problem looks like this during its first seconds.
2. **Short spikes.** GPUs draw millisecond-scale transients. A 1 Hz sample can catch one above OCP; the PSU ignores it because it didn't persist.
3. **Boundary and formula details.** We use `≥`; the firmware's exact comparison and spread formula are unknown. The PSU also samples at different moments than we do.
4. **Safeguard+ disabled** on the PSU (§6.7): the PSU never alarms.

*Behavior:* red squares in tray and popup, plus a *RED START/END* log entry. **No overlay or sound** (by design: spikes must not cause alarms). One extra signal: if RED lasts continuously longer than `TriggerTime + 10 s` while the PSU still says Normal, the popup shows *"Over the PSU limit for 35 s — PSU has not raised an alarm"*, and the log gets one *RED SUSTAINED* line. This is still not an alarm. It flags that the two views don't match, which is worth knowing (e.g. protection disabled, or thresholds disagree).

**B. PSU alarm, local GREEN or YELLOW.** This is possible when the PSU saw something between our 1 Hz samples, or its formula differs from ours.

*Behavior:* **the PSU wins.** The full alarm fires. Wires flagged in `E1` turn red. The popup and overlay show the PSU's reason even if our numbers look fine.

### 6.4 Other PSU flags
The `E1` rail OCP (12V/5V/3.3V), OPP, OTP and Fan flags:
- shown in the popup status line
- logged
- the tray icon shows a PSU-fault marker
- **no overlay and no sound**

### 6.5 Connector presence
A connector counts as *connected* if any wire read > 0 A in the last 10 s (F11). This drives the hint dot in Settings and the first-run default.

### 6.6 Stale data
If the last valid sample is older than 1 tick, values are shown as stale (dimmed). After NO DATA (§5.4), everything is grey.

### 6.7 Safeguard+ disabled
If `C0.IsEnable = 0`:
- the popup shows the persistent warning *"PSU Safeguard+ is OFF — the PSU will not protect the cable"*
- the tray icon shows a warning marker
- the event is logged

The app never re-enables it (read-only).

---

## 7. User interface

### 7.1 Tray icons
- One icon per **tracked** connector (1 or 2), in the fixed order #1 then #2.
- The glyph is **6 squares in 2 rows of 3**. Square *n* = wire *n* (row 1: wires 1–3, row 2: wires 4–6), colored per §6.1.
- Visual states (exact look is decided in the UI design phase):
  - neutral (normal, "dark cockpit") / amber / red squares — see DESIGN.md
  - **PSU alarm on this connector**: must look distinct from local red (e.g. blinking)
  - **grey**: NO DATA, or connector not connected
  - **connecting**: before the first successful connection, a single hollow icon with the tooltip *"MeltAlarm · connecting to the PSU…"* (§5.1)
  - markers for PSU-fault flags and Safeguard+ OFF
- Tooltip: `MeltAlarm · 12V-2x6 #1 · Normal · max 8.6 A · spread 0.7 A`
- Left click toggles the status popup for **that icon's connector**.
- Right click opens a menu: *Settings…*, *Open alarm log*, *Exit* (Exit asks for confirmation: "Monitoring will stop").
- Windows 11 hides new tray icons in the overflow area by default. First run explains how to pin the icon.

### 7.2 Status popup
- Anchored right above the clicked icon, using the icon's actual position. If that position can't be determined (e.g. the icon is in the overflow area), it anchors near the cursor.
- Content:
  - connector name `12V-2x6 #1`
  - PSU status (§6.2) in words
  - **6 vertical bars**, filled toward the PSU limit (OCP), colored like the icon, with the current under each (1 decimal, e.g. `8.6 A`) and the wire number
  - a secondary line: `total 47.9 A (sum of wires) · spread 0.7 A`
  - lines for PSU faults, Safeguard+ OFF, and the "RED SUSTAINED" notice (§6.3) when relevant
  - during a PSU alarm: the countdown (§8.4)
- Updates live at 1 Hz while open. Closes on focus loss or Esc, like native flyouts.
- In NO DATA: "Monitoring interrupted", with the age of the last valid reading.

### 7.3 Settings window

> **v0 (first personal build):** there is no settings window yet. The same three settings plus *Test alarm* sit in the tray right-click menu as checkmark items (ARCHITECTURE §7). The window below arrives in v1, and the menu then shrinks to *Settings… / Open alarm log / Exit*.
| Setting | Default | Notes |
|---|---|---|
| Run at Windows startup | **On** | Creates or removes the scheduled task (§4) |
| Track 12V-2x6 #1 ☐ ● / Track 12V-2x6 #2 ☐ ● | First run: the connector(s) currently *connected*; if none, #1 | At least one must stay checked (the last checked box is disabled). The **●** hint dot is live: green = connected, red = not connected (§6.5). |
| Alarm (overlay + sound + voice) | **On** | Master switch. Off means tray colors and log only. The log is always written. |

- Read-only info block: PSU model, firmware, serial, and the Safeguard+ state and thresholds read from `C0` (e.g. "Enabled · OCP 12 A · Imbalance 5.5 A · trigger 20 s · power cut after 180 s").
- Buttons: **Test alarm** (full overlay + sound + voice with a "TEST" label) and **Open log folder**.
- Settings apply immediately. They are stored in `%APPDATA%\MeltAlarm\settings.toml`.

---

## 8. Alarm

### 8.1 Trigger
The alarm fires when the `C1` status is non-zero on **any** connector, tracked or not, and the Alarm setting is on. Tracking only controls icons; a safety event is never filtered out.

### 8.2 Overlay ("notch")
- Attached to the **top edge, centered, on every monitor**. Flat top edge; rounded bottom corners.
- **Size:**
  - width = 32 % of the monitor's width, clamped to **560–880 DIP**
  - height = fits the content, about **200–260 DIP**
  - This is readable at arm's length without covering the middle of the screen. Examples: 1920×1080 → ~614×230 px; 2560×1440 → ~819×230 px; 4K at 150 % → 880×230 DIP.
  - Final proportions are set in the UI design phase.
- Topmost and **never steals focus**, so the game keeps input and a borderless game doesn't minimize. Clicking the button is the only interaction.
- Content, in priority order (aviation/space style: flat, high contrast, no decoration, readable at a glance):
  1. **Headline:** `GPU POWER CABLE OVERLOAD`
  2. **Action:** `STOP GPU LOAD NOW — quit the game or render`
  3. **Details:**
     - connector (`12V-2x6 #1`)
     - anomaly (`Current imbalance`)
     - flagged wires with currents
     - spread
     - PSU threshold
     - countdown `PSU power cut in ~2:47` (§8.4)
  4. **Big button:** `SNOOZE 30 s   (Ctrl+Alt+G)`

### 8.3 Sound and voice
- **Cycle** (repeats while the overlay is visible):
  1. `Windows Critical Stop.wav`, played **3 times** with ~0.3 s gaps
  2. **one voice line**
  3. repeat

  One cycle takes about 8 s. The WAV is 0.9 s long.
- Voice line (built-in English SAPI voice): *"Warning. GPU power cable overload on connector 1. Stop the game now."* It names the actual connector(s).
- The sound plays **from the file** `%WINDIR%\Media\Windows Critical Stop.wav`, not the system alias (F13). If the file is missing, the fallback is the system default beep.
- Played at the **current system volume**; no forced unmute. The PSU's own buzzer sounds regardless.
- Output goes to the default audio device. While streaming VR (VD/Link) that is the headset, so this is how v1 alerts VR users.
- The voice engine is loaded only for an alarm and released afterwards (§10).

### 8.4 Countdown
`remaining = Warning_Time − (now − first tick with non-zero status)`, shown as `~m:ss` and labeled approximate. At 0 it reads *"PSU power cut expected now"*. Snoozing doesn't reset it. If the PSU status returns to 0, the countdown ends.

### 8.5 Snooze
- The button or **Ctrl+Alt+G** hides the overlay and stops sound and voice for **30 s**.
- The hotkey is registered only while the overlay is visible. If registration fails because another app took the combination, the overlay shows the button without a hotkey hint.
- After 30 s the app checks a fresh `C1`. If it is still non-zero, the full alarm shows again. If it is 0, nothing is shown.
- Snooze is unlimited.
- **Escalation cancels snooze immediately:** status becomes 3, the code changes, or another connector enters an alarm.

### 8.6 Clearing
- When all connectors return to status 0, sound and voice stop at once. The overlay turns **green** — *"Back to normal · event logged"* — for 5 s, then disappears. This also happens if the alarm was snoozed.
- If data is lost (NO DATA) during an alarm, the alarm **stays**, with the note "PSU data lost, cannot confirm". The app never assumes all-clear without data.

### 8.7 Known limitations (documented to users)
- True **exclusive fullscreen** games (rare today; most use borderless or flip-model) can't be drawn over. Sound and voice still work.
- **VR:** no visual in the headset in v1. Sound and voice come through the headset audio.
- Locked workstation or disconnected RDP session: the overlay isn't visible, and sound depends on the session's audio.
- System muted: silent by the user's choice. The PSU buzzer still sounds.

---

## 9. Alarm log

- File: `%LOCALAPPDATA%\MeltAlarm\alarms.log`, UTF-8, one line per **event**, local time. Never per-tick, and never YELLOW.
- Size: events are rare, so growth is tiny. As a safety net, at 5 MB the file is rotated to `alarms.old.log` (at most 2 files).
- Events:

```
2026-09-27 18:02:11 | RED START     | 12V-2x6 #1 | wire 3 = 12.4 A >= 12.0 A | wires 9.1 9.3 12.4 8.8 9.0 9.2 | spread 3.6 A
2026-09-27 18:02:14 | RED END       | 12V-2x6 #1 | 3 s | peak wire 3 = 12.9 A | peak spread 4.1 A
2026-09-27 18:04:05 | RED SUSTAINED | 12V-2x6 #1 | 30 s over PSU limit, PSU still Normal
2026-09-27 18:05:40 | PSU ALARM     | 12V-2x6 #1 | status 2 Current imbalance | E1 wires: 3 | wires 9.8 9.9 2.1 9.7 9.8 9.9
2026-09-27 18:05:40 | PSU RAW C1    | 51C102...          (at start, then every 10 s while the alarm lasts)
2026-09-27 18:06:22 | PSU CLEAR     | 12V-2x6 #1 | 42 s
2026-09-27 18:10:00 | PSU FLAG      | OTP (PSU over-temperature) set
2026-09-27 19:00:00 | NO DATA       | PSU stopped answering
2026-09-27 19:00:12 | DATA BACK     | after 12 s
2026-09-28 00:06:32 | NOT CONNECTED | PSU present but not answering (Timeout) — still trying     (once, 2 min after start)
2026-09-28 00:07:10 | CONNECTED     | MSI MPG Ai1300TS after 2 min 38 s                            (only if NOT CONNECTED was logged)
2026-09-28 00:02:00 | STOPPED       | Monitoring not started: no supported PSU found                (startup exit, §5.1)
2026-09-27 20:00:00 | CONFIG        | Safeguard+ ON · OCP 12.0 A · Diff 5.5 A · trig 20/20 s · cut 180 s   (at start and on change)
2026-09-27 20:00:00 | CONFIG        | WARNING: Safeguard+ is OFF on the PSU
```

- A RED episode is **merged** if it re-enters within 10 s, so one noisy minute produces one START/END pair.
- The log **covers all connectors, tracked or not**.

---

## 10. Non-functional requirements
| Area | Requirement |
|---|---|
| Footprint — idle | Only the tray icons visible. **Private memory ≤ 5 MB** (target ~2 MB; the F18 prototype uses 1.5 MB). Average CPU ≤ 0.1 %. No GPU rendering, so the dGPU is never kept awake. |
| Footprint — active | Popup, Settings or alarm open: higher memory is allowed temporarily (text rendering, voice engine, audio). Target ≤ 30 MB. **Released when the window or alarm closes**, back to the idle budget. |
| Binary | Single exe, target < 2 MB (the prototype is 175 KB). |
| Latency | PSU status change → alarm visible and audible ≤ 2 s |
| UI | English. Per-monitor DPI-aware. Popup and Settings follow the system light/dark theme; the alarm overlay uses its own fixed high-contrast scheme. |
| Robustness | Survives USB replug, sleep/resume, explorer.exe restart (tray icons re-added), and display changes (overlay re-laid out). |
| Coexistence | Any combination of MSI Center, Afterburner PSU plugin and HWiNFO64 (§5.2). |
| Network | None. |
| Tech | Rust; Win32 + Direct2D/DirectWrite in software mode (see DESIGN.md). |

---

## 11. Acceptance tests
- **T1.** The read-only contract unit test passes. A grep shows a single HID write call site.
- **T2.** The app starts elevated via the scheduled task with no UAC prompt. A second instance exits.
- **T3. Coexistence matrix**, 30 min each: MeltAlarm alone; + MSI Center; + HWiNFO64; + Afterburner PSU plugin; all together. Pass criteria: no NO DATA, fewer than 0.1 % failed samples, and the other tools keep showing sane values.
- **T4.** Unplug the PSU USB: grey icons within ≤ 3 s, NO DATA logged. Replug: live again, DATA BACK logged.
- **T5.** Colors on a synthetic frame feed (test build only):
  - every threshold boundary, with non-default `C0` values
  - spread attribution
  - an E1 flag override
  - disagreement cases A and B (§6.3)
- **T6.** *Test alarm*:
  - overlay on all monitors
  - no focus stolen from a borderless game
  - 3× sound + voice cycle
  - snooze by button and by Ctrl+Alt+G
  - re-show after 30 s
  - green clear
- **T7.** Test alarm while in SteamVR and while in VDXR: voice and sound are heard in the headset.
- **T8.** Sleep/resume: no false NO DATA in the log.
- **T9.** Idle private memory ≤ 5 MB after 24 h, and memory returns to idle after an alarm closes. Log size unchanged when nothing happened.
- **T10.** Ai1600TS: community test via a GitHub issue template (probe output attached).
- **T11. Cold boot, no MSI software** (MSI Center service stopped, Afterburner PSU plugin off, HWiNFO closed): after logon MeltAlarm connects without any error, and the log shows no NOT CONNECTED line. This is the scenario that failed on 2026-09-28 (F19).
- **T12. Cold boot with MSI Center running**: both connect. MSI Center keeps showing sane values while MeltAlarm handshakes and reconnects (the handshake is repeated by design).
- **T13. Silent PSU** (simulated source that is present but never answers): the hollow "connecting" icon appears, the notification comes once after 2 min, the app never exits, and it connects once the PSU answers.

---

## 12. Open items
- To be learned from the first real event (logged automatically): the RunTime field, E1 behavior for status 2, and the firmware timings.
- Ai1600TS hardware confirmation (T10).
- What `FA 51` changes inside the PSU firmware (F19). The working assumption is "host connected, start answering", backed by MSI's own use. It is sent only on connect, never per tick.
