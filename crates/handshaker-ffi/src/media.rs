//! FFI: media service (Phase E / E5) — photo/video/audio libraries,
//! on-disk thumbnail caching, and EXIF metadata.
//!
//! Thumbnail design (Phase E priority 1): the thumbnail bytes returned by
//! the application are written to `<state_dir>/thumbnails/<kind>-<hash>.thumb`
//! on first request and reused on later requests (no re-fetch), so the
//! host app can render from local files. `<state_dir>` is the runtime's
//! configured `state_dir`, or the core default config directory when not
//! configured; failures to create the cache directory or write a thumbnail
//! are surfaced as explicit errors.
//!
//! Every exported function follows the crate-wide contract: panic isolation
//! via `catch`, NULL-safe handles, and stable `InvalidArgument` errors for
//! bad input. Complex arguments/results are UTF-8 JSON buffers.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use handshaker_application::{
    AudioAlbumDto, ImageFileDto, PublicError, PublicErrorCode, SessionId, VideoFileDto,
};
use serde::Deserialize;

use crate::HsRuntime;
use crate::ffi_try;
use crate::result::{HsCallResult, catch, err, input_str, ok};
use crate::runtime_ref;

// ---------------------------------------------------------------------------
// Library snapshots (no request parameters in the application API yet)
// ---------------------------------------------------------------------------

/// `hs_media_photo_library` request JSON: `{}`. The buffer is kept for ABI
/// symmetry with the thumbnail/exif endpoints and reserved for future
/// pagination (`limit`/`offset`); its content must be valid JSON but is
/// currently ignored. Result JSON: `PhotoLibraryDto`
/// (`{"images":[...],"albums":[...],"camera_album_id":N}`).
///
/// # Safety
/// `runtime` must be a valid handle; `request_ptr`/`request_len` must
/// describe valid, readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_media_photo_library(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("media_photo_library", || {
        let runtime = ffi_try!(runtime_ref(runtime, "media_photo_library"));
        let json = ffi_try!(input_str(request_ptr, request_len, "media_photo_library"));
        ffi_try!(validate_placeholder_request(json, "media_photo_library"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.get_photo_library(SessionId(session_id)).await })
        {
            Ok(library) => ok(&library),
            Err(error) => err(&error),
        }
    })
}

/// `hs_media_video_library` request JSON: `{}` (placeholder, see
/// `hs_media_photo_library`). Result JSON: `VideoLibraryDto`
/// (`{"videos":[...],"albums":[...]}`).
///
/// # Safety
/// `runtime` must be a valid handle; `request_ptr`/`request_len` must
/// describe valid, readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_media_video_library(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("media_video_library", || {
        let runtime = ffi_try!(runtime_ref(runtime, "media_video_library"));
        let json = ffi_try!(input_str(request_ptr, request_len, "media_video_library"));
        ffi_try!(validate_placeholder_request(json, "media_video_library"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.get_video_library(SessionId(session_id)).await })
        {
            Ok(library) => ok(&library),
            Err(error) => err(&error),
        }
    })
}

/// `hs_media_audio_library` request JSON: `{}` (placeholder, see
/// `hs_media_photo_library`). Result JSON: `AudioLibraryDto`
/// (`{"tracks":[...],"albums":[...]}`).
///
/// # Safety
/// `runtime` must be a valid handle; `request_ptr`/`request_len` must
/// describe valid, readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_media_audio_library(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("media_audio_library", || {
        let runtime = ffi_try!(runtime_ref(runtime, "media_audio_library"));
        let json = ffi_try!(input_str(request_ptr, request_len, "media_audio_library"));
        ffi_try!(validate_placeholder_request(json, "media_audio_library"));
        match runtime
            ._tokio
            .block_on(async { runtime.app.get_audio_library(SessionId(session_id)).await })
        {
            Ok(library) => ok(&library),
            Err(error) => err(&error),
        }
    })
}

/// Validate that a library-request buffer is well-formed JSON (the library
/// endpoints currently take no parameters; the buffer is accepted for ABI
/// symmetry and future pagination).
fn validate_placeholder_request(json: &str, operation: &str) -> Result<(), HsCallResult> {
    serde_json::from_str::<serde_json::Value>(json).map_err(|error| {
        err(
            &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                .with_detail(error.to_string())
                .operation(operation),
        )
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Thumbnails (Phase E / E5: on-disk cache + cache-path responses)
// ---------------------------------------------------------------------------

/// Thumbnail request entry for an image: only identity fields are
/// significant (the core fetches by path/media id); unknown extra fields
/// (e.g. a full `ImageFileDto` from the library response) are ignored.
#[derive(Debug, Default, Deserialize)]
struct FfiThumbnailImageItem {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    media_id: Option<u64>,
    #[serde(default)]
    album_id: Option<u64>,
}

/// Thumbnail request entry for a video (same identity fields).
#[derive(Debug, Default, Deserialize)]
struct FfiThumbnailVideoItem {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    media_id: Option<u64>,
    #[serde(default)]
    album_id: Option<u64>,
}

/// Thumbnail request entry for an audio album (identified by path/album id;
/// name/artist fields are carried through for album matching).
#[derive(Debug, Default, Deserialize)]
struct FfiThumbnailAudioItem {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    album_id: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    artist_id: Option<u64>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    year: Option<u32>,
}

/// Thumbnail request JSON:
/// `{"images":[{"path":"/sdcard/DCIM/1.jpg",...}, ...],
///   "videos":[...], "audio_albums":[...]}` — all three arrays optional
/// (default empty). Entries may be minimal `{"path":"..."}` objects or full
/// library DTOs (extra fields ignored).
#[derive(Debug, Default, Deserialize)]
struct FfiThumbnailRequest {
    #[serde(default)]
    images: Vec<FfiThumbnailImageItem>,
    #[serde(default)]
    videos: Vec<FfiThumbnailVideoItem>,
    #[serde(default)]
    audio_albums: Vec<FfiThumbnailAudioItem>,
}

/// `hs_media_thumbnail` — fetch thumbnails for the requested media entries
/// and cache them on disk. Result JSON:
/// `{"images":[{"path":"/remote/path","cache_path":"/abs/cache/file","size":N}, ...],
///   "videos":[...], "audio_albums":[...]}` — only entries that returned
/// thumbnail data are listed; each `cache_path` points at
/// `<state_dir>/thumbnails/<kind>-<hash>.thumb` and is reused across calls
/// without re-fetching.
///
/// # Safety
/// `runtime` must be a valid handle; `request_ptr`/`request_len` must
/// describe valid, readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_media_thumbnail(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("media_thumbnail", || {
        let runtime = ffi_try!(runtime_ref(runtime, "media_thumbnail"));
        let json = ffi_try!(input_str(request_ptr, request_len, "media_thumbnail"));
        let ffi: FfiThumbnailRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        let images: Vec<ImageFileDto> = ffi
            .images
            .iter()
            .map(|item| ImageFileDto {
                path: item.path.clone(),
                media_id: item.media_id,
                album_id: item.album_id,
                ..ImageFileDto::default()
            })
            .collect();
        let videos: Vec<VideoFileDto> = ffi
            .videos
            .iter()
            .map(|item| VideoFileDto {
                path: item.path.clone(),
                media_id: item.media_id,
                album_id: item.album_id,
                ..VideoFileDto::default()
            })
            .collect();
        let audio_albums: Vec<AudioAlbumDto> = ffi
            .audio_albums
            .iter()
            .map(|item| AudioAlbumDto {
                path: item.path.clone(),
                album_id: item.album_id,
                name: item.name.clone(),
                artist_id: item.artist_id,
                artist: item.artist.clone(),
                year: item.year,
                thumbnail: None,
                thumbnail_error: false,
            })
            .collect();
        let thumbnails = match runtime._tokio.block_on(async {
            runtime
                .app
                .get_thumbnails(SessionId(session_id), &images, &videos, &audio_albums)
                .await
        }) {
            Ok(thumbnails) => thumbnails,
            Err(error) => return err(&error),
        };
        let cache_dir = ffi_try!(thumbnail_cache_dir(runtime, "media_thumbnail"));
        let mut out_images = Vec::new();
        for image in &thumbnails.images {
            if let (Some(path), Some(bytes)) = (&image.path, &image.thumbnail) {
                if bytes.is_empty() {
                    continue;
                }
                let cache_path = ffi_try!(cache_thumbnail(
                    &cache_dir,
                    "image",
                    path,
                    bytes,
                    "media_thumbnail"
                ));
                out_images.push(serde_json::json!({
                    "path": path,
                    "cache_path": cache_path,
                    "size": bytes.len(),
                }));
            }
        }
        let mut out_videos = Vec::new();
        for video in &thumbnails.videos {
            if let (Some(path), Some(bytes)) = (&video.path, &video.thumbnail) {
                if bytes.is_empty() {
                    continue;
                }
                let cache_path = ffi_try!(cache_thumbnail(
                    &cache_dir,
                    "video",
                    path,
                    bytes,
                    "media_thumbnail"
                ));
                out_videos.push(serde_json::json!({
                    "path": path,
                    "cache_path": cache_path,
                    "size": bytes.len(),
                }));
            }
        }
        let mut out_audio_albums = Vec::new();
        for album in &thumbnails.audio_albums {
            if let (Some(path), Some(bytes)) = (&album.path, &album.thumbnail) {
                if bytes.is_empty() {
                    continue;
                }
                let cache_path = ffi_try!(cache_thumbnail(
                    &cache_dir,
                    "audio",
                    path,
                    bytes,
                    "media_thumbnail"
                ));
                out_audio_albums.push(serde_json::json!({
                    "path": path,
                    "cache_path": cache_path,
                    "size": bytes.len(),
                }));
            }
        }
        ok(&serde_json::json!({
            "images": out_images,
            "videos": out_videos,
            "audio_albums": out_audio_albums,
        }))
    })
}

/// Resolve the thumbnail cache directory: `<state_dir>/thumbnails` where
/// `state_dir` is the runtime's configured value or, when unset, the core
/// default config directory (mirrors `handshaker_core::default_config_dir`,
/// i.e. `ProjectDirs::from("", "", "handshaker")`, without adding a
/// dependency). Creates the directory; failures are explicit errors.
fn thumbnail_cache_dir(runtime: &HsRuntime, operation: &str) -> Result<PathBuf, HsCallResult> {
    let base = match &runtime.app.config().state_dir {
        Some(dir) => dir.clone(),
        None => default_state_dir().ok_or_else(|| {
            err(&PublicError::new(
                PublicErrorCode::InvalidArgument,
                "state_dir is not configured",
            )
            .operation(operation))
        })?,
    };
    let dir = base.join("thumbnails");
    std::fs::create_dir_all(&dir).map_err(|error| {
        err(&PublicError::new(
            PublicErrorCode::Internal,
            "cannot create thumbnail cache dir",
        )
        .with_detail(format!("{}: {error}", dir.display()))
        .operation(operation))
    })?;
    Ok(dir)
}

/// Default state/config directory, mirroring
/// `handshaker_core::default_config_dir()` (`ProjectDirs::from("", "",
/// "handshaker")`) so caches land where the core would put them.
fn default_state_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support/handshaker"))
    }
    #[cfg(target_os = "linux")]
    {
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(xdg) => Some(PathBuf::from(xdg).join("handshaker")),
            None => {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/handshaker"))
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("handshaker"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Write one thumbnail to the cache (or reuse the existing file) and return
/// its absolute path as a string. The file name is
/// `<kind>-<fnv1a64(remote_path) hex>.thumb`; the hash is a simple stable
/// 64-bit FNV-1a (no new dependencies).
fn cache_thumbnail(
    cache_dir: &Path,
    kind: &str,
    remote_path: &str,
    bytes: &[u8],
    operation: &str,
) -> Result<String, HsCallResult> {
    let file_name = format!("{kind}-{:016x}.thumb", fnv1a64(remote_path));
    let path = cache_dir.join(file_name);
    if !path.exists() {
        std::fs::write(&path, bytes).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::Internal, "thumbnail cache write failed")
                    .with_detail(format!("{}: {error}", path.display()))
                    .operation(operation),
            )
        })?;
    }
    Ok(path.display().to_string())
}

/// FNV-1a 64-bit hash (stable across processes and platforms) used to name
/// thumbnail cache files from the remote path.
fn fnv1a64(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------
// EXIF metadata
// ---------------------------------------------------------------------------

/// `hs_media_fetch_exif` request JSON: `{"path":"/sdcard/DCIM/1.jpg"}`
/// (required). Result JSON: `ExifDataDto`.
///
/// # Safety
/// `runtime` must be a valid handle; `request_ptr`/`request_len` must
/// describe valid, readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_media_fetch_exif(
    runtime: *mut c_void,
    session_id: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> HsCallResult {
    catch("media_fetch_exif", || {
        let runtime = ffi_try!(runtime_ref(runtime, "media_fetch_exif"));
        let json = ffi_try!(input_str(request_ptr, request_len, "media_fetch_exif"));
        #[derive(Deserialize)]
        struct FfiFetchExifRequest {
            path: String,
        }
        let ffi: FfiFetchExifRequest = ffi_try!(serde_json::from_str(json).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid request JSON")
                    .with_detail(error.to_string()),
            )
        }));
        match runtime._tokio.block_on(async {
            runtime
                .app
                .fetch_exif(SessionId(session_id), &ffi.path)
                .await
        }) {
            Ok(exif) => ok(&exif),
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
    fn photo_library_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_media_photo_library(std::ptr::null_mut(), 1, b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn photo_library_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_photo_library(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn photo_library_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_photo_library(runtime, 999, b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn video_library_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_media_video_library(std::ptr::null_mut(), 1, b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn video_library_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_video_library(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn video_library_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_video_library(runtime, 999, b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn audio_library_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_media_audio_library(std::ptr::null_mut(), 1, b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn audio_library_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_audio_library(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn audio_library_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_audio_library(runtime, 999, b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn thumbnail_null_handle_returns_invalid_argument() {
        let result = unsafe { hs_media_thumbnail(std::ptr::null_mut(), 1, b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn thumbnail_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_thumbnail(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn thumbnail_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_thumbnail(runtime, 999, b"{}".as_ptr(), 2) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn fetch_exif_null_handle_returns_invalid_argument() {
        let request = br#"{"path":"/a.jpg"}"#;
        let result = unsafe {
            hs_media_fetch_exif(std::ptr::null_mut(), 1, request.as_ptr(), request.len())
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn fetch_exif_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe { hs_media_fetch_exif(runtime, 1, b"{oops".as_ptr(), 6) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn fetch_exif_missing_session_returns_session_not_found() {
        let runtime = runtime_ptr();
        let request = br#"{"path":"/a.jpg"}"#;
        let result = unsafe { hs_media_fetch_exif(runtime, 999, request.as_ptr(), request.len()) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn fnv1a64_is_stable_and_distinct() {
        // Deterministic across processes: FNV-1a reference vectors.
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64("foobar"), 0x8594_4171_f739_67e8);
        assert_ne!(fnv1a64("/a.jpg"), fnv1a64("/b.jpg"));
    }
}
