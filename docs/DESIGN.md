# MeltAlarm — UI Design

**Status:** approved 2026-09-27 · Visual reference: the concept board artifact (private) — https://claude.ai/artifact/LPiwjezynYcWmUvdteydLr

Principle: **quiet when fine, unmistakable when not.**

## Decisions

| Topic | Decision |
|---|---|
| Normal state | **Dark cockpit.** Normal wires use the neutral system tone (white on a dark taskbar, near-black on a light one). Color appears only for caution, warning, alarm and no data. |
| Two modes | *Ambient* surfaces (tray, popup, settings) look native to Windows 11. The *alert* surface (the alarm) is solid, flat, high contrast and deliberately non-native. |
| Color = state only | Amber = caution, red = warning or alarm, hollow grey = no data. Never decorative. Every state also differs in shape or lightness (filled / hollow / dim / solid tile). |
| Popup visualization | **Vertical bars**, one per wire, filled toward the PSU limit (`OCP` from `C0`), with a dashed caution line at 80 %. The value sits under each bar. |
| Fonts | Segoe UI Variable for UI text; Bahnschrift for numbers and the alarm (tabular figures). Both ship with Windows. |
| Motion | Popup: 150 ms rise and fade. Alarm: 200 ms drop from the top edge. Tray PSU alarm blinks at 1 Hz. Nothing else animates. |

## State colors

| State | Dark theme | Light theme |
|---|---|---|
| Normal (neutral) | `#F2F2F2` (tray) / `#E6E6E6` (bars) | `#1B1B1B` (tray) / `#3A3A3A` (bars) |
| Caution | `#F5A623` | `#B86E00` (graphics), `#8F5600` (text) |
| Warning | `#FF4D4F` | `#D1242F` (graphics), `#B81F29` (text) |
| PSU alarm band / tile | `#C8102E` + white | same |
| Cleared band | `#1E7F45` + white | same |
| Accent (toggles, links) | `#4CC2FF` | `#005FB8` |

## Tray icon (16 px grid at 100 % scaling)

- Squares 4×4 with a 1 px gap. Glyph 14×9 at origin (1,2). Row 1 = wires 1–3, row 2 = wires 4–6.
- 24 px grid (150 %): squares 6×6, 2 px gap, origin (1,3).
- **States:**

  | State | Look |
  |---|---|
  | normal | neutral squares |
  | caution / warning | colored squares for the affected wires only |
  | PSU alarm | red rounded tile, white squares, the flagged wire knocked out (`#5A0010`); alternates at 1 Hz with a frame showing the live colors |
  | no data | hollow outlines |
  | not connected | squares at 28 % opacity |
  | PSU attention (Safeguard+ off or fault flag) | amber bar under the glyph |

## App icon

The exe's own icon, seen in the Start menu, Installed apps, the UAC prompt, Explorer and notification headers. *Added 2026-09-28, after the first install showed Windows' generic icon.*

- The tray glyph's six squares, `#F2F2F2`, centered on a rounded tile `#1F1F1F` (corner radius 20 %). From 24 px up, a 1 px inner edge of white at 12 % keeps the tile visible on dark backgrounds.
- Neutral on purpose: color stays reserved for state ("color = state only").
- Drawn separately at each size (16, 20, 24, 32, 40, 48, 64, 96, 128 px), with squares snapped to whole pixels, never scaled from one master. At 16 px: squares 3×3, gap 1.
- The app name everywhere is just **MeltAlarm** (the exe's FileDescription), never a tagline.

## Notifications

Windows notifications (toasts), sent through the tray icon. Rare: install welcome, second launch while connecting, PSU not reachable after 2 min. **Alarms are never notifications**: the notch owns them.

- Header: supplied by Windows: the live tray glyph and "MeltAlarm" (the exe's FileDescription).
- **No icon in the body.** The title carries the meaning; a warning triangle for good news is wrong, and a stretched glyph is worse.
- Title: what happened, sentence case, ≤ 48 characters. Text: at most two short sentences; no long menu paths. If there is something to do, clicking the notification does it.
- **Info** (welcome, already running): silent. **Warning** (PSU not reachable): the default notification sound.
- Welcome after install: *"MeltAlarm is running"* / *"Click here to keep its icon visible on the taskbar."* The click opens Taskbar settings.

## Popup (360 px wide, Windows 11 flyout)

From top to bottom:
1. Header: connector name, plus a chip ("PSU · Normal", solid red "PSU ALARM", or "No data").
2. During a PSU alarm: a red strip with the reason and the countdown.
3. Bars (96 px track) with values and wire numbers.
4. A note strip for the §6.3 notice or for no data.
5. Footer stats: total (sum of wires), spread, PSU limits.

## Settings (Windows 11 window, cards)

Sections, in order:
1. **Monitored connectors:** checkboxes, each with a live dot and a text label ("In use" / "No load").
2. **Alarm:** a toggle, plus the *Test alarm* button.
3. **Startup:** a toggle.
4. **Power supply:** read-only model, firmware, Safeguard+ state and limits.

Footer: *Open log folder* and the version.

## Alarm overlay ("notch")

- Anchored to the top edge, centered, on every monitor. Width is 32 % of the monitor, clamped to 560–880 DIP; about 280 px tall.
- 44 px band: headline plus the connector.
- Action: **STOP GPU LOAD NOW** plus one explanatory line.
- Detail row:
  - mini bars
  - what the PSU reported, with numbers
  - a right-hand block: the countdown, "ANY SECOND" for status 3, or the alarm duration once cleared
- Full-width **SNOOZE 30 s** button with the `Ctrl+Alt+G` hint.
- Variants:

  | Variant | Band | Differences |
  |---|---|---|
  | critical (status 3) | red | "POWER CUT: ANY SECOND" instead of the countdown |
  | data lost | red | details dimmed, plus the note "cannot confirm" |
  | cleared | green | alarm duration, 5 s progress bar, no button |
  | test | red | TEST chip in the band |

## Rendering stack (constraint for the architecture)

- Win32 windows with **Direct2D + DirectWrite in software mode** (WARP/software render target), so no GPU driver is loaded into the process.
- A small set of self-drawn controls: button, toggle, checkbox, bars, chip, text.
- DWM supplies the Windows 11 rounded corners, backdrop and dark title bar. Where that isn't available (Windows 10, or trouble in the prototype), the fallback is solid surface colors.
