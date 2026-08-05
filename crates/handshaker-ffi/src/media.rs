//! FFI: media service (Phase E / E5) — photo/video/audio libraries,
//! on-disk thumbnail caching, and EXIF metadata.
//!
//! Thumbnail design (Phase E priority 1): bytes returned by the application
//! are written to a device/path/revision-keyed file under
//! `<state_dir>/thumbnails/`, so the host app renders local files without
//! moving image bytes through JSON. Mixed hit/miss requests only send misses
//! to the phone; corrupt entries are repaired and per-item failures remain
//! observable. `<state_dir>` is the runtime's configured value, or the core
//! default config directory when not configured; cache I/O failures are
//! surfaced as explicit errors.
//!
//! Every exported function follows the crate-wide contract: panic isolation
//! via `catch`, NULL-safe handles, and stable `InvalidArgument` errors for
//! bad input. Complex arguments/results are UTF-8 JSON buffers.

// FFI entry points are `unsafe fn` by design; every unsafe operation inside
// them is audited and documented (Safety sections).
#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::{Path, PathBuf};

use handshaker_application::{
    AudioAlbumDto, AudioLibraryDto, ImageFileDto, MediaChangeDto, PhotoLibraryDto, PublicError,
    PublicErrorCode, SessionId, VideoFileDto, VideoLibraryDto, merge_audio_library,
    merge_photo_library, merge_video_library,
};
use serde::{Deserialize, Serialize};

use crate::HsRuntime;
use crate::ffi_try;
use crate::result::{HsCallResult, catch, err, input_str, ok};
use crate::runtime_ref;

// ---------------------------------------------------------------------------
// Library snapshots (P1-9: paged, metadata-only; thumbnails via cache path)
// ---------------------------------------------------------------------------

/// P1-9: request parsing for the three library endpoints. `{}` (legacy)
/// means full snapshot; `{"limit":N,"cursor":M}` requests one page.
#[derive(Debug, Clone, Copy)]
enum MediaPageRequest {
    Full,
    Page {
        cursor: Option<u64>,
        limit: Option<usize>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaPageRequestRaw {
    limit: Option<usize>,
    cursor: Option<u64>,
}

/// Parse a library request body. `{}` or `null` → `Full`; any other valid
/// JSON object → `Page` (empty object `{}` with no fields also means
/// `Full`). `limit` may not exceed `MEDIA_PAGE_MAX_LIMIT` (the
/// application layer enforces this too, but fail fast here with the same
/// message shape).
fn parse_page_request(json: &str, operation: &str) -> Result<MediaPageRequest, PublicError> {
    let raw: MediaPageRequestRaw = serde_json::from_str(json).map_err(|error| {
        PublicError::new(
            PublicErrorCode::InvalidArgument,
            "invalid media library request",
        )
        .with_detail(error.to_string())
        .operation(operation)
    })?;
    match (raw.limit, raw.cursor) {
        (None, None) => Ok(MediaPageRequest::Full),
        (Some(limit), cursor) => {
            if limit == 0 || limit > handshaker_application::MEDIA_PAGE_MAX_LIMIT {
                return Err(PublicError::new(
                    PublicErrorCode::InvalidArgument,
                    format!(
                        "limit must be 1..={}",
                        handshaker_application::MEDIA_PAGE_MAX_LIMIT
                    ),
                )
                .operation(operation));
            }
            Ok(MediaPageRequest::Page {
                cursor,
                limit: Some(limit),
            })
        }
        (None, Some(cursor)) => Ok(MediaPageRequest::Page {
            cursor: Some(cursor),
            limit: None,
        }),
    }
}

/// `hs_media_photo_library` request JSON (P1-9): `{}` returns the full
/// metadata-only library (legacy behavior, `next_cursor` is null);
/// `{"limit":N,"cursor":M}` returns one page — `limit` defaults to 500
/// and is capped at 1000 (larger is rejected), `cursor` is the previous
/// page's `next_cursor`. Result JSON: `PhotoLibraryDto` with
/// `next_cursor` (null on the last page). Library responses never embed
/// thumbnail bytes — use `hs_media_thumbnail` (cache path) instead.
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
        let page = match parse_page_request(json, "media_photo_library") {
            Ok(page) => page,
            Err(error) => return err(&error),
        };
        match page {
            MediaPageRequest::Full => {
                match runtime
                    ._tokio
                    .block_on(async { runtime.app.get_photo_library(SessionId(session_id)).await })
                {
                    Ok(library) => ok(&library),
                    Err(error) => err(&error),
                }
            }
            MediaPageRequest::Page { cursor, limit } => {
                match runtime._tokio.block_on(async {
                    runtime
                        .app
                        .get_photo_library_page(SessionId(session_id), cursor, limit)
                        .await
                }) {
                    Ok(library) => ok(&library),
                    Err(error) => err(&error),
                }
            }
        }
    })
}

/// `hs_media_video_library` request JSON (P1-9): same as
/// `hs_media_photo_library` (`{}` = full; `{"limit":N,"cursor":M}` =
/// page). Result JSON: `VideoLibraryDto` with `next_cursor`.
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
        let page = match parse_page_request(json, "media_video_library") {
            Ok(page) => page,
            Err(error) => return err(&error),
        };
        match page {
            MediaPageRequest::Full => {
                match runtime
                    ._tokio
                    .block_on(async { runtime.app.get_video_library(SessionId(session_id)).await })
                {
                    Ok(library) => ok(&library),
                    Err(error) => err(&error),
                }
            }
            MediaPageRequest::Page { cursor, limit } => {
                match runtime._tokio.block_on(async {
                    runtime
                        .app
                        .get_video_library_page(SessionId(session_id), cursor, limit)
                        .await
                }) {
                    Ok(library) => ok(&library),
                    Err(error) => err(&error),
                }
            }
        }
    })
}

/// `hs_media_audio_library` request JSON (P1-9): same as
/// `hs_media_photo_library` (`{}` = full; `{"limit":N,"cursor":M}` =
/// page). Result JSON: `AudioLibraryDto` with `next_cursor`.
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
        let page = match parse_page_request(json, "media_audio_library") {
            Ok(page) => page,
            Err(error) => return err(&error),
        };
        match page {
            MediaPageRequest::Full => {
                match runtime
                    ._tokio
                    .block_on(async { runtime.app.get_audio_library(SessionId(session_id)).await })
                {
                    Ok(library) => ok(&library),
                    Err(error) => err(&error),
                }
            }
            MediaPageRequest::Page { cursor, limit } => {
                match runtime._tokio.block_on(async {
                    runtime
                        .app
                        .get_audio_library_page(SessionId(session_id), cursor, limit)
                        .await
                }) {
                    Ok(library) => ok(&library),
                    Err(error) => err(&error),
                }
            }
        }
    })
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
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    modified_at: Option<u64>,
    #[serde(default)]
    mini_thumb_magic: Option<String>,
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
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    modified_at: Option<u64>,
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
#[serde(deny_unknown_fields)]
struct FfiThumbnailRequest {
    #[serde(default)]
    images: Vec<FfiThumbnailImageItem>,
    #[serde(default)]
    videos: Vec<FfiThumbnailVideoItem>,
    #[serde(default)]
    audio_albums: Vec<FfiThumbnailAudioItem>,
}

#[derive(Debug, Serialize)]
struct FfiThumbnailCacheEntry {
    path: String,
    cache_path: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize)]
struct FfiThumbnailFailure {
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    album_id: Option<u64>,
    reason: &'static str,
}

#[derive(Debug, Default, Serialize)]
struct FfiThumbnailResult {
    images: Vec<FfiThumbnailCacheEntry>,
    videos: Vec<FfiThumbnailCacheEntry>,
    audio_albums: Vec<FfiThumbnailCacheEntry>,
    failed_images: Vec<FfiThumbnailFailure>,
    failed_videos: Vec<FfiThumbnailFailure>,
    failed_audio_albums: Vec<FfiThumbnailFailure>,
}

trait ThumbnailResponseItem {
    fn path(&self) -> Option<&str>;
    fn media_id(&self) -> Option<u64>;
    fn album_id(&self) -> Option<u64>;
    fn thumbnail(&self) -> Option<&[u8]>;
    fn thumbnail_error(&self) -> bool;
}

struct CachedThumbnailResponses {
    cached: Vec<FfiThumbnailCacheEntry>,
    failed: Vec<FfiThumbnailFailure>,
    responded: HashSet<String>,
}

impl ThumbnailResponseItem for ImageFileDto {
    fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
    fn media_id(&self) -> Option<u64> {
        self.media_id
    }
    fn album_id(&self) -> Option<u64> {
        self.album_id
    }
    fn thumbnail(&self) -> Option<&[u8]> {
        self.thumbnail.as_deref()
    }
    fn thumbnail_error(&self) -> bool {
        self.thumbnail_error
    }
}

impl ThumbnailResponseItem for VideoFileDto {
    fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
    fn media_id(&self) -> Option<u64> {
        self.media_id
    }
    fn album_id(&self) -> Option<u64> {
        self.album_id
    }
    fn thumbnail(&self) -> Option<&[u8]> {
        self.thumbnail.as_deref()
    }
    fn thumbnail_error(&self) -> bool {
        self.thumbnail_error
    }
}

impl ThumbnailResponseItem for AudioAlbumDto {
    fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
    fn media_id(&self) -> Option<u64> {
        None
    }
    fn album_id(&self) -> Option<u64> {
        self.album_id
    }
    fn thumbnail(&self) -> Option<&[u8]> {
        self.thumbnail.as_deref()
    }
    fn thumbnail_error(&self) -> bool {
        self.thumbnail_error
    }
}

fn cache_thumbnail_responses<T: ThumbnailResponseItem>(
    items: &[T],
    cache_dir: &Path,
    kind: &str,
    device_key: &str,
    revisions: &HashMap<String, String>,
    operation: &str,
) -> Result<CachedThumbnailResponses, HsCallResult> {
    let mut cached = Vec::new();
    let mut failed = Vec::new();
    let mut responded = HashSet::new();
    for item in items {
        let identity_id = item.media_id().or_else(|| item.album_id());
        mark_thumbnail_response(&mut responded, item.path(), identity_id);
        let failure = |reason| FfiThumbnailFailure {
            path: item.path().map(str::to_owned),
            media_id: item.media_id(),
            album_id: item.album_id(),
            reason,
        };
        if item.thumbnail_error() {
            failed.push(failure("thumbnail_error"));
            continue;
        }
        let Some(path) = item.path() else {
            failed.push(failure("missing_path"));
            continue;
        };
        let Some(bytes) = item.thumbnail().filter(|bytes| !bytes.is_empty()) else {
            failed.push(failure("missing_data"));
            continue;
        };
        let revision = revisions.get(path).map(String::as_str).unwrap_or("");
        let cache_path = cache_thumbnail(
            cache_dir, kind, device_key, path, revision, bytes, operation,
        )?;
        cached.push(FfiThumbnailCacheEntry {
            path: path.to_string(),
            cache_path,
            size: bytes.len() as u64,
        });
    }
    Ok(CachedThumbnailResponses {
        cached,
        failed,
        responded,
    })
}

/// `hs_media_thumbnail` — fetch thumbnails for the requested media entries
/// and cache them on disk. Result JSON:
/// `{"images":[{"path":"/remote/path","cache_path":"/abs/cache/file","size":N}, ...],
///   "videos":[...], "audio_albums":[...], "failed_images":[...],
///   "failed_videos":[...], "failed_audio_albums":[...]}`. Successful
/// entries carry local cache paths; failures carry stable `reason` tokens.
/// Metadata-rich requests version the cache by media id/size/modification
/// fields, while path-only legacy requests retain the original stable key.
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
        if ffi
            .images
            .iter()
            .any(|item| item.path.is_none() && item.media_id.is_none())
            || ffi
                .videos
                .iter()
                .any(|item| item.path.is_none() && item.media_id.is_none())
            || ffi
                .audio_albums
                .iter()
                .any(|item| item.path.is_none() && item.album_id.is_none())
        {
            return err(&PublicError::new(
                PublicErrorCode::InvalidArgument,
                "thumbnail entries require a path or category id",
            )
            .operation("media_thumbnail"));
        }
        // Cache key is per-device: two phones sharing the same remote path
        // must not collide. Prefer the phone UUID, then the reconciled stable
        // id, and finally the transport descriptor id. Session ids are only
        // runtime-local and therefore are not safe cache identities.
        let device_key = match runtime._tokio.block_on(async {
            runtime
                .app
                .get_session_snapshot(SessionId(session_id))
                .await
        }) {
            Ok(snapshot) => snapshot
                .device_info
                .phone_id
                .clone()
                .or_else(|| snapshot.device.stable_id.map(|id| id.0))
                .unwrap_or(snapshot.device.id.0),
            Err(error) => return err(&error),
        };
        let cache_dir = ffi_try!(thumbnail_cache_dir(runtime, "media_thumbnail"));
        let mut result = FfiThumbnailResult::default();
        let mut images = Vec::new();
        let mut videos = Vec::new();
        let mut audio_albums = Vec::new();
        let mut image_revisions = HashMap::new();
        let mut video_revisions = HashMap::new();
        let mut audio_revisions = HashMap::new();
        let mut pending_images = Vec::new();
        let mut pending_videos = Vec::new();
        let mut pending_audio = Vec::new();
        let mut scheduled_images = HashSet::new();
        let mut scheduled_videos = HashSet::new();
        let mut scheduled_audio = HashSet::new();

        // Partition every category independently. A mixed hit/miss batch now
        // sends only misses to the phone instead of re-fetching every hit.
        for item in &ffi.images {
            let revision = image_cache_revision(item);
            if let Some(path) = &item.path {
                image_revisions.insert(path.clone(), revision.clone());
                let cache_path = cache_path_for(&cache_dir, "image", &device_key, path, &revision);
                if let Some(size) = valid_cache_file_size(&cache_path) {
                    result.images.push(FfiThumbnailCacheEntry {
                        path: path.clone(),
                        cache_path: ffi_try!(cache_path_string(&cache_path, "media_thumbnail")),
                        size,
                    });
                    continue;
                }
            }
            let identity = thumbnail_identity(item.path.as_deref(), item.media_id);
            if scheduled_images.insert(identity) {
                images.push(ImageFileDto {
                    path: item.path.clone(),
                    size: item.size,
                    modified_at: item.modified_at,
                    media_id: item.media_id,
                    album_id: item.album_id,
                    mini_thumb_magic: item.mini_thumb_magic.clone(),
                    ..ImageFileDto::default()
                });
                pending_images.push(FfiThumbnailFailure {
                    path: item.path.clone(),
                    media_id: item.media_id,
                    album_id: item.album_id,
                    reason: "missing_response",
                });
            }
        }
        for item in &ffi.videos {
            let revision = video_cache_revision(item);
            if let Some(path) = &item.path {
                video_revisions.insert(path.clone(), revision.clone());
                let cache_path = cache_path_for(&cache_dir, "video", &device_key, path, &revision);
                if let Some(size) = valid_cache_file_size(&cache_path) {
                    result.videos.push(FfiThumbnailCacheEntry {
                        path: path.clone(),
                        cache_path: ffi_try!(cache_path_string(&cache_path, "media_thumbnail")),
                        size,
                    });
                    continue;
                }
            }
            let identity = thumbnail_identity(item.path.as_deref(), item.media_id);
            if scheduled_videos.insert(identity) {
                videos.push(VideoFileDto {
                    path: item.path.clone(),
                    size: item.size,
                    modified_at: item.modified_at,
                    media_id: item.media_id,
                    album_id: item.album_id,
                    ..VideoFileDto::default()
                });
                pending_videos.push(FfiThumbnailFailure {
                    path: item.path.clone(),
                    media_id: item.media_id,
                    album_id: item.album_id,
                    reason: "missing_response",
                });
            }
        }
        for item in &ffi.audio_albums {
            let revision = audio_cache_revision(item);
            if let Some(path) = &item.path {
                audio_revisions.insert(path.clone(), revision.clone());
                let cache_path = cache_path_for(&cache_dir, "audio", &device_key, path, &revision);
                if let Some(size) = valid_cache_file_size(&cache_path) {
                    result.audio_albums.push(FfiThumbnailCacheEntry {
                        path: path.clone(),
                        cache_path: ffi_try!(cache_path_string(&cache_path, "media_thumbnail")),
                        size,
                    });
                    continue;
                }
            }
            let identity = thumbnail_identity(item.path.as_deref(), item.album_id);
            if scheduled_audio.insert(identity) {
                audio_albums.push(AudioAlbumDto {
                    path: item.path.clone(),
                    album_id: item.album_id,
                    name: item.name.clone(),
                    artist_id: item.artist_id,
                    artist: item.artist.clone(),
                    year: item.year,
                    thumbnail: None,
                    thumbnail_error: false,
                });
                pending_audio.push(FfiThumbnailFailure {
                    path: item.path.clone(),
                    media_id: None,
                    album_id: item.album_id,
                    reason: "missing_response",
                });
            }
        }

        if images.is_empty() && videos.is_empty() && audio_albums.is_empty() {
            return ok(&result);
        }

        let thumbnails = match runtime._tokio.block_on(async {
            runtime
                .app
                .get_thumbnails(SessionId(session_id), &images, &videos, &audio_albums)
                .await
        }) {
            Ok(thumbnails) => thumbnails,
            Err(error) => return err(&error),
        };
        let image_responses = ffi_try!(cache_thumbnail_responses(
            &thumbnails.images,
            &cache_dir,
            "image",
            &device_key,
            &image_revisions,
            "media_thumbnail",
        ));
        let video_responses = ffi_try!(cache_thumbnail_responses(
            &thumbnails.videos,
            &cache_dir,
            "video",
            &device_key,
            &video_revisions,
            "media_thumbnail",
        ));
        let audio_responses = ffi_try!(cache_thumbnail_responses(
            &thumbnails.audio_albums,
            &cache_dir,
            "audio",
            &device_key,
            &audio_revisions,
            "media_thumbnail",
        ));
        result.images.extend(image_responses.cached);
        result.videos.extend(video_responses.cached);
        result.audio_albums.extend(audio_responses.cached);
        result.failed_images.extend(image_responses.failed);
        result.failed_videos.extend(video_responses.failed);
        result.failed_audio_albums.extend(audio_responses.failed);

        result
            .failed_images
            .extend(pending_images.into_iter().filter(|item| {
                !thumbnail_response_seen(
                    &image_responses.responded,
                    item.path.as_deref(),
                    item.media_id,
                )
            }));
        result
            .failed_videos
            .extend(pending_videos.into_iter().filter(|item| {
                !thumbnail_response_seen(
                    &video_responses.responded,
                    item.path.as_deref(),
                    item.media_id,
                )
            }));
        result
            .failed_audio_albums
            .extend(pending_audio.into_iter().filter(|item| {
                !thumbnail_response_seen(
                    &audio_responses.responded,
                    item.path.as_deref(),
                    item.album_id,
                )
            }));
        ok(&result)
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

fn thumbnail_identity(path: Option<&str>, id: Option<u64>) -> String {
    match (id, path) {
        (Some(id), _) => format!("id:{id}"),
        (None, Some(path)) => format!("path:{path}"),
        (None, None) => "missing".to_string(),
    }
}

fn mark_thumbnail_response(seen: &mut HashSet<String>, path: Option<&str>, id: Option<u64>) {
    if let Some(id) = id {
        seen.insert(format!("id:{id}"));
    }
    if let Some(path) = path {
        seen.insert(format!("path:{path}"));
    }
}

fn thumbnail_response_seen(seen: &HashSet<String>, path: Option<&str>, id: Option<u64>) -> bool {
    id.is_some_and(|id| seen.contains(&format!("id:{id}")))
        || path.is_some_and(|path| seen.contains(&format!("path:{path}")))
}

fn image_cache_revision(item: &FfiThumbnailImageItem) -> String {
    if item.media_id.is_none()
        && item.size.is_none()
        && item.modified_at.is_none()
        && item.mini_thumb_magic.is_none()
    {
        String::new()
    } else {
        format!(
            "media_id={:?};size={:?};modified_at={:?};mini_thumb_magic={:?}",
            item.media_id, item.size, item.modified_at, item.mini_thumb_magic
        )
    }
}

fn video_cache_revision(item: &FfiThumbnailVideoItem) -> String {
    if item.media_id.is_none() && item.size.is_none() && item.modified_at.is_none() {
        String::new()
    } else {
        format!(
            "media_id={:?};size={:?};modified_at={:?}",
            item.media_id, item.size, item.modified_at
        )
    }
}

fn audio_cache_revision(item: &FfiThumbnailAudioItem) -> String {
    if item.album_id.is_none()
        && item.name.is_none()
        && item.artist_id.is_none()
        && item.artist.is_none()
        && item.year.is_none()
    {
        String::new()
    } else {
        format!(
            "album_id={:?};name={:?};artist_id={:?};artist={:?};year={:?}",
            item.album_id, item.name, item.artist_id, item.artist, item.year
        )
    }
}

/// A valid cache entry is a non-empty regular file. Symlinks, directories,
/// truncated zero-byte writes and unreadable entries are treated as misses.
fn valid_cache_file_size(path: &Path) -> Option<u64> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return None;
    }
    std::fs::File::open(path).ok()?;
    Some(metadata.len())
}

fn cache_path_string(path: &Path, operation: &str) -> Result<String, HsCallResult> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        err(&PublicError::new(
            PublicErrorCode::InvalidArgument,
            "thumbnail cache path is not valid UTF-8",
        )
        .operation(operation))
    })
}

/// The on-disk cache path for one thumbnail (stable across calls). The
/// digest covers the full device/path pair and, when supplied by a metadata
/// library item, a revision fingerprint. The latter prevents a replaced
/// media file at the same remote path from reusing stale image bytes.
fn cache_path_for(
    cache_dir: &Path,
    kind: &str,
    device_key: &str,
    remote_path: &str,
    revision: &str,
) -> PathBuf {
    let device = sanitize_cache_component(device_key);
    let digest = if revision.is_empty() {
        fnv1a64(&format!("{device_key}\0{remote_path}"))
    } else {
        fnv1a64(&format!("{device_key}\0{remote_path}\0{revision}"))
    };
    cache_dir.join(format!("{device}-{kind}-{digest:016x}.thumb"))
}

/// Write one thumbnail to the cache (or reuse the existing file) and return
/// its absolute path as a string. The file name is
/// `<device_key>-<kind>-<fnv1a64(device_key + NUL + remote_path) hex>.thumb`;
/// the digest covers the full pair (a truncated device prefix alone could
/// collide across devices) and the hash is a simple stable 64-bit FNV-1a
/// (no new dependencies). Writes are atomic (temp file + rename) with a
/// unique temp suffix so concurrent callers never observe a half-written
/// cache entry or clobber each other's temp file.
fn cache_thumbnail(
    cache_dir: &Path,
    kind: &str,
    device_key: &str,
    remote_path: &str,
    revision: &str,
    bytes: &[u8],
    operation: &str,
) -> Result<String, HsCallResult> {
    let path = cache_path_for(cache_dir, kind, device_key, remote_path, revision);
    if valid_cache_file_size(&path).is_none() {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| {
                err(&PublicError::new(
                    PublicErrorCode::Internal,
                    "invalid thumbnail cache entry cannot be replaced",
                )
                .with_detail(format!("{}: {error}", path.display()))
                .operation(operation))
            })?;
        }
        // Full-nanosecond timestamp: the subsec-nanos form wraps every
        // second and could collide for same-second concurrent writers
        // (security review fix); pid + wall-clock nanos makes the temp
        // name unique in practice.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let tmp_name = format!(
            "{}.{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("thumb"),
            std::process::id(),
            nanos
        );
        let tmp = cache_dir.join(tmp_name);
        std::fs::write(&tmp, bytes).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::Internal, "thumbnail cache write failed")
                    .with_detail(format!("{}: {error}", tmp.display()))
                    .operation(operation),
            )
        })?;
        std::fs::rename(&tmp, &path).map_err(|error| {
            err(
                &PublicError::new(PublicErrorCode::Internal, "thumbnail cache commit failed")
                    .with_detail(format!("{}: {error}", path.display()))
                    .operation(operation),
            )
        })?;
    }
    cache_path_string(&path, operation)
}

/// Restrict a cache-file component to ASCII alphanumerics plus `-`/`_` so
/// a device identifier can never smuggle path separators into a file name.
fn sanitize_cache_component(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect()
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
        #[serde(deny_unknown_fields)]
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

// ---------------------------------------------------------------------------
// Incremental media-library merge (pure function, no device round-trip)
// ---------------------------------------------------------------------------

/// `hs_media_merge_change` — merge a phone-pushed `MediaChangeDto` into a
/// library snapshot and return the merged snapshot. `kind` selects the
/// library type: `"photo"` | `"video"` | `"audio"` (anything else is
/// `InvalidArgument`). `library_json` is the current library DTO
/// (`PhotoLibraryDto`/`VideoLibraryDto`/`AudioLibraryDto`); `change_json`
/// is a `MediaChangeDto`
/// (`{"media_kind":"photo","added":[...],"deleted":[...],"updated":[...]}`
/// with `MediaChangeItemDto` entries). The change's `media_kind` must match
/// `kind` (mismatch → `InvalidState`). Entries are upserted by `media_id`
/// (falling back to `path`) preserving snapshot-only fields (thumbnail,
/// starred, GPS); deleted entries are removed by the same key. This is a
/// pure data transformation and never touches the device, so it needs no
/// session. Result JSON: the merged library DTO.
///
/// # Safety
/// `runtime` must be a valid handle; `kind_ptr`/`kind_len`,
/// `library_ptr`/`library_len` and `change_ptr`/`change_len` must describe
/// valid, readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_media_merge_change(
    runtime: *mut c_void,
    kind_ptr: *const u8,
    kind_len: usize,
    library_ptr: *const u8,
    library_len: usize,
    change_ptr: *const u8,
    change_len: usize,
) -> HsCallResult {
    catch("media_merge_change", || {
        let _runtime = ffi_try!(runtime_ref(runtime, "media_merge_change"));
        let kind = ffi_try!(input_str(kind_ptr, kind_len, "media_merge_change"));
        let library_json = ffi_try!(input_str(library_ptr, library_len, "media_merge_change"));
        let change_json = ffi_try!(input_str(change_ptr, change_len, "media_merge_change"));
        let invalid_json = |error: serde_json::Error| {
            err(
                &PublicError::new(PublicErrorCode::InvalidArgument, "invalid JSON")
                    .with_detail(error.to_string())
                    .operation("media_merge_change"),
            )
        };
        let change: MediaChangeDto =
            ffi_try!(serde_json::from_str(change_json).map_err(invalid_json));
        match kind {
            "photo" => {
                let library: PhotoLibraryDto =
                    ffi_try!(serde_json::from_str(library_json).map_err(invalid_json));
                match merge_photo_library(&library, &change) {
                    Ok(merged) => ok(&merged),
                    Err(error) => err(&error),
                }
            }
            "video" => {
                let library: VideoLibraryDto =
                    ffi_try!(serde_json::from_str(library_json).map_err(invalid_json));
                match merge_video_library(&library, &change) {
                    Ok(merged) => ok(&merged),
                    Err(error) => err(&error),
                }
            }
            "audio" => {
                let library: AudioLibraryDto =
                    ffi_try!(serde_json::from_str(library_json).map_err(invalid_json));
                match merge_audio_library(&library, &change) {
                    Ok(merged) => ok(&merged),
                    Err(error) => err(&error),
                }
            }
            _ => err(&PublicError::new(
                PublicErrorCode::InvalidArgument,
                format!("invalid media kind \"{kind}\""),
            )
            .operation("media_merge_change")),
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
    fn page_request_rejects_limit_out_of_range() {
        let runtime = runtime_ptr();
        // limit 0
        let result = unsafe { hs_media_photo_library(runtime, 1, br#"{"limit":0}"#.as_ptr(), 11) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        // limit above the 1000 cap
        let result =
            unsafe { hs_media_photo_library(runtime, 1, br#"{"limit":1001}"#.as_ptr(), 14) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        // limit exactly at the cap is a valid request shape (fails on the
        // session lookup, not on validation)
        let result =
            unsafe { hs_media_photo_library(runtime, 1, br#"{"limit":1000}"#.as_ptr(), 14) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn page_request_cursor_is_forwarded() {
        let runtime = runtime_ptr();
        // cursor-only and full-page shapes pass validation and reach the
        // session lookup (session 999 does not exist).
        let result =
            unsafe { hs_media_photo_library(runtime, 999, br#"{"cursor":5}"#.as_ptr(), 12) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        let result = unsafe {
            hs_media_photo_library(runtime, 999, br#"{"limit":50,"cursor":5}"#.as_ptr(), 23)
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "session_not_found");
        // cursor must be an integer media id, not an opaque string.
        let result =
            unsafe { hs_media_photo_library(runtime, 999, br#"{"cursor":"abc"}"#.as_ptr(), 16) };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
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
    fn thumbnail_entry_requires_path_or_id() {
        let runtime = runtime_ptr();
        let request = br#"{"images":[{}]}"#;
        let result = unsafe { hs_media_thumbnail(runtime, 1, request.as_ptr(), request.len()) };
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
    fn media_merge_change_null_handle_returns_invalid_argument() {
        let result = unsafe {
            hs_media_merge_change(
                std::ptr::null_mut(),
                b"photo".as_ptr(),
                5,
                b"{}".as_ptr(),
                2,
                b"{}".as_ptr(),
                2,
            )
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
    }

    #[test]
    fn media_merge_change_bad_kind_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe {
            hs_media_merge_change(
                runtime,
                b"documents".as_ptr(),
                10,
                b"{}".as_ptr(),
                2,
                b"{}".as_ptr(),
                2,
            )
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn media_merge_change_bad_json_is_rejected() {
        let runtime = runtime_ptr();
        let result = unsafe {
            hs_media_merge_change(
                runtime,
                b"photo".as_ptr(),
                5,
                b"{oops".as_ptr(),
                6,
                b"{}".as_ptr(),
                2,
            )
        };
        assert_eq!(result.status, 1);
        assert_eq!(error_code_of(result), "invalid_argument");
        unsafe { hs_runtime_destroy(runtime) };
    }

    #[test]
    fn media_merge_change_photo_merges_without_device() {
        // Pure function: no session, no device — a minimal library plus a
        // change must produce the merged snapshot JSON.
        let runtime = runtime_ptr();
        let library = br#"{"images":[{"path":"/a.jpg","size":100,"starred":true,"thumbnail_error":false}],"albums":[],"camera_album_id":null}"#;
        let change = br#"{"media_kind":"photo","added":[{"path":"/b.jpg","size":2048}],"deleted":[],"updated":[{"path":"/a.jpg","size":4096}]}"#;
        let result = unsafe {
            hs_media_merge_change(
                runtime,
                b"photo".as_ptr(),
                5,
                library.as_ptr(),
                library.len(),
                change.as_ptr(),
                change.len(),
            )
        };
        assert_eq!(result.status, 0, "merge must succeed without a device");
        let bytes = unsafe { crate::buffer::into_vec(result.value) };
        let merged: serde_json::Value = serde_json::from_slice(&bytes).expect("merged json");
        unsafe { crate::result::free_result(HsCallResult::default()) };
        let images = merged["images"].as_array().expect("images array");
        assert_eq!(images.len(), 2, "one updated in place, one added");
        let updated = images
            .iter()
            .find(|image| image["path"] == "/a.jpg")
            .expect("updated entry");
        assert_eq!(updated["size"], 4096, "overlap field overwritten");
        assert_eq!(updated["starred"], true, "snapshot-only field preserved");
        assert!(
            images.iter().any(|image| image["path"] == "/b.jpg"),
            "added entry present"
        );
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

    #[test]
    fn thumbnail_cache_round_trip_reuses_file() {
        // Review follow-up: the cache logic deserves direct tests even though
        // the FFI symbol itself needs a session/device.
        let dir = std::env::temp_dir().join(format!(
            "hs-thumb-cache-test-{}-{dir_nanos}",
            std::process::id(),
            dir_nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let path1 = cache_thumbnail(&dir, "image", "phone:abc", "/a.jpg", "", b"bytes-1", "test")
            .expect("first write");
        let path2 = cache_thumbnail(&dir, "image", "phone:abc", "/a.jpg", "", b"bytes-2", "test")
            .expect("reuse");
        // Same stable cache path; the second call must reuse the existing
        // file (its content stays the first write) — no duplicate, no
        // clobbering.
        assert_eq!(path1, path2);
        assert_eq!(std::fs::read(&path1).unwrap(), b"bytes-1");
        assert_eq!(std::fs::read(&path2).unwrap(), b"bytes-1");

        // Distinct devices with the same remote path get distinct entries.
        let other = cache_thumbnail(&dir, "image", "phone:xyz", "/a.jpg", "", b"other", "test")
            .expect("other device");
        assert_ne!(path1, other);
        assert_eq!(std::fs::read(&other).unwrap(), b"other");

        // A metadata revision change must not serve stale bytes for a file
        // replaced at the same remote path.
        let revised = cache_thumbnail(
            &dir,
            "image",
            "phone:abc",
            "/a.jpg",
            "modified_at=2;size=99",
            b"revised",
            "test",
        )
        .expect("revised entry");
        assert_ne!(path1, revised);
        assert_eq!(std::fs::read(&revised).unwrap(), b"revised");

        // A zero-byte/truncated cache entry is a miss and is atomically
        // replaced instead of being returned forever as a successful hit.
        let corrupt = cache_path_for(&dir, "video", "phone:abc", "/bad.mp4", "");
        std::fs::write(&corrupt, []).unwrap();
        assert_eq!(valid_cache_file_size(&corrupt), None);
        let repaired =
            cache_thumbnail(&dir, "video", "phone:abc", "/bad.mp4", "", b"fixed", "test")
                .expect("repair corrupt entry");
        assert_eq!(std::fs::read(repaired).unwrap(), b"fixed");

        // Sanitized device keys can never inject path separators: dots and
        // slashes are both stripped, so no ".." escape survives.
        let evil = sanitize_cache_component("../../etc/passwd");
        assert_eq!(evil, "etcpasswd");
        assert!(!evil.contains('/'));
        assert!(!evil.contains('.'));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thumbnail_responses_cache_success_and_surface_item_failures() {
        let dir = std::env::temp_dir().join(format!(
            "hs-thumb-response-test-{}-{nanos}",
            std::process::id(),
            nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let items = vec![
            ImageFileDto {
                path: Some("/ok.jpg".into()),
                media_id: Some(1),
                thumbnail: Some(vec![0xff, 0xd8, 0xff]),
                ..ImageFileDto::default()
            },
            ImageFileDto {
                path: Some("/phone-error.jpg".into()),
                media_id: Some(2),
                thumbnail_error: true,
                ..ImageFileDto::default()
            },
            ImageFileDto {
                path: Some("/missing.jpg".into()),
                media_id: Some(3),
                ..ImageFileDto::default()
            },
        ];
        let revisions = HashMap::from([("/ok.jpg".to_string(), "revision=1".to_string())]);
        let output =
            cache_thumbnail_responses(&items, &dir, "image", "phone:abc", &revisions, "test")
                .expect("cache thumbnail responses");

        assert_eq!(output.cached.len(), 1);
        assert!(std::path::Path::new(&output.cached[0].cache_path).is_file());
        assert_eq!(
            output
                .failed
                .iter()
                .map(|failure| failure.reason)
                .collect::<Vec<_>>(),
            ["thumbnail_error", "missing_data"]
        );
        assert!(output.responded.contains("id:1"));
        assert!(output.responded.contains("path:/ok.jpg"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
