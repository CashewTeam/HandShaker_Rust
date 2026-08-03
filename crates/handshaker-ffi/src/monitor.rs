//! FFI: directory monitor (Phase E / E3) — register/unregister a phone-side
//! folder monitor; change events arrive over the runtime event hub as
//! `RemoteFileChanged`.
//!
//! Every exported function follows the crate-wide contract: panic isolation
//! via `catch`, NULL-safe handles, and stable `InvalidArgument` errors for
//! bad input. Complex arguments/results are UTF-8 JSON buffers.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;

use handshaker_application::{PublicError, PublicErrorCode, SessionId};
use serde::Deserialize;

use crate::ffi_try;
use crate::result::{HsCallResult, catch, err, input_str, ok};
use crate::runtime_ref;

/// `hs_monitor_folder` request JSON: `{"path":"/storage/emulated/0/DCIM",
/// "enabled":true}` (path required; enabled optional, defaults to `true`).
/// Result JSON: `{"registered":true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_monitor_folder(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("monitor_folder", || {
        let runtime = ffi_try!(runtime_ref(runtime, "monitor_folder"));
        let json = ffi_try!(input_str(request_ptr, request_len, "monitor_folder"));
        #[derive(Deserialize)]
        struct FfiMonitorFolderRequest {
            path: String,
            enabled: Option<bool>,
        }
        let ffi: FfiMonitorFolderRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let enabled = ffi.enabled.unwrap_or(true);
        match runtime._tokio.block_on(async {
            runtime
                .app
                .monitor_folder(SessionId(session_id), ffi.path, enabled)
                .await
        }) {
            Ok(()) => ok(&serde_json::json!({ "registered": true })),
            Err(error) => err(&error),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ffi_test_util::{error_code_of, runtime_ptr};
    use crate::hs_runtime_destroy;

    #[test]
    fn monitor_folder_null_handle_returns_invalid_argument() {
        let request = br#"{"path":"/DCIM"}"#;
        let result =
            unsafe { hs_monitor_folder(std::ptr::null_mut(), 1, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn monitor_folder_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_monitor_folder(runtime, 1, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn monitor_folder_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let request = br#"{"path":"/DCIM","enabled":true}"#;
        let result = unsafe { hs_monitor_folder(runtime, 999, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }
}
