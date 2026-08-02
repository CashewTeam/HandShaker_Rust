use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

pub const DEFAULT_LANGUAGE: &str = "zh-CN";

#[derive(Debug, Clone, Copy)]
pub enum MessageKey {
    WireLogWarning,
    NoDevices,
    NoWifiDevices,
    WifiDeviceListHeader,
    DeviceListHeader,
    FileListHeader,
    ClipboardHeader,
    ShellWelcome,
    ShellBye,
    Yes,
    No,
    ShellOnlyHuman,
    PingResult,
    FileCount,
    Exists,
    Missing,
    DirectoryCreated,
    RenameDone,
    DeletedCount,
    DownloadDone,
    UploadDone,
    ClipboardWritten,
    ClipboardDeleted,
    ClipboardCleared,
    ShellNoStdin,
    ClipboardSetRequired,
    ShellNested,
    ShellHelp,
    Error,
    CommandParseError,
    RemoteNotDirectory,
    LocalNotDirectory,
    ConfirmationRequired,
    UserNotConfirmed,
    Download,
    Upload,
    Progress,
    Directory,
    File,
    DeviceInfo,
    RemoteMissing,
    DeleteRecursiveRequired,
    DeleteAction,
    LocalTargetExists,
    ReadDirFailed,
    DownloadPathEscape,
    UpdateFileInfoFailed,
    DuplicateTarget,
    PullNeedsSeparator,
    BatchConcurrencyRange,
    OverwriteLocalAction,
    RemoteTargetExists,
    RecursiveRequired,
    BatchDone,
    BatchFailures,
    OverwriteLocalBatch,
    OverwriteRemoteBatch,
    BatchProgress,
    OverwriteRemoteAction,
    DeleteClipboardAction,
    ClearClipboardAction,
    RemoteNameMissing,
    DryRunReport,
    InvalidDuration,
    WifiTrustHint,
    TrustNone,
    WatchRegistered,
    WatchLagged,
    WatchDisconnected,
    WatchNested,
    ExifNotImplemented,
    ExifParseFailed,
    ExifFileTooLarge,
    MediaChangeKindMismatch,
    MediaPreviewTruncated,
    MediaThumbnailWritten,
    MediaThumbnailPartial,
    MediaThumbnailFailed,
    MediaThumbnailInvalidId,
    TrustListHeader,
    TrustRemoveAction,
    TrustMissing,
    TrustRemoved,
    TrustResetNeedsWifi,
    TrustResetAction,
    TrustResetDone,
}

impl MessageKey {
    fn as_str(self) -> &'static str {
        match self {
            Self::WireLogWarning => "wire_log.warning",
            Self::NoDevices => "device.none",
            Self::NoWifiDevices => "device.wifi_none",
            Self::WifiDeviceListHeader => "device.wifi_list_header",
            Self::DeviceListHeader => "device.list_header",
            Self::FileListHeader => "file.list_header",
            Self::ClipboardHeader => "clipboard.header",
            Self::ShellWelcome => "shell.welcome",
            Self::ShellBye => "shell.bye",
            Self::Yes => "value.yes",
            Self::No => "value.no",
            Self::ShellOnlyHuman => "shell.human_only",
            Self::PingResult => "device.ping_result",
            Self::FileCount => "file.count",
            Self::Exists => "file.exists",
            Self::Missing => "file.missing",
            Self::DirectoryCreated => "file.directory_created",
            Self::RenameDone => "file.rename_done",
            Self::DeletedCount => "file.deleted_count",
            Self::DownloadDone => "file.download_done",
            Self::UploadDone => "file.upload_done",
            Self::ClipboardWritten => "clipboard.written",
            Self::ClipboardDeleted => "clipboard.deleted",
            Self::ClipboardCleared => "clipboard.cleared",
            Self::ShellNoStdin => "shell.no_stdin",
            Self::ClipboardSetRequired => "clipboard.set_required",
            Self::ShellNested => "shell.nested",
            Self::ShellHelp => "shell.help",
            Self::Error => "error.display",
            Self::CommandParseError => "shell.parse_error",
            Self::RemoteNotDirectory => "shell.remote_not_directory",
            Self::LocalNotDirectory => "shell.local_not_directory",
            Self::ConfirmationRequired => "confirm.required_hint",
            Self::UserNotConfirmed => "confirm.declined",
            Self::Download => "transfer.download",
            Self::Upload => "transfer.upload",
            Self::Progress => "transfer.progress",
            Self::Directory => "file.directory",
            Self::File => "file.file",
            Self::DeviceInfo => "device.info",
            Self::RemoteMissing => "file.remote_missing",
            Self::DeleteRecursiveRequired => "file.recursive_required",
            Self::DeleteAction => "file.delete_action",
            Self::LocalTargetExists => "file.local_exists",
            Self::ReadDirFailed => "client.read_dir_failed",
            Self::DownloadPathEscape => "client.download_path_escape",
            Self::UpdateFileInfoFailed => "client.update_file_info_failed",
            Self::DuplicateTarget => "cli.duplicate_target",
            Self::PullNeedsSeparator => "cli.pull_needs_separator",
            Self::BatchConcurrencyRange => "client.batch_concurrency_range",
            Self::OverwriteLocalAction => "file.overwrite_local_action",
            Self::RemoteTargetExists => "file.remote_exists",
            Self::RecursiveRequired => "cli.recursive_required",
            Self::BatchDone => "file.batch_done",
            Self::BatchFailures => "file.batch_failures",
            Self::OverwriteLocalBatch => "file.overwrite_local_batch",
            Self::OverwriteRemoteBatch => "file.overwrite_remote_batch",
            Self::BatchProgress => "file.batch_progress",
            Self::OverwriteRemoteAction => "file.overwrite_remote_action",
            Self::DeleteClipboardAction => "clipboard.delete_action",
            Self::ClearClipboardAction => "clipboard.clear_action",
            Self::RemoteNameMissing => "file.remote_name_missing",
            Self::DryRunReport => "cli.dry_run_report",
            Self::InvalidDuration => "duration.invalid",
            Self::WifiTrustHint => "wifi.trust_hint",
            Self::TrustNone => "trust.none",
            Self::WatchRegistered => "watch.registered",
            Self::WatchLagged => "watch.lagged",
            Self::WatchDisconnected => "watch.disconnected",
            Self::WatchNested => "watch.nested",
            Self::ExifNotImplemented => "exif.not_implemented",
            Self::ExifParseFailed => "exif.parse_failed",
            Self::ExifFileTooLarge => "exif.file_too_large",
            Self::MediaChangeKindMismatch => "media.change_kind_mismatch",
            Self::MediaPreviewTruncated => "media.preview_truncated",
            Self::MediaThumbnailWritten => "media.thumbnail_written",
            Self::MediaThumbnailPartial => "media.thumbnail_partial",
            Self::MediaThumbnailFailed => "media.thumbnail_failed",
            Self::MediaThumbnailInvalidId => "media.thumbnail_invalid_id",
            Self::TrustListHeader => "trust.list_header",
            Self::TrustRemoveAction => "trust.remove_action",
            Self::TrustMissing => "trust.missing",
            Self::TrustRemoved => "trust.removed",
            Self::TrustResetNeedsWifi => "trust.reset_needs_wifi",
            Self::TrustResetAction => "trust.reset_action",
            Self::TrustResetDone => "trust.reset_done",
        }
    }
}

pub trait Localizer {
    fn text(&self, key: MessageKey) -> &'static str;

    fn format(&self, key: MessageKey, arguments: &[&str]) -> String {
        format(key.as_str(), arguments)
    }
}

pub struct ZhCn;

impl Localizer for ZhCn {
    fn text(&self, key: MessageKey) -> &'static str {
        text(key.as_str())
    }
}

#[derive(Debug, Deserialize)]
struct LanguageFile {
    language: String,
    messages: HashMap<String, String>,
}

static LANGUAGE: OnceLock<LanguageFile> = OnceLock::new();

fn language_file() -> &'static LanguageFile {
    LANGUAGE.get_or_init(|| {
        let language: LanguageFile = serde_json::from_str(include_str!("../locales/zh-CN.json"))
            .expect("bundled language file must be valid JSON");
        assert_eq!(language.language, DEFAULT_LANGUAGE);
        language
    })
}

pub fn language() -> &'static str {
    &language_file().language
}

pub fn text(key: &str) -> &'static str {
    language_file()
        .messages
        .get(key)
        .map(String::as_str)
        .unwrap_or_else(|| panic!("missing language key: {key}"))
}

pub fn format(key: &str, arguments: &[&str]) -> String {
    let mut message = text(key).to_string();
    for (index, argument) in arguments.iter().enumerate() {
        message = message.replace(&format!("{{{index}}}"), argument);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_language_file_loads() {
        assert_eq!(language(), DEFAULT_LANGUAGE);
        assert!(!text("cli.about").is_empty());
    }

    #[test]
    fn positional_arguments_are_replaced() {
        let rendered = format("device.ping_result", &["12"]);
        assert!(rendered.contains("12"));
        assert!(!rendered.contains("{0}"));
    }
}
