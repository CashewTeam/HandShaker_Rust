import Foundation
import HandShakerFFI

// MARK: - Request DTOs (FFI JSON contracts, handshaker_ffi.h)

/// `hs_media_thumbnail` request:
/// `{"images":[...],"videos":[...],"audio_albums":[...]}` (all optional).
private struct ThumbnailRequest: Encodable {
    let images: [ImageFile]
    let videos: [VideoFile]
    let audioAlbums: [AudioAlbum]

    private enum CodingKeys: String, CodingKey {
        case images
        case videos
        case audioAlbums = "audio_albums"
    }
}

private func thumbnailBatches(
    images: [ImageFile],
    videos: [VideoFile],
    audioAlbums: [AudioAlbum],
    batchSize: Int
) -> [ThumbnailRequest] {
    var batches: [ThumbnailRequest] = []
    for start in stride(from: 0, to: images.count, by: batchSize) {
        batches.append(ThumbnailRequest(
            images: Array(images[start..<min(start + batchSize, images.count)]),
            videos: [],
            audioAlbums: []
        ))
    }
    for start in stride(from: 0, to: videos.count, by: batchSize) {
        batches.append(ThumbnailRequest(
            images: [],
            videos: Array(videos[start..<min(start + batchSize, videos.count)]),
            audioAlbums: []
        ))
    }
    for start in stride(from: 0, to: audioAlbums.count, by: batchSize) {
        batches.append(ThumbnailRequest(
            images: [],
            videos: [],
            audioAlbums: Array(audioAlbums[start..<min(start + batchSize, audioAlbums.count)])
        ))
    }
    return batches
}

/// `hs_media_fetch_exif` request: `{"path":"..."}`.
private struct ExifRequest: Encodable {
    let path: String
}

/// Shared paged media request. Encoding through `JSONEncoder` avoids raw
/// string interpolation mistakes and omits absent optional fields.
struct MediaPageRequest: Encodable {
    let limit: Int?
    let cursor: UInt64?
}

// MARK: - Media service

extension HandShakerRuntime {
    // MARK: Media

    /// Photo library snapshot (`hs_media_photo_library`, request `{}`).
    /// Photo library snapshot (`hs_media_photo_library`). Pass `{}` for
    /// the full library, or `limit`/`cursor` for one page (P1-9):
    /// `limit` defaults to 500 (max 1000), `cursor` is the previous
    /// response's `nextCursor`. `nextCursor == nil` on the last page.
    public func photoLibrary(
        sessionID: UInt64,
        limit: Int? = nil,
        cursor: UInt64? = nil
    ) async throws -> PhotoLibrary {
        let body = try ServicesJSON.encode(MediaPageRequest(limit: limit, cursor: cursor))
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: PhotoLibrary.self) {
                        hs_media_photo_library(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Video library snapshot (`hs_media_video_library`, request `{}`).
    /// Video library snapshot (`hs_media_video_library`). Supports the
    /// same paged request shape as `photoLibrary` (P1-9).
    public func videoLibrary(
        sessionID: UInt64,
        limit: Int? = nil,
        cursor: UInt64? = nil
    ) async throws -> VideoLibrary {
        let body = try ServicesJSON.encode(MediaPageRequest(limit: limit, cursor: cursor))
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: VideoLibrary.self) {
                        hs_media_video_library(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Audio library snapshot (`hs_media_audio_library`, request `{}`).
    /// Audio library snapshot (`hs_media_audio_library`). Supports the
    /// same paged request shape as `photoLibrary` (P1-9).
    public func audioLibrary(
        sessionID: UInt64,
        limit: Int? = nil,
        cursor: UInt64? = nil
    ) async throws -> AudioLibrary {
        let body = try ServicesJSON.encode(MediaPageRequest(limit: limit, cursor: cursor))
        return try await callNative {
            try self.handle.withRuntime { runtime in
                try withHsRequestThrowing(body) { ptr, len in
                    try hsCall(as: AudioLibrary.self) {
                        hs_media_audio_library(runtime, sessionID, ptr, len)
                    }
                }
            }
        }
    }

    /// Fetch thumbnails for the requested media entries and cache them on
    /// disk (`hs_media_thumbnail`). The result carries cache paths, not
    /// bytes; per-item phone failures are returned in the matching `failed*`
    /// arrays instead of disappearing from the result.
    ///
    /// - Parameters:
    ///   - sessionID: open session id.
    ///   - images: photo metadata entries (identified by media id or path).
    ///   - videos: video metadata entries (identified by media id or path).
    ///   - audioAlbums: album metadata entries (identified by album id or path).
    public func thumbnail(
        sessionID: UInt64,
        images: [ImageFile] = [],
        videos: [VideoFile] = [],
        audioAlbums: [AudioAlbum] = []
    ) async throws -> ThumbnailResult {
        let request = ThumbnailRequest(
            images: images,
            videos: videos,
            audioAlbums: audioAlbums
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

    /// Lazily fetch thumbnail batches and yield each completed batch as soon
    /// as it is cached. This is the preferred API for a scrolling grid/list:
    /// pass only the currently needed entries, update visible cells for every
    /// yielded result, and cancel the consuming task when the view disappears.
    ///
    /// Batches run with bounded concurrency. Cancelling the stream stops new
    /// batches from being scheduled; an FFI call already executing still runs
    /// to completion because the synchronous C ABI has no mid-call cancel
    /// handle. Its result remains safely cached for a later request.
    public func thumbnailStream(
        sessionID: UInt64,
        images: [ImageFile] = [],
        videos: [VideoFile] = [],
        audioAlbums: [AudioAlbum] = [],
        batchSize: Int = 8,
        maxConcurrentBatches: Int = 2
    ) -> AsyncThrowingStream<ThumbnailResult, Error> {
        guard batchSize > 0, maxConcurrentBatches > 0 else {
            return AsyncThrowingStream { continuation in
                continuation.finish(throwing: HandShakerError.invalidArgument(
                    "batchSize and maxConcurrentBatches must be greater than zero"
                ))
            }
        }
        let batches = thumbnailBatches(
            images: images,
            videos: videos,
            audioAlbums: audioAlbums,
            batchSize: batchSize
        )
        return AsyncThrowingStream { continuation in
            let producer = Task {
                do {
                    try await withThrowingTaskGroup(of: ThumbnailResult.self) { group in
                        var next = 0
                        while next < min(maxConcurrentBatches, batches.count) {
                            let batch = batches[next]
                            next += 1
                            group.addTask {
                                try await self.thumbnail(
                                    sessionID: sessionID,
                                    images: batch.images,
                                    videos: batch.videos,
                                    audioAlbums: batch.audioAlbums
                                )
                            }
                        }
                        while let result = try await group.next() {
                            try Task.checkCancellation()
                            continuation.yield(result)
                            if next < batches.count {
                                let batch = batches[next]
                                next += 1
                                group.addTask {
                                    try await self.thumbnail(
                                        sessionID: sessionID,
                                        images: batch.images,
                                        videos: batch.videos,
                                        audioAlbums: batch.audioAlbums
                                    )
                                }
                            }
                        }
                    }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in producer.cancel() }
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
