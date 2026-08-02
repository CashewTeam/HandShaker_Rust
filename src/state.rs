use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::i18n;
use base64::Engine as _;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TrustRecord {
    pub device_name: Option<String>,
    /// base64-encoded 256-byte derived key echoed by the phone on TRUST_ALWAYS.
    pub derived_key: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    pub schema_version: u32,
    pub host_uuid: Uuid,
    #[serde(default)]
    pub trust: BTreeMap<String, TrustRecord>,
}

#[derive(Clone)]
pub(crate) struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn discover() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "handshaker").ok_or_else(|| {
            Error::Configuration(i18n::text("state.config_dir_missing").to_string())
        })?;
        Ok(Self {
            path: dirs.config_dir().join("state.json"),
        })
    }

    #[cfg(test)]
    pub(crate) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load_or_create(&self) -> Result<State> {
        if self.path.exists() {
            set_path_permissions(&self.path)?;
            let bytes = fs::read(&self.path).map_err(|error| {
                Error::Configuration(i18n::format(
                    "state.read_failed",
                    &[&self.path.display().to_string(), &error.to_string()],
                ))
            })?;
            let state: State = serde_json::from_slice(&bytes).map_err(|error| {
                Error::Configuration(i18n::format(
                    "state.parse_failed",
                    &[&self.path.display().to_string(), &error.to_string()],
                ))
            })?;
            if state.schema_version != 1 {
                return Err(Error::Configuration(i18n::format(
                    "state.version_unsupported",
                    &[&state.schema_version.to_string()],
                )));
            }
            return Ok(state);
        }

        let state = State {
            schema_version: 1,
            host_uuid: Uuid::new_v4(),
            trust: BTreeMap::new(),
        };
        self.save(&state)?;
        Ok(state)
    }

    /// Persist (or refresh) the trust record for a WiFi device after a
    /// successful TRUST_ALWAYS handshake.
    pub(crate) fn upsert_trust(
        &self,
        device_uuid: &str,
        device_name: Option<&str>,
        derived_key: &[u8],
    ) -> Result<()> {
        let mut state = self.load_or_create()?;
        state.trust.insert(
            device_uuid.to_string(),
            TrustRecord {
                device_name: device_name.map(str::to_string),
                derived_key: base64::engine::general_purpose::STANDARD.encode(derived_key),
                updated_at: unix_seconds(),
            },
        );
        self.save(&state)
    }

    /// Remove the local trust record for a device; returns whether one existed.
    pub(crate) fn remove_trust(&self, device_uuid: &str) -> Result<bool> {
        let mut state = self.load_or_create()?;
        let removed = state.trust.remove(device_uuid).is_some();
        if removed {
            self.save(&state)?;
        }
        Ok(removed)
    }

    fn save(&self, state: &State) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Configuration(i18n::text("state.parent_missing").to_string()))?;
        fs::create_dir_all(parent).map_err(|error| {
            Error::Configuration(i18n::format(
                "state.create_failed",
                &[&parent.display().to_string(), &error.to_string()],
            ))
        })?;
        set_directory_permissions(parent)?;

        let bytes = serde_json::to_vec_pretty(state).map_err(|error| {
            Error::Configuration(i18n::format(
                "state.serialize_failed",
                &[&error.to_string()],
            ))
        })?;
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        set_file_mode(&mut options);
        let mut file = options.open(&self.path).map_err(|error| {
            Error::Configuration(i18n::format(
                "state.write_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        file.write_all(&bytes).map_err(|error| {
            Error::Configuration(i18n::format(
                "state.write_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        file.sync_all().map_err(|error| {
            Error::Configuration(i18n::format(
                "state.sync_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        set_path_permissions(&self.path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn set_file_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_file_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        Error::Configuration(i18n::format(
            "state.permission_failed",
            &[&path.display().to_string(), &error.to_string()],
        ))
    })
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_path_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        Error::Configuration(i18n::format(
            "state.permission_failed",
            &[&path.display().to_string(), &error.to_string()],
        ))
    })
}

#[cfg(not(unix))]
fn set_path_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, StateStore) {
        let temp = tempfile::tempdir().expect("temporary directory");
        let store = StateStore::at(temp.path().join("state.json"));
        (temp, store)
    }

    #[test]
    fn trust_record_round_trips_through_upsert() {
        let (_temp, store) = temp_store();
        store
            .upsert_trust("device-1", Some("Phone"), &[0x42_u8; 256])
            .expect("upsert");
        let state = store.load_or_create().expect("load");
        let record = state.trust.get("device-1").expect("trust record present");
        assert_eq!(record.device_name.as_deref(), Some("Phone"));
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&record.derived_key)
            .expect("base64 derived key");
        assert_eq!(decoded, vec![0x42_u8; 256]);
    }

    #[test]
    fn upsert_refreshes_existing_record() {
        let (_temp, store) = temp_store();
        store
            .upsert_trust("device-1", Some("Old"), &[1_u8; 32])
            .expect("first upsert");
        store
            .upsert_trust("device-1", Some("New"), &[2_u8; 32])
            .expect("second upsert");
        let state = store.load_or_create().expect("load");
        let record = state.trust.get("device-1").expect("record");
        assert_eq!(record.device_name.as_deref(), Some("New"));
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&record.derived_key)
            .expect("base64 derived key");
        assert_eq!(decoded, vec![2_u8; 32]);
    }

    #[test]
    fn remove_trust_reports_whether_record_existed() {
        let (_temp, store) = temp_store();
        assert!(!store.remove_trust("missing").expect("remove missing"));
        store
            .upsert_trust("device-1", None, &[3_u8; 16])
            .expect("upsert");
        assert!(store.remove_trust("device-1").expect("remove"));
        let state = store.load_or_create().expect("load");
        assert!(state.trust.is_empty());
    }
}
