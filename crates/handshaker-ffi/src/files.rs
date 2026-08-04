//! FFI: file service (Phase E / E1) — stat, count, move, delete.
//!
//! Every exported function follows the crate-wide contract: panic isolation
//! via `catch`, NULL-safe handles, and stable `InvalidArgument` errors for
//! bad input. Complex arguments/results are UTF-8 JSON buffers.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;

use handshaker_application::{
    CountFilesRequest, DeletePathsRequest, MovePathRequest, PublicError, PublicErrorCode,
    SessionId, StatFileRequest, UpdateFileInfoItemDto, UpdateFileInfoRequest,
};
use serde::Deserialize;

use crate::ffi_try;
use crate::result::{HsCallResult, catch, err, input_str, ok};
use crate::runtime_ref;

/// `hs_stat_file` request JSON: `{"path":"/storage/emulated/0/a.txt"}`
/// (path optional, defaults to `"."`). Result JSON:
/// `{"file": <FileEntryDto|null>}` — `null` when the phone reports the
/// path as missing.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_stat_file(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("stat_file", || {
        let runtime = ffi_try!(runtime_ref(runtime, "stat_file"));
        let json = ffi_try!(input_str(request_ptr, request_len, "stat_file"));
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FfiStatFileRequest {
            path: Option<String>,
        }
        let ffi: FfiStatFileRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let request = StatFileRequest {
            session_id: SessionId(session_id),
            path: ffi.path.unwrap_or_else(|| ".".to_string()),
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.stat_file(request).await })
        {
            Ok(file) => ok(&serde_json::json!({ "file": file })),
            Err(error) => err(&error),
        }
    })
}

/// `hs_count_files` request JSON: `{"path":".","depth":1,"exclusions":[]}`
/// (all optional; path defaults to `"."`, depth to 1, exclusions to none).
/// Result JSON: `{"count": N}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_count_files(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("count_files", || {
        let runtime = ffi_try!(runtime_ref(runtime, "count_files"));
        let json = ffi_try!(input_str(request_ptr, request_len, "count_files"));
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FfiCountFilesRequest {
            path: Option<String>,
            depth: Option<u32>,
            #[serde(default)]
            exclusions: Vec<String>,
        }
        let ffi: FfiCountFilesRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let request = CountFilesRequest {
            session_id: SessionId(session_id),
            path: ffi.path.unwrap_or_else(|| ".".to_string()),
            depth: ffi.depth.unwrap_or(1),
            exclusions: ffi.exclusions,
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.count_files(request).await })
        {
            Ok(count) => ok(&serde_json::json!({ "count": count })),
            Err(error) => err(&error),
        }
    })
}

/// `hs_move_path` request JSON: `{"source":"/a.txt","target":"/b.txt"}`
/// (both required). Result JSON: `{"moved":true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_move_path(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("move_path", || {
        let runtime = ffi_try!(runtime_ref(runtime, "move_path"));
        let json = ffi_try!(input_str(request_ptr, request_len, "move_path"));
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FfiMovePathRequest {
            source: String,
            target: String,
        }
        let ffi: FfiMovePathRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let request = MovePathRequest {
            session_id: SessionId(session_id),
            source: ffi.source,
            target: ffi.target,
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.move_path(request).await })
        {
            Ok(()) => ok(&serde_json::json!({ "moved": true })),
            Err(error) => err(&error),
        }
    })
}

/// `hs_delete_paths` request JSON:
/// `{"paths":["/a.txt","/b.txt"],"trash":false,"sync":false}` (all
/// optional except the intent; defaults: empty paths, no trash, no sync).
/// Result JSON: a `DeleteResultDto` (`{"deleted":[<FileEntryDto>,...]}`).
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_delete_paths(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("delete_paths", || {
        let runtime = ffi_try!(runtime_ref(runtime, "delete_paths"));
        let json = ffi_try!(input_str(request_ptr, request_len, "delete_paths"));
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FfiDeletePathsRequest {
            #[serde(default)]
            paths: Vec<String>,
            #[serde(default)]
            trash: bool,
            #[serde(default)]
            sync: bool,
        }
        let ffi: FfiDeletePathsRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let request = DeletePathsRequest {
            session_id: SessionId(session_id),
            paths: ffi.paths,
            trash: ffi.trash,
            sync: ffi.sync,
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.delete_paths(request).await })
        {
            Ok(result) => ok(&result),
            Err(error) => err(&error),
        }
    })
}

/// `hs_update_file_info` request JSON:
/// `{"files":[{"path":"/sdcard/a.jpg","size":1024,"is_directory":false,
///   "created_at":123,"modified_at":456,"checksum":null,"is_trash":null,
///   "id":7,"ext_data":null}],"is_sync":false}` — `files` and `is_sync`
/// optional (default: empty list, no sync); `session_id` always comes from
/// the call argument and overrides any JSON value. The phone writes the
/// reported fields back into its media store. Result JSON: `{"updated":true}`.
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_update_file_info(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("update_file_info", || {
        let runtime = ffi_try!(runtime_ref(runtime, "update_file_info"));
        let json = ffi_try!(input_str(request_ptr, request_len, "update_file_info"));
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FfiUpdateFileInfoRequest {
            #[serde(default)]
            files: Vec<UpdateFileInfoItemDto>,
            #[serde(default)]
            is_sync: bool,
        }
        let ffi: FfiUpdateFileInfoRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string())
                    .operation("update_file_info"),
            )
        }));
        let request = UpdateFileInfoRequest {
            session_id: SessionId(session_id),
            files: ffi.files,
            is_sync: ffi.is_sync,
        };
        match runtime
            ._tokio
            .block_on(async { runtime.app.update_files_info(request).await })
        {
            Ok(_updated) => ok(&serde_json::json!({ "updated": true })),
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
    fn stat_file_null_handle_returns_invalid_argument() {
        let request = br#"{"path":"/a"}"#;
        let result =
            unsafe { hs_stat_file(std::ptr::null_mut(), 1, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn stat_file_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_stat_file(runtime, 1, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn stat_file_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_stat_file(runtime, 999, br#"{}"#.as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn count_files_null_handle_returns_invalid_argument() {
        let request = br#"{"path":"/a"}"#;
        let result =
            unsafe { hs_count_files(std::ptr::null_mut(), 1, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn count_files_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_count_files(runtime, 1, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn count_files_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_count_files(runtime, 999, br#"{}"#.as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn move_path_null_handle_returns_invalid_argument() {
        let request = br#"{"source":"/a","target":"/b"}"#;
        let result =
            unsafe { hs_move_path(std::ptr::null_mut(), 1, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn move_path_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_move_path(runtime, 1, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn move_path_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let request = br#"{"source":"/a","target":"/b"}"#;
        let result = unsafe { hs_move_path(runtime, 999, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn delete_paths_null_handle_returns_invalid_argument() {
        let request = br#"{"paths":["/a"]}"#;
        let result =
            unsafe { hs_delete_paths(std::ptr::null_mut(), 1, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn delete_paths_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_delete_paths(runtime, 1, b"{oops".as_ptr(), 5) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn delete_paths_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let request = br#"{"paths":["/a"]}"#;
        let result = unsafe { hs_delete_paths(runtime, 999, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn update_file_info_null_handle_returns_invalid_argument() {
        let request = br#"{"files":[]}"#;
        let result = unsafe {
            hs_update_file_info(std::ptr::null_mut(), 1, request.as_ptr(), request.len())
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn update_file_info_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_update_file_info(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn update_file_info_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let request = br#"{"files":[]}"#;
        let result = unsafe { hs_update_file_info(runtime, 999, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }
}
