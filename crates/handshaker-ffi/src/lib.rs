//! handshaker-ffi: stable C ABI for `handshaker-application`.
//!
//! Design (M8 §7):
//! - opaque handles (`HsRuntime*`, `HsSubscription*`); Session/Transfer use
//!   plain `uint64_t` ids;
//! - complex arguments/results are UTF-8 JSON buffers (`HsCallResult`);
//! - every extern function catches panics; nothing unwinds across the ABI;
//! - Rust-allocated memory is freed only by Rust (`hs_byte_buffer_free`,
//!   `hs_call_result_free`);
//! - short operations block the calling thread on the runtime's tokio
//!   executor; callers must call from a background thread (never the Swift
//!   main thread). Long tasks use ids + event polling.
//!
//! ABI version: independent of the Rust crate version.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

mod buffer;
mod result;

use std::ffi::c_void;
use std::path::PathBuf;
use std::time::Duration;

use handshaker_application::{
    ConnectRequest, DeviceDescriptor, DownloadRequest, HandShakerRuntime, ListDevicesRequest,
    ListFilesRequest, PublicError, PublicErrorCode, RuntimeConfig, SessionId, TransferId,
    UploadRequest,
};
use serde::Deserialize;
use tokio::sync::broadcast;

use buffer::HsByteBuffer;
use result::{HsCallResult, catch, err, free_result, input_str, ok, out_slot};

/// ABI version (M8 §7.3), independent of Cargo versions.
pub const ABI_VERSION_MAJOR: u32 = 1;
pub const ABI_VERSION_MINOR: u32 = 1;
pub const ABI_VERSION_PATCH: u32 = 0;

/// Runtime handle: owns a tokio executor and the application runtime.
pub struct HsRuntime {
    _tokio: tokio::runtime::Runtime,
    app: HandShakerRuntime,
}

/// Event subscription handle: a broadcast receiver owned by the caller.
/// The receiver is mutex-guarded so concurrent `hs_subscription_next` calls
/// on one handle are serialized instead of racing.
pub struct HsSubscription {
    _tokio: tokio::runtime::Runtime,
    receiver: tokio::sync::Mutex<broadcast::Receiver<handshaker_application::EventEnvelope>>,
}

// ---------------------------------------------------------------------------
// ABI version
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn hs_abi_version_major() -> u32 {
    ABI_VERSION_MAJOR
}

#[unsafe(no_mangle)]
pub extern "C" fn hs_abi_version_minor() -> u32 {
    ABI_VERSION_MINOR
}

#[unsafe(no_mangle)]
pub extern "C" fn hs_abi_version_patch() -> u32 {
    ABI_VERSION_PATCH
}

// ---------------------------------------------------------------------------
// Buffers & results
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn hs_byte_buffer_free(buffer: HsByteBuffer) {
    // Safety: documented contract — buffer must come from this library.
    // Panics (e.g. allocator double-free detection) must never unwind across
    // the ABI, so catch and swallow them.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        buffer::free_buffer(buffer)
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn hs_call_result_free(call_result: HsCallResult) {
    // Safety: documented contract — result must come from this library.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        free_result(call_result)
    }));
}

// ---------------------------------------------------------------------------
// Runtime lifecycle
// ---------------------------------------------------------------------------

/// `hs_runtime_create` config JSON (M8 §7.7). Fields are optional; defaults
/// match `RuntimeConfig::default()`.
#[derive(Debug, Default, Deserialize)]
struct FfiRuntimeConfig {
    adb_path_utf8: Option<String>,
    default_timeout_ms: Option<u64>,
    heartbeat_interval_ms: Option<u64>,
    state_dir_utf8: Option<String>,
    #[allow(dead_code)]
    wire_log_utf8: Option<String>,
    event_capacity: Option<u32>,
}

fn config_from_json(json: &str) -> Result<RuntimeConfig, HsCallResult> {
    let ffi: FfiRuntimeConfig = serde_json::from_str(json).map_err(|error| {
        err(
            &PublicError::new(PublicErrorCode::InvalidArgument, "invalid config JSON")
                .with_detail(error.to_string()),
        )
    })?;
    Ok(RuntimeConfig {
        adb_path: ffi
            .adb_path_utf8
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("adb")),
        default_timeout: Duration::from_millis(ffi.default_timeout_ms.unwrap_or(30_000)),
        heartbeat_interval: Duration::from_millis(ffi.heartbeat_interval_ms.unwrap_or(10_000)),
        state_dir: ffi.state_dir_utf8.map(PathBuf::from),
        wire_log: None,
        event_capacity: ffi
            .event_capacity
            .map(|value| value as usize)
            .unwrap_or(1024),
    })
}

/// Create a runtime. `out_runtime` is written only on success.
///
/// # Safety
/// `config_ptr`/`config_len` must describe valid memory; `out_runtime` must be
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_runtime_create(
    config_ptr: *const u8,
    config_len: usize,
    out_runtime: *mut *mut c_void,
) -> HsCallResult {
    catch("runtime_create", || {
        let slot = ffi_try!(out_slot(out_runtime, "runtime_create"));
        let json = ffi_try!(input_str(config_ptr, config_len, "runtime_create"));
        let config = ffi_try!(config_from_json(json));
        let tokio_runtime = ffi_try!(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    err(&PublicError::new(
                        PublicErrorCode::Internal,
                        "tokio runtime creation failed",
                    )
                    .with_detail(error.to_string()))
                })
        );
        let app = tokio_runtime.block_on(async { HandShakerRuntime::create(config).await });
        let app = match app {
            Ok(app) => app,
            Err(error) => return err(&error),
        };
        let runtime = Box::new(HsRuntime {
            _tokio: tokio_runtime,
            app,
        });
        *slot = Box::into_raw(runtime) as *mut c_void;
        ok(&serde_json::json!({ "created": true }))
    })
}

/// Shutdown a runtime (idempotent). Blocks on the runtime's executor.
///
/// # Safety
/// `runtime` must be a valid handle or NULL (NULL is a no-op success).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_runtime_shutdown(runtime: *mut c_void) -> HsCallResult {
    catch("runtime_shutdown", || {
        if runtime.is_null() {
            return ok(&serde_json::json!({ "shutdown": true }));
        }
        let runtime = &*(runtime as *const HsRuntime);
        match runtime
            ._tokio
            .block_on(async { runtime.app.shutdown().await })
        {
            Ok(()) => ok(&serde_json::json!({ "shutdown": true })),
            Err(error) => err(&error),
        }
    })
}

/// Destroy a runtime handle (conservative cleanup; safe on NULL).
///
/// # Safety
/// `runtime` must be a valid handle (or NULL); after this call the handle is
/// invalid and must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_runtime_destroy(runtime: *mut c_void) {
    if runtime.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let runtime = Box::from_raw(runtime as *mut HsRuntime);
        let _ = runtime
            ._tokio
            .block_on(async { runtime.app.shutdown().await });
        drop(runtime);
    }));
}

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// Borrow a runtime handle; NULL returns a stable InvalidArgument error.
fn runtime_ref<'a>(runtime: *mut c_void, operation: &str) -> Result<&'a HsRuntime, HsCallResult> {
    if runtime.is_null() {
        return Err(err(&PublicError::new(
            PublicErrorCode::InvalidArgument,
            "NULL runtime handle",
        )
        .operation(operation)));
    }
    Ok(unsafe { &*(runtime as *const HsRuntime) })
}

// ---------------------------------------------------------------------------
// Devices, sessions, files
// ---------------------------------------------------------------------------

/// `hs_list_devices` request JSON: `{"include_adb":true,"include_wifi":true,
/// "include_usb":true,"wifi_browse_timeout_ms":3000}` (all optional).
#[derive(Debug, Default, Deserialize)]
struct FfiListDevicesRequest {
    include_adb: Option<bool>,
    include_wifi: Option<bool>,
    include_usb: Option<bool>,
    wifi_browse_timeout_ms: Option<u64>,
}

fn list_devices_request_from_json(json: &str) -> Result<ListDevicesRequest, HsCallResult> {
    let ffi: FfiListDevicesRequest = serde_json::from_str(json).map_err(|error| {
        err(
            &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                .with_detail(error.to_string()),
        )
    })?;
    Ok(ListDevicesRequest {
        include_adb: ffi.include_adb.unwrap_or(true),
        include_wifi: ffi.include_wifi.unwrap_or(true),
        include_usb: ffi.include_usb.unwrap_or(true),
        wifi_browse_timeout: Duration::from_millis(ffi.wifi_browse_timeout_ms.unwrap_or(3000)),
    })
}

/// List devices. Result JSON: an array of `DeviceDescriptor`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_list_devices(
    runtime: *mut c_void,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("list_devices", || {
        let runtime = ffi_try!(runtime_ref(runtime, "list_devices"));
        let json = ffi_try!(input_str(request_ptr, request_len, "list_devices"));
        let request = ffi_try!(list_devices_request_from_json(json));
        match runtime
            ._tokio
            .block_on(async { runtime.app.list_devices(request).await })
        {
            Ok(devices) => ok(&devices),
            Err(error) => err(&error),
        }
    })
}

/// `hs_connect` request JSON: a full `DeviceDescriptor` (as returned by
/// `hs_list_devices`). Result JSON: `{"session_id": 1}`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_connect(
    runtime: *mut c_void,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("connect", || {
        let runtime = ffi_try!(runtime_ref(runtime, "connect"));
        let json = ffi_try!(input_str(request_ptr, request_len, "connect"));
        let device: DeviceDescriptor = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid device JSON")
                    .with_detail(error.to_string()),
            )
        }));
        match runtime
            ._tokio
            .block_on(async { runtime.app.connect(ConnectRequest { device }).await })
        {
            Ok(session_id) => ok(&serde_json::json!({ "session_id": session_id.0 })),
            Err(error) => err(&error),
        }
    })
}

/// Disconnect a session. Result JSON: `{"disconnected": true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_disconnect(runtime: *mut c_void, session_id: u64) -> HsCallResult {
    catch("disconnect", || {
        let runtime = ffi_try!(runtime_ref(runtime, "disconnect"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.disconnect(SessionId(session_id)).await })
        {
            Ok(()) => ok(&serde_json::json!({ "disconnected": true })),
            Err(error) => err(&error),
        }
    })
}

/// Get a session snapshot. Result JSON: `SessionSnapshot`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_get_session(runtime: *mut c_void, session_id: u64) -> HsCallResult {
    catch("get_session", || {
        let runtime = ffi_try!(runtime_ref(runtime, "get_session"));
        match runtime._tokio.block_on(async {
            runtime
                .app
                .get_session_snapshot(SessionId(session_id))
                .await
        }) {
            Ok(snapshot) => ok(&snapshot),
            Err(error) => err(&error),
        }
    })
}

/// `hs_list_files` request JSON: `{"path":"/storage/emulated/0","depth":1}`
/// (both optional; path defaults to the device root, depth to 1). Result:
/// array of `FileEntryDto`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_list_files(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("list_files", || {
        let runtime = ffi_try!(runtime_ref(runtime, "list_files"));
        let json = ffi_try!(input_str(request_ptr, request_len, "list_files"));
        #[derive(Deserialize)]
        struct FfiListFilesRequest {
            path: Option<String>,
            depth: Option<u32>,
        }
        let ffi: FfiListFilesRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let request = ListFilesRequest {
            session_id: SessionId(session_id),
            path: ffi.path.unwrap_or_else(|| ".".to_string()),
            depth: ffi.depth.unwrap_or(1),
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.list_files(request).await })
        {
            Ok(files) => ok(&files),
            Err(error) => err(&error),
        }
    })
}

// ---------------------------------------------------------------------------
// Transfers (M8 Phase 6 suggested surface, ABI 1.1)
// ---------------------------------------------------------------------------

/// `hs_transfer_start_download` request JSON:
/// `{"remote_path":"/sdcard/a.bin","local_path":"/tmp/a.bin","overwrite":false}`
/// (overwrite optional, default false). Result JSON: `{"transfer_id": N}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_transfer_start_download(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("transfer_start_download", || {
        let runtime = ffi_try!(runtime_ref(runtime, "transfer_start_download"));
        let json = ffi_try!(input_str(
            request_ptr,
            request_len,
            "transfer_start_download"
        ));
        let (remote_path, local_path, overwrite) = ffi_try!(ffi_transfer_paths(json));
        let request = DownloadRequest {
            session_id: SessionId(session_id),
            remote_path,
            local_path: PathBuf::from(local_path),
            overwrite,
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.start_download(request).await })
        {
            Ok(id) => ok(&serde_json::json!({ "transfer_id": id.0 })),
            Err(error) => err(&error),
        }
    })
}

/// `hs_transfer_start_upload` request JSON: same shape as
/// `hs_transfer_start_download` (`remote_path` is the destination).
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_transfer_start_upload(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("transfer_start_upload", || {
        let runtime = ffi_try!(runtime_ref(runtime, "transfer_start_upload"));
        let json = ffi_try!(input_str(request_ptr, request_len, "transfer_start_upload"));
        let (remote_path, local_path, overwrite) = ffi_try!(ffi_transfer_paths(json));
        let request = UploadRequest {
            session_id: SessionId(session_id),
            local_path: PathBuf::from(local_path),
            remote_path,
            overwrite,
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.start_upload(request).await })
        {
            Ok(id) => ok(&serde_json::json!({ "transfer_id": id.0 })),
            Err(error) => err(&error),
        }
    })
}

/// Cancel a transfer. Result JSON: `{"cancelled": true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_transfer_cancel(
    runtime: *mut c_void,
    transfer_id: u64,
) -> HsCallResult {
    catch("transfer_cancel", || {
        let runtime = ffi_try!(runtime_ref(runtime, "transfer_cancel"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.cancel_transfer(TransferId(transfer_id)).await })
        {
            Ok(()) => ok(&serde_json::json!({ "cancelled": true })),
            Err(error) => err(&error),
        }
    })
}

/// Get one transfer snapshot. Result JSON: `TransferSnapshot`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_transfer_get(runtime: *mut c_void, transfer_id: u64) -> HsCallResult {
    catch("transfer_get", || {
        let runtime = ffi_try!(runtime_ref(runtime, "transfer_get"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.get_transfer(TransferId(transfer_id)).await })
        {
            Ok(snapshot) => ok(&snapshot),
            Err(error) => err(&error),
        }
    })
}

/// List transfer snapshots (finished entries reaped). Result JSON: an array
/// of `TransferSnapshot`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_transfer_list(runtime: *mut c_void) -> HsCallResult {
    catch("transfer_list", || {
        let runtime = ffi_try!(runtime_ref(runtime, "transfer_list"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.list_transfers().await })
        {
            Ok(snapshots) => ok(&snapshots),
            Err(error) => err(&error),
        }
    })
}

/// Shared parser for the transfer request JSON.
fn ffi_transfer_paths(json: &str) -> std::result::Result<(String, String, bool), HsCallResult> {
    #[derive(Deserialize)]
    struct FfiTransferPaths {
        remote_path: String,
        local_path: String,
        overwrite: Option<bool>,
    }
    let ffi: FfiTransferPaths = serde_json::from_str(json).map_err(|error| {
        err(
            &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                .with_detail(error.to_string()),
        )
    })?;
    Ok((
        ffi.remote_path,
        ffi.local_path,
        ffi.overwrite.unwrap_or(false),
    ))
}

// ---------------------------------------------------------------------------
// Event subscription (queue-pull model, M8 §7.10)
// ---------------------------------------------------------------------------

/// Subscribe to backend events. `out_subscription` is written on success.
///
/// # Safety
/// `runtime` must be a valid handle; `out_subscription` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_subscribe_events(
    runtime: *mut c_void,
    out_subscription: *mut *mut c_void,
) -> HsCallResult {
    catch("subscribe_events", || {
        let runtime = ffi_try!(runtime_ref(runtime, "subscribe_events"));
        let slot = ffi_try!(out_slot(out_subscription, "subscribe_events"));
        let receiver = runtime.app.subscribe_events();
        let subscription = Box::new(HsSubscription {
            _tokio: ffi_try!(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| {
                        err(&PublicError::new(
                            PublicErrorCode::Internal,
                            "tokio runtime creation failed",
                        )
                        .with_detail(error.to_string()))
                    })
            ),
            receiver: tokio::sync::Mutex::new(receiver),
        });
        *slot = Box::into_raw(subscription) as *mut c_void;
        ok(&serde_json::json!({ "subscribed": true }))
    })
}

/// Wait up to `timeout_ms` for the next event. Result JSON: an
/// `EventEnvelope`. On timeout, returns `status == 0` with value JSON
/// `{"timeout":true}`. After runtime shutdown the subscription is closed:
/// `{"closed":true}`.
///
/// # Safety
/// `subscription` must be a valid handle or NULL (NULL returns a closed
/// result).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_subscription_next(
    subscription: *mut c_void,
    timeout_ms: u32,
) -> HsCallResult {
    catch("subscription_next", || {
        if subscription.is_null() {
            return ok(&serde_json::json!({ "closed": true }));
        }
        let subscription = &*(subscription as *const HsSubscription);
        let timeout = Duration::from_millis(timeout_ms as u64);
        let outcome = subscription._tokio.block_on(async {
            tokio::time::timeout(timeout, subscription.receiver.lock().await.recv()).await
        });
        match outcome {
            Ok(Ok(envelope)) => ok(&envelope),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                ok(&serde_json::json!({ "closed": true }))
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped))) => err(
                &PublicError::new(PublicErrorCode::InvalidState, "subscription lagged")
                    .with_detail(format!("skipped {skipped} events")),
            ),
            Err(_elapsed) => ok(&serde_json::json!({ "timeout": true })),
        }
    })
}

/// Destroy a subscription handle (safe on NULL).
///
/// # Safety
/// `subscription` must be a valid handle (or NULL); after this call the
/// handle must not be used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_subscription_destroy(subscription: *mut c_void) {
    if subscription.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(Box::from_raw(subscription as *mut HsSubscription));
    }));
}

#[cfg(test)]
mod ffi_smoke_tests {
    use super::*;

    fn runtime_ptr() -> *mut c_void {
        let mut out: *mut c_void = std::ptr::null_mut();
        let result = unsafe { hs_runtime_create(b"{}".as_ptr(), 2, &mut out) };
        assert_eq!(result.status, 0, "runtime create must succeed");
        assert!(!out.is_null());
        unsafe { free_result(result) };
        out
    }

    #[test]
    fn runtime_lifecycle_create_list_shutdown_destroy() {
        let runtime = runtime_ptr();
        // list_devices with everything disabled -> empty array, status 0.
        let request = br#"{"include_adb":false,"include_wifi":false,"include_usb":false}"#;
        let result = unsafe { hs_list_devices(runtime, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 0);
        let bytes = unsafe { crate::buffer::into_vec(result.value) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(decoded, serde_json::json!([]));
        unsafe { free_result(HsCallResult::default()) };

        let result = unsafe { hs_runtime_shutdown(runtime) };
        assert_eq!(result.status, 0);
        unsafe { free_result(result) };
        // Shutdown twice is fine (idempotent).
        let result = unsafe { hs_runtime_shutdown(runtime) };
        assert_eq!(result.status, 0);
        unsafe { free_result(result) };
        unsafe { hs_runtime_destroy(runtime) };
        unsafe { hs_runtime_destroy(std::ptr::null_mut()) };
    }

    #[test]
    fn null_handles_return_errors_not_crashes() {
        let result = unsafe { hs_list_devices(std::ptr::null_mut(), b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        let bytes = unsafe { crate::buffer::into_vec(result.error) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert!(decoded["code"].is_string());
        unsafe { free_result(HsCallResult::default()) };
    }

    #[test]
    fn invalid_json_config_is_rejected() {
        let mut out: *mut c_void = std::ptr::null_mut();
        let result = unsafe { hs_runtime_create(b"{not json".as_ptr(), 9, &mut out) };
        assert_eq!(result.status, 1);
        let bytes = unsafe { crate::buffer::into_vec(result.error) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(decoded["code"], "invalid_argument");
        assert!(out.is_null(), "out_runtime must not be written on failure");
        unsafe { free_result(HsCallResult::default()) };
    }

    #[test]
    fn abi_version_is_1_1_0() {
        assert_eq!(hs_abi_version_major(), 1);
        assert_eq!(hs_abi_version_minor(), 1);
        assert_eq!(hs_abi_version_patch(), 0);
    }

    #[test]
    fn transfer_null_handle_returns_invalid_argument() {
        let result = unsafe {
            hs_transfer_start_download(
                std::ptr::null_mut(),
                1,
                br#"{"remote_path":"/a","local_path":"/b"}"#.as_ptr(),
                41,
            )
        };
        assert_eq!(result.status, 1);
        let bytes = unsafe { crate::buffer::into_vec(result.error) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(decoded["code"], "invalid_argument");
        unsafe { free_result(HsCallResult::default()) };
    }

    #[test]
    fn transfer_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let request = br#"{"remote_path":"/a.bin","local_path":"/tmp/a.bin","overwrite":false}"#;
        let result =
            unsafe { hs_transfer_start_download(runtime, 999, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        let bytes = unsafe { crate::buffer::into_vec(result.error) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(decoded["code"], "session_not_found");
        unsafe { free_result(HsCallResult::default()) };
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn transfer_bad_request_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_transfer_start_upload(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        let bytes = unsafe { crate::buffer::into_vec(result.error) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(decoded["code"], "invalid_argument");
        unsafe { free_result(HsCallResult::default()) };
        unsafe { hs_runtime_destroy(runtime) };
    }
}
