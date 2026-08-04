//! Photo-sync engine: plan (diff) + execute (download/delete) + incremental
//! application of FILE_CHANGE(38) events.
//!
//! Direction is one-way: phone -> host. The local ledger (`SyncSnapshot`)
//! records what was downloaded and the local SHA-256 at commit time, so a
//! re-run is idempotent and a user-modified local file is never silently
//! overwritten or deleted (it is reported as a conflict instead).

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::client::HandShakerClient;
use crate::domain::{RemoteFile, SyncConfig, SyncDiff, SyncFileRecord, SyncSnapshot};
use crate::error::{Error, Result};
use crate::events::FileChange;
use crate::i18n;
use crate::sync_journal::{PendingSyncAction, SyncJournal};
use crate::sync_store::SyncStore;

/// The phone's photo root must never be reachable through a relative escape.
const PHONE_ROOT_MISSING: &str = "sync.path_outside_root";

/// Result of executing a sync plan.
#[derive(Debug, Clone, Default, serde::Serialize, PartialEq, Eq)]
pub struct SyncRunResult {
    /// Phone paths downloaded successfully.
    pub downloaded: Vec<String>,
    /// Phone paths removed locally.
    pub deleted: Vec<String>,
    /// Phone paths that failed to download (aggregated, run continues).
    pub failures: Vec<String>,
    /// Phone paths kept because the local file differs from the ledger.
    pub conflicts: Vec<String>,
}

/// Pure diff between the phone's current state and the ledger snapshot.
///
/// Classification (docs/10 §10.4): a recorded file whose phone checksum
/// changed is content-modified (re-download); a file whose checksum is
/// unchanged but whose ext_data/modified_at changed is metadata-only; a
/// recorded file absent on the phone is deleted. Files new on the phone are
/// added. Local-file conflict checks happen separately against the disk.
pub fn plan_diff(phone_files: &[RemoteFile], snapshot: &SyncSnapshot) -> SyncDiff {
    let mut diff = SyncDiff::default();
    for file in phone_files {
        if file.is_directory {
            continue;
        }
        match snapshot.files.get(&file.path) {
            None => diff.added.push(file.path.clone()),
            Some(record) => {
                // P1-1: any observable checksum state change is a content
                // change — including None -> Some and Some -> None (only
                // "both absent" means unchanged). The old guard
                // `record.checksum.is_some() &&` skipped None->Some.
                let checksum_changed = record.checksum.as_deref() != file.checksum.as_deref();
                let metadata_changed = record.ext_data.as_deref() != file.ext_data.as_deref()
                    || record.modified_at != file.modified_at;
                if checksum_changed {
                    diff.added.push(file.path.clone());
                } else if metadata_changed {
                    diff.info_modified.push(file.path.clone());
                }
            }
        }
    }
    for path in snapshot.files.keys() {
        if !phone_files.iter().any(|file| file.path == *path) {
            diff.deleted.push(path.clone());
        }
    }
    diff
}

/// Report conflicts: entries the plan would touch (download/delete) whose
/// local file exists and differs from the ledger's recorded SHA-256. Such
/// files are preserved and excluded from execution.
pub fn check_conflicts(diff: &SyncDiff, snapshot: &SyncSnapshot) -> Vec<String> {
    let mut conflicts = Vec::new();
    let mut seen = BTreeMap::<String, ()>::new();
    for path in diff.added.iter().chain(diff.deleted.iter()) {
        let Some(record) = snapshot.files.get(path) else {
            // New download, nothing recorded locally yet: nothing to protect.
            continue;
        };
        let Some(expected) = record.local_sha256.as_deref() else {
            continue;
        };
        let local = PathBuf::from(&record.local_path);
        if !local.exists() {
            continue;
        }
        // Fail closed (P1-1): an existing local file that cannot be read
        // (permissions, transient I/O) is a conflict — treating it as
        // "no conflict" would let the plan overwrite or delete a file whose
        // content we could not verify against the ledger's SHA-256.
        match sha256_file(&local) {
            Ok(actual) if actual != expected => {
                if seen.insert(path.clone(), ()).is_none() {
                    conflicts.push(path.clone());
                }
            }
            Ok(_) => {} // matches the ledger: safe to touch
            Err(_) => {
                if seen.insert(path.clone(), ()).is_none() {
                    conflicts.push(path.clone());
                }
            }
        }
    }
    conflicts
}

/// Execute a plan with per-item checkpointing (round-2 P0-2): every
/// download/delete is journaled (WAL), applied, and the ledger saved
/// before the next item starts, so a crash or hard abort at any point
/// leaves at most one recoverable pending action instead of an
/// inconsistent "files changed, ledger stale" window. Failures are
/// aggregated, never aborting the run. `info_modified` entries are
/// metadata-only and committed with the final save.
#[allow(clippy::too_many_arguments)]
pub async fn execute_plan_with_checkpoint(
    client: &HandShakerClient,
    config: &SyncConfig,
    phone_files: &[RemoteFile],
    diff: &SyncDiff,
    snapshot: &SyncSnapshot,
    conflicts: &[String],
    journal: &SyncJournal,
    store: &SyncStore,
) -> Result<(SyncRunResult, SyncSnapshot)> {
    let mut result = SyncRunResult {
        conflicts: conflicts.to_vec(),
        ..SyncRunResult::default()
    };
    let mut updated = snapshot.clone();

    for path in &diff.added {
        if conflicts.contains(path) {
            continue;
        }
        // New files have no ledger record; always take the record fields from
        // the phone's current state (a modified file's old checksum must not
        // leak into the new ledger row).
        let Some(file) = phone_files.iter().find(|file| file.path == *path) else {
            continue;
        };
        let destination = local_destination(config, path)?;
        journaled_download(
            client,
            path,
            file,
            destination,
            &mut updated,
            &mut result,
            journal,
            store,
        )
        .await?;
    }

    for path in &diff.deleted {
        if conflicts.contains(path) {
            continue;
        }
        let Some(record) = snapshot.files.get(path) else {
            continue;
        };
        let local = PathBuf::from(&record.local_path);
        journaled_delete(path, &local, &mut updated, &mut result, journal, store)?;
    }

    for path in &diff.info_modified {
        let Some(file) = phone_files.iter().find(|file| file.path == *path) else {
            continue;
        };
        if let Some(record) = updated.files.get_mut(path) {
            record.ext_data = file.ext_data.clone();
            record.modified_at = file.modified_at;
            record.size = file.size;
            record.checksum = file.checksum.clone();
        }
    }

    // Metadata-only changes since the last per-item checkpoint.
    store.save(&updated)?;
    Ok((result, updated))
}

/// Download one file with journal + checkpoint (round-2 P0-2):
/// staged fsync → journal → rename staged→final → ledger save → clear.
/// A rename failure keeps the journal so the next run's recovery finishes
/// or cleanly abandons the action; the item is reported as a failure.
#[allow(clippy::too_many_arguments)]
async fn journaled_download(
    client: &HandShakerClient,
    remote_path: &str,
    file: &RemoteFile,
    destination: PathBuf,
    snapshot: &mut SyncSnapshot,
    result: &mut SyncRunResult,
    journal: &SyncJournal,
    store: &SyncStore,
) -> Result<()> {
    let (part, hash) = match download_to_staged(client, remote_path, &destination).await {
        Ok(staged) => staged,
        Err(_) => {
            result.failures.push(remote_path.to_string());
            return Ok(());
        }
    };
    let new_record = SyncFileRecord {
        size: file.size,
        checksum: file.checksum.clone(),
        ext_data: file.ext_data.clone(),
        modified_at: file.modified_at,
        local_path: destination.display().to_string(),
        local_sha256: Some(hash),
    };
    let action = PendingSyncAction::Download {
        remote_path: remote_path.to_string(),
        final_path: destination.clone(),
        staged_path: part.clone(),
        new_record: new_record.clone(),
    };
    if let Err(error) = journal.write(&action) {
        let _ = fs::remove_file(&part);
        result.failures.push(remote_path.to_string());
        return Err(error);
    }
    if let Err(_error) = fs::rename(&part, &destination) {
        // Failure is aggregated (the run continues with the next item);
        // the journal stays so the next run's recovery finishes the
        // rename from staged or abandons it.
        result.failures.push(remote_path.to_string());
        return Ok(());
    }
    snapshot.files.insert(remote_path.to_string(), new_record);
    result.downloaded.push(remote_path.to_string());
    store.save(snapshot)?;
    journal.clear()?;
    Ok(())
}

/// Delete one synced file with journal + checkpoint (round-2 P0-2):
/// journal → rename file→same-dir trash → ledger save → remove trash →
/// clear. A failed removal keeps the ledger row (retryable) and is
/// reported as a failure, never silently dropped.
fn journaled_delete(
    remote_path: &str,
    local: &Path,
    snapshot: &mut SyncSnapshot,
    result: &mut SyncRunResult,
    journal: &SyncJournal,
    store: &SyncStore,
) -> Result<()> {
    if !local.exists() {
        // Already in the desired final state; safe to drop the row.
        snapshot.files.remove(remote_path);
        store.save(snapshot)?;
        return Ok(());
    }
    if local.is_dir() {
        // A directory where a synced file is expected: never trash it
        // (it may hold user data); keep the row and report a failure so
        // the operator can resolve it.
        result.failures.push(remote_path.to_string());
        return Ok(());
    }
    let mut trash_name = local.as_os_str().to_owned();
    trash_name.push(format!(".hs-trash.{}", rand::random::<u64>()));
    let trash = PathBuf::from(trash_name);
    let action = PendingSyncAction::Delete {
        remote_path: remote_path.to_string(),
        original_path: local.to_path_buf(),
        trash_path: trash.clone(),
    };
    if let Err(error) = journal.write(&action) {
        result.failures.push(remote_path.to_string());
        return Err(error);
    }
    if let Err(_error) = fs::rename(local, &trash) {
        // File untouched; failure aggregated. The journaled Delete is
        // abandoned by recovery (original present) and retried next run.
        result.failures.push(remote_path.to_string());
        return Ok(());
    }
    snapshot.files.remove(remote_path);
    result.deleted.push(remote_path.to_string());
    store.save(snapshot)?;
    // Remove the trash before clearing the journal: a failure here keeps
    // the journal so the next run's recovery retries the removal.
    fs::remove_file(&trash).map_err(|error| {
        Error::LocalIo(i18n::format(
            "sync.delete_failed",
            &[&trash.display().to_string(), &error.to_string()],
        ))
    })?;
    journal.clear()?;
    Ok(())
}

/// Apply a single FILE_CHANGE(38) event to the snapshot in place, returning
/// the run result for this one file (empty unless a download/delete ran).
/// Apply one incremental change with journal + checkpoint (round-2 P0-2):
/// downloads and deletes go through the same journaled helpers as the
/// full-sync plan, so a crash mid-batch leaves at most one recoverable
/// pending action. Metadata-only changes are applied in memory; the
/// caller persists them with the batch's final save.
pub async fn apply_file_change_with_checkpoint(
    client: &HandShakerClient,
    config: &SyncConfig,
    change: &FileChange,
    snapshot: &mut SyncSnapshot,
    journal: &SyncJournal,
    store: &SyncStore,
) -> Result<SyncRunResult> {
    use crate::events::FileChangeStatus;
    let mut result = SyncRunResult::default();
    let Some(file) = change.file.as_ref() else {
        return Ok(result);
    };
    if file.is_directory {
        return Ok(result);
    }
    let path = file.path.clone();
    match change.status {
        FileChangeStatus::Added
        | FileChangeStatus::Modified
        | FileChangeStatus::FileAndInfoModified => {
            let destination = local_destination(config, &path)?;
            journaled_download(
                client,
                &path,
                file,
                destination,
                snapshot,
                &mut result,
                journal,
                store,
            )
            .await?;
        }
        FileChangeStatus::Deleted => {
            if let Some(record) = snapshot.files.get(&path) {
                let local = PathBuf::from(&record.local_path);
                // Same helper and same semantics as the full-sync delete
                // path: a failed removal keeps the ledger row (retryable)
                // and is reported as a failure, never silently dropped.
                journaled_delete(&path, &local, snapshot, &mut result, journal, store)?;
            }
        }
        FileChangeStatus::InfoModified | FileChangeStatus::None | FileChangeStatus::Unknown => {
            if let Some(record) = snapshot.files.get_mut(&path) {
                record.ext_data = file.ext_data.clone();
                record.modified_at = file.modified_at;
                record.size = file.size;
                record.checksum = file.checksum.clone();
            }
        }
    }
    Ok(result)
}

/// Map a phone path under `phone_root` to a local destination under
/// `local_root`, rejecting any path that escapes the subtree.
pub fn local_destination(config: &SyncConfig, phone_path: &str) -> Result<PathBuf> {
    let root = Path::new(&config.phone_root);
    let relative = Path::new(phone_path)
        .strip_prefix(root)
        .map_err(|_| Error::Protocol(i18n::text(PHONE_ROOT_MISSING).to_string()))?;
    for component in relative.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(Error::Protocol(i18n::text(PHONE_ROOT_MISSING).to_string()));
            }
        }
    }
    Ok(Path::new(&config.local_root).join(relative))
}

/// Download one file to a unique temp path in the destination directory,
/// hash it, then atomically rename over the destination. Returns
/// (local path, sha256 hex).
///
/// P1-3: the temp name is `<destination>.<random>.hs-part` — the old
/// `with_extension("hs-part")` collapsed `a.jpg` and `a.png` onto the
/// same `a.hs-part`, so two profiles/runtimes (or a re-entrant sync)
/// could clobber each other's partial download. A guard removes the temp
/// file on every error path (download failure, hash failure, rename
/// failure); the SHA-256 read runs on a blocking pool so a large photo
/// does not stall the Tokio workers (heartbeat/events stay responsive).
/// Download to a unique staged temp in the destination's directory and
/// hash it; does NOT rename (the caller owns the rename inside the
/// journal transaction, round-2 P0-2). Returns (staged_path, sha256).
async fn download_to_staged(
    client: &HandShakerClient,
    phone_path: &str,
    destination: &Path,
) -> Result<(PathBuf, String)> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            Error::LocalIo(i18n::format(
                "sync.local_dir_failed",
                &[&parent.display().to_string(), &error.to_string()],
            ))
        })?;
    }
    // Unique temp path in the SAME directory as the destination so the
    // final rename stays atomic (same filesystem).
    let mut temp_name = destination.as_os_str().to_owned();
    temp_name.push(format!(".hs-part.{}", rand::random::<u64>()));
    let part = PathBuf::from(temp_name);
    let outcome = async {
        client
            .download_with_options(
                phone_path,
                &part,
                crate::domain::TransferOptions {
                    overwrite: true,
                    progress: None,
                    offset: 0,
                },
                crate::cancellation::RequestOptions::default(),
            )
            .await?;
        let hash = hash_file_blocking(&part).await?;
        Ok::<_, Error>((hash,))
    }
    .await;
    let hash = match outcome {
        Ok((hash,)) => hash,
        Err(error) => {
            // Guard: never leave a partial download behind.
            let _ = fs::remove_file(&part);
            return Err(error);
        }
    };
    Ok((part, hash))
}

/// SHA-256 of a file on the blocking pool (P1-3: large local files must
/// not stall the async workers that also serve heartbeats and events).
async fn hash_file_blocking(path: &Path) -> Result<String> {
    let path = path.to_path_buf();
    let display = path.display().to_string();
    tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|_| {
            Error::LocalIo(i18n::format(
                "sync.local_read_failed",
                &[&display, "hash task panicked"],
            ))
        })?
}

/// SHA-256 hex digest of a file.
fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|error| {
        Error::LocalIo(i18n::format(
            "sync.local_read_failed",
            &[&path.display().to_string(), &error.to_string()],
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            Error::LocalIo(i18n::format(
                "sync.local_read_failed",
                &[&path.display().to_string(), &error.to_string()],
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

/// Small hex encoder (avoids a new dependency for a fixed 32-byte digest).
fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_store::{SyncLedgerIdentity, SyncStore};

    /// Tempdir + store + journal for checkpointed tests (round-2 P0-2).
    struct TestLedger {
        _temp: tempfile::TempDir,
        store: SyncStore,
        journal: SyncJournal,
    }

    fn test_ledger() -> TestLedger {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SyncStore::discover(
            temp.path(),
            &SyncLedgerIdentity {
                device_uuid: "test-dev".to_string(),
                remote_root: "/storage/emulated/0/DCIM/Camera".to_string(),
                local_root: temp.path().display().to_string(),
            },
        );
        let journal = SyncJournal::for_ledger(store.path());
        TestLedger {
            _temp: temp,
            store,
            journal,
        }
    }

    use crate::domain::{SyncConfig, SyncFileRecord};

    fn config(local_root: &Path) -> SyncConfig {
        SyncConfig {
            device_uuid: "d".to_string(),
            phone_root: "/storage/emulated/0/DCIM/Camera".to_string(),
            local_root: local_root.display().to_string(),
            pc_id: "hs-abc".to_string(),
        }
    }

    fn phone_file_help(path: &str, checksum: &str, ext_data: Option<&str>) -> RemoteFile {
        RemoteFile {
            path: path.to_string(),
            size: 10,
            created_at: None,
            modified_at: Some(1),
            is_directory: false,
            checksum: Some(checksum.to_string()),
            is_trash: None,
            id: None,
            ext_data: ext_data.map(str::to_string),
        }
    }

    #[test]
    fn plan_diffs_added_modified_info_and_deleted() {
        let snapshot = SyncSnapshot {
            files: BTreeMap::from([
                (
                    "/storage/emulated/0/DCIM/Camera/keep.jpg".to_string(),
                    SyncFileRecord {
                        size: 10,
                        checksum: Some("same".to_string()),
                        ext_data: None,
                        modified_at: Some(1),
                        local_path: "/tmp/keep.jpg".to_string(),
                        local_sha256: Some("x".to_string()),
                    },
                ),
                (
                    "/storage/emulated/0/DCIM/Camera/mod.jpg".to_string(),
                    SyncFileRecord {
                        size: 10,
                        checksum: Some("old".to_string()),
                        ext_data: None,
                        modified_at: Some(1),
                        local_path: "/tmp/mod.jpg".to_string(),
                        local_sha256: Some("x".to_string()),
                    },
                ),
                (
                    "/storage/emulated/0/DCIM/Camera/meta.jpg".to_string(),
                    SyncFileRecord {
                        size: 10,
                        checksum: Some("same".to_string()),
                        ext_data: None,
                        modified_at: Some(1),
                        local_path: "/tmp/meta.jpg".to_string(),
                        local_sha256: Some("x".to_string()),
                    },
                ),
                (
                    "/storage/emulated/0/DCIM/Camera/gone.jpg".to_string(),
                    SyncFileRecord {
                        size: 10,
                        checksum: Some("same".to_string()),
                        ext_data: None,
                        modified_at: Some(1),
                        local_path: "/tmp/gone.jpg".to_string(),
                        local_sha256: Some("x".to_string()),
                    },
                ),
            ]),
        };
        let phone = vec![
            phone_file_help("/storage/emulated/0/DCIM/Camera/new.jpg", "c-new", None),
            phone_file_help("/storage/emulated/0/DCIM/Camera/keep.jpg", "same", None),
            phone_file_help(
                "/storage/emulated/0/DCIM/Camera/mod.jpg",
                "new-checksum",
                None,
            ),
            phone_file_help(
                "/storage/emulated/0/DCIM/Camera/meta.jpg",
                "same",
                Some(r#"{"star":true}"#),
            ),
        ];
        let diff = plan_diff(&phone, &snapshot);
        assert_eq!(
            diff.added,
            vec![
                "/storage/emulated/0/DCIM/Camera/new.jpg",
                "/storage/emulated/0/DCIM/Camera/mod.jpg"
            ]
        );
        assert_eq!(
            diff.info_modified,
            vec!["/storage/emulated/0/DCIM/Camera/meta.jpg"]
        );
        assert_eq!(
            diff.deleted,
            vec!["/storage/emulated/0/DCIM/Camera/gone.jpg"]
        );
        assert!(diff.conflicts.is_empty());
    }

    #[test]
    fn local_destination_rejects_escapes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let ok = local_destination(&config, "/storage/emulated/0/DCIM/Camera/a.jpg").expect("ok");
        assert_eq!(ok, temp.path().join("a.jpg"));
        let escape = local_destination(&config, "/storage/emulated/0/DCIM/Camera/../x.jpg");
        assert!(matches!(escape, Err(Error::Protocol(_))));
        let outside = local_destination(&config, "/storage/emulated/0/DCIM/Other/x.jpg");
        assert!(matches!(outside, Err(Error::Protocol(_))));
    }

    #[test]
    fn conflicts_flag_user_modified_local_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local = temp.path().join("edited.jpg");
        fs::write(&local, b"user-modified").expect("write");
        let snapshot = SyncSnapshot {
            files: BTreeMap::from([(
                "/storage/emulated/0/DCIM/Camera/edited.jpg".to_string(),
                SyncFileRecord {
                    size: 10,
                    checksum: Some("c".to_string()),
                    ext_data: None,
                    modified_at: Some(1),
                    local_path: local.display().to_string(),
                    local_sha256: Some(hex_encode(&Sha256::digest(b"original"))),
                },
            )]),
        };
        let diff = SyncDiff {
            added: vec!["/storage/emulated/0/DCIM/Camera/edited.jpg".to_string()],
            ..SyncDiff::default()
        };
        let conflicts = check_conflicts(&diff, &snapshot);
        assert_eq!(
            conflicts,
            vec!["/storage/emulated/0/DCIM/Camera/edited.jpg"]
        );
    }

    #[tokio::test]
    async fn execute_plan_downloads_new_files_and_rerun_is_idempotent() {
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let ledger = test_ledger();
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let snapshot = SyncSnapshot::default();
        let phone = vec![phone_file_help(
            "/storage/emulated/0/DCIM/Camera/a.jpg",
            "c-a",
            None,
        )];
        let diff = plan_diff(&phone, &snapshot);
        let conflicts = check_conflicts(&diff, &snapshot);
        let (result, updated) = execute_plan_with_checkpoint(
            &client,
            &config,
            &phone,
            &diff,
            &snapshot,
            &conflicts,
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("execute");
        assert_eq!(
            result.downloaded,
            vec!["/storage/emulated/0/DCIM/Camera/a.jpg".to_string()],
            "failures: {:?}",
            result.failures
        );
        let local = temp.path().join("a.jpg");
        assert_eq!(fs::read(&local).expect("read"), b"download-data");
        let record = &updated.files["/storage/emulated/0/DCIM/Camera/a.jpg"];
        assert!(record.local_sha256.is_some());
        assert_eq!(record.checksum.as_deref(), Some("c-a"));
        // No part file may linger.
        assert!(!temp.path().join("a.jpg.hs-part").exists());

        // Re-running with the updated ledger produces an empty plan.
        let rerun = plan_diff(&phone, &updated);
        assert!(rerun.added.is_empty());
        assert!(rerun.deleted.is_empty());
        assert!(rerun.info_modified.is_empty());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[test]
    fn plan_diff_checksum_none_to_some_is_a_content_change() {
        // P1-1: the old guard `record.checksum.is_some() &&` treated a
        // record with no checksum as never-content-modified, so a phone
        // that started reporting checksums (None -> Some) would silently
        // skip the re-download. Only "both absent" means unchanged.
        let snapshot = SyncSnapshot {
            files: BTreeMap::from([(
                "/storage/emulated/0/DCIM/Camera/a.jpg".to_string(),
                SyncFileRecord {
                    size: 10,
                    checksum: None, // legacy record without a checksum
                    ext_data: None,
                    modified_at: Some(1),
                    local_path: "/tmp/a.jpg".to_string(),
                    local_sha256: Some("x".to_string()),
                },
            )]),
        };
        let phone = vec![phone_file_help(
            "/storage/emulated/0/DCIM/Camera/a.jpg",
            "now-hashed",
            None,
        )];
        let diff = plan_diff(&phone, &snapshot);
        assert_eq!(
            diff.added,
            vec!["/storage/emulated/0/DCIM/Camera/a.jpg".to_string()],
            "None -> Some checksum must count as a content change"
        );
    }

    #[cfg(unix)]
    #[test]
    fn check_conflicts_fails_closed_on_unreadable_local_file() {
        // P1-1: an existing local file that cannot be read must be treated
        // as a conflict (its content cannot be verified against the
        // ledger), never as "no conflict" — the plan would otherwise
        // overwrite or delete an unverifiable file.
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let local = temp.path().join("locked.jpg");
        std::fs::write(&local, b"user content").expect("write");
        std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o000))
            .expect("chmod 000");

        let snapshot = SyncSnapshot {
            files: BTreeMap::from([(
                "/storage/emulated/0/DCIM/Camera/locked.jpg".to_string(),
                SyncFileRecord {
                    size: 10,
                    checksum: Some("c".to_string()),
                    ext_data: None,
                    modified_at: Some(1),
                    local_path: local.to_string_lossy().into_owned(),
                    local_sha256: Some("expected-sha".to_string()),
                },
            )]),
        };
        let diff = SyncDiff {
            deleted: vec!["/storage/emulated/0/DCIM/Camera/locked.jpg".to_string()],
            ..SyncDiff::default()
        };
        let conflicts = check_conflicts(&diff, &snapshot);
        assert_eq!(
            conflicts,
            vec!["/storage/emulated/0/DCIM/Camera/locked.jpg".to_string()],
            "unreadable local file must fail closed as a conflict"
        );
    }

    #[tokio::test]
    async fn execute_plan_deletes_locally_when_phone_file_disappears() {
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let ledger = test_ledger();
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let phone = vec![phone_file_help(
            "/storage/emulated/0/DCIM/Camera/a.jpg",
            "c-a",
            None,
        )];
        let snapshot = SyncSnapshot::default();
        let (_, updated) = execute_plan_with_checkpoint(
            &client,
            &config,
            &phone,
            &plan_diff(&phone, &snapshot),
            &snapshot,
            &[],
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("first sync");
        let local = temp.path().join("a.jpg");
        assert!(local.exists());

        // Phone no longer has the file: plan deletes it locally.
        let empty: Vec<RemoteFile> = Vec::new();
        let diff = plan_diff(&empty, &updated);
        assert_eq!(
            diff.deleted,
            vec!["/storage/emulated/0/DCIM/Camera/a.jpg".to_string()]
        );
        let (result, final_snapshot) = execute_plan_with_checkpoint(
            &client,
            &config,
            &empty,
            &diff,
            &updated,
            &[],
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("delete run");
        assert_eq!(
            result.deleted,
            vec!["/storage/emulated/0/DCIM/Camera/a.jpg".to_string()]
        );
        assert!(!local.exists());
        assert!(final_snapshot.files.is_empty());
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn execute_plan_preserves_user_modified_local_files_as_conflicts() {
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let ledger = test_ledger();
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let phone = vec![phone_file_help(
            "/storage/emulated/0/DCIM/Camera/a.jpg",
            "c-a",
            None,
        )];
        let snapshot = SyncSnapshot::default();
        let (_, updated) = execute_plan_with_checkpoint(
            &client,
            &config,
            &phone,
            &plan_diff(&phone, &snapshot),
            &snapshot,
            &[],
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("first sync");
        let local = temp.path().join("a.jpg");
        // User edits the file behind the ledger's back.
        fs::write(&local, b"user content").expect("edit");

        // Phone content changes -> plan would re-download, conflict protects it.
        let changed = vec![phone_file_help(
            "/storage/emulated/0/DCIM/Camera/a.jpg",
            "c-b",
            None,
        )];
        let diff = plan_diff(&changed, &updated);
        assert_eq!(
            diff.added,
            vec!["/storage/emulated/0/DCIM/Camera/a.jpg".to_string()]
        );
        let conflicts = check_conflicts(&diff, &updated);
        assert_eq!(
            conflicts,
            vec!["/storage/emulated/0/DCIM/Camera/a.jpg".to_string()]
        );
        let (result, final_snapshot) = execute_plan_with_checkpoint(
            &client,
            &config,
            &changed,
            &diff,
            &updated,
            &conflicts,
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("run");
        assert!(result.downloaded.is_empty());
        assert_eq!(fs::read(&local).expect("read"), b"user content");
        // Ledger unchanged for the conflicted file.
        assert_eq!(
            final_snapshot.files["/storage/emulated/0/DCIM/Camera/a.jpg"]
                .checksum
                .as_deref(),
            Some("c-a")
        );
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn download_one_uses_unique_temp_names_and_leaves_no_partials() {
        // P1-3: `a.jpg` and `a.png` both map to `a.hs-part` under the old
        // naming; both must now download independently (unique random temp
        // files) and no `.hs-part` file may linger after success.
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let jpg = temp.path().join("a.jpg");
        let png = temp.path().join("a.png");
        let (jpg_part, jpg_sha) =
            download_to_staged(&client, "/storage/emulated/0/DCIM/Camera/a.jpg", &jpg)
                .await
                .expect("download jpg");
        let (png_part, png_sha) =
            download_to_staged(&client, "/storage/emulated/0/DCIM/Camera/a.png", &png)
                .await
                .expect("download png");
        // Staged files are distinct (unique random names), and the
        // caller-owned rename yields the final files.
        assert_ne!(jpg_part, png_part);
        fs::rename(&jpg_part, &jpg).expect("rename jpg");
        fs::rename(&png_part, &png).expect("rename png");
        assert_eq!(fs::read(&jpg).expect("read"), b"download-data");
        assert_eq!(fs::read(&png).expect("read"), b"download-data");
        assert!(!jpg_sha.is_empty());
        assert!(!png_sha.is_empty());
        // No partial file may remain in the destination directory.
        let leftovers: Vec<_> = std::fs::read_dir(temp.path())
            .expect("dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".hs-part."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files must be cleaned up: {leftovers:?}"
        );
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn apply_file_change_adds_then_deletes_and_updates_metadata() {
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let ledger = test_ledger();
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let mut snapshot = SyncSnapshot::default();

        // Added: downloads and records the file.
        let added = FileChange {
            file: Some(phone_file_help(
                "/storage/emulated/0/DCIM/Camera/live.jpg",
                "c-live",
                None,
            )),
            status: crate::events::FileChangeStatus::Added,
        };
        let result = apply_file_change_with_checkpoint(
            &client,
            &config,
            &added,
            &mut snapshot,
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("apply added");
        assert_eq!(
            result.downloaded,
            vec!["/storage/emulated/0/DCIM/Camera/live.jpg".to_string()]
        );
        let local = temp.path().join("live.jpg");
        assert_eq!(fs::read(&local).expect("read"), b"download-data");
        assert!(
            snapshot
                .files
                .contains_key("/storage/emulated/0/DCIM/Camera/live.jpg")
        );

        // InfoModified: metadata-only, no download.
        let info = FileChange {
            file: Some(phone_file_help(
                "/storage/emulated/0/DCIM/Camera/live.jpg",
                "c-live",
                Some(r#"{"star":true}"#),
            )),
            status: crate::events::FileChangeStatus::InfoModified,
        };
        let result = apply_file_change_with_checkpoint(
            &client,
            &config,
            &info,
            &mut snapshot,
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("apply info");
        assert!(result.downloaded.is_empty());
        assert_eq!(
            snapshot.files["/storage/emulated/0/DCIM/Camera/live.jpg"]
                .ext_data
                .as_deref(),
            Some(r#"{"star":true}"#)
        );

        // Deleted: removes local file and ledger row.
        let deleted = FileChange {
            file: Some(phone_file_help(
                "/storage/emulated/0/DCIM/Camera/live.jpg",
                "c-live",
                None,
            )),
            status: crate::events::FileChangeStatus::Deleted,
        };
        let result = apply_file_change_with_checkpoint(
            &client,
            &config,
            &deleted,
            &mut snapshot,
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("apply deleted");
        assert_eq!(
            result.deleted,
            vec!["/storage/emulated/0/DCIM/Camera/live.jpg".to_string()]
        );
        assert!(!local.exists());
        assert!(snapshot.files.is_empty());
        client.close().await.expect("close");
        fake.finish().await;
    }

    fn ledger_snapshot_with(path: &str, local_path: &Path) -> SyncSnapshot {
        let mut snapshot = SyncSnapshot::default();
        snapshot.files.insert(
            path.to_string(),
            SyncFileRecord {
                size: 4,
                checksum: Some("c-keep".to_string()),
                ext_data: None,
                modified_at: Some(0),
                local_path: local_path.to_string_lossy().into_owned(),
                local_sha256: Some("deadbeef".to_string()),
            },
        );
        snapshot
    }

    #[tokio::test]
    async fn execute_plan_delete_success_removes_file_and_ledger_row() {
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let ledger = test_ledger();
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let path = "/storage/emulated/0/DCIM/Camera/del.jpg";
        let local = temp.path().join("del.jpg");
        fs::write(&local, b"x").expect("write");
        let snapshot = ledger_snapshot_with(path, &local);
        let diff = SyncDiff {
            deleted: vec![path.to_string()],
            ..SyncDiff::default()
        };
        let (result, updated) = execute_plan_with_checkpoint(
            &client,
            &config,
            &[],
            &diff,
            &snapshot,
            &[],
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("run");
        assert_eq!(result.deleted, vec![path.to_string()]);
        assert!(result.failures.is_empty());
        assert!(!local.exists());
        assert!(!updated.files.contains_key(path));
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn execute_plan_delete_missing_file_drops_row_without_failure() {
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let ledger = test_ledger();
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let path = "/storage/emulated/0/DCIM/Camera/gone.jpg";
        let local = temp.path().join("gone.jpg"); // never created
        let snapshot = ledger_snapshot_with(path, &local);
        let diff = SyncDiff {
            deleted: vec![path.to_string()],
            ..SyncDiff::default()
        };
        let (result, updated) = execute_plan_with_checkpoint(
            &client,
            &config,
            &[],
            &diff,
            &snapshot,
            &[],
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("run");
        assert!(result.deleted.is_empty());
        assert!(result.failures.is_empty());
        assert!(!updated.files.contains_key(path));
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn execute_plan_delete_failure_keeps_file_and_ledger_row() {
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let ledger = test_ledger();
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let path = "/storage/emulated/0/DCIM/Camera/locked.jpg";
        // A directory is not removable with fs::remove_file: the removal
        // fails while the entry still exists — same observable outcome as
        // PermissionDenied (file stays, ledger row must stay, failure listed).
        let local = temp.path().join("locked.jpg");
        fs::create_dir(&local).expect("mkdir");
        let snapshot = ledger_snapshot_with(path, &local);
        let diff = SyncDiff {
            deleted: vec![path.to_string()],
            ..SyncDiff::default()
        };
        let (result, updated) = execute_plan_with_checkpoint(
            &client,
            &config,
            &[],
            &diff,
            &snapshot,
            &[],
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("run");
        assert!(result.deleted.is_empty());
        assert_eq!(result.failures, vec![path.to_string()]);
        assert!(local.exists());
        assert!(updated.files.contains_key(path), "ledger row must survive");
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_plan_delete_permission_denied_keeps_file_and_ledger_row() {
        use std::os::unix::fs::PermissionsExt;

        let ledger = test_ledger();
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let path = "/storage/emulated/0/DCIM/Camera/ro.jpg";
        let readonly = temp.path().join("ro-dir");
        fs::create_dir(&readonly).expect("mkdir");
        let local = readonly.join("ro.jpg");
        fs::write(&local, b"x").expect("write");
        fs::set_permissions(&readonly, fs::Permissions::from_mode(0o555)).expect("chmod");
        let snapshot = ledger_snapshot_with(path, &local);
        let diff = SyncDiff {
            deleted: vec![path.to_string()],
            ..SyncDiff::default()
        };
        let (result, updated) = execute_plan_with_checkpoint(
            &client,
            &config,
            &[],
            &diff,
            &snapshot,
            &[],
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("run");
        assert!(result.deleted.is_empty());
        assert_eq!(result.failures, vec![path.to_string()]);
        assert!(updated.files.contains_key(path), "ledger row must survive");
        client.close().await.expect("close");
        fake.finish().await;
    }

    #[tokio::test]
    async fn apply_file_change_deleted_failure_keeps_row_and_is_retryable() {
        let fake = crate::test_support::FakeWifiSsp::start().await;
        let ledger = test_ledger();
        let client = fake.connect().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config(temp.path());
        let path = "/storage/emulated/0/DCIM/Camera/live-del.jpg";
        // First round: removal fails (path is a directory), row must stay.
        let local = temp.path().join("live-del.jpg");
        fs::create_dir(&local).expect("mkdir");
        let mut snapshot = ledger_snapshot_with(path, &local);
        let deleted = FileChange {
            file: Some(phone_file_help(path, "c-live-del", None)),
            status: crate::events::FileChangeStatus::Deleted,
        };
        let result = apply_file_change_with_checkpoint(
            &client,
            &config,
            &deleted,
            &mut snapshot,
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("apply deleted");
        assert!(result.deleted.is_empty());
        assert_eq!(result.failures, vec![path.to_string()]);
        assert!(snapshot.files.contains_key(path));

        // Second round (e.g. next sync after a restart): the blocking entry
        // is gone, the row is retried and reaches the expected final state.
        // The file no longer exists, so no delete operation is recorded —
        // the ledger row is dropped and the run reports no failure.
        fs::remove_dir(&local).expect("rmdir");
        let result = apply_file_change_with_checkpoint(
            &client,
            &config,
            &deleted,
            &mut snapshot,
            &ledger.journal,
            &ledger.store,
        )
        .await
        .expect("apply deleted retry");
        assert!(result.deleted.is_empty());
        assert!(result.failures.is_empty());
        assert!(!snapshot.files.contains_key(path));
        client.close().await.expect("close");
        fake.finish().await;
    }
}
