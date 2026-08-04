//! FFI: trust service & device discovery (Phase E / E3) — trust records,
//! wifi trust reset, and multi-transport discovery.
//!
//! Every exported function follows the crate-wide contract: panic isolation
//! via `catch`, NULL-safe handles, and stable `InvalidArgument` errors for
//! bad input. Complex arguments/results are UTF-8 JSON buffers.
//!
//! None of these functions take a `session_id`: trust records and discovery
//! are runtime-scoped, not session-scoped.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;

use handshaker_application::{
    DeviceId, ListDevicesRequest, PublicError, PublicErrorCode, RemoveTrustRequest,
    ResetWifiTrustRequest,
};
use serde::Deserialize;

use crate::ffi_try;
use crate::result::{HsCallResult, catch, err, input_str, ok};
use crate::runtime_ref;

/// Normalize a caller-supplied device id to the canonical `phone:<uuid>`
/// shape: ids already carrying the `phone:` prefix pass through, everything
/// else gets the prefix prepended.
fn normalize_device_id(raw: String) -> DeviceId {
    if raw.starts_with("phone:") {
        DeviceId(raw)
    } else {
        DeviceId(format!("phone:{raw}"))
    }
}

/// List locally persisted WiFi trust records. No request buffer.
/// Result JSON: an array of `TrustRecordDto`
/// (`[{"device_id":"phone:<uuid>","device_name":...,"updated_at_ms":N}, ...]`).
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_trust_list(runtime: *mut c_void) -> HsCallResult {
    catch("trust_list", || {
        let runtime = ffi_try!(runtime_ref(runtime, "trust_list"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.list_trust_records().await })
        {
            Ok(records) => ok(&records),
            Err(error) => err(&error),
        }
    })
}

/// `hs_trust_remove` request JSON: `{"device_id":"phone:<uuid>"}` (required;
/// a bare uuid without the `phone:` prefix is accepted and normalized).
/// Result JSON: `{"removed":true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_trust_remove(
    runtime: *mut c_void,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("trust_remove", || {
        let runtime = ffi_try!(runtime_ref(runtime, "trust_remove"));
        let json = ffi_try!(input_str(request_ptr, request_len, "trust_remove"));
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FfiTrustRemoveRequest {
            device_id: String,
        }
        let ffi: FfiTrustRemoveRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let request = RemoveTrustRequest {
            device_id: normalize_device_id(ffi.device_id),
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.remove_trust_record(request).await })
        {
            Ok(result) => ok(&result),
            Err(error) => err(&error),
        }
    })
}

/// `hs_trust_reset` request JSON:
/// `{"endpoint":"192.168.1.5:5555","expected_device_id":"phone:<uuid>"}`
/// (both required; `expected_device_id` accepts a bare uuid and normalizes
/// it). Clears the phone-side WiFi trust for the device and the local
/// record. Result JSON: `{"reset":true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_trust_reset(
    runtime: *mut c_void,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("trust_reset", || {
        let runtime = ffi_try!(runtime_ref(runtime, "trust_reset"));
        let json = ffi_try!(input_str(request_ptr, request_len, "trust_reset"));
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FfiTrustResetRequest {
            endpoint: String,
            expected_device_id: String,
        }
        let ffi: FfiTrustResetRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let request = ResetWifiTrustRequest {
            endpoint: ffi.endpoint,
            expected_device_id: normalize_device_id(ffi.expected_device_id),
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.reset_wifi_trust(request).await })
        {
            Ok(()) => ok(&serde_json::json!({ "reset": true })),
            Err(error) => err(&error),
        }
    })
}

/// Run a multi-transport device discovery sweep. No request buffer
/// (all transports are enabled with the default 3s wifi browse timeout).
/// Result JSON: a `DeviceDiscoveryResult`
/// (`{"devices":[<DeviceDescriptor>, ...],"warnings":[...]}`); a broken
/// transport surfaces as a structured warning, never as a call failure.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_discover_devices(runtime: *mut c_void) -> HsCallResult {
    catch("discover_devices", || {
        let runtime = ffi_try!(runtime_ref(runtime, "discover_devices"));
        match runtime._tokio.block_on(async {
            runtime
                .app
                .discover_devices(ListDevicesRequest::default())
                .await
        }) {
            Ok(result) => ok(&result),
            Err(error) => err(&error),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ffi_test_util::{error_code_of, runtime_ptr};
    use crate::hs_runtime_destroy;
    use crate::result::free_result;

    /// A runtime rooted at a temp state dir, so trust-store operations do
    /// not depend on the host's config directory.
    fn runtime_ptr_with_state_dir() -> *mut c_void {
        let state_dir =
            std::env::temp_dir().join(format!("hs-ffi-trust-test-{}", std::process::id()));
        let cfg = format!(r#"{{"state_dir_utf8":"{}"}}"#, state_dir.display());
        let mut out: *mut c_void = std::ptr::null_mut();
        let result = unsafe { crate::hs_runtime_create(cfg.as_ptr(), cfg.len(), &mut out) };
        assert_eq!(result.status, 0, "runtime create must succeed");
        assert!(!out.is_null());
        unsafe { free_result(result) };
        out
    }

    #[test]
    fn trust_list_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_trust_list(std::ptr::null_mut()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn trust_list_returns_ok_with_empty_array() {
        let runtime = runtime_ptr_with_state_dir();
        let result = unsafe { hs_trust_list(runtime) };
        assert_eq!(result.status, 0, "trust list must succeed without devices");
        let bytes = unsafe { crate::buffer::into_vec(result.value) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(decoded, serde_json::json!([]));
        unsafe { free_result(HsCallResult::default()) };
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn trust_remove_null_handle_returns_invalid_argument() {
        let request = br#"{"device_id":"phone:abc"}"#;
        let result =
            unsafe { hs_trust_remove(std::ptr::null_mut(), request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn trust_remove_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_trust_remove(runtime, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn trust_reset_null_handle_returns_invalid_argument() {
        let request = br#"{"endpoint":"1.2.3.4:5555","expected_device_id":"phone:abc"}"#;
        let result =
            unsafe { hs_trust_reset(std::ptr::null_mut(), request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn trust_reset_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_trust_reset(runtime, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn discover_devices_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_discover_devices(std::ptr::null_mut()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn discover_devices_returns_ok_without_devices() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_discover_devices(runtime) };
        assert_eq!(result.status, 0, "discovery must succeed without devices");
        let bytes = unsafe { crate::buffer::into_vec(result.value) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert!(decoded["devices"].is_array());
        assert!(decoded["warnings"].is_array());
        unsafe { free_result(HsCallResult::default()) };
        unsafe { hs_runtime_destroy(runtime) };
    }
}
