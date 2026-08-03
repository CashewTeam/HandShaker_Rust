//! Owned byte buffers crossing the FFI boundary.
//!
//! Ownership rules (M8 §7.5):
//! - memory allocated by Rust is freed only by Rust (`hs_byte_buffer_free`);
//! - an empty buffer is `{ NULL, 0, 0 }` and free is safe on it;
//! - callers never modify `capacity`.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;

/// Byte buffer owned by the Rust side.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HsByteBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl Default for HsByteBuffer {
    fn default() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }
}

/// Build a buffer from bytes (copies into a fresh `Vec`).
pub fn from_bytes(bytes: Vec<u8>) -> HsByteBuffer {
    if bytes.is_empty() {
        return HsByteBuffer::default();
    }
    let mut boxed = bytes.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    let len = boxed.len();
    let capacity = len;
    // Leak the box; `hs_byte_buffer_free` reclaims it via `Vec::from_raw_parts`.
    std::mem::forget(boxed);
    HsByteBuffer { ptr, len, capacity }
}

pub fn from_str(value: &str) -> HsByteBuffer {
    from_bytes(value.as_bytes().to_vec())
}

/// Reclaim a buffer produced by `from_bytes`. Safe on empty and NULL buffers;
/// double-free is caller error (documented, not detected).
///
/// # Safety
/// `buffer` must have been produced by `from_bytes` (or be the empty
/// `{ NULL, 0, 0 }`) and must not have been freed already.
pub unsafe fn free_buffer(buffer: HsByteBuffer) {
    if buffer.ptr.is_null() {
        return;
    }
    let slice = std::slice::from_raw_parts_mut(buffer.ptr, buffer.capacity);
    let vec = Vec::from_raw_parts(slice.as_mut_ptr(), buffer.len, buffer.capacity);
    drop(vec);
}

/// Convert back into a `Vec` (used only inside tests).
#[allow(dead_code)]
pub unsafe fn into_vec(buffer: HsByteBuffer) -> Vec<u8> {
    if buffer.ptr.is_null() {
        return Vec::new();
    }
    let slice = std::slice::from_raw_parts_mut(buffer.ptr, buffer.capacity);
    Vec::from_raw_parts(slice.as_mut_ptr(), buffer.len, buffer.capacity)
}

/// Raw pointer value that is safe to pass through FFI (never dereferenced by
/// C). Used for opaque handles.
#[allow(dead_code)]
pub fn opaque<T>(value: &T) -> *const c_void {
    value as *const T as *const c_void
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_round_trip_preserves_bytes() {
        let buffer = from_bytes(vec![1, 2, 3, 4]);
        assert_eq!(buffer.len, 4);
        let bytes = unsafe { into_vec(buffer) };
        assert_eq!(bytes, vec![1, 2, 3, 4]);
    }

    #[test]
    fn empty_buffer_is_null_and_free_is_safe() {
        let buffer = HsByteBuffer::default();
        assert!(buffer.ptr.is_null());
        unsafe { free_buffer(buffer) };
        // Freeing the default value again must not touch freed memory.
        unsafe { free_buffer(HsByteBuffer::default()) };
    }

    #[test]
    fn free_reclaims_allocation() {
        let buffer = from_bytes(vec![0_u8; 64]);
        unsafe { free_buffer(buffer) };
    }
}
