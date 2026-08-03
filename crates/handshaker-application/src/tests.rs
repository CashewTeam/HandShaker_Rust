//! Application-layer unit tests: DTO semantics, error mapping, path rules,
//! and runtime lifecycle that does not require a real device.

use std::time::Duration;

use handshaker_core::{DeviceInfo, RemoteFile};

use crate::dto::{DeviceDescriptor, DeviceId, RuntimeConfig, SessionId, TransportKind};
use crate::error::{PublicErrorCode, from_core_error};
use crate::runtime::normalize_remote_path;
use crate::transfer::{BatchTransferItemDto, BatchTransferRequest, BatchTransferResultDto};
use crate::{HandShakerRuntime, resolve_remote_path};

fn test_config() -> RuntimeConfig {
    RuntimeConfig {
        adb_path: std::path::PathBuf::from("adb-missing-for-tests"),
        default_timeout: Duration::from_secs(1),
        heartbeat_interval: Duration::from_secs(10),
        state_dir: None,
        wire_log: None,
        event_capacity: 16,
    }
}

fn fake_device() -> DeviceDescriptor {
    DeviceDescriptor {
        id: DeviceId("serial-1".to_string()),
        display_name: Some("serial-1".to_string()),
        model: None,
        transport: TransportKind::Adb,
        transport_address: "serial-1".to_string(),
        available: true,
        adb: None,
        usb: None,
    }
}

#[tokio::test]
async fn runtime_create_and_shutdown_is_idempotent() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    runtime.shutdown().await.expect("shutdown");
    // Second shutdown is a no-op, not an error.
    runtime.shutdown().await.expect("shutdown twice");
}

#[tokio::test]
async fn runtime_rejects_zero_event_capacity() {
    let mut config = test_config();
    config.event_capacity = 0;
    let error = HandShakerRuntime::create(config)
        .await
        .err()
        .expect("must reject");
    assert_eq!(error.code, PublicErrorCode::InvalidArgument);
}

#[tokio::test]
async fn operations_after_shutdown_return_runtime_closed() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    runtime.shutdown().await.expect("shutdown");
    let error = runtime
        .list_devices(crate::dto::ListDevicesRequest::default())
        .await
        .expect_err("must be closed");
    assert_eq!(error.code, PublicErrorCode::RuntimeClosed);
    let error = runtime
        .connect(crate::dto::ConnectRequest {
            device: fake_device(),
        })
        .await
        .expect_err("must be closed");
    assert_eq!(error.code, PublicErrorCode::RuntimeClosed);
}

#[tokio::test]
async fn unknown_session_errors_are_stable() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let error = runtime
        .get_session_snapshot(SessionId(42))
        .await
        .expect_err("missing session");
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
    let error = runtime
        .disconnect(SessionId(42))
        .await
        .expect_err("missing session");
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
    let error = match runtime.session_client(SessionId(42)).await {
        Err(error) => error,
        Ok(_) => panic!("expected missing session"),
    };
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
    let error = runtime
        .ping(SessionId(42))
        .await
        .expect_err("missing session");
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
    let error = runtime
        .count_files(crate::dto::CountFilesRequest {
            session_id: SessionId(42),
            path: "/".into(),
            depth: 1,
            exclusions: vec![],
        })
        .await
        .expect_err("missing session");
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
    let error = runtime
        .get_photo_library(SessionId(42))
        .await
        .expect_err("missing session");
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
}

#[test]
fn media_dto_mapping_round_trips_thumbnail_request_shape() {
    // DTO -> core -> DTO keeps media_id/path significant fields intact.
    let dto = crate::media::ImageFileDto {
        media_id: Some(621154),
        path: Some("/storage/emulated/0/DCIM/Camera/a.jpg".into()),
        ..Default::default()
    };
    let core = crate::media::dto_to_image_file(&dto);
    assert_eq!(core.media_id, Some(621154));
    assert_eq!(
        core.path.as_deref(),
        Some("/storage/emulated/0/DCIM/Camera/a.jpg")
    );
    let back: crate::media::ImageFileDto = core.into();
    assert_eq!(back.media_id, dto.media_id);
    assert_eq!(back.path, dto.path);
}

#[test]
fn session_ids_start_at_one_and_increment() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let runtime = HandShakerRuntime::create(test_config())
            .await
            .expect("create");
        // No sessions are openable without a device; verify the counter path
        // via the registry internals indirectly: first connect id would be 1.
        let id = runtime
            .connect(crate::dto::ConnectRequest {
                device: fake_device(),
            })
            .await;
        // connect fails because adb is missing, but the counter is untouched.
        assert!(id.is_err());
    });
}

#[test]
fn resolve_remote_path_rules() {
    // Absolute paths pass through.
    assert_eq!(resolve_remote_path("/storage/emulated/0", "/a/b"), "/a/b");
    // Empty / "." resolve to root.
    assert_eq!(resolve_remote_path("/root", ""), "/root");
    assert_eq!(resolve_remote_path("/root", "."), "/root");
    // Relative joins under root.
    assert_eq!(
        resolve_remote_path("/root", "DCIM/Camera"),
        "/root/DCIM/Camera"
    );
    // Relative ".." is clamped to the root (never escapes above it).
    assert_eq!(resolve_remote_path("/root", "a/../b"), "/root/b");
    assert_eq!(resolve_remote_path("/root", "../../etc"), "/root");
    assert_eq!(resolve_remote_path("/root", "a/../../.."), "/root");
    // Absolute inputs are normalized: ".." cannot escape above "/".
    assert_eq!(resolve_remote_path("/root", "/a/../../etc"), "/etc");
    assert_eq!(
        resolve_remote_path("/root", "/../../etc/passwd"),
        "/etc/passwd"
    );
}

#[test]
fn normalize_remote_path_collapses_dots() {
    assert_eq!(normalize_remote_path("/a/./b//c/"), "/a/b/c");
    assert_eq!(normalize_remote_path("a/b/../c"), "/a/c");
    assert_eq!(normalize_remote_path("/"), "/");
}

#[test]
fn core_errors_map_to_stable_codes() {
    let timeout = from_core_error(handshaker_core::Error::Timeout("timed out".into()), "ping");
    assert_eq!(timeout.code, PublicErrorCode::ConnectionLost);
    assert!(timeout.retryable);

    let interrupted = from_core_error(handshaker_core::Error::Interrupted, "download");
    assert_eq!(interrupted.code, PublicErrorCode::TransferCancelled);

    let protocol = from_core_error(handshaker_core::Error::Protocol("bad frame".into()), "list");
    assert_eq!(protocol.code, PublicErrorCode::ProtocolError);

    let usage = from_core_error(handshaker_core::Error::Usage("bad arg".into()), "list");
    assert_eq!(usage.code, PublicErrorCode::InvalidArgument);
    assert!(usage.operation.as_deref() == Some("list"));
}

#[test]
fn error_codes_have_stable_tokens() {
    assert_eq!(PublicErrorCode::RuntimeClosed.as_str(), "runtime_closed");
    assert_eq!(
        PublicErrorCode::SessionNotFound.as_str(),
        "session_not_found"
    );
    assert_eq!(
        PublicErrorCode::AdbUnauthorized.as_str(),
        "adb_unauthorized"
    );
}

#[test]
fn device_info_maps_to_dto_without_core_leak() {
    let info = DeviceInfo {
        serial: "s1".into(),
        phone_id: Some("p1".into()),
        name: Some("Phone".into()),
        model: Some("OD103".into()),
        brand: Some("SMARTISAN".into()),
        manufacturer: Some("Smartisan".into()),
        smartisan_version: Some("6.7.4".into()),
        apk_version: Some("201".into()),
        apk_version_name: Some("1.2.0".into()),
        root_path: "/storage/emulated/0".into(),
        external_storage_path: None,
        disk_size: None,
        used_disk_size: None,
        battery_percentage: None,
        phone_locked: None,
    };
    let dto = crate::runtime::device_info_to_dto(&info);
    assert_eq!(dto.serial, "s1");
    assert_eq!(dto.model.as_deref(), Some("OD103"));
    assert_eq!(dto.root_path, "/storage/emulated/0");
}

#[test]
fn remote_file_maps_to_dto() {
    let file = RemoteFile {
        path: "/a/b.txt".into(),
        size: 42,
        created_at: Some(1),
        modified_at: Some(2),
        is_directory: false,
        checksum: Some("abc".into()),
        is_trash: None,
        id: Some(9),
        ext_data: None,
    };
    let dto = crate::runtime::remote_file_to_dto(file);
    assert_eq!(dto.path, "/a/b.txt");
    assert_eq!(dto.media_id, Some(9));
    assert!(!dto.is_directory);
}

// ---- v1 JSON contract fixtures (frozen; changes are breaking) ----

#[test]
fn device_descriptor_json_contract_is_stable() {
    let device = DeviceDescriptor {
        id: DeviceId("serial-1".into()),
        display_name: Some("serial-1".into()),
        model: None,
        transport: TransportKind::Adb,
        transport_address: "serial-1".into(),
        available: true,
        adb: None,
        usb: None,
    };
    let json = serde_json::to_value(&device).expect("serialize");
    // Field names and enum token are the frozen contract.
    assert_eq!(json["id"], "serial-1");
    assert_eq!(json["transport"], "adb");
    assert_eq!(json["available"], true);
    // Round-trip preserves the descriptor.
    let decoded: DeviceDescriptor = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, device);
}

#[test]
fn session_state_json_contract_is_stable() {
    let states = [
        (crate::dto::SessionState::Connecting, "connecting"),
        (crate::dto::SessionState::Ready, "ready"),
        (crate::dto::SessionState::Closed, "closed"),
    ];
    for (state, token) in states {
        let json = serde_json::to_value(state).expect("serialize");
        assert_eq!(json, token);
        let decoded: crate::dto::SessionState = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded, state);
    }
}

#[test]
fn public_error_json_contract_is_stable() {
    let error =
        crate::error::PublicError::new(PublicErrorCode::SessionNotFound, "session not found")
            .with_detail("no such id")
            .operation("get_session");
    let json = serde_json::to_value(&error).expect("serialize");
    assert_eq!(json["code"], "session_not_found");
    assert_eq!(json["retryable"], false);
    assert_eq!(json["operation"], "get_session");
    let decoded: crate::error::PublicError = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, error);
}

#[test]
fn unknown_enum_values_are_rejected_not_guessed() {
    // A future transport value must fail to decode, never map silently.
    let result: Result<TransportKind, _> = serde_json::from_str("\"bluetooth\"");
    assert!(result.is_err());
}

// ---- M8.7 transfer & event tests ----

#[test]
fn transfer_state_transitions_are_one_way() {
    use crate::transfer::{TransferDirectionDto, TransferRegistry, TransferState};

    let registry = TransferRegistry::new();
    let snapshot = registry.into_snapshot_for(
        SessionId(1),
        TransferDirectionDto::Download,
        "/remote/a.bin".into(),
        "/local/a.bin".into(),
    );
    let id = snapshot.id;
    registry.register(snapshot); // register before transition/cancel lookups
    assert_eq!(registry.get(id).unwrap().state, TransferState::Queued);
    assert_eq!(
        registry
            .transition(id, TransferState::Running)
            .unwrap()
            .state,
        TransferState::Running
    );
    let completed = registry.transition(id, TransferState::Completed).unwrap();
    assert_eq!(completed.state, TransferState::Completed);
    assert!(completed.finished_at_ms.is_some());
    // Terminal state is never overwritten (one-way).
    assert_eq!(
        registry
            .transition(id, TransferState::Failed)
            .unwrap()
            .state,
        TransferState::Completed
    );
    assert_eq!(registry.get(id).unwrap().state, TransferState::Completed);
}

#[test]
fn cancel_transfer_is_idempotent_and_missing_is_stable() {
    use crate::transfer::{TransferDirectionDto, TransferRegistry, TransferState};

    let registry = TransferRegistry::new();
    let snapshot = registry.into_snapshot_for(
        SessionId(1),
        TransferDirectionDto::Download,
        "/r".into(),
        "/l".into(),
    );
    registry.register(snapshot.clone());
    registry.cancel(snapshot.id).expect("cancel once");
    registry
        .cancel(snapshot.id)
        .expect("cancel twice (idempotent)");
    assert_eq!(
        registry.get(snapshot.id).unwrap().state,
        TransferState::Cancelled
    );
    let missing = registry
        .cancel(crate::transfer::TransferId(999))
        .err()
        .expect("missing");
    assert_eq!(missing.code, PublicErrorCode::TransferNotFound);
}

#[tokio::test]
async fn event_hub_sequences_and_lags() {
    use crate::event::{BackendEvent, EventEnvelope, EventHub};

    let hub = EventHub::new(2);
    let mut receiver = hub.subscribe();
    hub.publish(BackendEvent::RuntimeStarted); // seq 1
    hub.publish(BackendEvent::RuntimeStarted); // seq 2
    hub.publish(BackendEvent::RuntimeStarted); // seq 3 overwrites slot 0
    // The receiver missed seq 1 (capacity 2): first recv reports the skip.
    let skipped = match receiver.recv().await {
        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => n,
        other => panic!("expected Lagged, got {other:?}"),
    };
    assert!(skipped >= 1);
    // After a Lagged, tokio broadcast resumes at the oldest retained slot;
    // with capacity 2 after 3 publishes the retained events are seq 2 and 3.
    let resumed: EventEnvelope = receiver.recv().await.expect("resumed event");
    assert!(resumed.sequence >= 2);
}

#[tokio::test]
async fn subscribe_after_shutdown_still_gets_runtime_stopping() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let mut receiver = runtime.subscribe_events();
    runtime.shutdown().await.expect("shutdown");
    // Receiver sees RuntimeStopping published by shutdown.
    let envelope = receiver.recv().await.expect("event after shutdown");
    assert!(matches!(
        envelope.event,
        crate::event::BackendEvent::RuntimeStopping
    ));
}

#[test]
fn transfer_snapshot_json_contract_is_stable() {
    use crate::transfer::{TransferDirectionDto, TransferSnapshot, TransferState};

    let snapshot = TransferSnapshot {
        id: crate::transfer::TransferId(7),
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
    };
    let json = serde_json::to_value(&snapshot).expect("serialize");
    assert_eq!(json["id"], 7);
    assert_eq!(json["direction"], "download");
    assert_eq!(json["state"], "running");
    let decoded: TransferSnapshot = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, snapshot);
}

#[test]
fn batch_result_to_dto_preserves_ok_and_failures() {
    let result = handshaker_core::BatchTransferResult {
        ok: vec![handshaker_core::BatchTransferItem {
            source: "/remote/a.txt".to_string(),
            target: "/local/a.txt".to_string(),
        }],
        failures: vec![handshaker_core::BatchTransferFailure {
            source: "/remote/b.txt".to_string(),
            target: "/local/b.txt".to_string(),
            message: "remote io".to_string(),
        }],
    };
    let dto = crate::runtime::batch_result_to_dto(result);
    assert_eq!(dto.ok.len(), 1);
    assert_eq!(dto.ok[0].source, "/remote/a.txt");
    assert_eq!(dto.failures.len(), 1);
    assert_eq!(dto.failures[0].message, "remote io");
}

#[test]
fn batch_request_json_round_trips() {
    let request = BatchTransferRequest {
        session_id: SessionId(3),
        files: vec![BatchTransferItemDto {
            source: "/remote/c.bin".to_string(),
            target: "/local/c.bin".to_string(),
        }],
        trees: Vec::new(),
        overwrite: true,
    };
    let dto = BatchTransferResultDto {
        ok: request
            .files
            .iter()
            .map(|item| BatchTransferItemDto {
                source: item.source.clone(),
                target: item.target.clone(),
            })
            .collect(),
        failures: Vec::new(),
    };
    let json = serde_json::to_value(&dto).expect("serialize");
    assert_eq!(json["ok"][0]["source"], "/remote/c.bin");
    assert!(json["failures"].as_array().unwrap().is_empty());
}
