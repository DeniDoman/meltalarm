//! "Run at Windows startup": a Task Scheduler task that starts MeltAlarm elevated at logon
//! without a UAC prompt (docs/FUNCTIONAL_SPEC.md §4). v0 drives schtasks.exe with an XML
//! definition; the architecture's COM `ITaskService` variant can replace it later.

use std::os::windows::process::CommandExt;
use std::process::Command;

const TASK: &str = "MeltAlarm";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> Option<std::process::Output> {
    Command::new("schtasks.exe").args(args).creation_flags(CREATE_NO_WINDOW).output().ok()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Is a task present that starts *this* executable?
pub fn is_enabled() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(out) = schtasks(&["/Query", "/TN", TASK, "/XML", "ONE"]) else { return false };
    out.status.success() && {
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        text.contains(&xml_escape(&exe.to_string_lossy()).to_lowercase())
    }
}

pub fn set(enable: bool) -> Result<(), String> {
    if !enable {
        let out = schtasks(&["/Delete", "/TN", TASK, "/F"]).ok_or("schtasks.exe failed to start")?;
        // Deleting a task that does not exist is fine.
        return if out.status.success() || !task_exists() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).into()) };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>MeltAlarm: 12V-2x6 GPU cable monitor for MSI Ai1x00TS PSUs</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command></Exec></Actions>
</Task>
"#,
        user = xml_escape(&user),
        exe = xml_escape(&exe.to_string_lossy())
    );
    let path = std::env::temp_dir().join("meltalarm-task.xml");
    let mut bytes = vec![0xFF, 0xFE]; // UTF-16LE BOM, as schtasks expects
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let out = schtasks(&["/Create", "/TN", TASK, "/XML", &path.to_string_lossy(), "/F"]).ok_or("schtasks.exe failed to start");
    let _ = std::fs::remove_file(&path);
    let out = out?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()) }
}

fn task_exists() -> bool {
    schtasks(&["/Query", "/TN", TASK]).is_some_and(|o| o.status.success())
}
