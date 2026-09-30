# MeltAlarm — UI Design

**Status:** approved 2026-09-27; alert ladder added 2026-10-01 · Visual reference: the concept board artifact (private) — https://claude.ai/artifact/LPiwjezynYcWmUvdteydLr

Principle: **quiet when fine, unmistakable when not.**

## Decisions

| Topic | Decision |
|---|---|
| Normal state | **Dark cockpit.** Normal wires use the neutral system tone (white on a dark taskbar, near-black on a light one). Color appears only for caution, warning, alarm and no data. |
| Two modes | *Ambient* surfaces (tray, popup, settings) look native to Windows 11. The *alert* surface (the alarm) is solid, flat, high contrast and deliberately non-native. |
| Color = state only | Amber = caution, red = warning or alarm, hollow grey = no data. Never decorative. Every state also differs in shape or lightness (filled / hollow / dim / solid tile). |
| Popup visualization | **Vertical bars**, one per wire, filled toward the PSU limit (`OCP` from `C0`), with a dashed caution line at 80 %. The value sits under each bar. |
| Fonts | Segoe UI Variable for UI text; Bahnschrift for numbers and the alarm (tabular figures). Both ship with Windows. |
| Motion | Popup: 150 ms rise and fade. Alarm notch and caution strip: 200 ms drop from the top edge. Tray alarm blinks at 1 Hz. Locate pulse 0.6 s. Nothing else animates. |
| Alert ladder | Spec §8. **Alarm** = the red notch + alarm sound + voice. **Caution** = the amber strip + one chime. **Advisory** = a silent notification. **Status** = the attention marker. Each channel means exactly one level. |
| Vocabulary | One set of state words on every surface: `OK`, `Caution`, `ALARM`, `No data`. The PSU's own verdict appears only as information ("PSU status", "reported by the PSU"). |

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
  | alarm (either judge) | red rounded tile, white squares, the flagged or overloaded wire knocked out (`#5A0010`); alternates at 1 Hz with a frame showing the live colors |
  | no data | hollow outlines |
  | not connected | squares at 28 % opacity |
  | attention: something to read in the flyout (a cable note, a PSU fault, Safeguard+ off) | amber bar under the glyph |

## App icon

The exe's own icon, seen in the Start menu, Installed apps, the UAC prompt, Explorer and notification headers. *Added 2026-09-28, after the first install showed Windows' generic icon.*

- The tray glyph's six squares, `#F2F2F2`, centered on a rounded tile `#1F1F1F` (corner radius 20 %). From 24 px up, a 1 px inner edge of white at 12 % keeps the tile visible on dark backgrounds.
- Neutral on purpose: color stays reserved for state ("color = state only").
- Drawn separately at each size (16, 20, 24, 32, 40, 48, 64, 96, 128 px), with squares snapped to whole pixels, never scaled from one master. At 16 px: squares 3×3, gap 1.
- The app name everywhere is just **MeltAlarm** (the exe's FileDescription), never a tagline.

## Notifications

Windows notifications (toasts), sent through the tray icon. Rare and never urgent: install welcome, second launch while connecting, PSU not reachable after 2 min, the uneven-load advisory, and "last session ended during a cable alarm". **Alarms and cautions are never notifications**: Windows holds notifications back during games (Spec F22); the notch and the strip own them.

- Header: supplied by Windows: the live tray glyph and "MeltAlarm" (the exe's FileDescription).
- **No icon in the body.** The title carries the meaning; a warning triangle for good news is wrong, and a stretched glyph is worse.
- Title: what happened, sentence case, ≤ 48 characters. Text: at most two short sentences; no long menu paths. If there is something to do, clicking the notification does it.
- **Info** (welcome, already running, uneven load): silent. **Warning** (PSU not reachable, session ended during an alarm): the default notification sound.
- Welcome after install: *"MeltAlarm is running"* / *"Click here to keep its icon visible on the taskbar."* The click opens Taskbar settings.
- Cable notifications (uneven load, session ended during an alarm): the click opens that connector's flyout.

## Popup (360 px wide, Windows 11 flyout)

From top to bottom:
1. Header: connector name, then **directly after it** the state chip. The right edge holds only the window's one button: *pop out* in the tray flyout, × when floating. The same header is used in every layout.

   | Chip | Look |
   |---|---|
   | `OK` | neutral track fill, green dot |
   | `Caution` | amber tint (`#F5A623` at 18 %), amber dot, caution text color |
   | `ALARM` | solid `#C8102E`, white dot and text |
   | `No data` | neutral track fill, no dot, tertiary text |

2. During an alarm: a red strip with the reason (`Wire 3 overload · 12.4 A`, or `PSU: Current imbalance · wire 3`) and, from the PSU, the countdown.
3. Bars (96 px track) with values and wire numbers. **Full scale = the alarm limit (10.5 A)**: the top of the track is the limit, labelled in red at the top right (no line); a dashed amber line at the **rating (9.5 A)**, its label just under the line so the two labels never touch. Values above the alarm limit fill the bar completely, in red.
4. Live lines (amber or info tint): above the rating, over the alarm limit, uneven load, PSU faults, Safeguard+ OFF, monitoring interrupted.
5. The **cable note**, once the connector is back to OK: the caution tint, the note text, and a **Dismiss** link (accent color, right-aligned under the text, 13 px). Hit area at least 28 px tall.
6. Footer stats: total (sum of wires), spread, **PSU status** (the PSU's own verdict in words; `—` without one).

## Floating monitor

Mockup (approved 2026-09-30): https://claude.ai/artifact/DAk9umrqm4UfJTGeX9LTMU

A connector's view taken out of the tray to watch it for a long time (Spec §7.4). Two layouts, the same look as the popup.

| | Compact | Full |
|---|---|---|
| Size at 100 % | 220 × 152, **fixed** in every state | the popup card (360 wide), grows downward for notes like the popup |
| Content | name + chip (`OK` / `PSU ALARM` / `No data`), six bars (56 px track, 10 px wide) with values, one summary line | exactly the popup |
| Summary line | the most important thing, in the Spec §7.4 order: alarm (`Wire 3 · 12.4 A · stop`, `Imbalance · cut ~2:13`) in warning red; caution (`Wire 3 · 9.9 A · over rating`) and uneven load (`Uneven · Δ 4.1A`) in caution text; no data (`Last reading 12 s ago`); a cable note (`Check the cable`) in caution text; else `Σ 47.5A · Δ 0.4A` | — |

- **Calm when fine:** at rest only data is visible. On hover: × (top right), a 36 × 18 tab with a chevron hanging off the bottom edge (⌄ = Full, ⌃ = Compact), a faint resize grip in the bottom-right corner, and the border brightens to `#555555`.
- **Scaling is uniform:** the layout is drawn at the chosen scale and never distorts. Each layout remembers its own scale.
- **Locate pulse:** a 2 px accent ring (`#4CC2FF`) with a soft 6 px halo, for 0.6 s.
- Card, shadow, colors, fonts: as the popup, following the system light or dark theme.

## Settings (Windows 11 window, cards)

Sections, in order:
1. **Monitored connectors:** checkboxes, each with a live dot and a text label ("In use" with a green dot / "No load" with a **grey** dot; an unused connector is never red).
2. **Alerts:** a toggle ("Alerts: on screen, sound and voice"; off: "Colors, cable notes and the log only"), plus the *Test alarm* button.
3. **Startup:** a toggle (installed copy) or *Install…* (portable).
4. **Limits:** read-only, two short paragraphs, MeltAlarm's first, then the PSU's (Spec §7.3). *custom* in caution text after an overridden value.
5. **Power supply:** read-only model, firmware, serial, Safeguard+ state.

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
  | MeltAlarm overload | red | right block `WIRE 3` / `12.4 A` instead of the countdown |
  | PSU status | red | right block: the PSU's countdown |
  | critical (status 3) | red | "POWER CUT: ANY SECOND" instead of the countdown |
  | data lost | red | details dimmed, plus the note "cannot confirm" |
  | cleared | green | "Load back to normal", the advice to inspect the cable, alarm duration, 5 s progress bar, no button |
  | test | red | TEST chip in the band |

## Caution strip

The notch's little sibling (Spec §8.8), same place: top edge, centered, every monitor.

- **Shape:** the notch shape at a smaller size: flat top fused to the screen edge, 12 px bottom corners, **40 px tall**. Width fits the text: 24 px padding each side, from 360 to 720 px.
- **Look:** body `#0F0F10` at 97 %, a 2 px `#F5A623` frame, soft shadow like the notch. No band: amber is only the frame and the symbol, so it never reads as the red alarm.
- **Content, one line, left to right:**
  - an amber caution symbol (the notch's triangle, amber)
  - the what-and-where in white, Bahnschrift 15 semibold: `#1 · Wire 3 at 9.9 A, above the 9.5 A rating`
  - ` · ` and the action in `#F5A623`, Segoe UI 14: `Ease the GPU load`
  - for a test, a `TEST` chip on the right (amber fill, dark text)
- **Behavior:** click-through (it never catches the mouse), never activates, 10 s, then gone. The same motion as the notch.

## Caution chime

One soft bell-like chime, **synthesized by MeltAlarm** so it doesn't depend on the user's Windows sound scheme (Spec F13):
- a single C6 (1047 Hz) tone with a quiet bell partial (x2.76), a 5 ms attack and an exponential decay (tau about 160 ms), 0.7 s long, peak about -12 dBFS
- clearly not the alarm: one gentle note, against the alarm's harsh triple "Critical Stop" and the voice
- played once per strip, at the current system volume

## Rendering stack (constraint for the architecture)

- Win32 windows with **Direct2D + DirectWrite in software mode** (WARP/software render target), so no GPU driver is loaded into the process.
- A small set of self-drawn controls: button, toggle, checkbox, bars, chip, text.
- DWM supplies the Windows 11 rounded corners, backdrop and dark title bar. Where that isn't available (Windows 10, or trouble in the prototype), the fallback is solid surface colors.
