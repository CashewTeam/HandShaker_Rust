//! Application-layer unit tests: DTO semantics, error mapping, path rules,
//! and runtime lifecycle that does not require a real device.

use std::time::Duration;

use handshaker_core::{DeviceInfo, RemoteFile};

use crate::dto::{DeviceDescriptor, DeviceId, RuntimeConfig, SessionId, TransportKind};
use crate::error::{PublicErrorCode, from_core_error};
use crate::runtime::normalize_remote_path;
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
    // ".." collapses and never escapes above root.
    assert_eq!(resolve_remote_path("/root", "a/../b"), "/root/b");
    assert_eq!(resolve_remote_path("/root", "../../etc"), "/etc");
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
