#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::Context;
use hanako_bootstrap::{
    install_package_with_installer, uninstall_installation, write_embedded_package,
};
use uuid::Uuid;
use windows_sys::Win32::{
    Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW},
    UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const EMBEDDED_PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));

fn main() {
    let raw = env::args_os().collect::<Vec<_>>();
    let interactive = raw.len() == 1;
    match run(&raw) {
        Ok(message) => {
            if interactive {
                show_message("Hanako Local Bridge", &message, false);
            } else {
                println!("{message}");
            }
        }
        Err(error) => {
            if interactive {
                show_message("Hanako Local Bridge 安装失败", &format!("{error:#}"), true);
            } else {
                eprintln!("{error:#}");
            }
            std::process::exit(1);
        }
    }
}

fn run(arguments: &[std::ffi::OsString]) -> anyhow::Result<String> {
    if has_flag(arguments, "--smoke-test") {
        return Ok("smoke-test passed".to_string());
    }
    if has_flag(arguments, "--uninstall-worker") {
        let install_root =
            optional_path(arguments, "--install-root").context("missing --install-root")?;
        uninstall_installation(&install_root)?;
        schedule_self_delete()?;
        return Ok("uninstalled".to_string());
    }
    if has_flag(arguments, "--uninstall") {
        let install_root =
            optional_path(arguments, "--install-root").unwrap_or_else(default_install_root);
        launch_uninstall_worker(&install_root)?;
        return Ok("卸载程序已启动。".to_string());
    }
    let install_root =
        optional_path(arguments, "--install-root").unwrap_or_else(default_install_root);
    let test_mode = has_flag(arguments, "--test-mode");
    if let Some(package) = optional_path(arguments, "--payload") {
        let result = install_package_with_installer(
            &package,
            &install_root,
            test_mode,
            Some(&env::current_exe()?),
        )?;
        return Ok(serde_json::to_string(&result)?);
    }
    let (_temp, package) = write_embedded_package(EMBEDDED_PAYLOAD)?;
    let result = install_package_with_installer(
        &package,
        &install_root,
        test_mode,
        Some(&env::current_exe()?),
    )?;
    Ok(format!(
        "Hanako Local Bridge {} 已安装到\n{}",
        result.version,
        result.install_root.display()
    ))
}

fn launch_uninstall_worker(install_root: &Path) -> anyhow::Result<()> {
    let worker_root = env::temp_dir()
        .join("HanakoUninstall")
        .join(Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&worker_root)?;
    let worker = worker_root.join("hanako-uninstall.exe");
    std::fs::copy(env::current_exe()?, &worker)?;
    let mut command = Command::new(worker);
    command
        .arg("--uninstall-worker")
        .arg("--install-root")
        .arg(install_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    command.spawn()?;
    Ok(())
}

fn schedule_self_delete() -> anyhow::Result<()> {
    let current = wide(&env::current_exe()?.to_string_lossy());
    unsafe {
        if MoveFileExW(
            current.as_ptr(),
            std::ptr::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        ) == 0
        {
            anyhow::bail!("cannot schedule temporary uninstaller cleanup");
        }
    }
    thread::sleep(Duration::from_millis(200));
    Ok(())
}

fn default_install_root() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("HanakoLocalBridge")
}

fn optional_path(arguments: &[std::ffi::OsString], name: &str) -> Option<PathBuf> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(PathBuf::from)
}

fn has_flag(arguments: &[std::ffi::OsString], name: &str) -> bool {
    arguments
        .iter()
        .any(|argument| argument == OsStr::new(name))
}

fn show_message(title: &str, message: &str, error: bool) {
    let title = wide(title);
    let message = wide(message);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | if error { MB_ICONERROR } else { 0 },
        );
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[allow(dead_code)]
fn _assert_path(path: &Path) -> anyhow::Result<()> {
    path.parent().context("path has no parent")?;
    Ok(())
}
