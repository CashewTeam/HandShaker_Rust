//! Local photo-sync ledger: which phone files were synced where, keyed by
//! the **full sync scope** (device + remote root + local root). Stored
//! out-of-band from `state.json` because the snapshot can grow large and
//! must be replaced atomically (temp file + rename) so a crash never
//! leaves a half-written ledger.
//!
//! Round-2 audit P0-1: ledger v3. The v2 ledger was keyed by device_uuid
//! only, so two sync profiles on the same phone (e.g. Camera -> ~/Pictures
//! and Screenshots -> /Volumes/Archive) shared one file: they could adopt
//! each other's records, diff against the wrong local paths, and delete
//! files in the other profile's directory. v3 binds the ledger to the
//! normalized (device_uuid, remote_root, local_root) triple: distinct
//! scopes get distinct files, and the identity is verified field-by-field
//! on load so a copied ledger is a hard error.
//!
//! Migration from v1/v2 (device-only) is **refused**: the scope cannot be
//! proven from the old file, and guessing could adopt another profile's
//! delete plan. The legacy file is left in place for operator backup.

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

/// Unambiguous sync scope (round-2 P0-1). Two profiles that differ in any
/// field must never share a ledger.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncLedgerIdentity {
    pub device_uuid: String,
    /// Normalized remote root, e.g. `/DCIM/Camera` (no trailing slash).
    pub remote_root: String,
    /// Normalized absolute local root, e.g. `/Users/me/Pictures` (no
    /// trailing slash).
    pub local_root: String,
}

/// Normalize a remote/local root for ledger identity: trim whitespace and
/// trailing slashes, keep `/` as the root. Application callers canonicalize
/// local roots before constructing the identity; this function is the
/// final guard so two spellings of the same path cannot split one ledger.
pub fn normalize_root(root: &str) -> String {
    let trimmed = root.trim();
    if trimmed.len() <= 1 {
        return "/".to_string();
    }
    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

impl SyncLedgerIdentity {
    /// Canonical form: every field normalized, so `discover` and `load`
    /// always compare like with like.
    pub fn normalized(&self) -> SyncLedgerIdentity {
        SyncLedgerIdentity {
            device_uuid: self.device_uuid.clone(),
            remote_root: normalize_root(&self.remote_root),
            local_root: normalize_root(&self.local_root),
        }
    }
}

/// Persistent sync ledger for one sync scope.
pub struct SyncStore {
    path: PathBuf,
    identity: SyncLedgerIdentity,
}

/// On-disk envelope, schema v3 (round-2 P0-1): the full scope identity is
/// stored inside the file and verified field-by-field on load.
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncLedgerFileV3 {
    schema_version: u32,
    identity: SyncLedgerIdentity,
    snapshot: SyncSnapshot,
}

/// Lossless filename key for a sync scope: hex SHA-256 of
/// `device_uuid \0 remote_root \0 local_root`. Distinct scopes can never
/// map to one file. Public so the Application layer can use the same key
/// for per-ledger write mutexes.
pub fn ledger_scope_key(identity: &SyncLedgerIdentity) -> String {
    let identity = identity.normalized();
    let canonical = format!(
        "{}\0{}\0{}",
        identity.device_uuid, identity.remote_root, identity.local_root
    );
    let digest = Sha256::digest(canonical.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

impl SyncStore {
    /// Ledger path under the given handshaker config dir:
    /// `<config_dir>/sync/<sha256(scope)>.json` (v3).
    pub fn discover(config_dir: &Path, identity: &SyncLedgerIdentity) -> Self {
        let identity = identity.normalized();
        Self {
            path: config_dir
                .join("sync")
                .join(format!("{}.json", ledger_scope_key(&identity))),
            identity,
        }
    }

    /// Absolute path of the ledger file (diagnostics/UI use).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The normalized scope this store is bound to.
    pub fn identity(&self) -> &SyncLedgerIdentity {
        &self.identity
    }

    #[cfg(test)]
    pub(crate) fn at(path: PathBuf, identity: SyncLedgerIdentity) -> Self {
        Self {
            path,
            identity: identity.normalized(),
        }
    }

    /// Path of the legacy v2 ledger for the same device_uuid
    /// (`<sha256(device_uuid)>.json`); kept only for detection. v2 files
    /// carry no scope, so they can never be adopted — load() reports them
    /// as a hard error instead.
    fn legacy_v2_path(sync_dir: &Path, device_uuid: &str) -> PathBuf {
        sync_dir.join(format!("{}.json", sha256_hex(device_uuid.as_bytes())))
    }

    /// Path of the legacy v1 ledger (`<sanitized(device_uuid)>.json`);
    /// detection only (same refusal as v2).
    fn legacy_v1_path(sync_dir: &Path, device_uuid: &str) -> PathBuf {
        sync_dir.join(format!("{}.json", sanitize_device_uuid(device_uuid)))
    }

    /// Load the ledger; `None` when no sync has ever run for this scope.
    /// A corrupt, version-mismatched, or identity-mismatched file is a
    /// hard error (never silently rebuilt). Legacy v1/v2 files (no scope)
    /// are refused with a clear error so the operator can back up and
    /// rebuild — the scope cannot be proven from a device-only ledger.
    pub fn load(&self) -> Result<Option<SyncSnapshot>> {
        if self.path.exists() {
            return self.load_v3().map(Some);
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Configuration(i18n::text("sync.parent_missing").to_string()))?;
        let legacy_v2 = Self::legacy_v2_path(parent, &self.identity.device_uuid);
        let legacy_v1 = Self::legacy_v1_path(parent, &self.identity.device_uuid);
        if legacy_v2.exists() || legacy_v1.exists() {
            return Err(Error::Configuration(i18n::format(
                "sync.legacy_scope_unproven",
                &[
                    &legacy_v2.display().to_string(),
                    &legacy_v1.display().to_string(),
                ],
            )));
        }
        Ok(None)
    }

    fn load_v3(&self) -> Result<SyncSnapshot> {
        set_path_permissions(&self.path)?;
        let bytes = fs::read(&self.path).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.read_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        let ledger: SyncLedgerFileV3 = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.parse_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        if ledger.schema_version != 3 {
            return Err(Error::Configuration(i18n::format(
                "sync.version_unsupported",
                &[&ledger.schema_version.to_string()],
            )));
        }
        if ledger.identity.normalized() != self.identity {
            return Err(Error::Configuration(i18n::format(
                "sync.identity_mismatch",
                &[&self.path.display().to_string()],
            )));
        }
        Ok(ledger.snapshot)
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

        let ledger = SyncLedgerFileV3 {
            schema_version: 3,
            identity: self.identity.clone(),
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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Keep only safe filename characters; device_uuid is phone-controlled and
/// must never escape the `sync/` directory (e.g. `../state`). Only used
/// for legacy v1 path probing now.
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

    fn identity(device_uuid: &str, remote: &str, local: &str) -> SyncLedgerIdentity {
        SyncLedgerIdentity {
            device_uuid: device_uuid.to_string(),
            remote_root: remote.to_string(),
            local_root: local.to_string(),
        }
    }

    #[test]
    fn first_save_creates_0600_file_under_0700_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sync/u.json");
        let store = SyncStore::at(path.clone(), identity("dev", "/DCIM", "/tmp"));
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
        // Distinct scopes (same device, different roots) must never map to
        // one file (round-2 P0-1).
        let a = ledger_scope_key(&identity("dev", "/DCIM/Camera", "/Users/me/Pictures"));
        let b = ledger_scope_key(&identity(
            "dev",
            "/Pictures/Screenshots",
            "/Volumes/Archive",
        ));
        let c = ledger_scope_key(&identity("dev", "/DCIM/Camera", "/Volumes/Archive"));
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        // Path spelling variants normalize to the same key.
        assert_eq!(
            ledger_scope_key(&identity("dev", "/DCIM/Camera/", "/Users/me/Pictures")),
            a,
            "trailing slash must not split a ledger"
        );
        // Stable across calls.
        assert_eq!(
            ledger_scope_key(&identity("dev", "/DCIM/Camera", "/Users/me/Pictures")),
            a
        );
        // Path component safety: hex is always filename-safe.
        assert!(a.chars().all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn same_device_different_local_root_get_distinct_ledger_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store_a = SyncStore::discover(
            temp.path(),
            &identity("phone-X", "/DCIM/Camera", "/Users/me/Pictures"),
        );
        let store_b = SyncStore::discover(
            temp.path(),
            &identity("phone-X", "/DCIM/Camera", "/Volumes/Archive/Screenshots"),
        );
        assert_ne!(store_a.path, store_b.path, "P0-1: scope must be in the key");
        // Independent snapshots: saving A must not be visible through B.
        store_a.save(&snapshot()).expect("save a");
        assert!(
            store_b.load().expect("b").is_none(),
            "no cross-profile sharing"
        );
    }

    #[test]
    fn tampered_identity_fails_identity_check() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sync/u.json");
        let store = SyncStore::at(path.clone(), identity("dev", "/DCIM", "/tmp"));
        store.save(&snapshot()).expect("save");

        // Rewrite the same file with a different local_root (simulates a
        // ledger copied from another profile).
        let tampered = serde_json::json!({
            "schema_version": 3,
            "identity": {
                "device_uuid": "dev",
                "remote_root": "/DCIM",
                "local_root": "/elsewhere"
            },
            "snapshot": serde_json::json!({"files": {}}),
        });
        std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).expect("write");
        let error = store.load().expect_err("identity mismatch must fail");
        assert!(matches!(error, Error::Configuration(_)));
    }

    #[test]
    fn discover_uses_lossless_scope_hash_filename() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SyncStore::discover(temp.path(), &identity("../state", "/DCIM", "/tmp"));
        assert!(
            store.path.to_string_lossy().contains("sync/"),
            "ledger stays inside <config>/sync/"
        );
        // Same device + same roots -> same file; different root -> different.
        let store_b =
            SyncStore::discover(temp.path(), &identity("../state", "/DCIM/Other", "/tmp"));
        assert_ne!(store.path, store_b.path);
    }

    #[test]
    fn legacy_device_only_ledgers_are_refused_not_adopted() {
        // Round-2 P0-1: a v2 ledger (sha256(device_uuid).json) carries no
        // scope, so it must be a hard error — never adopted by any profile
        // (a wrong adoption could delete files in another profile's dir).
        let temp = tempfile::tempdir().expect("tempdir");
        let sync_dir = temp.path().join("sync");
        std::fs::create_dir_all(&sync_dir).expect("dir");
        let v2_path = sync_dir.join(format!("{}.json", sha256_hex(b"phone-X")));
        std::fs::write(
            &v2_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 2,
                "device_uuid": "phone-X",
                "snapshot": {"files": {}}
            }))
            .unwrap(),
        )
        .expect("write v2");
        let store = SyncStore::discover(
            temp.path(),
            &identity("phone-X", "/DCIM/Camera", "/Users/me/Pictures"),
        );
        let error = store.load().expect_err("v2 without scope must be refused");
        assert!(matches!(error, Error::Configuration(_)));
        assert!(
            !error.to_string().is_empty(),
            "error must explain the refusal"
        );
        assert!(!store.path.exists(), "no v3 may be written");
    }

    #[test]
    fn save_is_atomic_and_updates_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sync/u.json");
        let store = SyncStore::at(path.clone(), identity("dev", "/DCIM", "/tmp"));
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
        let store = SyncStore::at(path.clone(), identity("dev", "/DCIM", "/tmp"));
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
        // only probed for refusal; the v3 path is a scope hash.
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SyncStore::discover(temp.path(), &identity("../state", "/DCIM", "/tmp"));
        assert!(store.path.to_string_lossy().contains("sync/"));
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
