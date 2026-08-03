//! Media library DTOs (mirror `handshaker-core` media domain types with the
//! same field names so the CLI JSON contract is preserved exactly).
//!
//! Freeze contract: field names/serde shape mirror the core types; changes
//! here are part of the application v1 contract.

use serde::{Deserialize, Serialize};

use handshaker_core::{
    AudioAlbum, AudioFile, AudioLibrary, ExifData, ImageAlbum, ImageFile, PhotoLibrary, Thumbnails,
    VideoAlbum, VideoFile, VideoLibrary,
};

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PhotoLibraryDto {
    pub images: Vec<ImageFileDto>,
    pub albums: Vec<ImageAlbumDto>,
    pub camera_album_id: Option<u64>,
}

/// Video library snapshot (mirrors core `VideoLibrary`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VideoLibraryDto {
    pub videos: Vec<VideoFileDto>,
    pub albums: Vec<VideoAlbumDto>,
}

/// Audio library snapshot (mirrors core `AudioLibrary`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AudioLibraryDto {
    pub tracks: Vec<AudioFileDto>,
    pub albums: Vec<AudioAlbumDto>,
}

/// Thumbnail responses keyed by media category (mirrors core `Thumbnails`).
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
        }
    }
}

impl From<VideoLibrary> for VideoLibraryDto {
    fn from(library: VideoLibrary) -> Self {
        Self {
            videos: library.videos.into_iter().map(Into::into).collect(),
            albums: library.albums.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<AudioLibrary> for AudioLibraryDto {
    fn from(library: AudioLibrary) -> Self {
        Self {
            tracks: library.tracks.into_iter().map(Into::into).collect(),
            albums: library.albums.into_iter().map(Into::into).collect(),
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
