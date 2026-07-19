use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use chrono::Utc;
use rand::{Rng, distr::Alphanumeric};
use serde::{Serialize, de::DeserializeOwned};

use crate::{BridgeError, BridgeResult};

fn random_suffix() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect()
}

fn corrupt_path(path: &Path) -> PathBuf {
    let stamp = Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ");
    PathBuf::from(format!(
        "{}.corrupt-{stamp}-{}",
        path.display(),
        random_suffix()
    ))
}

pub fn write_json_file<T: Serialize>(path: &Path, value: &T) -> BridgeResult<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| BridgeError::Write {
        path: parent.display().to_string(),
        source,
    })?;
    let temp = PathBuf::from(format!(
        "{}.{}.{}.tmp",
        path.display(),
        std::process::id(),
        random_suffix()
    ));
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| BridgeError::Json {
        path: path.display().to_string(),
        source,
    })?;
    let result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if let Err(source) = result {
        let _ = fs::remove_file(&temp);
        return Err(BridgeError::Write {
            path: path.display().to_string(),
            source,
        });
    }
    Ok(())
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> BridgeResult<()> {
    if path.exists() {
        let backup = PathBuf::from(format!("{}.bak", path.display()));
        let _ = fs::copy(path, backup);
    }
    write_json_file(path, value)
}

pub fn load_json<T>(path: &Path, fallback: impl FnOnce() -> T) -> BridgeResult<T>
where
    T: Serialize + DeserializeOwned + Clone,
{
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => return Ok(value),
            Err(_) => {
                let _ = fs::rename(path, corrupt_path(path));
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(BridgeError::Read {
                path: path.display().to_string(),
                source,
            });
        }
    }

    let backup = PathBuf::from(format!("{}.bak", path.display()));
    match fs::read(&backup) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => {
                write_json_file(path, &value)?;
                return Ok(value);
            }
            Err(_) => {
                let _ = fs::rename(&backup, corrupt_path(&backup));
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(BridgeError::Read {
                path: backup.display().to_string(),
                source,
            });
        }
    }

    Ok(fallback())
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

    use super::*;

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct State {
        value: u32,
    }

    #[test]
    fn recovers_from_backup_and_preserves_corrupt_primary() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        write_json_atomic(&path, &State { value: 7 }).unwrap();
        write_json_atomic(&path, &State { value: 9 }).unwrap();
        fs::write(&path, b"{broken").unwrap();

        let recovered = load_json(&path, || State { value: 0 }).unwrap();

        assert_eq!(recovered, State { value: 7 });
        assert!(
            fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
        );
    }
}
