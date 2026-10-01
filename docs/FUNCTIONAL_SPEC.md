# MeltAlarm — Functional Specification

**Status:** v2.6 · **Date:** 2026-10-01 · Companions: `DESIGN.md` (how it looks), `ARCHITECTURE.md` (how it is built) · Changes: §13

**Supported hardware:**
- MSI **MPG Ai1300TS** and **MPG Ai1600TS** PSUs, connected by USB.
- **Any GPU** powered through the PSU's 12V-2x6 connectors.
- **Both connectors** (#1 and #2): one GPU on either connector, or two GPUs.

**Distribution:** public GitHub project. The app must not assume anything about the user's machine beyond the list above.

MeltAlarm is a lightweight Windows tray app. It watches the per-wire currents that MSI MPG Ai1x00TS PSUs measure on their two 12V-2x6 connectors, shows them at a glance, and raises an impossible-to-miss alarm when the cable is in danger.

**One alarm, two judges.** The alarm fires when **MeltAlarm's own cable limits** see a wire overloaded, or when the **PSU's GPU Safeguard+** raises its own alarm, whichever comes first (§6, §8). MeltAlarm's limits come from the physical connector, not from the PSU, so they are the same for every PSU and every GPU. The PSU's verdict is relayed unchanged, with its power-cut countdown. The user sees one alarm; its details say who raised it.

MeltAlarm is a **notifier, not a protector**. It cannot reduce the load or cut power; only the PSU firmware can. MeltAlarm makes sure the user *knows* in time to react, earlier than the PSU's own policy would tell them (F21).

---

## 1. Research findings

Measured on 2026-09-27 on a **reference system** (Ai1300TS, fw `MFR_Version 1.0`, rev `10`, RTX 5090 on connector #1) with a read-only R&D probe (v1 publishes it as the `psu-probe` tool). Raw captures are kept privately because they contain the unit serial.

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
| F11 | An unused connector reads **exactly 0.000 A**. The used connector at idle reads 0.06–0.19 A per wire. | idle watch | Presence detection (§6.7) |
| F12 | Healthy cable at full load (2× FurMark, ~575 W, 47–50 A): hottest wire **max 8.56 A**, imbalance (max−min) **max 0.69 A**, relative deviation max 5.2 %. At idle, relative deviation reaches ~40 % from quantization noise. | load watch, 180 loaded samples | **Deviation is measured in amps, not percent**; the healthy peak sits below the rating with margin (§6.2) |
| F13 | A Windows sound scheme can remap "Critical Stop" (`SystemHand`); on the reference system it plays `Windows Foreground.wav`. The file `C:\Windows\Media\Windows Critical Stop.wav` (0.9 s) is standard on Windows 10/11. | registry, filesystem | Play the file, not the alias (§8.3) |
| F14 | English SAPI voices ship with Windows (David, Zira, Mark…). | registry | Voice needs no install |
| F15 | Ctrl+Alt+G was free on the reference system. | `RegisterHotKey` test | Hotkey; a registration failure is handled (§8.5) |
| F16 | **Other PSU readers vary per user.** Seen on the reference system: MSI Center service (running, then stopped), Afterburner PSU plugin (installed, disabled), HWiNFO64 (portable). The mutex existed even with the MSI Center service stopped. | processes, configs | App must work with any combination (§5.2, test T3) |
| F17 | **With HWiNFO64 running:** our mutex waits reached **258 ms**, versus ≤ 2 ms with MSI Center alone, so HWiNFO evidently uses the same mutex. On 109 of 180 transactions our input queue held *foreign* replies. Draining the queue plus the echo check gave **0 failures and 0 wrong frames**. | 60 s watch with HWiNFO sensors open | Mutex timeout 2 s; drain + echo check are mandatory (§5.2) |
| F18 | A minimal Rust prototype (tray icon with 6 squares redrawn at 1 Hz, HID handle open) uses **1.5 MB private memory**, 9.4 MB working set (mostly shared system DLLs), and 16 ms CPU per 15 s. | footprint prototype | Footprint budget (§10) |
| F19 | **Cold boot without MSI software:** the PSU enumerates on USB but does **not answer** `51` reads (every attempt timed out for 2 min). MSI Center's own `CONNECT_PSU` sends `00 FA 51` once per connection, under the mutex, and requires the echo `FA 51` with byte 3 ≠ `FE`. Its per-tick reads (`Get`) send only `51 <reg>`. The Afterburner plugin sends the same handshake on its connect, so this PSU routinely receives it from several clients. What `FA 51` does inside the firmware is not documented. | MeltAlarm log 2026-09-28 00:06; decompiled MSI Center `cPSU.CONNECT_PSU` and `cPSU.Get` | Handshake on every connect, never per tick (§3, §5.1) |
| F20 | **The weak point is the contact, and it heats in seconds.** A 12V-2x6 contact is rated about **9.5 A** (Amphenol spec). Local heat is `I² × R_contact`: at 12 A a 10 mΩ contact dissipates 1.4 W, at 16.5 A 2.7 W. A conditional model (50 °C ambient, 60 K/W, 0.3 J/K) reaches 105 °C in **18 s at 12 A** and **7 s at 16.5 A**. These are scenarios, not measured MSI temperatures. The six ground returns are not measured at all, and a slow, even degradation of all contacts shows no imbalance. | Research brief 2026-09-30 (connector ratings, Hardware Busters teardown, thermal scenarios) | MeltAlarm's own limits come from the connector rating (§6.1). Documented honestly as engineering defaults, not safe limits (§6.1, §8.7). |
| F21 | **The PSU's policy is too slow for the contact.** Safeguard+ raises its status after `TriggerTime` (20 s) and cuts power `Warning_Time` (180 s) later; Hardware Busters measured the warning at ~12.5 A and an immediate shutdown only at **~17 A**. A wire at 16.5 A may run **~200 s** before the cut. An even overload below the PSU's limit (e.g. 11.9 A on every wire) never raises it at all. | MSI's published sequence; Hardware Busters Ai1300TS review; our simulation 2026-09-30 | The PSU's verdict can't be the only alarm (§6.3, §8.1) |
| F22 | **Windows holds notifications back during games.** Windows 11 turns on Do Not Disturb automatically while a game or a full-screen app runs (on by default); notifications then go silently to the notification center and are there after the game. | Windows 11 notification settings | Notifications are the right channel for "after the session" advice, and the wrong one for anything urgent (§8.8, §8.9) |

**Not verified, and not provoked on purpose (hardware safety):**
- whether an `E0` reading is an instantaneous value, an average or a held peak (matters for the one-reading 15 A rule, §6.3)
- PSU behavior during a real alarm
- the firmware's exact imbalance formula
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
- the alarm (overlay + sound + voice) from MeltAlarm's cable limits or the PSU's status; cautions, advisories and cable notes (§8)
- settings (the cable limits are not in the Settings window; advanced users edit the file, §6.1)
- event log
- compatibility check
- autostart
- install, update and uninstall, done by the exe itself (§4)

**Not in v1:**
- Automatic update checks (the app uses no network, §10). Package managers such as winget.
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
3. A unit test asserts every constructible packet. A lint forbids every hidapi call that sends bytes to a device (`write`, `send_output_report`, `send_feature_report`, `get_feature_report`), except the one reviewed `write` call site per source (ARCHITECTURE §9).
4. The app never modifies MSI Center, Afterburner, HWiNFO or their configs.

---

## 4. Process model, privileges, distribution

**Lifecycle rules for every platform.** §4.1–4.8 are how Windows meets them. Linux (next iteration) meets them in its own way (ARCHITECTURE §11.1).

| # | Rule | Windows | Linux (planned) |
|---|---|---|---|
| L1 | Install, update and uninstall use the **platform's normal mechanism**. | The exe does it itself; no package manager is assumed. | The distro package or AppImage. The app offers no install/update/uninstall of its own. |
| L2 | Autostart never grants more rights than the program file's location protects. | Elevated at logon → only the copy in Program Files (§4.3). | Runs unprivileged; device access comes from a udev rule installed by the package. |
| L3 | Program and user data are separate. Update and uninstall keep user data unless the user asks otherwise. | §4.3, §4.7 | XDG config/state folders |
| L4 | Where the app replaces itself, the user is never left without a working monitor, and replacement is refused during an alarm. | §4.6 | not applicable: the package manager replaces the file, and the running instance continues until restarted |
| L5 | Every monitoring gap caused by install, update or uninstall is logged (§9). | §4.6, §4.7 | not applicable (no gaps: the running instance is not stopped) |
| L6 | A second launch never starts a second monitor; it brings the running one forward. | §4.1 | same |

### 4.1 Process and privileges
- **Single executable** (Rust) and no external DLLs (hidapi is linked through its `windows-native` backend). There is **no separate installer**: the same file runs, installs, updates and uninstalls itself (§4.4–4.7).
- **Runs elevated** (manifest `requireAdministrator`). Elevation is needed because of F3: any other PSU reader may own the mutex, and admin is the minimum that can use it. A SYSTEM service is not needed.
- **Autostart** is a Task Scheduler task `MeltAlarm` that runs at logon of the user who enabled it, with "Run with highest privileges". It starts elevated **without a UAC prompt**. It always starts the **installed copy** (§4.3), never a downloaded file.
- A manual launch shows one UAC prompt (expected).
- **Single instance.** A second launch of the same copy never starts a second monitor. It brings the running instance forward (its Settings window; before that window exists, the status popup of the first tracked connector) and exits.
- Side benefit of elevation: the alarm overlay can also appear above elevated apps.

### 4.2 Distribution
- A GitHub release holds the exe and a `SHA256SUMS` file. The downloaded file's name and folder don't matter (browsers may rename it, e.g. `meltalarm (1).exe`).
- v1 is unsigned. On first launch Windows SmartScreen shows "Windows protected your PC"; the README explains *More info → Run anyway* and how to check the SHA256.
- The app uses no network, so it never checks for updates. Users learn about new versions from GitHub (*Watch → Releases*).

### 4.3 Where it lives, and why

| What | Where | Scope |
|---|---|---|
| Program | `%ProgramFiles%\MeltAlarm\meltalarm.exe` | machine |
| Start menu shortcut | *MeltAlarm*, in the all-users Start menu, so the app can be started again after *Exit* | machine |
| Installed-apps entry | *Settings → Apps → Installed apps*: name, version, *Uninstall* | machine |
| Startup task | `MeltAlarm` (§4.1) | the user who enabled it |
| Settings | `%APPDATA%\MeltAlarm\settings.toml` (§7.3) | user |
| Cable notes, open alarm | `%APPDATA%\MeltAlarm\state.toml` (§8.10) | user |
| Alarm log | `%LOCALAPPDATA%\MeltAlarm\alarms.log` (§9) | user |

**Why Program Files.** The startup task gives the program administrator rights at every logon without asking. If the task pointed to a folder the user can write to (Downloads, `%LOCALAPPDATA%`, the desktop), any unelevated program could replace the file and receive administrator rights at the next logon. Program Files is writable only by administrators. Hence the rule: **the startup task only ever points to the installed copy in Program Files.**

### 4.4 What a launch does
Checked in this order when the exe starts (unless started with `--portable` or `--uninstall`):

1. **This is the installed copy** → normal start (single instance, §4.1).
2. **A MeltAlarm is already running from this same file** → second launch (§4.1).
3. **Nothing is installed** → the Install dialog (§4.5).
4. **Another copy is installed** → compare versions (§4.6).

The dialogs appear before monitoring starts, and they work with or without a PSU connected.

### 4.5 Install

> **Install MeltAlarm 0.2.0?**
> It will be copied to Program Files and get a Start menu entry. You can uninstall it from Installed apps.
> ☑ Start with Windows
> **[Install]** [Run without installing]

- **Install:**
  1. stops any running MeltAlarm (from any folder)
  2. copies itself to Program Files, creates the shortcut and the installed-apps entry
  3. creates the startup task if *Start with Windows* is checked, and stores that choice as the *Run at Windows startup* setting
  4. starts the installed copy, which shows the "pin the tray icon" hint (§7.1); this copy exits. The downloaded file can then be deleted.
- **Install is all or nothing.** If a step fails (disk, antivirus, permissions), the steps already done are undone, an error names the failed step, and this copy continues as a portable one.
- **Run without installing** (portable): monitoring works normally, with no startup task. *Run at Windows startup* is replaced by **Install…**, which opens this dialog. The question is asked on every launch of a portable copy (use `--portable` to skip it, §4.8).

### 4.6 Update
Updates are manual: the user downloads the new exe and runs it. When another copy is installed, the versions are compared:

| This file vs installed | Result |
|---|---|
| newer | **"Update MeltAlarm 0.1.0 → 0.2.0?"** [Update] [Cancel] |
| older | **"Replace MeltAlarm 0.2.0 with the older 0.1.0?"** [Replace] [Cancel]. This is how a bad update is rolled back. |
| same | no dialog: acts as a second launch (§4.1). If the installed copy isn't running, it is started. |

- **Update / Replace:** the running MeltAlarm is stopped (logged, §9), the installed file replaced, the installed-apps version updated, and the installed copy started. Settings, the log, the startup task and pinned tray icons are kept. Monitoring pauses for a few seconds.
- **Refused during an alarm** (either judge): "A cable alarm is active. Update MeltAlarm after it has cleared." Stopping the app would silence the alarm. Install is refused the same way while another copy is in an alarm.
- **Never leaves the user without a working MeltAlarm.** If replacing fails, the previous version stays installed and is started again.
- A running MeltAlarm that hasn't closed 5 s after being asked (an older version that doesn't understand the request, or a hung one) is terminated.

### 4.7 Uninstall
Two entry points with the same result: **Uninstall…** in Settings (in v0, the tray menu) and the installed-apps entry (which runs `meltalarm.exe --uninstall`).

> **Uninstall MeltAlarm?**
> Monitoring stops and MeltAlarm will no longer start with Windows.
> ☐ Also delete my settings and the alarm log
> **[Uninstall]** [Cancel]

- Stops any running MeltAlarm, then removes the startup task, the Start menu shortcut, the installed-apps entry and the program folder. With the box checked, it also removes the settings and log folders.
- The box is **unchecked by default**: the alarm log may be the only record of a cable incident.
- Nothing else is left on the machine (and §3.4 still holds: other PSU software is never touched).
- A portable copy is never installed, so it offers no *Uninstall*. Deleting the file removes it; its settings and log stay in the folders of §4.3.

### 4.8 Developer runs
- `--portable` skips §4.4–4.6: no dialogs, no install, no startup task. Used for development, CI and deliberate portable use.
- `simulate` builds (dev only) never install. They run unelevated and use their own data folders.

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

Per connector *c* (1 or 2), from each valid `E0` sample: wires `I1..I6`, `max`, `min`, **imbalance** `= max − min` (called *spread* or *Delta* in research notes), `avg` (mean of the six), `median`.

Two independent judges look at the same connector:
- **MeltAlarm's cable limits** (§6.1–6.4): computed from the six currents alone. They work for any source that reports per-wire currents, with or without a verdict of its own.
- **The PSU's verdict** (§6.5): the `C1` status, relayed as it is.

Either one can raise the alarm (§8.1). Neither overrides the other.

### 6.1 Cable limits

Physical limits of the 12V-2x6 connector (F20), the same for every PSU and GPU. They are **engineering defaults, not validated safe limits**: MeltAlarm can't see the ground returns, and an even degradation of all contacts stays invisible (§8.7).

| Limit | Default | Used for |
|---|---|---|
| **Rating** | 9.5 A per wire | Caution (§6.4). An overload episode ends below it (§6.3). |
| **Alarm limit** | 10.5 A | An overload episode starts here; the alarm follows after **4 s** |
| **Fast limit** | 12.0 A | The alarm follows on the **second** reading at or above it |
| **Instant limit** | 15.0 A | The alarm follows on **one** reading |
| **Uneven load** | imbalance 3.0 A while `avg` ≥ 3.0 A | Advisory (§6.4) |

- The defaults are versioned (**limits v1**) and logged at start (§9).
- They are **not in the Settings window**. Advanced users can override any of them in `settings.toml` (keys in §7.3). Only overridden values are written to the file, so users on defaults get improved defaults with updates.
- An override set that breaks the order *rating < alarm limit < fast limit < instant limit*, or a value outside 5–30 A, is ignored as a whole: the defaults apply and a `CONFIG` warning is logged.
- Where the limits are shown (Settings, the log), overridden values are marked **custom**.
- The PSU's own thresholds (`C0`) no longer drive colors or text. They are shown only as the PSU's information (§7.3) and drive its countdown (§8.4).

### 6.2 Live levels (colors)

Instant, per sample, no debounce. Colors only; **a level alone never interrupts the user**.

| Level | A wire | The imbalance |
|---|---|---|
| **Warning** (red) | `I ≥ alarm limit`, or the PSU flags the wire in `E1` | the PSU reports status *Current imbalance* |
| **Caution** (amber) | `I ≥ rating` | `imbalance ≥ 3.0 A` and `avg ≥ 3.0 A` |
| **Normal** | otherwise | otherwise |

**Attribution of an imbalance level to wires.** When the imbalance is at caution level, every wire with `|I − median| ≥ 1.5 A` (half the uneven threshold) gets Caution. Math guarantees at least one wire: the median lies between min and max. This points at the outlier, not at an innocent neighbor. A wire's color is the highest of its current level, its attributed imbalance level and its `E1` flag.

On the reference system a healthy cable at 575 W peaks at 8.56 A and 0.69 A imbalance (F12): Normal with margin.

### 6.3 Overload rule (MeltAlarm's alarm)

Per wire, on valid samples:
- An **overload episode** starts at the first reading `≥ alarm limit`. It lasts until the wire reads **below the rating** (hysteresis), so normal noise around 10.5 A (10.4, 10.6, 10.4…) keeps one episode alive instead of restarting it.
- Within an episode, **the alarm is raised** when a reading arrives that is:
  - `≥ instant limit`, or
  - `≥ fast limit` and is the second such reading of the episode, or
  - `≥ alarm limit`, and the episode started at least **4 s** earlier.
- The rule triggers only on a **fresh sample**; it never fires because a timer ran out without data.
- **Gaps:** consecutive samples more than **3 s** apart, a failed or malformed sample, or NO DATA break every episode; it restarts on the next valid sample. Missing data is never read as zero current.
- Identical consecutive values count as fresh readings.

**The connector's overload lasts** from the triggering reading until a valid sample shows **every wire below the rating**. A gap or NO DATA never ends it (§8.6).

Why these numbers (simulated on the reference captures and synthetic faults, 2026-09-30):
- A healthy card at 575–600 W with ±0.5 A noise, and a 13 A spike once a minute: no alarm.
- A single 11 A reading: no alarm.
- Two 12.2 A readings: alarm after 1 s.
- One 16.5 A reading: alarm at once.
- Two contacts open at full load (the others at 11.9 A): alarm after 4 s. The PSU alarms after 20 s.

The one-reading 15 A rule favors early warning and can in principle fire on a harmless peak. With 1 Hz samples, no rule can promise both no false alarms and instant detection. This tradeoff is documented, not hidden (open item §12).

### 6.4 Caution and advisory episodes

Colors (§6.2) are instant. Anything that **interrupts** needs the condition to persist first.

| Episode | Starts | Qualifies (the user is told) | Ends |
|---|---|---|---|
| **Caution: wire above rating** | a wire `≥ rating` | after **10 s** with no wire of the connector falling below `rating − 0.5 A` in between | after **30 s** with every wire below `rating − 0.5 A` |
| **Advisory: uneven load** | `imbalance ≥ 3.0 A` and `avg ≥ 3.0 A` | after **10 s** with `imbalance ≥ 2.5 A` and `avg ≥ 3.0 A` throughout | after **30 s** with `imbalance < 2.5 A` or `avg < 3.0 A` |

- Each episode tells the user **once**, when it qualifies (§8.8, §8.9). A condition that keeps coming and going within the 30 s end window stays one episode.
- Gaps (> 3 s) and NO DATA break qualification but don't end an episode that already qualified.
- **Uneven load alone is only an advisory.** The contact that loses current is not the hot one; the danger is the neighbors that take over its share, and they are caught by the wire rules (caution at 9.5 A, alarm at 10.5 A). A severe imbalance also raises the PSU's own alarm (§6.5). The user can't reseat a cable mid-game anyway, so the advice waits until after the session (F22).

### 6.5 PSU status (relayed)
The `C1` status byte per connector:

| Code | Name shown to user | Meaning |
|---|---|---|
| 0 | Normal | — |
| 1 | Over-current | A wire has been above OCP for OCP_TriggerTime |
| 2 | Current imbalance | The imbalance has been above Diff for Diff_TriggerTime |
| 3 | Critical over-current (>18 A) | Hard firmware limit; power cut may be immediate |
| other | Unknown PSU alert (0xNN) | Treated as an alarm |

Any non-zero status raises the alarm (§8.1), **whatever our own readings say**: the PSU may have seen something between our 1 Hz samples, and it is the one that will cut power. Wires flagged in `E1` turn red.

Our overload and the PSU's status are independent. Each one raises, holds and clears its own cause; the alarm lasts while any cause is active. They can and will disagree in both directions (the PSU is slower by design, F21; it may also catch a spike we missed).

### 6.6 Other PSU flags
The `E1` rail OCP (12V/5V/3.3V), OPP, OTP and Fan flags are **PSU faults**:
- each newly set flag is a **caution** (§8.8): the PSU may shut down on its own
- shown in the popup as a line while set, with the attention marker on the tray icon
- logged when set and when cleared

### 6.7 Connector presence
A connector counts as *connected* if any wire read > 0 A in the last 10 s (F11). This drives "In use" / "No load" in Settings and the first-run default.

### 6.8 Stale data
If the last valid sample is older than 1 tick, values are shown as stale (dimmed). After NO DATA (§5.4), everything is grey.

**Losing monitoring is itself a caution** (§8.8): when NO DATA begins after the app has had data, the user is told once, because they would otherwise believe they are protected. Suspend and resume are not a loss.

### 6.9 PSU protection disabled
If the PSU reports its own protection as off (MSI: Safeguard+, `C0.IsEnable = 0`):
- the popup shows the persistent line *"PSU Safeguard+ is OFF: the PSU won't cut power. MeltAlarm still alarms."* The protection's name comes from the source; MeltAlarm's own text never names a vendor feature.
- the tray icon shows the attention marker
- the event is logged

This is a **status** (§8), not an episode: nothing interrupts the user. The app never re-enables it (read-only).

---

## 7. User interface

### 7.0 Names and words

- **The thing we watch is the "GPU power cable"**, everywhere the user reads it: screens, notifications, voice, Settings and the log. "12V-2x6" appears only in the README, which bridges the two: *"MeltAlarm watches the GPU power cable: the 16-pin cable from your PSU's 12V-2x6 socket to the graphics card. Cable 1 and cable 2 are the PSU's two 12V-2x6 sockets, numbered as on the PSU."*
- **The number appears only when it tells something.** With one tracked cable (most users) the screens say just *GPU power cable*. The number is added (*GPU power cable 2*, short *Cable 2* where space is tight) when more than one cable is tracked, or when the cable isn't tracked (an alarm on it must say which). Settings lists both sockets, so it always numbers. The **log always numbers**: it is a permanent record and stays unambiguous after a second GPU is added.
- **No wire numbers on screen.** Which physical pin a PSU wire number is has not been verified (§1), so "wire 3" can't send anyone to a pin. Screens say *a wire* / *one wire*, and the affected bar carries the color. The log keeps the wire index for diagnostics.
- **Imbalance**, not "spread": the difference between the most and the least loaded wire. The event of too much imbalance is *uneven load*.
- **No total current.** The sum of the wires is roughly the GPU's power draw; no decision depends on it and GPU tools show it in watts. It is not shown.

### 7.1 Tray icons
- One icon per **tracked** cable (1 or 2), in the fixed order 1 then 2.
- The glyph is **6 squares in 2 rows of 3**. Square *n* = wire *n* (row 1: wires 1–3, row 2: wires 4–6), colored per §6.2.
- Visual states (the look: DESIGN.md "Tray icon"):
  - neutral (normal, "dark cockpit") / amber / red squares
  - **alarm on this connector** (ours or the PSU's): must look distinct from red squares (blinking tile)
  - **grey**: NO DATA, or connector not connected
  - **connecting**: before the first successful connection, a single hollow icon with the tooltip *"MeltAlarm · connecting to the PSU…"* (§5.1)
  - **attention marker** (one meaning: "there is something to read in the flyout"): a cable note (§8.10), a PSU fault (§6.6) or Safeguard+ OFF (§6.9)
- **One vocabulary on every surface** (tooltip, flyout chip, floating view): `OK`, `Caution`, `ALARM`, `No data` (plus `Not connected` in the tooltip).
- Tooltip, one line (Windows wraps tray tips at about 50 characters): `MeltAlarm · OK · max 8.6A · Δ 0.7A` (Δ = imbalance); with two tracked cables `MeltAlarm · Cable 2 · OK · max 8.6A · Δ 0.7A`. Other states: `Caution · max 9.9A · Δ 0.7A`, `ALARM: <short reason>`, `No data`, `Not connected`. With a cable note and no live problem: `MeltAlarm · OK · check the cable`.
- Left click toggles the status popup for **that icon's connector**.
- Right click opens a menu: *Settings…*, *Open alarm log*, *Exit* (Exit asks for confirmation: "Monitoring will stop"). The menu header shows the app version. Until the Settings window exists, the settings sit in this menu (§7.3).
- Windows 11 hides new tray icons in the overflow area by default. First run explains how to pin the icon.

### 7.2 Status popup
- Anchored right above the clicked icon, using the icon's actual position. If that position can't be determined (e.g. the icon is in the overflow area), it anchors near the cursor.
- Content:
  - the cable's name (§7.0), then the state chip (`OK` / `Caution` / `ALARM` / `No data`)
  - during an alarm: a red strip with the reason (e.g. `Wire overload · 12.4 A`, or `PSU: Current imbalance`) and, from the PSU, the countdown (§8.4)
  - **6 vertical bars**, one per wire, in the PSU's wire order, with the current under each (1 decimal), no wire numbers. The top of a bar is the **alarm limit**; the bar is **cut at the rating**, so its short top part is the caution zone. The scale gives the decision range room: 0–6 A take the bottom fifth, 6 A to the alarm limit the rest (DESIGN.md "Popup"). No lines or numbers mark the limits; colors only show state.
  - live lines, while true: wire above the rating or over the alarm limit, uneven load, PSU faults, Safeguard+ OFF, monitoring interrupted
  - the **cable note** (§8.10), with *Dismiss*
  - footer: **Imbalance** and **PSU status**: the PSU's own verdict in words (`Normal`, `Current imbalance`, `Safeguard+ off`; `—` for a source without a verdict)
- Updates live at 1 Hz while open. Closes on focus loss or Esc, like native flyouts.
- In NO DATA: "Monitoring interrupted", with the age of the last valid reading.
- A **pop out** button (top right) and dragging the header turn it into the floating monitor (§7.4).

### 7.3 Settings window

> **Until v1:** there is no Settings window yet. The three settings below plus *Test alarm* sit in the tray right-click menu as checkmark items, together with *Install…* (portable copy) or *Uninstall…* (installed copy). The window arrives in v1, and the menu then shrinks to *Settings… / Open alarm log / Exit*.

| Setting | Default | Notes |
|---|---|---|
| Run at Windows startup | **On** | Installed copy only: creates or removes the startup task (§4.1). A portable copy shows an **Install…** button here instead (§4.5). |
| Track GPU power cable 1 ☐ ● / Track GPU power cable 2 ☐ ● | First run: the cable(s) currently *connected*; if none, cable 1 | At least one must stay checked (the last checked box is disabled). The **●** hint dot is live: green = in use, **grey** = no load (§6.7). An unused connector is normal, never red. |
| Alerts (on screen, sound, voice) | **On** | Master switch for every interruption: the alarm, the caution strip, advisory and after-alarm notifications (§8). Off means colors, cable notes and the log only. The log is always written. The file key stays `alarm`. |

- **Limits** (read-only), both judges side by side, in sentences:
  > **MeltAlarm** alarms when a wire stays above 10.5 A for 4 s (sooner above 12 A). Amber from 9.5 A per wire or a 3 A imbalance.
  > **Your PSU** alarms at 12.0 A per wire or 5.5 A imbalance after 20 s, and cuts power 180 s later.

  Overridden values are marked *custom*. For a PSU without a verdict the second sentence reads "Your PSU reports currents only".
- Read-only info block: MeltAlarm version, PSU model, firmware, serial, and the state of the PSU's protection (MSI: Safeguard+).
- Buttons: **Test alarm** (§8.5), **Open log folder**, and **Uninstall…** (installed copy only, §4.7).
- Settings apply immediately. They are stored in `%APPDATA%\MeltAlarm\settings.toml`.
- **Cable limits in the file only** (§6.1), for advanced users; the README documents them: `limit_rating`, `limit_alarm`, `limit_alarm_seconds`, `limit_fast`, `limit_instant`, `limit_uneven` (amps, seconds). Read at start.

### 7.4 Floating monitor

**Why:** the popup is for a glance; it closes as soon as another window is clicked. To *watch* a connector during a stress test or a game, the user takes its view out of the tray and places it anywhere, including a small dedicated display. Design: DESIGN.md "Floating monitor".

Each tracked connector has **one view**, in one of three states: *hidden*, *flyout* (the §7.2 popup) or *floating*. There is never a flyout and a floating view of the same connector at once.

| Event | Hidden | Flyout | Floating |
|---|---|---|---|
| Tray icon left click | → flyout | → hidden | **locate:** bring to front, move fully onto a visible display if needed, pulse once |
| Pop-out button in the flyout | — | → floating, at the last floating placement; the first time, in *Full* at the flyout's spot | — |
| Drag the flyout's header | — | → floating in *Full*, where it is dropped | — |
| × (floating) | — | — | → hidden; the next tray click opens the flyout |
| Click on another window | — | → hidden | stays |
| Right click | tray menu | tray menu | tray menu (on the floating view too) |
| Connector un-tracked | — | closes | closes, and its placement is forgotten |
| Exit, restart, reboot | — | — | restored at the next start once the connector reports |
| No data, not connected, caution, alarm | — | shown in the view | shown in the view; the caution strip and the alarm notch (§8) still appear on top |
| Display removed, resolution or DPI change | — | — | moved fully onto a visible display (the primary one if its own is gone); scale kept |

**Floating view rules:**
- Always on top, and **never takes focus**: clicking or dragging it doesn't take keyboard or mouse focus from a game. No taskbar button, not in Alt+Tab.
- Drag anywhere on it to move. Drag an edge or corner to scale it **uniformly**, from 75 % up to the size of its display. No free aspect ratio, no snapping.
- Two layouts, switched with the tab on its bottom edge: **Compact** (bars, values and one summary line) and **Full** (the §7.2 content). Compact never changes size with the state.
- The Compact summary line shows the most important thing, in this order: the alarm reason (`Overload · 12.4 A · stop` / `Imbalance · cut ~2:13`), a live caution (`9.9 A · over rating`), uneven load (`Imbalance 4.1 A`, caution color), no data (`Last reading 12 s ago`), a cable note (`Check the cable`), otherwise `Imbalance 0.4 A`.
- Its controls (×, tab, resize grip) appear only while the mouse is over it.
- Exclusive-fullscreen games hide it (like the notch, §8.7). Borderless games don't.

**Remembered per connector**, saved at every change: floating or not, the display (by its hardware identity), the position on that display, the layout, and a scale for each layout. The file sits next to the settings (`%APPDATA%\MeltAlarm\window.toml`). Deleting it just resets the placements.

---

## 8. Alerts

**The alert ladder.** Every abnormal condition sits on exactly one level. Each level has **one** attention channel, and each channel means exactly one thing (aviation practice: warning, caution, advisory, status).

| Level | Triggers | Gets attention | Sound | Stays afterwards |
|---|---|---|---|---|
| **Alarm** (red): act now | MeltAlarm's overload (§6.3), the PSU's status (§6.5) | the **notch** (§8.2) until cleared or snoozed | alarm cycle + voice (§8.3) | cable note; a notification at the next start if the session ended during the alarm (§8.10) |
| **Caution** (amber): be aware now | a wire above the rating (§6.4), monitoring lost (§6.8), a PSU fault (§6.6) | the **caution strip** (§8.8): 10 s, once per episode | **one chime** | cable note (wire above rating only) |
| **Advisory**: deal with it after the session | uneven load (§6.4) | a **silent Windows notification** (§8.9), which Windows holds back during games (F22) | — | cable note |
| **Status** | Safeguard+ OFF (§6.9) | the attention marker and a flyout line | — | while true |

- **Colors are live, interruptions are earned:** tray, flyout and floating views show the instant levels (§6.2); only a persistent, qualified condition interrupts.
- **Higher levels supersede:** while the notch is shown, no caution strip appears. A caution that becomes an alarm grows into the notch.
- The **Alerts** switch (§7.3) turns off every interruption (notch, strip, chime, sound, advisory and after-alarm notifications). Colors, cable notes and the log stay.
- Every level covers **all connectors, tracked or not**. Tracking only controls icons; a safety event is never filtered out.

### 8.1 Trigger
The alarm fires when **either judge** raises it on any connector, and Alerts are on:
- **MeltAlarm's overload** (§6.3), or
- **the PSU's status** (`C1`) is non-zero (§6.5).

Each (connector, cause) pair is tracked separately. The alarm lasts while any pair is active.

### 8.2 Overlay ("notch")
- Attached to the **top edge, centered, on every monitor**. Flat top edge; rounded bottom corners.
- **Size:**
  - width = 32 % of the monitor's width, clamped to **560–880 DIP**
  - height = fits the content, about **280 DIP** with the snooze button
  - This is readable at arm's length without covering the middle of the screen. Widths: 1920×1080 → ~614 px; 2560×1440 → ~819 px; 4K at 150 % → 880 DIP.
- Topmost and **never steals focus**, so the game keeps input and a borderless game doesn't minimize. Clicking the button is the only interaction.
- Content, in priority order (flat, high contrast, readable at a glance):
  1. **Headline:** `GPU POWER CABLE OVERLOAD`
  2. **Action:** `STOP GPU LOAD NOW`, plus one line that says why
  3. **Details:** mini bars, what happened and who says so, the numbers, and a right-hand block:

     | Cause | What | Right-hand block |
     |---|---|---|
     | MeltAlarm's overload | `Wire overload · measured by MeltAlarm`; `A wire carries 12.4 A, rated 9.5 A. The PSU hasn't raised an alarm yet.` | `HIGHEST WIRE` · `12.4 A` |
     | the PSU's status | `Current imbalance · reported by the PSU`; the currents (`Lowest wire 2.1 A, the others 9.7–9.9 A. Imbalance 7.8 A, PSU limit 5.5 A.`) | `POWER CUT IN` · `~2:47` (§8.4), or `ANY SECOND` for status 3 |
     | both on one cable | the PSU's line, then our wire line | the PSU's block |

     The band names the cable (§7.0); for two cables in alarm, `GPU power cables 1 + 2`.

  4. **Big button:** `SNOOZE 30 s   (Ctrl+Alt+G)`

### 8.3 Sound and voice
- **Cycle** (repeats while the overlay is visible):
  1. `Windows Critical Stop.wav`, played **3 times** with ~0.3 s gaps
  2. **one voice line**
  3. repeat

  One cycle takes about 8 s. The WAV is 0.9 s long.
- Voice line (built-in English SAPI voice): *"Warning. GPU power cable overload. Stop the game now."* When the cable carries a number (§7.0) it adds it: *"…overload on cable 2."* The same line for both judges.
- The sound plays **from the file** `%WINDIR%\Media\Windows Critical Stop.wav`, not the system alias (F13). If the file is missing, the fallback is the system default beep.
- Played at the **current system volume**; no forced unmute. The PSU's own buzzer sounds regardless.
- Output goes to the default audio device. While streaming VR (VD/Link) that is the headset, so this is how v1 alerts VR users.
- The voice engine is loaded only for an alarm and released afterwards (§10).
- The alarm sound and the caution chime (§8.8) are distinct by design; the voice speaks only in an alarm.

### 8.4 Countdown
Only for the PSU's status. `remaining = Warning_Time − (now − first tick with non-zero status)`, shown as `~m:ss` and labeled approximate. At 0 it reads *"PSU power cut expected now"*. Snoozing doesn't reset it. If the PSU status returns to 0, the countdown ends. MeltAlarm's own overload has no countdown: the PSU isn't cutting power yet, which is exactly the danger (F21).

### 8.5 Snooze and Test
- The button or **Ctrl+Alt+G** hides the overlay and stops sound and voice for **30 s**.
- The hotkey is registered only while the overlay is visible. If registration fails because another app took the combination, the overlay shows the button without a hotkey hint.
- After 30 s the app checks fresh data. If any cause is still active, the full alarm shows again. If none is, nothing is shown.
- Snooze is unlimited.
- **Escalation cancels snooze immediately:** a new (connector, cause) pair becomes active, e.g. the PSU joins our overload, the PSU's code changes or becomes 3, or another connector enters an alarm.
- **Test alarm runs the whole ladder**, so the user hears both sounds in a calm moment: the caution strip with its chime for 3 s (marked TEST), then the full alarm with a TEST chip, sound and voice, until snoozed or 30 s. A real alarm pre-empts a test; a test is never logged as an alarm and leaves no cable note.

### 8.6 Clearing
- A cause clears when its judge says so with fresh data: our overload when every wire of that connector is below the rating (§6.3); the PSU's status when it returns to 0.
- When no cause is left, sound and voice stop at once. The overlay turns **green** for 5 s, then disappears; this also happens if the alarm was snoozed. The green overlay doesn't say "all good": a lower current doesn't prove the cable is undamaged. It says *"Load back to normal. Inspect the cable before the next session; the note is in MeltAlarm."*
- If data is lost (NO DATA) during an alarm, the alarm **stays**, with the note "PSU data lost, cannot confirm". The app never assumes all-clear without data.

### 8.7 Known limitations (documented to users)
- MeltAlarm's limits are **engineering defaults, not guaranteed safe limits**. The six ground returns are not measured, and an even degradation of all contacts shows no imbalance (F20). An alert also needs a human to react.
- True **exclusive fullscreen** games (rare today; most use borderless or flip-model) can't be drawn over. The alarm sound, the voice and the caution chime still work.
- **VR:** no visual in the headset in v1. Sound, voice and the chime come through the headset audio.
- Locked workstation or disconnected RDP session: the overlay and the strip aren't visible, and sound depends on the session's audio.
- System muted: silent by the user's choice. The PSU buzzer still sounds.

### 8.8 Caution strip
A small amber strip at the same place as the notch: the top edge, centered, on every monitor. It is the notch's little sibling, so the user learns one location: *small and amber = look soon, big and red = act now*.

- **Appears** when a caution qualifies (§6.4, §6.6, §6.8), **once per episode**, with **one chime** (a short, soft chime, distinct from the alarm sound; exact sound chosen in design).
- **One line: what, where, what to do.** Examples:
  - `A wire at 9.9 A, above the 9.5 A rating · Ease the GPU load` (with two cables: `Cable 2 · A wire at 9.9 A, …`)
  - `Monitoring lost · MeltAlarm can't read the PSU`
  - `PSU fault · Fan failure · The PSU may shut down`
- **Shows for 10 s**, then disappears. Its text is a snapshot (no live flicker).
- A second caution during those 10 s replaces the text and restarts the 10 s, without a second chime.
- **Never interactive:** topmost, never takes focus, and **clicks pass through it** to the game.
- Not shown while the notch is visible (§8). When an alarm starts, the strip is replaced by the notch at once.
- Exclusive fullscreen hides it; the chime still sounds (§8.7).

### 8.9 Advisories and notifications
Windows notifications from the tray icon (DESIGN.md "Notifications"), used only for things that are **not urgent**:

| Notification | When | Sound | Click |
|---|---|---|---|
| *Uneven load on the GPU power cable* (or *on GPU power cable 2*) / *Check that the cable is fully seated at both ends. Details are in MeltAlarm.* | an uneven-load advisory qualifies (§6.4), once per episode, and no alarm is active (higher levels supersede) | silent | opens that connector's flyout |
| *Last session ended during a cable alarm* / *Inspect the GPU power cable with the PC off before gaming.* | at start, if the last session ended while an alarm was active (§8.10) | default | opens that connector's flyout |
| *MeltAlarm can't reach the PSU* (§5.1), *MeltAlarm is running* (§4.5), *already running* (§4.1) | as before | as before | as before |

During a game Windows holds notifications back silently and shows them in the notification center afterwards (F22). That timing is right for advice and wrong for anything urgent, which is why cautions use the strip.

### 8.10 Cable notes
**Anything that reached the user about the cable leaves a note until they dismiss it.** One note per connector.

| Event | Note |
|---|---|
| An alarm (either judge) | *"Alarm on Sep 30, 18:02: a wire reached 12.4 A. Inspect the cable and both connectors with the PC off before the next session."* For the PSU's status: *"PSU alarm on Sep 30, 18:02: Current imbalance. Inspect …"* |
| A wire-above-rating caution | *"A wire ran above the 9.5 A rating on Sep 30, 18:02 (peak 10.2 A). Check that the cable is fully seated, or lower the GPU power limit."* |
| An uneven-load advisory | *"Uneven load on Sep 30, 18:02: one wire carried 0.5 A while the others carried up to 9.5 A. Check that the cable is fully seated at both ends."* |

- **Severity** alarm > caution > advisory: an event replaces the connector's note if it is at least as severe. The peak is updated while the event lasts.
- **Created by the condition**, whether or not the Alerts switch let it interrupt. A test never creates one.
- **Shown** in the flyout and the floating *Full* view (with *Dismiss*) once the connector is back to `OK` (while a problem is live, the live line says it), in the Compact summary line (`Check the cable`), as the tray attention marker, and in the tooltip.
- **Dismiss** removes it. It is kept across restarts until then, in `%APPDATA%\MeltAlarm\state.toml`.
- **Session ended during an alarm.** The same file records whether an alarm is active. If the app starts and finds it set, the last session ended during an alarm. The usual reason: the PSU cut the power, which also killed MeltAlarm. The app then shows the after-alarm notification (§8.9) once, and clears the flag. The note already says what happened.
- Uninstall's "Also delete my settings" removes the file (§4.7).

---

## 9. Alarm log

- File: `%LOCALAPPDATA%\MeltAlarm\alarms.log`, UTF-8, one line per **event**, local time. Never per tick; live colors are never logged, only qualified episodes.
- Size: events are rare, so growth is tiny. As a safety net, at 5 MB the file is rotated to `alarms.old.log` (at most 2 files).
- Events:

```
2026-09-27 18:00:58 | LIMITS        | v1 · rating 9.5 A · alarm 10.5 A for 4 s, 12.0 A twice, 15.0 A once · uneven 3.0 A      (at connect; "v1 custom" marks overrides)
2026-09-27 18:00:59 | CONFIG        | Safeguard+ ON · OCP 12.0 A for 20 s · imbalance 5.5 A for 20 s · power cut 180 s after alarm   (at start and on change)
2026-09-27 18:01:40 | CAUTION       | GPU power cable 1 | wire 3 = 9.9 A above the 9.5 A rating for 10 s | wires 9.1 9.3 9.9 8.8 9.0 9.2
2026-09-27 18:02:11 | OVERLOAD      | GPU power cable 1 | wire 3 = 12.4 A · 2 readings >= 12.0 A | wires 9.1 9.3 12.4 8.8 9.0 9.2 | imbalance 3.6 A
2026-09-27 18:02:40 | OVERLOAD END  | GPU power cable 1 | 29 s | peak wire 3 = 12.9 A
2026-09-27 18:03:30 | CAUTION END   | GPU power cable 1 | 110 s | peak wire 3 = 12.9 A
2026-09-27 18:04:05 | UNEVEN LOAD   | GPU power cable 1 | imbalance 4.1 A at 7.9 A average for 10 s | wires 9.4 9.3 5.3 8.8 9.0 9.2
2026-09-27 18:05:40 | PSU ALARM     | GPU power cable 1 | status Current imbalance | flagged wires: 3 | wires 9.8 9.9 2.1 9.7 9.8 9.9
2026-09-27 18:05:40 | PSU RAW       | C1 51C102...       (when the alarm starts, then every 10 s while it lasts)
2026-09-27 18:06:22 | PSU CLEAR     | GPU power cable 1 | 42 s
2026-09-27 18:09:00 | UNEVEN END    | GPU power cable 1 | 295 s | peak imbalance 4.4 A
2026-09-27 18:10:00 | PSU FLAG      | OTP (PSU over-temperature) set
2026-09-27 19:00:00 | NO DATA       | PSU stopped answering
2026-09-27 19:00:12 | DATA BACK     | after 12 s
2026-09-27 20:00:00 | CONFIG        | WARNING: Safeguard+ is OFF on the PSU
2026-09-28 00:06:32 | NOT CONNECTED | PSU found but not answering (Timeout) — still trying     (once, 2 min after start)
2026-09-28 00:07:10 | CONNECTED     | MSI MPG Ai1300TS after 158 s                              (only if NOT CONNECTED was logged)
2026-09-29 08:02:00 | STOPPED       | Monitoring not started: MeltAlarm supports only MSI MPG Ai1300TS / Ai1600TS power supplies…   (startup exit, §5.1)
2026-10-02 18:00:00 | INSTALL       | MeltAlarm 0.2.0 installed · starts with Windows          (§4.5)
2026-10-05 18:00:00 | STOPPED       | Monitoring stopped: updating to 0.3.0                     (written by the updater, §4.6)
2026-10-05 18:00:02 | INSTALL       | updated 0.2.0 → 0.3.0                                      (or: replaced 0.3.0 with 0.2.0)
2026-10-09 18:00:00 | STOPPED       | Monitoring stopped: uninstalled                           (only if the log is kept, §4.7)
```

- Episodes follow §6.3–6.4: a condition that comes and goes within the 30 s end window is one episode, so one noisy minute produces one pair of lines.
- The log records conditions whether or not the Alerts switch let them interrupt; a *Test alarm* is never logged as an event.
- The log **covers all connectors, tracked or not**.

---

## 10. Non-functional requirements
| Area | Requirement |
|---|---|
| Footprint — idle | Only the tray icons visible. **Private memory ≤ 5 MB** (target ~2 MB; the F18 prototype uses 1.5 MB). Average CPU ≤ 0.1 %. No GPU rendering, so the dGPU is never kept awake. |
| Footprint — floating | A floating monitor may stay open for hours: **private memory ≤ 20 MB and flat** (no growth over time), CPU ≤ 0.2 % (one software redraw per second). *Measured 2026-09-30: 13–15 MB, the same as an open flyout; the cost is the text-rendering stack, not the window. The first target (≤ 10 MB) was set before measuring.* |
| Footprint — active | Popup, Settings or alarm open: higher memory is allowed temporarily (text rendering, voice engine, audio). Target ≤ 30 MB. **Released when the window or alarm closes**, back to the idle budget. |
| Binary | Single exe, target < 2 MB (the prototype is 175 KB). |
| Latency | PSU status change → alarm visible and audible ≤ 2 s. A qualifying reading (§6.3, §6.4) → alarm, strip or chime ≤ 1 s. |
| UI | English. Per-monitor DPI-aware. Popup and Settings follow the system light/dark theme; the alarm overlay uses its own fixed high-contrast scheme. |
| Robustness | Survives USB replug, sleep/resume, explorer.exe restart (tray icons re-added), and display changes (overlay re-laid out). |
| Coexistence | Any combination of MSI Center, Afterburner PSU plugin and HWiNFO64 (§5.2). |
| Network | None. No update check (§4.2). |
| Tech | Rust; Win32 + Direct2D/DirectWrite in software mode (see DESIGN.md). |

---

## 11. Acceptance tests
- **T1.** The read-only contract unit test passes. Clippy (with the deny-list) passes, and exactly one `#[allow(clippy::disallowed_methods)]` exists per source crate.
- **T2.** The installed app starts elevated via the scheduled task with no UAC prompt. A second launch brings the running instance forward and exits.
- **T3. Coexistence matrix**, 30 min each: MeltAlarm alone; + MSI Center; + HWiNFO64; + Afterburner PSU plugin; all together. Pass criteria: no NO DATA, fewer than 0.1 % failed samples, and the other tools keep showing sane values.
- **T4.** Unplug the PSU USB: grey icons within ≤ 3 s, NO DATA logged. Replug: live again, DATA BACK logged.
- **T5.** Colors on a synthetic frame feed (test build only):
  - every limit boundary (§6.2), with default and custom limits; `C0` values never change a color
  - imbalance attribution, and the `avg ≥ 3 A` gate at idle
  - an E1 flag override
  - both judges disagreeing in each direction (§6.5): our overload with the PSU Normal, and the PSU's alarm with our levels Normal
- **T6.** *Test alarm*:
  - first the caution strip with its chime for 3 s (TEST), then the notch
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
- **T14. Install** from a downloaded file on a machine without MeltAlarm: one dialog; afterwards the program is in Program Files, the shortcut and installed-apps entry exist, the startup task points to Program Files, the downloaded file can be deleted, and after a reboot the app starts with no UAC prompt. With a simulated failure (read-only target), nothing is left behind and the copy runs portable.
- **T15. Update** while the installed older version is monitoring: one dialog; the gap is logged (STOPPED, INSTALL); settings, log, startup task and pinned icons are kept. Rolling back to the older file works the same way. Refused while an alarm (either judge) is active (*Test alarm* doesn't count). A failed replace leaves the old version running.
- **T16. Uninstall** from the tray and from Installed apps: task, shortcut, entry and program folder are gone; settings and log remain unless the box was checked.
- **T17. Portable:** *Run without installing* and `--portable` monitor normally and create no task, shortcut or entry.
- **T18. Floating states:** every row of the §7.4 table, for both connectors, including pop out by button and by dragging, locate on tray click, and × then tray click opening the flyout.
- **T19. Floating placement:** put a view on a second display, scale it, switch layouts; exit and restart: same display, position, layout and scale. Unplug that display: the view moves fully onto the primary display.
- **T20. Floating focus:** with a borderless game focused, clicking, dragging and scaling the view never takes focus from the game.
- **T21. Floating footprint:** one view floating for 1 h: private memory ≤ 20 MB and not growing (last 30 min within ±1 MB).
- **T22. Overload rule** (§6.3, unit tests on synthetic samples):
  - a single 11 A reading: no alarm
  - two 12.2 A readings on the same wire: alarm on the second
  - one 16.5 A reading: alarm at once
  - constant 10.7 A: alarm on the first reading at least 4 s after the first
  - 10.4 / 10.6 alternating: one episode, alarm after 4 s
  - 12.5 A alternating between two wires (each dropping to 8 A in between): no alarm
  - a gap > 3 s restarts qualification but never clears an active overload
  - clears only when a valid sample shows every wire below the rating
- **T23. Caution strip:** a wire held at 9.9 A in the simulator → after 10 s the strip appears on every monitor with one chime, stays 10 s, doesn't come back in the same episode; clicks pass through; a borderless game keeps focus; the Alerts switch off suppresses it; an overload during it replaces it with the notch.
- **T24. Monitoring lost:** unplug the PSU USB during load → one strip and chime (*Monitoring lost*), not repeated until data came back and was lost again.
- **T25. Uneven load:** the simulator's uneven scenario → after 10 s one silent notification (in a game: held until the game ends), a cable note, the tray marker; clicking the notification opens the flyout.
- **T26. Cable notes:** after an alarm, a caution and an advisory, the note shows the most severe; it survives a restart; *Dismiss* removes it and the tray marker.
- **T27. Session ended during an alarm:** end the process during a simulated alarm (as a power cut would); the next start shows the after-alarm notification once, and the note is there. A normal exit without an alarm shows nothing.
- **T28. No false alarms:** 2 h of real gaming and a stress test on the reference PC: no alarm, no caution, no advisory. The highest single-wire reading is recorded for the release notes.

---

## 12. Open items
- To be learned from the first real event (logged automatically): the RunTime field, E1 behavior for status 2, and the firmware timings.
- Ai1600TS hardware confirmation (T10).
- Whether an `E0` reading is instantaneous, averaged or a held peak (§1). If it is a held peak, the one-reading 15 A rule and the persistence rules need revalidation. A one-off read-only probe (10 readings/s of `E0` for a few minutes under load, only with the user's go-ahead) would settle it.
- The cable-limit defaults (v1) are engineering defaults; T28 and user reports may tune them in a later limits version.
- What `FA 51` changes inside the PSU firmware (F19). The working assumption is "host connected, start answering", backed by MSI's own use. It is sent only on connect, never per tick.

---

## 13. Document history

| Version | Date | Changes |
|---|---|---|
| 2.6 | 2026-10-01 | Clean-up: log examples match the real log lines, the PSU's protection is named by the source (§6.9), the full hidapi deny-list (§3), notch height, the update refusal text |
| 2.5 | 2026-10-01 | Names: "GPU power cable", numbered only when it helps; "imbalance" for the spread; no wire numbers or total current on screen (§7.0) |
| 2.4 | 2026-10-01 | MeltAlarm's own cable limits next to the PSU's verdict ("one alarm, two judges"); the alert ladder; cable notes (§6, §7, §8, F20–F22) |
| 2.3 | 2026-09-30 | Floating monitor (§7.4) |
| 2.2 | 2026-09-28 | One self-installing exe: install, update, uninstall (§4) |
| 2.1 | 2026-09-28 | The `FA 51` connect handshake (F19), identification by USB ID, a startup that never gives up on a present PSU |
| 2.0 | 2026-09-27 | First published version, after the first measurements |
