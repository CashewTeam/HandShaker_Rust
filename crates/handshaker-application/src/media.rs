//! Media library DTOs (mirror `handshaker-core` media domain types with the
//! same field names so the CLI JSON contract is preserved exactly).
//!
//! Freeze contract: field names/serde shape mirror the core types; changes
//! here are part of the application v1 contract.

use serde::{Deserialize, Serialize};

use handshaker_core::{
    AudioAlbum, AudioFile, AudioLibrary, ExifData, ImageAlbum, ImageFile, MediaItem, MediaKind,
    MediaLibraryChange, PhotoLibrary, Thumbnails, VideoAlbum, VideoFile, VideoLibrary,
    media_merge::{apply_audio, apply_photo, apply_video},
};

use crate::dto::{MediaChangeDto, MediaChangeItemDto, MediaKindDto};
use crate::error::{AppResult, PublicError, PublicErrorCode, from_core_error};

/// One image entry (mirrors core `ImageFile`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ImageFileDto {
    pub path: Option<String>,
    pub size: Option<u64>,
    pub created_at: Option<u64>,
    pub modified_at: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u32>,
    pub media_id: Option<u64>,
    pub album_id: Option<u64>,
    pub mime_type: Option<String>,
    pub thumbnail: Option<Vec<u8>>,
    pub album_name: Option<String>,
    pub date_taken: Option<u64>,
    pub latitude: Option<String>,
    pub longitude: Option<String>,
    pub mini_thumb_magic: Option<String>,
    pub title: Option<String>,
    pub thumbnail_error: bool,
    pub starred: bool,
}

/// One photo album (mirrors core `ImageAlbum`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageAlbumDto {
    pub path: Option<String>,
    pub album_id: Option<u64>,
    pub name: Option<String>,
    pub cover_image: Option<Box<ImageFileDto>>,
}

/// One video entry (mirrors core `VideoFile`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VideoFileDto {
    pub path: Option<String>,
    pub size: Option<u64>,
    pub created_at: Option<u64>,
    pub modified_at: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u32>,
    pub media_id: Option<u64>,
    pub album_id: Option<u64>,
    pub mime_type: Option<String>,
    pub thumbnail: Option<Vec<u8>>,
    pub thumbnail_error: bool,
    pub duration: Option<f64>,
}

/// One video album (mirrors core `VideoAlbum`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoAlbumDto {
    pub path: Option<String>,
    pub album_id: Option<u64>,
    pub name: Option<String>,
}

/// One audio track (mirrors core `AudioFile`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AudioFileDto {
    pub path: Option<String>,
    pub size: Option<u64>,
    pub created_at: Option<u64>,
    pub modified_at: Option<u64>,
    pub media_id: Option<u64>,
    pub album_id: Option<u64>,
    pub title: Option<String>,
    pub mime_type: Option<String>,
    pub artist_id: Option<u64>,
    pub artist: Option<String>,
    pub composer: Option<String>,
    pub genre: Option<u32>,
    pub comment: Option<String>,
    pub copyright: Option<String>,
    pub audio_codec: Option<String>,
    pub track: Option<u32>,
    pub duration: Option<f64>,
}

/// One audio album (mirrors core `AudioAlbum`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioAlbumDto {
    pub path: Option<String>,
    pub album_id: Option<u64>,
    pub name: Option<String>,
    pub artist_id: Option<u64>,
    pub artist: Option<String>,
    pub year: Option<u32>,
    pub thumbnail: Option<Vec<u8>>,
    pub thumbnail_error: bool,
}

/// Photo library snapshot (mirrors core `PhotoLibrary`).
///
/// P1-9: library *list* responses are metadata-only (`thumbnail` is
/// always `None` — thumbnails come from `get_thumbnails`/the cache-path
/// endpoint); `next_cursor` is set by the paged variants (non-null while
/// more pages exist).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PhotoLibraryDto {
    pub images: Vec<ImageFileDto>,
    pub albums: Vec<ImageAlbumDto>,
    pub camera_album_id: Option<u64>,
    #[serde(default)]
    pub next_cursor: Option<u64>,
}

/// Video library snapshot (mirrors core `VideoLibrary`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VideoLibraryDto {
    pub videos: Vec<VideoFileDto>,
    pub albums: Vec<VideoAlbumDto>,
    #[serde(default)]
    pub next_cursor: Option<u64>,
}

/// Audio library snapshot (mirrors core `AudioLibrary`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AudioLibraryDto {
    pub tracks: Vec<AudioFileDto>,
    pub albums: Vec<AudioAlbumDto>,
    #[serde(default)]
    pub next_cursor: Option<u64>,
}

/// P1-9: default page size and hard cap for paged media-library list
/// responses. Requests above the cap are rejected, not silently clamped.
pub const MEDIA_PAGE_DEFAULT_LIMIT: usize = 500;
pub const MEDIA_PAGE_MAX_LIMIT: usize = 1000;

/// Slice a fully-sorted media list into one page. Items are ordered by
/// media_id (missing ids sort first) so the cursor is stable across
/// refreshes; `cursor` is the last-visible media_id of the previous page.
/// Returns (page, next_cursor) — `next_cursor` is `None` on the last page.
///
/// Defensive properties (P1-9 review):
/// - `limit == 0` returns an empty page instead of panicking;
/// - a trailing item without a media_id does not truncate pagination:
///   `next_cursor` falls back to the last id-bearing item on the page
///   (a page whose items all lack ids returns `None` — such a library
///   cannot be keyed, which callers should treat as one-shot);
/// - keyset semantics assume ids are unique within a page; duplicate
///   ids would skip same-value boundary items (documented precondition).
pub fn slice_page<T: Clone>(
    mut items: Vec<T>,
    cursor: Option<u64>,
    limit: usize,
    media_id: impl Fn(&T) -> Option<u64>,
) -> (Vec<T>, Option<u64>) {
    if limit == 0 {
        return (Vec::new(), None);
    }
    items.sort_by_key(|item| media_id(item).unwrap_or(0));
    let start = match cursor {
        Some(cursor) => items
            .iter()
            .position(|item| media_id(item).unwrap_or(0) > cursor)
            .unwrap_or(items.len()),
        None => 0,
    };
    let end = (start + limit).min(items.len());
    let page = items[start..end].to_vec();
    let next = if end < items.len() {
        // Last-visible id, walking back over trailing id-less items so a
        // None tail cannot fake "end of library".
        page.iter()
            .rev()
            .find_map(&media_id)
            .or_else(|| items[end..].iter().find_map(&media_id))
    } else {
        None
    };
    (page, next)
}

/// P1-9: media-library list responses are metadata-only — thumbnail byte
/// arrays belong to the dedicated `get_thumbnails` interface only.
pub fn strip_photo_thumbnails(library: &mut PhotoLibraryDto) {
    for image in &mut library.images {
        image.thumbnail = None;
        image.thumbnail_error = false;
    }
    // Album covers carry a full ImageFileDto — strip it too, otherwise
    // thumbnail bytes still leak into the list response.
    for album in &mut library.albums {
        if let Some(cover) = album.cover_image.as_mut() {
            cover.thumbnail = None;
            cover.thumbnail_error = false;
        }
    }
}

pub fn strip_video_thumbnails(library: &mut VideoLibraryDto) {
    for video in &mut library.videos {
        video.thumbnail = None;
        video.thumbnail_error = false;
    }
}

pub fn strip_audio_thumbnails(library: &mut AudioLibraryDto) {
    for album in &mut library.albums {
        album.thumbnail = None;
        album.thumbnail_error = false;
    }
}

/// Thumbnail responses keyed by media category (mirrors core `Thumbnails`).
/// This is the only interface that carries thumbnail byte arrays; library
/// list responses are metadata-only (P1-9).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ThumbnailsDto {
    pub images: Vec<ImageFileDto>,
    pub videos: Vec<VideoFileDto>,
    pub audio_albums: Vec<AudioAlbumDto>,
}

/// Exif metadata (mirrors core `ExifData`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ExifDataDto {
    pub orientation: Option<u32>,
    pub date_taken: Option<u64>,
    pub latitude: Option<String>,
    pub longitude: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length: Option<f64>,
    pub exposure_time: Option<f64>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
}

// ---- conversions from core types ----

impl From<ImageFile> for ImageFileDto {
    fn from(file: ImageFile) -> Self {
        Self {
            path: file.path,
            size: file.size,
            created_at: file.created_at,
            modified_at: file.modified_at,
            width: file.width,
            height: file.height,
            orientation: file.orientation,
            media_id: file.media_id,
            album_id: file.album_id,
            mime_type: file.mime_type,
            thumbnail: file.thumbnail,
            album_name: file.album_name,
            date_taken: file.date_taken,
            latitude: file.latitude,
            longitude: file.longitude,
            mini_thumb_magic: file.mini_thumb_magic,
            title: file.title,
            thumbnail_error: file.thumbnail_error,
            starred: file.starred,
        }
    }
}

impl From<ImageAlbum> for ImageAlbumDto {
    fn from(album: ImageAlbum) -> Self {
        Self {
            path: album.path,
            album_id: album.album_id,
            name: album.name,
            cover_image: album.cover_image.map(|cover| Box::new((*cover).into())),
        }
    }
}

impl From<VideoFile> for VideoFileDto {
    fn from(file: VideoFile) -> Self {
        Self {
            path: file.path,
            size: file.size,
            created_at: file.created_at,
            modified_at: file.modified_at,
            width: file.width,
            height: file.height,
            orientation: file.orientation,
            media_id: file.media_id,
            album_id: file.album_id,
            mime_type: file.mime_type,
            thumbnail: file.thumbnail,
            thumbnail_error: file.thumbnail_error,
            duration: file.duration,
        }
    }
}

impl From<VideoAlbum> for VideoAlbumDto {
    fn from(album: VideoAlbum) -> Self {
        Self {
            path: album.path,
            album_id: album.album_id,
            name: album.name,
        }
    }
}

impl From<AudioFile> for AudioFileDto {
    fn from(file: AudioFile) -> Self {
        Self {
            path: file.path,
            size: file.size,
            created_at: file.created_at,
            modified_at: file.modified_at,
            media_id: file.media_id,
            album_id: file.album_id,
            title: file.title,
            mime_type: file.mime_type,
            artist_id: file.artist_id,
            artist: file.artist,
            composer: file.composer,
            genre: file.genre,
            comment: file.comment,
            copyright: file.copyright,
            audio_codec: file.audio_codec,
            track: file.track,
            duration: file.duration,
        }
    }
}

impl From<AudioAlbum> for AudioAlbumDto {
    fn from(album: AudioAlbum) -> Self {
        Self {
            path: album.path,
            album_id: album.album_id,
            name: album.name,
            artist_id: album.artist_id,
            artist: album.artist,
            year: album.year,
            thumbnail: album.thumbnail,
            thumbnail_error: album.thumbnail_error,
        }
    }
}

impl From<PhotoLibrary> for PhotoLibraryDto {
    fn from(library: PhotoLibrary) -> Self {
        Self {
            images: library.images.into_iter().map(Into::into).collect(),
            albums: library.albums.into_iter().map(Into::into).collect(),
            camera_album_id: library.camera_album_id,
            next_cursor: None,
        }
    }
}

impl From<VideoLibrary> for VideoLibraryDto {
    fn from(library: VideoLibrary) -> Self {
        Self {
            videos: library.videos.into_iter().map(Into::into).collect(),
            albums: library.albums.into_iter().map(Into::into).collect(),
            next_cursor: None,
        }
    }
}

impl From<AudioLibrary> for AudioLibraryDto {
    fn from(library: AudioLibrary) -> Self {
        Self {
            tracks: library.tracks.into_iter().map(Into::into).collect(),
            albums: library.albums.into_iter().map(Into::into).collect(),
            next_cursor: None,
        }
    }
}

impl From<Thumbnails> for ThumbnailsDto {
    fn from(thumbnails: Thumbnails) -> Self {
        Self {
            images: thumbnails.images.into_iter().map(Into::into).collect(),
            videos: thumbnails.videos.into_iter().map(Into::into).collect(),
            audio_albums: thumbnails
                .audio_albums
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl From<ExifData> for ExifDataDto {
    fn from(data: ExifData) -> Self {
        Self {
            orientation: data.orientation,
            date_taken: data.date_taken,
            latitude: data.latitude,
            longitude: data.longitude,
            make: data.make,
            model: data.model,
            software: data.software,
            lens_model: data.lens_model,
            focal_length: data.focal_length,
            exposure_time: data.exposure_time,
            f_number: data.f_number,
            iso: data.iso,
        }
    }
}

// ---- reverse conversions (DTO -> core; used by runtime request mapping) ----

/// Rebuild a core `ImageFile` from its DTO (thumbnail-request shape: only
/// `media_id`/`path` are significant, but every field is carried through).
pub fn dto_to_image_file(file: &ImageFileDto) -> ImageFile {
    ImageFile {
        path: file.path.clone(),
        size: file.size,
        created_at: file.created_at,
        modified_at: file.modified_at,
        width: file.width,
        height: file.height,
        orientation: file.orientation,
        media_id: file.media_id,
        album_id: file.album_id,
        mime_type: file.mime_type.clone(),
        thumbnail: file.thumbnail.clone(),
        album_name: file.album_name.clone(),
        date_taken: file.date_taken,
        latitude: file.latitude.clone(),
        longitude: file.longitude.clone(),
        mini_thumb_magic: file.mini_thumb_magic.clone(),
        title: file.title.clone(),
        thumbnail_error: file.thumbnail_error,
        starred: file.starred,
    }
}

/// Rebuild a core `VideoFile` from its DTO.
pub fn dto_to_video_file(file: &VideoFileDto) -> VideoFile {
    VideoFile {
        path: file.path.clone(),
        size: file.size,
        created_at: file.created_at,
        modified_at: file.modified_at,
        width: file.width,
        height: file.height,
        orientation: file.orientation,
        media_id: file.media_id,
        album_id: file.album_id,
        mime_type: file.mime_type.clone(),
        thumbnail: file.thumbnail.clone(),
        thumbnail_error: file.thumbnail_error,
        duration: file.duration,
    }
}

/// Rebuild a core `AudioAlbum` from its DTO.
pub fn dto_to_audio_album(album: &AudioAlbumDto) -> AudioAlbum {
    AudioAlbum {
        path: album.path.clone(),
        album_id: album.album_id,
        name: album.name.clone(),
        artist_id: album.artist_id,
        artist: album.artist.clone(),
        year: album.year,
        thumbnail: album.thumbnail.clone(),
        thumbnail_error: album.thumbnail_error,
    }
}

/// Rebuild a core `ImageAlbum` from its DTO.
pub fn dto_to_image_album(album: &ImageAlbumDto) -> ImageAlbum {
    ImageAlbum {
        path: album.path.clone(),
        album_id: album.album_id,
        name: album.name.clone(),
        cover_image: album
            .cover_image
            .as_ref()
            .map(|cover| Box::new(dto_to_image_file(cover))),
    }
}

/// Rebuild a core `VideoAlbum` from its DTO.
pub fn dto_to_video_album(album: &VideoAlbumDto) -> VideoAlbum {
    VideoAlbum {
        path: album.path.clone(),
        album_id: album.album_id,
        name: album.name.clone(),
    }
}

/// Rebuild a core `AudioFile` from its DTO.
pub fn dto_to_audio_file(file: &AudioFileDto) -> AudioFile {
    AudioFile {
        path: file.path.clone(),
        size: file.size,
        created_at: file.created_at,
        modified_at: file.modified_at,
        media_id: file.media_id,
        album_id: file.album_id,
        title: file.title.clone(),
        mime_type: file.mime_type.clone(),
        artist_id: file.artist_id,
        artist: file.artist.clone(),
        composer: file.composer.clone(),
        genre: file.genre,
        comment: file.comment.clone(),
        copyright: file.copyright.clone(),
        audio_codec: file.audio_codec.clone(),
        track: file.track,
        duration: file.duration,
    }
}

/// Rebuild a core `PhotoLibrary` from its DTO (full field carry-over, so
/// snapshot-only fields like thumbnails and starred state survive a merge).
pub fn dto_to_photo_library(library: &PhotoLibraryDto) -> PhotoLibrary {
    PhotoLibrary {
        images: library.images.iter().map(dto_to_image_file).collect(),
        albums: library.albums.iter().map(dto_to_image_album).collect(),
        camera_album_id: library.camera_album_id,
    }
}

/// Rebuild a core `VideoLibrary` from its DTO.
pub fn dto_to_video_library(library: &VideoLibraryDto) -> VideoLibrary {
    VideoLibrary {
        videos: library.videos.iter().map(dto_to_video_file).collect(),
        albums: library.albums.iter().map(dto_to_video_album).collect(),
    }
}

/// Rebuild a core `AudioLibrary` from its DTO.
pub fn dto_to_audio_library(library: &AudioLibraryDto) -> AudioLibrary {
    AudioLibrary {
        tracks: library.tracks.iter().map(dto_to_audio_file).collect(),
        albums: library.albums.iter().map(dto_to_audio_album).collect(),
    }
}

// ---------------------------------------------------------------------------
// Incremental media-library merging (mirrors `handshaker_core::media_merge`)
// ---------------------------------------------------------------------------

/// Map the DTO media category to the core category.
fn kind_to_core(kind: MediaKindDto) -> MediaKind {
    match kind {
        MediaKindDto::Photo => MediaKind::Photo,
        MediaKindDto::Video => MediaKind::Video,
        MediaKindDto::Audio => MediaKind::Audio,
    }
}

/// Rebuild a core `MediaItem` from its DTO (the DTO carries the stable
/// subset the phone reports on the event channel; the rest stays `None`).
fn item_to_core(item: &MediaChangeItemDto) -> MediaItem {
    MediaItem {
        media_id: item.media_id,
        path: item.path.clone(),
        size: item.size,
        created_at: item.created_at,
        modified_at: item.modified_at,
        mime_type: item.mime_type.clone(),
        title: item.title.clone(),
        album_name: item.album_name.clone(),
        ..MediaItem::default()
    }
}

/// Rebuild a core `MediaLibraryChange` from its DTO (album payloads are
/// intentionally not bridged yet).
fn change_to_core(change: &MediaChangeDto) -> MediaLibraryChange {
    MediaLibraryChange {
        kind: kind_to_core(change.media_kind),
        added: change.added.iter().map(item_to_core).collect(),
        deleted: change.deleted.iter().map(item_to_core).collect(),
        updated: change.updated.iter().map(item_to_core).collect(),
        albums: Vec::new(),
    }
}

/// Reject a change whose category does not match the library being merged
/// (e.g. a Photo event applied to a video library).
fn ensure_kind_matches(actual: MediaKindDto, expected: MediaKindDto) -> AppResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(PublicError::new(
            PublicErrorCode::InvalidState,
            "media kind mismatch",
        ))
    }
}

/// Merge a media-library change into a photo-library snapshot. Entries in
/// `added`/`updated` are upserted by `media_id` (falling back to `path`),
/// overlapping fields are overwritten while snapshot-only fields
/// (thumbnail, starred, GPS, ...) are preserved; `deleted` entries are
/// removed by the same key. Pure data transformation — no device I/O.
/// Returns the merged snapshot.
pub fn merge_photo_library(
    library: &PhotoLibraryDto,
    change: &MediaChangeDto,
) -> AppResult<PhotoLibraryDto> {
    ensure_kind_matches(change.media_kind, MediaKindDto::Photo)?;
    let mut core = dto_to_photo_library(library);
    apply_photo(&mut core, &change_to_core(change))
        .map_err(|error| from_core_error(error, "merge_photo_library"))?;
    Ok(core.into())
}

/// Merge a media-library change into a video-library snapshot (same
/// upsert/preserve/remove semantics as `merge_photo_library`). Pure data
/// transformation — no device I/O. Returns the merged snapshot.
pub fn merge_video_library(
    library: &VideoLibraryDto,
    change: &MediaChangeDto,
) -> AppResult<VideoLibraryDto> {
    ensure_kind_matches(change.media_kind, MediaKindDto::Video)?;
    let mut core = dto_to_video_library(library);
    apply_video(&mut core, &change_to_core(change))
        .map_err(|error| from_core_error(error, "merge_video_library"))?;
    Ok(core.into())
}

/// Merge a media-library change into an audio-library snapshot (same
/// upsert/preserve/remove semantics as `merge_photo_library`). Pure data
/// transformation — no device I/O. Returns the merged snapshot.
pub fn merge_audio_library(
    library: &AudioLibraryDto,
    change: &MediaChangeDto,
) -> AppResult<AudioLibraryDto> {
    ensure_kind_matches(change.media_kind, MediaKindDto::Audio)?;
    let mut core = dto_to_audio_library(library);
    apply_audio(&mut core, &change_to_core(change))
        .map_err(|error| from_core_error(error, "merge_audio_library"))?;
    Ok(core.into())
}
