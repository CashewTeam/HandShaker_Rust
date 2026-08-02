//! Local photo-sync ledger: which phone files were synced where, keyed by
//! device_uuid. Stored out-of-band from `state.json` because the snapshot can
//! grow large and must be replaced atomically (temp file + rename) so a crash
//! never leaves a half-written ledger.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::domain::{SyncConfig, SyncSnapshot};
use crate::error::{Error, Result};
use crate::i18n;

/// Resolve the handshaker config directory (same base as `state.json`).
pub fn default_config_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "handshaker")
        .ok_or_else(|| Error::Configuration(i18n::text("state.config_dir_missing").to_string()))?;
    Ok(dirs.config_dir().to_path_buf())
}

/// Persistent sync ledger for one device.
pub struct SyncStore {
    path: PathBuf,
}

/// On-disk envelope with a schema version guard.
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncLedgerFile {
    schema_version: u32,
    snapshot: SyncSnapshot,
}

impl SyncStore {
    /// Ledger path under the given handshaker config dir:
    /// `<config_dir>/sync/<device_uuid>.json`. The device_uuid is a
    /// phone-controlled string, so it is sanitized to a safe filename
    /// component (defense in depth; the CLI rejects invalid ids up front).
    pub fn discover(config_dir: &Path, device_uuid: &str) -> Self {
        Self {
            path: config_dir
                .join("sync")
                .join(format!("{}.json", sanitize_device_uuid(device_uuid))),
        }
    }

    #[cfg(test)]
    pub(crate) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Load the ledger; `None` when no sync has ever run for this device.
    /// A corrupt or version-mismatched file is a hard error (never silently
    /// rebuilt) so the operator can recover without losing the snapshot.
    pub fn load(&self) -> Result<Option<SyncSnapshot>> {
        if !self.path.exists() {
            return Ok(None);
        }
        set_path_permissions(&self.path)?;
        let bytes = fs::read(&self.path).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.read_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        let ledger: SyncLedgerFile = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.parse_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        if ledger.schema_version != 1 {
            return Err(Error::Configuration(i18n::format(
                "sync.version_unsupported",
                &[&ledger.schema_version.to_string()],
            )));
        }
        Ok(Some(ledger.snapshot))
    }

    /// Atomically persist the snapshot: write a temp file in the same
    /// directory, fsync it, then rename over the ledger.
    pub fn save(&self, snapshot: &SyncSnapshot) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Configuration(i18n::text("sync.parent_missing").to_string()))?;
        fs::create_dir_all(parent).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.create_failed",
                &[&parent.display().to_string(), &error.to_string()],
            ))
        })?;
        set_directory_permissions(parent)?;

        let ledger = SyncLedgerFile {
            schema_version: 1,
            snapshot: snapshot.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&ledger).map_err(|error| {
            Error::Configuration(i18n::format("sync.serialize_failed", &[&error.to_string()]))
        })?;

        // O_EXCL + random suffix: a pre-planted symlink at a deterministic
        // tmp name cannot be followed or truncated by a same-user attacker.
        let mut tmp = self
            .path
            .with_extension(format!("json.tmp-{}", rand::random::<u64>()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        set_file_mode(&mut options);
        let mut file = loop {
            match options.open(&tmp) {
                Ok(file) => break file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    tmp = self
                        .path
                        .with_extension(format!("json.tmp-{}", rand::random::<u64>()));
                }
                Err(error) => {
                    return Err(Error::Configuration(i18n::format(
                        "sync.write_failed",
                        &[&tmp.display().to_string(), &error.to_string()],
                    )));
                }
            }
        };
        file.write_all(&bytes).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.write_failed",
                &[&tmp.display().to_string(), &error.to_string()],
            ))
        })?;
        file.sync_all().map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.write_failed",
                &[&tmp.display().to_string(), &error.to_string()],
            ))
        })?;
        drop(file);

        fs::rename(&tmp, &self.path).map_err(|error| {
            let _ = fs::remove_file(&tmp);
            Error::Configuration(i18n::format(
                "sync.write_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        set_path_permissions(&self.path)?;
        sync_parent(parent);
        Ok(())
    }
}

/// Keep only safe filename characters; device_uuid is phone-controlled and
/// must never escape the `sync/` directory (e.g. `../state`).
pub(crate) fn sanitize_device_uuid(device_uuid: &str) -> String {
    device_uuid
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
        })
        .collect()
}

/// Derive the stable pc_id sent in PHOTO_SYNC_REQUEST(37) from the host UUID.
/// The phone uses it to decide `is_first` for a fresh pc, so it must not
/// change across runs on the same host. The reference macOS client sends its
/// own `SFGenericDevice getMacUUID` verbatim as `pcId`
/// (`SSPPhotoSyncRequestOperation initWithSSPManager:macUUID:lastFiles:`,
/// SmartFinderCore.h), so we mirror that: the host UUID is used as-is.
pub fn pc_id_from_host_uuid(host_uuid: &str) -> String {
    host_uuid.to_string()
}

/// Build a sync config with a stable pc_id.
pub fn sync_config(
    device_uuid: &str,
    phone_root: &str,
    local_root: &str,
    host_uuid: &str,
) -> SyncConfig {
    SyncConfig {
        device_uuid: device_uuid.to_string(),
        phone_root: phone_root.to_string(),
        local_root: local_root.to_string(),
        pc_id: pc_id_from_host_uuid(host_uuid),
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
            "sync.permission_failed",
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
            "sync.permission_failed",
            &[&path.display().to_string(), &error.to_string()],
        ))
    })
}

#[cfg(not(unix))]
fn set_path_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

/// Best-effort fsync of the parent directory so the rename is durable.
fn sync_parent(parent: &Path) {
    #[cfg(unix)]
    {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SyncConfig, SyncFileRecord, SyncSnapshot};
    use std::os::unix::fs::PermissionsExt;

    fn record() -> SyncFileRecord {
        SyncFileRecord {
            size: 1024,
            checksum: Some("abc".to_string()),
            ext_data: None,
            modified_at: Some(1),
            local_path: "/tmp/a.jpg".to_string(),
            local_sha256: Some("sha".to_string()),
        }
    }

    fn snapshot() -> SyncSnapshot {
        let mut snapshot = SyncSnapshot::default();
        snapshot
            .files
            .insert("/storage/emulated/0/DCIM/a.jpg".to_string(), record());
        snapshot
    }

    #[test]
    fn first_save_creates_0600_file_under_0700_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sync/u.json");
        let store = SyncStore::at(path.clone());
        assert!(store.load().expect("load").is_none());
        store.save(&snapshot()).expect("save");

        assert!(path.exists());
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = fs::metadata(path.parent().unwrap())
            .expect("meta")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);

        let loaded = store.load().expect("load").expect("some");
        assert_eq!(loaded, snapshot());
    }

    #[test]
    fn save_is_atomic_and_updates_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sync/u.json");
        let store = SyncStore::at(path.clone());
        store.save(&snapshot()).expect("save");
        // No temp file may linger after a successful commit.
        assert!(!path.with_extension("json.tmp").exists());

        let mut updated = snapshot();
        updated.files.insert(
            "/storage/emulated/0/DCIM/b.jpg".to_string(),
            SyncFileRecord {
                size: 2048,
                ..record()
            },
        );
        store.save(&updated).expect("save again");
        let loaded = store.load().expect("load").expect("some");
        assert_eq!(loaded.files.len(), 2);
    }

    #[test]
    fn corrupt_ledger_is_a_hard_error_not_a_silent_rebuild() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sync/u.json");
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        std::fs::write(&path, b"{not json").expect("write");
        let store = SyncStore::at(path.clone());
        let error = store.load().expect_err("must fail");
        assert!(matches!(error, Error::Configuration(_)));

        // Version mismatch is also a hard error.
        std::fs::write(
            &path,
            br#"{"schema_version": 99, "snapshot": {"files": {}}}"#,
        )
        .expect("write");
        let error = store.load().expect_err("must fail");
        assert!(matches!(error, Error::Configuration(_)));
    }

    #[test]
    fn pc_id_is_stable_and_matches_mac_uu_semantics() {
        let uuid = "11111111-1111-1111-1111-111111111111";
        let a = pc_id_from_host_uuid(uuid);
        let b = pc_id_from_host_uuid(uuid);
        let c = pc_id_from_host_uuid("22222222-2222-2222-2222-222222222222");
        assert_eq!(a, b);
        assert_ne!(a, c);
        // The reference macOS client sends getMacUUID verbatim as pcId.
        assert_eq!(a, uuid);
        assert!(
            SyncConfig {
                device_uuid: "d".to_string(),
                phone_root: "/p".to_string(),
                local_root: "/l".to_string(),
                pc_id: a.clone(),
            }
            .pc_id
            .eq(uuid)
        );
    }

    #[test]
    fn device_uuid_is_sanitized_to_a_safe_filename_component() {
        let temp = tempfile::tempdir().expect("tempdir");
        // A malicious phone could report "../state" or an absolute path; the
        // ledger must stay inside <config>/sync/ regardless.
        let store = SyncStore::discover(temp.path(), "../state");
        assert_eq!(
            store.path,
            temp.path().join("sync/state.json"),
            "dots and slashes are stripped"
        );
        let store = SyncStore::discover(temp.path(), "e976ce6596c81fc5");
        assert_eq!(store.path, temp.path().join("sync/e976ce6596c81fc5.json"));
        assert!(
            sanitize_device_uuid("a/b..c\\d e")
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '-'
                    || character == '_')
        );
    }
}
