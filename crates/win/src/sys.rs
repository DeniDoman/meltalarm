//! Small Windows helpers: paths, wall clock, theme, messages, shell.

use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONWARNING, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW, SW_SHOWNORMAL,
};
use windows::core::{HSTRING, PCSTR, PCWSTR, w};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn app_dir(env: &str) -> PathBuf {
    std::env::var_os(env).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("MeltAlarm")
}

pub fn wall_clock() -> String {
    // SAFETY: plain Win32 call with no pointers.
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

fn personalize_dword(name: PCWSTR) -> Option<u32> {
    let mut value = 0u32;
    let mut size = 4u32;
    // SAFETY: valid key/value names and a 4-byte output buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok().then_some(value)
}

/// Taskbar and system flyouts (tray popup) are light.
pub fn system_light() -> bool {
    personalize_dword(w!("SystemUsesLightTheme")) == Some(1)
}

pub fn message(title: &str, text: &str, error: bool) {
    let icon = if error { MB_ICONERROR } else { MB_ICONWARNING };
    // SAFETY: HSTRINGs outlive the call.
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), icon | MB_TOPMOST | MB_SETFOREGROUND);
    }
}

pub fn confirm(owner: HWND, title: &str, text: &str) -> bool {
    // SAFETY: as above.
    unsafe { MessageBoxW(Some(owner), &HSTRING::from(text), &HSTRING::from(title), MB_YESNO | MB_ICONWARNING | MB_TOPMOST) == IDYES }
}

pub fn open_path(path: &std::path::Path) {
    // SAFETY: HSTRING outlives the call.
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL);
    }
}

/// Dark context menus when Windows is in dark mode (uxtheme ordinals 135/136, stable since
/// Windows 10 1903 but undocumented; failure just leaves light menus).
pub fn allow_dark_menus() {
    // SAFETY: we only call the exports if they resolve, with the documented-by-convention
    // signatures `SetPreferredAppMode(int) -> int` and `FlushMenuThemes()`.
    unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
            let set_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(1); // AllowDark: follow the system setting
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
            let flush: extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}
