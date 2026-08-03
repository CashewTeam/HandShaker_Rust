//! FFI: runtime diagnostics (Phase E / E6) — ABI/crate/platform identity,
//! adb availability, state-dir and wire-log config, and live session/
//! transfer counters.
//!
//! `hs_runtime_diagnostics` takes no request buffer. Everything is derived
//! from the runtime handle, so NULL is rejected with the stable
//! `InvalidArgument` error and no panic ever crosses the ABI.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::process::Command;

use handshaker_application::TransferState;

use crate::ffi_try;
use crate::result::{HsCallResult, catch, ok};
use crate::runtime_ref;

/// `hs_runtime_diagnostics` — no request. Result JSON:
/// `{"abi":"1.5.0","application_api":"1.0.0-preview.1",
///   "crate_version":"0.x.y","platform":"macos","arch":"aarch64",
///   "adb_path":"adb","adb_available":true|false,
///   "adb_version":"Android Debug Bridge version 1.0.41"|null,
///   "state_dir":"/path"|null,"wire_log_enabled":true|false,
///   "active_sessions":N,"active_transfers":N,
///   "capabilities":["files","clipboard","trust","media","batch","sync",
///    "monitor","events","discovery","diagnostics","update_file_info",
///    "media_merge"]}`.
/// `adb_available`/`adb_version` probe the configured adb binary with
/// `adb version` (first output line); `active_transfers` counts snapshots
/// that are not in a terminal state (Queued or Running).
///
/// # Safety
/// `runtime` must be a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_runtime_diagnostics(runtime: *mut c_void) -> HsCallResult {
    catch("runtime_diagnostics", || {
        let runtime = ffi_try!(runtime_ref(runtime, "runtime_diagnostics"));
        let config = runtime.app.config();
        let adb_path = config.adb_path.display().to_string();
        let (adb_available, adb_version) = adb_probe(&adb_path);
        let state_dir = config
            .state_dir
            .as_ref()
            .map(|dir| dir.display().to_string());
        let wire_log_enabled = config.wire_log.is_some();
        let active_sessions = runtime
            ._tokio
            .block_on(async { runtime.app.session_count().await });
        let active_transfers: i64 = match runtime
            ._tokio
            .block_on(async { runtime.app.list_transfers().await })
        {
            Ok(snapshots) => snapshots
                .iter()
                .filter(|snapshot| !is_terminal(&snapshot.state))
                .count() as i64,
            // Diagnostics must never fail because of a transfer-registry
            // error; report the counter as unknown (-1) instead.
            Err(_) => -1,
        };
        ok(&serde_json::json!({
            "abi": format!(
                "{}.{}.{}",
                crate::ABI_VERSION_MAJOR,
                crate::ABI_VERSION_MINOR,
                crate::ABI_VERSION_PATCH
            ),
            "application_api": handshaker_application::APPLICATION_API_VERSION,
            "crate_version": env!("CARGO_PKG_VERSION"),
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "adb_path": adb_path,
            "adb_available": adb_available,
            "adb_version": adb_version,
            "state_dir": state_dir,
            "wire_log_enabled": wire_log_enabled,
            "active_sessions": active_sessions,
            "active_transfers": active_transfers,
            "capabilities": [
                "files", "clipboard", "trust", "media", "batch", "sync",
                "monitor", "events", "discovery", "diagnostics",
                "update_file_info", "media_merge",
            ],
        }))
    })
}

/// Run `adb version` synchronously with a bounded deadline and return
/// (available, first-line version). Any failure (missing binary, non-zero
/// exit, no output, timeout) yields `(false, None)` — diagnostics never
/// fail on an adb problem, and a hung adb cannot block the caller
/// (review fix: `Command::output()` alone would wait forever).
fn adb_probe(adb_path: &str) -> (bool, Option<String>) {
    const ADB_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let Ok(mut child) = Command::new(adb_path)
        .arg("version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return (false, None);
    };
    let deadline = std::time::Instant::now() + ADB_PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return (false, None);
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            // A try_wait error still leaves the child behind: kill + reap so
            // a rare poll failure cannot leak a zombie (review fix).
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return (false, None);
            }
        }
    };
    if !status.success() {
        return (false, None);
    }
    let Ok(output) = child.wait_with_output() else {
        return (false, None);
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string);
    (true, first_line)
}

/// Terminal transfer states: Completed, Failed, Cancelled. Everything else
/// (Queued, Running, or any future state) counts as active.
fn is_terminal(state: &TransferState) -> bool {
    matches!(
        state,
        TransferState::Completed | TransferState::Failed | TransferState::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::buffer;
    use crate::ffi_test_util::runtime_ptr;
    use crate::hs_runtime_destroy;
    use crate::result::free_result;

    #[test]
    fn diagnostics_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_runtime_diagnostics(std::ptr::null_mut()) };
        assert_eq!(result.status, 1);
        let bytes = unsafe { buffer::into_vec(result.error) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("error json");
        assert_eq!(decoded["code"], "invalid_argument");
        unsafe { free_result(Default::default()) };
    }

    #[test]
    fn diagnostics_reports_abi_platform_and_capabilities() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_runtime_diagnostics(runtime) };
        assert_eq!(result.status, 0, "diagnostics must succeed");
        let bytes = unsafe { buffer::into_vec(result.value) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("value json");
        unsafe { free_result(Default::default()) };
        assert_eq!(decoded["abi"], "1.5.0");
        assert_eq!(decoded["application_api"], "1.0.0-preview.1");
        assert!(decoded["crate_version"].is_string());
        assert!(decoded["platform"].is_string());
        assert!(decoded["arch"].is_string());
        assert!(decoded["adb_path"].is_string());
        assert!(decoded["adb_available"].is_boolean());
        assert!(
            decoded["adb_version"].is_null() || decoded["adb_version"].is_string(),
            "adb_version must be a string or null"
        );
        assert!(
            decoded["state_dir"].is_null() || decoded["state_dir"].is_string(),
            "state_dir must be a string or null"
        );
        assert!(decoded["wire_log_enabled"].is_boolean());
        assert!(decoded["active_sessions"].is_number());
        assert!(decoded["active_transfers"].is_number());
        let capabilities: Vec<String> = decoded["capabilities"]
            .as_array()
            .expect("capabilities array")
            .iter()
            .map(|value| value.as_str().expect("capability string").to_string())
            .collect();
        for expected in [
            "files",
            "clipboard",
            "trust",
            "media",
            "batch",
            "sync",
            "monitor",
            "events",
            "discovery",
            "diagnostics",
            "update_file_info",
            "media_merge",
        ] {
            assert!(
                capabilities.contains(&expected.to_string()),
                "missing {expected}"
            );
        }
        // The photo-sync FFI ships since ABI 1.4.0: the capability must be
        // present so feature discovery via hs_runtime_diagnostics sees it.
        assert!(
            capabilities.contains(&"sync".to_string()),
            "missing sync capability"
        );
        unsafe { hs_runtime_destroy(runtime) };
    }
}
