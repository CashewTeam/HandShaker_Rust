//! Unified FFI call result: `status == 0` means success; on failure the
//! `error` buffer holds the `PublicError` JSON and `value` is empty.
//!
//! Every exported function catches panics and maps them to `Internal`, so no
//! unwind ever crosses the ABI.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

use handshaker_application::{PublicError, PublicErrorCode};

use crate::buffer::{HsByteBuffer, free_buffer, from_str};

/// Unified result for all FFI calls.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct HsCallResult {
    /// 0 = success; non-zero = failure (error JSON in `error`).
    pub status: i32,
    /// Success payload: UTF-8 JSON (empty on failure).
    pub value: HsByteBuffer,
    /// Failure payload: `PublicError` JSON (empty on success).
    pub error: HsByteBuffer,
}

pub fn ok<T: serde::Serialize>(value: &T) -> HsCallResult {
    match serde_json::to_string(value) {
        Ok(json) => HsCallResult {
            status: 0,
            value: from_str(&json),
            error: HsByteBuffer::default(),
        },
        Err(error) => err(
            &PublicError::new(PublicErrorCode::Internal, "serialization failed")
                .with_detail(error.to_string()),
        ),
    }
}

pub fn err(error: &PublicError) -> HsCallResult {
    let json = serde_json::to_string(error).unwrap_or_else(|_| {
        "{\"code\":\"internal\",\"message\":\"error serialization failed\",\"detail\":null,\"retryable\":false,\"operation\":null}".to_string()
    });
    HsCallResult {
        status: 1,
        value: HsByteBuffer::default(),
        error: from_str(&json),
    }
}

/// Free both buffers of a result. Safe on empty/NULL buffers; double-free is
/// caller error.
///
/// # Safety
/// `result` must come from this library and not have been freed already.
pub unsafe fn free_result(result: HsCallResult) {
    free_buffer(result.value);
    free_buffer(result.error);
}

/// Wrap a fallible closure with panic isolation. `operation` labels the
/// PublicError for diagnostics.
pub fn catch<F>(operation: &str, f: F) -> HsCallResult
where
    F: FnOnce() -> HsCallResult,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(_payload) => {
            err(&PublicError::new(PublicErrorCode::Internal, "internal panic").operation(operation))
        }
    }
}

/// `?`-like helper for `Result<_, HsCallResult>` inside `catch` closures
/// (which return `HsCallResult` directly). Usage: `let x = ffi_try!(f()?);`
#[macro_export]
macro_rules! ffi_try {
    ($expression:expr) => {
        match $expression {
            Ok(value) => value,
            Err(error) => return error,
        }
    };
}

/// Convert a pointer+len pair into a `&str`, or a NULL/invalid-UTF-8 error.
///
/// # Safety
/// `ptr`/`len` must describe a valid buffer for the call duration.
pub unsafe fn input_str<'a>(
    ptr: *const u8,
    len: usize,
    operation: &str,
) -> Result<&'a str, HsCallResult> {
    if ptr.is_null() {
        return Err(err(&PublicError::new(
            PublicErrorCode::InvalidArgument,
            "NULL input",
        )
        .operation(operation)));
    }
    let bytes = std::slice::from_raw_parts(ptr, len);
    std::str::from_utf8(bytes).map_err(|_| {
        err(
            &PublicError::new(PublicErrorCode::InvalidArgument, "input is not valid UTF-8")
                .operation(operation),
        )
    })
}

/// Dereference an output handle slot safely (write only on success).
///
/// # Safety
/// `out` must point to writable memory sized for `*mut c_void`.
pub unsafe fn out_slot<'a>(
    out: *mut *mut c_void,
    operation: &str,
) -> Result<&'a mut *mut c_void, HsCallResult> {
    if out.is_null() {
        return Err(err(&PublicError::new(
            PublicErrorCode::InvalidArgument,
            "NULL output slot",
        )
        .operation(operation)));
    }
    Ok(&mut *out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize)]
    struct Payload {
        n: u32,
    }

    #[test]
    fn ok_result_has_json_value_and_no_error() {
        let result = ok(&Payload { n: 7 });
        assert_eq!(result.status, 0);
        let bytes = unsafe { crate::buffer::into_vec(result.value) };
        assert_eq!(bytes, br#"{"n":7}"#);
        assert!(result.error.ptr.is_null());
        // value Vec moved out; error is empty — nothing to free.
        unsafe { free_result(HsCallResult::default()) };
    }

    #[test]
    fn err_result_has_error_json_and_no_value() {
        let error = PublicError::new(PublicErrorCode::SessionNotFound, "missing");
        let result = err(&error);
        assert_eq!(result.status, 1);
        let bytes = unsafe { crate::buffer::into_vec(result.error) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(decoded["code"], "session_not_found");
        assert!(result.value.ptr.is_null());
        // error Vec moved out; value is empty — nothing to free.
        unsafe { free_result(HsCallResult::default()) };
    }

    #[test]
    fn catch_converts_panic_to_internal() {
        let result = catch("test", || -> HsCallResult {
            panic!("boom");
        });
        assert_eq!(result.status, 1);
        let bytes = unsafe { crate::buffer::into_vec(result.error) };
        let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(decoded["code"], "internal");
        assert_eq!(decoded["operation"], "test");
        // error Vec moved out; value is empty — nothing to free.
        unsafe { free_result(HsCallResult::default()) };
    }

    #[test]
    fn input_str_rejects_null_and_invalid_utf8() {
        let null = unsafe { input_str(std::ptr::null(), 0, "t") };
        assert!(null.is_err());
        let invalid = unsafe { input_str([0xff, 0xfe].as_ptr(), 2, "t") };
        assert!(invalid.is_err());
        let ok = unsafe { input_str(b"hello".as_ptr(), 5, "t") };
        assert_eq!(ok.unwrap(), "hello");
    }
}

#[cfg(test)]
mod panic_mini_tests {
    #[test]
    fn raw_catch_unwind_works() {
        let r = std::panic::catch_unwind(|| -> i32 {
            panic!("mini");
        });
        assert!(r.is_err());
    }
}
