//! "Run at Windows startup": a Task Scheduler task that starts MeltAlarm elevated at logon
//! without a UAC prompt (docs/FUNCTIONAL_SPEC.md §4.1). It only ever targets the installed
//! copy (Spec L2); callers pass that path. Driven through schtasks.exe with an XML definition.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use meltalarm_lifecycle::Autostart;

const TASK: &str = "MeltAlarm";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn schtasks(args: &[&str]) -> Option<std::process::Output> {
    Command::new("schtasks.exe").args(args).creation_flags(CREATE_NO_WINDOW).output().ok()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&gt;", ">").replace("&lt;", "<").replace("&amp;", "&")
}

pub struct TaskScheduler;

impl Autostart for TaskScheduler {
    fn target(&self) -> Option<PathBuf> {
        let out = schtasks(&["/Query", "/TN", TASK, "/XML", "ONE"])?;
        if !out.status.success() {
            return None;
        }
        // schtasks prints the XML in the console code page; the path is ASCII in practice.
        let text = String::from_utf8_lossy(&out.stdout);
        let start = text.find("<Command>")? + "<Command>".len();
        let end = start + text[start..].find("</Command>")?;
        Some(PathBuf::from(xml_unescape(text[start..end].trim().trim_matches('"'))))
    }

    fn set(&self, target: Option<&Path>) -> Result<(), String> {
        match target {
            None => delete(),
            Some(exe) => create(exe),
        }
    }
}

fn delete() -> Result<(), String> {
    let out = schtasks(&["/Delete", "/TN", TASK, "/F"]).ok_or("schtasks.exe failed to start")?;
    // Deleting a task that does not exist is fine.
    if out.status.success() || !task_exists() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()) }
}

fn create(exe: &Path) -> Result<(), String> {
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>MeltAlarm: GPU power cable monitor</Description></RegistrationInfo>
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
