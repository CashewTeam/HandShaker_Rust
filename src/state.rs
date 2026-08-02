use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::i18n;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TrustRecord {
    pub device_name: Option<String>,
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
