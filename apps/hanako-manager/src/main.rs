#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env,
    os::windows::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::Context;
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

struct ManagerApp {
    url: String,
    window: Option<Window>,
    webview: Option<WebView>,
    tray: Option<TrayIcon>,
    open_item: MenuItem,
    exit_item: MenuItem,
    exiting: bool,
}

impl ManagerApp {
    fn new(url: String) -> anyhow::Result<Self> {
        let open_item = MenuItem::new("打开管理器", true, None);
        let exit_item = MenuItem::new("退出管理器", true, None);
        Ok(Self {
            url,
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
    ensure_service(&bridge_exe, runtime.config.filesystem.approval_port);
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
    let mut app = ManagerApp::new(url)?;
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

fn ensure_service(bridge_exe: &Path, port: u16) {
    if health_ok(port) {
        return;
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
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

fn health_ok(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}")
            .parse()
            .expect("valid loopback address"),
        Duration::from_millis(350),
    )
    .is_ok()
}

fn app_icon_rgba(size: u32) -> Vec<u8> {
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
