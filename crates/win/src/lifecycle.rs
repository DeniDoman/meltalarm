//! Windows backend of the program lifecycle (docs/ARCHITECTURE.md §7.1, FUNCTIONAL_SPEC.md §4):
//! the install, update and uninstall steps, their dialogs, and control of a running instance.
//! What to do is decided by `meltalarm_lifecycle`; this module only carries it out.
// Simulated builds never install; they only use the window class and control messages.
#![cfg_attr(feature = "simulate", allow(dead_code))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use meltalarm_core::{LogEvent, Settings};
use meltalarm_lifecycle::{self as lc, Autostart, Facts, Failed, Flag, Instance, Instances, Launch, Policy, Running, Step, Version};
use meltalarm_runtime::{LogSink, Paths, SettingsStore};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW,
    TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TD_ERROR_ICON, TD_INFORMATION_ICON, TD_SHIELD_ICON, TD_WARNING_ICON,
    TDF_ALLOW_DIALOG_CANCELLATION, TDF_POSITION_RELATIVE_TO_WINDOW, TDF_VERIFICATION_FLAG_CHECKED, TaskDialogIndirect,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_ProgramFiles, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, PostMessageW, RegisterWindowMessageW, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW,
};
use windows::core::{BOOL, GUID, HSTRING, Interface, PCWSTR, PWSTR, w};

use crate::autostart::TaskScheduler;

/// Class of the hidden main window, which also receives control messages.
#[cfg(feature = "simulate")]
pub const MAIN_CLASS: PCWSTR = w!("MeltAlarmSimMain");
#[cfg(not(feature = "simulate"))]
pub const MAIN_CLASS: PCWSTR = w!("MeltAlarmMain");

/// `wParam` of the registered control message. Only same-or-higher integrity processes can
/// send it (UIPI), i.e. another elevated MeltAlarm.
pub const SHOW: usize = 1;
pub const ALARM: usize = 2;
pub const QUIT: usize = 3;
/// Replies to `ALARM` (0 = an older version that doesn't know the message).
pub const NO_ALARM: isize = 1;
pub const IN_ALARM: isize = 2;

const STOP_GRACE: Duration = Duration::from_secs(5);
const UNINSTALL_KEY: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\MeltAlarm");
const TITLE: &str = "MeltAlarm";
const ALARM_ACTIVE: &str = "A cable alarm is active. Update MeltAlarm after it has cleared.";

pub fn control_message() -> u32 {
    // SAFETY: static string.
    unsafe { RegisterWindowMessageW(w!("MeltAlarm.Control")) }
}

pub fn version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_default()
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: the returned buffer is freed with CoTaskMemFree after copying.
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

pub fn install_dir() -> PathBuf {
    known_folder(&FOLDERID_ProgramFiles).unwrap_or_else(|| PathBuf::from(r"C:\Program Files")).join("MeltAlarm")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("meltalarm.exe")
}

fn shortcut_path() -> Option<PathBuf> {
    known_folder(&FOLDERID_CommonPrograms).map(|d| d.join("MeltAlarm.lnk"))
}

/// Same file on disk (final paths compared, so case, 8.3 names and links don't matter).
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
        _ => false,
    }
}

/// The file's VERSIONINFO (written by build.rs).
fn file_version(path: &Path) -> Option<Version> {
    let name = HSTRING::from(path.as_os_str());
    // SAFETY: buffers are sized by the API; the fixed-info pointer points into `buf`.
    unsafe {
        let size = GetFileVersionInfoSizeW(&name, None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(&name, None, size, buf.as_mut_ptr().cast()).ok()?;
        let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
        if !VerQueryValueW(buf.as_ptr().cast(), w!("\\"), &mut ptr, &mut len).as_bool() || ptr.is_null() {
            return None;
        }
        let fi = &*(ptr as *const VS_FIXEDFILEINFO);
        Some(Version {
            major: (fi.dwFileVersionMS >> 16) as u16,
            minor: (fi.dwFileVersionMS & 0xFFFF) as u16,
            patch: (fi.dwFileVersionLS >> 16) as u16,
        })
    }
}

fn spawn(exe: &Path, args: &[&str]) -> Result<(), String> {
    // SAFETY: plain call; lets the child's dialog or popup come to the front.
    unsafe {
        let _ = AllowSetForegroundWindow(u32::MAX); // ASFW_ANY
    }
    Command::new(exe).args(args).spawn().map(|_| ()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------
// Instances

struct Win32Running {
    hwnd: HWND,
    pid: u32,
    msg: u32,
}

impl Win32Running {
    fn send(&self, cmd: usize) -> Option<isize> {
        let mut result = 0usize;
        // SAFETY: a window of another process; bounded by the timeout.
        let ok = unsafe { SendMessageTimeoutW(self.hwnd, self.msg, WPARAM(cmd), LPARAM(0), SMTO_ABORTIFHUNG, 2000, Some(&mut result)) };
        (ok.0 != 0).then_some(result as isize)
    }
}

impl Running for Win32Running {
    fn path(&self) -> Option<PathBuf> {
        // SAFETY: handle closed below; buffer length passed in and out.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, self.pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = CloseHandle(h);
            r.ok().map(|_| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
        }
    }

    fn show(&self) {
        // SAFETY: plain call.
        unsafe {
            let _ = AllowSetForegroundWindow(self.pid);
        }
        self.send(SHOW);
    }

    fn alarm_active(&self) -> Option<bool> {
        match self.send(ALARM)? {
            NO_ALARM => Some(false),
            IN_ALARM => Some(true),
            _ => None,
        }
    }

    fn stop(&self, grace: Duration) -> Result<(), String> {
        // SAFETY: handle closed below; QUIT is posted to the instance's own main window.
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, self.pid)
                .map_err(|e| format!("cannot access the running MeltAlarm ({e})"))?;
            let _ = PostMessageW(Some(self.hwnd), self.msg, WPARAM(QUIT), LPARAM(0));
            let mut result = Ok(());
            if WaitForSingleObject(h, grace.as_millis() as u32) != WAIT_OBJECT_0 {
                // An older version without the control message, or a hung one.
                let _ = TerminateProcess(h, 1);
                if WaitForSingleObject(h, 5000) != WAIT_OBJECT_0 {
                    result = Err("the running MeltAlarm did not exit".into());
                }
            }
            let _ = CloseHandle(h);
            result
        }
    }
}

pub struct Win32Instances {
    pub msg: u32,
}

impl Instances for Win32Instances {
    fn running(&self) -> Option<Box<dyn Running>> {
        // SAFETY: plain lookups.
        unsafe {
            let hwnd = FindWindowW(MAIN_CLASS, PCWSTR::null()).ok()?;
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            (pid != 0 && pid != std::process::id()).then(|| Box::new(Win32Running { hwnd, pid, msg: self.msg }) as Box<dyn Running>)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Steps

/// Close the running MeltAlarm; undo starts it again.
struct StopRunning {
    msg: u32,
    stopped: Option<PathBuf>,
}

impl Step for StopRunning {
    fn name(&self) -> String {
        "close the running MeltAlarm".into()
    }
    fn apply(&mut self) -> Result<(), String> {
        // One per session; loop in case a second one was starting at the same moment.
        for _ in 0..3 {
            let Some(r) = (Win32Instances { msg: self.msg }).running() else { return Ok(()) };
            let path = r.path();
            r.stop(STOP_GRACE)?;
            self.stopped = self.stopped.take().or(path);
        }
        Ok(())
    }
    fn undo(&mut self) {
        if let Some(p) = self.stopped.take().filter(|p| p.exists()) {
            let _ = spawn(&p, &[]);
        }
    }
}

/// Put this file at the install path; an existing copy is kept as `.old` until done.
struct PlaceProgram {
    from: PathBuf,
    to: PathBuf,
    backup: Option<PathBuf>,
    created_dir: bool,
}

impl PlaceProgram {
    fn copy(&mut self) -> Result<(), String> {
        let dir = self.to.parent().ok_or("bad install path")?;
        if !dir.exists() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            self.created_dir = true;
        }
        if self.to.exists() {
            let old = self.to.with_extension("exe.old");
            fs::rename(&self.to, &old).map_err(|e| format!("the installed file is in use ({e})"))?;
            self.backup = Some(old);
        }
        // Copy the bytes only: the download's zone mark (Mark of the Web) is not carried over.
        let bytes = fs::read(&self.from).map_err(|e| e.to_string())?;
        let tmp = self.to.with_extension("exe.new");
        fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.to).map_err(|e| e.to_string())
    }
}

impl Step for PlaceProgram {
    fn name(&self) -> String {
        format!("copy the program to {}", self.to.parent().unwrap_or(&self.to).display())
    }
    fn apply(&mut self) -> Result<(), String> {
        let r = self.copy();
        if r.is_err() {
            self.undo();
        }
        r
    }
    fn undo(&mut self) {
        let _ = fs::remove_file(self.to.with_extension("exe.new"));
        let _ = fs::remove_file(&self.to);
        if let Some(old) = self.backup.take() {
            let _ = fs::rename(old, &self.to);
        }
        if self.created_dir
            && let Some(dir) = self.to.parent()
        {
            let _ = fs::remove_dir(dir);
        }
    }
}

struct Shortcut {
    link: PathBuf,
    target: PathBuf,
}

impl Step for Shortcut {
    fn name(&self) -> String {
        "create the Start menu shortcut".into()
    }
    fn apply(&mut self) -> Result<(), String> {
        // SAFETY: COM is initialized on this thread (main); interfaces are released on drop.
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
            link.SetPath(&HSTRING::from(self.target.as_os_str())).map_err(|e| e.to_string())?;
            link.SetDescription(w!("GPU power cable monitor for MSI Ai1300TS/Ai1600TS")).map_err(|e| e.to_string())?;
            if let Some(dir) = self.target.parent() {
                let _ = link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()));
            }
            let file: IPersistFile = link.cast().map_err(|e| e.to_string())?;
            file.Save(&HSTRING::from(self.link.as_os_str()), true).map_err(|e| e.to_string())
        }
    }
    fn undo(&mut self) {
        let _ = fs::remove_file(&self.link);
    }
}

fn reg_sz(key: HKEY, name: &str, value: &str) -> Result<(), String> {
    let data: Vec<u8> = crate::sys::wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
    // SAFETY: open key and a NUL-terminated UTF-16 buffer.
    unsafe { RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(&data)).ok().map_err(|e| e.to_string()) }
}

fn reg_dword(key: HKEY, name: &str, value: u32) -> Result<(), String> {
    // SAFETY: open key and a 4-byte buffer.
    unsafe { RegSetValueExW(key, &HSTRING::from(name), None, REG_DWORD, Some(&value.to_le_bytes())).ok().map_err(|e| e.to_string()) }
}

fn installed_apps_version() -> Option<String> {
    let mut buf = [0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: output buffer and its byte size.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            UNINSTALL_KEY,
            w!("DisplayVersion"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    r.is_ok().then(|| String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
}

fn delete_apps_entry() -> Result<(), String> {
    // SAFETY: fixed key path.
    let r = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, UNINSTALL_KEY) };
    if r.is_ok() || r.0 == 2 { Ok(()) } else { Err(r.to_hresult().message()) } // 2 = not found
}

/// The entry in Settings → Apps → Installed apps.
struct AppsEntry {
    exe: PathBuf,
    version: Version,
    /// `Some(previous DisplayVersion)` if the entry existed before.
    previous: Option<Option<String>>,
}

impl AppsEntry {
    fn write(&self, version: &str) -> Result<(), String> {
        let mut key = HKEY::default();
        // SAFETY: creates or opens our own key; closed below.
        unsafe {
            RegCreateKeyExW(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None)
                .ok()
                .map_err(|e| e.to_string())?;
        }
        let exe = self.exe.display().to_string();
        let dir = self.exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
        let size_kb = fs::metadata(&self.exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
        let r = (|| {
            reg_sz(key, "DisplayName", "MeltAlarm")?;
            reg_sz(key, "DisplayVersion", version)?;
            reg_sz(key, "Publisher", "MeltAlarm project")?;
            reg_sz(key, "DisplayIcon", &exe)?;
            reg_sz(key, "InstallLocation", &dir)?;
            reg_sz(key, "UninstallString", &format!("\"{exe}\" --uninstall"))?;
            reg_dword(key, "EstimatedSize", size_kb)?;
            reg_dword(key, "NoModify", 1)?;
            reg_dword(key, "NoRepair", 1)
        })();
        // SAFETY: opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
        r
    }
}

impl Step for AppsEntry {
    fn name(&self) -> String {
        "register MeltAlarm in Installed apps".into()
    }
    fn apply(&mut self) -> Result<(), String> {
        self.previous = Some(installed_apps_version()).filter(Option::is_some);
        self.write(&self.version.to_string())
    }
    fn undo(&mut self) {
        match self.previous.take() {
            Some(Some(v)) => {
                let _ = self.write(&v);
            }
            _ => {
                let _ = delete_apps_entry();
            }
        }
    }
}

/// The startup task and the stored "Run at Windows startup" setting, set together.
struct Startup {
    exe: PathBuf,
    enable: bool,
    store: SettingsStore,
    previous: Option<(Option<PathBuf>, Settings)>,
}

impl Step for Startup {
    fn name(&self) -> String {
        "set up the start with Windows".into()
    }
    fn apply(&mut self) -> Result<(), String> {
        let settings = self.store.load();
        self.previous = Some((TaskScheduler.target(), settings.clone()));
        TaskScheduler.set(self.enable.then_some(self.exe.as_path()))?;
        self.store.save(&Settings { run_at_startup: self.enable, ..settings });
        Ok(())
    }
    fn undo(&mut self) {
        if let Some((target, settings)) = self.previous.take() {
            let _ = TaskScheduler.set(target.as_deref());
            self.store.save(&settings);
        }
    }
}

/// A step without undo (uninstall).
struct Do<F: FnMut() -> Result<(), String>>(&'static str, F);

impl<F: FnMut() -> Result<(), String>> Step for Do<F> {
    fn name(&self) -> String {
        self.0.into()
    }
    fn apply(&mut self) -> Result<(), String> {
        (self.1)()
    }
}

fn remove_file_if_present(p: &Path) -> Result<(), String> {
    match fs::remove_file(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

fn delay_delete(p: &Path) -> Result<(), String> {
    // SAFETY: NUL-terminated path; a null target means "delete at reboot".
    unsafe { MoveFileExW(&HSTRING::from(p.as_os_str()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT).map_err(|e| e.to_string()) }
}

/// Remove the program folder, even when this process runs from it: a running image can be
/// renamed (not deleted), so it moves out to the Windows temp folder and is deleted at reboot.
fn remove_program(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    if let Ok(me) = std::env::current_exe()
        && me.starts_with(dir)
    {
        let temp = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Temp");
        let parked = temp.join(format!("meltalarm-uninstall-{}.exe", std::process::id()));
        match fs::rename(&me, &parked) {
            Ok(()) => delay_delete(&parked)?,
            Err(_) => delay_delete(&me)?,
        }
    }
    if fs::remove_dir_all(dir).is_err() {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
            let _ = fs::remove_file(entry.path()).or_else(|_| delay_delete(&entry.path()).map_err(std::io::Error::other));
        }
        delay_delete(dir)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Dialogs

const BTN_PRIMARY: i32 = 100;
const BTN_SECONDARY: i32 = 101;

/// A task dialog; returns the pressed button id (IDCANCEL = 2 for Esc/close) and the checkbox.
fn dialog(icon: PCWSTR, heading: &str, text: &str, buttons: &[(i32, &str)], check: Option<(&str, bool)>) -> (i32, bool) {
    let title = HSTRING::from(TITLE);
    let heading = HSTRING::from(heading);
    let text = HSTRING::from(text);
    let labels: Vec<HSTRING> = buttons.iter().map(|(_, l)| HSTRING::from(*l)).collect();
    let btns: Vec<TASKDIALOG_BUTTON> =
        buttons.iter().zip(&labels).map(|((id, _), l)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: PCWSTR(l.as_ptr()) }).collect();
    let check_text = check.map(|(t, _)| HSTRING::from(t));
    let mut flags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW;
    if check.is_some_and(|(_, on)| on) {
        flags |= TDF_VERIFICATION_FLAG_CHECKED;
    }
    let mut cfg = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: flags,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(heading.as_ptr()),
        pszContent: PCWSTR(text.as_ptr()),
        cButtons: btns.len() as u32,
        pButtons: btns.as_ptr(),
        nDefaultButton: buttons.first().map_or(0, |b| b.0),
        ..Default::default()
    };
    cfg.Anonymous1.pszMainIcon = icon;
    if let Some(t) = &check_text {
        cfg.pszVerificationText = PCWSTR(t.as_ptr());
    }
    let (mut button, mut checked) = (2, BOOL(0));
    // SAFETY: all strings and the button array outlive the modal call.
    unsafe {
        if TaskDialogIndirect(&cfg, Some(&mut button), None, Some(&mut checked)).is_err() {
            return (2, false);
        }
    }
    (button, checked.as_bool())
}

fn info(text: &str) {
    dialog(TD_INFORMATION_ICON, TITLE, text, &[(BTN_PRIMARY, "OK")], None);
}

fn error(heading: &str, text: &str) {
    dialog(TD_ERROR_ICON, heading, text, &[(BTN_PRIMARY, "OK")], None);
}

fn failure_text(f: &Failed) -> String {
    format!("Could not {}:\n{}", f.step, f.error)
}

// ---------------------------------------------------------------------------------------------
// Launch

pub enum Start {
    /// Start monitoring in this process.
    Monitor {
        portable: bool,
    },
    Exit,
}

/// Spec §4.4: decide what this launch does and carry out any install, update or uninstall.
pub fn launch(paths: &Paths) -> Start {
    let msg = control_message();
    let me = std::env::current_exe().unwrap_or_default();
    let target = installed_exe();
    let running = Win32Instances { msg }.running();
    let running_kind = match &running {
        None => Instance::None,
        Some(r) if r.path().is_some_and(|p| same_file(&p, &me)) => Instance::SameFile,
        Some(_) => Instance::OtherFile,
    };
    let facts = Facts {
        policy: Policy::SelfManaged,
        flag: Flag::from_args(std::env::args().skip(1)),
        this_version: version(),
        this_is_installed: same_file(&me, &target),
        installed: target.exists().then(|| file_version(&target).unwrap_or_default()),
        running: running_kind,
    };
    let ctx = Ctx { paths, msg, me, target, running };
    match lc::decide(&facts) {
        Launch::Monitor { portable } => {
            if !portable {
                // Left by the previous update.
                let _ = fs::remove_file(ctx.target.with_extension("exe.old"));
            }
            Start::Monitor { portable }
        }
        Launch::HandOff => {
            if let Some(r) = &ctx.running {
                r.show();
            }
            Start::Exit
        }
        Launch::StartInstalled => {
            match &ctx.running {
                Some(r) => r.show(),
                None => {
                    if let Err(e) = spawn(&ctx.target, &[]) {
                        error("MeltAlarm could not be started", &e);
                    }
                }
            }
            Start::Exit
        }
        Launch::OfferInstall => ctx.install(&facts),
        Launch::OfferUpdate { from, to } => ctx.update(from, to, false),
        Launch::OfferReplace { from, to } => ctx.update(from, to, true),
        Launch::Uninstall => ctx.uninstall(),
        Launch::NotInstalled => {
            info("MeltAlarm is not installed on this PC.");
            Start::Exit
        }
    }
}

struct Ctx<'a> {
    paths: &'a Paths,
    msg: u32,
    me: PathBuf,
    target: PathBuf,
    running: Option<Box<dyn Running>>,
}

impl Ctx<'_> {
    fn log(&self, events: &[LogEvent]) {
        let sink = LogSink::new(self.paths.log_dir.join("alarms.log"));
        let wall = crate::sys::wall_clock();
        for e in events {
            sink.write(&e.format(&wall));
        }
    }

    fn alarm_active(&self) -> bool {
        self.running.as_ref().and_then(|r| r.alarm_active()) == Some(true)
    }

    fn place(&self) -> PlaceProgram {
        PlaceProgram { from: self.me.clone(), to: self.target.clone(), backup: None, created_dir: false }
    }

    fn install(&self, facts: &Facts) -> Start {
        if self.alarm_active() {
            error("MeltAlarm can't be installed right now", ALARM_ACTIVE);
            return Start::Exit;
        }
        let v = version();
        let (button, autostart) = dialog(
            TD_SHIELD_ICON,
            &format!("Install MeltAlarm {v}?"),
            "It will be copied to Program Files and get a Start menu entry. \
             You can uninstall it from Installed apps.",
            &[(BTN_PRIMARY, "Install"), (BTN_SECONDARY, "Run without installing")],
            Some(("Start with Windows", true)),
        );
        match button {
            BTN_PRIMARY => {}
            BTN_SECONDARY => {
                return match facts.without_installing() {
                    Launch::Monitor { portable } => Start::Monitor { portable },
                    _ => {
                        if let Some(r) = &self.running {
                            r.show();
                        }
                        Start::Exit
                    }
                };
            }
            _ => return Start::Exit,
        }

        let mut steps: Vec<Box<dyn Step>> = vec![Box::new(StopRunning { msg: self.msg, stopped: None }), Box::new(self.place())];
        if let Some(link) = shortcut_path() {
            steps.push(Box::new(Shortcut { link, target: self.target.clone() }));
        }
        steps.push(Box::new(AppsEntry { exe: self.target.clone(), version: v, previous: None }));
        steps.push(Box::new(Startup {
            exe: self.target.clone(),
            enable: autostart,
            store: SettingsStore::new(self.paths.config_dir.join("settings.toml")),
            previous: None,
        }));

        match lc::run_atomic(&mut steps) {
            Ok(()) => {
                let mut events = vec![];
                if self.running.is_some() {
                    events.push(LogEvent::MonitoringStopped { reason: format!("installing {v}") });
                }
                events.push(LogEvent::Installed { version: v.to_string(), autostart });
                self.log(&events);
                if let Err(e) = spawn(&self.target, &["--installed"]) {
                    error("MeltAlarm was installed but could not be started", &e);
                }
                Start::Exit
            }
            Err(f) => {
                error(
                    "MeltAlarm could not be installed",
                    &format!("{}\n\nNothing was changed. MeltAlarm will run from this file without installing.", failure_text(&f)),
                );
                // A MeltAlarm that was closed for the install has been started again.
                if (Win32Instances { msg: self.msg }).running().is_some() { Start::Exit } else { Start::Monitor { portable: true } }
            }
        }
    }

    fn update(&self, from: Version, to: Version, downgrade: bool) -> Start {
        if self.alarm_active() {
            error("MeltAlarm can't be updated right now", ALARM_ACTIVE);
            return Start::Exit;
        }
        let (heading, button) = if downgrade {
            (format!("Replace MeltAlarm {from} with the older {to}?"), "Replace")
        } else {
            (format!("Update MeltAlarm {from} → {to}?"), "Update")
        };
        let (pressed, _) = dialog(
            TD_SHIELD_ICON,
            &heading,
            "Monitoring pauses for a few seconds. Settings, the alarm log and the start with Windows are kept.",
            &[(BTN_PRIMARY, button), (BTN_SECONDARY, "Cancel")],
            None,
        );
        if pressed != BTN_PRIMARY {
            return Start::Exit;
        }
        let mut steps: Vec<Box<dyn Step>> = vec![
            Box::new(StopRunning { msg: self.msg, stopped: None }),
            Box::new(self.place()),
            Box::new(AppsEntry { exe: self.target.clone(), version: to, previous: None }),
        ];
        match lc::run_atomic(&mut steps) {
            Ok(()) => {
                let mut events = vec![];
                if self.running.is_some() {
                    events.push(LogEvent::MonitoringStopped { reason: format!("updating to {to}") });
                }
                events.push(LogEvent::Updated { from: from.to_string(), to: to.to_string(), downgrade });
                self.log(&events);
                if let Err(e) = spawn(&self.target, &[]) {
                    error("MeltAlarm was updated but could not be started", &e);
                }
            }
            Err(f) => error(
                "MeltAlarm could not be updated",
                &format!("{}\n\nMeltAlarm {from} stays installed and keeps running.", failure_text(&f)),
            ),
        }
        Start::Exit
    }

    fn uninstall(&self) -> Start {
        let (pressed, delete_data) = dialog(
            TD_WARNING_ICON,
            "Uninstall MeltAlarm?",
            "Monitoring stops and MeltAlarm will no longer start with Windows.",
            &[(BTN_PRIMARY, "Uninstall"), (BTN_SECONDARY, "Cancel")],
            Some(("Also delete my settings and the alarm log", false)),
        );
        if pressed != BTN_PRIMARY {
            return Start::Exit;
        }
        let dir = install_dir();
        let (config_dir, log_dir) = (self.paths.config_dir.clone(), self.paths.log_dir.clone());
        let msg = self.msg;
        let mut steps: Vec<Box<dyn Step>> = vec![
            Box::new(StopRunning { msg, stopped: None }),
            Box::new(Do("remove the start with Windows", || TaskScheduler.set(None))),
            Box::new(Do("remove the Start menu shortcut", || shortcut_path().map_or(Ok(()), |p| remove_file_if_present(&p)))),
            Box::new(Do("remove the Installed apps entry", delete_apps_entry)),
            Box::new(Do("remove the program folder", move || remove_program(&dir))),
        ];
        if delete_data {
            steps.push(Box::new(Do("delete settings and the alarm log", move || {
                for d in [&config_dir, &log_dir] {
                    if d.exists() {
                        fs::remove_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
                    }
                }
                Ok(())
            })));
        } else {
            self.log(&[LogEvent::MonitoringStopped { reason: "uninstalled".into() }]);
        }
        let failed = lc::run_best_effort(&mut steps);
        if failed.is_empty() {
            info("MeltAlarm was uninstalled.");
        } else {
            let list: Vec<String> = failed.iter().map(failure_text).collect();
            error("MeltAlarm was uninstalled, with leftovers", &list.join("\n\n"));
        }
        Start::Exit
    }
}

/// Tray menu "Install…" (portable copy): run the install flow in a new process, which closes
/// this one when the user confirms. Uninstall has one entry point, Windows' Installed apps.
pub fn request_install() -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    spawn(&me, &["--install"])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meltalarm-place-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn place(dir: &Path, from: &Path) -> PlaceProgram {
        PlaceProgram { from: from.into(), to: dir.join("MeltAlarm").join("meltalarm.exe"), backup: None, created_dir: false }
    }

    #[test]
    fn fresh_install_and_its_undo_leave_nothing() {
        let d = temp("fresh");
        fs::write(d.join("new.exe"), b"new").unwrap();
        let mut step = place(&d, &d.join("new.exe"));
        step.apply().unwrap();
        assert_eq!(fs::read(&step.to).unwrap(), b"new");
        step.undo();
        assert!(!d.join("MeltAlarm").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn update_keeps_old_until_done_and_undo_restores_it() {
        let d = temp("update");
        fs::create_dir_all(d.join("MeltAlarm")).unwrap();
        fs::write(d.join("MeltAlarm").join("meltalarm.exe"), b"old").unwrap();
        fs::write(d.join("new.exe"), b"new").unwrap();
        let mut step = place(&d, &d.join("new.exe"));
        step.apply().unwrap();
        assert_eq!(fs::read(&step.to).unwrap(), b"new");
        assert_eq!(fs::read(step.to.with_extension("exe.old")).unwrap(), b"old");
        step.undo();
        assert_eq!(fs::read(&step.to).unwrap(), b"old");
        assert!(!step.to.with_extension("exe.old").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn failed_copy_restores_the_installed_file() {
        let d = temp("fail");
        fs::create_dir_all(d.join("MeltAlarm")).unwrap();
        fs::write(d.join("MeltAlarm").join("meltalarm.exe"), b"old").unwrap();
        let mut step = place(&d, &d.join("missing.exe"));
        assert!(step.apply().is_err());
        assert_eq!(fs::read(&step.to).unwrap(), b"old");
        assert!(d.join("MeltAlarm").exists());
        let _ = fs::remove_dir_all(&d);
    }
}
