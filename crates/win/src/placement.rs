//! Where each connector's floating view sits (docs/ARCHITECTURE.md §7.2, D14): a small
//! `window.toml` next to the settings, and the displays it refers to.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use meltalarm_model::ConnectorKey;
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MONITORINFOEXW, MonitorFromPoint, MonitorFromRect,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::{BOOL, PCWSTR};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Compact,
    Full,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub floating: bool,
    pub layout: Layout,
    /// User scale per layout: `[compact, full]`.
    pub scale: [f32; 2],
    /// Display identity (device interface name), and the window's top-left corner in DIPs
    /// relative to that display's work area.
    pub display: String,
    pub x: f32,
    pub y: f32,
}

impl Placement {
    pub fn scale(&self) -> f32 {
        self.scale[self.layout as usize]
    }
    pub fn set_scale(&mut self, s: f32) {
        self.scale[self.layout as usize] = s;
    }
}

pub struct Placements {
    path: PathBuf,
    pub map: BTreeMap<ConnectorKey, Placement>,
}

impl Placements {
    pub fn load(path: PathBuf) -> Placements {
        let map = fs::read_to_string(&path).map(|t| parse(&t)).unwrap_or_default();
        Placements { path, map }
    }

    /// Atomic, like the settings file. Failures are ignored: placement is a convenience.
    pub fn save(&self) {
        meltalarm_runtime::write_atomic(&self.path, &format(&self.map));
    }
}

fn format(map: &BTreeMap<ConnectorKey, Placement>) -> String {
    let mut s = String::from("# MeltAlarm floating monitor placement. Delete this file to reset.\n");
    for (key, p) in map {
        s.push_str(&format!(
            "\n[{key}]\nfloating = {}\nlayout = {}\nscale_compact = {:.3}\nscale_full = {:.3}\nx = {:.1}\ny = {:.1}\ndisplay = {}\n",
            p.floating,
            if p.layout == Layout::Compact { "compact" } else { "full" },
            p.scale[0],
            p.scale[1],
            p.x,
            p.y,
            p.display
        ));
    }
    s
}

/// Lenient: unknown keys, broken lines, incomplete sections and unknown connectors are ignored.
fn parse(text: &str) -> BTreeMap<ConnectorKey, Placement> {
    let mut map = BTreeMap::new();
    let mut current: Option<(Option<ConnectorKey>, Placement)> = None;
    let num = |v: &str| v.parse::<f32>().ok().filter(|f| f.is_finite());
    for line in text.lines().map(str::trim) {
        if let Some(key) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if let Some((Some(k), p)) = current.take() {
                map.insert(k, p);
            }
            let p = Placement { floating: false, layout: Layout::Full, scale: [1.0, 1.0], display: String::new(), x: 0.0, y: 0.0 };
            current = Some((ConnectorKey::parse(key), p));
            continue;
        }
        let (Some((_, p)), Some((k, v))) = (current.as_mut(), line.split_once('=')) else { continue };
        let v = v.trim();
        match k.trim() {
            "floating" => p.floating = v == "true",
            "layout" => p.layout = if v == "compact" { Layout::Compact } else { Layout::Full },
            "scale_compact" => p.scale[0] = num(v).unwrap_or(1.0),
            "scale_full" => p.scale[1] = num(v).unwrap_or(1.0),
            "x" => p.x = num(v).unwrap_or(0.0),
            "y" => p.y = num(v).unwrap_or(0.0),
            "display" => p.display = v.to_owned(),
            _ => {}
        }
    }
    if let Some((Some(k), p)) = current {
        map.insert(k, p);
    }
    map
}

// ---------------------------------------------------------------------------------------------
// Displays

#[derive(Clone, Debug)]
pub struct Display {
    pub id: String,
    pub work: RECT,
    /// DPI scale (1.0 = 96 DPI).
    pub dpi: f32,
}

fn describe(mon: HMONITOR) -> Display {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize says which one it is.
    unsafe {
        let _ = GetMonitorInfoW(mon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO);
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY: valid monitor and out pointers.
    unsafe {
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    }
    // The monitor's device interface name is stable across reboots; "\\.\DISPLAYn" is not.
    let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    // SAFETY: szDevice is NUL-terminated; dd.cb is set.
    let found = unsafe { EnumDisplayDevicesW(PCWSTR(mi.szDevice.as_ptr()), 0, &mut dd, 1).as_bool() }; // EDD_GET_DEVICE_INTERFACE_NAME
    let wstr = |w: &[u16]| String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())]);
    let id = if found && dd.DeviceID[0] != 0 { wstr(&dd.DeviceID) } else { wstr(&mi.szDevice) };
    Display { id, work: mi.monitorInfo.rcWork, dpi: dx as f32 / 96.0 }
}

pub fn displays() -> Vec<Display> {
    unsafe extern "system" fn collect(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec passed below, alive for the whole enumeration.
        let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        list.push(mon);
        true.into()
    }
    let mut mons: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into `mons`.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut mons as *mut _ as isize));
    }
    mons.into_iter().map(describe).collect()
}

pub fn primary() -> Display {
    // SAFETY: plain lookup.
    describe(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) })
}

/// The display a screen rectangle is mostly on.
pub fn display_of(r: &RECT) -> Display {
    // SAFETY: plain lookup.
    describe(unsafe { MonitorFromRect(r, MONITOR_DEFAULTTONEAREST) })
}

/// The saved display, or the primary one if it is gone.
pub fn find(id: &str) -> Display {
    displays().into_iter().find(|d| d.id == id).unwrap_or_else(primary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_file_round_trips_and_tolerates_junk() {
        let key = |s: &str| ConnectorKey::parse(s).unwrap();
        let mut map = BTreeMap::new();
        map.insert(
            key("msi:ai1300ts:1"),
            Placement {
                floating: true,
                layout: Layout::Compact,
                scale: [3.0, 1.25],
                display: r"\\?\DISPLAY#GSM5B09#5&1a2b&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}".into(),
                x: 12.5,
                y: -3.0,
            },
        );
        let text = format(&map);
        assert_eq!(parse(&text), map);
        let junk = format!("garbage\nx = 5\n{text}\n[msi:ai1300ts:2]\nscale_full = NaN\nwhat\n[nonsense]\nx = 1\n");
        let parsed = parse(&junk);
        assert_eq!(parsed[&key("msi:ai1300ts:1")], map[&key("msi:ai1300ts:1")]);
        assert_eq!(parsed[&key("msi:ai1300ts:2")].scale, [1.0, 1.0]);
        assert!(!parsed[&key("msi:ai1300ts:2")].floating);
        assert_eq!(parsed.len(), 2, "an unknown section is ignored");
    }
}
