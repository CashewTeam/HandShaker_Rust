//! Photo-sync FFI surface (ABI 1.4).
//!
//! No call blocks on a full sync run: `hs_sync_start` registers the job
//! and spawns the background run, returning immediately with the profile
//! id — progress is polled with `hs_sync_status` and/or observed through
//! the event subscription (`SyncWatchApplied`, `TransferUpdated`,
//! `Warning`). The caller orchestrates: plan → start → poll/events →
//! (optionally) start_watch → stop_watch/stop. (`hs_sync_plan` performs
//! one bounded device round-trip for the photo listing; the phone-side
//! reject retry is capped at 3 × 1.5 s.)

use std::ffi::c_void;

use serde_json::Value;

use crate::ffi_try;
use crate::result::{HsCallResult, catch, err, input_str, ok};
use crate::{PublicError, PublicErrorCode, SessionId, runtime_ref};

use handshaker_application::{SyncProfileDto, SyncStatusDto};

/// Legacy camera folder used when `remote_root` is omitted.
const DEFAULT_SYNC_ROOT: &str = "/storage/emulated/0/DCIM/Camera";

/// Parse a `SyncProfileDto` JSON request. The session id always comes from
/// the call argument (never from the payload); `id` defaults to
/// `device_uuid`, `remote_root` to the camera folder and `enabled` to true.
fn parse_profile(
    session_id: u64,
    request_json: &[u8],
    operation: &str,
) -> Result<SyncProfileDto, HsCallResult> {
    let value: Value = serde_json::from_slice(request_json).map_err(|error| {
        err(
            &PublicError::new(PublicErrorCode::InvalidArgument, "invalid JSON request")
                .with_detail(error.to_string())
                .operation(operation),
        )
    })?;
    let device_uuid = value
        .get("device_uuid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "device_uuid is required")
                    .operation(operation),
            )
        })?
        .to_string();
    let local_root = value
        .get("local_root")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "local_root is required")
                    .operation(operation),
            )
        })?
        .to_string();
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&device_uuid)
        .to_string();
    let remote_root = value
        .get("remote_root")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SYNC_ROOT)
        .to_string();
    let enabled = value
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Ok(SyncProfileDto {
        id,
        session_id: SessionId(session_id),
        device_uuid,
        remote_root,
        local_root,
        enabled,
    })
}

/// Sync plan (ABI 1.4). Request: SyncProfileDto JSON (session id comes from
/// the call argument). Result: SyncPlanDto (downloads/metadata_updates/
/// deletions/conflicts/total_bytes/executable).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_sync_plan(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("sync.plan", || {
        let runtime = ffi_try!(runtime_ref(runtime, "sync.plan"));
        let request = ffi_try!(input_str(request_ptr, request_len, "sync.plan"));
        let profile = ffi_try!(parse_profile(session_id, request.as_bytes(), "sync.plan"));
        let plan = match runtime
            ._tokio
            .block_on(async { runtime.app.plan_sync(profile).await })
        {
            Ok(plan) => plan,
            Err(error) => return err(&error),
        };
        ok(&plan)
    })
}

/// Start a background sync run (ABI 1.4). Request: SyncProfileDto JSON.
/// Result: {"profile_id":"<id>"}. The run proceeds in the background;
/// poll `hs_sync_status` or subscribe to events.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_sync_start(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("sync.start", || {
        let runtime = ffi_try!(runtime_ref(runtime, "sync.start"));
        let request = ffi_try!(input_str(request_ptr, request_len, "sync.start"));
        let profile = ffi_try!(parse_profile(session_id, request.as_bytes(), "sync.start"));
        let profile_id = match runtime
            ._tokio
            .block_on(async { runtime.app.start_sync(profile).await })
        {
            Ok(profile_id) => profile_id,
            Err(error) => return err(&error),
        };
        ok(&serde_json::json!({ "profile_id": profile_id }))
    })
}

/// Sync job status (ABI 1.4). `profile_id` is the id returned by
/// `hs_sync_start`. Result: SyncStatusDto (running/monitoring/last run/
/// last error). Errors: NotFound when the profile was never started.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_sync_status(
    runtime: *mut c_void,
    profile_id_ptr: *const u8,
    profile_id_len: usize,
) -> HsCallResult {
    catch("sync.status", || {
        let runtime = ffi_try!(runtime_ref(runtime, "sync.status"));
        let profile_id = ffi_try!(input_str(profile_id_ptr, profile_id_len, "sync.status"));
        let status: SyncStatusDto = match runtime
            ._tokio
            .block_on(async { runtime.app.get_sync_status(profile_id).await })
        {
            Ok(status) => status,
            Err(error) => return err(&error),
        };
        ok(&status)
    })
}

/// Stop a running sync job (ABI 1.4). Result: {"stopped":true}.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_sync_stop(
    runtime: *mut c_void,
    profile_id_ptr: *const u8,
    profile_id_len: usize,
) -> HsCallResult {
    catch("sync.stop", || {
        let runtime = ffi_try!(runtime_ref(runtime, "sync.stop"));
        let profile_id = ffi_try!(input_str(profile_id_ptr, profile_id_len, "sync.stop"));
        if let Err(error) = runtime
            ._tokio
            .block_on(async { runtime.app.stop_sync(profile_id).await })
        {
            return err(&error);
        }
        ok(&serde_json::json!({ "stopped": true }))
    })
}

/// Start the incremental watch loop for a completed sync run (ABI 1.4).
/// The phone must be in SYNCING state (a `hs_sync_start` run must have
/// finished first — check `hs_sync_status` for `running:false`); the
/// monitor then applies debounced batches, published as
/// `SyncWatchApplied` events. Result: {"started":true}.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_sync_start_watch(
    runtime: *mut c_void,
    profile_id_ptr: *const u8,
    profile_id_len: usize,
) -> HsCallResult {
    catch("sync.start_watch", || {
        let runtime = ffi_try!(runtime_ref(runtime, "sync.start_watch"));
        let profile_id = ffi_try!(input_str(
            profile_id_ptr,
            profile_id_len,
            "sync.start_watch"
        ));
        if let Err(error) = runtime
            ._tokio
            .block_on(async { runtime.app.start_sync_watch(profile_id).await })
        {
            return err(&error);
        }
        ok(&serde_json::json!({ "started": true }))
    })
}

/// Stop the incremental watch loop (ABI 1.4). Result: {"stopped":true}.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_sync_stop_watch(
    runtime: *mut c_void,
    profile_id_ptr: *const u8,
    profile_id_len: usize,
) -> HsCallResult {
    catch("sync.stop_watch", || {
        let runtime = ffi_try!(runtime_ref(runtime, "sync.stop_watch"));
        let profile_id = ffi_try!(input_str(profile_id_ptr, profile_id_len, "sync.stop_watch"));
        if let Err(error) = runtime
            ._tokio
            .block_on(async { runtime.app.stop_sync_watch(profile_id).await })
        {
            return err(&error);
        }
        ok(&serde_json::json!({ "stopped": true }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ffi_test_util::{error_code_of, runtime_ptr};
    use crate::hs_runtime_destroy;

    const PROFILE: &str = r#"{
        "id":"phone:abc",
        "device_uuid":"phone:abc",
        "remote_root":"/storage/emulated/0/DCIM/Camera",
        "local_root":"/tmp/sync-out",
        "enabled":true
    }"#;

    #[test]
    fn sync_plan_null_handle_returns_invalid_argument() {
        let result =
            unsafe { hs_sync_plan(std::ptr::null_mut(), 1, PROFILE.as_ptr(), PROFILE.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn sync_start_null_handle_returns_invalid_argument() {
        let result =
            unsafe { hs_sync_start(std::ptr::null_mut(), 1, PROFILE.as_ptr(), PROFILE.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn sync_plan_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_sync_plan(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn sync_plan_missing_device_uuid_is_rejected() {
        let runtime = runtime_ptr();
        let request = br#"{"local_root":"/tmp/sync-out"}"#;
        let result = unsafe { hs_sync_plan(runtime, 1, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn sync_plan_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_sync_plan(runtime, 999, PROFILE.as_ptr(), PROFILE.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn sync_start_registers_job_even_for_missing_session() {
        // start_sync registers the job and spawns the background run; the
        // failure (missing session) surfaces later via hs_sync_status's
        // last_error, not as a call error — matching the CLI poll loop.
        let runtime = runtime_ptr();
        let result = unsafe { hs_sync_start(runtime, 999, PROFILE.as_ptr(), PROFILE.len()) };
        assert_eq!(result.status, 0);
        let bytes = unsafe { crate::buffer::into_vec(result.value) };
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["profile_id"], "phone:abc");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn sync_status_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_sync_status(std::ptr::null_mut(), b"phone:abc".as_ptr(), 9) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn sync_status_unknown_profile_returns_not_found() {
        let runtime = runtime_ptr();
        let id = b"phone:nope";
        let result = unsafe { hs_sync_status(runtime, id.as_ptr(), id.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn sync_stop_unknown_profile_returns_not_found() {
        let runtime = runtime_ptr();
        let id = b"phone:nope";
        let result = unsafe { hs_sync_stop(runtime, id.as_ptr(), id.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn sync_start_watch_unknown_profile_returns_not_found() {
        let runtime = runtime_ptr();
        let id = b"phone:nope";
        let result = unsafe { hs_sync_start_watch(runtime, id.as_ptr(), id.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn sync_stop_watch_unknown_profile_returns_not_found() {
        let runtime = runtime_ptr();
        let id = b"phone:nope";
        let result = unsafe { hs_sync_stop_watch(runtime, id.as_ptr(), id.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn parse_profile_applies_defaults_and_session_override() {
        let request = br#"{
            "device_uuid":"phone:def",
            "local_root":"/tmp/out"
        }"#;
        let profile = parse_profile(42, request, "test").expect("defaults");
        assert_eq!(profile.id, "phone:def");
        assert_eq!(profile.session_id, SessionId(42));
        assert_eq!(profile.remote_root, DEFAULT_SYNC_ROOT);
        assert!(profile.enabled);
        // Explicit id/remote_root/enabled win over defaults.
        let request = br#"{
            "id":"custom",
            "device_uuid":"phone:def",
            "remote_root":"/sdcard/DCIM",
            "local_root":"/tmp/out",
            "enabled":false
        }"#;
        let profile = parse_profile(7, request, "test").expect("explicit");
        assert_eq!(profile.id, "custom");
        assert_eq!(profile.session_id, SessionId(7));
        assert_eq!(profile.remote_root, "/sdcard/DCIM");
        assert!(!profile.enabled);
    }

    #[test]
    fn parse_profile_rejects_missing_required_fields() {
        assert!(parse_profile(1, br#"{"device_uuid":"phone:a"}"#, "test").is_err());
        assert!(parse_profile(1, br#"{"local_root":"/tmp/out"}"#, "test").is_err());
        assert!(parse_profile(1, br#"{"device_uuid":"","local_root":""}"#, "test").is_err());
        assert!(parse_profile(1, b"{oops", "test").is_err());
    }
}
