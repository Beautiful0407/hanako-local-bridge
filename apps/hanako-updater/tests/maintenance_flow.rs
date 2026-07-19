use std::{
    fs,
    io::Write as _,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hanako_bridge_core::update::{
    PayloadManifest, UpdateManifest, UpdateState, sha256_file, sign_manifest,
};
use rsa::{
    BigUint, RsaPrivateKey,
    traits::{PrivateKeyParts as _, PublicKeyParts as _},
};
use tempfile::tempdir;
use zip::{ZipWriter, write::SimpleFileOptions};

fn xml_component(name: &str, value: &BigUint) -> String {
    format!("<{name}>{}</{name}>", STANDARD.encode(value.to_bytes_be()))
}

fn key_xml() -> (String, String) {
    let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
    let public = format!(
        "<RSAKeyValue>{}{}</RSAKeyValue>",
        xml_component("Modulus", key.n()),
        xml_component("Exponent", key.e())
    );
    let private = format!(
        "<RSAKeyValue>{}{}{}{}{}</RSAKeyValue>",
        xml_component("Modulus", key.n()),
        xml_component("Exponent", key.e()),
        xml_component("P", &key.primes()[0]),
        xml_component("Q", &key.primes()[1]),
        xml_component("D", key.d())
    );
    (public, private)
}

fn write_payload_manifest(path: &Path, version: &str, files: &[&str], dirs: &[&str]) {
    let manifest = PayloadManifest {
        schema_version: 1,
        version: version.to_string(),
        managed_directories: dirs.iter().map(|value| (*value).to_string()).collect(),
        files: files.iter().map(|value| (*value).to_string()).collect(),
    };
    fs::write(path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
}

fn create_package(stage: &Path, package: &Path) {
    let file = fs::File::create(package).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for relative in [
        "hanako-bridge.exe",
        "hanako-manager.exe",
        "hanako-maintenance.exe",
        "update-public-key.xml",
        "payload-manifest.json",
    ] {
        zip.start_file(relative, options).unwrap();
        zip.write_all(&fs::read(stage.join(relative)).unwrap())
            .unwrap();
    }
    zip.finish().unwrap();
}

fn wait_for_final_state(path: &Path) -> UpdateState {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(bytes) = fs::read(path)
            && let Ok(state) = serde_json::from_slice::<UpdateState>(&bytes)
            && matches!(state.status.as_str(), "succeeded" | "failed")
        {
            return state;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {}", path.display());
}

#[test]
fn signed_detached_update_preserves_persistent_data() {
    let temp = tempdir().unwrap();
    let install = temp.path().join("install");
    let release = temp.path().join("release");
    let stage = temp.path().join("stage");
    fs::create_dir_all(install.join("data")).unwrap();
    fs::create_dir_all(install.join("logs")).unwrap();
    fs::create_dir_all(install.join("legacy")).unwrap();
    fs::create_dir_all(&release).unwrap();
    fs::create_dir_all(&stage).unwrap();
    fs::write(install.join("hanako-bridge.exe"), "old bridge").unwrap();
    fs::write(install.join("legacy/old.dll"), "stale").unwrap();
    fs::write(install.join("config.json"), "{\"keep\":true}").unwrap();
    fs::write(install.join("data/device.json"), "{\"id\":\"keep\"}").unwrap();
    fs::write(install.join("logs/old.log"), "keep log").unwrap();
    write_payload_manifest(
        &install.join("payload-manifest.json"),
        "2.0.0-alpha.1",
        &[
            "hanako-bridge.exe",
            "legacy/old.dll",
            "payload-manifest.json",
        ],
        &["legacy"],
    );

    let (public_key, private_key) = key_xml();
    fs::write(install.join("update-public-key.xml"), &public_key).unwrap();
    fs::write(stage.join("hanako-bridge.exe"), "new bridge").unwrap();
    fs::write(stage.join("hanako-manager.exe"), "new manager").unwrap();
    fs::write(stage.join("hanako-maintenance.exe"), "new maintenance").unwrap();
    fs::write(stage.join("update-public-key.xml"), &public_key).unwrap();
    write_payload_manifest(
        &stage.join("payload-manifest.json"),
        "2.0.0-alpha.2",
        &[
            "hanako-bridge.exe",
            "hanako-manager.exe",
            "hanako-maintenance.exe",
            "update-public-key.xml",
            "payload-manifest.json",
        ],
        &[],
    );
    let package = release.join("HanakoLocalBridge-2.0.0-alpha.2-win-x64.zip");
    create_package(&stage, &package);
    let mut manifest = UpdateManifest {
        schema_version: 1,
        channel: "alpha".to_string(),
        version: "2.0.0-alpha.2".to_string(),
        published_at: "2026-07-19T00:00:00Z".to_string(),
        package_url: package.file_name().unwrap().to_string_lossy().into_owned(),
        sha256: sha256_file(&package).unwrap(),
        size: fs::metadata(&package).unwrap().len(),
        notes: "Rust updater integration".to_string(),
        signature_algorithm: String::new(),
        signature: String::new(),
    };
    sign_manifest(&mut manifest, &private_key).unwrap();
    let manifest_path = release.join("update-manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let executable = env!("CARGO_BIN_EXE_hanako-maintenance");
    let check = Command::new(executable)
        .args([
            "check",
            "--install-root",
            install.to_str().unwrap(),
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--current-version",
            "2.0.0-alpha.1",
        ])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let check_json: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(check_json["available"], true);
    assert_eq!(check_json["targetVersion"], "2.0.0-alpha.2");

    let state_path = install.join("data/update-state.json");
    let launch = Command::new(executable)
        .args([
            "apply",
            "--install-root",
            install.to_str().unwrap(),
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--expected-version",
            "2.0.0-alpha.2",
            "--state-path",
            state_path.to_str().unwrap(),
            "--test-mode",
        ])
        .output()
        .unwrap();
    assert!(
        launch.status.success(),
        "{}",
        String::from_utf8_lossy(&launch.stderr)
    );
    let handoff: serde_json::Value = serde_json::from_slice(&launch.stdout).unwrap();
    assert_eq!(handoff["started"], true);
    let state = wait_for_final_state(&state_path);
    assert_eq!(state.status, "succeeded", "{}", state.message);
    assert_eq!(state.installed_version, "2.0.0-alpha.2");
    assert_eq!(
        fs::read_to_string(install.join("hanako-bridge.exe")).unwrap(),
        "new bridge"
    );
    assert!(install.join("hanako-manager.exe").is_file());
    assert!(!install.join("legacy/old.dll").exists());
    assert_eq!(
        fs::read_to_string(install.join("config.json")).unwrap(),
        "{\"keep\":true}"
    );
    assert_eq!(
        fs::read_to_string(install.join("data/device.json")).unwrap(),
        "{\"id\":\"keep\"}"
    );
    assert_eq!(
        fs::read_to_string(install.join("logs/old.log")).unwrap(),
        "keep log"
    );
}
