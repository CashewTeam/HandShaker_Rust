//! Sync action journal (round-2 P0-2): a small WAL that makes the
//! "local file side effect + ledger update" pair crash-safe.
//!
//! For every download/delete the order is:
//!   download:  staged fsync → journal → rename staged→final → ledger save → clear journal
//!   delete:    journal → rename file→trash → ledger save → remove trash → clear journal
//!
//! A crash (or hard abort) at any point leaves at most one pending action
//! in the journal; `recover` replays it deterministically on the next run:
//!   - download with final present → record the file, save, clear;
//!   - download with staged present → finish the rename, record, save, clear;
//!   - download with neither → the transfer never landed, drop the action;
//!   - delete with the original still present → rename never happened,
//!     drop the action (retried next run);
//!   - delete with trash present → finish the removal, update the ledger,
//!     save, clear;
//!   - delete with neither → file is gone, update the ledger, save, clear.
//!
//! `recover` never guesses: it only ever completes the exact action that
//! was journaled, so a user-modified file that was never touched by this
//! sync can never be adopted or overwritten.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::domain::{SyncFileRecord, SyncSnapshot};
use crate::error::{Error, Result};
use crate::i18n;

/// One pending file side effect. Serialized to `<ledger>.journal.json`
/// before the irreversible rename/remove step.
#[derive(serde::Serialize, serde::Deserialize)]
pub enum PendingSyncAction {
    Download {
        remote_path: String,
        /// Destination the staged file was going to be renamed to.
        final_path: PathBuf,
        /// Fully downloaded (and fsynced) temp file; may be missing when
        /// the crash happened before the transfer completed.
        staged_path: PathBuf,
        /// The exact ledger record to insert once the file is final.
        new_record: SyncFileRecord,
    },
    Delete {
        remote_path: String,
        /// Original synced file; still present when the rename never ran.
        original_path: PathBuf,
        /// Same-directory trash name the file was (or will be) renamed to.
        trash_path: PathBuf,
    },
}

/// Journal file next to its ledger: `<sync>/<ledger-key>.journal.json`.
/// Single-action (one entry per committed side effect) — recovery is
/// therefore deterministic and idempotent.
pub struct SyncJournal {
    path: PathBuf,
}

impl SyncJournal {
    /// Journal path for a ledger file.
    pub fn for_ledger(ledger_path: &Path) -> Self {
        let mut name = ledger_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ledger".to_string());
        name.push_str(".journal");
        let path = ledger_path.with_file_name(name);
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Persist the pending action (temp + fsync + rename, same atomic
    /// discipline as the ledger).
    pub fn write(&self, action: &PendingSyncAction) -> Result<()> {
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
        let bytes = serde_json::to_vec_pretty(action).map_err(|error| {
            Error::Configuration(i18n::format("sync.serialize_failed", &[&error.to_string()]))
        })?;
        let tmp = self
            .path
            .with_extension(format!("tmp-{}", rand::random::<u64>()));
        let mut file = fs::File::create(&tmp).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.write_failed",
                &[&tmp.display().to_string(), &error.to_string()],
            ))
        })?;
        file.write_all(&bytes).map_err(|error| {
            let _ = fs::remove_file(&tmp);
            Error::Configuration(i18n::format(
                "sync.write_failed",
                &[&tmp.display().to_string(), &error.to_string()],
            ))
        })?;
        // fsync the journal before the irreversible file step; a sync
        // failure propagates (a truncated journal would hard-block every
        // later run).
        file.sync_all().map_err(|error| {
            let _ = fs::remove_file(&tmp);
            Error::Configuration(i18n::format(
                "sync.write_failed",
                &[&tmp.display().to_string(), &error.to_string()],
            ))
        })?;
        // Sensitive journal (paths, ledger state): 0600 like the ledger.
        #[cfg(unix)]
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(|error| {
            let _ = fs::remove_file(&tmp);
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
        // Best-effort directory fsync so the rename itself is durable.
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    /// The pending action, or `None` when the journal is empty/absent.
    /// A corrupt journal is a hard error (never silently dropped — the
    /// file side effect may be half-applied).
    pub fn read(&self) -> Result<Option<PendingSyncAction>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.read_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        let action: PendingSyncAction = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Configuration(i18n::format(
                "sync.parse_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))
        })?;
        Ok(Some(action))
    }

    /// Remove the journal after the side effect and ledger are both
    /// committed.
    pub fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::Configuration(i18n::format(
                "sync.write_failed",
                &[&self.path.display().to_string(), &error.to_string()],
            ))),
        }
    }

    /// Replay a pending action against the ledger snapshot, if any.
    /// Returns the possibly-updated snapshot. Never touches files outside
    /// the journaled paths; a failure (rename/remove/save) keeps the
    /// journal so the next run retries.
    /// Journaled paths must be absolute and contain no `..` — the journal
    /// lives in the state dir and could be tampered with by anything that
    /// can write there; `recover` must never touch files outside the
    /// journaled paths (round-2 hardening).
    fn path_is_safe(path: &Path) -> bool {
        path.is_absolute()
            && !path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
    }

    pub fn recover(
        &self,
        snapshot: &SyncSnapshot,
        save: impl FnOnce(&SyncSnapshot) -> Result<()>,
    ) -> Result<SyncSnapshot> {
        let Some(action) = self.read()? else {
            return Ok(snapshot.clone());
        };
        // Reject tampered/foreign journals before touching anything.
        match &action {
            PendingSyncAction::Download {
                final_path,
                staged_path,
                ..
            } => {
                if !Self::path_is_safe(final_path) || !Self::path_is_safe(staged_path) {
                    return Err(Error::Configuration(
                        i18n::text("sync.journal_unsafe").to_string(),
                    ));
                }
            }
            PendingSyncAction::Delete {
                original_path,
                trash_path,
                ..
            } => {
                if !Self::path_is_safe(original_path) || !Self::path_is_safe(trash_path) {
                    return Err(Error::Configuration(
                        i18n::text("sync.journal_unsafe").to_string(),
                    ));
                }
            }
        }
        let mut updated = snapshot.clone();
        match action {
            PendingSyncAction::Download {
                remote_path,
                final_path,
                staged_path,
                new_record,
            } => {
                if !final_path.exists() && staged_path.exists() {
                    // The rename never happened; finish it now.
                    if let Some(parent) = final_path.parent() {
                        fs::create_dir_all(parent).map_err(|error| {
                            Error::LocalIo(i18n::format(
                                "sync.local_dir_failed",
                                &[&parent.display().to_string(), &error.to_string()],
                            ))
                        })?;
                    }
                    fs::rename(&staged_path, &final_path).map_err(|error| {
                        // Keep the staged file: a transient failure
                        // (permissions, destination-is-a-directory) must
                        // not lose the download — the next recovery
                        // retries the rename or drops it if the caller
                        // cleaned up.
                        Error::LocalIo(i18n::format(
                            "sync.rename_failed",
                            &[&final_path.display().to_string(), &error.to_string()],
                        ))
                    })?;
                }
                if final_path.exists() {
                    updated.files.insert(remote_path, new_record);
                    save(&updated)?;
                    self.clear()?;
                } else {
                    // Neither staged nor final: the transfer never landed.
                    self.clear()?;
                }
            }
            PendingSyncAction::Delete {
                remote_path,
                original_path,
                trash_path,
            } => {
                if original_path.exists() {
                    // Rename never happened (or the file was recreated
                    // after a crash): a leftover trash from an earlier
                    // crash is an orphan — remove it, keep the original,
                    // and let the next sync retry the delete.
                    if trash_path.exists() {
                        fs::remove_file(&trash_path).map_err(|error| {
                            Error::LocalIo(i18n::format(
                                "sync.delete_failed",
                                &[&trash_path.display().to_string(), &error.to_string()],
                            ))
                        })?;
                    }
                    self.clear()?;
                } else if trash_path.exists() {
                    fs::remove_file(&trash_path).map_err(|error| {
                        Error::LocalIo(i18n::format(
                            "sync.delete_failed",
                            &[&trash_path.display().to_string(), &error.to_string()],
                        ))
                    })?;
                    updated.files.remove(&remote_path);
                    save(&updated)?;
                    self.clear()?;
                } else {
                    // File is gone (removed externally or fully deleted
                    // before the crash): drop the ledger row too.
                    updated.files.remove(&remote_path);
                    save(&updated)?;
                    self.clear()?;
                }
            }
        }
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_store::{SyncLedgerIdentity, SyncStore};
    use std::path::PathBuf;

    fn store_at(temp: &tempfile::TempDir) -> SyncStore {
        SyncStore::discover(
            temp.path(),
            &SyncLedgerIdentity {
                device_uuid: "dev".to_string(),
                remote_root: "/DCIM".to_string(),
                local_root: "/tmp".to_string(),
            },
        )
    }

    fn save_ok(_snapshot: &SyncSnapshot) -> Result<()> {
        Ok(())
    }

    fn record(path: &str) -> SyncFileRecord {
        SyncFileRecord {
            size: 10,
            checksum: None,
            ext_data: None,
            modified_at: None,
            local_path: path.to_string(),
            local_sha256: None,
        }
    }

    fn snapshot_with(path: &str, local: &str) -> SyncSnapshot {
        let mut snapshot = SyncSnapshot::default();
        snapshot.files.insert(path.to_string(), record(local));
        snapshot
    }

    #[test]
    fn empty_journal_recover_is_noop() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let snapshot = snapshot_with("/a.jpg", "/tmp/a.jpg");
        let recovered = journal.recover(&snapshot, save_ok).expect("recover no-op");
        assert_eq!(recovered, snapshot);
        assert!(!journal.path().exists());
    }

    // Scenario: journal written (download) but rename never happened →
    // recover finishes the rename and records the file.
    #[test]
    fn download_recover_finishes_rename_and_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let final_path = temp.path().join("a.jpg");
        let staged = temp.path().join("a.jpg.hs-part.1");
        fs::write(&staged, b"data").expect("staged");
        let action = PendingSyncAction::Download {
            remote_path: "/a.jpg".to_string(),
            final_path: final_path.clone(),
            staged_path: staged.clone(),
            new_record: record("/a.jpg"),
        };
        journal.write(&action).expect("journal");
        // "Crash": rename + save never ran.
        let snapshot = SyncSnapshot::default();
        let recovered = journal
            .recover(&snapshot, |updated| store.save(updated))
            .expect("recover");
        assert!(final_path.exists(), "rename replayed");
        assert!(!staged.exists());
        assert!(recovered.files.contains_key("/a.jpg"));
        assert!(!journal.path().exists(), "journal cleared");
        // Ledger persisted: a fresh load sees the record.
        let persisted = store.load().expect("load").expect("some");
        assert!(persisted.files.contains_key("/a.jpg"));
    }

    // Scenario: rename happened, ledger save crashed → recover records idempotently.
    #[test]
    fn download_recover_after_rename_records_without_touching_final() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let final_path = temp.path().join("a.jpg");
        fs::write(&final_path, b"final").expect("final");
        let action = PendingSyncAction::Download {
            remote_path: "/a.jpg".to_string(),
            final_path: final_path.clone(),
            staged_path: temp.path().join("gone-part"),
            new_record: record("/a.jpg"),
        };
        journal.write(&action).expect("journal");
        let recovered = journal
            .recover(&SyncSnapshot::default(), |updated| store.save(updated))
            .expect("recover");
        assert_eq!(fs::read(&final_path).unwrap(), b"final", "final untouched");
        assert!(recovered.files.contains_key("/a.jpg"));
        assert!(!journal.path().exists());
    }

    // Scenario: download never landed (staged and final both missing) →
    // recover drops the action; no phantom record.
    #[test]
    fn download_recover_drops_unlanded_action() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let action = PendingSyncAction::Download {
            remote_path: "/a.jpg".to_string(),
            final_path: temp.path().join("a.jpg"),
            staged_path: temp.path().join("never-was"),
            new_record: record("/a.jpg"),
        };
        journal.write(&action).expect("journal");
        let recovered = journal
            .recover(&SyncSnapshot::default(), save_ok)
            .expect("recover");
        assert!(recovered.files.is_empty());
        assert!(!temp.path().join("a.jpg").exists());
        assert!(!journal.path().exists());
    }

    // Scenario: delete journaled but rename never happened → recover
    // abandons the action; file and row both survive.
    #[test]
    fn delete_recover_abandons_before_rename() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let local = temp.path().join("a.jpg");
        fs::write(&local, b"data").expect("file");
        let action = PendingSyncAction::Delete {
            remote_path: "/a.jpg".to_string(),
            original_path: local.clone(),
            trash_path: temp.path().join("a.jpg.hs-trash.1"),
        };
        journal.write(&action).expect("journal");
        let snapshot = snapshot_with("/a.jpg", &local.display().to_string());
        let recovered = journal.recover(&snapshot, save_ok).expect("recover");
        assert!(local.exists(), "file untouched");
        assert!(recovered.files.contains_key("/a.jpg"), "row kept");
        assert!(!journal.path().exists());
    }

    // Scenario: delete rename happened, ledger save crashed → recover
    // removes trash and drops the row.
    #[test]
    fn delete_recover_after_rename_finishes_removal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let trash = temp.path().join("a.jpg.hs-trash.1");
        fs::write(&trash, b"data").expect("trash");
        let action = PendingSyncAction::Delete {
            remote_path: "/a.jpg".to_string(),
            original_path: temp.path().join("a.jpg"),
            trash_path: trash.clone(),
        };
        journal.write(&action).expect("journal");
        let snapshot = snapshot_with("/a.jpg", "/tmp/a.jpg");
        let recovered = journal
            .recover(&snapshot, |updated| store.save(updated))
            .expect("recover");
        assert!(!trash.exists(), "trash removed");
        assert!(!recovered.files.contains_key("/a.jpg"));
        assert!(!journal.path().exists());
    }

    // Scenario: file gone entirely (deleted externally or fully removed)
    // → recover drops the ledger row.
    #[test]
    fn delete_recover_with_neither_file_drops_row() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let action = PendingSyncAction::Delete {
            remote_path: "/a.jpg".to_string(),
            original_path: temp.path().join("a.jpg"),
            trash_path: temp.path().join("a.jpg.hs-trash.1"),
        };
        journal.write(&action).expect("journal");
        let snapshot = snapshot_with("/a.jpg", "/tmp/a.jpg");
        let recovered = journal
            .recover(&snapshot, |updated| store.save(updated))
            .expect("recover");
        assert!(!recovered.files.contains_key("/a.jpg"));
        assert!(!journal.path().exists());
    }

    // Scenario 8 (audit): ledger save fails during recovery → error
    // propagates and the journal is kept for the next run.
    #[test]
    fn recover_save_failure_keeps_journal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let final_path = temp.path().join("a.jpg");
        fs::write(&final_path, b"final").expect("final");
        let action = PendingSyncAction::Download {
            remote_path: "/a.jpg".to_string(),
            final_path: final_path.clone(),
            staged_path: PathBuf::from("gone"),
            new_record: record("/a.jpg"),
        };
        journal.write(&action).expect("journal");
        // Force save failure: make the ledger path a directory.
        std::fs::create_dir_all(store.path()).expect("dir-as-ledger");
        let error = journal
            .recover(&SyncSnapshot::default(), |updated| store.save(updated))
            .expect_err("save must fail");
        assert!(matches!(error, Error::Configuration(_)));
        assert!(journal.path().exists(), "journal retained for retry");
    }
    // Round-2 hardening: a transient rename failure must keep the staged
    // file (recovery retries), not delete the download.
    #[test]
    fn download_recover_rename_failure_keeps_staged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        // Make the rename fail: the destination directory is read-only.
        let ro_dir = temp.path().join("ro-dir");
        fs::create_dir_all(&ro_dir).expect("dir");
        #[cfg(unix)]
        fs::set_permissions(&ro_dir, fs::Permissions::from_mode(0o555)).expect("perms");
        let final_path = ro_dir.join("a.jpg");
        let staged = temp.path().join("a.jpg.hs-part.1");
        fs::write(&staged, b"data").expect("staged");
        let action = PendingSyncAction::Download {
            remote_path: "/a.jpg".to_string(),
            final_path: final_path.clone(),
            staged_path: staged.clone(),
            new_record: record("/a.jpg"),
        };
        journal.write(&action).expect("journal");
        let error = journal
            .recover(&SyncSnapshot::default(), save_ok)
            .expect_err("rename must fail");
        assert!(matches!(error, Error::LocalIo(_)));
        assert!(staged.exists(), "staged must survive a failed replay");
        assert!(journal.path().exists(), "journal kept for retry");
        #[cfg(unix)]
        fs::set_permissions(&ro_dir, fs::Permissions::from_mode(0o755)).expect("restore perms");
    }

    // Round-2 hardening: original recreated after a crash → the leftover
    // trash is an orphan and must be removed while the original survives.
    #[test]
    fn delete_recover_removes_orphan_trash_when_original_recreated() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let original = temp.path().join("a.jpg");
        let trash = temp.path().join("a.jpg.hs-trash.1");
        fs::write(&original, b"new").expect("original recreated");
        fs::write(&trash, b"old").expect("orphan trash");
        let action = PendingSyncAction::Delete {
            remote_path: "/a.jpg".to_string(),
            original_path: original.clone(),
            trash_path: trash.clone(),
        };
        journal.write(&action).expect("journal");
        let snapshot = snapshot_with("/a.jpg", &original.display().to_string());
        let recovered = journal.recover(&snapshot, save_ok).expect("recover");
        assert!(original.exists(), "recreated original untouched");
        assert!(!trash.exists(), "orphan trash removed");
        assert!(recovered.files.contains_key("/a.jpg"), "row kept for retry");
        assert!(!journal.path().exists());
    }

    // Round-2 hardening: a tampered journal with a relative path is
    // rejected without touching anything.
    #[test]
    fn recover_rejects_unsafe_journal_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store_at(&temp);
        let journal = SyncJournal::for_ledger(store.path());
        let action = PendingSyncAction::Download {
            remote_path: "/a.jpg".to_string(),
            final_path: PathBuf::from("../escape/a.jpg"),
            staged_path: temp.path().join("a.jpg.hs-part.1"),
            new_record: record("/a.jpg"),
        };
        journal.write(&action).expect("journal");
        let error = journal
            .recover(&SyncSnapshot::default(), save_ok)
            .expect_err("unsafe path must be rejected");
        assert!(matches!(error, Error::Configuration(_)));
        assert!(journal.path().exists(), "journal kept for inspection");
    }
}
