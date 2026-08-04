//! Generate the authoritative Rust-side event JSON fixtures.
//!
//! Every `BackendEvent` variant is built with real DTO values, wrapped in
//! an `EventEnvelope` (sequence/timestamp fixed for determinism) and
//! written as pretty JSON to:
//!   - crates/handshaker-application/tests/fixtures/event_<name>.json
//!   - platform/macos/Tests/HandShakerCoreTests/Fixtures/event_<name>.json
//!
//! Run:  cargo run -p handshaker-application --example gen_event_fixtures
//! Commit the generated files; the Rust tests
//! (`tests.rs::event_fixtures_round_trip_preserve_kind`) and the Swift
//! tests (`ModelTests.testAllEventFixturesDecodeAndReencode`) pin them.

use handshaker_application::{
    BackendEvent, ClipboardEntryDto, DeviceDescriptor, DeviceId, DeviceInfoDto, EventEnvelope,
    MediaChangeDto, MediaChangeItemDto, MediaKindDto, PublicError, RemoteFileChangeDto,
    RemoteFileChangeKind, SessionId, SessionSnapshot,
};
use handshaker_application::{
    BatchTransferItemDto, BatchTransferResultDto, PublicErrorCode, SessionState, SyncRunResultDto,
    TransferDirectionDto, TransferId, TransferSnapshot, TransferState, TransportKind,
};

fn envelope(event: BackendEvent) -> EventEnvelope {
    EventEnvelope {
        sequence: 1,
        timestamp_ms: 1_700_000_000_000,
        event,
    }
}

fn device() -> DeviceDescriptor {
    DeviceDescriptor {
        id: DeviceId("phone:9a3f-77ee".into()),
        stable_id: Some(DeviceId("phone:9a3f-77ee".into())),
        display_name: Some("U2 Pro".into()),
        model: Some("OD103".into()),
        transport: TransportKind::Wifi,
        transport_address: "192.168.1.23:41000".into(),
        available: true,
        adb: None,
        usb: None,
    }
}

fn all_fixtures() -> Vec<(&'static str, EventEnvelope)> {
    vec![
        (
            "event_runtime_started",
            envelope(BackendEvent::RuntimeStarted),
        ),
        (
            "event_runtime_stopping",
            envelope(BackendEvent::RuntimeStopping),
        ),
        (
            "event_device_added",
            envelope(BackendEvent::DeviceAdded(device())),
        ),
        (
            "event_device_updated",
            envelope(BackendEvent::DeviceUpdated {
                session_id: SessionId(7),
                device: device(),
            }),
        ),
        (
            "event_device_removed",
            envelope(BackendEvent::DeviceRemoved {
                device_id: DeviceId("phone:9a3f-77ee".into()),
            }),
        ),
        (
            "event_session_state_changed",
            envelope(BackendEvent::SessionStateChanged(Box::new(
                SessionSnapshot {
                    id: SessionId(7),
                    device: device(),
                    device_info: DeviceInfoDto {
                        serial: "3f13d4b4".into(),
                        phone_id: Some("9a3f-77ee".into()),
                        name: Some("U2 Pro".into()),
                        model: Some("OD103".into()),
                        root_path: "/storage/emulated/0".into(),
                        ..Default::default()
                    },
                    state: SessionState::Ready,
                    connected_at_ms: 1_700_000_000_000,
                    last_activity_at_ms: Some(1_700_000_000_100),
                },
            ))),
        ),
        (
            "event_transfer_updated",
            envelope(BackendEvent::TransferUpdated(TransferSnapshot {
                id: TransferId(7),
                session_id: SessionId(1),
                direction: TransferDirectionDto::Download,
                source: "/remote/f.bin".into(),
                destination: "/local/f.bin".into(),
                state: TransferState::Running,
                transferred_bytes: 12,
                total_bytes: Some(100),
                started_at_ms: Some(1),
                finished_at_ms: None,
                error: None,
                item_count: 3,
                completed_items: 1,
                failed_items: 0,
                current_item: Some("/remote/g.bin".into()),
                batch_result: Some(BatchTransferResultDto {
                    ok: vec![BatchTransferItemDto {
                        source: "/remote/g.bin".into(),
                        target: "/local/g.bin".into(),
                    }],
                    failures: Vec::new(),
                }),
            })),
        ),
        (
            "event_connection_lost",
            envelope(BackendEvent::ConnectionLost {
                session_id: SessionId(7),
            }),
        ),
        (
            "event_clipboard_changed",
            envelope(BackendEvent::ClipboardChanged {
                session_id: SessionId(7),
                entries: vec![ClipboardEntryDto {
                    text: "hello 锤子".into(),
                    timestamp_ms: 1_700_000_000_000,
                }],
            }),
        ),
        (
            "event_media_changed",
            envelope(BackendEvent::MediaChanged {
                session_id: SessionId(7),
                change: MediaChangeDto {
                    media_kind: MediaKindDto::Photo,
                    added: vec![MediaChangeItemDto {
                        media_id: Some(42),
                        path: Some("/storage/emulated/0/DCIM/Camera/IMG_0042.jpg".into()),
                        size: Some(2048),
                        ..Default::default()
                    }],
                    deleted: vec![],
                    updated: vec![],
                },
            }),
        ),
        (
            "event_remote_file_changed",
            envelope(BackendEvent::RemoteFileChanged {
                session_id: SessionId(7),
                change: RemoteFileChangeDto {
                    change_kind: RemoteFileChangeKind::DirectoryChanged,
                    paths: vec!["/storage/emulated/0/DCIM/Camera".into()],
                    files: vec![],
                    statuses: vec![],
                },
            }),
        ),
        (
            "event_sync_watch_applied",
            envelope(BackendEvent::SyncWatchApplied {
                profile_id: "photos".into(),
                session_id: SessionId(7),
                result: Box::new(SyncRunResultDto {
                    downloaded: vec!["/storage/emulated/0/DCIM/Camera/IMG_0042.jpg".into()],
                    deleted: vec![],
                    failures: vec![],
                    conflicts: vec![],
                }),
            }),
        ),
        (
            "event_warning",
            envelope(BackendEvent::Warning(
                PublicError::new(PublicErrorCode::RemoteIo, "remote read failed").operation("sync"),
            )),
        ),
    ]
}

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let rust_dir = std::path::Path::new(manifest_dir).join("tests/fixtures");
    let swift_dir = std::path::Path::new(manifest_dir)
        .join("../../platform/macos/Tests/HandShakerCoreTests/Fixtures");
    std::fs::create_dir_all(&rust_dir).expect("rust fixtures dir");
    std::fs::create_dir_all(&swift_dir).expect("swift fixtures dir");

    for (name, envelope) in all_fixtures() {
        let json = format!(
            "{}\n",
            serde_json::to_string_pretty(&envelope).expect("serialize")
        );
        let file = format!("{name}.json");
        std::fs::write(rust_dir.join(&file), &json).expect("write rust fixture");
        std::fs::write(swift_dir.join(&file), &json).expect("write swift fixture");
        println!("wrote {file}");
    }
    println!("done: {} fixtures", all_fixtures().len());
}
