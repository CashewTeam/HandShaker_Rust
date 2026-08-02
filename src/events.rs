use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::cancellation::CancellationInfo;
use crate::domain::DeviceInfo;
use crate::i18n;

const EVENT_CHANNEL_CAPACITY: usize = 64;

/// The stable category of a phone-initiated event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A new device information snapshot.
    DeviceInfoChanged,
    /// A phone clipboard history update.
    ClipboardChanged,
    /// A photo, video, or audio library update.
    MediaLibraryChanged,
    /// A directory monitor file event.
    DirectoryChanged,
    /// A synchronization file change event.
    FileChanged,
    /// A one-shot photo synchronization response.
    PhotoSyncChanged,
    /// A real-time synchronization monitor response.
    SyncMonitorChanged,
    /// A phone-side cancellation notification.
    RequestCancelled,
    /// A message that could not be classified safely.
    Unknown,
}

/// A typed event received outside the response path of a pending request.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ClientEvent {
    /// Updated device information.
    DeviceInfoChanged(DeviceInfo),
    /// Updated clipboard entries.
    ClipboardChanged(Vec<crate::domain::ClipboardEntry>),
    /// A media library update.
    MediaLibraryChanged(MediaLibraryChange),
    /// File events from a monitored directory.
    DirectoryChanged(Vec<FileEvent>),
    /// File changes used by synchronization.
    FileChanged(Vec<FileChange>),
    /// A one-shot synchronization response.
    PhotoSyncChanged(PhotoSyncChange),
    /// A synchronization monitor response.
    SyncMonitorChanged(SyncMonitorChange),
    /// A phone cancellation that did not belong to a current request.
    RequestCancelled(CancellationInfo),
    /// A safely summarized event that was not understood.
    Unknown(UnknownEvent),
}

impl ClientEvent {
    /// Return the stable kind of this event.
    pub fn kind(&self) -> EventKind {
        match self {
            Self::DeviceInfoChanged(_) => EventKind::DeviceInfoChanged,
            Self::ClipboardChanged(_) => EventKind::ClipboardChanged,
            Self::MediaLibraryChanged(_) => EventKind::MediaLibraryChanged,
            Self::DirectoryChanged(_) => EventKind::DirectoryChanged,
            Self::FileChanged(_) => EventKind::FileChanged,
            Self::PhotoSyncChanged(_) => EventKind::PhotoSyncChanged,
            Self::SyncMonitorChanged(_) => EventKind::SyncMonitorChanged,
            Self::RequestCancelled(_) => EventKind::RequestCancelled,
            Self::Unknown(_) => EventKind::Unknown,
        }
    }
}

/// Select event kinds for a subscription.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFilter {
    kinds: Option<BTreeSet<EventKind>>,
}

impl EventFilter {
    /// Subscribe to every event kind.
    pub fn all() -> Self {
        Self { kinds: None }
    }

    /// Subscribe only to the supplied event kinds.
    pub fn only<I>(kinds: I) -> Self
    where
        I: IntoIterator<Item = EventKind>,
    {
        Self {
            kinds: Some(kinds.into_iter().collect()),
        }
    }

    pub(crate) fn accepts(&self, kind: EventKind) -> bool {
        self.kinds
            .as_ref()
            .map(|kinds| kinds.contains(&kind))
            .unwrap_or(true)
    }
}

/// An event stream error that does not expose Tokio channel types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStreamError {
    /// The receiver fell behind and missed this many events. It remains usable.
    Lagged { missed: u64 },
    /// The owning Session has closed.
    Closed,
}

impl fmt::Display for EventStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lagged { missed } => {
                formatter.write_str(&i18n::format("event.stream_lagged", &[&missed.to_string()]))
            }
            Self::Closed => formatter.write_str(i18n::text("event.stream_closed")),
        }
    }
}

impl std::error::Error for EventStreamError {}

/// A bounded subscription to the Session event bus.
pub struct EventSubscription {
    receiver: broadcast::Receiver<ClientEvent>,
    filter: EventFilter,
    closed: bool,
}

impl EventSubscription {
    pub(crate) fn new(receiver: broadcast::Receiver<ClientEvent>, filter: EventFilter) -> Self {
        Self {
            receiver,
            filter,
            closed: false,
        }
    }

    pub(crate) fn closed(filter: EventFilter) -> Self {
        let (sender, receiver) = broadcast::channel(1);
        drop(sender);
        Self::new(receiver, filter)
    }

    /// Wait for the next event accepted by this filter.
    pub async fn recv(&mut self) -> Result<ClientEvent, EventStreamError> {
        if self.closed {
            return Err(EventStreamError::Closed);
        }
        loop {
            match self.receiver.recv().await {
                Ok(event) if self.filter.accepts(event.kind()) => return Ok(event),
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    return Err(EventStreamError::Lagged { missed });
                }
                Err(broadcast::error::RecvError::Closed) => return Err(EventStreamError::Closed),
            }
        }
    }

    /// Try to receive an event without waiting. `Ok(None)` means no event is ready.
    pub fn try_recv(&mut self) -> Result<Option<ClientEvent>, EventStreamError> {
        if self.closed {
            return Err(EventStreamError::Closed);
        }
        loop {
            match self.receiver.try_recv() {
                Ok(event) if self.filter.accepts(event.kind()) => return Ok(Some(event)),
                Ok(_) => continue,
                Err(broadcast::error::TryRecvError::Empty) => return Ok(None),
                Err(broadcast::error::TryRecvError::Lagged(missed)) => {
                    return Err(EventStreamError::Lagged { missed });
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    return Err(EventStreamError::Closed);
                }
            }
        }
    }

    /// Stop this subscription and release its receiver.
    pub fn close(&mut self) {
        self.closed = true;
    }
}

pub(crate) fn event_channel() -> broadcast::Sender<ClientEvent> {
    broadcast::channel(EVENT_CHANNEL_CAPACITY).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::{CancellationInfo, CancellationOrigin};

    fn unknown(sid: u32) -> ClientEvent {
        ClientEvent::Unknown(UnknownEvent {
            sid,
            request_type: None,
            payload_len: 3,
            reason: UnknownEventReason::MissingTypeAmbiguous,
        })
    }

    #[tokio::test]
    async fn subscriptions_are_independent_and_filter_events() {
        let sender = event_channel();
        let mut all = EventSubscription::new(sender.subscribe(), EventFilter::all());
        let mut cancellations = EventSubscription::new(
            sender.subscribe(),
            EventFilter::only([EventKind::RequestCancelled]),
        );
        sender.send(unknown(1)).unwrap();
        sender
            .send(ClientEvent::RequestCancelled(CancellationInfo {
                sid: 2,
                origin: CancellationOrigin::Remote { error_code: None },
                connection_closed: false,
            }))
            .unwrap();

        assert_eq!(all.recv().await.unwrap().kind(), EventKind::Unknown);
        assert_eq!(
            all.recv().await.unwrap().kind(),
            EventKind::RequestCancelled
        );
        assert_eq!(
            cancellations.recv().await.unwrap().kind(),
            EventKind::RequestCancelled
        );
    }

    #[tokio::test]
    async fn slow_subscriber_reports_lag_and_can_continue() {
        let sender = event_channel();
        let mut subscription = EventSubscription::new(sender.subscribe(), EventFilter::all());
        for sid in 0..=EVENT_CHANNEL_CAPACITY as u32 {
            sender.send(unknown(sid)).unwrap();
        }

        assert_eq!(
            subscription.try_recv(),
            Err(EventStreamError::Lagged { missed: 1 })
        );
        assert_eq!(
            subscription.recv().await.unwrap().kind(),
            EventKind::Unknown
        );
    }

    #[tokio::test]
    async fn closed_sender_ends_subscription() {
        let sender = event_channel();
        let mut subscription = EventSubscription::new(sender.subscribe(), EventFilter::all());
        drop(sender);
        assert_eq!(subscription.recv().await, Err(EventStreamError::Closed));
    }

    #[test]
    fn close_stops_subscription_without_closing_other_subscribers() {
        let sender = event_channel();
        let mut closed = EventSubscription::new(sender.subscribe(), EventFilter::all());
        let mut active = EventSubscription::new(sender.subscribe(), EventFilter::all());
        closed.close();
        sender.send(unknown(7)).unwrap();
        assert_eq!(closed.try_recv(), Err(EventStreamError::Closed));
        assert_eq!(
            active.try_recv().unwrap().unwrap().kind(),
            EventKind::Unknown
        );
    }

    #[test]
    fn event_json_uses_stable_english_kind_names() {
        let value = serde_json::to_value(unknown(8)).unwrap();
        assert_eq!(value["kind"], "unknown");
        assert_eq!(value["data"]["sid"], 8);
        assert!(value["data"].get("payload").is_none());
    }

    #[test]
    fn all_event_kinds_serialize_to_stable_snake_case_tags() {
        // 0.2.0 compatibility contract: these JSON `kind` tags are part of the
        // public watch schema and must not change without a major bump.
        let cases = [
            (EventKind::DeviceInfoChanged, "device_info_changed"),
            (EventKind::ClipboardChanged, "clipboard_changed"),
            (EventKind::MediaLibraryChanged, "media_library_changed"),
            (EventKind::DirectoryChanged, "directory_changed"),
            (EventKind::FileChanged, "file_changed"),
            (EventKind::PhotoSyncChanged, "photo_sync_changed"),
            (EventKind::SyncMonitorChanged, "sync_monitor_changed"),
            (EventKind::RequestCancelled, "request_cancelled"),
            (EventKind::Unknown, "unknown"),
        ];
        for (kind, expected) in cases {
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{expected}\"")
            );
        }
    }

    #[test]
    fn directory_and_file_events_round_trip_with_stable_kinds() {
        let directory = ClientEvent::DirectoryChanged(vec![FileEvent {
            file: Some(crate::domain::RemoteFile {
                path: "/storage/emulated/0/DCIM/a.jpg".to_string(),
                size: 0,
                created_at: None,
                modified_at: None,
                is_directory: false,
                checksum: None,
                is_trash: None,
                id: None,
            }),
            kind: FileEventKind::Create,
        }]);
        let value = serde_json::to_value(&directory).unwrap();
        assert_eq!(value["kind"], "directory_changed");
        assert_eq!(
            value["data"][0]["file"]["path"],
            "/storage/emulated/0/DCIM/a.jpg"
        );
        assert_eq!(value["data"][0]["kind"], "create");

        let file = ClientEvent::FileChanged(vec![FileChange {
            file: Some(crate::domain::RemoteFile {
                path: "/storage/emulated/0/b.txt".to_string(),
                size: 0,
                created_at: None,
                modified_at: None,
                is_directory: false,
                checksum: None,
                is_trash: None,
                id: None,
            }),
            status: FileChangeStatus::Modified,
        }]);
        let value = serde_json::to_value(&file).unwrap();
        assert_eq!(value["kind"], "file_changed");
        assert_eq!(
            value["data"][0]["file"]["path"],
            "/storage/emulated/0/b.txt"
        );
        assert_eq!(value["data"][0]["status"], "modified");
    }
}

/// Which media library produced a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// Photo library.
    Photo,
    /// Video library.
    Video,
    /// Audio library.
    Audio,
}

/// A normalized media library change. Optional fields preserve proto2 omission.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MediaLibraryChange {
    /// Source media category.
    pub kind: MediaKind,
    /// Added entries.
    pub added: Vec<MediaItem>,
    /// Deleted entries.
    pub deleted: Vec<MediaItem>,
    /// Updated entries.
    pub updated: Vec<MediaItem>,
    /// Album changes, when supplied by the phone.
    pub albums: Vec<MediaAlbum>,
}

/// A normalized media entry from a change event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MediaItem {
    /// Media-store identifier.
    pub media_id: Option<u64>,
    /// Album identifier.
    pub album_id: Option<u64>,
    /// Remote file path.
    pub path: Option<String>,
    /// File size in bytes.
    pub size: Option<u64>,
    /// Creation timestamp.
    pub created_at: Option<u64>,
    /// Modification timestamp.
    pub modified_at: Option<u64>,
    /// Width in pixels.
    pub width: Option<u32>,
    /// Height in pixels.
    pub height: Option<u32>,
    /// Orientation value.
    pub orientation: Option<u32>,
    /// MIME type.
    pub mime_type: Option<String>,
    /// Display title.
    pub title: Option<String>,
    /// Album name.
    pub album_name: Option<String>,
    /// Duration in protocol units.
    pub duration: Option<f64>,
    /// Artist name for audio entries.
    pub artist: Option<String>,
}

/// An album-level change from the phone media library.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MediaAlbum {
    /// Album identifier.
    pub album_id: Option<u64>,
    /// Album path.
    pub path: Option<String>,
    /// Album name.
    pub name: Option<String>,
    /// Artist name for audio albums.
    pub artist: Option<String>,
}

/// A directory monitor event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileEvent {
    /// File metadata supplied by the phone.
    pub file: Option<crate::domain::RemoteFile>,
    /// File observer operation.
    pub kind: FileEventKind,
}

/// File observer operation from a directory monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileEventKind {
    /// A new file or directory was created.
    Create,
    /// A file or directory was deleted.
    Delete,
    /// A file was closed after writing.
    CloseWrite,
    /// A path was moved away from the monitored directory.
    MovedFrom,
    /// A path was moved into the monitored directory.
    MovedTo,
    /// The monitored directory itself was deleted.
    DeleteSelf,
    /// The monitored directory itself was moved.
    MoveSelf,
    /// The directory contents changed.
    DirectoryChanged,
    /// The phone reported an unrecognized file event.
    Unknown,
}

/// One synchronization file change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileChange {
    /// File metadata supplied by the phone.
    pub file: Option<crate::domain::RemoteFile>,
    /// Synchronization operation.
    pub status: FileChangeStatus,
}

/// Synchronization file operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatus {
    /// No file change.
    None,
    /// A file was added.
    Added,
    /// A file was deleted.
    Deleted,
    /// File contents changed.
    Modified,
    /// File metadata changed.
    InfoModified,
    /// File contents and metadata changed.
    FileAndInfoModified,
    /// The phone reported an unrecognized status.
    Unknown,
}

/// A photo synchronization response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhotoSyncChange {
    /// Whether this is the first synchronization response.
    pub is_first: Option<bool>,
    /// Current phone file snapshot.
    pub files: Vec<crate::domain::RemoteFile>,
    /// Whether synchronization succeeded.
    pub is_success: Option<bool>,
}

/// A synchronization monitor response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncMonitorChange {
    /// Whether the monitor operation succeeded.
    pub is_success: Option<bool>,
}

/// A safely summarized event that could not be decoded as a known event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownEvent {
    /// Session id on the wire.
    pub sid: u32,
    /// Request type if field 1 was present and recognized.
    pub request_type: Option<i32>,
    /// Length of the protobuf body, excluding the normal length prefix.
    pub payload_len: usize,
    /// Stable reason for classification failure.
    pub reason: UnknownEventReason,
}

/// Why an unsolicited message could not be safely classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownEventReason {
    /// The request type value is not known to this client.
    UnknownRequestType,
    /// Field 1 was absent and multiple message shapes were plausible.
    MissingTypeAmbiguous,
    /// The request type was known but its message could not be decoded.
    DecodeFailed,
}
