import Foundation
import HandShakerFFI

// MARK: - Request DTOs (FFI JSON contracts, handshaker_ffi.h)

/// `hs_media_thumbnail` request entry: minimal identity object
/// `{"path":"..."}` — only entries with a path are sent. The library DTO
/// fields beyond the path are not significant for the fetch
/// (`FfiThumbnailImageItem` in ffi/src/media.rs).
private struct ThumbnailEntry: Encodable {
    let path: String
}

/// `hs_media_thumbnail` request:
/// `{"images":[...],"videos":[...],"audio_albums":[...]}` (all optional).
private struct ThumbnailRequest: Encodable {
    let images: [ThumbnailEntry]
    let videos: [ThumbnailEntry]
    let audioAlbums: [ThumbnailEntry]

    private enum CodingKeys: String, CodingKey {
        case images
        case videos
        case audioAlbums = "audio_albums"
    }
}

/// `hs_media_fetch_exif` request: `{"path":"..."}`.
private struct ExifRequest: Encodable {
    let path: String
}

// MARK: - Media service

extension HandShakerRuntime {
    // MARK: Media

    /// Photo library snapshot (`hs_media_photo_library`, request `{}`).
    public func photoLibrary(sessionID: UInt64) async throws -> PhotoLibrary {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing("{}") { ptr, len in
                    try hsCall(as: PhotoLibrary.self) {
                        hs_media_photo_library(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Video library snapshot (`hs_media_video_library`, request `{}`).
    public func videoLibrary(sessionID: UInt64) async throws -> VideoLibrary {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing("{}") { ptr, len in
                    try hsCall(as: VideoLibrary.self) {
                        hs_media_video_library(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Audio library snapshot (`hs_media_audio_library`, request `{}`).
    public func audioLibrary(sessionID: UInt64) async throws -> AudioLibrary {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing("{}") { ptr, len in
                    try hsCall(as: AudioLibrary.self) {
                        hs_media_audio_library(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Fetch thumbnails for the requested media entries and cache them on
    /// disk (`hs_media_thumbnail`). The result carries cache paths, not
    /// bytes; only entries that returned thumbnail data are listed.
    ///
    /// - Parameters:
    ///   - sessionID: open session id.
    ///   - images: photo entries to thumbnail (identified by `path`).
    ///   - videos: video entries to thumbnail (identified by `path`).
    ///   - audioAlbums: audio albums to thumbnail (identified by `path`).
    public func thumbnail(
        sessionID: UInt64,
        images: [ImageFile] = [],
        videos: [VideoFile] = [],
        audioAlbums: [AudioAlbum] = []
    ) async throws -> ThumbnailResult {
        let request = ThumbnailRequest(
            images: images.compactMap(\.path).map(ThumbnailEntry.init(path:)),
            videos: videos.compactMap(\.path).map(ThumbnailEntry.init(path:)),
            audioAlbums: audioAlbums.compactMap(\.path).map(ThumbnailEntry.init(path:))
        )
        let body = try ServicesJSON.encode(request)
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: ThumbnailResult.self) {
                        hs_media_thumbnail(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// EXIF metadata of one remote image (`hs_media_fetch_exif`, request
    /// `{"path":"..."}`).
    public func fetchExif(sessionID: UInt64, path: String) async throws -> ExifData {
        let body = try ServicesJSON.encode(ExifRequest(path: path))
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: ExifData.self) {
                        hs_media_fetch_exif(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Merge a phone-pushed media change into a photo library
    /// (`hs_media_merge_change`, ABI 1.5 — pure function, no session).
    /// Entries are upserted by `media_id` (fallback `path`), preserving
    /// snapshot-only fields; deleted entries are removed by the same key.
    ///
    /// - Throws: `.invalidArgument` for a bad kind/JSON, `.invalidState`
    ///   when `change.mediaKind` is not `.photo`.
    public func mergePhotoChange(
        library: PhotoLibrary,
        change: MediaChange
    ) async throws -> PhotoLibrary {
        try await mergeMediaChange(
            kind: "photo",
            libraryJSON: ServicesJSON.encode(library),
            changeJSON: ServicesJSON.encode(change)
        )
    }

    /// Merge a phone-pushed media change into a video library
    /// (`hs_media_merge_change`). Throws `.invalidState` when
    /// `change.mediaKind` is not `.video`.
    public func mergeVideoChange(
        library: VideoLibrary,
        change: MediaChange
    ) async throws -> VideoLibrary {
        try await mergeMediaChange(
            kind: "video",
            libraryJSON: ServicesJSON.encode(library),
            changeJSON: ServicesJSON.encode(change)
        )
    }

    /// Merge a phone-pushed media change into an audio library
    /// (`hs_media_merge_change`). Throws `.invalidState` when
    /// `change.mediaKind` is not `.audio`.
    public func mergeAudioChange(
        library: AudioLibrary,
        change: MediaChange
    ) async throws -> AudioLibrary {
        try await mergeMediaChange(
            kind: "audio",
            libraryJSON: ServicesJSON.encode(library),
            changeJSON: ServicesJSON.encode(change)
        )
    }

    /// Shared `hs_media_merge_change` plumbing: three independent C-string
    /// arguments (kind, library JSON, change JSON), result decoded as `T`.
    private func mergeMediaChange<T: Decodable & Sendable>(
        kind: String,
        libraryJSON: String,
        changeJSON: String
    ) async throws -> T {
        try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsString(kind) { kindPtr, kindLen in
                    try withHsString(libraryJSON) { libraryPtr, libraryLen in
                        try withHsString(changeJSON) { changePtr, changeLen in
                            try hsCall(as: T.self) {
                                hs_media_merge_change(
                                    runtime, kindPtr, kindLen,
                                    libraryPtr, libraryLen,
                                    changePtr, changeLen
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}
