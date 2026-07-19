use std::{
    collections::BTreeSet,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use hanako_bridge_core::update::{
    PayloadManifest, UpdateManifest, normalize_relative_path, read_payload_manifest, sha256_file,
    sign_manifest, update_available, validate_payload_manifest, verify_manifest_signature,
};
use reqwest::blocking::Client;
use serde::Serialize;
use sysinfo::{ProcessesToUpdate, System};
use url::Url;
use walkdir::WalkDir;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub available: bool,
    pub current_version: String,
    pub target_version: String,
    pub channel: String,
    pub published_at: String,
    pub notes: String,
    pub manifest_source: String,
    pub package_source: String,
}

pub struct PreparedUpdate {
    pub manifest: UpdateManifest,
    pub package_source: String,
    pub package_path: PathBuf,
    pub stage_root: PathBuf,
    pub payload: PayloadManifest,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageBuildResult {
    pub version: String,
    pub package_path: PathBuf,
    pub manifest_path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub files: Vec<String>,
}

pub struct PayloadTransaction {
    install_root: PathBuf,
    stage_root: PathBuf,
    backup_root: PathBuf,
    new_manifest: PayloadManifest,
    old_manifest: Option<PayloadManifest>,
    touched_files: BTreeSet<String>,
    existing_files: BTreeSet<String>,
}

pub fn check_update(
    install_root: &Path,
    manifest_source: &str,
    current_version: &str,
) -> anyhow::Result<UpdateCheck> {
    let manifest = load_verified_manifest(install_root, manifest_source)?;
    let package_source = resolve_reference(manifest_source, &manifest.package_url)?;
    Ok(UpdateCheck {
        available: update_available(current_version, &manifest.version)?,
        current_version: current_version.to_string(),
        target_version: manifest.version.clone(),
        channel: manifest.channel.clone(),
        published_at: manifest.published_at.clone(),
        notes: manifest.notes.clone(),
        manifest_source: manifest_source.to_string(),
        package_source,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_release_package(
    binaries_root: &Path,
    output_root: &Path,
    version: &str,
    channel: &str,
    public_key_path: &Path,
    package_url: Option<&str>,
    signing_key_path: Option<&Path>,
    notes: &str,
) -> anyhow::Result<PackageBuildResult> {
    hanako_bridge_core::update::parse_version(version)?;
    ensure!(
        public_key_path.is_file(),
        "update public key is missing: {}",
        public_key_path.display()
    );
    fs::create_dir_all(output_root)?;
    let temp = tempfile::Builder::new()
        .prefix("HanakoRustPackage-")
        .tempdir()?;
    let stage = temp.path().join("payload");
    fs::create_dir_all(&stage)?;
    let files = vec![
        "hanako-bridge.exe".to_string(),
        "hanako-manager.exe".to_string(),
        "hanako-maintenance.exe".to_string(),
        "update-public-key.xml".to_string(),
        "payload-manifest.json".to_string(),
    ];
    for executable in [
        "hanako-bridge.exe",
        "hanako-manager.exe",
        "hanako-maintenance.exe",
    ] {
        copy_file(&binaries_root.join(executable), &stage.join(executable))?;
    }
    copy_file(public_key_path, &stage.join("update-public-key.xml"))?;
    let payload = PayloadManifest {
        schema_version: 1,
        version: version.to_string(),
        managed_directories: vec![],
        files: files.clone(),
    };
    fs::write(
        stage.join("payload-manifest.json"),
        serde_json::to_vec_pretty(&payload)?,
    )?;
    let package_path = output_root.join(format!("HanakoLocalBridge-{version}-win-x64.zip"));
    if package_path.exists() {
        fs::remove_file(&package_path)?;
    }
    let target = fs::File::create(&package_path)?;
    let mut archive = ZipWriter::new(target);
    for relative in &files {
        archive.start_file(relative, SimpleFileOptions::default())?;
        let mut source = fs::File::open(stage.join(relative_path(relative)))?;
        std::io::copy(&mut source, &mut archive)?;
    }
    archive.finish()?;
    let sha256 = sha256_file(&package_path)?;
    let size = fs::metadata(&package_path)?.len();
    let package_reference = package_url.map(ToOwned::to_owned).unwrap_or_else(|| {
        package_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    });
    let mut manifest = UpdateManifest {
        schema_version: 1,
        channel: channel.to_string(),
        version: version.to_string(),
        published_at: chrono::Utc::now().to_rfc3339(),
        package_url: package_reference,
        sha256: sha256.clone(),
        size,
        notes: notes.to_string(),
        signature_algorithm: String::new(),
        signature: String::new(),
    };
    if let Some(signing_key_path) = signing_key_path {
        sign_manifest(&mut manifest, &fs::read_to_string(signing_key_path)?)?;
    } else {
        ensure!(
            !manifest.package_url.starts_with("https://"),
            "remote release manifests require a signing key"
        );
    }
    let manifest_path = output_root.join("update-manifest.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(PackageBuildResult {
        version: version.to_string(),
        package_path,
        manifest_path,
        sha256,
        size,
        files,
    })
}

pub fn prepare_update(
    install_root: &Path,
    manifest_source: &str,
    work_root: &Path,
) -> anyhow::Result<PreparedUpdate> {
    let manifest = load_verified_manifest(install_root, manifest_source)?;
    let package_source = resolve_reference(manifest_source, &manifest.package_url)?;
    fs::create_dir_all(work_root)?;
    let package_path = work_root.join("package.zip");
    download_resource(&package_source, &package_path)?;
    let actual_size = fs::metadata(&package_path)?.len();
    ensure!(
        manifest.size == 0 || actual_size == manifest.size,
        "update package size mismatch: expected {}, got {actual_size}",
        manifest.size
    );
    let actual_hash = sha256_file(&package_path)?;
    ensure!(
        !manifest.sha256.trim().is_empty(),
        "update manifest is missing SHA256"
    );
    ensure!(
        actual_hash.eq_ignore_ascii_case(manifest.sha256.trim()),
        "update package SHA256 mismatch: expected {}, got {actual_hash}",
        manifest.sha256
    );
    let stage_root = work_root.join("payload");
    if stage_root.exists() {
        fs::remove_dir_all(&stage_root)?;
    }
    fs::create_dir_all(&stage_root)?;
    extract_zip_safely(&package_path, &stage_root)?;
    let payload = read_payload_manifest(&stage_root.join("payload-manifest.json"))?;
    validate_payload_manifest(&payload, &manifest.version)?;
    for relative in &payload.files {
        let relative = normalize_relative_path(relative)?;
        ensure!(
            stage_root.join(relative_path(&relative)).is_file(),
            "update package is missing payload file: {relative}"
        );
    }
    Ok(PreparedUpdate {
        manifest,
        package_source,
        package_path,
        stage_root,
        payload,
    })
}

impl PayloadTransaction {
    pub fn prepare(
        install_root: &Path,
        stage_root: &Path,
        backup_root: &Path,
        new_manifest: PayloadManifest,
    ) -> anyhow::Result<Self> {
        let install_root = absolute(install_root)?;
        let stage_root = absolute(stage_root)?;
        let backup_root = absolute(backup_root)?;
        ensure!(
            stage_root != install_root && backup_root != install_root,
            "stage and backup roots must be outside the install root"
        );
        validate_payload_manifest(&new_manifest, &new_manifest.version)?;
        let old_manifest_path = install_root.join("payload-manifest.json");
        let old_manifest = if old_manifest_path.is_file() {
            Some(read_payload_manifest(&old_manifest_path)?)
        } else {
            None
        };
        let new_files = manifest_files(&new_manifest)?;
        let old_files = old_manifest
            .as_ref()
            .map(manifest_files)
            .transpose()?
            .unwrap_or_default();
        let touched_files = new_files
            .union(&old_files)
            .cloned()
            .collect::<BTreeSet<_>>();
        fs::create_dir_all(&backup_root)?;
        let mut existing_files = BTreeSet::new();
        for relative in &touched_files {
            let target = install_root.join(relative_path(relative));
            if target.is_dir() {
                anyhow::bail!("managed payload file is an existing directory: {relative}");
            }
            if target.is_file() {
                copy_file(&target, &backup_root.join(relative_path(relative)))?;
                existing_files.insert(relative.clone());
            }
        }
        Ok(Self {
            install_root,
            stage_root,
            backup_root,
            new_manifest,
            old_manifest,
            touched_files,
            existing_files,
        })
    }

    pub fn apply(&self) -> anyhow::Result<()> {
        let result = self.apply_inner();
        if let Err(error) = result {
            let rollback = self.rollback();
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => {
                    anyhow::anyhow!("{error:#}; rollback also failed: {rollback_error:#}")
                }
            });
        }
        Ok(())
    }

    pub fn rollback(&self) -> anyhow::Result<()> {
        for relative in &self.touched_files {
            let target = self.install_root.join(relative_path(relative));
            if target.is_file() {
                fs::remove_file(&target)?;
            }
        }
        for relative in &self.existing_files {
            copy_file(
                &self.backup_root.join(relative_path(relative)),
                &self.install_root.join(relative_path(relative)),
            )?;
        }
        self.remove_empty_managed_directories();
        Ok(())
    }

    pub fn installed_version(&self) -> &str {
        &self.new_manifest.version
    }

    fn apply_inner(&self) -> anyhow::Result<()> {
        let new_files = manifest_files(&self.new_manifest)?;
        let old_files = self
            .old_manifest
            .as_ref()
            .map(manifest_files)
            .transpose()?
            .unwrap_or_default();
        for relative in new_files
            .iter()
            .filter(|relative| relative.as_str() != "payload-manifest.json")
        {
            copy_file_atomic(
                &self.stage_root.join(relative_path(relative)),
                &self.install_root.join(relative_path(relative)),
            )?;
        }
        for relative in old_files.difference(&new_files) {
            let target = self.install_root.join(relative_path(relative));
            if target.is_file() {
                fs::remove_file(target)?;
            }
        }
        copy_file_atomic(
            &self.stage_root.join("payload-manifest.json"),
            &self.install_root.join("payload-manifest.json"),
        )?;
        self.remove_empty_managed_directories();
        Ok(())
    }

    fn remove_empty_managed_directories(&self) {
        let mut directories = self
            .old_manifest
            .iter()
            .flat_map(|manifest| manifest.managed_directories.iter())
            .filter_map(|relative| normalize_relative_path(relative).ok())
            .map(|relative| self.install_root.join(relative_path(&relative)))
            .collect::<Vec<_>>();
        directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for directory in directories {
            let _ = remove_empty_tree(&directory);
        }
    }
}

pub fn load_verified_manifest(install_root: &Path, source: &str) -> anyhow::Result<UpdateManifest> {
    let remote = is_remote(source)?;
    let bytes = load_resource(source)?;
    let manifest: UpdateManifest =
        serde_json::from_slice(&bytes).context("cannot parse update manifest")?;
    ensure!(
        manifest.schema_version == 1,
        "unsupported update manifest schema"
    );
    let public_key_path = install_root.join("update-public-key.xml");
    if remote || !manifest.signature.trim().is_empty() {
        let public_key = fs::read_to_string(&public_key_path).with_context(|| {
            format!(
                "cannot read update public key {}",
                public_key_path.display()
            )
        })?;
        verify_manifest_signature(&manifest, &public_key, remote)?;
    }
    Ok(manifest)
}

pub fn resolve_reference(base: &str, reference: &str) -> anyhow::Result<String> {
    if reference.starts_with("http://") {
        anyhow::bail!("remote update packages must use HTTPS");
    }
    if reference.starts_with("https://") {
        return Ok(Url::parse(reference)?.to_string());
    }
    if base.starts_with("http://") {
        anyhow::bail!("remote update manifests must use HTTPS");
    }
    if base.starts_with("https://") {
        let base_url = Url::parse(base)?;
        return Ok(base_url.join(reference)?.to_string());
    }
    let reference_path = Path::new(reference);
    let path = if reference_path.is_absolute() {
        reference_path.to_path_buf()
    } else {
        Path::new(base)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(reference_path)
    };
    Ok(absolute(&path)?.to_string_lossy().into_owned())
}

pub fn download_resource(source: &str, destination: &Path) -> anyhow::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = destination.with_extension(format!("{}.tmp", std::process::id()));
    if temp.exists() {
        fs::remove_file(&temp)?;
    }
    let result = if source.starts_with("http://") {
        anyhow::bail!("remote update resources must use HTTPS");
    } else if source.starts_with("https://") {
        let url = Url::parse(source)?;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(600))
            .build()?;
        let mut response = client.get(url).send()?.error_for_status()?;
        let mut file = fs::File::create(&temp)?;
        std::io::copy(&mut response, &mut file)?;
        file.flush()
    } else {
        fs::copy(source, &temp).map(|_| ())
    };
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    ensure!(
        fs::metadata(&temp)?.len() > 0,
        "update download produced an empty file"
    );
    replace_file(&temp, destination)?;
    Ok(())
}

pub fn extract_zip_safely(package_path: &Path, destination: &Path) -> anyhow::Result<()> {
    let file = fs::File::open(package_path)?;
    let mut archive = ZipArchive::new(file)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(enclosed) = entry.enclosed_name() else {
            anyhow::bail!("ZIP entry escapes the payload root: {}", entry.name());
        };
        let output = destination.join(enclosed);
        ensure!(
            output.starts_with(destination),
            "ZIP entry escapes the payload root"
        );
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut target = fs::File::create(output)?;
        std::io::copy(&mut entry, &mut target)?;
    }
    Ok(())
}

pub fn stop_installed_service_and_processes(install_root: &Path) -> anyhow::Result<()> {
    let bridge = install_root.join("hanako-bridge.exe");
    if bridge.is_file() {
        let _ = hidden_command(&bridge)
            .args(["--service-command", "stop"])
            .status();
    }
    let install_root = absolute(install_root)?;
    let current_pid = std::process::id();
    let target_names = [
        "hanako-bridge.exe",
        "hanako-manager.exe",
        "hanako-maintenance.exe",
    ];
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut system = System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let mut found = false;
        for process in system.processes().values() {
            if process.pid().as_u32() == current_pid {
                continue;
            }
            let Some(executable) = process.exe() else {
                continue;
            };
            let Some(name) = executable.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !target_names
                .iter()
                .any(|target| name.eq_ignore_ascii_case(target))
            {
                continue;
            }
            if absolute(executable).is_ok_and(|path| path.starts_with(&install_root)) {
                found = true;
                let _ = process.kill();
            }
        }
        if !found {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "installed Hanako processes did not exit before maintenance"
        );
        thread::sleep(Duration::from_millis(300));
    }
}

pub fn uninstall_installed_service_and_processes(install_root: &Path) -> anyhow::Result<()> {
    let bridge = install_root.join("hanako-bridge.exe");
    if bridge.is_file() {
        let _ = hidden_command(&bridge)
            .args(["--service-command", "uninstall"])
            .status();
    }
    stop_installed_service_and_processes(install_root)
}

pub fn start_installed_service(install_root: &Path) -> anyhow::Result<()> {
    let bridge = install_root.join("hanako-bridge.exe");
    ensure!(bridge.is_file(), "installed bridge executable is missing");
    let status = hidden_command(&bridge)
        .args(["--service-command", "repair"])
        .status()?;
    ensure!(status.success(), "installed bridge service failed to start");
    Ok(())
}

pub fn launch_installed_manager(install_root: &Path) {
    let manager = install_root.join("hanako-manager.exe");
    if manager.is_file() {
        let _ = configure_detached(&mut Command::new(manager)).spawn();
    }
}

fn is_remote(source: &str) -> anyhow::Result<bool> {
    if source.starts_with("http://") {
        anyhow::bail!("remote update manifests must use HTTPS");
    }
    Ok(source.starts_with("https://"))
}

fn load_resource(source: &str) -> anyhow::Result<Vec<u8>> {
    if is_remote(source)? {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .build()?;
        return Ok(client
            .get(source)
            .send()?
            .error_for_status()?
            .bytes()?
            .to_vec());
    }
    fs::read(source).with_context(|| format!("cannot read update resource {source}"))
}

fn manifest_files(manifest: &PayloadManifest) -> anyhow::Result<BTreeSet<String>> {
    manifest
        .files
        .iter()
        .map(|relative| normalize_relative_path(relative))
        .collect()
}

fn relative_path(relative: &str) -> PathBuf {
    relative.split('/').collect()
}

fn copy_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    ensure!(
        source.is_file(),
        "source file is missing: {}",
        source.display()
    );
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    Ok(())
}

fn copy_file_atomic(source: &Path, destination: &Path) -> anyhow::Result<()> {
    ensure!(
        source.is_file(),
        "source file is missing: {}",
        source.display()
    );
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = destination.with_extension(format!(
        "{}.{}.tmp",
        destination
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or(""),
        std::process::id()
    ));
    fs::copy(source, &temp)?;
    replace_file(&temp, destination)?;
    Ok(())
}

fn replace_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(source, destination)?;
    Ok(())
}

fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
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

fn hidden_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn remove_empty_tree(root: &Path) -> anyhow::Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut directories = WalkDir::new(root)
        .min_depth(0)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_dir())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        if fs::read_dir(&directory)?.next().is_none() {
            fs::remove_dir(directory)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn write_json(path: &Path, value: serde_json::Value) {
        fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }

    fn payload(version: &str, files: &[&str], managed: &[&str]) -> PayloadManifest {
        PayloadManifest {
            schema_version: 1,
            version: version.to_string(),
            managed_directories: managed.iter().map(|value| (*value).to_string()).collect(),
            files: files.iter().map(|value| (*value).to_string()).collect(),
        }
    }

    #[test]
    fn payload_transaction_preserves_data_cleans_stale_files_and_rolls_back() {
        let temp = tempdir().unwrap();
        let install = temp.path().join("install");
        let stage = temp.path().join("stage");
        let backup = temp.path().join("backup");
        fs::create_dir_all(install.join("legacy")).unwrap();
        fs::create_dir_all(install.join("data")).unwrap();
        fs::create_dir_all(install.join("custom-root")).unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::write(install.join("legacy/old.dll"), "old").unwrap();
        fs::write(install.join("hanako-bridge.exe"), "old bridge").unwrap();
        fs::write(install.join("data/device.json"), "device").unwrap();
        fs::write(install.join("custom-root/user.txt"), "user").unwrap();
        let old = payload(
            "2.0.0-alpha.1",
            &[
                "hanako-bridge.exe",
                "legacy/old.dll",
                "payload-manifest.json",
            ],
            &["legacy"],
        );
        write_json(
            &install.join("payload-manifest.json"),
            serde_json::to_value(&old).unwrap(),
        );
        fs::write(stage.join("hanako-bridge.exe"), "new bridge").unwrap();
        fs::write(stage.join("hanako-manager.exe"), "manager").unwrap();
        let new = payload(
            "2.0.0-alpha.2",
            &[
                "hanako-bridge.exe",
                "hanako-manager.exe",
                "payload-manifest.json",
            ],
            &[],
        );
        write_json(
            &stage.join("payload-manifest.json"),
            serde_json::to_value(&new).unwrap(),
        );

        let transaction = PayloadTransaction::prepare(&install, &stage, &backup, new).unwrap();
        transaction.apply().unwrap();
        assert_eq!(
            fs::read_to_string(install.join("hanako-bridge.exe")).unwrap(),
            "new bridge"
        );
        assert!(install.join("hanako-manager.exe").is_file());
        assert!(!install.join("legacy/old.dll").exists());
        assert_eq!(
            fs::read_to_string(install.join("data/device.json")).unwrap(),
            "device"
        );
        assert_eq!(
            fs::read_to_string(install.join("custom-root/user.txt")).unwrap(),
            "user"
        );

        transaction.rollback().unwrap();
        assert_eq!(
            fs::read_to_string(install.join("hanako-bridge.exe")).unwrap(),
            "old bridge"
        );
        assert!(install.join("legacy/old.dll").is_file());
        assert!(!install.join("hanako-manager.exe").exists());
        assert_eq!(
            fs::read_to_string(install.join("data/device.json")).unwrap(),
            "device"
        );
    }

    #[test]
    fn safe_zip_extraction_rejects_parent_traversal() {
        let temp = tempdir().unwrap();
        let package = temp.path().join("bad.zip");
        let file = fs::File::create(&package).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("../escape.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"bad").unwrap();
        writer.finish().unwrap();
        let destination = temp.path().join("stage");
        fs::create_dir_all(&destination).unwrap();
        assert!(extract_zip_safely(&package, &destination).is_err());
        assert!(!temp.path().join("escape.txt").exists());
    }

    #[test]
    fn resolves_relative_local_package_references() {
        let temp = tempdir().unwrap();
        let manifest = temp.path().join("release/update-manifest.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let resolved = resolve_reference(manifest.to_str().unwrap(), "package.zip").unwrap();
        assert_eq!(
            PathBuf::from(resolved),
            temp.path().join("release/package.zip")
        );
    }

    #[test]
    fn serialized_check_shape_stays_manager_friendly() {
        let value = serde_json::to_value(UpdateCheck {
            available: true,
            current_version: "1.0.0".to_string(),
            target_version: "2.0.0".to_string(),
            channel: "stable".to_string(),
            published_at: String::new(),
            notes: String::new(),
            manifest_source: "manifest.json".to_string(),
            package_source: "package.zip".to_string(),
        })
        .unwrap();
        assert_eq!(value["targetVersion"], json!("2.0.0"));
    }

    #[test]
    fn release_pack_contains_only_rust_runtime_files() {
        let temp = tempdir().unwrap();
        let binaries = temp.path().join("binaries");
        let output = temp.path().join("release");
        fs::create_dir_all(&binaries).unwrap();
        for name in [
            "hanako-bridge.exe",
            "hanako-manager.exe",
            "hanako-maintenance.exe",
        ] {
            fs::write(binaries.join(name), name).unwrap();
        }
        let public_key = temp.path().join("update-public-key.xml");
        fs::write(&public_key, "<RSAKeyValue/>").unwrap();
        let result = build_release_package(
            &binaries,
            &output,
            "2.0.0-alpha.2",
            "alpha",
            &public_key,
            None,
            None,
            "test",
        )
        .unwrap();
        assert_eq!(result.files.len(), 5);
        let file = fs::File::open(result.package_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([
                "hanako-bridge.exe".to_string(),
                "hanako-manager.exe".to_string(),
                "hanako-maintenance.exe".to_string(),
                "payload-manifest.json".to_string(),
                "update-public-key.xml".to_string(),
            ])
        );
    }
}
