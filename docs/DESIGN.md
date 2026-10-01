# MeltAlarm — UI Design

**Status:** approved 2026-09-27; alert ladder, cut bars and names 2026-10-01 · Companion of `FUNCTIONAL_SPEC.md` (what) and `ARCHITECTURE.md` (how) · Visual reference: the concept board artifact (private) — https://claude.ai/artifact/LPiwjezynYcWmUvdteydLr

Every surface the user can see has an entry here and is checked on screen at real size before it ships.

Principle: **quiet when fine, unmistakable when not.**

## Decisions

| Topic | Decision |
|---|---|
| Normal state | **Dark cockpit.** Normal wires use the neutral system tone (white on a dark taskbar, near-black on a light one). Color appears only for caution, warning, alarm and no data. |
| Two modes | *Ambient* surfaces (tray, popup, settings) look native to Windows 11. The *alert* surface (the alarm) is solid, flat, high contrast and deliberately non-native. |
| Color = state only | Amber = caution, red = warning or alarm, hollow grey = no data. Never decorative. Every state also differs in shape or lightness (filled / hollow / dim / solid tile). |
| Popup visualization | **Cut bars** (2026-10-01): one bar per wire, its top is the alarm limit, cut straight across at the rating; a knee scale gives the decision range room. No limit lines, no labels, no wire numbers; the value sits under each bar. Mockups: https://claude.ai/artifact/J2r5BVZFm4CpT16r4W9GyT |
| Names | "GPU power cable", numbered only when two are tracked or for an untracked one (Spec §7.0); "Imbalance", never "spread"; no total current. |
| Fonts | Segoe UI Variable for UI text; Bahnschrift for numbers and the alarm (tabular figures). Both ship with Windows. |
| Motion | Only a change of state moves, briefly; nothing moves at rest, data never animates. See "Motion" below. |
| Alert ladder | Spec §8. **Alarm** = the red notch + alarm sound + voice. **Caution** = the amber strip + one chime. **Advisory** = a silent notification. **Status** = the attention marker. Each channel means exactly one level. |
| Vocabulary | One set of state words on every surface: `OK`, `Caution`, `ALARM`, `No data`, `Not connected`. The PSU's own verdict appears only as information ("PSU status", "reported by the PSU"). |

## State colors

| State | Dark theme | Light theme |
|---|---|---|
| Normal (neutral) | `#F2F2F2` (tray) / `#E6E6E6` (bars) | `#1B1B1B` (tray) / `#3A3A3A` (bars) |
| Caution | `#F5A623` | `#B86E00` (graphics), `#8F5600` (text) |
| Warning | `#FF4D4F` (graphics), `#FF6B6D` (text) | `#D1242F` (graphics), `#B81F29` (text) |
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

- **The face of the 16-pin plug**: the housing outline, the latch on top and the six pins, `#F2F2F2` on a rounded tile `#1F1F1F` (corner radius 20 %). From 24 px up, a 1 px inner edge of white at 12 % keeps the tile visible on dark backgrounds.
- **Below 24 px** only the six pins remain, like the tray glyph (squares 3×3, gap 1 at 16 px).
- Neutral on purpose: color stays reserved for state ("color = state only").
- Drawn separately at each size (16, 20, 24, 32, 40, 48, 64, 96, 128 px), with edges snapped to whole pixels, never scaled from one master.
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
   | `Not connected` | as `No data`: nothing to vouch for. The bars stay, reading 0.0 |

2. During an alarm: a red strip with the reason (`Wire overload · 12.4 A`, or `PSU: Current imbalance`) and, from the PSU, the countdown (`Power cut in ~2:47`).
3. **Cut bars** (96 px tall, 14 px wide) with the value under each; no wire numbers. See "Cut bars" below.
4. Live lines (amber or info tint): above the rating, over the alarm limit, uneven load, PSU faults, Safeguard+ OFF, monitoring interrupted.
5. The **cable note**, once the connector is back to OK: the caution tint, the note text, and a **Dismiss** link (accent color, right-aligned under the text, 13 px). Hit area at least 28 px tall.
6. Footer stats, two columns: **Imbalance** (colored by its level) and **PSU status** (the PSU's own verdict in words; `—` without one).

## Cut bars

The one bar language for every surface that shows wires: the flyout, the floating view (both layouts) and the alarm banner's small bars. *Why:* on a 0–10.5 A scale the rating and the alarm limit are 9 px apart, so any lines or labels for them merge (seen in 0.3.0).

- **The top of the bar is the alarm limit.** Nothing marks it: a completely full bar means alarm.
- **Cut at the rating.** The bar is sliced straight across with a **2 px** gap: the short head keeps the round top and has a flat bottom; the body has a flat top and the round bottom. 2 px is the weight of MeltAlarm's meaningful edges (alarm banner and caution strip frames, the locate ring) and stays crisp at 125 % scaling.
- **Knee scale.** 0–6 A fill the bottom 20 % of the bar, 6 A to the alarm limit the top 80 %. The head is then 17 px in the flyout (10 px Compact, 8.5 px on the banner) instead of 9, a 575 W load sits at 57 %, a 360 W card at full load at 17 %. With custom limits the knee is at 6 A or 60 % of the alarm limit, whichever is lower.
- **Fill:** round top, like the Windows 11 surfaces around it; flat where it meets the cut or the end of the bar. Its color is the wire's state: neutral, amber from the rating (the fill jumps the cut), red at the alarm limit (the bar is full).
- **Track:** the neutral track tone for both parts; no second shade, no hatching, no text. Color only ever means state.

## Floating monitor

Mockup (approved 2026-09-30): https://claude.ai/artifact/DAk9umrqm4UfJTGeX9LTMU

A connector's view taken out of the tray to watch it for a long time (Spec §7.4). Two layouts, the same look as the popup.

| | Compact | Full |
|---|---|---|
| Size at 100 % | 250 × 152, **fixed** in every state (the longest header, `GPU power cable` + `Not connected`, fits beside the hover ×) | the popup card (360 wide), grows downward for notes like the popup |
| Content | name (the short form `Cable 2` when numbered, Spec §7.0) + chip (`OK` / `Caution` / `ALARM` / `No data` / `Not connected`), six cut bars (56 px, 10 px wide) with values, one summary line | exactly the popup |
| Summary line | the most important thing, in the Spec §7.4 order: alarm (`Overload · 12.4 A · stop`, `Imbalance · cut ~2:13`) in warning red; caution (`9.9 A · over rating`) and uneven load (`Imbalance 4.1 A`) in caution text; no data (`Last reading 12 s ago`); a cable note (`Check the cable`) in caution text; not connected (`No current on any wire`) in tertiary text; else `Imbalance 0.4 A` | — |

- **Calm when fine:** at rest only data is visible. On hover: × (top right), a 36 × 18 tab with a chevron hanging off the bottom edge (⌄ = Full, ⌃ = Compact), a faint resize grip in the bottom-right corner, and the border brightens to `#555555` (light theme `#B0B0B0`).
- **Scaling is uniform:** the layout is drawn at the chosen scale and never distorts. Each layout remembers its own scale.
- **Locate pulse:** a 2 px accent ring (`#4CC2FF`) with a soft 6 px halo, for 0.6 s.
- Card, shadow, colors, fonts: as the popup, following the system light or dark theme.

## Settings (Windows 11 window, cards)

Sections, in order:
1. **Monitored cables:** "GPU power cable 1" and "GPU power cable 2", checkboxes, each with a live dot and a text label ("In use" with a green dot / "Not connected" with a **grey** dot; an unused cable is never red).
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
  - small cut bars (10 × 48 px)
  - what happened and who says so, then the numbers: at most two lines, **one fact per line**, so a sentence never wraps into the next (no orphan words) at any notch width
  - a right-hand block: the countdown, "ANY SECOND" for status 3, or the alarm duration once cleared. At most 140 DIP wide: a word value shrinks its font (36 → 20 pt) so the text column keeps room on the narrowest notch
- Full-width **SNOOZE 30 s** button with the `Ctrl+Alt+G` hint.
- Variants:

  | Variant | Band | Differences |
  |---|---|---|
  | MeltAlarm overload | red | right block `HIGHEST WIRE` / `12.4 A` instead of the countdown |
  | PSU status | red | right block: the PSU's countdown |
  | critical (status 3) | red | "POWER CUT: ANY SECOND" instead of the countdown; the band names the status, so the what-line only says "Reported by the PSU" |
  | data lost | red | details dimmed, plus the note "cannot confirm" |
  | cleared | green | "Load back to normal", the advice to inspect the cable, alarm duration, 5 s progress bar, no button |
  | test | red | TEST chip in the band |

## Caution strip

The notch's little sibling (Spec §8.8), same place: top edge, centered, every monitor.

- **Shape:** the notch shape at a smaller size: flat top fused to the screen edge, 12 px bottom corners, **40 px tall**. Width fits the text: 24 px padding each side, from 360 to 720 px.
- **Look:** body `#0F0F10` at 97 %, a 2 px `#F5A623` frame, soft shadow like the notch. No band: amber is only the frame and the symbol, so it never reads as the red alarm.
- **Content, one line, left to right:**
  - an amber caution symbol (the notch's triangle, amber)
  - the what-and-where in white, Bahnschrift 15 semibold: `A wire at 9.9 A, above the 9.5 A rating` (with two cables: `Cable 2 · A wire at …`)
  - ` · ` and the action in `#F5A623`, Segoe UI 14: `Ease the GPU load`
  - for a test, a `TEST` chip on the right (amber fill, dark text)
- **Behavior:** click-through (it never catches the mouse), never activates, 10 s, then gone. The same motion as the notch.

## Caution chime

One soft bell-like chime, **synthesized by MeltAlarm** so it doesn't depend on the user's Windows sound scheme (Spec F13):
- a single C6 (1047 Hz) tone with a quiet bell partial (x2.76), a 5 ms attack and an exponential decay (tau about 160 ms), 0.7 s long, peak about -12 dBFS
- clearly not the alarm: one gentle note, against the alarm's harsh triple "Critical Stop" and the voice
- played once per strip, at the current system volume

## Motion

Approved 2026-10-01 with a mockup: https://claude.ai/artifact/RBqQGfCdhzy1WRXE5hVuP6

Where it comes from:
- **Dark cockpit:** nothing moves while all is well; motion only marks a change of state.
- **Master Warning / Master Caution:** warnings flash, cautions stay steady. Only the small tray icon blinks; nothing large ever flashes (photosensitivity, and it would fight the game).
- **The instrument failure flag:** alerts drop out of the screen's top edge like the OFF flag falling into an attitude indicator.
- **ECAM:** a new state is shown at once; red turns green in one frame.
- **Apollo and Dragon panels:** data never animates. A bar jumps to each reading, like a digital readout; gliding would show values the PSU never measured.

| Surface | Appears | Leaves |
|---|---|---|
| Alarm notch | drops from the top edge, 200 ms, decelerating | retracts upward, 150 ms, accelerating |
| Caution strip | drops, 200 ms | retracts, 150 ms; **instantly** when the alarm takes over |
| Red to green (cleared) | instant | — |
| Tray window (flyout) | rises 8 DIP from its icon and fades in, 150 ms, decelerating | fades out, 100 ms; instantly when it turns into the floating view |
| Bars, numbers, chips | never | — |
| Tray icon in an alarm | blinks at 1 Hz | — |
| Floating view | the locate pulse, 0.6 s | — |

- **Entries decelerate, exits accelerate, nothing overshoots.** A bounce on an alarm reads as playful.
- **Motion never delays the alarm.** Sound and voice start with the first frame.
- **The drop is a reveal.** The surface is drawn once and slides out from behind the screen's edge, so it never spills onto a monitor stacked above.
- **Windows' "Animation effects" setting** (Accessibility → Visual effects) turns all of it off: every change is instant.
- A transition is about a dozen frames; nothing is drawn at rest.

## Rendering stack (constraint for the architecture)

- Win32 windows with **Direct2D + DirectWrite in software mode** (software render target), so no GPU driver is loaded into the process.
- The flyout, the floating view, the notch and the strip are **layered windows drawn entirely by MeltAlarm**: card, rounded corners, shadow and border included, with solid surface colors (no Mica or acrylic), so Windows 10 needs no separate look (to be confirmed in the v1 Windows 10 pass).
- A small set of self-drawn controls: button, toggle, checkbox, bars, chip, text. The Settings window (v1) is a normal window; DWM supplies its title bar and corners.

## History

| Date | Changes |
|---|---|
| 2026-10-01 | Motion: the drop, the rise, instant state changes, Windows' animation setting respected |
| 2026-10-01 | Notch numbers one fact per line (no orphan words) |
| 2026-10-01 | `Not connected` chip; Compact 250 wide with the short name `Cable 2` |
| 2026-10-01 | Clean-up: the app icon that ships, motion marked as not built, rendering stack as built |
| 2026-10-01 | Cut bars, names ("GPU power cable"), imbalance; no wire numbers or total |
| 2026-10-01 | The alert ladder: caution strip, chime, cable notes, notifications |
| 2026-09-30 | Floating monitor |
| 2026-09-28 | App icon and notifications, after the first install showed undesigned ones |
| 2026-09-27 | First version: dark cockpit, tray glyph, popup, alarm notch |
