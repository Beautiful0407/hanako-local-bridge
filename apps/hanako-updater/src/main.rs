use std::{
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use chrono::Utc;
use hanako_bridge_core::{
    store::write_json_atomic,
    update::{
        UpdateManifest, UpdateState, read_payload_manifest, sha256_file, sign_manifest,
        write_update_state,
    },
};
use hanako_maintenance::{
    PayloadTransaction, build_release_package, check_update, launch_product_entry, prepare_update,
    start_installed_service, stop_installed_service_and_processes,
};
use serde::Serialize;
use sysinfo::{Pid, ProcessesToUpdate, System};
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HandoffResult {
    started: bool,
    attempt_id: String,
    expected_version: String,
    state_path: PathBuf,
    worker_pid: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let arguments = env::args_os().collect::<Vec<_>>();
    let command = arguments
        .get(1)
        .and_then(|value| value.to_str())
        .unwrap_or("help");
    match command {
        "check" => run_check(&arguments),
        "apply" => run_apply_launcher(&arguments),
        "worker" => run_worker(&arguments),
        "cleanup" => run_cleanup(&arguments),
        "sign" => run_sign(&arguments),
        "hash" => run_hash(&arguments),
        "pack" => run_pack(&arguments),
        "version" | "--version" | "-V" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            print_help();
            Ok(())
        }
    }
}

fn run_check(arguments: &[std::ffi::OsString]) -> anyhow::Result<()> {
    let install_root = required_path(arguments, "--install-root")?;
    let manifest = required_string(arguments, "--manifest")?;
    let current_version = optional_string(arguments, "--current-version").unwrap_or_else(|| {
        installed_version(&install_root).unwrap_or_else(|_| "0.0.0".to_string())
    });
    let result = check_update(&install_root, &manifest, &current_version)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn run_apply_launcher(arguments: &[std::ffi::OsString]) -> anyhow::Result<()> {
    let install_root = required_path(arguments, "--install-root")?;
    let manifest = required_string(arguments, "--manifest")?;
    let expected_version = optional_string(arguments, "--expected-version").unwrap_or_else(|| {
        check_update(
            &install_root,
            &manifest,
            &installed_version(&install_root).unwrap_or_else(|_| "0.0.0".to_string()),
        )
        .map(|value| value.target_version)
        .unwrap_or_default()
    });
    ensure!(
        !expected_version.is_empty(),
        "expected update version is missing"
    );
    let attempt_id = optional_string(arguments, "--attempt-id")
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let state_path = optional_path(arguments, "--state-path")
        .unwrap_or_else(|| install_root.join("data/update-state.json"));
    let worker_root = env::temp_dir().join("HanakoMaintenance").join(&attempt_id);
    fs::create_dir_all(&worker_root)?;
    let worker_exe = worker_root.join("hanako-maintenance.exe");
    fs::copy(env::current_exe()?, &worker_exe)?;
    let mut command = Command::new(&worker_exe);
    command
        .arg("worker")
        .arg("--install-root")
        .arg(&install_root)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--expected-version")
        .arg(&expected_version)
        .arg("--attempt-id")
        .arg(&attempt_id)
        .arg("--state-path")
        .arg(&state_path)
        .arg("--worker-root")
        .arg(&worker_root)
        .arg("--launcher-pid")
        .arg(std::process::id().to_string());
    if has_flag(arguments, "--test-mode") {
        ensure!(
            !manifest.starts_with("https://"),
            "test mode cannot be used with remote updates"
        );
        command.arg("--test-mode");
    }
    let mut child = configure_detached(&mut command).spawn()?;
    wait_for_handoff(&state_path, &mut child, Duration::from_secs(10))?;
    println!(
        "{}",
        serde_json::to_string(&HandoffResult {
            started: true,
            attempt_id,
            expected_version,
            state_path,
            worker_pid: child.id(),
        })?
    );
    Ok(())
}

fn run_worker(arguments: &[std::ffi::OsString]) -> anyhow::Result<()> {
    let install_root = required_path(arguments, "--install-root")?;
    let manifest_source = required_string(arguments, "--manifest")?;
    let expected_version = required_string(arguments, "--expected-version")?;
    let attempt_id = required_string(arguments, "--attempt-id")?;
    let state_path = required_path(arguments, "--state-path")?;
    let worker_root = required_path(arguments, "--worker-root")?;
    let launcher_pid = optional_string(arguments, "--launcher-pid")
        .map(|value| value.parse::<u32>())
        .transpose()?;
    let test_mode = has_flag(arguments, "--test-mode");
    let log_path = install_root.join("logs/update.log");
    let started_at = Utc::now().to_rfc3339();
    fs::create_dir_all(log_path.parent().context("update log has no parent")?)?;
    append_log(
        &log_path,
        &format!(
            "\n=== Rust update {attempt_id} started {started_at}; expected {expected_version} ===\n"
        ),
    )?;
    let mut state = UpdateState {
        schema_version: 1,
        attempt_id: attempt_id.clone(),
        status: "running".to_string(),
        expected_version: expected_version.clone(),
        installed_version: String::new(),
        message: "Update worker accepted the request.".to_string(),
        log_path: log_path.clone(),
        started_at,
        finished_at: String::new(),
        exit_code: 1,
    };
    write_update_state(&state_path, &state)?;
    if let Some(launcher_pid) = launcher_pid {
        ensure!(
            wait_for_process_exit(launcher_pid, Duration::from_secs(30)),
            "update launcher did not exit before replacement"
        );
    }

    let work_root = worker_root.join("work");
    let backup_root = worker_root.join("backup");
    let mut transaction = None;
    let mut applied = false;
    let mut service_stopped = false;
    let update_result = (|| -> anyhow::Result<String> {
        let prepared = prepare_update(&install_root, &manifest_source, &work_root)?;
        ensure!(
            prepared.manifest.version == expected_version,
            "expected version {expected_version}, but manifest contains {}",
            prepared.manifest.version
        );
        append_log(
            &log_path,
            &format!(
                "Verified package {} ({})\n",
                prepared.package_source,
                prepared.package_path.display()
            ),
        )?;
        let prepared_transaction = PayloadTransaction::prepare(
            &install_root,
            &prepared.stage_root,
            &backup_root,
            prepared.payload,
        )?;
        transaction = Some(prepared_transaction);
        if !test_mode {
            stop_installed_service_and_processes(&install_root)?;
            service_stopped = true;
        }
        transaction
            .as_ref()
            .context("update transaction missing")?
            .apply()?;
        applied = true;
        let installed = transaction
            .as_ref()
            .context("update transaction missing")?
            .installed_version()
            .to_string();
        if !test_mode {
            start_installed_service(&install_root)?;
        }
        Ok(installed)
    })();

    match update_result {
        Ok(installed_version) => {
            state.status = "succeeded".to_string();
            state.installed_version = installed_version;
            state.message = "Update completed successfully.".to_string();
            state.finished_at = Utc::now().to_rfc3339();
            state.exit_code = 0;
            append_log(&log_path, "Update completed successfully.\n")?;
            write_update_state(&state_path, &state)?;
            if !test_mode {
                launch_product_entry(&install_root);
            }
        }
        Err(error) => {
            append_log(&log_path, &format!("Update failed: {error:#}\n"))?;
            if applied
                && let Some(transaction) = &transaction
                && let Err(rollback_error) = transaction.rollback()
            {
                append_log(&log_path, &format!("Rollback failed: {rollback_error:#}\n"))?;
            }
            if service_stopped && !test_mode {
                let _ = start_installed_service(&install_root);
            }
            state.status = "failed".to_string();
            state.message = error.to_string();
            state.finished_at = Utc::now().to_rfc3339();
            state.exit_code = 1;
            write_update_state(&state_path, &state)?;
        }
    }
    if !test_mode {
        spawn_cleanup(&install_root, &worker_root);
    }
    Ok(())
}

fn run_cleanup(arguments: &[std::ffi::OsString]) -> anyhow::Result<()> {
    let pid = required_string(arguments, "--pid")?.parse::<u32>()?;
    let path = required_path(arguments, "--path")?;
    let temp_root = absolute(&env::temp_dir())?;
    let path = absolute(&path)?;
    ensure!(
        path.starts_with(&temp_root),
        "cleanup path is outside the temporary directory"
    );
    let _ = wait_for_process_exit(pid, Duration::from_secs(60));
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn run_sign(arguments: &[std::ffi::OsString]) -> anyhow::Result<()> {
    let manifest_path = required_path(arguments, "--manifest")?;
    let private_key_path = required_path(arguments, "--private-key")?;
    let mut manifest: UpdateManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    sign_manifest(&mut manifest, &fs::read_to_string(private_key_path)?)?;
    write_json_atomic(&manifest_path, &manifest)?;
    println!("{}", serde_json::to_string(&manifest)?);
    Ok(())
}

fn run_hash(arguments: &[std::ffi::OsString]) -> anyhow::Result<()> {
    let path = arguments
        .get(2)
        .map(PathBuf::from)
        .context("usage: hanako-maintenance hash <file>")?;
    println!("{}", sha256_file(&path)?);
    Ok(())
}

fn run_pack(arguments: &[std::ffi::OsString]) -> anyhow::Result<()> {
    let binaries = required_path(arguments, "--binaries")?;
    let output = required_path(arguments, "--output")?;
    let public_key = required_path(arguments, "--public-key")?;
    let version = optional_string(arguments, "--version")
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let channel = optional_string(arguments, "--channel").unwrap_or_else(|| "alpha".to_string());
    let package_url = optional_string(arguments, "--package-url");
    let signing_key = optional_path(arguments, "--signing-key");
    let notes = optional_string(arguments, "--notes")
        .unwrap_or_else(|| format!("Hanako Local Bridge {version}"));
    let result = build_release_package(
        &binaries,
        &output,
        &version,
        &channel,
        &public_key,
        package_url.as_deref(),
        signing_key.as_deref(),
        &notes,
    )?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn installed_version(install_root: &Path) -> anyhow::Result<String> {
    let payload_path = install_root.join("payload-manifest.json");
    if payload_path.is_file() {
        return Ok(read_payload_manifest(&payload_path)?.version);
    }
    let package_path = install_root.join("package.json");
    if package_path.is_file() {
        let value: serde_json::Value = serde_json::from_slice(&fs::read(package_path)?)?;
        if let Some(version) = value["version"].as_str() {
            return Ok(version.to_string());
        }
    }
    anyhow::bail!("installed version metadata is missing")
}

fn spawn_cleanup(install_root: &Path, worker_root: &Path) {
    let maintenance = install_root.join("hanako-maintenance.exe");
    if !maintenance.is_file() {
        return;
    }
    let mut command = Command::new(maintenance);
    command
        .arg("cleanup")
        .arg("--pid")
        .arg(std::process::id().to_string())
        .arg("--path")
        .arg(worker_root);
    let _ = configure_detached(&mut command).spawn();
}

fn wait_for_handoff(
    state_path: &Path,
    worker: &mut std::process::Child,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(bytes) = fs::read(state_path)
            && let Ok(state) = serde_json::from_slice::<UpdateState>(&bytes)
            && matches!(state.status.as_str(), "running" | "succeeded" | "failed")
        {
            return Ok(());
        }
        if let Some(status) = worker.try_wait()? {
            if let Ok(bytes) = fs::read(state_path)
                && let Ok(state) = serde_json::from_slice::<UpdateState>(&bytes)
                && matches!(state.status.as_str(), "running" | "succeeded" | "failed")
            {
                return Ok(());
            }
            anyhow::bail!(
                "update worker exited with {} before confirming handoff",
                status.code().unwrap_or(1)
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("timed out waiting for update worker handoff")
}

fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let mut system = System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);
        if system.process(Pid::from_u32(pid)).is_none() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn append_log(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn configure_detached(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    command
}

fn required_string(arguments: &[std::ffi::OsString], name: &str) -> anyhow::Result<String> {
    optional_string(arguments, name).with_context(|| format!("missing {name}"))
}

fn optional_string(arguments: &[std::ffi::OsString], name: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
}

fn required_path(arguments: &[std::ffi::OsString], name: &str) -> anyhow::Result<PathBuf> {
    optional_path(arguments, name).with_context(|| format!("missing {name}"))
}

fn optional_path(arguments: &[std::ffi::OsString], name: &str) -> Option<PathBuf> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(PathBuf::from)
}

fn has_flag(arguments: &[std::ffi::OsString], name: &str) -> bool {
    arguments.iter().any(|argument| argument == name)
}

fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn print_help() {
    println!(
        "Hanako Rust maintenance\n\
         check --install-root <path> --manifest <path-or-https-url> [--current-version <version>]\n\
         apply --install-root <path> --manifest <path-or-https-url> [--expected-version <version>]\n\
         pack --binaries <path> --output <path> --public-key <path> [--signing-key <path>]\n\
         sign --manifest <path> --private-key <xml-path>\n\
         hash <file>"
    );
}
