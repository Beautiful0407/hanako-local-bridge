use std::{
    env, fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use hanako_bridge_core::{
    RuntimeConfig,
    store::write_json_atomic,
    update::{PayloadManifest, read_payload_manifest, validate_payload_manifest},
};
use hanako_maintenance::{
    PayloadTransaction, extract_zip_safely, launch_installed_manager, start_installed_service,
    stop_installed_service_and_processes, uninstall_installed_service_and_processes,
};
use mslnk::ShellLink;
use serde::Serialize;
use tempfile::TempDir;
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub ok: bool,
    pub version: String,
    pub install_root: PathBuf,
    pub config_created: bool,
    pub service_started: bool,
}

pub fn install_package(
    package_path: &Path,
    install_root: &Path,
    test_mode: bool,
) -> anyhow::Result<InstallResult> {
    install_package_with_installer(package_path, install_root, test_mode, None)
}

pub fn install_package_with_installer(
    package_path: &Path,
    install_root: &Path,
    test_mode: bool,
    installer_executable: Option<&Path>,
) -> anyhow::Result<InstallResult> {
    ensure!(package_path.is_file(), "installer payload is missing");
    let install_root = absolute(install_root)?;
    let work = tempfile::Builder::new()
        .prefix("HanakoRustInstaller-")
        .tempdir()?;
    let stage = work.path().join("payload");
    let backup = work.path().join("backup");
    fs::create_dir_all(&stage)?;
    extract_zip_safely(package_path, &stage)?;
    let payload = read_payload_manifest(&stage.join("payload-manifest.json"))?;
    validate_installer_payload(&payload, &stage)?;
    let transaction = PayloadTransaction::prepare(&install_root, &stage, &backup, payload.clone())?;
    let existing_install = install_root.join("payload-manifest.json").is_file();
    if existing_install && !test_mode {
        stop_installed_service_and_processes(&install_root)?;
    }
    fs::create_dir_all(&install_root)?;
    transaction.apply()?;
    let config_path = install_root.join("config.json");
    let config_created = !config_path.is_file();
    if config_created {
        let runtime = RuntimeConfig::load(&install_root, None)?;
        write_json_atomic(&config_path, &runtime.config)?;
    }
    if !test_mode && let Some(installer_executable) = installer_executable {
        fs::copy(
            installer_executable,
            install_root.join("HanakoLocalBridge-Setup.exe"),
        )?;
        install_shell_integration(&install_root, &payload.version)?;
    }
    let service_started = if test_mode {
        false
    } else {
        if let Err(error) = start_installed_service(&install_root) {
            transaction.rollback()?;
            anyhow::bail!("installed service failed to start: {error:#}");
        }
        if env::var_os("HANA_INSTALLER_SKIP_MANAGER").is_none() {
            launch_installed_manager(&install_root);
        }
        true
    };
    Ok(InstallResult {
        ok: true,
        version: payload.version,
        install_root,
        config_created,
        service_started,
    })
}

pub fn uninstall_installation(install_root: &Path) -> anyhow::Result<()> {
    let install_root = absolute(install_root)?;
    if install_root.is_dir() {
        uninstall_installed_service_and_processes(&install_root)?;
    }
    remove_shell_integration()?;
    if install_root.is_dir() {
        remove_dir_all_with_retry(&install_root, Duration::from_secs(15))?;
    }
    Ok(())
}

fn remove_dir_all_with_retry(path: &Path, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error.into()),
            Err(_) => thread::sleep(Duration::from_millis(250)),
        }
    }
}

pub fn write_embedded_package(bytes: &[u8]) -> anyhow::Result<(TempDir, PathBuf)> {
    ensure!(
        !bytes.is_empty(),
        "this installer build has no embedded payload"
    );
    let temp = tempfile::Builder::new()
        .prefix("HanakoEmbeddedPayload-")
        .tempdir()?;
    let path = temp.path().join("payload.zip");
    fs::write(&path, bytes)?;
    Ok((temp, path))
}

fn validate_installer_payload(payload: &PayloadManifest, stage: &Path) -> anyhow::Result<()> {
    validate_payload_manifest(payload, &payload.version)?;
    for required in [
        "hanako-bridge.exe",
        "hanako-manager.exe",
        "hanako-maintenance.exe",
        "update-public-key.xml",
        "payload-manifest.json",
    ] {
        ensure!(
            payload
                .files
                .iter()
                .any(|path| path.replace('\\', "/") == required),
            "installer payload manifest is missing {required}"
        );
        ensure!(
            stage.join(required).is_file(),
            "installer payload is missing {required}"
        );
    }
    Ok(())
}

fn install_shell_integration(install_root: &Path, version: &str) -> anyhow::Result<()> {
    let manager = install_root.join("hanako-manager.exe");
    let desktop = env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .context("USERPROFILE is missing")?
        .join("Desktop")
        .join("Hanako Local Bridge.lnk");
    let start_menu_dir = env::var_os("APPDATA")
        .map(PathBuf::from)
        .context("APPDATA is missing")?
        .join("Microsoft/Windows/Start Menu/Programs/Hanako Local Bridge");
    fs::create_dir_all(desktop.parent().context("desktop shortcut has no parent")?)?;
    fs::create_dir_all(&start_menu_dir)?;
    let mut shortcut = ShellLink::new(&manager)?;
    shortcut.set_name(Some("Hanako Local Bridge".to_string()));
    shortcut.set_icon_location(Some(manager.to_string_lossy().into_owned()));
    shortcut.create_lnk(&desktop)?;
    shortcut.create_lnk(start_menu_dir.join("Hanako Local Bridge.lnk"))?;

    let uninstall = install_root.join("HanakoLocalBridge-Setup.exe");
    let uninstall_string = format!(
        "\"{}\" --uninstall --install-root \"{}\"",
        uninstall.display(),
        install_root.display()
    );
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(uninstall_registry_key())?;
    key.set_value("DisplayName", &"Hanako Local Bridge")?;
    key.set_value("DisplayVersion", &version)?;
    key.set_value("Publisher", &"Hanako")?;
    key.set_value("InstallLocation", &install_root.to_string_lossy().as_ref())?;
    key.set_value("DisplayIcon", &manager.to_string_lossy().as_ref())?;
    key.set_value("UninstallString", &uninstall_string)?;
    key.set_value("QuietUninstallString", &uninstall_string)?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    Ok(())
}

fn remove_shell_integration() -> anyhow::Result<()> {
    if let Some(profile) = env::var_os("USERPROFILE") {
        let _ = fs::remove_file(
            PathBuf::from(profile)
                .join("Desktop")
                .join("Hanako Local Bridge.lnk"),
        );
    }
    if let Some(appdata) = env::var_os("APPDATA") {
        let start_menu = PathBuf::from(appdata)
            .join("Microsoft/Windows/Start Menu/Programs/Hanako Local Bridge");
        if start_menu.is_dir() {
            let _ = fs::remove_dir_all(start_menu);
        }
    }
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.delete_subkey_all(uninstall_registry_key()) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn uninstall_registry_key() -> String {
    std::env::var("HANA_INSTALLER_UNINSTALL_KEY").unwrap_or_else(|_| {
        "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\HanakoLocalBridge".to_string()
    })
}

fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("cannot resolve current directory")?
            .join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hanako_bridge_core::update::PayloadManifest;
    use std::io::Write as _;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn create_payload(root: &Path, version: &str) -> PathBuf {
        let package = root.join("payload.zip");
        let manifest = PayloadManifest {
            schema_version: 1,
            version: version.to_string(),
            managed_directories: vec![],
            files: vec![
                "hanako-bridge.exe".to_string(),
                "hanako-manager.exe".to_string(),
                "hanako-maintenance.exe".to_string(),
                "update-public-key.xml".to_string(),
                "payload-manifest.json".to_string(),
            ],
        };
        let file = fs::File::create(&package).unwrap();
        let mut zip = ZipWriter::new(file);
        for (name, content) in [
            ("hanako-bridge.exe", b"bridge".as_slice()),
            ("hanako-manager.exe", b"manager".as_slice()),
            ("hanako-maintenance.exe", b"maintenance".as_slice()),
            ("update-public-key.xml", b"<RSAKeyValue/>".as_slice()),
            (
                "payload-manifest.json",
                serde_json::to_vec_pretty(&manifest).unwrap().as_slice(),
            ),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
        package
    }

    #[test]
    fn installs_and_overwrites_without_touching_persistent_data() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("install");
        let package = create_payload(temp.path(), "2.0.0-alpha.2");
        let first = install_package(&package, &install, true).unwrap();
        assert!(first.ok);
        assert!(first.config_created);
        fs::create_dir_all(install.join("data")).unwrap();
        fs::create_dir_all(install.join("logs")).unwrap();
        fs::write(install.join("data/device.json"), "device").unwrap();
        fs::write(install.join("logs/bridge.log"), "log").unwrap();
        fs::write(install.join("config.json"), "{\"preserve\":true}").unwrap();
        fs::write(install.join("hanako-bridge.exe"), "old").unwrap();
        let second = install_package(&package, &install, true).unwrap();
        assert!(!second.config_created);
        assert_eq!(
            fs::read_to_string(install.join("hanako-bridge.exe")).unwrap(),
            "bridge"
        );
        assert_eq!(
            fs::read_to_string(install.join("data/device.json")).unwrap(),
            "device"
        );
        assert_eq!(
            fs::read_to_string(install.join("logs/bridge.log")).unwrap(),
            "log"
        );
        assert_eq!(
            fs::read_to_string(install.join("config.json")).unwrap(),
            "{\"preserve\":true}"
        );
    }
}
