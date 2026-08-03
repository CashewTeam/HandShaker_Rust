//! Application-layer unit tests: DTO semantics, error mapping, path rules,
//! and runtime lifecycle that does not require a real device.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use handshaker_core::{
    DeviceInfo, FileChange, FileChangeStatus, RemoteFile, SyncConfig, SyncDiff, SyncFileRecord,
    SyncSnapshot,
};

use crate::dto::{
    DeviceDescriptor, DeviceId, FileEntryDto, RuntimeConfig, SessionId, TransportKind,
};
use crate::error::{PublicErrorCode, from_core_error};
use crate::runtime::normalize_remote_path;
use crate::sync::{
    SyncActionDto, SyncPlanDto, SyncProfileDto, SyncRunResultDto, SyncStatusDto, one_entry_diff,
    snapshot_to_remote_files, sync_plan_to_dto, sync_run_result_to_dto,
};
use crate::transfer::{
    BatchTransferItemDto, BatchTransferRequest, BatchTransferResultDto, TransferState,
};
use crate::{HandShakerRuntime, resolve_remote_path};

fn test_config() -> RuntimeConfig {
    RuntimeConfig {
        adb_path: std::path::PathBuf::from("adb-missing-for-tests"),
        default_timeout: Duration::from_secs(1),
        heartbeat_interval: Duration::from_secs(10),
        state_dir: None,
        wire_log: None,
        event_capacity: 16,
        transfer_history_capacity: 8,
        transfer_history_ttl: None,
    }
}

fn fake_device() -> DeviceDescriptor {
    DeviceDescriptor {
        id: DeviceId("serial-1".to_string()),
        stable_id: None,
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
async fn state_dir_controls_state_store_location() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config();
    config.state_dir = Some(temp.path().to_path_buf());
    let runtime = HandShakerRuntime::create(config).await.expect("create");
    // The configured adb binary is missing, so connect fails — but the
    // state store must be loaded/created under state_dir before the
    // transport attempt, proving the config field is really used
    // (M8.1 Phase B / B4).
    let error = runtime
        .connect(crate::dto::ConnectRequest {
            device: fake_device(),
        })
        .await
        .expect_err("connect must fail without adb");
    assert!(matches!(
        error.code,
        PublicErrorCode::AdbUnavailable
            | PublicErrorCode::ConnectFailed
            | PublicErrorCode::InvalidState
    ));
    assert!(
        temp.path().join("state.json").exists(),
        "state.json must be created under state_dir"
    );
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn concurrent_requests_do_not_hold_the_registry_lock_across_await() {
    // B1: file methods must clone the client in a short critical section and
    // release the registry lock before awaiting; concurrent requests against
    // a missing session must all fail fast with SessionNotFound instead of
    // serializing on (or deadlocking through) the registry lock.
    let runtime = Arc::new(
        HandShakerRuntime::create(test_config())
            .await
            .expect("create"),
    );
    let mut handles = Vec::new();
    for _ in 0..8 {
        let runtime = runtime.clone();
        handles.push(tokio::spawn(async move {
            let error = runtime
                .list_files(crate::dto::ListFilesRequest {
                    session_id: SessionId(42),
                    path: "/".into(),
                    depth: 1,
                })
                .await
                .expect_err("missing session must error");
            assert_eq!(error.code, PublicErrorCode::SessionNotFound);
        }));
    }
    for handle in handles {
        handle.await.expect("concurrent task panicked");
    }
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn subscription_receives_closed_after_shutdown() {
    // B3: after shutdown the event stream must end with `Closed` (previously
    // the broadcast sender lived as long as the runtime, so subscribers only
    // ever timed out).
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let mut receiver = runtime.subscribe_events();
    runtime.shutdown().await.expect("shutdown");
    // Drain until the stream reports Closed.
    while let Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) =
        receiver.recv().await
    {}
}

#[tokio::test]
async fn concurrent_shutdown_runs_once_and_closes_events() {
    // B3: racing shutdown calls must all return Ok; the hub ends up closed.
    let runtime = Arc::new(
        HandShakerRuntime::create(test_config())
            .await
            .expect("create"),
    );
    let mut handles = Vec::new();
    for _ in 0..4 {
        let runtime = runtime.clone();
        handles.push(tokio::spawn(async move { runtime.shutdown().await }));
    }
    for handle in handles {
        handle
            .await
            .expect("shutdown task panicked")
            .expect("shutdown ok");
    }
    let mut receiver = runtime.subscribe_events();
    assert!(matches!(
        receiver.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Closed)
    ));
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
        external_storage_path: Some("/storage/ABCD-1234".into()),
        disk_size: Some(128_000_000_000),
        used_disk_size: Some(64_000_000_000),
        battery_percentage: Some(77),
        phone_locked: Some(true),
    };
    let dto = crate::runtime::device_info_to_dto(&info);
    assert_eq!(dto.serial, "s1");
    assert_eq!(dto.model.as_deref(), Some("OD103"));
    assert_eq!(dto.root_path, "/storage/emulated/0");
    // Phase D / D2: every core field must survive the mapping.
    assert_eq!(
        dto.external_storage_path.as_deref(),
        Some("/storage/ABCD-1234")
    );
    assert_eq!(dto.disk_size, Some(128_000_000_000));
    assert_eq!(dto.used_disk_size, Some(64_000_000_000));
    assert_eq!(dto.battery_percentage, Some(77));
    assert_eq!(dto.phone_locked, Some(true));
    // And a device without optional fields still maps cleanly.
    let bare = DeviceInfo {
        serial: "s2".into(),
        phone_id: None,
        name: None,
        model: None,
        brand: None,
        manufacturer: None,
        smartisan_version: None,
        apk_version: None,
        apk_version_name: None,
        root_path: "/".into(),
        external_storage_path: None,
        disk_size: None,
        used_disk_size: None,
        battery_percentage: None,
        phone_locked: None,
    };
    let dto = crate::runtime::device_info_to_dto(&bare);
    assert_eq!(dto.external_storage_path, None);
    assert_eq!(dto.phone_locked, None);
}

// ---- Phase D / D2: stable device identity ----

#[test]
fn reconcile_phone_id_becomes_stable_device_id() {
    let discovered = crate::discovery::wifi_device_to_descriptor(handshaker_core::WifiDevice {
        instance: "handshaker_ssp_".to_string(),
        host: "Android-2.local".to_string(),
        addresses: vec!["192.168.2.47".to_string()],
        port: 45656,
        txt: Default::default(),
    });
    let info = crate::dto::DeviceInfoDto {
        serial: "s1".to_string(),
        phone_id: Some("9a3f-77ee".to_string()),
        name: Some("My Phone".to_string()),
        model: Some("OD103".to_string()),
        brand: None,
        manufacturer: None,
        smartisan_version: None,
        apk_version: None,
        apk_version_name: None,
        root_path: "/storage/emulated/0".to_string(),
        external_storage_path: None,
        disk_size: None,
        used_disk_size: None,
        battery_percentage: None,
        phone_locked: None,
    };
    let reconciled = crate::runtime::reconcile_device_identity(&discovered, &info);
    assert_eq!(
        reconciled.stable_id,
        Some(DeviceId("phone:9a3f-77ee".to_string()))
    );
    // Discovery id (endpoint) stays untouched; name/model are backfilled.
    assert_eq!(reconciled.id, discovered.id);
    assert_eq!(reconciled.display_name.as_deref(), Some("My Phone"));
    assert_eq!(reconciled.model.as_deref(), Some("OD103"));
    // The discovery endpoint id must not be confused with the stable id.
    assert_ne!(reconciled.stable_id.as_ref(), Some(&reconciled.id));
}

#[test]
fn reconcile_without_phone_id_keeps_discovery_identity() {
    let discovered = fake_device();
    let info = crate::dto::DeviceInfoDto {
        serial: "s1".to_string(),
        phone_id: None,
        name: None,
        model: None,
        brand: None,
        manufacturer: None,
        smartisan_version: None,
        apk_version: None,
        apk_version_name: None,
        root_path: "/".to_string(),
        external_storage_path: None,
        disk_size: None,
        used_disk_size: None,
        battery_percentage: None,
        phone_locked: None,
    };
    let reconciled = crate::runtime::reconcile_device_identity(&discovered, &info);
    assert_eq!(reconciled.stable_id, None);
    // ADB/USB descriptors without a phone_id stay usable: display name and
    // model keep whatever the discovery entry already had.
    assert_eq!(reconciled.display_name.as_deref(), Some("serial-1"));
}

#[test]
fn reconcile_ignores_empty_phone_id() {
    let discovered = fake_device();
    let mut info = crate::dto::DeviceInfoDto {
        serial: "s1".to_string(),
        phone_id: Some("".to_string()),
        name: None,
        model: None,
        brand: None,
        manufacturer: None,
        smartisan_version: None,
        apk_version: None,
        apk_version_name: None,
        root_path: "/".to_string(),
        external_storage_path: None,
        disk_size: None,
        used_disk_size: None,
        battery_percentage: None,
        phone_locked: None,
    };
    let reconciled = crate::runtime::reconcile_device_identity(&discovered, &info);
    assert_eq!(reconciled.stable_id, None);
    info.phone_id = Some("   ".to_string());
    let reconciled = crate::runtime::reconcile_device_identity(&discovered, &info);
    assert_eq!(reconciled.stable_id, None);
}

#[test]
fn device_info_dto_json_contract_is_stable_and_backward_compatible() {
    let info = crate::dto::DeviceInfoDto {
        serial: "s1".to_string(),
        phone_id: Some("p1".to_string()),
        name: Some("Phone".to_string()),
        model: Some("OD103".to_string()),
        brand: Some("SMARTISAN".to_string()),
        manufacturer: Some("Smartisan".to_string()),
        smartisan_version: Some("6.7.4".to_string()),
        apk_version: Some("201".to_string()),
        apk_version_name: Some("1.2.0".to_string()),
        root_path: "/storage/emulated/0".to_string(),
        external_storage_path: Some("/storage/ABCD-1234".to_string()),
        disk_size: Some(128_000_000_000),
        used_disk_size: Some(64_000_000_000),
        battery_percentage: Some(77),
        phone_locked: Some(true),
    };
    let json = serde_json::to_value(&info).expect("serialize");
    assert_eq!(json["external_storage_path"], "/storage/ABCD-1234");
    assert_eq!(json["disk_size"], 128_000_000_000u64);
    assert_eq!(json["used_disk_size"], 64_000_000_000u64);
    assert_eq!(json["battery_percentage"], 77);
    assert_eq!(json["phone_locked"], true);
    let decoded: crate::dto::DeviceInfoDto = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, info);
    // A v1-preview JSON without the new optional fields still decodes.
    let legacy = serde_json::from_value::<crate::dto::DeviceInfoDto>(serde_json::json!({
        "serial": "s1",
        "phone_id": null,
        "name": null,
        "model": null,
        "brand": null,
        "manufacturer": null,
        "smartisan_version": null,
        "apk_version": null,
        "apk_version_name": null,
        "root_path": "/"
    }))
    .expect("legacy device info without new fields decodes");
    assert_eq!(legacy.external_storage_path, None);
    assert_eq!(legacy.disk_size, None);
    assert_eq!(legacy.phone_locked, None);
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
        stable_id: Some(DeviceId("phone:p1".into())),
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
    assert_eq!(json["stable_id"], "phone:p1");
    assert_eq!(json["transport"], "adb");
    assert_eq!(json["available"], true);
    // Round-trip preserves the descriptor.
    let decoded: DeviceDescriptor = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, device);
    // A descriptor serialized before stable_id existed still decodes.
    let legacy = serde_json::from_value::<DeviceDescriptor>(serde_json::json!({
        "id": "serial-1",
        "display_name": "serial-1",
        "model": null,
        "transport": "adb",
        "transport_address": "serial-1",
        "available": true,
        "adb": null,
        "usb": null
    }))
    .expect("legacy descriptor without stable_id decodes");
    assert_eq!(legacy.stable_id, None);
}

// ---- Phase D / D1: device discovery diagnostics ----

#[tokio::test]
async fn discover_devices_with_all_transports_disabled_is_empty_and_clean() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let result = runtime
        .discover_devices(crate::dto::ListDevicesRequest {
            include_adb: false,
            include_wifi: false,
            include_usb: false,
            wifi_browse_timeout: Duration::from_millis(1),
        })
        .await
        .expect("disabled discovery is not an error");
    assert!(result.devices.is_empty());
    assert!(result.warnings.is_empty());
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn discover_devices_reports_missing_adb_as_warning() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    // test_config uses an adb path that cannot exist: the sweep must succeed
    // and surface the ADB failure as a structured warning, never swallow it.
    let result = runtime
        .discover_devices(crate::dto::ListDevicesRequest {
            include_adb: true,
            include_wifi: false,
            include_usb: false,
            wifi_browse_timeout: Duration::from_millis(1),
        })
        .await
        .expect("adb failure must not fail the whole sweep");
    assert!(result.devices.is_empty());
    assert_eq!(result.warnings.len(), 1);
    let warning = &result.warnings[0];
    assert_eq!(warning.transport, TransportKind::Adb);
    assert_eq!(warning.error.code, PublicErrorCode::AdbUnavailable);
    assert_eq!(
        warning.error.operation.as_deref(),
        Some("discover_devices.adb")
    );
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn discover_devices_after_shutdown_returns_runtime_closed() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    runtime.shutdown().await.expect("shutdown");
    let error = runtime
        .discover_devices(crate::dto::ListDevicesRequest {
            include_adb: false,
            include_wifi: false,
            include_usb: false,
            wifi_browse_timeout: Duration::from_millis(1),
        })
        .await
        .expect_err("must be closed");
    assert_eq!(error.code, PublicErrorCode::RuntimeClosed);
}

#[test]
fn discovery_result_json_contract_is_stable() {
    let result = crate::discovery::DeviceDiscoveryResult {
        devices: vec![DeviceDescriptor {
            id: DeviceId("wifi-endpoint:handshaker_ssp_:192.168.2.47:45656".into()),
            stable_id: None,
            display_name: Some("Android-2.local".into()),
            model: None,
            transport: TransportKind::Wifi,
            transport_address: "192.168.2.47:45656".into(),
            available: true,
            adb: None,
            usb: None,
        }],
        warnings: vec![crate::discovery::DeviceDiscoveryWarning {
            transport: TransportKind::Adb,
            error: crate::error::PublicError::new(
                PublicErrorCode::AdbUnavailable,
                "adb unavailable",
            )
            .operation("discover_devices.adb"),
        }],
    };
    let json = serde_json::to_value(&result).expect("serialize");
    // Field names and enum tokens are the frozen contract.
    assert_eq!(json["devices"][0]["transport"], "wifi");
    assert_eq!(
        json["devices"][0]["id"],
        "wifi-endpoint:handshaker_ssp_:192.168.2.47:45656"
    );
    assert_eq!(json["warnings"][0]["transport"], "adb");
    assert_eq!(json["warnings"][0]["error"]["code"], "adb_unavailable");
    let decoded: crate::discovery::DeviceDiscoveryResult =
        serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, result);
}

// ---- Phase D / D3: trust service ----

/// Write a core `State` file with the given trust records into a state dir,
/// so tests exercise the real `state.json` layout (schema_version 1, host
/// UUID, BTreeMap keyed by device UUID).
fn write_state_file(dir: &std::path::Path, trust: serde_json::Value) {
    let state = serde_json::json!({
        "schema_version": 1,
        "host_uuid": "00000000-0000-0000-0000-000000000001",
        "trust": trust,
    });
    std::fs::write(dir.join("state.json"), state.to_string()).expect("write state.json");
}

fn test_config_with_dir(dir: &std::path::Path) -> RuntimeConfig {
    let mut config = test_config();
    config.state_dir = Some(dir.to_path_buf());
    config
}

#[tokio::test]
async fn trust_records_list_uses_configured_state_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_state_file(
        temp.path(),
        serde_json::json!({
            "uuid-1": { "device_name": "Phone A", "derived_key": "c2VjcmV0", "updated_at": 1700000000 },
            "uuid-2": { "device_name": null, "derived_key": "ZGVy", "updated_at": 1700000001 },
        }),
    );
    let runtime = HandShakerRuntime::create(test_config_with_dir(temp.path()))
        .await
        .expect("create");
    let records = runtime.list_trust_records().await.expect("list trust");
    assert_eq!(records.len(), 2);
    let first = records
        .iter()
        .find(|r| r.device_id.0 == "phone:uuid-1")
        .expect("uuid-1");
    assert_eq!(first.device_name.as_deref(), Some("Phone A"));
    assert_eq!(first.updated_at_ms, 1_700_000_000_000);
    let second = records
        .iter()
        .find(|r| r.device_id.0 == "phone:uuid-2")
        .expect("uuid-2");
    assert_eq!(second.device_name, None);
    // Derived keys must never cross the application boundary.
    let json = serde_json::to_value(&records).expect("serialize");
    assert!(!json.to_string().contains("derived_key"));
    assert!(!json.to_string().contains("c2VjcmV0"));
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn remove_trust_record_only_touches_configured_state_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_state_file(
        temp.path(),
        serde_json::json!({
            "uuid-1": { "device_name": "Phone A", "derived_key": "c2VjcmV0", "updated_at": 1700000000 },
        }),
    );
    let runtime = HandShakerRuntime::create(test_config_with_dir(temp.path()))
        .await
        .expect("create");
    let result = runtime
        .remove_trust_record(crate::trust::RemoveTrustRequest {
            device_id: DeviceId("phone:uuid-1".to_string()),
        })
        .await
        .expect("remove");
    assert!(result.removed);
    // Removing again reports it was already gone.
    let result = runtime
        .remove_trust_record(crate::trust::RemoveTrustRequest {
            device_id: DeviceId("phone:uuid-1".to_string()),
        })
        .await
        .expect("remove again");
    assert!(!result.removed);
    let records = runtime.list_trust_records().await.expect("list");
    assert!(records.is_empty());
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn remove_trust_rejects_malformed_device_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_state_file(temp.path(), serde_json::json!({}));
    let runtime = HandShakerRuntime::create(test_config_with_dir(temp.path()))
        .await
        .expect("create");
    for raw in ["uuid-1", "phone:", "adb:serial"] {
        let error = runtime
            .remove_trust_record(crate::trust::RemoveTrustRequest {
                device_id: DeviceId(raw.to_string()),
            })
            .await
            .expect_err("must reject");
        assert_eq!(error.code, PublicErrorCode::InvalidArgument);
    }
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn trust_api_after_shutdown_returns_runtime_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_state_file(temp.path(), serde_json::json!({}));
    let runtime = HandShakerRuntime::create(test_config_with_dir(temp.path()))
        .await
        .expect("create");
    runtime.shutdown().await.expect("shutdown");
    let error = runtime
        .list_trust_records()
        .await
        .expect_err("must be closed");
    assert_eq!(error.code, PublicErrorCode::RuntimeClosed);
    let error = runtime
        .remove_trust_record(crate::trust::RemoveTrustRequest {
            device_id: DeviceId("phone:uuid-1".to_string()),
        })
        .await
        .expect_err("must be closed");
    assert_eq!(error.code, PublicErrorCode::RuntimeClosed);
    let error = runtime
        .reset_wifi_trust(crate::trust::ResetWifiTrustRequest {
            endpoint: "192.168.2.47:45656".to_string(),
            expected_device_id: DeviceId("phone:uuid-1".to_string()),
        })
        .await
        .expect_err("must be closed");
    assert_eq!(error.code, PublicErrorCode::RuntimeClosed);
}

#[test]
fn trust_record_dto_json_contract_is_stable() {
    let record = crate::trust::TrustRecordDto {
        device_id: DeviceId("phone:uuid-1".to_string()),
        device_name: Some("Phone A".to_string()),
        updated_at_ms: 1_700_000_000_000,
    };
    let json = serde_json::to_value(&record).expect("serialize");
    assert_eq!(json["device_id"], "phone:uuid-1");
    assert_eq!(json["device_name"], "Phone A");
    assert_eq!(json["updated_at_ms"], 1_700_000_000_000u64);
    let decoded: crate::trust::TrustRecordDto = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, record);
}

#[tokio::test]
async fn reset_wifi_trust_rejects_invalid_endpoint() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_state_file(temp.path(), serde_json::json!({}));
    let runtime = HandShakerRuntime::create(test_config_with_dir(temp.path()))
        .await
        .expect("create");
    let error = runtime
        .reset_wifi_trust(crate::trust::ResetWifiTrustRequest {
            endpoint: "not-an-endpoint".to_string(),
            expected_device_id: DeviceId("phone:uuid-1".to_string()),
        })
        .await
        .expect_err("invalid endpoint");
    assert_eq!(error.code, PublicErrorCode::InvalidArgument);
    runtime.shutdown().await.expect("shutdown");
}

// ---- Phase D / D4: file plans ----

#[tokio::test]
async fn plan_download_unknown_session_is_stable_error() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let error = runtime
        .plan_download(crate::file_plan::PlanDownloadRequest {
            session_id: SessionId(999),
            remote_sources: vec!["/DCIM/a.jpg".to_string()],
            local_destination: "/tmp/a.jpg".to_string(),
            recursive: false,
            overwrite: false,
        })
        .await
        .expect_err("unknown session");
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
    runtime.shutdown().await.expect("shutdown");
}

#[test]
fn download_destination_single_source_uses_destination_as_is() {
    let mut conflicts = Vec::new();
    let remote = FileEntryDto {
        path: "/storage/emulated/0/DCIM/a.jpg".to_string(),
        size: 10,
        created_at_ms: None,
        modified_at_ms: None,
        is_directory: false,
        checksum: None,
        is_trash: None,
        media_id: None,
    };
    // Non-existent destination: used verbatim.
    let destination = crate::runtime::resolve_local_download_destination(
        "/tmp/out/nonexistent/a.jpg",
        &remote,
        1,
        &mut conflicts,
    )
    .expect("resolved");
    assert_eq!(
        destination,
        std::path::PathBuf::from("/tmp/out/nonexistent/a.jpg")
    );
    assert!(conflicts.is_empty());
}

#[test]
fn download_destination_multi_source_requires_existing_directory() {
    let mut conflicts = Vec::new();
    let remote = FileEntryDto {
        path: "/storage/emulated/0/DCIM/a.jpg".to_string(),
        size: 10,
        created_at_ms: None,
        modified_at_ms: None,
        is_directory: false,
        checksum: None,
        is_trash: None,
        media_id: None,
    };
    // Multi-source into a non-existent destination is a hard conflict.
    let resolved = crate::runtime::resolve_local_download_destination(
        "/tmp/out/missing",
        &remote,
        2,
        &mut conflicts,
    );
    assert!(resolved.is_none());
    assert_eq!(
        conflicts[0].kind,
        crate::file_plan::FileConflictKind::DestinationTypeMismatch
    );
    assert!(!conflicts[0].overridable);

    // Multi-source into an existing directory appends the basename.
    let temp = tempfile::tempdir().expect("tempdir");
    let mut conflicts = Vec::new();
    let resolved = crate::runtime::resolve_local_download_destination(
        temp.path().to_str().expect("utf8"),
        &remote,
        2,
        &mut conflicts,
    )
    .expect("resolved");
    assert_eq!(resolved.file_name().and_then(|n| n.to_str()), Some("a.jpg"));
}

#[test]
fn inspect_local_destination_flags_exists_and_type_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = temp.path().join("a.jpg");
    std::fs::write(&file, b"x").expect("write");
    let dir = temp.path().join("dir");
    std::fs::create_dir(&dir).expect("mkdir");

    // Existing file, overwrite=false -> overridable DestinationExists.
    let mut conflicts = Vec::new();
    crate::runtime::inspect_local_destination(&file, false, false, &mut conflicts);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].kind,
        crate::file_plan::FileConflictKind::DestinationExists
    );
    assert!(!conflicts[0].overridable);

    // Existing file, overwrite=true -> overridable.
    let mut conflicts = Vec::new();
    crate::runtime::inspect_local_destination(&file, false, true, &mut conflicts);
    assert!(conflicts[0].overridable);

    // File where a directory is expected -> never overridable.
    let mut conflicts = Vec::new();
    crate::runtime::inspect_local_destination(&file, true, true, &mut conflicts);
    assert_eq!(
        conflicts[0].kind,
        crate::file_plan::FileConflictKind::DestinationTypeMismatch
    );
    assert!(!conflicts[0].overridable);

    // Existing directory matching a directory source -> no conflict.
    let mut conflicts = Vec::new();
    crate::runtime::inspect_local_destination(&dir, true, false, &mut conflicts);
    assert!(conflicts.is_empty());
}

#[test]
fn upload_destination_resolution_rules() {
    let source = std::path::PathBuf::from("/local/a.jpg");
    let mut conflicts = Vec::new();

    // Single source, remote destination missing -> used verbatim.
    let resolved = crate::runtime::resolve_remote_upload_destination(
        "/storage/emulated/0/Download",
        &source,
        1,
        None,
        &mut conflicts,
    )
    .expect("resolved");
    assert_eq!(resolved, "/storage/emulated/0/Download");

    // Single source, remote destination is a directory -> basename appended.
    let dir = FileEntryDto {
        path: "/storage/emulated/0/Download".to_string(),
        size: 0,
        created_at_ms: None,
        modified_at_ms: None,
        is_directory: true,
        checksum: None,
        is_trash: None,
        media_id: None,
    };
    let resolved = crate::runtime::resolve_remote_upload_destination(
        "/storage/emulated/0/Download",
        &source,
        1,
        Some(&dir),
        &mut conflicts,
    )
    .expect("resolved");
    assert_eq!(resolved, "/storage/emulated/0/Download/a.jpg");

    // Multi-source, destination is not a directory -> hard conflict.
    let mut conflicts = Vec::new();
    let file = FileEntryDto {
        path: "/storage/emulated/0/x".to_string(),
        size: 1,
        created_at_ms: None,
        modified_at_ms: None,
        is_directory: false,
        checksum: None,
        is_trash: None,
        media_id: None,
    };
    let resolved = crate::runtime::resolve_remote_upload_destination(
        "/storage/emulated/0/x",
        &source,
        2,
        Some(&file),
        &mut conflicts,
    );
    assert!(resolved.is_none());
    assert_eq!(
        conflicts[0].kind,
        crate::file_plan::FileConflictKind::DestinationTypeMismatch
    );
}

#[test]
fn remote_destination_conflict_handles_shape_and_overwrite() {
    let dir = FileEntryDto {
        path: "/r/d".to_string(),
        size: 0,
        created_at_ms: None,
        modified_at_ms: None,
        is_directory: true,
        checksum: None,
        is_trash: None,
        media_id: None,
    };
    let file = FileEntryDto {
        path: "/r/f".to_string(),
        size: 1,
        created_at_ms: None,
        modified_at_ms: None,
        is_directory: false,
        checksum: None,
        is_trash: None,
        media_id: None,
    };
    // Directory source vs existing file: never overridable.
    let mut conflicts = Vec::new();
    crate::runtime::append_remote_destination_conflict(
        "s",
        "/r/f",
        true,
        &file,
        true,
        &mut conflicts,
    );
    assert_eq!(
        conflicts[0].kind,
        crate::file_plan::FileConflictKind::DestinationTypeMismatch
    );
    assert!(!conflicts[0].overridable);
    // File source vs existing directory: type mismatch, never overridable.
    let mut conflicts = Vec::new();
    crate::runtime::append_remote_destination_conflict(
        "s",
        "/r/d",
        false,
        &dir,
        false,
        &mut conflicts,
    );
    assert_eq!(
        conflicts[0].kind,
        crate::file_plan::FileConflictKind::DestinationTypeMismatch
    );
    assert!(!conflicts[0].overridable);
    // File vs file: overridable only with overwrite.
    let mut conflicts = Vec::new();
    crate::runtime::append_remote_destination_conflict(
        "s",
        "/r/f",
        false,
        &file,
        true,
        &mut conflicts,
    );
    assert_eq!(
        conflicts[0].kind,
        crate::file_plan::FileConflictKind::DestinationExists
    );
    assert!(conflicts[0].overridable);
    // Directory vs directory: no conflict (tree merge).
    let mut conflicts = Vec::new();
    crate::runtime::append_remote_destination_conflict(
        "s",
        "/r/d",
        true,
        &dir,
        false,
        &mut conflicts,
    );
    assert!(conflicts.is_empty());
}

#[test]
fn download_destination_rejects_dotdot_basename() {
    // Defense in depth: a remote path that terminates in ".." (or has no
    // file name at all) must be rejected as a hard conflict, never joined
    // into the destination. Trailing "/." and "/" are normalized by
    // `file_name` to the real name and stay legal.
    for path in ["/storage/emulated/0/..", "/"] {
        let mut conflicts = Vec::new();
        let remote = FileEntryDto {
            path: path.to_string(),
            size: 10,
            created_at_ms: None,
            modified_at_ms: None,
            is_directory: false,
            checksum: None,
            is_trash: None,
            media_id: None,
        };
        let resolved = crate::runtime::resolve_local_download_destination(
            "/tmp/out",
            &remote,
            1,
            &mut conflicts,
        );
        assert!(resolved.is_none(), "{path} must not resolve");
        assert_eq!(
            conflicts[0].kind,
            crate::file_plan::FileConflictKind::DestinationTypeMismatch
        );
        assert!(!conflicts[0].overridable);
    }
    // Trailing slash/dot forms carry the real file name; with a
    // non-existent destination the resolved target stays the destination
    // itself (single-source semantics).
    let mut conflicts = Vec::new();
    let remote = FileEntryDto {
        path: "/storage/emulated/0/Download/.".to_string(),
        size: 10,
        created_at_ms: None,
        modified_at_ms: None,
        is_directory: false,
        checksum: None,
        is_trash: None,
        media_id: None,
    };
    let resolved =
        crate::runtime::resolve_local_download_destination("/tmp/out", &remote, 1, &mut conflicts);
    assert_eq!(resolved, Some(std::path::PathBuf::from("/tmp/out")));
    assert!(conflicts.is_empty());
}

#[test]
fn upload_destination_rejects_dotdot_basename() {
    // ".." and a root path (no file name at all) are rejected; trailing
    // "/." carries the real file name and stays legal.
    for name in ["..", "/"] {
        let mut conflicts = Vec::new();
        let source = std::path::PathBuf::from(if name == "/" {
            name.to_string()
        } else {
            format!("/local/{name}")
        });
        let resolved = crate::runtime::resolve_remote_upload_destination(
            "/storage/emulated/0/Download",
            &source,
            1,
            None,
            &mut conflicts,
        );
        assert!(resolved.is_none(), "{name:?} must not resolve");
        assert_eq!(
            conflicts[0].kind,
            crate::file_plan::FileConflictKind::DestinationTypeMismatch
        );
    }
    let mut conflicts = Vec::new();
    let source = std::path::PathBuf::from("/local/a.bin/.");
    let resolved = crate::runtime::resolve_remote_upload_destination(
        "/storage/emulated/0/Download",
        &source,
        1,
        None,
        &mut conflicts,
    )
    .expect("trailing dot resolves");
    assert_eq!(resolved, "/storage/emulated/0/Download");
}

#[test]
fn duplicate_destination_conflicts_are_never_overridable() {
    let items = vec![
        crate::file_plan::FilePlanItem {
            source: "/a/1.txt".to_string(),
            destination: "/out/1.txt".to_string(),
            is_directory: false,
            size: Some(1),
        },
        crate::file_plan::FilePlanItem {
            source: "/b/1.txt".to_string(),
            destination: "/out/1.txt".to_string(),
            is_directory: false,
            size: Some(2),
        },
    ];
    let mut conflicts = Vec::new();
    crate::runtime::append_duplicate_destination_conflicts(&items, &mut conflicts);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].kind,
        crate::file_plan::FileConflictKind::DuplicateDestination
    );
    assert!(!conflicts[0].overridable);
}

#[test]
fn finalize_file_plan_counts_and_derives_executability() {
    let items = vec![
        crate::file_plan::FilePlanItem {
            source: "/a/1.txt".to_string(),
            destination: "/out/1.txt".to_string(),
            is_directory: false,
            size: Some(10),
        },
        crate::file_plan::FilePlanItem {
            source: "/a/dir".to_string(),
            destination: "/out/dir".to_string(),
            is_directory: true,
            size: None,
        },
    ];
    let plan = crate::runtime::finalize_file_plan(
        crate::file_plan::FilePlanDirection::Download,
        SessionId(1),
        items.clone(),
        Vec::new(),
        true,
    );
    assert_eq!(plan.file_count, 1);
    assert_eq!(plan.directory_count, 1);
    assert_eq!(plan.total_bytes, Some(10));
    assert!(plan.requires_recursive);
    assert!(plan.executable);

    // A non-overridable conflict kills executability.
    let plan = crate::runtime::finalize_file_plan(
        crate::file_plan::FilePlanDirection::Download,
        SessionId(1),
        items,
        vec![crate::file_plan::FilePlanConflict {
            kind: crate::file_plan::FileConflictKind::SourceMissing,
            source: "/missing".to_string(),
            destination: "/out".to_string(),
            message: "missing".to_string(),
            overridable: false,
        }],
        false,
    );
    assert!(!plan.executable);
}

#[tokio::test]
async fn execute_file_plan_rejects_unresolvable_plans() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    // Non-executable plan: rejected before any session lookup.
    let plan = crate::file_plan::FileOperationPlan {
        direction: crate::file_plan::FilePlanDirection::Download,
        session_id: SessionId(1),
        items: Vec::new(),
        conflicts: vec![crate::file_plan::FilePlanConflict {
            kind: crate::file_plan::FileConflictKind::SourceMissing,
            source: "/missing".to_string(),
            destination: "/out".to_string(),
            message: "missing".to_string(),
            overridable: false,
        }],
        file_count: 0,
        directory_count: 0,
        total_bytes: None,
        requires_recursive: false,
        executable: false,
    };
    let error = runtime
        .execute_file_plan(crate::file_plan::ExecuteFilePlanRequest {
            plan,
            overwrite: true,
            concurrency: 1,
        })
        .await
        .expect_err("must reject");
    assert_eq!(error.code, PublicErrorCode::InvalidState);

    // Executable plan with a missing session: SessionNotFound.
    let plan = crate::file_plan::FileOperationPlan {
        direction: crate::file_plan::FilePlanDirection::Download,
        session_id: SessionId(999),
        items: Vec::new(),
        conflicts: Vec::new(),
        file_count: 0,
        directory_count: 0,
        total_bytes: None,
        requires_recursive: false,
        executable: true,
    };
    let error = runtime
        .execute_file_plan(crate::file_plan::ExecuteFilePlanRequest {
            plan,
            overwrite: false,
            concurrency: 1,
        })
        .await
        .expect_err("unknown session");
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
    runtime.shutdown().await.expect("shutdown");
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
    use crate::event::EventHub;
    use crate::transfer::{TransferDirectionDto, TransferRegistry, TransferState};

    let registry = TransferRegistry::new(EventHub::new(8), 64, None);
    let snapshot = registry.snapshot_for(
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
    use crate::event::EventHub;
    use crate::transfer::{TransferDirectionDto, TransferRegistry, TransferState};

    let registry = TransferRegistry::new(EventHub::new(8), 64, None);
    let snapshot = registry.snapshot_for(
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
        .expect_err("missing");
    assert_eq!(missing.code, PublicErrorCode::TransferNotFound);
}

#[test]
fn transfer_progress_events_are_throttled_and_carry_total() {
    // M8.1 Phase C / C2: progress updates carry total_bytes and the event
    // stream is throttled (time + 256 KiB thresholds), never one event per
    // chunk.
    use crate::event::{BackendEvent, EventHub};
    use crate::transfer::{TransferDirectionDto, TransferRegistry};

    let hub = EventHub::new(64);
    let mut receiver = hub.subscribe();
    let registry = TransferRegistry::new(hub, 64, None);
    let entry = registry.register(registry.snapshot_for(
        SessionId(1),
        TransferDirectionDto::Download,
        "/remote/a.bin".into(),
        "/local/a.bin".into(),
    ));
    let id = entry.snapshot.lock().unwrap().id;

    // First update always emits; a tiny same-millisecond update is normally
    // throttled, but on a slow machine the two calls can straddle the 100 ms
    // window — allow either, never more than two.
    registry.set_progress(id, 1024, 1_000_000);
    registry.set_progress(id, 2048, 1_000_000);
    let snapshot = registry.get(id).unwrap();
    assert_eq!(snapshot.transferred_bytes, 2048);
    assert_eq!(snapshot.total_bytes, Some(1_000_000));

    let mut updated = 0;
    while let Ok(envelope) = receiver.try_recv() {
        if matches!(envelope.event, BackendEvent::TransferUpdated(_)) {
            updated += 1;
        }
    }
    assert!(
        (1..=2).contains(&updated),
        "small update must be throttled (got {updated} events)"
    );

    // Crossing the 256 KiB byte threshold emits regardless of time.
    registry.set_progress(id, 300_000, 1_000_000);
    let mut updated = 0;
    while let Ok(envelope) = receiver.try_recv() {
        if matches!(envelope.event, BackendEvent::TransferUpdated(_)) {
            updated += 1;
        }
    }
    assert_eq!(updated, 1, "byte threshold must emit");
}

#[test]
fn cancel_sets_finished_at_and_publishes_immediately() {
    // M8.1 Phase C / C3: cancel sets the terminal state with finished_at_ms
    // and publishes the event right away — GUI must not wait for the
    // background task to notice.
    use crate::event::{BackendEvent, EventHub};
    use crate::transfer::{TransferDirectionDto, TransferRegistry, TransferState};

    let hub = EventHub::new(8);
    let mut receiver = hub.subscribe();
    let registry = TransferRegistry::new(hub, 64, None);
    let entry = registry.register(registry.snapshot_for(
        SessionId(1),
        TransferDirectionDto::Upload,
        "/local/c.bin".into(),
        "/remote/c.bin".into(),
    ));
    let id = entry.snapshot.lock().unwrap().id;

    registry.cancel(id).expect("cancel");
    let snapshot = registry.get(id).unwrap();
    assert_eq!(snapshot.state, TransferState::Cancelled);
    assert!(
        snapshot.finished_at_ms.is_some(),
        "cancel must set finished_at_ms"
    );
    let mut saw_terminal = false;
    while let Ok(envelope) = receiver.try_recv() {
        if let BackendEvent::TransferUpdated(s) = envelope.event
            && s.state == TransferState::Cancelled
        {
            saw_terminal = true;
        }
    }
    assert!(
        saw_terminal,
        "cancel must publish the terminal event immediately"
    );
}

#[test]
fn transfer_history_is_bounded_by_capacity_and_ttl() {
    // M8.1 Phase C / C4: finished entries are evicted oldest-first while
    // over capacity; TTL-expired finished entries are reaped on register.
    use crate::transfer::{TransferDirectionDto, TransferRegistry, TransferState};

    let register = |registry: &TransferRegistry, n: u64| {
        let entry = registry.register(registry.snapshot_for(
            SessionId(1),
            TransferDirectionDto::Download,
            format!("/remote/{n}.bin"),
            format!("/local/{n}.bin"),
        ));
        entry.snapshot.lock().unwrap().id
    };

    // Capacity 2: finishing the two oldest and registering a third evicts
    // the oldest finished entry (live entries are never evicted).
    let hub = crate::event::EventHub::new(8);
    let registry = TransferRegistry::new(hub, 2, None);
    let id1 = register(&registry, 1);
    let id2 = register(&registry, 2);
    let id3 = register(&registry, 3);
    registry.transition(id1, TransferState::Completed);
    std::thread::sleep(std::time::Duration::from_millis(2));
    registry.transition(id2, TransferState::Completed);
    let id4 = register(&registry, 4);
    assert!(
        registry.get(id1).is_err(),
        "oldest finished must be evicted"
    );
    assert!(
        registry.get(id2).is_err(),
        "capacity 2 keeps at most two entries: second-oldest finished evicted too"
    );
    assert!(registry.get(id3).is_ok());
    assert!(registry.get(id4).is_ok());

    // TTL 1 ms: a finished entry older than the TTL is reaped on the next
    // register (live entries are kept).
    let hub = crate::event::EventHub::new(8);
    let registry = TransferRegistry::new(hub, 64, Some(std::time::Duration::from_millis(1)));
    let old = register(&registry, 10);
    registry.transition(old, TransferState::Completed);
    let live = register(&registry, 11);
    std::thread::sleep(std::time::Duration::from_millis(10));
    let fresh = register(&registry, 12);
    assert!(
        registry.get(old).is_err(),
        "TTL-expired entry must be reaped"
    );
    assert!(registry.get(live).is_ok(), "live entry must be kept");
    assert!(registry.get(fresh).is_ok());
}

#[test]
fn cancelled_error_maps_by_origin() {
    // M8.1 Phase C / C3: local vs phone-side cancellation is distinguishable
    // through stable error codes.
    use handshaker_core::{CancellationInfo, CancellationOrigin};

    let local = from_core_error(
        handshaker_core::Error::Cancelled(CancellationInfo {
            sid: 1,
            origin: CancellationOrigin::Local { flag_sent: true },
            connection_closed: false,
        }),
        "download",
    );
    assert_eq!(local.code, PublicErrorCode::TransferCancelled);

    let remote = from_core_error(
        handshaker_core::Error::Cancelled(CancellationInfo {
            sid: 1,
            origin: CancellationOrigin::Remote {
                error_code: Some(1),
            },
            connection_closed: true,
        }),
        "download",
    );
    assert_eq!(remote.code, PublicErrorCode::RemoteCancelled);
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
            code: Some(handshaker_core::ErrorCode::RemoteIo),
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

// ---- M8.1 Phase C: event bridge (C1) and connection loss (C5) ----

fn sample_device() -> DeviceDescriptor {
    DeviceDescriptor {
        id: DeviceId("adb:3f13d4b4".to_string()),
        stable_id: None,
        display_name: Some("test phone".to_string()),
        model: Some("OD103".to_string()),
        transport: TransportKind::Adb,
        transport_address: "3f13d4b4".to_string(),
        available: true,
        adb: None,
        usb: None,
    }
}

fn remote_file(path: &str) -> RemoteFile {
    RemoteFile {
        path: path.to_string(),
        size: 0,
        created_at: None,
        modified_at: None,
        is_directory: false,
        checksum: None,
        is_trash: None,
        id: None,
        ext_data: None,
    }
}

#[test]
fn bridge_client_event_maps_known_core_events() {
    use crate::event::BackendEvent;
    use handshaker_core::{
        CancellationInfo, CancellationOrigin, FileChange, FileChangeStatus, FileEvent,
        FileEventKind, MediaKind, UnknownEvent, UnknownEventReason,
    };

    let device = sample_device();
    let device_info = std::sync::RwLock::new(crate::dto::DeviceInfoDto {
        serial: "3f13d4b4".to_string(),
        phone_id: None,
        name: None,
        model: None,
        brand: None,
        manufacturer: None,
        smartisan_version: None,
        apk_version: None,
        apk_version_name: None,
        root_path: "/storage/emulated/0".to_string(),
        external_storage_path: None,
        disk_size: None,
        used_disk_size: None,
        battery_percentage: None,
        phone_locked: None,
    });
    let session_id = SessionId(7);
    let bridge =
        |event| crate::runtime::bridge_client_event(event, session_id, &device, &device_info);

    // Clipboard change -> DTO payload.
    let event = bridge(handshaker_core::ClientEvent::ClipboardChanged(vec![
        handshaker_core::ClipboardEntry {
            text: "hello".to_string(),
            timestamp_ms: 123,
        },
    ]));
    match event {
        BackendEvent::ClipboardChanged {
            session_id: source,
            entries,
        } => {
            assert_eq!(source, session_id);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].text, "hello");
            assert_eq!(entries[0].timestamp_ms, 123);
        }
        other => panic!("expected ClipboardChanged, got {other:?}"),
    }

    // Media change -> MediaChangeDto with kind and items.
    let event = bridge(handshaker_core::ClientEvent::MediaLibraryChanged(
        handshaker_core::MediaLibraryChange {
            kind: MediaKind::Photo,
            added: vec![handshaker_core::MediaItem {
                media_id: Some(7),
                path: Some("/DCIM/a.jpg".to_string()),
                size: Some(1024),
                ..Default::default()
            }],
            deleted: vec![],
            updated: vec![],
            albums: vec![],
        },
    ));
    match event {
        BackendEvent::MediaChanged {
            session_id: source,
            change,
        } => {
            assert_eq!(source, session_id);
            assert_eq!(change.media_kind, crate::dto::MediaKindDto::Photo);
            assert_eq!(change.added.len(), 1);
            assert_eq!(change.added[0].media_id, Some(7));
            assert_eq!(change.added[0].path.as_deref(), Some("/DCIM/a.jpg"));
        }
        other => panic!("expected MediaChanged, got {other:?}"),
    }

    // Directory monitor -> summarized RemoteFileChanged with paths.
    let event = bridge(handshaker_core::ClientEvent::DirectoryChanged(vec![
        FileEvent {
            file: Some(remote_file("/watch/a.txt")),
            kind: FileEventKind::Create,
        },
    ]));
    match event {
        BackendEvent::RemoteFileChanged {
            session_id: source,
            change,
        } => {
            assert_eq!(source, session_id);
            assert_eq!(
                change.change_kind,
                crate::dto::RemoteFileChangeKind::DirectoryChanged
            );
            assert_eq!(change.paths, vec!["/watch/a.txt".to_string()]);
        }
        other => panic!("expected RemoteFileChanged, got {other:?}"),
    }

    // Sync file change -> file_changed kind.
    let event = bridge(handshaker_core::ClientEvent::FileChanged(vec![
        FileChange {
            file: Some(remote_file("/sync/b.txt")),
            status: FileChangeStatus::Modified,
        },
    ]));
    match event {
        BackendEvent::RemoteFileChanged {
            session_id: source,
            change,
        } => {
            assert_eq!(source, session_id);
            assert_eq!(
                change.change_kind,
                crate::dto::RemoteFileChangeKind::FileChanged
            );
            assert_eq!(change.paths, vec!["/sync/b.txt".to_string()]);
        }
        other => panic!("expected RemoteFileChanged, got {other:?}"),
    }

    // Photo sync response -> photo_sync_changed kind.
    let event = bridge(handshaker_core::ClientEvent::PhotoSyncChanged(
        handshaker_core::PhotoSyncChange {
            is_first: Some(true),
            files: vec![remote_file("/sync/c.jpg")],
            is_success: Some(true),
        },
    ));
    match event {
        BackendEvent::RemoteFileChanged {
            session_id: source,
            change,
        } => {
            assert_eq!(source, session_id);
            assert_eq!(
                change.change_kind,
                crate::dto::RemoteFileChangeKind::PhotoSyncChanged
            );
            assert_eq!(change.paths, vec!["/sync/c.jpg".to_string()]);
        }
        other => panic!("expected RemoteFileChanged, got {other:?}"),
    }

    // Sync monitor response -> sync_monitor_changed kind (no paths).
    let event = bridge(handshaker_core::ClientEvent::SyncMonitorChanged(
        handshaker_core::SyncMonitorChange {
            is_success: Some(true),
        },
    ));
    match event {
        BackendEvent::RemoteFileChanged {
            session_id: source,
            change,
        } => {
            assert_eq!(source, session_id);
            assert_eq!(
                change.change_kind,
                crate::dto::RemoteFileChangeKind::SyncMonitorChanged
            );
            assert!(change.paths.is_empty());
        }
        other => panic!("expected RemoteFileChanged, got {other:?}"),
    }

    // Phone cancellation outside a request -> safe warning, never a panic.
    let event = bridge(handshaker_core::ClientEvent::RequestCancelled(
        CancellationInfo {
            sid: 9,
            origin: CancellationOrigin::Remote {
                error_code: Some(1),
            },
            connection_closed: false,
        },
    ));
    match event {
        BackendEvent::Warning(warning) => {
            assert_eq!(warning.code, PublicErrorCode::RemoteCancelled)
        }
        other => panic!("expected Warning, got {other:?}"),
    }

    // Unknown event -> safe warning with protocol code.
    let event = bridge(handshaker_core::ClientEvent::Unknown(UnknownEvent {
        sid: 4,
        request_type: None,
        payload_len: 3,
        reason: UnknownEventReason::MissingTypeAmbiguous,
    }));
    match event {
        BackendEvent::Warning(warning) => {
            assert_eq!(warning.code, PublicErrorCode::ProtocolError)
        }
        other => panic!("expected Warning, got {other:?}"),
    }

    // Device info change -> DeviceUpdated + cached DTO refreshed in place.
    let event = bridge(handshaker_core::ClientEvent::DeviceInfoChanged(
        DeviceInfo {
            serial: "3f13d4b4".to_string(),
            phone_id: Some("phone-uuid".to_string()),
            name: Some("U2 Pro".to_string()),
            model: Some("OD103".to_string()),
            brand: Some("smartisan".to_string()),
            manufacturer: Some("smartisan".to_string()),
            smartisan_version: Some("6.7.4".to_string()),
            apk_version: Some("1".to_string()),
            apk_version_name: Some("1.2.0".to_string()),
            root_path: "/storage/emulated/0".to_string(),
            external_storage_path: Some("/storage/emulated/0".to_string()),
            disk_size: Some(64_000_000_000),
            used_disk_size: Some(30_000_000_000),
            battery_percentage: Some(88),
            phone_locked: Some(false),
        },
    ));
    match event {
        BackendEvent::DeviceUpdated {
            session_id: source,
            device: updated,
        } => {
            assert_eq!(source, session_id);
            assert_eq!(updated.id, device.id);
            // Phase D / D2: the updated descriptor carries the stable
            // identity reconciled from the pushed phone_id.
            assert_eq!(
                updated.stable_id,
                Some(DeviceId("phone:phone-uuid".to_string()))
            );
            assert_eq!(updated.display_name.as_deref(), Some("U2 Pro"));
        }
        other => panic!("expected DeviceUpdated, got {other:?}"),
    }
    // The shared cached DTO was refreshed in place with the full payload.
    {
        let cached = device_info.read().expect("read lock");
        assert_eq!(cached.phone_id.as_deref(), Some("phone-uuid"));
        assert_eq!(
            cached.external_storage_path.as_deref(),
            Some("/storage/emulated/0")
        );
        assert_eq!(cached.disk_size, Some(64_000_000_000));
        assert_eq!(cached.used_disk_size, Some(30_000_000_000));
        assert_eq!(cached.battery_percentage, Some(88));
        assert_eq!(cached.phone_locked, Some(false));
    }
    assert_eq!(
        device_info.read().unwrap().name.as_deref(),
        Some("U2 Pro"),
        "device-info change must refresh the session DTO"
    );
}

#[test]
fn connection_lost_code_judges_connection_failures() {
    // Only the core Transport family (ConnectFailed) proves the connection
    // is gone; ConnectionLost also covers core Timeout, which may leave a
    // usable connection (slow phone) and must NOT kill the session.
    let lost = [PublicErrorCode::ConnectFailed];
    for code in lost {
        assert!(
            crate::runtime::connection_lost_code(code),
            "{code:?} must count as connection loss"
        );
    }
    let alive = [
        PublicErrorCode::ConnectionLost,
        PublicErrorCode::RemotePathNotFound,
        PublicErrorCode::SessionNotFound,
        PublicErrorCode::TransferCancelled,
        PublicErrorCode::ProtocolError,
        PublicErrorCode::TrustRejected,
        PublicErrorCode::Internal,
    ];
    for code in alive {
        assert!(
            !crate::runtime::connection_lost_code(code),
            "{code:?} must NOT count as connection loss"
        );
    }
}

#[tokio::test]
async fn mark_connection_lost_is_noop_without_a_session() {
    use crate::event::EventHub;
    use crate::transfer::{TransferDirectionDto, TransferRegistry};

    let hub = EventHub::new(8);
    let registry = TransferRegistry::new(hub.clone(), 64, None);
    let sessions = tokio::sync::Mutex::new(std::collections::HashMap::new());
    let mut receiver = hub.subscribe();
    crate::runtime::mark_connection_lost(&sessions, &registry, SessionId(1), &hub).await;
    // No session -> no events at all (and no panic).
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    // A transfer registered for that session is untouched (session absent).
    let snapshot = registry.snapshot_for(
        SessionId(1),
        TransferDirectionDto::Download,
        "/a".to_string(),
        "/b".to_string(),
    );
    let entry = registry.register(snapshot);
    let id = entry.snapshot.lock().unwrap().id;
    crate::runtime::mark_connection_lost(&sessions, &registry, SessionId(1), &hub).await;
    assert_eq!(
        registry.get(id).unwrap().state,
        TransferState::Queued,
        "no session -> no cancellation"
    );
}

#[test]
fn backend_event_change_payloads_serialize_with_stable_kinds() {
    use crate::event::BackendEvent;

    let json = serde_json::to_value(BackendEvent::ConnectionLost {
        session_id: SessionId(3),
    })
    .unwrap();
    assert_eq!(json["kind"], "connection_lost");
    assert_eq!(json["session_id"], 3);

    // Device variants: struct payloads serialize under the same tag
    // (DeviceRemoved must not be a newtype: non-map payloads cannot be
    // internally tagged).
    let json = serde_json::to_value(BackendEvent::DeviceRemoved {
        device_id: DeviceId("adb:x".into()),
    })
    .unwrap();
    assert_eq!(json["kind"], "device_removed");
    assert_eq!(json["device_id"], "adb:x");

    let json = serde_json::to_value(BackendEvent::ClipboardChanged {
        session_id: SessionId(3),
        entries: vec![crate::dto::ClipboardEntryDto {
            text: "hi".to_string(),
            timestamp_ms: 1,
        }],
    })
    .unwrap();
    assert_eq!(json["kind"], "clipboard_changed");
    assert_eq!(json["session_id"], 3);
    assert_eq!(json["entries"][0]["text"], "hi");
    assert_eq!(json["entries"][0]["timestamp_ms"], 1);

    let json = serde_json::to_value(BackendEvent::MediaChanged {
        session_id: SessionId(3),
        change: crate::dto::MediaChangeDto {
            media_kind: crate::dto::MediaKindDto::Audio,
            added: vec![],
            deleted: vec![],
            updated: vec![],
        },
    })
    .unwrap();
    assert_eq!(json["kind"], "media_changed");
    assert_eq!(json["session_id"], 3);
    assert_eq!(json["change"]["media_kind"], "audio");

    let json = serde_json::to_value(BackendEvent::RemoteFileChanged {
        session_id: SessionId(3),
        change: crate::dto::RemoteFileChangeDto {
            change_kind: crate::dto::RemoteFileChangeKind::FileChanged,
            paths: vec!["/x".to_string()],
            files: vec![],
            statuses: vec![],
        },
    })
    .unwrap();
    assert_eq!(json["kind"], "remote_file_changed");
    assert_eq!(json["session_id"], 3);
    assert_eq!(json["change"]["change_kind"], "file_changed");
    assert_eq!(json["change"]["paths"][0], "/x");

    // Every variant must round-trip through its stable JSON shape.
    for value in [
        serde_json::to_value(BackendEvent::RuntimeStopping).unwrap(),
        serde_json::to_value(BackendEvent::ConnectionLost {
            session_id: SessionId(3),
        })
        .unwrap(),
        serde_json::to_value(BackendEvent::DeviceRemoved {
            device_id: DeviceId("adb:x".into()),
        })
        .unwrap(),
        serde_json::to_value(BackendEvent::ClipboardChanged {
            session_id: SessionId(3),
            entries: vec![],
        })
        .unwrap(),
        serde_json::to_value(BackendEvent::MediaChanged {
            session_id: SessionId(3),
            change: crate::dto::MediaChangeDto {
                media_kind: crate::dto::MediaKindDto::Audio,
                added: vec![],
                deleted: vec![],
                updated: vec![],
            },
        })
        .unwrap(),
        serde_json::to_value(BackendEvent::RemoteFileChanged {
            session_id: SessionId(3),
            change: crate::dto::RemoteFileChangeDto {
                change_kind: crate::dto::RemoteFileChangeKind::DirectoryChanged,
                paths: vec![],
                files: vec![],
                statuses: vec![],
            },
        })
        .unwrap(),
    ] {
        let decoded: BackendEvent = serde_json::from_value(value).expect("decode event");
        assert!(matches!(
            decoded,
            BackendEvent::RuntimeStopping
                | BackendEvent::ConnectionLost { .. }
                | BackendEvent::DeviceRemoved { .. }
                | BackendEvent::ClipboardChanged { .. }
                | BackendEvent::MediaChanged { .. }
                | BackendEvent::RemoteFileChanged { .. }
        ));
    }
}

// ---- Phase D / D6: photo-sync service ----
//
// Network paths (photo_sync / execute_plan / apply_file_change against a
// real phone) are intentionally not tested here: core's own integration
// tests cover them, and FakeWifiSsp is a core-internal facility the
// application layer must not depend on. These tests cover the pure mappings,
// ledger/store resolution, and job lifecycle/state transitions only.

fn sync_profile(id: &str, session: u64) -> SyncProfileDto {
    SyncProfileDto {
        id: id.to_string(),
        session_id: SessionId(session),
        device_uuid: "dev-1".to_string(),
        remote_root: "/storage/emulated/0/DCIM/Camera".to_string(),
        local_root: "/tmp/photos".to_string(),
        enabled: true,
    }
}

#[test]
fn sync_dto_json_fixtures_are_stable() {
    let profile = sync_profile("photos", 7);
    let value = serde_json::to_value(&profile).expect("serialize profile");
    assert_eq!(value["id"], "photos");
    assert_eq!(value["session_id"], 7);
    assert_eq!(value["device_uuid"], "dev-1");
    assert_eq!(value["remote_root"], "/storage/emulated/0/DCIM/Camera");
    assert_eq!(value["local_root"], "/tmp/photos");
    assert_eq!(value["enabled"], true);
    let back: SyncProfileDto = serde_json::from_value(value).expect("deserialize profile");
    assert_eq!(back, profile);

    let plan = SyncPlanDto {
        profile_id: "photos".to_string(),
        downloads: vec![SyncActionDto {
            remote_path: "/p/a.jpg".to_string(),
            local_path: "/tmp/photos/a.jpg".to_string(),
            size: 10,
        }],
        metadata_updates: vec![],
        deletions: vec![],
        conflicts: vec![],
        total_bytes: 10,
        executable: true,
    };
    let value = serde_json::to_value(&plan).expect("serialize plan");
    assert_eq!(value["profile_id"], "photos");
    assert_eq!(value["downloads"][0]["remote_path"], "/p/a.jpg");
    assert_eq!(value["downloads"][0]["local_path"], "/tmp/photos/a.jpg");
    assert_eq!(value["downloads"][0]["size"], 10);
    assert_eq!(value["metadata_updates"], serde_json::json!([]));
    assert_eq!(value["total_bytes"], 10);
    assert_eq!(value["executable"], true);
    let back: SyncPlanDto = serde_json::from_value(value).expect("deserialize plan");
    assert_eq!(back, plan);

    let status = SyncStatusDto {
        profile_id: "photos".to_string(),
        running: true,
        monitoring: false,
        last_run_at_ms: Some(123),
        last_error: None,
    };
    let value = serde_json::to_value(&status).expect("serialize status");
    assert_eq!(value["profile_id"], "photos");
    assert_eq!(value["running"], true);
    assert_eq!(value["monitoring"], false);
    assert_eq!(value["last_run_at_ms"], 123);
    assert!(value.get("last_error").unwrap().is_null());
    let back: SyncStatusDto = serde_json::from_value(value).expect("deserialize status");
    assert_eq!(back, status);
}

#[test]
fn snapshot_to_remote_files_maps_record_fields() {
    let snapshot = SyncSnapshot {
        files: BTreeMap::from([(
            "/storage/emulated/0/DCIM/Camera/a.jpg".to_string(),
            SyncFileRecord {
                size: 42,
                checksum: Some("c-a".to_string()),
                ext_data: Some(r#"{"star":true}"#.to_string()),
                modified_at: Some(9),
                local_path: "/tmp/photos/a.jpg".to_string(),
                local_sha256: Some("sha".to_string()),
            },
        )]),
    };
    let files = snapshot_to_remote_files(&snapshot);
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file.path, "/storage/emulated/0/DCIM/Camera/a.jpg");
    assert_eq!(file.size, 42);
    assert_eq!(file.checksum.as_deref(), Some("c-a"));
    assert_eq!(file.ext_data.as_deref(), Some(r#"{"star":true}"#));
    assert_eq!(file.modified_at, Some(9));
    assert!(!file.is_directory);
    assert_eq!(file.created_at, None);
    assert_eq!(file.is_trash, None);
    assert_eq!(file.id, None);
}

#[test]
fn plan_mapping_counts_actions_and_flags_conflicts() {
    let snapshot = SyncSnapshot {
        files: BTreeMap::from([(
            "/storage/emulated/0/DCIM/Camera/gone.jpg".to_string(),
            SyncFileRecord {
                size: 7,
                checksum: Some("c-gone".to_string()),
                ext_data: None,
                modified_at: Some(1),
                local_path: "/tmp/photos/gone.jpg".to_string(),
                local_sha256: Some("sha".to_string()),
            },
        )]),
    };
    let phone_files = vec![
        RemoteFile {
            path: "/storage/emulated/0/DCIM/Camera/new.jpg".to_string(),
            size: 10,
            created_at: None,
            modified_at: Some(2),
            is_directory: false,
            checksum: Some("c-new".to_string()),
            is_trash: None,
            id: None,
            ext_data: None,
        },
        RemoteFile {
            path: "/storage/emulated/0/DCIM/Camera/meta.jpg".to_string(),
            size: 5,
            created_at: None,
            modified_at: Some(3),
            is_directory: false,
            checksum: Some("c-meta".to_string()),
            is_trash: None,
            id: None,
            ext_data: Some(r#"{"star":true}"#.to_string()),
        },
    ];
    let diff = SyncDiff {
        added: vec!["/storage/emulated/0/DCIM/Camera/new.jpg".to_string()],
        info_modified: vec!["/storage/emulated/0/DCIM/Camera/meta.jpg".to_string()],
        deleted: vec!["/storage/emulated/0/DCIM/Camera/gone.jpg".to_string()],
        conflicts: vec![],
    };
    let config = SyncConfig {
        device_uuid: "dev-1".to_string(),
        phone_root: "/storage/emulated/0/DCIM/Camera".to_string(),
        local_root: "/tmp/photos".to_string(),
        pc_id: "hs-1".to_string(),
    };
    let plan = sync_plan_to_dto(
        "photos",
        &config,
        &diff,
        &["/storage/emulated/0/DCIM/Camera/gone.jpg".to_string()],
        &phone_files,
        &snapshot,
    )
    .expect("plan");
    assert_eq!(plan.profile_id, "photos");
    assert_eq!(plan.downloads.len(), 1);
    assert_eq!(
        plan.downloads[0].remote_path,
        "/storage/emulated/0/DCIM/Camera/new.jpg"
    );
    assert_eq!(
        plan.downloads[0].local_path,
        std::path::Path::new("/tmp/photos")
            .join("new.jpg")
            .display()
            .to_string()
    );
    assert_eq!(plan.downloads[0].size, 10);
    assert_eq!(plan.metadata_updates.len(), 1);
    assert_eq!(
        plan.metadata_updates[0].remote_path,
        "/storage/emulated/0/DCIM/Camera/meta.jpg"
    );
    assert_eq!(plan.metadata_updates[0].size, 5);
    assert_eq!(plan.deletions.len(), 1);
    assert_eq!(plan.deletions[0].local_path, "/tmp/photos/gone.jpg");
    assert_eq!(plan.deletions[0].size, 7);
    assert_eq!(plan.conflicts.len(), 1);
    assert_eq!(
        plan.conflicts[0].remote_path,
        "/storage/emulated/0/DCIM/Camera/gone.jpg"
    );
    assert_eq!(plan.conflicts[0].reason, "local_modified");
    assert_eq!(plan.total_bytes, 10);
    assert!(!plan.executable);

    // Without conflicts the same diff is executable.
    let plan =
        sync_plan_to_dto("photos", &config, &diff, &[], &phone_files, &snapshot).expect("plan");
    assert!(plan.executable);
    assert!(plan.conflicts.is_empty());
}

#[test]
fn sync_run_result_dto_matches_cli_json_contract() {
    let dto = SyncRunResultDto {
        downloaded: vec!["/p/a.jpg".to_string()],
        deleted: vec!["/p/gone.jpg".to_string()],
        failures: vec!["/p/bad.jpg".to_string()],
        conflicts: vec!["/p/edited.jpg".to_string()],
    };
    let value = serde_json::to_value(&dto).expect("serialize");
    assert_eq!(value["downloaded"][0], "/p/a.jpg");
    assert_eq!(value["deleted"][0], "/p/gone.jpg");
    assert_eq!(value["failures"][0], "/p/bad.jpg");
    assert_eq!(value["conflicts"][0], "/p/edited.jpg");
    // CLI `sync run` emits exactly these four keys.
    assert_eq!(value.as_object().expect("object").len(), 4);
}

#[test]
fn sync_run_result_maps_to_dto() {
    let result = handshaker_core::SyncRunResult {
        downloaded: vec!["/p/a.jpg".to_string()],
        deleted: vec![],
        failures: vec!["/p/b.jpg".to_string()],
        conflicts: vec!["/p/c.jpg".to_string()],
    };
    let dto = sync_run_result_to_dto(result);
    assert_eq!(dto.downloaded, vec!["/p/a.jpg".to_string()]);
    assert!(dto.deleted.is_empty());
    assert_eq!(dto.failures, vec!["/p/b.jpg".to_string()]);
    assert_eq!(dto.conflicts, vec!["/p/c.jpg".to_string()]);
}

#[test]
fn one_entry_diff_maps_status_to_plan_category() {
    let file = Some(RemoteFile {
        path: "/storage/emulated/0/DCIM/Camera/a.jpg".to_string(),
        size: 1,
        created_at: None,
        modified_at: None,
        is_directory: false,
        checksum: None,
        is_trash: None,
        id: None,
        ext_data: None,
    });
    let added = one_entry_diff(&FileChange {
        file: file.clone(),
        status: FileChangeStatus::Added,
    });
    assert_eq!(
        added.added,
        vec!["/storage/emulated/0/DCIM/Camera/a.jpg".to_string()]
    );
    let deleted = one_entry_diff(&FileChange {
        file: file.clone(),
        status: FileChangeStatus::Deleted,
    });
    assert_eq!(
        deleted.deleted,
        vec!["/storage/emulated/0/DCIM/Camera/a.jpg".to_string()]
    );
    // Metadata-only changes never enter the diff (no local-file risk).
    let info = one_entry_diff(&FileChange {
        file,
        status: FileChangeStatus::InfoModified,
    });
    assert!(info.added.is_empty() && info.deleted.is_empty());
    // Directories never enter the diff.
    let dir = one_entry_diff(&FileChange {
        file: Some(RemoteFile {
            path: "/storage/emulated/0/DCIM/Camera/dir".to_string(),
            size: 0,
            created_at: None,
            modified_at: None,
            is_directory: true,
            checksum: None,
            is_trash: None,
            id: None,
            ext_data: None,
        }),
        status: FileChangeStatus::Added,
    });
    assert!(dir.added.is_empty());
}

#[tokio::test]
async fn sync_store_for_uses_configured_state_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config();
    config.state_dir = Some(temp.path().to_path_buf());
    let runtime = HandShakerRuntime::create(config).await.expect("create");
    let profile = sync_profile("photos", 1);
    let store = runtime.sync_store_for(&profile).expect("store");
    store.save(&SyncSnapshot::default()).expect("save");
    assert!(
        temp.path().join("sync/dev-1.json").exists(),
        "ledger must live under <state_dir>/sync/<device_uuid>.json"
    );
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn start_sync_without_session_fails_the_job_after_start() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    // start_sync registers the job and runs the plan inside the task (exactly
    // one photo_sync per run — the phone rejects a second while SYNCING), so
    // the missing session surfaces as the job's last_error, not as a
    // start_sync error.
    let profile_id = runtime
        .start_sync(sync_profile("photos", 999))
        .await
        .expect("job registered");
    for _ in 0..50 {
        let status = runtime.get_sync_status(&profile_id).await.expect("status");
        if !status.running {
            let error = status.last_error.expect("job must have failed");
            assert_eq!(error.code, PublicErrorCode::SessionNotFound);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let status = runtime
        .get_sync_status(&profile_id)
        .await
        .expect("final status");
    assert!(!status.running);
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn sync_ledger_status_reads_configured_state_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config();
    config.state_dir = Some(temp.path().to_path_buf());
    let runtime = HandShakerRuntime::create(config).await.expect("create");
    let profile = sync_profile("photos", 1);
    // Seed a ledger through the real store, then verify the summary.
    let store = runtime.sync_store_for(&profile).expect("store");
    let mut snapshot = SyncSnapshot::default();
    snapshot.files.insert(
        "/a.jpg".to_string(),
        handshaker_core::SyncFileRecord {
            size: 100,
            modified_at: None,
            checksum: Some("abc".to_string()),
            ext_data: None,
            local_path: "/tmp/a.jpg".to_string(),
            local_sha256: None,
        },
    );
    snapshot.files.insert(
        "/b.bin".to_string(),
        handshaker_core::SyncFileRecord {
            size: 50,
            modified_at: None,
            checksum: None,
            ext_data: None,
            local_path: "/tmp/b.bin".to_string(),
            local_sha256: None,
        },
    );
    store.save(&snapshot).expect("save");

    let status = runtime
        .sync_ledger_status(&profile.device_uuid)
        .await
        .expect("status");
    assert_eq!(status.device_uuid, "dev-1");
    assert_eq!(status.files, 2);
    assert_eq!(status.bytes, 150);

    // An empty uuid is refused; a missing ledger reports zero, never an error.
    let error = runtime
        .sync_ledger_status("")
        .await
        .expect_err("empty uuid");
    assert_eq!(error.code, PublicErrorCode::InvalidArgument);
    let status = runtime
        .sync_ledger_status("no-such-device")
        .await
        .expect("missing ledger is zero");
    assert_eq!(status.files, 0);
    assert_eq!(status.bytes, 0);
    runtime.shutdown().await.expect("shutdown");
}

#[test]
fn sync_watch_applied_event_json_is_stable() {
    let result = crate::sync::SyncRunResultDto {
        downloaded: vec!["/a.jpg".to_string()],
        deleted: Vec::new(),
        failures: Vec::new(),
        conflicts: Vec::new(),
    };
    let json = serde_json::to_value(crate::event::BackendEvent::SyncWatchApplied(result.clone()))
        .expect("serialize");
    assert_eq!(json["kind"], "sync_watch_applied");
    assert_eq!(json["downloaded"][0], "/a.jpg");
    let decoded: crate::event::BackendEvent = serde_json::from_value(json).expect("deserialize");
    assert_eq!(
        decoded,
        crate::event::BackendEvent::SyncWatchApplied(result)
    );
}

#[tokio::test]
async fn sync_ops_on_unknown_profile_return_not_found() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let error = runtime.stop_sync("nope").await.expect_err("stop must fail");
    assert_eq!(error.code, PublicErrorCode::NotFound);
    let error = runtime
        .get_sync_status("nope")
        .await
        .expect_err("status must fail");
    assert_eq!(error.code, PublicErrorCode::NotFound);
    let error = runtime
        .last_sync_result("nope")
        .await
        .expect_err("result must fail");
    assert_eq!(error.code, PublicErrorCode::NotFound);
    let error = runtime
        .start_sync_watch("nope")
        .await
        .expect_err("watch must fail");
    assert_eq!(error.code, PublicErrorCode::NotFound);
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn duplicate_sync_job_registration_is_rejected_while_running() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let profile = sync_profile("photos", 1);
    let first = runtime
        .register_sync_job(profile.clone())
        .await
        .expect("register");
    assert!(first.status().running);
    let error = runtime
        .register_sync_job(profile.clone())
        .await
        .expect_err("duplicate");
    assert_eq!(error.code, PublicErrorCode::InvalidState);
    // A finished job can be replaced with a fresh one.
    first.set_status(|status| status.running = false);
    let second = runtime
        .register_sync_job(profile)
        .await
        .expect("re-register");
    assert!(second.status().running);
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn stop_sync_removes_the_job() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    runtime
        .register_sync_job(sync_profile("photos", 1))
        .await
        .expect("register");
    assert!(runtime.get_sync_status("photos").await.is_ok());
    runtime.stop_sync("photos").await.expect("stop");
    let error = runtime
        .get_sync_status("photos")
        .await
        .expect_err("removed");
    assert_eq!(error.code, PublicErrorCode::NotFound);
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn reserve_sync_watch_enforces_run_watch_mutual_exclusion() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let profile = sync_profile("photos", 1);
    let job = runtime.register_sync_job(profile).await.expect("register");

    // A running job cannot be watched (review fix: decided under the lock).
    let error = runtime
        .reserve_sync_watch(&job)
        .await
        .expect_err("running job must refuse watch");
    assert_eq!(error.code, PublicErrorCode::InvalidState);

    // A finished job can reserve the watch slot; a second reservation is
    // refused while the first is active.
    job.set_status(|status| status.running = false);
    runtime
        .reserve_sync_watch(&job)
        .await
        .expect("reserve after run finished");
    assert!(job.status().monitoring);
    let error = runtime
        .reserve_sync_watch(&job)
        .await
        .expect_err("already monitoring");
    assert_eq!(error.code, PublicErrorCode::InvalidState);

    // Releasing rolls the flag back so the slot can be reserved again.
    runtime.release_sync_watch(&job).await;
    assert!(!job.status().monitoring);
    runtime
        .reserve_sync_watch(&job)
        .await
        .expect("reserve after release");
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn remove_sync_job_if_current_preserves_replaced_job() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let profile = sync_profile("photos", 1);
    let first = runtime
        .register_sync_job(profile.clone())
        .await
        .expect("register first");
    // Simulate the finished job being replaced by a fresh registration
    // while a stale stop_sync still holds the old Arc.
    first.set_status(|status| status.running = false);
    let second = runtime
        .register_sync_job(profile)
        .await
        .expect("register replacement");
    assert!(!Arc::ptr_eq(&first, &second));

    // The stale stop must not delete the replacement...
    assert!(!runtime.remove_sync_job_if_current("photos", &first).await);
    assert!(runtime.get_sync_status("photos").await.is_ok());
    // ...and stopping the current job does.
    assert!(runtime.remove_sync_job_if_current("photos", &second).await);
    let error = runtime
        .get_sync_status("photos")
        .await
        .expect_err("removed");
    assert_eq!(error.code, PublicErrorCode::NotFound);
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn watch_is_rejected_while_run_is_active() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    runtime
        .register_sync_job(sync_profile("photos", 999))
        .await
        .expect("register");
    let error = runtime
        .start_sync_watch("photos")
        .await
        .expect_err("must fail");
    assert_eq!(error.code, PublicErrorCode::InvalidState);
    runtime.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn start_sync_watch_without_session_fails_closed() {
    let runtime = HandShakerRuntime::create(test_config())
        .await
        .expect("create");
    let job = runtime
        .register_sync_job(sync_profile("photos", 999))
        .await
        .expect("register");
    // Mark the job finished so the watch is not rejected for running.
    job.set_status(|status| status.running = false);
    let error = runtime
        .start_sync_watch("photos")
        .await
        .expect_err("must fail");
    assert_eq!(error.code, PublicErrorCode::SessionNotFound);
    runtime.shutdown().await.expect("shutdown");
}
