use std::io::Read;

use flate2::read::GzDecoder;
use prost::Message;

use crate::domain::{ClipboardEntry, DeviceInfo, RemoteFile};
use crate::events::{
    ClientEvent, FileChange, FileChangeStatus, FileEvent, FileEventKind, MediaAlbum, MediaItem,
    MediaKind, MediaLibraryChange, PhotoSyncChange, SyncMonitorChange, UnknownEvent,
    UnknownEventReason,
};
use crate::protocol::proto::*;

pub(crate) fn decode_event(sid: u32, body: &[u8], serial: &str) -> ClientEvent {
    let request_type = SspRequest::decode(body)
        .ok()
        .and_then(|request| request.r#type);
    let event = match request_type.and_then(|value| SspRequestType::try_from(value).ok()) {
        Some(SspRequestType::GetDeviceInfoRequest) => SspGetDeviceInfoResponse::decode(body)
            .ok()
            .map(|response| ClientEvent::DeviceInfoChanged(device_info(response, serial))),
        Some(SspRequestType::PhotoLibChange) => SspPhotoLibraryChange::decode(body)
            .ok()
            .map(|change| ClientEvent::MediaLibraryChanged(photo_change(change))),
        Some(SspRequestType::VideoLibChange) => SspVideoLibraryChange::decode(body)
            .ok()
            .map(|change| ClientEvent::MediaLibraryChanged(video_change(change))),
        Some(SspRequestType::AudioLibChange) => SspAudioLibraryChange::decode(body)
            .ok()
            .map(|change| ClientEvent::MediaLibraryChanged(audio_change(change))),
        Some(SspRequestType::MonitorFolderResponse) => SspMonitorFolderResponse::decode(body)
            .ok()
            .map(|change| ClientEvent::DirectoryChanged(directory_change(change))),
        Some(SspRequestType::ClipboardChange) => SspClipboardChange::decode(body)
            .ok()
            .and_then(|change| clipboard_change(change).map(ClientEvent::ClipboardChanged)),
        Some(SspRequestType::PhotoSyncRequest) => SspPhotoSyncResponse::decode(body)
            .ok()
            .map(|change| ClientEvent::PhotoSyncChanged(photo_sync_change(change))),
        Some(SspRequestType::FileChange) => SspFileChange::decode(body)
            .ok()
            .map(|change| ClientEvent::FileChanged(file_change(change))),
        Some(SspRequestType::SyncMonitorRequest) => SspSyncMonitorResponse::decode(body)
            .ok()
            .map(|change| ClientEvent::SyncMonitorChanged(sync_monitor_change(change))),
        Some(SspRequestType::CancelRequest) => None,
        _ => infer_event(body, serial),
    };

    event.unwrap_or_else(|| {
        ClientEvent::Unknown(UnknownEvent {
            sid,
            request_type,
            payload_len: body.len(),
            reason: match request_type {
                Some(value) if SspRequestType::try_from(value).is_err() => {
                    UnknownEventReason::UnknownRequestType
                }
                Some(_) => UnknownEventReason::DecodeFailed,
                None => UnknownEventReason::MissingTypeAmbiguous,
            },
        })
    })
}

pub(crate) fn decode_cancel(body: &[u8]) -> Option<SspCancelRequest> {
    let request = SspCancelRequest::decode(body).ok()?;
    if request.session_id.is_some() && request.r#type == Some(SspRequestType::CancelRequest as i32)
    {
        Some(request)
    } else {
        None
    }
}

fn infer_event(body: &[u8], serial: &str) -> Option<ClientEvent> {
    let mut candidates = Vec::new();

    if let Ok(response) = SspGetDeviceInfoResponse::decode(body)
        && (response.phone_id.is_some()
            || response.phone_name.is_some()
            || response.phone_model.is_some()
            || response.root_path.is_some())
    {
        candidates.push(ClientEvent::DeviceInfoChanged(device_info(
            response, serial,
        )));
    }
    if let Ok(change) = SspClipboardChange::decode(body)
        && !change.clipboard.is_empty()
        && let Some(event) = clipboard_change(change).map(ClientEvent::ClipboardChanged)
    {
        candidates.push(event);
    }
    if let Ok(change) = SspPhotoLibraryChange::decode(body)
        && (!change.added_image.is_empty() || !change.deleted_image.is_empty())
    {
        candidates.push(ClientEvent::MediaLibraryChanged(photo_change(change)));
    }
    if let Ok(change) = SspVideoLibraryChange::decode(body)
        && (!change.added_video.is_empty()
            || !change.deleted_video.is_empty()
            || !change.updated_video.is_empty())
    {
        candidates.push(ClientEvent::MediaLibraryChanged(video_change(change)));
    }
    if let Ok(change) = SspAudioLibraryChange::decode(body)
        && (!change.added_audio.is_empty()
            || !change.deleted_audio.is_empty()
            || !change.added_album.is_empty())
    {
        candidates.push(ClientEvent::MediaLibraryChanged(audio_change(change)));
    }
    if let Ok(change) = SspMonitorFolderResponse::decode(body)
        && !change.event.is_empty()
    {
        candidates.push(ClientEvent::DirectoryChanged(directory_change(change)));
    }
    if let Ok(change) = SspFileChange::decode(body)
        && !change.file_change_items.is_empty()
    {
        candidates.push(ClientEvent::FileChanged(file_change(change)));
    }
    if let Ok(change) = SspPhotoSyncResponse::decode(body)
        && (change.is_first.is_some() || change.is_success.is_some() || !change.files.is_empty())
    {
        candidates.push(ClientEvent::PhotoSyncChanged(photo_sync_change(change)));
    }
    if let Ok(change) = SspSyncMonitorResponse::decode(body)
        && change.is_success.is_some()
    {
        candidates.push(ClientEvent::SyncMonitorChanged(sync_monitor_change(change)));
    }

    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

fn device_info(response: SspGetDeviceInfoResponse, serial: &str) -> DeviceInfo {
    DeviceInfo {
        serial: serial.to_string(),
        phone_id: response.phone_id,
        name: response.phone_name,
        model: response.phone_model,
        brand: response.product_brand,
        manufacturer: response.product_manufacturer,
        smartisan_version: response.smartisan_version,
        apk_version: response.apk_version,
        apk_version_name: None,
        root_path: response.root_path.unwrap_or_else(|| "/sdcard".to_string()),
        external_storage_path: response.external_storage_path,
        disk_size: response.disk_size,
        used_disk_size: response.used_disk_size,
        battery_percentage: response.battery_percentage,
        phone_locked: response.phone_locked,
    }
}

fn clipboard_change(change: SspClipboardChange) -> Option<Vec<ClipboardEntry>> {
    change
        .clipboard
        .into_iter()
        .map(|clipboard| {
            let mut text = String::new();
            GzDecoder::new(clipboard.content.unwrap_or_default().as_slice())
                .read_to_string(&mut text)
                .ok()?;
            Some(ClipboardEntry {
                text,
                timestamp_ms: clipboard.mstimestamp.unwrap_or_default(),
            })
        })
        .collect()
}

fn photo_change(change: SspPhotoLibraryChange) -> MediaLibraryChange {
    MediaLibraryChange {
        kind: MediaKind::Photo,
        added: change.added_image.into_iter().map(photo_item).collect(),
        deleted: change.deleted_image.into_iter().map(photo_item).collect(),
        updated: Vec::new(),
        albums: Vec::new(),
    }
}

fn video_change(change: SspVideoLibraryChange) -> MediaLibraryChange {
    MediaLibraryChange {
        kind: MediaKind::Video,
        added: change.added_video.into_iter().map(video_item).collect(),
        deleted: change.deleted_video.into_iter().map(video_item).collect(),
        updated: change.updated_video.into_iter().map(video_item).collect(),
        albums: Vec::new(),
    }
}

fn audio_change(change: SspAudioLibraryChange) -> MediaLibraryChange {
    MediaLibraryChange {
        kind: MediaKind::Audio,
        added: change.added_audio.into_iter().map(audio_item).collect(),
        deleted: Vec::new(),
        updated: Vec::new(),
        albums: change.added_album.into_iter().map(audio_album).collect(),
    }
}

fn photo_item(item: SspImageFile) -> MediaItem {
    MediaItem {
        media_id: item.media_id,
        album_id: item.album_id,
        path: item.path,
        size: item.file_size,
        created_at: item.created_timestamp,
        modified_at: item.modified_timestamp,
        width: item.width,
        height: item.height,
        orientation: item.orientation,
        mime_type: item.mime_type,
        title: item.title,
        album_name: item.album_name,
        duration: None,
        artist: None,
    }
}

fn video_item(item: SspVideoFile) -> MediaItem {
    MediaItem {
        media_id: item.media_id,
        album_id: item.album_id,
        path: item.path,
        size: item.file_size,
        created_at: item.created_timestamp.map(u64::from),
        modified_at: item.modified_timestamp.map(u64::from),
        width: item.width,
        height: item.height,
        orientation: item.orientation,
        mime_type: item.mime_type,
        title: None,
        album_name: None,
        duration: item.duration,
        artist: None,
    }
}

fn audio_item(item: SspAudioFile) -> MediaItem {
    MediaItem {
        media_id: item.media_id,
        album_id: item.album_id,
        path: item.path,
        size: item.file_size,
        created_at: item.created_timestamp,
        modified_at: item.modified_timestamp,
        width: None,
        height: None,
        orientation: None,
        mime_type: item.mime_type,
        title: item.title,
        album_name: None,
        // The phone reports audio durations in milliseconds (proto comment);
        // the library query path converts to seconds, so do the same here.
        duration: item.duration.map(|millis| millis / 1000.0),
        artist: item.artist,
    }
}

fn audio_album(album: SspAudioAlbum) -> MediaAlbum {
    MediaAlbum {
        album_id: album.album_id,
        path: album.album_path,
        name: album.album_name,
        artist: album.artist,
    }
}

fn directory_change(change: SspMonitorFolderResponse) -> Vec<FileEvent> {
    change
        .event
        .into_iter()
        .map(|event| FileEvent {
            file: event.file.map(remote_file),
            kind: event
                .event
                .and_then(|value| SspFileEventType::try_from(value).ok())
                .map(file_event_kind)
                .unwrap_or(FileEventKind::Unknown),
        })
        .collect()
}

fn file_change(change: SspFileChange) -> Vec<FileChange> {
    change
        .file_change_items
        .into_iter()
        .map(|item| FileChange {
            file: item.file.map(remote_file),
            status: item
                .status
                .and_then(|value| SspFileChangeStatus::try_from(value).ok())
                .map(file_change_status)
                .unwrap_or(FileChangeStatus::Unknown),
        })
        .collect()
}

fn photo_sync_change(change: SspPhotoSyncResponse) -> PhotoSyncChange {
    PhotoSyncChange {
        is_first: change.is_first,
        files: change.files.into_iter().map(remote_file).collect(),
        is_success: change.is_success,
    }
}

fn sync_monitor_change(change: SspSyncMonitorResponse) -> SyncMonitorChange {
    SyncMonitorChange {
        is_success: change.is_success,
    }
}

fn file_event_kind(value: SspFileEventType) -> FileEventKind {
    match value {
        SspFileEventType::FileEventCreate => FileEventKind::Create,
        SspFileEventType::FileEventDelete => FileEventKind::Delete,
        SspFileEventType::FileEventCloseWrite => FileEventKind::CloseWrite,
        SspFileEventType::FileEventMovedFrom => FileEventKind::MovedFrom,
        SspFileEventType::FileEventMovedTo => FileEventKind::MovedTo,
        SspFileEventType::FileEventDeleteSelf => FileEventKind::DeleteSelf,
        SspFileEventType::FileEventMoveSelf => FileEventKind::MoveSelf,
        SspFileEventType::FileEventDirChanged => FileEventKind::DirectoryChanged,
    }
}

fn file_change_status(value: SspFileChangeStatus) -> FileChangeStatus {
    match value {
        SspFileChangeStatus::None => FileChangeStatus::None,
        SspFileChangeStatus::Added => FileChangeStatus::Added,
        SspFileChangeStatus::Deleted => FileChangeStatus::Deleted,
        SspFileChangeStatus::Modified => FileChangeStatus::Modified,
        SspFileChangeStatus::InfoModified => FileChangeStatus::InfoModified,
        SspFileChangeStatus::FileAndInfoModified => FileChangeStatus::FileAndInfoModified,
    }
}

fn remote_file(file: SspFile) -> RemoteFile {
    RemoteFile {
        path: file.path.unwrap_or_default(),
        size: file.file_size.unwrap_or_default(),
        created_at: file.created_timestamp,
        modified_at: file.modified_timestamp,
        is_directory: file.is_directory.unwrap_or(false),
        checksum: file.checksum,
        is_trash: file.is_trash,
        id: file.id,
        ext_data: file.ext_data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventKind;

    #[test]
    fn decodes_device_info_event_with_type() {
        let body = SspGetDeviceInfoResponse {
            r#type: Some(SspRequestType::GetDeviceInfoRequest as i32),
            phone_name: Some("phone".to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let event = decode_event(9, &body, "serial");
        let ClientEvent::DeviceInfoChanged(info) = event else {
            panic!("device event")
        };
        assert_eq!(info.serial, "serial");
        assert_eq!(info.name.as_deref(), Some("phone"));
        assert_eq!(info.root_path, "/sdcard");
    }

    #[test]
    fn audio_change_duration_is_converted_from_millis_to_seconds() {
        let change = SspAudioLibraryChange {
            r#type: Some(SspRequestType::AudioLibChange as i32),
            added_audio: vec![SspAudioFile {
                media_id: Some(7),
                path: Some("/storage/emulated/0/Music/a.mp3".to_string()),
                title: Some("track".to_string()),
                artist: Some("artist".to_string()),
                duration: Some(210_000.0), // milliseconds on the wire
                ..Default::default()
            }],
            ..Default::default()
        };
        let body = change.encode_to_vec();
        let event = decode_event(9, &body, "serial");
        let ClientEvent::MediaLibraryChanged(library) = event else {
            panic!("media event")
        };
        assert_eq!(library.kind, MediaKind::Audio);
        assert_eq!(library.added.len(), 1);
        assert_eq!(library.added[0].duration, Some(210.0), "ms -> seconds");
    }

    #[test]
    fn decodes_each_supported_typed_event_family() {
        let cases = [
            (
                SspClipboardChange {
                    r#type: Some(SspRequestType::ClipboardChange as i32),
                    ..Default::default()
                }
                .encode_to_vec(),
                EventKind::ClipboardChanged,
            ),
            (
                SspPhotoLibraryChange {
                    r#type: Some(SspRequestType::PhotoLibChange as i32),
                    ..Default::default()
                }
                .encode_to_vec(),
                EventKind::MediaLibraryChanged,
            ),
            (
                SspVideoLibraryChange {
                    r#type: Some(SspRequestType::VideoLibChange as i32),
                    ..Default::default()
                }
                .encode_to_vec(),
                EventKind::MediaLibraryChanged,
            ),
            (
                SspAudioLibraryChange {
                    r#type: Some(SspRequestType::AudioLibChange as i32),
                    ..Default::default()
                }
                .encode_to_vec(),
                EventKind::MediaLibraryChanged,
            ),
            (
                SspMonitorFolderResponse {
                    r#type: Some(SspRequestType::MonitorFolderResponse as i32),
                    ..Default::default()
                }
                .encode_to_vec(),
                EventKind::DirectoryChanged,
            ),
            (
                SspPhotoSyncResponse {
                    r#type: Some(SspRequestType::PhotoSyncRequest as i32),
                    ..Default::default()
                }
                .encode_to_vec(),
                EventKind::PhotoSyncChanged,
            ),
            (
                SspFileChange {
                    r#type: Some(SspRequestType::FileChange as i32),
                    ..Default::default()
                }
                .encode_to_vec(),
                EventKind::FileChanged,
            ),
            (
                SspSyncMonitorResponse {
                    r#type: Some(SspRequestType::SyncMonitorRequest as i32),
                    ..Default::default()
                }
                .encode_to_vec(),
                EventKind::SyncMonitorChanged,
            ),
        ];
        for (body, expected) in cases {
            assert_eq!(decode_event(12, &body, "serial").kind(), expected);
        }
    }

    #[test]
    fn infers_clipboard_event_when_type_is_missing() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"event").unwrap();
        let body = SspClipboardChange {
            clipboard: vec![SspClipboard {
                content: Some(encoder.finish().unwrap()),
                mstimestamp: Some(42),
            }],
            ..Default::default()
        }
        .encode_to_vec();
        let event = decode_event(10, &body, "serial");
        let ClientEvent::ClipboardChanged(entries) = event else {
            panic!("clipboard event")
        };
        assert_eq!(entries[0].text, "event");
        assert_eq!(entries[0].timestamp_ms, 42);
    }

    #[test]
    fn unknown_event_contains_only_safe_metadata() {
        let body = vec![0x08, 0xe7, 0x07];
        let event = decode_event(11, &body, "serial");
        let ClientEvent::Unknown(unknown) = event else {
            panic!("unknown event")
        };
        assert_eq!(unknown.sid, 11);
        assert_eq!(unknown.request_type, Some(999));
        assert_eq!(unknown.payload_len, body.len());
        assert_eq!(unknown.reason, UnknownEventReason::UnknownRequestType);
        assert!(!serde_json::to_string(&unknown).unwrap().contains("e707"));
    }

    #[test]
    fn recognizes_explicit_remote_cancel() {
        let body = SspCancelRequest {
            r#type: Some(SspRequestType::CancelRequest as i32),
            session_id: Some(0x8000_0002),
            error_code: Some(SspCancelErrorCode::ErrorCodeSdcardRemoved as i32),
        }
        .encode_to_vec();
        let cancel = decode_cancel(&body).expect("cancel");
        assert_eq!(cancel.session_id, Some(0x8000_0002));
        assert_eq!(
            cancel.error_code,
            Some(SspCancelErrorCode::ErrorCodeSdcardRemoved as i32)
        );
    }
}
