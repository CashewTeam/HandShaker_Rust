//! Incremental media-library merging.
//!
//! `watch` receives `MediaLibraryChange` events whose entries only carry the
//! fields the phone reports on the event channel. Applying a change to a
//! library snapshot therefore *upserts* the overlapping fields by key
//! (`media_id`, falling back to `path`) while preserving snapshot-only fields
//! such as thumbnails, GPS and starred state. Deleted entries are removed by
//! the same key. Events and queries stay decoupled: this module is pure data
//! transformation, and callers keep their own snapshots.

use crate::domain::{AudioFile, AudioLibrary, ImageFile, PhotoLibrary, VideoFile, VideoLibrary};
use crate::error::{Error, Result};
use crate::events::{MediaItem, MediaKind, MediaLibraryChange};
use crate::i18n;

/// Stable key used to match event entries against library entries.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MediaKey {
    Id(u64),
    Path(String),
}

fn item_key(item: &MediaItem) -> Option<MediaKey> {
    if let Some(id) = item.media_id {
        return Some(MediaKey::Id(id));
    }
    item.path.clone().map(MediaKey::Path)
}

fn image_matches(image: &ImageFile, key: &MediaKey) -> bool {
    match key {
        MediaKey::Id(id) => image.media_id == Some(*id),
        MediaKey::Path(path) => image.path.as_deref() == Some(path.as_str()),
    }
}

fn video_matches(video: &VideoFile, key: &MediaKey) -> bool {
    match key {
        MediaKey::Id(id) => video.media_id == Some(*id),
        MediaKey::Path(path) => video.path.as_deref() == Some(path.as_str()),
    }
}

fn audio_matches(audio: &AudioFile, key: &MediaKey) -> bool {
    match key {
        MediaKey::Id(id) => audio.media_id == Some(*id),
        MediaKey::Path(path) => audio.path.as_deref() == Some(path.as_str()),
    }
}

fn ensure_kind(library_kind: MediaKind, change_kind: MediaKind) -> Result<()> {
    if library_kind != change_kind {
        return Err(Error::Protocol(
            i18n::format(
                "media.change_kind_mismatch",
                &[&format!("{change_kind:?}"), &format!("{library_kind:?}")],
            )
            .to_string(),
        ));
    }
    Ok(())
}

/// Apply a photo-library change to a snapshot.
pub fn apply_photo(library: &mut PhotoLibrary, change: &MediaLibraryChange) -> Result<()> {
    ensure_kind(MediaKind::Photo, change.kind)?;
    for item in &change.added {
        upsert_photo(library, item);
    }
    for item in &change.updated {
        upsert_photo(library, item);
    }
    for item in &change.deleted {
        remove_photo(library, item);
    }
    Ok(())
}

/// Apply a video-library change to a snapshot.
pub fn apply_video(library: &mut VideoLibrary, change: &MediaLibraryChange) -> Result<()> {
    ensure_kind(MediaKind::Video, change.kind)?;
    for item in &change.added {
        upsert_video(library, item);
    }
    for item in &change.updated {
        upsert_video(library, item);
    }
    for item in &change.deleted {
        remove_video(library, item);
    }
    Ok(())
}

/// Apply an audio-library change to a snapshot.
pub fn apply_audio(library: &mut AudioLibrary, change: &MediaLibraryChange) -> Result<()> {
    ensure_kind(MediaKind::Audio, change.kind)?;
    for item in &change.added {
        upsert_audio(library, item);
    }
    for item in &change.updated {
        upsert_audio(library, item);
    }
    for item in &change.deleted {
        remove_audio(library, item);
    }
    Ok(())
}

fn upsert_photo(library: &mut PhotoLibrary, item: &MediaItem) {
    if let Some(key) = item_key(item) {
        if let Some(existing) = library
            .images
            .iter_mut()
            .find(|image| image_matches(image, &key))
        {
            apply_image_overlap(existing, item);
            return;
        }
    }
    library.images.push(image_from_item(item));
}

fn apply_image_overlap(image: &mut ImageFile, item: &MediaItem) {
    if item.media_id.is_some() {
        image.media_id = item.media_id;
    }
    if item.album_id.is_some() {
        image.album_id = item.album_id;
    }
    if item.path.is_some() {
        image.path = item.path.clone();
    }
    if item.size.is_some() {
        image.size = item.size;
    }
    if item.created_at.is_some() {
        image.created_at = item.created_at;
    }
    if item.modified_at.is_some() {
        image.modified_at = item.modified_at;
    }
    if item.width.is_some() {
        image.width = item.width;
    }
    if item.height.is_some() {
        image.height = item.height;
    }
    if item.orientation.is_some() {
        image.orientation = item.orientation;
    }
    if item.mime_type.is_some() {
        image.mime_type = item.mime_type.clone();
    }
    if item.title.is_some() {
        image.title = item.title.clone();
    }
    if item.album_name.is_some() {
        image.album_name = item.album_name.clone();
    }
}

fn image_from_item(item: &MediaItem) -> ImageFile {
    ImageFile {
        media_id: item.media_id,
        album_id: item.album_id,
        path: item.path.clone(),
        size: item.size,
        created_at: item.created_at,
        modified_at: item.modified_at,
        width: item.width,
        height: item.height,
        orientation: item.orientation,
        mime_type: item.mime_type.clone(),
        title: item.title.clone(),
        album_name: item.album_name.clone(),
        // Snapshot-only fields default to empty; a later full query fills them.
        ..ImageFile::default()
    }
}

fn remove_photo(library: &mut PhotoLibrary, item: &MediaItem) {
    let Some(key) = item_key(item) else {
        return;
    };
    library.images.retain(|image| !image_matches(image, &key));
}

fn upsert_video(library: &mut VideoLibrary, item: &MediaItem) {
    if let Some(key) = item_key(item) {
        if let Some(existing) = library
            .videos
            .iter_mut()
            .find(|video| video_matches(video, &key))
        {
            apply_video_overlap(existing, item);
            return;
        }
    }
    library.videos.push(video_from_item(item));
}

fn apply_video_overlap(video: &mut VideoFile, item: &MediaItem) {
    if item.media_id.is_some() {
        video.media_id = item.media_id;
    }
    if item.album_id.is_some() {
        video.album_id = item.album_id;
    }
    if item.path.is_some() {
        video.path = item.path.clone();
    }
    if item.size.is_some() {
        video.size = item.size;
    }
    if item.created_at.is_some() {
        video.created_at = item.created_at;
    }
    if item.modified_at.is_some() {
        video.modified_at = item.modified_at;
    }
    if item.width.is_some() {
        video.width = item.width;
    }
    if item.height.is_some() {
        video.height = item.height;
    }
    if item.orientation.is_some() {
        video.orientation = item.orientation;
    }
    if item.mime_type.is_some() {
        video.mime_type = item.mime_type.clone();
    }
    if item.duration.is_some() {
        video.duration = item.duration;
    }
}

fn video_from_item(item: &MediaItem) -> VideoFile {
    VideoFile {
        media_id: item.media_id,
        album_id: item.album_id,
        path: item.path.clone(),
        size: item.size,
        created_at: item.created_at,
        modified_at: item.modified_at,
        width: item.width,
        height: item.height,
        orientation: item.orientation,
        mime_type: item.mime_type.clone(),
        duration: item.duration,
        ..VideoFile::default()
    }
}

fn remove_video(library: &mut VideoLibrary, item: &MediaItem) {
    let Some(key) = item_key(item) else {
        return;
    };
    library.videos.retain(|video| !video_matches(video, &key));
}

fn upsert_audio(library: &mut AudioLibrary, item: &MediaItem) {
    if let Some(key) = item_key(item) {
        if let Some(existing) = library
            .tracks
            .iter_mut()
            .find(|audio| audio_matches(audio, &key))
        {
            apply_audio_overlap(existing, item);
            return;
        }
    }
    library.tracks.push(audio_from_item(item));
}

fn apply_audio_overlap(audio: &mut AudioFile, item: &MediaItem) {
    if item.media_id.is_some() {
        audio.media_id = item.media_id;
    }
    if item.album_id.is_some() {
        audio.album_id = item.album_id;
    }
    if item.path.is_some() {
        audio.path = item.path.clone();
    }
    if item.size.is_some() {
        audio.size = item.size;
    }
    if item.created_at.is_some() {
        audio.created_at = item.created_at;
    }
    if item.modified_at.is_some() {
        audio.modified_at = item.modified_at;
    }
    if item.mime_type.is_some() {
        audio.mime_type = item.mime_type.clone();
    }
    if item.title.is_some() {
        audio.title = item.title.clone();
    }
    if item.duration.is_some() {
        audio.duration = item.duration;
    }
    if item.artist.is_some() {
        audio.artist = item.artist.clone();
    }
}

fn audio_from_item(item: &MediaItem) -> AudioFile {
    AudioFile {
        media_id: item.media_id,
        album_id: item.album_id,
        path: item.path.clone(),
        size: item.size,
        created_at: item.created_at,
        modified_at: item.modified_at,
        mime_type: item.mime_type.clone(),
        title: item.title.clone(),
        duration: item.duration,
        artist: item.artist.clone(),
        ..AudioFile::default()
    }
}

fn remove_audio(library: &mut AudioLibrary, item: &MediaItem) {
    let Some(key) = item_key(item) else {
        return;
    };
    library.tracks.retain(|audio| !audio_matches(audio, &key));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(
        kind: MediaKind,
        added: Vec<MediaItem>,
        deleted: Vec<MediaItem>,
    ) -> MediaLibraryChange {
        MediaLibraryChange {
            kind,
            added,
            deleted,
            updated: Vec::new(),
            albums: Vec::new(),
        }
    }

    #[test]
    fn photo_added_creates_entry_with_default_snapshot_fields() {
        let mut library = PhotoLibrary::default();
        let item = MediaItem {
            media_id: Some(1),
            path: Some("/storage/emulated/0/DCIM/new.jpg".to_string()),
            size: Some(2048),
            width: Some(640),
            mime_type: Some("image/jpeg".to_string()),
            ..Default::default()
        };
        apply_photo(
            &mut library,
            &change(MediaKind::Photo, vec![item], Vec::new()),
        )
        .expect("apply");
        assert_eq!(library.images.len(), 1);
        assert_eq!(library.images[0].media_id, Some(1));
        assert_eq!(library.images[0].width, Some(640));
        assert_eq!(
            library.images[0].thumbnail, None,
            "snapshot-only field defaults"
        );
    }

    #[test]
    fn photo_updated_keeps_snapshot_only_fields() {
        let mut library = PhotoLibrary::default();
        library.images.push(ImageFile {
            media_id: Some(1),
            path: Some("/storage/emulated/0/DCIM/a.jpg".to_string()),
            size: Some(100),
            starred: true,
            thumbnail: Some(vec![0xFF, 0xD8]),
            ..Default::default()
        });
        let updated = MediaItem {
            media_id: Some(1),
            size: Some(4096),
            orientation: Some(6),
            ..Default::default()
        };
        apply_photo(
            &mut library,
            &change(MediaKind::Photo, Vec::new(), Vec::new()),
        )
        .expect("no-op");
        let mut change = change(MediaKind::Photo, Vec::new(), Vec::new());
        change.updated.push(updated);
        apply_photo(&mut library, &change).expect("apply update");
        assert_eq!(library.images.len(), 1);
        assert_eq!(library.images[0].size, Some(4096));
        assert_eq!(library.images[0].orientation, Some(6));
        assert_eq!(
            library.images[0].starred, true,
            "snapshot-only field preserved"
        );
        assert_eq!(
            library.images[0].thumbnail,
            Some(vec![0xFF, 0xD8]),
            "snapshot-only field preserved"
        );
    }

    #[test]
    fn deleted_removes_by_media_id_and_path() {
        let mut library = PhotoLibrary::default();
        library.images.push(ImageFile {
            media_id: Some(1),
            path: Some("/a.jpg".to_string()),
            ..Default::default()
        });
        library.images.push(ImageFile {
            media_id: None,
            path: Some("/b.jpg".to_string()),
            ..Default::default()
        });
        let deleted = vec![
            MediaItem {
                media_id: Some(1),
                ..Default::default()
            },
            MediaItem {
                path: Some("/b.jpg".to_string()),
                ..Default::default()
            },
        ];
        apply_photo(&mut library, &change(MediaKind::Photo, Vec::new(), deleted)).expect("apply");
        assert!(library.images.is_empty());
    }

    #[test]
    fn audio_duration_is_in_seconds_and_artist_is_mapped() {
        let mut library = AudioLibrary::default();
        let item = MediaItem {
            media_id: Some(9),
            title: Some("track".to_string()),
            artist: Some("artist".to_string()),
            duration: Some(210.0), // seconds (event channel is ms->s since M5)
            ..Default::default()
        };
        apply_audio(
            &mut library,
            &change(MediaKind::Audio, vec![item], Vec::new()),
        )
        .expect("apply");
        assert_eq!(library.tracks.len(), 1);
        assert_eq!(library.tracks[0].duration, Some(210.0));
        assert_eq!(library.tracks[0].artist.as_deref(), Some("artist"));
    }

    #[test]
    fn video_added_maps_duration() {
        let mut library = VideoLibrary::default();
        let item = MediaItem {
            media_id: Some(3),
            path: Some("/v.mp4".to_string()),
            duration: Some(12.5),
            ..Default::default()
        };
        apply_video(
            &mut library,
            &change(MediaKind::Video, vec![item], Vec::new()),
        )
        .expect("apply");
        assert_eq!(library.videos[0].duration, Some(12.5));
    }

    #[test]
    fn kind_mismatch_is_a_protocol_error() {
        let mut library = PhotoLibrary::default();
        let error = apply_photo(
            &mut library,
            &change(MediaKind::Video, vec![MediaItem::default()], Vec::new()),
        )
        .expect_err("kind mismatch");
        assert!(matches!(error, Error::Protocol(_)));
    }

    #[test]
    fn empty_change_is_a_no_op() {
        let mut library = PhotoLibrary::default();
        library.images.push(ImageFile {
            media_id: Some(1),
            ..Default::default()
        });
        apply_photo(
            &mut library,
            &change(MediaKind::Photo, Vec::new(), Vec::new()),
        )
        .expect("no-op");
        assert_eq!(library.images.len(), 1);
    }
}
