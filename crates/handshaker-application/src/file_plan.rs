//! File preflight and execution plans (Phase D / D4).
//!
//! The application layer owns every preflight rule a GUI used to reimplement:
//! source type (file vs directory), recursive requirement, destination
//! existence, overwrite conflicts, multiple sources mapping onto one target,
//! and batch file/directory accounting. GUIs render the plan and the user's
//! choices; execution stays here via [`crate::HandShakerRuntime::execute_file_plan`].

use serde::{Deserialize, Serialize};

use crate::dto::SessionId;

/// Direction of a file operation plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilePlanDirection {
    Upload,
    Download,
}

/// Kind of a preflight conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileConflictKind {
    /// The destination already exists; resolved only by `overwrite`.
    DestinationExists,
    /// File/directory shape of source and destination disagree; never
    /// overridable by overwrite.
    DestinationTypeMismatch,
    /// A directory source requires `recursive` mode.
    RecursiveRequired,
    /// Two sources map to the same destination.
    DuplicateDestination,
    /// A source does not exist (remote for download, local for upload).
    SourceMissing,
    /// The local side is not readable/usable (upload sources).
    LocalPermissionDenied,
}

/// One preflight conflict. `overridable` says whether the user can resolve
/// it by re-running with `recursive`/`overwrite`; `executable` on the plan
/// is false while any non-overridable conflict remains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePlanConflict {
    pub kind: FileConflictKind,
    pub source: String,
    pub destination: String,
    pub message: String,
    pub overridable: bool,
}

/// One planned source/destination pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePlanItem {
    /// Remote path for download, local path for upload.
    pub source: String,
    /// Local path for download, remote path for upload.
    pub destination: String,
    pub is_directory: bool,
    /// File size in bytes; `None` for directories or unknown sizes.
    pub size: Option<u64>,
}

/// A complete preflight plan for one batch operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOperationPlan {
    pub direction: FilePlanDirection,
    pub session_id: SessionId,
    pub items: Vec<FilePlanItem>,
    pub conflicts: Vec<FilePlanConflict>,
    pub file_count: u64,
    pub directory_count: u64,
    /// Sum of the planned file sizes; `None` when sizes are unknown.
    pub total_bytes: Option<u64>,
    /// True when at least one item is a directory (needs tree transfer).
    pub requires_recursive: bool,
    /// False while any non-overridable conflict remains; a true plan still
    /// requires the caller to pass matching `overwrite` at execution time.
    pub executable: bool,
}

/// Preflight a download batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDownloadRequest {
    pub session_id: SessionId,
    pub remote_sources: Vec<String>,
    pub local_destination: String,
    pub recursive: bool,
    pub overwrite: bool,
}

/// Preflight an upload batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanUploadRequest {
    pub session_id: SessionId,
    pub local_sources: Vec<String>,
    pub remote_destination: String,
    pub recursive: bool,
    pub overwrite: bool,
}

/// Execute a preflighted plan as a background transfer; returns the unified
/// transfer id immediately (GUI polls `get_transfer` / `TransferUpdated`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteFilePlanRequest {
    pub plan: FileOperationPlan,
    /// Must be true when the plan contains `DestinationExists` conflicts.
    pub overwrite: bool,
    /// Core batch concurrency (1 = serial; higher parallelizes files).
    pub concurrency: usize,
}

impl FileOperationPlan {
    /// True when the plan can be executed as-is. `DestinationExists` is
    /// overridable by the caller's `overwrite`; `RecursiveRequired` is
    /// overridable because directory items are transferred as trees.
    pub fn is_executable_with(&self, overwrite: bool) -> bool {
        self.conflicts.iter().all(|conflict| {
            if !conflict.overridable {
                return false;
            }
            match conflict.kind {
                FileConflictKind::DestinationExists => overwrite,
                _ => true,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_json_contract_is_stable() {
        let plan = FileOperationPlan {
            direction: FilePlanDirection::Download,
            session_id: SessionId(3),
            items: vec![FilePlanItem {
                source: "/storage/emulated/0/DCIM/a.jpg".to_string(),
                destination: "/tmp/a.jpg".to_string(),
                is_directory: false,
                size: Some(1024),
            }],
            conflicts: vec![FilePlanConflict {
                kind: FileConflictKind::DestinationExists,
                source: "/storage/emulated/0/DCIM/a.jpg".to_string(),
                destination: "/tmp/a.jpg".to_string(),
                message: "destination exists".to_string(),
                overridable: true,
            }],
            file_count: 1,
            directory_count: 0,
            total_bytes: Some(1024),
            requires_recursive: false,
            executable: true,
        };
        let json = serde_json::to_value(&plan).expect("serialize");
        assert_eq!(json["direction"], "download");
        assert_eq!(json["session_id"], 3);
        assert_eq!(json["items"][0]["is_directory"], false);
        assert_eq!(json["conflicts"][0]["kind"], "destination_exists");
        assert_eq!(json["conflicts"][0]["overridable"], true);
        let decoded: FileOperationPlan = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded, plan);
    }

    #[test]
    fn executable_only_depends_on_conflicts_and_overwrite() {
        let plan = FileOperationPlan {
            direction: FilePlanDirection::Upload,
            session_id: SessionId(1),
            items: Vec::new(),
            conflicts: vec![FilePlanConflict {
                kind: FileConflictKind::DestinationExists,
                source: "a".to_string(),
                destination: "b".to_string(),
                message: "exists".to_string(),
                overridable: true,
            }],
            file_count: 0,
            directory_count: 0,
            total_bytes: None,
            requires_recursive: false,
            executable: true,
        };
        assert!(!plan.is_executable_with(false));
        assert!(plan.is_executable_with(true));
    }
}
