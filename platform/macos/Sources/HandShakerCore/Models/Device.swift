import Foundation

/// Stable device id — Rust `DeviceId` is a string newtype
/// (`crates/handshaker-application/src/dto.rs`), so it is a plain String in
/// JSON (e.g. "adb:serial", "wifi-endpoint:...", "phone:<uuid>").
public typealias DeviceID = String

/// Transport of a discovered device (dto.rs `TransportKind`,
/// `#[serde(rename_all = "snake_case")]`).
public enum TransportKind: String, Codable, Sendable, Equatable {
    case adb
    case wifi
    case usbAccessory = "usb_accessory"
    /// Forward compatibility: unknown transport tokens decode safely.
    case unknown

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? .unknown
    }
}

/// ADB-specific detail (dto.rs `AdbDetailDto`).
public struct AdbDetail: Codable, Sendable, Equatable {
    /// ADB state, e.g. "device" or "unauthorized".
    public let state: String
    public let product: String?
    public let model: String?
    public let device: String?

    private enum CodingKeys: String, CodingKey {
        case state, product, model, device
    }
}

/// USB AOA detail (dto.rs `UsbDetailDto`).
public struct UsbDetail: Codable, Sendable, Equatable {
    public let busNumber: UInt8
    public let serial: String?
    public let vendorID: UInt16
    public let productID: UInt16
    /// Accessory-mode token as the core serializes it ("Accessory"/"Plain").
    public let mode: String

    private enum CodingKeys: String, CodingKey {
        case busNumber = "bus_number"
        case serial
        case vendorID = "vendor_id"
        case productID = "product_id"
        case mode
    }
}

/// A discovered device, UI-ready (dto.rs `DeviceDescriptor`).
public struct DeviceDescriptor: Codable, Sendable, Equatable {
    /// Identity of this discovery entry. For Wi-Fi this is only a discovery
    /// *endpoint* id (the mDNS SRV port is dynamic) and must never be used
    /// as a long-lived device identity. UI should prefer `stableID ?? id`.
    public let id: DeviceID
    /// Stable identity ("phone:<uuid>") reconciled after connection;
    /// `#[serde(default)]` in Rust — may be absent in JSON.
    public let stableID: DeviceID?
    public let displayName: String?
    public let model: String?
    public let transport: TransportKind
    public let transportAddress: String
    public let available: Bool
    public let adb: AdbDetail?
    public let usb: UsbDetail?

    private enum CodingKeys: String, CodingKey {
        case id
        case stableID = "stable_id"
        case displayName = "display_name"
        case model
        case transport
        case transportAddress = "transport_address"
        case available
        case adb
        case usb
    }
}

/// Fixed session states (dto.rs `SessionState`, snake_case).
public enum SessionState: String, Codable, Sendable, Equatable {
    case connecting
    case ready
    case disconnecting
    case closed
    case failed
    /// Forward compatibility: unknown state tokens decode safely.
    case unknown

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? .unknown
    }
}

/// UI-ready device information snapshot (dto.rs `DeviceInfoDto`).
/// Every field after `root_path` is `#[serde(default)]` in Rust and may be
/// absent in JSON — Swift optionals tolerate missing/null keys.
public struct DeviceInfo: Codable, Sendable, Equatable {
    public let serial: String
    public let phoneID: String?
    public let name: String?
    public let model: String?
    public let brand: String?
    public let manufacturer: String?
    public let smartisanVersion: String?
    public let apkVersion: String?
    public let apkVersionName: String?
    public let rootPath: String
    /// External storage path when reported (e.g. "/storage/XXXX-XXXX").
    public let externalStoragePath: String?
    /// Total internal storage size in bytes, when reported.
    public let diskSize: UInt64?
    /// Used internal storage size in bytes, when reported.
    public let usedDiskSize: UInt64?
    /// Battery percentage, when reported.
    public let batteryPercentage: UInt32?
    /// Whether the phone reports a locked screen.
    public let phoneLocked: Bool?

    private enum CodingKeys: String, CodingKey {
        case serial
        case phoneID = "phone_id"
        case name
        case model
        case brand
        case manufacturer
        case smartisanVersion = "smartisan_version"
        case apkVersion = "apk_version"
        case apkVersionName = "apk_version_name"
        case rootPath = "root_path"
        case externalStoragePath = "external_storage_path"
        case diskSize = "disk_size"
        case usedDiskSize = "used_disk_size"
        case batteryPercentage = "battery_percentage"
        case phoneLocked = "phone_locked"
    }
}

/// Snapshot of one open session (dto.rs `SessionSnapshot`).
public struct SessionSnapshot: Codable, Sendable, Equatable {
    public let id: UInt64
    public let device: DeviceDescriptor
    public let deviceInfo: DeviceInfo
    public let state: SessionState
    public let connectedAtMs: UInt64
    public let lastActivityAtMs: UInt64?

    private enum CodingKeys: String, CodingKey {
        case id
        case device
        case deviceInfo = "device_info"
        case state
        case connectedAtMs = "connected_at_ms"
        case lastActivityAtMs = "last_activity_at_ms"
    }
}

/// Round-trip latency of a device ping (dto.rs `PingResultDto`).
public struct PingResult: Codable, Sendable, Equatable {
    public let roundTripMs: UInt64

    private enum CodingKeys: String, CodingKey {
        case roundTripMs = "round_trip_ms"
    }
}

/// Result of a multi-transport discovery sweep
/// (discovery.rs `DeviceDiscoveryResult`).
public struct DeviceDiscoveryResult: Codable, Sendable, Equatable {
    public let devices: [DeviceDescriptor]
    public let warnings: [DeviceDiscoveryWarning]
}

/// A per-transport failure during discovery (discovery.rs
/// `DeviceDiscoveryWarning`); never aborts devices other transports found.
public struct DeviceDiscoveryWarning: Codable, Sendable, Equatable {
    public let transport: TransportKind
    public let error: HandShakerNativeError
}
