//! FFI: clipboard service (Phase E / E2) — list, set, delete, clear.
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

/// List the clipboard history of an open session. No request buffer.
/// Result JSON: an array of `ClipboardEntryDto`
/// (`[{"text":"...","timestamp_ms":N}, ...]`).
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_clipboard_list(runtime: *mut c_void, session_id: u64) -> HsCallResult {
    catch("clipboard_list", || {
        let runtime = ffi_try!(runtime_ref(runtime, "clipboard_list"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.list_clipboards(SessionId(session_id)).await })
        {
            Ok(entries) => ok(&entries),
            Err(error) => err(&error),
        }
    })
}

/// `hs_clipboard_set` request JSON: `{"text":"copied content"}` (required).
/// Result JSON: `{"set":true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_clipboard_set(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("clipboard_set", || {
        let runtime = ffi_try!(runtime_ref(runtime, "clipboard_set"));
        let json = ffi_try!(input_str(request_ptr, request_len, "clipboard_set"));
        #[derive(Deserialize)]
        struct FfiClipboardSetRequest {
            text: String,
        }
        let ffi: FfiClipboardSetRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        match runtime._tokio.block_on(async {
            runtime
                .app
                .set_clipboard(SessionId(session_id), &ffi.text)
                .await
        }) {
            Ok(()) => ok(&serde_json::json!({ "set": true })),
            Err(error) => err(&error),
        }
    })
}

/// `hs_clipboard_delete` request JSON: `{"timestamp_ms":123}` (required,
/// signed 64-bit). Result JSON: `{"deleted":true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_clipboard_delete(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("clipboard_delete", || {
        let runtime = ffi_try!(runtime_ref(runtime, "clipboard_delete"));
        let json = ffi_try!(input_str(request_ptr, request_len, "clipboard_delete"));
        #[derive(Deserialize)]
        struct FfiClipboardDeleteRequest {
            timestamp_ms: i64,
        }
        let ffi: FfiClipboardDeleteRequest =
            ffi_try!(serde_json::from_str(json).map_err(|error| {
                err(
                    &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                        .with_detail(error.to_string()),
                )
            }));
        match runtime._tokio.block_on(async {
            runtime
                .app
                .delete_clipboard(SessionId(session_id), ffi.timestamp_ms)
                .await
        }) {
            Ok(()) => ok(&serde_json::json!({ "deleted": true })),
            Err(error) => err(&error),
        }
    })
}

/// Clear the whole clipboard history of an open session. No request buffer.
/// Result JSON: `{"cleared":true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_clipboard_clear(runtime: *mut c_void, session_id: u64) -> HsCallResult {
    catch("clipboard_clear", || {
        let runtime = ffi_try!(runtime_ref(runtime, "clipboard_clear"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.clear_clipboards(SessionId(session_id)).await })
        {
            Ok(()) => ok(&serde_json::json!({ "cleared": true })),
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
    fn clipboard_list_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_clipboard_list(std::ptr::null_mut(), 1) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn clipboard_list_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_clipboard_list(runtime, 999) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn clipboard_set_null_handle_returns_invalid_argument() {
        let result =
            unsafe { hs_clipboard_set(std::ptr::null_mut(), 1, br#"{"text":"hi"}"#.as_ptr(), 13) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn clipboard_set_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_clipboard_set(runtime, 1, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn clipboard_set_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_clipboard_set(runtime, 999, br#"{"text":"hi"}"#.as_ptr(), 13) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn clipboard_delete_null_handle_returns_invalid_argument() {
        let request = br#"{"timestamp_ms":123}"#;
        let result = unsafe {
            hs_clipboard_delete(std::ptr::null_mut(), 1, request.as_ptr(), request.len())
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn clipboard_delete_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_clipboard_delete(runtime, 1, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn clipboard_delete_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let request = br#"{"timestamp_ms":123}"#;
        let result = unsafe { hs_clipboard_delete(runtime, 999, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn clipboard_clear_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_clipboard_clear(std::ptr::null_mut(), 1) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn clipboard_clear_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_clipboard_clear(runtime, 999) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }
}
