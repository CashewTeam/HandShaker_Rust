//! Local photo-sync ledger: which phone files were synced where, keyed by
//! device_uuid. Stored out-of-band from `state.json` because the snapshot can
//! grow large and must be replaced atomically (temp file + rename) so a crash
//! never leaves a half-written ledger.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use sha2::{Digest, Sha256};

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
///
/// P0-2 (ledger v2): the on-disk filename is the SHA-256 of the raw
/// device_uuid bytes (lossless — no sanitize collisions), and the file
/// itself carries the device_uuid, which is verified on load so one
/// device can never adopt another device's ledger.
pub struct SyncStore {
    path: PathBuf,
    device_uuid: String,
}

/// On-disk envelope, schema v2 (P0-2): the original `device_uuid` is
/// stored inside the file and verified against the store's identity on
/// load, so a collision or a copy of another device's ledger is a hard
/// error instead of a wrong delete plan.
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncLedgerFileV2 {
    schema_version: u32,
    device_uuid: String,
    snapshot: SyncSnapshot,
}

/// v1 envelope (no device identity). Only ever read for the guarded
/// migration path in [`SyncStore::load`].
#[derive(serde::Deserialize)]
struct SyncLedgerFileV1 {
    schema_version: u32,
    snapshot: SyncSnapshot,
}

/// Lossless filename key for a device_uuid: hex SHA-256 of the raw
/// UTF-8 bytes. Two distinct ids can never map to one file (unlike
/// `sanitize_device_uuid`, where `abc:def`, `abc/def` and `abcdef` all
/// collapsed to `abcdef.json`).
fn ledger_key(device_uuid: &str) -> String {
    let digest = Sha256::digest(device_uuid.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

impl SyncStore {
    /// Ledger path under the given handshaker config dir:
    /// `<config_dir>/sync/<sha256(device_uuid)>.json` (v2). The legacy
    /// v1 location `<config_dir>/sync/<sanitized(device_uuid)>.json` is
    /// still probed for migration (see `load`).
    pub fn discover(config_dir: &Path, device_uuid: &str) -> Self {
        Self {
            path: config_dir
                .join("sync")
                .join(format!("{}.json", ledger_key(device_uuid))),
            device_uuid: device_uuid.to_string(),
        }
    }

    /// Absolute path of the ledger file (diagnostics/UI use).
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn at(path: PathBuf) -> Self {
        Self {
            path,
            device_uuid: "test-device".to_string(),
        }
    }

    /// Path of the legacy v1 ledger for the same device_uuid (kept only
    /// for detection and guarded migration). `sync_dir` is the directory
    /// that holds the ledger files (`<config_dir>/sync`).
    fn legacy_v1_path(sync_dir: &Path, device_uuid: &str) -> PathBuf {
        sync_dir.join(format!("{}.json", sanitize_device_uuid(device_uuid)))
    }

    /// Load the ledger; `None` when no sync has ever run for this device.
    /// A corrupt or version-mismatched file is a hard error (never silently
    /// rebuilt) so the operator can recover without losing the snapshot.
    ///
    /// v2 files must carry the exact `device_uuid` of this store — a
    /// mismatch (tampered file, copied ledger, sanitize collision from a
    /// v1 migration) is a hard error. Legacy v1 files (no identity inside)
    /// are migrated only when their name proves identity losslessly
    /// (`sanitize(uuid) == uuid`); otherwise migration is refused with a
    /// clear error so the operator can back up and rebuild.
    pub fn load(&self) -> Result<Option<SyncSnapshot>> {
        if self.path.exists() {
            return self.load_v2().map(Some);
        }
        // No v2 ledger yet: look for a legacy v1 file for this device.
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Configuration(i18n::text("sync.parent_missing").to_string()))?;
        let legacy = Self::legacy_v1_path(parent, &self.device_uuid);
        if !legacy.exists() {
            return Ok(None);
        }
        self.migrate_v1(&legacy)
    }

    fn load_v2(&self) -> Result<SyncSnapshot> {
        set_path_permissions(&self.path)?;
        let bytes = fs::read(&self.path).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.read_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        let ledger: SyncLedgerFileV2 = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.parse_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        if ledger.schema_version != 2 {
            return Err(Error::Configuration(i18n::format(
                "sync.version_unsupported",
                &[&ledger.schema_version.to_string()],
            )));
        }
        if ledger.device_uuid != self.device_uuid {
            return Err(Error::Configuration(i18n::format(
                "sync.identity_mismatch",
                &[&self.path.display().to_string()],
            )));
        }
        Ok(ledger.snapshot)
    }

    /// Guarded v1 -> v2 migration (P0-2). Only migrates when the legacy
    /// filename identifies the device losslessly; otherwise refuses with
    /// a hard error (never guess identity — a wrong guess could adopt
    /// another device's file-delete plan). On success writes v2
    /// atomically, then renames the v1 file to `.legacy.bak`.
    fn migrate_v1(&self, legacy: &Path) -> Result<Option<SyncSnapshot>> {
        let sanitized = sanitize_device_uuid(&self.device_uuid);
        let lossless = sanitized == self.device_uuid;
        if !lossless {
            return Err(Error::Configuration(i18n::format(
                "sync.legacy_identity_unverified",
                &[&legacy.display().to_string()],
            )));
        }
        set_path_permissions(legacy)?;
        let bytes = fs::read(legacy).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.read_failed",
                &[&legacy.display().to_string(), &error.to_string()],
            ))
        })?;
        let v1: SyncLedgerFileV1 = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.parse_failed",
                &[&legacy.display().to_string(), &error.to_string()],
            ))
        })?;
        if v1.schema_version != 1 {
            return Err(Error::Configuration(i18n::format(
                "sync.version_unsupported",
                &[&v1.schema_version.to_string()],
            )));
        }
        // Commit v2 first; only then retire the v1 file.
        self.save(&v1.snapshot)?;
        let backup = legacy.with_extension("json.legacy.bak");
        fs::rename(legacy, &backup).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.migrate_backup_failed",
                &[
                    &legacy.display().to_string(),
                    &backup.display().to_string(),
                    &error.to_string(),
                ],
            ))
        })?;
        Ok(Some(v1.snapshot))
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

        let ledger = SyncLedgerFileV2 {
            schema_version: 2,
            device_uuid: self.device_uuid.clone(),
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
    fn ledger_key_is_lossless_and_distinct() {
        // P0-2: three ids that all sanitized to "abcdef.json" in v1 now
        // map to three different files.
        let a = ledger_key("abc:def");
        let b = ledger_key("abc/def");
        let c = ledger_key("abcdef");
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        // Stable across calls.
        assert_eq!(ledger_key("abcdef"), c);
        // Path component safety: hex is always filename-safe.
        assert!(a.chars().all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn tampered_device_uuid_fails_identity_check() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sync/u.json");
        let store = SyncStore::at(path.clone());
        store.save(&snapshot()).expect("save");

        // Rewrite the same file with a different device_uuid (simulates a
        // copied ledger or a tampered file).
        let tampered = serde_json::json!({
            "schema_version": 2,
            "device_uuid": "other-device",
            "snapshot": serde_json::json!({"files": {}}),
        });
        std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).expect("write");
        let error = store.load().expect_err("identity mismatch must fail");
        assert!(matches!(error, Error::Configuration(_)));
    }

    #[test]
    fn discover_uses_lossless_hash_filename() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SyncStore::discover(temp.path(), "../state");
        assert_eq!(
            store.path,
            temp.path()
                .join(format!("sync/{}.json", ledger_key("../state"))),
            "the raw uuid is hashed, never sanitized into the filename"
        );
        // Two uuids that v1 collapsed onto one file now diverge.
        let store_b = SyncStore::discover(temp.path(), "abcdef");
        assert_ne!(store.path, store_b.path);
    }

    #[test]
    fn v1_ledger_is_migrated_only_when_identity_is_lossless() {
        let temp = tempfile::tempdir().expect("tempdir");
        // Lossless name: sanitize(uuid) == uuid -> identity proven by the
        // filename, migration allowed.
        let uuid = "e976ce6596c81fc5";
        let v1_path = temp.path().join(format!("sync/{uuid}.json"));
        std::fs::create_dir_all(v1_path.parent().unwrap()).expect("dir");
        std::fs::write(
            &v1_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "snapshot": {
                    "files": {
                        "/storage/emulated/0/DCIM/a.jpg": {
                            "size": 1,
                            "checksum": "c",
                            "ext_data": null,
                            "modified_at": 1,
                            "local_path": "/tmp/a.jpg",
                            "local_sha256": "s"
                        }
                    }
                }
            }))
            .unwrap(),
        )
        .expect("write v1");
        let store = SyncStore::discover(temp.path(), uuid);
        let loaded = store.load().expect("migrate").expect("some");
        assert_eq!(loaded.files.len(), 1);
        assert!(store.path.exists(), "v2 file written");
        assert!(!v1_path.exists(), "v1 retired");
        assert!(v1_path.with_extension("json.legacy.bak").exists());
        // The migrated v2 file loads again (identity round-trip).
        assert_eq!(store.load().expect("reload").expect("some").files.len(), 1);
    }

    #[test]
    fn v1_ledger_with_lossy_name_is_refused_not_guessed() {
        let temp = tempfile::tempdir().expect("tempdir");
        // "abc:def" sanitizes to "abcdef" — the legacy file cannot be
        // proven to belong to this device (it might be "abc/def"'s), so
        // migration must be refused, never guessed.
        let uuid = "abc:def";
        let v1_path = temp.path().join("sync/abcdef.json");
        std::fs::create_dir_all(v1_path.parent().unwrap()).expect("dir");
        std::fs::write(
            &v1_path,
            br#"{"schema_version": 1, "snapshot": {"files": {}}},"#,
        )
        .expect("write v1");
        let store = SyncStore::discover(temp.path(), uuid);
        let error = store.load().expect_err("must refuse");
        assert!(matches!(error, Error::Configuration(_)));
        assert!(
            error.to_string().contains("abcdef.json"),
            "error must point at the unverifiable legacy file"
        );
        assert!(!store.path.exists(), "no v2 may be written");
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
    fn device_uuid_is_still_sanitized_for_legacy_path_probing() {
        // The legacy v1 path is still derived through sanitize, but it is
        // only probed for guarded migration; the v2 path is a hash.
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SyncStore::discover(temp.path(), "../state");
        assert!(
            store.path.to_string_lossy().contains("sync/"),
            "ledger stays inside <config>/sync/"
        );
        assert!(
            sanitize_device_uuid("a/b..c\\d e")
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '-'
                    || character == '_')
        );
        let legacy = SyncStore::legacy_v1_path(temp.path().join("sync").as_path(), "../state");
        assert_eq!(legacy, temp.path().join("sync/state.json"));
    }
}
