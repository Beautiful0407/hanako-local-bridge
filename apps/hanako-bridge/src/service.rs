use std::{
    env, fs,
    os::windows::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use hanako_bridge_core::RuntimeConfig;
use serde::Serialize;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

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
    let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--service-command")
    else {
        return Ok(false);
    };
    let command = arguments
        .get(index + 1)
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let install_dir = env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("cannot resolve bridge install directory"))?;
    let runtime = RuntimeConfig::load(&install_dir, None)?;
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
            let wait_pid = arguments
                .get(index + 2)
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<u32>().ok());
            if let Some(pid) = wait_pid {
                wait_for_process_exit(pid, Duration::from_secs(30)).await;
            }
            let _ = stop_task(&task_name);
            tokio::time::sleep(Duration::from_millis(600)).await;
            start_task(&task_name)?;
        }
        "deferred-action" => {
            let action = arguments
                .get(index + 2)
                .and_then(|value| value.to_str())
                .unwrap_or("");
            tokio::time::sleep(Duration::from_millis(800)).await;
            match action {
                "stop" => stop_task(&task_name)?,
                "restart" => {
                    let _ = stop_task(&task_name);
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    start_task(&task_name)?;
                }
                "repair" => {
                    install_task(&runtime, &task_name)?;
                    start_task(&task_name)?;
                }
                _ => anyhow::bail!("unknown deferred service action: {action}"),
            }
        }
        "status" => print_json(&service_status(&runtime, &task_name).await?)?,
        _ => anyhow::bail!("unknown service command: {command}"),
    }
    Ok(true)
}

pub fn spawn_deferred_action(runtime: &RuntimeConfig, action: &str) -> anyhow::Result<()> {
    if !matches!(action, "stop" | "restart" | "repair") {
        anyhow::bail!("unsupported deferred service action: {action}");
    }
    let executable = env::current_exe()?;
    Command::new(executable)
        .args(["--service-command", "deferred-action", action])
        .env("HANA_LOCAL_BRIDGE_CONFIG", &runtime.config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(
            DETACHED_PROCESS
                | CREATE_NEW_PROCESS_GROUP
                | CREATE_NO_WINDOW
                | CREATE_BREAKAWAY_FROM_JOB,
        )
        .spawn()?;
    Ok(())
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
    fs::write(&task_file, xml.as_bytes())?;
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

fn start_task(task_name: &str) -> anyhow::Result<()> {
    run_schtasks(["/Run", "/TN", task_name])
}

fn stop_task(task_name: &str) -> anyhow::Result<()> {
    run_schtasks(["/End", "/TN", task_name])
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
    let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let output_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
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
    let executable = env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .args([
            "--service-command",
            "restart-worker",
            &std::process::id().to_string(),
        ])
        .env("HANA_LOCAL_BRIDGE_CONFIG", &runtime.config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(
            DETACHED_PROCESS
                | CREATE_NEW_PROCESS_GROUP
                | CREATE_NO_WINDOW
                | CREATE_BREAKAWAY_FROM_JOB,
        );
    command.spawn()?;
    let _ = task_name;
    Ok(())
}

fn current_user() -> anyhow::Result<String> {
    let output = Command::new("whoami.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("cannot resolve the current Windows user");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
    _config_path: &Path,
    restart_delay_seconds: u64,
) -> String {
    let description = format!("Hanako Local Bridge Rust service: {task_name}");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{}</UserId>
    </LogonTrigger>
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
      <Interval>PT{}S</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>--service</Arguments>
      <WorkingDirectory>{}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        xml_escape(&description),
        xml_escape(user),
        xml_escape(user),
        restart_delay_seconds.max(1),
        xml_escape(executable.to_string_lossy().as_ref()),
        xml_escape(
            executable
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_string_lossy()
                .as_ref()
        ),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!xml.contains("powershell"));
        assert!(!xml.contains("wscript"));
        assert!(!xml.contains("node.exe"));
    }
}
