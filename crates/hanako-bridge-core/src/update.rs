use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use quick_xml::{Reader, events::Event};
use rsa::{
    BigUint, RsaPrivateKey, RsaPublicKey,
    pkcs1v15::{Signature, SigningKey, VerifyingKey},
    signature::{SignatureEncoding as _, Signer as _, Verifier as _},
};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::store::write_json_atomic;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateManifest {
    pub schema_version: u32,
    pub channel: String,
    pub version: String,
    #[serde(default)]
    pub published_at: String,
    pub package_url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub signature_algorithm: String,
    #[serde(default)]
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadManifest {
    pub schema_version: u32,
    pub version: String,
    #[serde(default)]
    pub managed_directories: Vec<String>,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub schema_version: u32,
    pub attempt_id: String,
    pub status: String,
    pub expected_version: String,
    pub installed_version: String,
    pub message: String,
    pub log_path: PathBuf,
    pub started_at: String,
    pub finished_at: String,
    pub exit_code: i32,
}

#[derive(Default)]
struct RsaXmlComponents {
    modulus: Vec<u8>,
    exponent: Vec<u8>,
    d: Vec<u8>,
    p: Vec<u8>,
    q: Vec<u8>,
}

pub fn signature_payload(manifest: &UpdateManifest) -> String {
    [
        format!("schemaVersion={}", manifest.schema_version),
        format!("channel={}", manifest.channel),
        format!("version={}", manifest.version),
        format!("packageUrl={}", manifest.package_url),
        format!("sha256={}", manifest.sha256.trim().to_ascii_lowercase()),
        format!("size={}", manifest.size),
    ]
    .join("\n")
}

pub fn verify_manifest_signature(
    manifest: &UpdateManifest,
    public_key_xml: &str,
    required: bool,
) -> anyhow::Result<bool> {
    if manifest.signature.trim().is_empty() {
        ensure!(!required, "remote update manifest is not signed");
        return Ok(false);
    }
    ensure!(
        manifest.signature_algorithm == "RSA-SHA256",
        "unsupported update signature algorithm"
    );
    let components = parse_rsa_xml(public_key_xml)?;
    ensure!(
        !components.modulus.is_empty() && !components.exponent.is_empty(),
        "RSA public key XML is missing Modulus or Exponent"
    );
    let key = RsaPublicKey::new(
        BigUint::from_bytes_be(&components.modulus),
        BigUint::from_bytes_be(&components.exponent),
    )?;
    let signature = Signature::try_from(STANDARD.decode(manifest.signature.trim())?.as_slice())?;
    VerifyingKey::<Sha256>::new(key)
        .verify(signature_payload(manifest).as_bytes(), &signature)
        .context("update manifest signature verification failed")?;
    Ok(true)
}

pub fn sign_manifest(manifest: &mut UpdateManifest, private_key_xml: &str) -> anyhow::Result<()> {
    let components = parse_rsa_xml(private_key_xml)?;
    ensure!(
        !components.modulus.is_empty()
            && !components.exponent.is_empty()
            && !components.d.is_empty()
            && !components.p.is_empty()
            && !components.q.is_empty(),
        "RSA private key XML is missing required components"
    );
    let mut key = RsaPrivateKey::from_components(
        BigUint::from_bytes_be(&components.modulus),
        BigUint::from_bytes_be(&components.exponent),
        BigUint::from_bytes_be(&components.d),
        vec![
            BigUint::from_bytes_be(&components.p),
            BigUint::from_bytes_be(&components.q),
        ],
    )?;
    key.validate()?;
    key.precompute()?;
    manifest.signature_algorithm = "RSA-SHA256".to_string();
    let signature = SigningKey::<Sha256>::new(key).sign(signature_payload(manifest).as_bytes());
    manifest.signature = STANDARD.encode(signature.to_bytes());
    Ok(())
}

pub fn parse_version(value: &str) -> anyhow::Result<Version> {
    Version::parse(value.trim()).with_context(|| format!("invalid version: {value}"))
}

pub fn update_available(current: &str, target: &str) -> anyhow::Result<bool> {
    Ok(parse_version(target)? > parse_version(current)?)
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}

pub fn validate_payload_manifest(
    manifest: &PayloadManifest,
    expected_version: &str,
) -> anyhow::Result<()> {
    ensure!(
        manifest.schema_version == 1,
        "unsupported payload manifest schema"
    );
    ensure!(
        parse_version(&manifest.version)? == parse_version(expected_version)?,
        "payload version {} does not match expected version {expected_version}",
        manifest.version
    );
    ensure!(
        manifest.files.iter().any(|path| {
            normalize_relative_path(path).is_ok_and(|path| path == "payload-manifest.json")
        }),
        "payload manifest must include itself"
    );
    for path in &manifest.files {
        normalize_relative_path(path)?;
    }
    for path in &manifest.managed_directories {
        normalize_relative_path(path)?;
    }
    Ok(())
}

pub fn normalize_relative_path(value: &str) -> anyhow::Result<String> {
    let replaced = value.replace('\\', "/");
    let path = Path::new(&replaced);
    ensure!(
        !path.is_absolute(),
        "payload path must be relative: {value}"
    );
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                ensure!(
                    !part.is_empty() && part != "." && part != "..",
                    "invalid payload path: {value}"
                );
                parts.push(part.into_owned());
            }
            _ => anyhow::bail!("invalid payload path: {value}"),
        }
    }
    ensure!(!parts.is_empty(), "payload path is empty");
    let normalized = parts.join("/");
    ensure!(
        !is_persistent_path(&normalized),
        "payload cannot manage persistent path: {normalized}"
    );
    Ok(normalized)
}

pub fn is_persistent_path(relative: &str) -> bool {
    let normalized = relative.replace('\\', "/").to_ascii_lowercase();
    normalized == "config.json"
        || normalized == "data"
        || normalized.starts_with("data/")
        || normalized == "logs"
        || normalized.starts_with("logs/")
}

pub fn write_update_state(path: &Path, state: &UpdateState) -> anyhow::Result<()> {
    write_json_atomic(path, state).map_err(Into::into)
}

pub fn read_payload_manifest(path: &Path) -> anyhow::Result<PayloadManifest> {
    let bytes = fs::read(path)
        .with_context(|| format!("cannot read payload manifest {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot parse payload manifest {}", path.display()))
}

fn parse_rsa_xml(xml: &str) -> anyhow::Result<RsaXmlComponents> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut current = String::new();
    let mut values = BTreeMap::<String, Vec<u8>>::new();
    loop {
        match reader.read_event()? {
            Event::Start(event) => {
                current = String::from_utf8_lossy(event.name().as_ref()).into_owned();
            }
            Event::Text(text) if !current.is_empty() => {
                let decoded = text.decode()?.into_owned();
                if !decoded.trim().is_empty() {
                    values.insert(current.clone(), STANDARD.decode(decoded.trim())?);
                }
            }
            Event::End(_) => current.clear(),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(RsaXmlComponents {
        modulus: values.remove("Modulus").unwrap_or_default(),
        exponent: values.remove("Exponent").unwrap_or_default(),
        d: values.remove("D").unwrap_or_default(),
        p: values.remove("P").unwrap_or_default(),
        q: values.remove("Q").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::{
        RsaPrivateKey,
        traits::{PrivateKeyParts as _, PublicKeyParts as _},
    };

    fn xml_component(name: &str, value: &BigUint) -> String {
        format!("<{name}>{}</{name}>", STANDARD.encode(value.to_bytes_be()))
    }

    #[test]
    fn signs_and_verifies_dotnet_xml_rsa_manifests() {
        let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let public_xml = format!(
            "<RSAKeyValue>{}{}</RSAKeyValue>",
            xml_component("Modulus", key.n()),
            xml_component("Exponent", key.e())
        );
        let private_xml = format!(
            "<RSAKeyValue>{}{}{}{}{}</RSAKeyValue>",
            xml_component("Modulus", key.n()),
            xml_component("Exponent", key.e()),
            xml_component("P", &key.primes()[0]),
            xml_component("Q", &key.primes()[1]),
            xml_component("D", key.d())
        );
        let mut manifest = UpdateManifest {
            schema_version: 1,
            channel: "alpha".to_string(),
            version: "2.0.0-alpha.2".to_string(),
            published_at: String::new(),
            package_url: "https://example.invalid/update.zip".to_string(),
            sha256: "ab".repeat(32),
            size: 123,
            notes: String::new(),
            signature_algorithm: String::new(),
            signature: String::new(),
        };
        sign_manifest(&mut manifest, &private_xml).unwrap();
        assert!(verify_manifest_signature(&manifest, &public_xml, true).unwrap());
        manifest.size += 1;
        assert!(verify_manifest_signature(&manifest, &public_xml, true).is_err());
    }

    #[test]
    fn rejects_payload_paths_that_can_touch_persistent_data() {
        for path in [
            "../escape",
            "data/state.json",
            "logs/update.log",
            "config.json",
        ] {
            assert!(normalize_relative_path(path).is_err(), "{path}");
        }
        assert_eq!(
            normalize_relative_path("bin\\hanako-bridge.exe").unwrap(),
            "bin/hanako-bridge.exe"
        );
    }

    #[test]
    fn compares_semantic_prerelease_versions() {
        assert!(update_available("2.0.0-alpha.1", "2.0.0-alpha.2").unwrap());
        assert!(!update_available("2.0.0", "2.0.0-alpha.2").unwrap());
    }
}
