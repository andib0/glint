//! "Start with Windows" via a per-user Task Scheduler sign-in task.
//!
//! The classic HKCU\...\Run entry turned out to be unreliable on Windows 11:
//! Explorer enumerates it at sign-in but can silently skip unsigned apps even
//! when Settings > Apps > Startup shows them enabled. A logon task needs no
//! admin rights and is launched by Task Scheduler regardless of that list.

use super::util::wide;
use std::os::windows::process::CommandExt;
use std::process::Command;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};

const TASK: &str = "Glint";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// Legacy (v0.1 - v0.2.1) Run-key autostart, removed on migration.
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn schtasks(args: &[&str]) -> bool {
    Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Task definition: run `exe` 5 s after `user` signs in, no time limit.
pub fn task_xml(exe: &str, user: &str) -> String {
    let dir = std::path::Path::new(exe)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let (exe, dir, user) = (xml_escape(exe), xml_escape(&dir), xml_escape(user));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Start Glint at sign-in</Description></RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId><Delay>PT5S</Delay></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>{exe}</Command><WorkingDirectory>{dir}</WorkingDirectory></Exec>
  </Actions>
</Task>
"#
    )
}

fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

fn remove_legacy() {
    unsafe {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN).as_ptr(), wide(TASK).as_ptr());
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            wide(APPROVED).as_ptr(),
            wide(TASK).as_ptr(),
        );
    }
}

fn legacy_present() -> bool {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(RUN).as_ptr(),
            wide(TASK).as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        ) == 0
    }
}

pub fn is_enabled() -> bool {
    schtasks(&["/Query", "/TN", TASK])
}

pub fn set(enabled: bool) -> Result<(), String> {
    remove_legacy();
    if !enabled {
        if !is_enabled() || schtasks(&["/Delete", "/TN", TASK, "/F"]) {
            return Ok(());
        }
        return Err("could not remove the sign-in task".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let xml = task_xml(&exe.display().to_string(), &current_user());
    let path = std::env::temp_dir().join(format!("glint-task-{}.xml", std::process::id()));
    // schtasks expects UTF-16 with a BOM for an encoding="UTF-16" document.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(|u| u.to_le_bytes()));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let ok = schtasks(&[
        "/Create",
        "/TN",
        TASK,
        "/XML",
        &path.display().to_string(),
        "/F",
    ]);
    let _ = std::fs::remove_file(&path);
    if ok {
        Ok(())
    } else {
        Err("could not create the sign-in task".into())
    }
}

/// Upgrade path: anyone with the old Run-key autostart gets the task instead.
pub fn migrate() {
    if legacy_present() {
        let _ = set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_contains_escaped_paths_and_user() {
        let x = task_xml(r"C:\Apps & Tools\<g>\glint.exe", r"PC\andi");
        assert!(x.contains(r"<Command>C:\Apps &amp; Tools\&lt;g&gt;\glint.exe</Command>"));
        assert!(x.contains(r"<WorkingDirectory>C:\Apps &amp; Tools\&lt;g&gt;</WorkingDirectory>"));
        assert_eq!(x.matches(r"<UserId>PC\andi</UserId>").count(), 2);
        assert!(x.contains("<LogonTrigger>") && x.contains("<ExecutionTimeLimit>PT0S"));
    }
}
