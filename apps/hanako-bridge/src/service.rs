use std::{
    env,
    ffi::OsString,
    fs,
    net::{Ipv4Addr, TcpListener},
    os::windows::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use hanako_bridge_core::{RuntimeConfig, decode_console_bytes};
use serde::Serialize;
use serde_json::json;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestedServiceCommand {
    command: String,
    command_index: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub task_name: String,
    pub task_exists: bool,
    pub health_ok: bool,
    pub health_url: String,
    pub executable: PathBuf,
}

pub async fn run_service_command_if_requested() -> anyhow::Result<bool> {
    let arguments = env::args_os().collect::<Vec<_>>();
    let Some(request) = requested_service_command(&arguments) else {
        return Ok(false);
    };
    let command = request.command.as_str();
    let install_dir = env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("cannot resolve bridge install directory"))?;
    let config_override = argument_value(&arguments, "--service-config").map(PathBuf::from);
    let cleanup_task = argument_value(&arguments, "--cleanup-task");
    let runtime = RuntimeConfig::load(&install_dir, config_override.as_deref())?;
    let task_name = format!("{} MCP", runtime.config.service.task_prefix.trim());
    match command {
        "install" | "repair" => {
            install_task(&runtime, &task_name)?;
            start_task(&task_name)?;
            print_json(&service_status(&runtime, &task_name).await?)?;
        }
        "uninstall" => {
            let _ = stop_task(&task_name);
            delete_task(&task_name)?;
            let legacy_tunnel = format!("{} Tunnel", runtime.config.service.task_prefix.trim());
            let _ = delete_task(&legacy_tunnel);
            let _ = delete_task(&manager_action_task_name(&runtime));
            print_json(&service_status(&runtime, &task_name).await?)?;
        }
        "start" => {
            start_task(&task_name)?;
            print_json(&service_status(&runtime, &task_name).await?)?;
        }
        "stop" => {
            stop_task(&task_name)?;
            print_json(&service_status(&runtime, &task_name).await?)?;
        }
        "restart" => {
            spawn_restart_worker(&runtime, &task_name)?;
            print_json(&service_status(&runtime, &task_name).await?)?;
        }
        "restart-worker" => {
            let wait_pid = request
                .command_index
                .and_then(|index| arguments.get(index + 2))
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<u32>().ok());
            let result = async {
                if let Some(pid) = wait_pid {
                    wait_for_process_exit(pid, Duration::from_secs(30)).await;
                }
                let _ = stop_task(&task_name);
                tokio::time::sleep(Duration::from_millis(600)).await;
                start_task(&task_name)
            }
            .await;
            cleanup_action_task(cleanup_task.as_deref());
            result?;
        }
        "deferred-action" => {
            let action = request
                .command_index
                .and_then(|index| arguments.get(index + 2))
                .and_then(|value| value.to_str())
                .unwrap_or("");
            tokio::time::sleep(Duration::from_millis(800)).await;
            let result = match action {
                "stop" => stop_task(&task_name),
                "restart" => {
                    let _ = stop_task(&task_name);
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    start_task(&task_name)
                }
                "repair" => {
                    install_task(&runtime, &task_name).and_then(|()| start_task(&task_name))
                }
                _ => Err(anyhow::anyhow!("unknown deferred service action: {action}")),
            };
            cleanup_action_task(cleanup_task.as_deref());
            result?;
        }
        "status" => print_json(&service_status(&runtime, &task_name).await?)?,
        "doctor" => {
            let status = service_status(&runtime, &task_name).await?;
            print_json(&json!({
                "ok": status.task_exists && status.health_ok,
                "runtime": "rust",
                "version": env!("CARGO_PKG_VERSION"),
                "installDir": runtime.install_dir,
                "configPath": runtime.config_path,
                "service": status,
                "ports": {
                    "mcp": runtime.config.filesystem.port,
                    "manager": runtime.config.filesystem.approval_port
                },
                "cloudEnabled": runtime.config.cloud.enabled
            }))?;
        }
        _ => anyhow::bail!("unknown service command: {command}"),
    }
    Ok(true)
}

fn requested_service_command(arguments: &[OsString]) -> Option<RequestedServiceCommand> {
    if let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--service-command")
    {
        return Some(RequestedServiceCommand {
            command: arguments
                .get(index + 1)
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_string(),
            command_index: Some(index),
        });
    }
    for (alias, command) in [
        ("--status", "status"),
        ("--repair", "repair"),
        ("--doctor", "doctor"),
    ] {
        if arguments.iter().any(|argument| argument == alias) {
            return Some(RequestedServiceCommand {
                command: command.to_string(),
                command_index: None,
            });
        }
    }
    None
}

pub fn spawn_deferred_action(runtime: &RuntimeConfig, action: &str) -> anyhow::Result<()> {
    if !matches!(action, "stop" | "restart" | "repair") {
        anyhow::bail!("unsupported deferred service action: {action}");
    }
    schedule_service_action(runtime, "deferred-action", &[action])
}

pub async fn service_status(
    runtime: &RuntimeConfig,
    task_name: &str,
) -> anyhow::Result<ServiceStatus> {
    let health_url = format!("http://127.0.0.1:{}/health", runtime.config.filesystem.port);
    let health_ok = reqwest_health(&health_url).await;
    Ok(ServiceStatus {
        task_name: task_name.to_string(),
        task_exists: task_exists(task_name),
        health_ok,
        health_url,
        executable: env::current_exe()?,
    })
}

fn install_task(runtime: &RuntimeConfig, task_name: &str) -> anyhow::Result<()> {
    let executable = env::current_exe()?;
    let user = current_user()?;
    // Delete the task first so its per-minute trigger cannot relaunch the
    // bridge while we are stopping it. `/End` only ends the current run and the
    // trigger respawns the process within a minute, so the port never frees and
    // the install times out. We delete rather than `/Change /DISABLE` because
    // that verb is rejected as "The parameter is incorrect" on some Windows
    // versions. The `/Create /F` below recreates the task from scratch, so the
    // delete is safe.
    let _ = delete_task(task_name);
    let _ = stop_task(task_name);
    // `/End` only ends the scheduled-task run instance; the bridge process
    // itself is started by the task host (svchost) and survives `/End`, still
    // holding ports 8787/8788. Terminate the bridge executable directly so the
    // ports are actually released. The task is already deleted, so the
    // per-minute trigger cannot relaunch it while we are installing.
    kill_bridge_processes()?;
    wait_for_ports_released(
        &[
            runtime.config.filesystem.port,
            runtime.config.filesystem.approval_port,
        ],
        Duration::from_secs(8),
    )?;
    let xml = task_xml(
        task_name,
        &user,
        &executable,
        &runtime.config_path,
        runtime.config.service.restart_delay_seconds,
    );
    let task_file = env::temp_dir().join(format!(
        "hanako-bridge-task-{}-{}.xml",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    write_task_xml(&task_file, &xml)?;
    let result = run_schtasks([
        "/Create",
        "/TN",
        task_name,
        "/XML",
        task_file.to_string_lossy().as_ref(),
        "/F",
    ]);
    let _ = fs::remove_file(&task_file);
    result?;
    let legacy_tunnel = format!("{} Tunnel", runtime.config.service.task_prefix.trim());
    let _ = delete_task(&legacy_tunnel);
    Ok(())
}

fn wait_for_ports_released(ports: &[u16], timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut listeners = Vec::with_capacity(ports.len());
        let mut blocked_port = None;
        for port in ports {
            match TcpListener::bind((Ipv4Addr::LOCALHOST, *port)) {
                Ok(listener) => listeners.push(listener),
                Err(_) => {
                    blocked_port = Some(*port);
                    break;
                }
            }
        }
        drop(listeners);
        if blocked_port.is_none() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for local port {} to be released",
                blocked_port.unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn start_task(task_name: &str) -> anyhow::Result<()> {
    run_schtasks(["/Run", "/TN", task_name])
}

fn stop_task(task_name: &str) -> anyhow::Result<()> {
    run_schtasks(["/End", "/TN", task_name])
}

/// Terminates every running hanako-bridge.exe process except the caller.
///
/// The scheduled task's `/End` stops the task run instance but does not
/// reliably terminate the bridge process itself (started by the task host),
/// so ports 8787/8788 stay occupied and install/repair times out waiting for
/// them to be released. The caller must have deleted the scheduled task
/// first, otherwise the per-minute trigger could relaunch the process while
/// we are terminating it. The current process (the repair/install invocation
/// itself) is excluded so the command can finish recreating the task.
fn kill_bridge_processes() -> anyhow::Result<()> {
    let self_pid = std::process::id();
    // `tasklist /FI "IMAGENAME eq hanako-bridge.exe"` output: header line +
    // rows of `"hanako-bridge.exe","<pid>",...`. Parse every PID and kill
    // all but our own process.
    let tasklist = Command::new("tasklist.exe")
        .args(["/FI", "IMAGENAME eq hanako-bridge.exe", "/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let text = decode_console_bytes(&tasklist.stdout);
    let mut killed_any = false;
    for pid in parse_bridge_pids(&text) {
        if pid == self_pid {
            continue;
        }
        let _ = Command::new("taskkill.exe")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
        killed_any = true;
    }
    let _ = killed_any;
    Ok(())
}

/// Parses `tasklist /FI "IMAGENAME eq hanako-bridge.exe" /FO CSV /NH` output
/// into a list of PIDs. Each row looks like:
/// `"hanako-bridge.exe","1234","Console","1","10,123 K"`.
fn parse_bridge_pids(text: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split(',');
        let _image = parts.next();
        let Some(pid_field) = parts.next() else {
            continue;
        };
        let pid_text = pid_field.trim_matches('"');
        if let Ok(pid) = pid_text.parse::<u32>() {
            pids.push(pid);
        }
    }
    pids
}

fn delete_task(task_name: &str) -> anyhow::Result<()> {
    run_schtasks(["/Delete", "/TN", task_name, "/F"])
}

fn task_exists(task_name: &str) -> bool {
    run_schtasks(["/Query", "/TN", task_name]).is_ok()
}

fn run_schtasks<const N: usize>(arguments: [&str; N]) -> anyhow::Result<()> {
    let output = Command::new("schtasks.exe")
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let error = decode_console_bytes(&output.stderr).trim().to_string();
    let output_text = decode_console_bytes(&output.stdout).trim().to_string();
    anyhow::bail!(
        "{}",
        if !error.is_empty() {
            error
        } else {
            output_text
        }
    )
}

fn spawn_restart_worker(runtime: &RuntimeConfig, task_name: &str) -> anyhow::Result<()> {
    let _ = task_name;
    let pid = std::process::id().to_string();
    schedule_service_action(runtime, "restart-worker", &[&pid])
}

fn schedule_service_action(
    runtime: &RuntimeConfig,
    command: &str,
    arguments: &[&str],
) -> anyhow::Result<()> {
    let executable = env::current_exe()?;
    let user = current_user()?;
    let action_task = manager_action_task_name(runtime);
    let xml = action_task_xml(
        &action_task,
        &user,
        &executable,
        &runtime.config_path,
        command,
        arguments,
    );
    let task_file = env::temp_dir().join(format!(
        "hanako-manager-action-{}-{}.xml",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    write_task_xml(&task_file, &xml)?;
    let create_result = run_schtasks([
        "/Create",
        "/TN",
        &action_task,
        "/XML",
        task_file.to_string_lossy().as_ref(),
        "/F",
    ]);
    let _ = fs::remove_file(&task_file);
    create_result?;
    if let Err(error) = start_task(&action_task) {
        let _ = delete_task(&action_task);
        return Err(error);
    }
    Ok(())
}

fn manager_action_task_name(runtime: &RuntimeConfig) -> String {
    format!(
        "{} Manager Action",
        runtime.config.service.task_prefix.trim()
    )
}

fn cleanup_action_task(task_name: Option<&str>) {
    if let Some(task_name) = task_name.filter(|value| !value.trim().is_empty()) {
        let _ = delete_task(task_name);
    }
}

fn argument_value(arguments: &[std::ffi::OsString], name: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(|value| value.to_string_lossy().into_owned())
}

fn current_user() -> anyhow::Result<String> {
    let output = Command::new("whoami.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("cannot resolve the current Windows user");
    }
    Ok(decode_console_bytes(&output.stdout).trim().to_string())
}

async fn reqwest_health(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    let host = url.host_str().unwrap_or("127.0.0.1");
    let port = url.port_or_known_default().unwrap_or(80);
    let Ok(Ok(mut stream)) = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    else {
        return false;
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        url.path(),
        host,
        port
    );
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut response = Vec::new();
    if tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
        .await
        .is_err()
    {
        return false;
    }
    response.starts_with(b"HTTP/1.1 200") || response.starts_with(b"HTTP/1.0 200")
}

async fn wait_for_process_exit(pid: u32, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if !process_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn process_alive(pid: u32) -> bool {
    let output = Command::new("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
}

fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn task_xml(
    task_name: &str,
    user: &str,
    executable: &Path,
    config_path: &Path,
    restart_delay_seconds: u64,
) -> String {
    let description = format!("Hanako Local Bridge Rust service: {task_name}");
    let restart_minutes = restart_delay_seconds.saturating_add(59).max(60) / 60;
    let watchdog_start = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z");
    let mut service_arguments = vec!["--service".to_string()];
    if !config_path.as_os_str().is_empty() {
        service_arguments.push("--service-config".to_string());
        service_arguments.push(xml_quoted_argument(&config_path.to_string_lossy()));
    }
    let service_arguments = service_arguments.join(" ");
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{}</UserId>
    </LogonTrigger>
    <TimeTrigger>
      <StartBoundary>{}</StartBoundary>
      <Repetition>
        <Interval>PT{}M</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>true</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT{}M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>{}</Arguments>
      <WorkingDirectory>{}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        xml_escape(&description),
        xml_escape(user),
        watchdog_start,
        restart_minutes,
        xml_escape(user),
        restart_minutes,
        xml_escape(executable.to_string_lossy().as_ref()),
        xml_escape(&service_arguments),
        xml_escape(
            executable
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_string_lossy()
                .as_ref()
        ),
    )
}

fn action_task_xml(
    task_name: &str,
    user: &str,
    executable: &Path,
    config_path: &Path,
    command: &str,
    arguments: &[&str],
) -> String {
    let description = format!("Hanako Local Bridge manager action: {command}");
    let mut command_arguments = vec![
        "--service-command".to_string(),
        command.to_string(),
        "--service-config".to_string(),
        config_path.to_string_lossy().into_owned(),
        "--cleanup-task".to_string(),
        task_name.to_string(),
    ];
    command_arguments.splice(2..2, arguments.iter().map(|value| (*value).to_string()));
    let command_line = command_arguments
        .iter()
        .map(|value| xml_quoted_argument(value))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{}</Description>
  </RegistrationInfo>
  <Triggers />
  <Principals>
    <Principal id="Author">
      <UserId>{}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT5M</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>{}</Arguments>
      <WorkingDirectory>{}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        xml_escape(&description),
        xml_escape(user),
        xml_escape(executable.to_string_lossy().as_ref()),
        command_line,
        xml_escape(
            executable
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_string_lossy()
                .as_ref()
        ),
    )
}

fn xml_quoted_argument(value: &str) -> String {
    format!("&quot;{}&quot;", xml_escape(value))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn write_task_xml(path: &Path, xml: &str) -> anyhow::Result<()> {
    let mut bytes = Vec::with_capacity(xml.len() * 2 + 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn task_xml_escapes_paths_and_runs_the_rust_service_directly() {
        let xml = task_xml(
            "Hanako & Bridge MCP",
            r"DOMAIN\User",
            Path::new(r"C:\Apps\Hanako & Bridge\hanako-bridge.exe"),
            Path::new(r"C:\Apps\Hanako & Bridge\config.json"),
            3,
        );
        assert!(xml.contains("Hanako &amp; Bridge MCP"));
        assert!(xml.contains("hanako-bridge.exe"));
        assert!(xml.contains("--service"));
        assert!(xml.contains("<Interval>PT1M</Interval>"));
        assert!(xml.contains("<TimeTrigger>"));
        assert!(xml.contains("<StartBoundary>"));
        assert!(xml.contains("<StopAtDurationEnd>false</StopAtDurationEnd>"));
        assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(!xml.contains("powershell"));
        assert!(!xml.contains("wscript"));
        assert!(!xml.contains("node.exe"));
    }

    #[test]
    fn task_xml_writer_emits_utf16_with_bom() {
        let path = std::env::temp_dir().join(format!(
            "hanako-task-xml-{}.xml",
            uuid::Uuid::new_v4().simple()
        ));
        let source = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><Task/>";
        write_task_xml(&path, source).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        assert_eq!(String::from_utf16(&units).unwrap(), source);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn manager_actions_run_in_an_independent_on_demand_task() {
        let xml = action_task_xml(
            "Hanako & Bridge Manager Action",
            r"DOMAIN\User",
            Path::new(r"C:\Apps\Hanako & Bridge\hanako-bridge.exe"),
            Path::new(r"C:\Apps\Hanako & Bridge\config.json"),
            "deferred-action",
            &["repair"],
        );
        assert!(xml.contains("<Triggers />"));
        assert!(xml.contains("&quot;deferred-action&quot;"));
        assert!(xml.contains("&quot;repair&quot;"));
        assert!(xml.contains("&quot;--service-config&quot;"));
        assert!(xml.contains(r"C:\Apps\Hanako &amp; Bridge\config.json"));
        assert!(xml.contains("&quot;--cleanup-task&quot;"));
        assert!(xml.contains("Hanako &amp; Bridge Manager Action"));
        assert!(!xml.contains("CREATE_BREAKAWAY_FROM_JOB"));
    }

    // Regression for the install failure where an existing per-minute service
    // task kept relaunching the bridge, so ports never freed and repair/install
    // timed out. install_task deletes the task before stopping the bridge, so
    // the trigger cannot respawn it; the delete must succeed (and not fail with
    // "The parameter is incorrect", which /Change /DISABLE did on some Windows
    // versions). Uses a throwaway task name so it never touches a real install.
    #[test]
    fn delete_task_removes_a_self_restarting_task() {
        let task_name = format!("HanakoDeleteTest {}", uuid::Uuid::new_v4().simple());
        // Create a minimal task that repeats every minute (schtasks CLI form).
        let created = run_schtasks([
            "/Create",
            "/TN",
            &task_name,
            "/TR",
            "cmd.exe /c exit",
            "/SC",
            "MINUTE",
            "/MO",
            "1",
            "/F",
        ]);
        if created.is_err() {
            // Task creation can be blocked in a locked-down CI account; skip
            // rather than fail on an environment limitation.
            return;
        }
        delete_task(&task_name).expect("delete should succeed without a parameter error");
        // After deletion the task must no longer exist.
        assert!(
            !task_exists(&task_name),
            "task should be gone after delete_task"
        );
    }

    #[test]
    fn waits_for_legacy_service_ports_before_starting_replacement() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(listener);
        });
        let started = Instant::now();
        wait_for_ports_released(&[port], Duration::from_secs(2)).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(150));
    }

    #[test]
    fn parses_bridge_pids_from_tasklist_csv() {
        let sample = "\"hanako-bridge.exe\",\"1234\",\"Console\",\"1\",\"10,123 K\"\r\n\
\"hanako-bridge.exe\",\"5678\",\"Console\",\"1\",\"8,001 K\"\r\n";
        let pids = parse_bridge_pids(sample);
        assert_eq!(pids, vec![1234, 5678]);
    }

    #[test]
    fn parses_bridge_pids_skips_malformed_rows() {
        let sample = "\"hanako-bridge.exe\",\"1234\",\"Console\",\"1\",\"10,123 K\"\r\n\
not-a-csv-row\r\n\
\"hanako-bridge.exe\",\"abc\",\"Console\",\"1\",\"8,001 K\"\r\n";
        let pids = parse_bridge_pids(sample);
        assert_eq!(pids, vec![1234]);
    }

    #[test]
    fn top_level_product_aliases_map_to_service_commands() {
        for (alias, expected) in [
            ("--status", "status"),
            ("--repair", "repair"),
            ("--doctor", "doctor"),
        ] {
            let arguments = vec![OsString::from("hanako-bridge.exe"), OsString::from(alias)];
            assert_eq!(
                requested_service_command(&arguments)
                    .map(|request| request.command)
                    .as_deref(),
                Some(expected)
            );
        }
    }
}
