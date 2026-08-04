//! Photo-sync service models and pure mappings (Phase D / D6).
//!
//! The public surface is DTO-only: `SyncStore`, `SyncSnapshot`, `RemoteFile`,
//! `SyncConfig`, `SyncDiff` and `SyncRunResult` never cross the application
//! boundary — every public method on `HandShakerRuntime` returns one of the
//! DTOs below. Runtime lifecycle (plan/run/status/watch) lives in
//! `crate::runtime`.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use handshaker_core::{
    FileChange, FileChangeStatus, RemoteFile, SyncConfig, SyncDiff, SyncSnapshot,
};

use crate::dto::SessionId;
use crate::error::{AppResult, PublicError, from_core_error};

// ---- DTOs ----

/// One sync profile: what to sync, where, and over which session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncProfileDto {
    /// Stable caller-chosen id; also the sync-jobs registry key.
    pub id: String,
    /// Session whose phone provides the files.
    pub session_id: SessionId,
    /// Stable phone identifier keying the ledger file
    /// (`<state_dir>/sync/<device_uuid>.json`).
    pub device_uuid: String,
    /// Phone-side root folder to sync.
    pub remote_root: String,
    /// Local destination directory for downloaded files.
    pub local_root: String,
    pub enabled: bool,
}

/// Preview of one sync run; no core types leaked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncPlanDto {
    pub profile_id: String,
    pub downloads: Vec<SyncActionDto>,
    pub metadata_updates: Vec<SyncActionDto>,
    pub deletions: Vec<SyncActionDto>,
    pub conflicts: Vec<SyncConflictDto>,
    pub total_bytes: u64,
    /// `false` when local conflicts would be clobbered; such a plan must not
    /// be executed (`start_sync` refuses it).
    pub executable: bool,
}

/// One planned file action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncActionDto {
    pub remote_path: String,
    pub local_path: String,
    pub size: u64,
}

/// One local conflict: the local file was preserved, not overwritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncConflictDto {
    pub remote_path: String,
    pub local_path: String,
    /// Stable token explaining the conflict (`local_modified`).
    pub reason: String,
}

/// Live status of a registered sync job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncStatusDto {
    pub profile_id: String,
    pub running: bool,
    pub monitoring: bool,
    pub last_run_at_ms: Option<u64>,
    pub last_error: Option<PublicError>,
    /// P1-2: set when the watch observed a sequence gap (Lagged) or a
    /// batch apply/commit failure — the incremental ledger can no longer
    /// be proven complete. `start_sync_watch` refuses to restart until a
    /// full sync succeeds (which clears the flag).
    #[serde(default)]
    pub reconciliation_required: bool,
    /// Most recently observed sequence gap (missed event count), if any.
    #[serde(default)]
    pub last_sequence_gap: Option<u64>,
}

/// Result of one executed sync run. Field names mirror the CLI `sync run`
/// JSON contract (`downloaded`/`deleted`/`failures`/`conflicts`), so a
/// migrated CLI can keep its output schema unchanged (hard constraint).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncRunResultDto {
    pub downloaded: Vec<String>,
    pub deleted: Vec<String>,
    pub failures: Vec<String>,
    pub conflicts: Vec<String>,
}

/// Ledger summary for the `sync status` command: how many files and bytes
/// the local ledger tracks for one device. Read without a session (the
/// ledger is local state); the ledger path follows the configured
/// `state_dir`, byte-compatible with the legacy CLI location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncLedgerStatusDto {
    pub device_uuid: String,
    /// Normalized remote root of the ledger scope (round-2 P0-1; `None`
    /// only for legacy callers that never send it).
    #[serde(default)]
    pub remote_root: Option<String>,
    /// Normalized local root of the ledger scope (round-2 P0-1).
    #[serde(default)]
    pub local_root: Option<String>,
    pub files: u64,
    pub bytes: u64,
}

// ---- job registry entry ----

/// One registered sync job per profile id. The run task and the watch task
/// are mutually exclusive on the same job: `start_sync` while watching and
/// `start_sync_watch` while running are both rejected. `cancel` is the
/// run-side cancellation signal; the watch task blocks on the core event
/// stream with no cooperative cancellation point, so it is aborted instead.
#[derive(Debug)]
pub(crate) struct SyncJob {
    pub profile: SyncProfileDto,
    pub cancel: handshaker_core::CancellationToken,
    /// Per-ledger write mutex for this job's scope (round-2 P0-1): held
    /// for the whole run/watch lifetime so two profiles that resolve to
    /// the same ledger serialize, and so the ledger is never written by
    /// an aborted task's tail.
    pub ledger_lock: Arc<tokio::sync::Mutex<()>>,
    /// Live run task (cleared by the task itself when it finishes).
    pub task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Live watch task (taken by `stop_sync_watch` / shutdown).
    pub watch_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub status: std::sync::RwLock<SyncStatusDto>,
    /// Most recent completed run (or watch batch) result.
    pub last_result: std::sync::RwLock<Option<SyncRunResultDto>>,
}

impl SyncJob {
    pub(crate) fn new(profile: SyncProfileDto, ledger_lock: Arc<tokio::sync::Mutex<()>>) -> Self {
        let profile_id = profile.id.clone();
        Self {
            profile,
            cancel: handshaker_core::CancellationToken::new(),
            ledger_lock,
            task: tokio::sync::Mutex::new(None),
            watch_task: tokio::sync::Mutex::new(None),
            status: std::sync::RwLock::new(SyncStatusDto {
                profile_id,
                running: false,
                monitoring: false,
                last_run_at_ms: None,
                last_error: None,
                reconciliation_required: false,
                last_sequence_gap: None,
            }),
            last_result: std::sync::RwLock::new(None),
        }
    }

    pub(crate) fn status(&self) -> SyncStatusDto {
        self.status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn set_status(&self, update: impl FnOnce(&mut SyncStatusDto)) {
        let mut guard = self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut guard);
    }

    pub(crate) fn last_result(&self) -> Option<SyncRunResultDto> {
        self.last_result
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn set_last_result(&self, result: SyncRunResultDto) {
        let mut guard = self
            .last_result
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(result);
    }
}

// ---- pure mappings ----

/// Ledger snapshot -> the `RemoteFile` list sent as the previous-snapshot
/// argument of PHOTO_SYNC_REQUEST(37). Mirrors the CLI's ledger projection;
/// `created_at`/`is_trash`/`id` are unknown for recorded entries.
pub(crate) fn snapshot_to_remote_files(snapshot: &SyncSnapshot) -> Vec<RemoteFile> {
    snapshot
        .files
        .iter()
        .map(|(path, record)| RemoteFile {
            path: path.clone(),
            size: record.size,
            created_at: None,
            modified_at: record.modified_at,
            is_directory: false,
            checksum: record.checksum.clone(),
            is_trash: None,
            id: None,
            ext_data: record.ext_data.clone(),
        })
        .collect()
}

/// Map a core diff (+ conflicts) onto the public plan DTO. Local paths are
/// resolved with the core `local_destination` rule (never copied here).
pub(crate) fn sync_plan_to_dto(
    profile_id: &str,
    config: &SyncConfig,
    diff: &SyncDiff,
    conflicts: &[String],
    phone_files: &[RemoteFile],
    snapshot: &SyncSnapshot,
) -> AppResult<SyncPlanDto> {
    let phone_by_path: std::collections::BTreeMap<&str, &RemoteFile> = phone_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let mut downloads = Vec::new();
    let mut metadata_updates = Vec::new();
    let mut deletions = Vec::new();
    let mut conflict_items = Vec::new();
    let mut total_bytes = 0u64;

    for path in &diff.added {
        let Some(file) = phone_by_path.get(path.as_str()) else {
            continue;
        };
        let local_path = handshaker_core::local_destination(config, path)
            .map_err(|error| from_core_error(error, "sync.plan"))?
            .display()
            .to_string();
        total_bytes += file.size;
        downloads.push(SyncActionDto {
            remote_path: path.clone(),
            local_path,
            size: file.size,
        });
    }
    for path in &diff.info_modified {
        let Some(file) = phone_by_path.get(path.as_str()) else {
            continue;
        };
        let local_path = handshaker_core::local_destination(config, path)
            .map_err(|error| from_core_error(error, "sync.plan"))?
            .display()
            .to_string();
        metadata_updates.push(SyncActionDto {
            remote_path: path.clone(),
            local_path,
            size: file.size,
        });
    }
    for path in &diff.deleted {
        let record = snapshot.files.get(path);
        deletions.push(SyncActionDto {
            remote_path: path.clone(),
            local_path: match record {
                Some(record) => record.local_path.clone(),
                None => String::new(),
            },
            size: record.map(|record| record.size).unwrap_or(0),
        });
    }
    for path in conflicts {
        let record = snapshot.files.get(path);
        conflict_items.push(SyncConflictDto {
            remote_path: path.clone(),
            local_path: match record {
                Some(record) => record.local_path.clone(),
                None => String::new(),
            },
            reason: "local_modified".to_string(),
        });
    }

    Ok(SyncPlanDto {
        profile_id: profile_id.to_string(),
        downloads,
        metadata_updates,
        deletions,
        conflicts: conflict_items,
        total_bytes,
        executable: conflicts.is_empty(),
    })
}

/// Map a core run result onto the CLI-compatible result DTO.
pub(crate) fn sync_run_result_to_dto(result: handshaker_core::SyncRunResult) -> SyncRunResultDto {
    SyncRunResultDto {
        downloaded: result.downloaded,
        deleted: result.deleted,
        failures: result.failures,
        conflicts: result.conflicts,
    }
}

/// One-entry diff for the watch path's conflict pre-check: reuses the core
/// `check_conflicts` instead of reimplementing the local-SHA-256 logic.
/// Metadata-only statuses produce an empty diff (no local-file risk).
pub(crate) fn one_entry_diff(change: &FileChange) -> SyncDiff {
    let mut diff = SyncDiff::default();
    let Some(file) = change.file.as_ref() else {
        return diff;
    };
    if file.is_directory {
        return diff;
    }
    match change.status {
        FileChangeStatus::Added
        | FileChangeStatus::Modified
        | FileChangeStatus::FileAndInfoModified => diff.added.push(file.path.clone()),
        FileChangeStatus::Deleted => diff.deleted.push(file.path.clone()),
        // InfoModified / None / Unknown touch only ledger metadata, which
        // never overwrites or removes a local file.
        _ => {}
    }
    diff
}
