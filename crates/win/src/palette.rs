//! Colors (DESIGN.md "State colors"). Color only ever means state; every surface takes its
//! colors from here.

use meltalarm_core::Level;

pub const CAUTION_DARK: u32 = 0xF5A623;
pub const CAUTION_LIGHT: u32 = 0xB86E00;
pub const WARNING_DARK: u32 = 0xFF4D4F;
pub const WARNING_LIGHT: u32 = 0xD1242F;
/// The alarm band and tile, the `ALARM` chip: the same in both themes.
pub const ALARM_RED: u32 = 0xC8102E;
/// The cleared notch's band.
pub const CLEARED_GREEN: u32 = 0x1E7F45;
/// The body of the alert surfaces (notch, strip): fixed high contrast, the same in both themes.
pub const ALERT_BODY: u32 = 0x0F0F10;
pub const ACCENT_DARK: u32 = 0x4CC2FF;
pub const ACCENT_LIGHT: u32 = 0x005FB8;

/// The ambient surfaces (flyout, floating view) follow the system theme.
pub(crate) struct Theme {
    pub bg: u32,
    pub border: u32,
    pub fg: u32,
    pub fg3: u32,
    pub track: (u32, f32),
    pub ok: u32,
    pub caution: u32,
    pub warn: u32,
    pub caution_text: u32,
    pub warn_text: u32,
    pub note_caution: (u32, f32, u32),
    pub note_info: (u32, f32, u32),
    /// Links and the locate ring (DESIGN.md "State colors": accent).
    pub accent: u32,
    /// The floating view on hover: its border and the layout tab (DESIGN.md "Floating monitor").
    pub hover_border: u32,
    pub tab_bg: u32,
}

pub(crate) fn theme(light: bool) -> Theme {
    if light {
        Theme {
            bg: 0xF9F9F9,
            border: 0xE0E0E0,
            fg: 0x1A1A1A,
            fg3: 0x5E5E5E,
            track: (0x000000, 0.08),
            ok: 0x3A3A3A,
            caution: CAUTION_LIGHT,
            warn: WARNING_LIGHT,
            caution_text: 0x8F5600,
            warn_text: 0xB81F29,
            note_caution: (CAUTION_LIGHT, 0.12, 0x6E4200),
            note_info: (0x000000, 0.05, 0x3A3A3A),
            accent: ACCENT_LIGHT,
            hover_border: 0xB0B0B0,
            tab_bg: 0xFFFFFF,
        }
    } else {
        Theme {
            bg: 0x2B2B2B,
            border: 0x3A3A3A,
            fg: 0xF2F2F2,
            fg3: 0xA8A8A8,
            track: (0xFFFFFF, 0.09),
            ok: 0xE6E6E6,
            caution: CAUTION_DARK,
            warn: WARNING_DARK,
            caution_text: CAUTION_DARK,
            warn_text: 0xFF6B6D,
            note_caution: (CAUTION_DARK, 0.14, 0xF7C878),
            note_info: (0xFFFFFF, 0.07, 0xD0D0D0),
            accent: ACCENT_DARK,
            hover_border: 0x555555,
            tab_bg: 0x333333,
        }
    }
}

/// (graphics, text) colors of a level.
pub(crate) fn level_colors(t: &Theme, l: Level) -> (u32, u32) {
    match l {
        Level::Normal => (t.ok, t.fg),
        Level::Caution => (t.caution, t.caution_text),
        Level::Warning => (t.warn, t.warn_text),
    }
}
