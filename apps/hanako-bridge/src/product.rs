use std::{
    env,
    ffi::{OsStr, OsString},
    os::windows::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, ensure};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn launch_manager_if_requested() -> anyhow::Result<bool> {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if !manager_requested(&arguments) {
        return Ok(false);
    }

    let install_dir = env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .context("cannot resolve Hanako Local Bridge install directory")?;
    let manager = manager_executable(&install_dir);
    ensure!(
        manager.is_file(),
        "Hanako Local Bridge internal manager is missing: {}",
        manager.display()
    );

    let mut command = Command::new(&manager);
    command
        .args(manager_arguments(&arguments))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    command
        .spawn()
        .with_context(|| format!("cannot open Hanako Local Bridge via {}", manager.display()))?;
    Ok(true)
}

fn manager_requested(arguments: &[OsString]) -> bool {
    arguments.is_empty()
        || arguments.iter().any(|argument| {
            matches!(
                argument.to_str(),
                Some("--manager" | "--open-manager" | "--url" | "--smoke-test")
            )
        })
}

fn manager_arguments(arguments: &[OsString]) -> Vec<OsString> {
    arguments
        .iter()
        .filter(|argument| {
            argument.as_os_str() != OsStr::new("--manager")
                && argument.as_os_str() != OsStr::new("--open-manager")
        })
        .cloned()
        .collect()
}

fn manager_executable(install_dir: &Path) -> PathBuf {
    let installed = install_dir.join("hanako-manager.exe");
    if installed.is_file() {
        return installed;
    }
    install_dir.join("hanako-manager")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn default_and_explicit_manager_arguments_open_the_product_ui() {
        assert!(manager_requested(&[]));
        assert!(manager_requested(&arguments(&["--manager"])));
        assert!(manager_requested(&arguments(&["--open-manager"])));
        assert!(manager_requested(&arguments(&[
            "--url",
            "http://127.0.0.1/"
        ])));
        assert!(manager_requested(&arguments(&["--smoke-test"])));
    }

    #[test]
    fn service_and_worker_arguments_do_not_open_the_product_ui() {
        for values in [
            &["--service"][..],
            &["--service-command", "status"][..],
            &["--status"][..],
            &["--repair"][..],
            &["--doctor"][..],
            &["--job-runner", "job.json"][..],
        ] {
            assert!(!manager_requested(&arguments(values)));
        }
    }

    #[test]
    fn internal_manager_does_not_receive_the_product_role_flag() {
        assert_eq!(
            manager_arguments(&arguments(&[
                "--manager",
                "--url",
                "http://127.0.0.1:8788/manager/"
            ])),
            arguments(&["--url", "http://127.0.0.1:8788/manager/"])
        );
    }
}
