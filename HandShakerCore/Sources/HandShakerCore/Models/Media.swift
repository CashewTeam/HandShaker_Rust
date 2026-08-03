import Foundation

/// Media library category (dto.rs `MediaKindDto`, snake_case).
public enum MediaKind: String, Codable, Sendable, Equatable {
    case photo
    case video
    case audio
    /// Forward compatibility: unknown kind tokens decode safely.
    case unknown

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? .unknown
    }
}

// MARK: - Photo

/// One image entry (media.rs `ImageFileDto`).
public struct ImageFile: Codable, Sendable, Equatable {
    public let path: String?
    public let size: UInt64?
    public let createdAt: UInt64?
    public let modifiedAt: UInt64?
    public let width: UInt32?
    public let height: UInt32?
    public let orientation: UInt32?
    public let mediaID: UInt64?
    public let albumID: UInt64?
    public let mimeType: String?
    /// Thumbnail bytes. Rust serializes `Vec<u8>` as a JSON number array
    /// (never base64); `thumbnailData` offers the byte view.
    public let thumbnail: [UInt8]?
    public let albumName: String?
    public let dateTaken: UInt64?
    public let latitude: String?
    public let longitude: String?
    public let miniThumbMagic: String?
    public let title: String?
    public let thumbnailError: Bool
    public let starred: Bool

    /// Byte view of `thumbnail` (the JSON wire format is a number array).
    public var thumbnailData: Data? { thumbnail.map(Data.init) }

    private enum CodingKeys: String, CodingKey {
        case path
        case size
        case createdAt = "created_at"
        case modifiedAt = "modified_at"
        case width
        case height
        case orientation
        case mediaID = "media_id"
        case albumID = "album_id"
        case mimeType = "mime_type"
        case thumbnail
        case albumName = "album_name"
        case dateTaken = "date_taken"
        case latitude
        case longitude
        case miniThumbMagic = "mini_thumb_magic"
        case title
        case thumbnailError = "thumbnail_error"
        case starred
    }
}

/// One photo album (media.rs `ImageAlbumDto`).
public struct ImageAlbum: Codable, Sendable, Equatable {
    public let path: String?
    public let albumID: UInt64?
    public let name: String?
    public let coverImage: ImageFile?

    private enum CodingKeys: String, CodingKey {
        case path
        case albumID = "album_id"
        case name
        case coverImage = "cover_image"
    }
}

/// Photo library snapshot (media.rs `PhotoLibraryDto`).
public struct PhotoLibrary: Codable, Sendable, Equatable {
    public let images: [ImageFile]
    public let albums: [ImageAlbum]
    public let cameraAlbumID: UInt64?

    private enum CodingKeys: String, CodingKey {
        case images
        case albums
        case cameraAlbumID = "camera_album_id"
    }
}

// MARK: - Video

/// One video entry (media.rs `VideoFileDto`).
public struct VideoFile: Codable, Sendable, Equatable {
    public let path: String?
    public let size: UInt64?
    public let createdAt: UInt64?
    public let modifiedAt: UInt64?
    public let width: UInt32?
    public let height: UInt32?
    public let orientation: UInt32?
    public let mediaID: UInt64?
    public let albumID: UInt64?
    public let mimeType: String?
    public let thumbnail: [UInt8]?
    public let thumbnailError: Bool
    public let duration: Double?

    /// Byte view of `thumbnail` (the JSON wire format is a number array).
    public var thumbnailData: Data? { thumbnail.map(Data.init) }

    private enum CodingKeys: String, CodingKey {
        case path
        case size
        case createdAt = "created_at"
        case modifiedAt = "modified_at"
        case width
        case height
        case orientation
        case mediaID = "media_id"
        case albumID = "album_id"
        case mimeType = "mime_type"
        case thumbnail
        case thumbnailError = "thumbnail_error"
        case duration
    }
}

/// One video album (media.rs `VideoAlbumDto`).
public struct VideoAlbum: Codable, Sendable, Equatable {
    public let path: String?
    public let albumID: UInt64?
    public let name: String?

    private enum CodingKeys: String, CodingKey {
        case path
        case albumID = "album_id"
        case name
    }
}

/// Video library snapshot (media.rs `VideoLibraryDto`).
public struct VideoLibrary: Codable, Sendable, Equatable {
    public let videos: [VideoFile]
    public let albums: [VideoAlbum]
}

// MARK: - Audio

/// One audio track (media.rs `AudioFileDto`).
public struct AudioFile: Codable, Sendable, Equatable {
    public let path: String?
    public let size: UInt64?
    public let createdAt: UInt64?
    public let modifiedAt: UInt64?
    public let mediaID: UInt64?
    public let albumID: UInt64?
    public let title: String?
    public let mimeType: String?
    public let artistID: UInt64?
    public let artist: String?
    public let composer: String?
    public let genre: UInt32?
    public let comment: String?
    public let copyright: String?
    public let audioCodec: String?
    public let track: UInt32?
    public let duration: Double?

    private enum CodingKeys: String, CodingKey {
        case path
        case size
        case createdAt = "created_at"
        case modifiedAt = "modified_at"
        case mediaID = "media_id"
        case albumID = "album_id"
        case title
        case mimeType = "mime_type"
        case artistID = "artist_id"
        case artist
        case composer
        case genre
        case comment
        case copyright
        case audioCodec = "audio_codec"
        case track
        case duration
    }
}

/// One audio album (media.rs `AudioAlbumDto`).
public struct AudioAlbum: Codable, Sendable, Equatable {
    public let path: String?
    public let albumID: UInt64?
    public let name: String?
    public let artistID: UInt64?
    public let artist: String?
    public let year: UInt32?
    public let thumbnail: [UInt8]?
    public let thumbnailError: Bool

    /// Byte view of `thumbnail` (the JSON wire format is a number array).
    public var thumbnailData: Data? { thumbnail.map(Data.init) }

    private enum CodingKeys: String, CodingKey {
        case path
        case albumID = "album_id"
        case name
        case artistID = "artist_id"
        case artist
        case year
        case thumbnail
        case thumbnailError = "thumbnail_error"
    }
}

/// Audio library snapshot (media.rs `AudioLibraryDto`).
public struct AudioLibrary: Codable, Sendable, Equatable {
    public let tracks: [AudioFile]
    public let albums: [AudioAlbum]
}

// MARK: - Thumbnails & EXIF

/// One thumbnail cache hit returned by `hs_media_thumbnail`
/// (ffi/src/media.rs): `{"path","cache_path","size"}`. Bytes are cached on
/// disk under `<state_dir>/thumbnails/` and reused across calls.
public struct ThumbnailCacheEntry: Codable, Sendable, Equatable {
    /// Remote media path.
    public let path: String
    /// Absolute path of the cached thumbnail file.
    public let cachePath: String
    /// Cached file size in bytes.
    public let size: UInt64

    private enum CodingKeys: String, CodingKey {
        case path
        case cachePath = "cache_path"
        case size
    }
}

/// `hs_media_thumbnail` result: only entries that returned thumbnail data
/// are listed.
public struct ThumbnailResult: Codable, Sendable, Equatable {
    public let images: [ThumbnailCacheEntry]
    public let videos: [ThumbnailCacheEntry]
    public let audioAlbums: [ThumbnailCacheEntry]

    private enum CodingKeys: String, CodingKey {
        case images
        case videos
        case audioAlbums = "audio_albums"
    }
}

/// Exif metadata (media.rs `ExifDataDto`).
public struct ExifData: Codable, Sendable, Equatable {
    public let orientation: UInt32?
    public let dateTaken: UInt64?
    public let latitude: String?
    public let longitude: String?
    public let make: String?
    public let model: String?
    public let software: String?
    public let lensModel: String?
    public let focalLength: Double?
    public let exposureTime: Double?
    public let fNumber: Double?
    public let iso: UInt32?

    private enum CodingKeys: String, CodingKey {
        case orientation
        case dateTaken = "date_taken"
        case latitude
        case longitude
        case make
        case model
        case software
        case lensModel = "lens_model"
        case focalLength = "focal_length"
        case exposureTime = "exposure_time"
        case fNumber = "f_number"
        case iso
    }
}

// MARK: - Media change (phone-initiated)

/// One media entry inside a library change (dto.rs `MediaChangeItemDto`).
public struct MediaChangeItem: Codable, Sendable, Equatable {
    public let mediaID: UInt64?
    public let path: String?
    public let size: UInt64?
    public let createdAt: UInt64?
    public let modifiedAt: UInt64?
    public let mimeType: String?
    public let title: String?
    public let albumName: String?

    private enum CodingKeys: String, CodingKey {
        case mediaID = "media_id"
        case path
        case size
        case createdAt = "created_at"
        case modifiedAt = "modified_at"
        case mimeType = "mime_type"
        case title
        case albumName = "album_name"
    }
}

/// A media library change pushed by the phone (dto.rs `MediaChangeDto`).
/// The category is `media_kind` (not `kind`) so the event JSON keeps its
/// own `kind` tag distinct from the payload.
public struct MediaChange: Codable, Sendable, Equatable {
    public let mediaKind: MediaKind
    public let added: [MediaChangeItem]
    public let deleted: [MediaChangeItem]
    public let updated: [MediaChangeItem]

    /// Public memberwise initializer (the synthesized one is internal and
    /// invisible to the test target).
    public init(mediaKind: MediaKind, added: [MediaChangeItem], deleted: [MediaChangeItem], updated: [MediaChangeItem]) {
        self.mediaKind = mediaKind
        self.added = added
        self.deleted = deleted
        self.updated = updated
    }

    private enum CodingKeys: String, CodingKey {
        case mediaKind = "media_kind"
        case added
        case deleted
        case updated
    }
}
