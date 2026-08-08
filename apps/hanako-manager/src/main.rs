#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env,
    io::{Read as _, Write as _},
    net::{Ipv4Addr, SocketAddrV4, TcpStream, UdpSocket},
    os::windows::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use hanako_bridge_core::RuntimeConfig;
use tray_icon::{
    TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Icon as WindowIcon, Window, WindowId},
};
use wry::{WebView, WebViewBuilder};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ACTIVATION_REQUEST: &[u8] = b"HANAKO_LOCAL_BRIDGE_MANAGER_SHOW_V1";
const ACTIVATION_ACK: &[u8] = b"HANAKO_LOCAL_BRIDGE_MANAGER_ACK_V1";
const ACTIVATION_PORT_BASE: u16 = 42_000;
const ACTIVATION_PORT_SPAN: u16 = 2_000;

enum ActivationRole {
    Primary(ManagerActivation),
    ActivatedExisting,
}

struct ManagerActivation {
    requested: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ManagerActivation {
    fn acquire(install_dir: &Path) -> anyhow::Result<ActivationRole> {
        let address = activation_address(install_dir);
        match UdpSocket::bind(address) {
            Ok(socket) => {
                socket.set_read_timeout(Some(Duration::from_millis(200)))?;
                let requested = Arc::new(AtomicBool::new(false));
                let stop = Arc::new(AtomicBool::new(false));
                let worker_requested = Arc::clone(&requested);
                let worker_stop = Arc::clone(&stop);
                let worker = thread::spawn(move || {
                    let mut buffer = [0u8; 128];
                    while !worker_stop.load(Ordering::Relaxed) {
                        match socket.recv_from(&mut buffer) {
                            Ok((length, peer)) if &buffer[..length] == ACTIVATION_REQUEST => {
                                worker_requested.store(true, Ordering::Release);
                                let _ = socket.send_to(ACTIVATION_ACK, peer);
                            }
                            Ok(_) => {}
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) => {}
                            Err(_) => break,
                        }
                    }
                });
                Ok(ActivationRole::Primary(Self {
                    requested,
                    stop,
                    worker: Some(worker),
                }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
                socket.set_read_timeout(Some(Duration::from_secs(2)))?;
                socket.send_to(ACTIVATION_REQUEST, address)?;
                let mut buffer = [0u8; 128];
                let (length, peer) = socket.recv_from(&mut buffer)?;
                ensure!(
                    peer == address.into() && &buffer[..length] == ACTIVATION_ACK,
                    "existing manager did not acknowledge activation"
                );
                Ok(ActivationRole::ActivatedExisting)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn take_request(&self) -> bool {
        self.requested.swap(false, Ordering::AcqRel)
    }
}

impl Drop for ManagerActivation {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct ManagerApp {
    url: String,
    activation: ManagerActivation,
    window: Option<Window>,
    webview: Option<WebView>,
    tray: Option<TrayIcon>,
    open_item: MenuItem,
    exit_item: MenuItem,
    exiting: bool,
}

impl ManagerApp {
    fn new(url: String, activation: ManagerActivation) -> anyhow::Result<Self> {
        let open_item = MenuItem::new("打开管理器", true, None);
        let exit_item = MenuItem::new("退出管理器", true, None);
        Ok(Self {
            url,
            activation,
            window: None,
            webview: None,
            tray: None,
            open_item,
            exit_item,
            exiting: false,
        })
    }

    fn show(&self) {
        if let Some(window) = &self.window {
            window.set_visible(true);
            window.set_minimized(false);
            window.focus_window();
        }
    }

    fn hide(&self) {
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        let rgba = app_icon_rgba(32);
        let window_icon = WindowIcon::from_rgba(rgba.clone(), 32, 32)?;
        let window = event_loop.create_window(
            Window::default_attributes()
                .with_title("Hanako Local Bridge")
                .with_inner_size(LogicalSize::new(980.0, 700.0))
                .with_min_inner_size(LogicalSize::new(760.0, 560.0))
                .with_window_icon(Some(window_icon)),
        )?;
        let webview = WebViewBuilder::new()
            .with_url(&self.url)
            .with_devtools(cfg!(debug_assertions))
            .build(&window)?;

        let menu = Menu::new();
        menu.append(&self.open_item)?;
        menu.append(&tray_icon::menu::PredefinedMenuItem::separator())?;
        menu.append(&self.exit_item)?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("Hanako Local Bridge")
            .with_menu(Box::new(menu))
            .with_icon(tray_icon::Icon::from_rgba(rgba, 32, 32)?)
            .build()?;
        self.window = Some(window);
        self.webview = Some(webview);
        self.tray = Some(tray);
        Ok(())
    }

    fn process_tray_events(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.open_item.id() {
                self.show();
            } else if event.id == self.exit_item.id() {
                self.exiting = true;
                event_loop.exit();
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                self.show();
            }
        }
    }
}

impl ApplicationHandler for ManagerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(error) = self.create_window(event_loop)
        {
            show_error(&format!("无法打开 Hanako 管理器：{error}"));
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested if !self.exiting => self.hide(),
            WindowEvent::Resized(PhysicalSize {
                width: 0,
                height: 0,
            }) => self.hide(),
            WindowEvent::Destroyed => event_loop.exit(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.activation.take_request() {
            self.show();
        }
        self.process_tray_events(event_loop);
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(200),
        ));
    }
}

fn main() -> anyhow::Result<()> {
    let install_dir = env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .context("cannot resolve manager directory")?;
    let runtime = RuntimeConfig::load(&install_dir, None)?;
    let bridge_exe = sibling_bridge(&install_dir);
    if env::args().any(|argument| argument == "--smoke-test") {
        anyhow::ensure!(bridge_exe.is_file(), "Rust bridge executable is missing");
        return Ok(());
    }
    let activation = match ManagerActivation::acquire(&install_dir)? {
        ActivationRole::Primary(activation) => activation,
        ActivationRole::ActivatedExisting => return Ok(()),
    };
    if let Err(error) = ensure_service(&bridge_exe, runtime.config.filesystem.approval_port) {
        show_error(&format!(
            "无法连接当前 Rust 服务：{error}\n\n请重新运行安装器执行覆盖修复。"
        ));
        return Ok(());
    }
    let url = env::args()
        .skip_while(|argument| argument != "--url")
        .nth(1)
        .unwrap_or_else(|| {
            format!(
                "http://127.0.0.1:{}/manager/",
                runtime.config.filesystem.approval_port
            )
        });
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = ManagerApp::new(url, activation)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn sibling_bridge(install_dir: &Path) -> PathBuf {
    let installed = install_dir.join("hanako-bridge.exe");
    if installed.is_file() {
        return installed;
    }
    install_dir.join("hanako-bridge")
}

fn ensure_service(bridge_exe: &Path, port: u16) -> anyhow::Result<()> {
    if health_ok(port) {
        return Ok(());
    }
    for action in ["start", "repair"] {
        let _ = Command::new(bridge_exe)
            .args(["--service-command", action])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if health_ok(port) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    anyhow::bail!(
        "端口 {port} 上没有运行 Hanako Rust {} 服务，可能仍被旧版服务占用",
        env!("CARGO_PKG_VERSION")
    )
}

fn health_ok(port: u16) -> bool {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let Ok(mut stream) = TcpStream::connect_timeout(&address.into(), Duration::from_millis(500))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let request =
        format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::new();
    if stream.read_to_end(&mut response).is_err() {
        return false;
    }
    let Some(body_offset) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&response[body_offset + 4..])
    else {
        return false;
    };
    value["ok"] == true
        && value["runtime"] == "rust"
        && value["version"] == env!("CARGO_PKG_VERSION")
}

fn activation_address(install_dir: &Path) -> SocketAddrV4 {
    let normalized = install_dir
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in normalized.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    SocketAddrV4::new(
        Ipv4Addr::LOCALHOST,
        ACTIVATION_PORT_BASE + (hash % u64::from(ACTIVATION_PORT_SPAN)) as u16,
    )
}

fn app_icon_rgba(size: u32) -> Vec<u8> {
    // Prefer the branded 32x32 icon (embedded RGBA generated from
    // assets/hanako-local-bridge-512.png). Fall back to the legacy procedural
    // “white H” glyph if the resource is missing.
    if size == 32 {
        const BRANDED: &[u8] = include_bytes!("../../../assets/app-icon-32.rgba");
        if BRANDED.len() == 32 * 32 * 4 {
            return BRANDED.to_vec();
        }
    }
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let index = ((y * size + x) * 4) as usize;
            let inset = size / 8;
            let inside = x >= inset && y >= inset && x < size - inset && y < size - inset;
            let h = (x >= size / 3 && x < size / 3 + 3)
                || (x >= size * 2 / 3 - 2 && x < size * 2 / 3 + 1)
                || (y >= size / 2 - 1 && y < size / 2 + 2 && x >= size / 3 && x < size * 2 / 3);
            let (red, green, blue, alpha) = if inside && h {
                (255, 255, 255, 255)
            } else if inside {
                (23, 105, 170, 255)
            } else {
                (0, 0, 0, 0)
            };
            pixels[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    pixels
}

fn show_error(message: &str) {
    let escaped = message.replace('\'', "''");
    let _ = Command::new("mshta.exe")
        .arg(format!(
            "javascript:alert('{}');close();",
            escaped.replace('\n', "\\n")
        ))
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn serve_health(body: String) -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 512];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        port
    }

    #[test]
    fn rejects_legacy_health_response_without_rust_identity() {
        let port =
            serve_health(r#"{"ok":true,"trustMode":"full","approvalRequired":false}"#.to_string());
        assert!(!health_ok(port));
    }

    #[test]
    fn accepts_matching_rust_health_response() {
        let body = serde_json::json!({
            "ok": true,
            "runtime": "rust",
            "version": env!("CARGO_PKG_VERSION")
        })
        .to_string();
        let port = serve_health(body);
        assert!(health_ok(port));
    }

    #[test]
    fn second_instance_activates_the_primary_manager() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let install_dir = env::temp_dir().join(format!(
            "hanako-manager-instance-{}-{unique}",
            std::process::id()
        ));
        let primary = match ManagerActivation::acquire(&install_dir).unwrap() {
            ActivationRole::Primary(primary) => primary,
            ActivationRole::ActivatedExisting => panic!("test manager unexpectedly already exists"),
        };
        assert!(matches!(
            ManagerActivation::acquire(&install_dir).unwrap(),
            ActivationRole::ActivatedExisting
        ));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if primary.take_request() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("primary manager did not receive the activation request");
    }
}
