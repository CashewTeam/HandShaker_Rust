//! FFI: batch transfers (Phase E / E4) — start batch download/upload.
//!
//! Both endpoints return a single `TransferId` immediately; progress,
//! per-item counts, cancellation and the final result are all observed
//! through the existing transfer surface (`hs_transfer_get`,
//! `hs_transfer_list`, `hs_transfer_cancel`), whose `TransferSnapshot`
//! carries the Phase E batch fields (`item_count`, `completed_items`,
//! `failed_items`, `current_item`, `batch_result`).
//!
//! Every exported function follows the crate-wide contract: panic isolation
//! via `catch`, NULL-safe handles, and stable `InvalidArgument` errors for
//! bad input. Complex arguments/results are UTF-8 JSON buffers.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;

use handshaker_application::{
    BatchTransferItemDto, BatchTransferRequest, PublicError, PublicErrorCode, SessionId,
    TreeTransferDto,
};
use serde::Deserialize;

use crate::ffi_try;
use crate::result::{HsCallResult, catch, err, input_str, ok};
use crate::runtime_ref;

/// Batch transfer request JSON: `{"files":[{"source":"/sdcard/a.bin",
/// "target":"/tmp/a.bin"}, ...], "trees":[{"source":"/sdcard/Docs",
/// "target":"/tmp/Docs"}, ...], "overwrite":false}`. `files`/`trees` are
/// optional (default empty), `overwrite` optional (default false). The
/// remote side of every pair (source for download, target for upload) is
/// resolved against the device root by the application layer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FfiBatchRequest {
    #[serde(default)]
    files: Vec<BatchTransferItemDto>,
    #[serde(default)]
    trees: Vec<TreeTransferDto>,
    overwrite: Option<bool>,
}

/// Parse a batch request, rejecting invalid JSON with a stable
/// `InvalidArgument` error.
fn batch_request_from_json(
    json: &str,
    session_id: SessionId,
) -> Result<BatchTransferRequest, HsCallResult> {
    let ffi: FfiBatchRequest = serde_json::from_str(json).map_err(|error| {
        err(
            &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                .with_detail(error.to_string()),
        )
    })?;
    Ok(BatchTransferRequest {
        session_id,
        files: ffi.files,
        trees: ffi.trees,
        overwrite: ffi.overwrite.unwrap_or(false),
    })
}

/// Start a batch download (files + directory trees) as one background
/// transfer. Result JSON: `{"transfer_id": N}`.
///
/// # Safety
/// `runtime` must be a valid handle; `request_ptr`/`request_len` must
/// describe valid, readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_transfer_start_batch_download(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("transfer_start_batch_download", || {
        let runtime = ffi_try!(runtime_ref(runtime, "transfer_start_batch_download"));
        let json = ffi_try!(input_str(
            request_ptr,
            request_len,
            "transfer_start_batch_download"
        ));
        let request = ffi_try!(batch_request_from_json(json, SessionId(session_id)));
        match runtime
            ._tokio
            .block_on(async { runtime.app.start_batch_download(request).await })
        {
            Ok(id) => ok(&serde_json::json!({ "transfer_id": id.0 })),
            Err(error) => err(&error),
        }
    })
}

/// Start a batch upload (files + directory trees) as one background
/// transfer. Same request shape as `hs_transfer_start_batch_download`
/// (`source` is the local side, `target` the remote destination).
/// Result JSON: `{"transfer_id": N}`.
///
/// # Safety
/// `runtime` must be a valid handle; `request_ptr`/`request_len` must
/// describe valid, readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_transfer_start_batch_upload(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("transfer_start_batch_upload", || {
        let runtime = ffi_try!(runtime_ref(runtime, "transfer_start_batch_upload"));
        let json = ffi_try!(input_str(
            request_ptr,
            request_len,
            "transfer_start_batch_upload"
        ));
        let request = ffi_try!(batch_request_from_json(json, SessionId(session_id)));
        match runtime
            ._tokio
            .block_on(async { runtime.app.start_batch_upload(request).await })
        {
            Ok(id) => ok(&serde_json::json!({ "transfer_id": id.0 })),
            Err(error) => err(&error),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ffi_test_util::{error_code_of, runtime_ptr};
    use crate::hs_runtime_destroy;

    const BATCH_REQUEST: &[u8] = br#"{"files":[{"source":"/sdcard/a.bin","target":"/tmp/a.bin"}],"trees":[],"overwrite":false}"#;

    #[test]
    fn batch_download_null_handle_returns_invalid_argument() {
        let result = unsafe {
            hs_transfer_start_batch_download(
                std::ptr::null_mut(),
                1,
                BATCH_REQUEST.as_ptr(),
                BATCH_REQUEST.len(),
            )
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn batch_download_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_transfer_start_batch_download(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn batch_download_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe {
            hs_transfer_start_batch_download(
                runtime,
                999,
                BATCH_REQUEST.as_ptr(),
                BATCH_REQUEST.len(),
            )
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn batch_upload_null_handle_returns_invalid_argument() {
        let result = unsafe {
            hs_transfer_start_batch_upload(
                std::ptr::null_mut(),
                1,
                BATCH_REQUEST.as_ptr(),
                BATCH_REQUEST.len(),
            )
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn batch_upload_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_transfer_start_batch_upload(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn batch_upload_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe {
            hs_transfer_start_batch_upload(
                runtime,
                999,
                BATCH_REQUEST.as_ptr(),
                BATCH_REQUEST.len(),
            )
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }
}
