import XCTest
@testable import HandShakerCore

/// Codable model decoding against real Rust JSON fixtures. Fixture strings
/// are copied verbatim from `crates/handshaker-application/src/tests.rs`
/// (the frozen v1 JSON contract tests) and `event.rs`/`transfer.rs` sample
/// shapes — see the per-test comments for the source lines.
final class ModelTests: XCTestCase {
    private let decoder = JSONDecoder()

    // MARK: - DeviceDescriptor (tests.rs:574 `device_descriptor_json_contract_is_stable`)

    func testDeviceDescriptorLegacyJSONDecodes() throws {
        // A descriptor serialized before stable_id existed still decodes.
        let json = """
        {"id":"serial-1","display_name":"serial-1","model":null,"transport":"adb",\
        "transport_address":"serial-1","available":true,"adb":null,"usb":null}
        """
        let device = try decoder.decode(DeviceDescriptor.self, from: Data(json.utf8))
        XCTAssertEqual(device.id, "serial-1")
        XCTAssertNil(device.stableID)
        XCTAssertEqual(device.displayName, "serial-1")
        XCTAssertNil(device.model)
        XCTAssertEqual(device.transport, .adb)
        XCTAssertEqual(device.transportAddress, "serial-1")
        XCTAssertTrue(device.available)
        XCTAssertNil(device.adb)
        XCTAssertNil(device.usb)
    }

    func testDeviceDescriptorFullJSONDecodes() throws {
        let json = """
        {"id":"adb:3f13d4b4","stable_id":"phone:9a3f-77ee","display_name":"test phone",
        "model":"OD103","transport":"wifi","transport_address":"192.0.2.47:45656",
        "available":true,"adb":{"state":"device","product":null,"model":null,"device":null},
        "usb":{"bus_number":1,"serial":"usb-serial","vendor_id":6353,"product_id":11521,"mode":"Accessory"}}
        """
        let device = try decoder.decode(DeviceDescriptor.self, from: Data(json.utf8))
        XCTAssertEqual(device.stableID, "phone:9a3f-77ee")
        XCTAssertEqual(device.transport, .wifi)
        XCTAssertEqual(device.adb?.state, "device")
        XCTAssertEqual(device.usb?.busNumber, 1)
        XCTAssertEqual(device.usb?.vendorID, 0x18d1)
    }

    // MARK: - DeviceInfo (tests.rs:512 legacy fixture)

    func testDeviceInfoLegacyJSONDecodes() throws {
        // A v1-preview JSON without the optional fields still decodes.
        let json = """
        {"serial":"s1","phone_id":null,"name":null,"model":null,"brand":null,\
        "manufacturer":null,"smartisan_version":null,"apk_version":null,\
        "apk_version_name":null,"root_path":"/"}
        """
        let info = try decoder.decode(DeviceInfo.self, from: Data(json.utf8))
        XCTAssertEqual(info.serial, "s1")
        XCTAssertEqual(info.rootPath, "/")
        XCTAssertNil(info.externalStoragePath)
        XCTAssertNil(info.diskSize)
        XCTAssertNil(info.phoneLocked)
    }

    func testDeviceInfoFullJSONDecodes() throws {
        let json = """
        {"serial":"s1","phone_id":"p1","name":"Phone","model":"OD103",\
        "brand":"SMARTISAN","manufacturer":"Smartisan","smartisan_version":"6.7.4",\
        "apk_version":"201","apk_version_name":"1.2.0","root_path":"/storage/emulated/0",\
        "external_storage_path":"/storage/ABCD-1234","disk_size":128000000000,\
        "used_disk_size":64000000000,"battery_percentage":77,"phone_locked":true}
        """
        let info = try decoder.decode(DeviceInfo.self, from: Data(json.utf8))
        XCTAssertEqual(info.externalStoragePath, "/storage/ABCD-1234")
        XCTAssertEqual(info.diskSize, 128_000_000_000)
        XCTAssertEqual(info.batteryPercentage, 77)
        XCTAssertEqual(info.phoneLocked, true)
    }

    // MARK: - FileEntry (event.rs `sample_file` shape, tests.rs `remote_file_maps_to_dto`)

    func testFileEntryJSONDecodes() throws {
        let json = """
        {"path":"/a/b.txt","size":42,"created_at_ms":1,"modified_at_ms":2,\
        "is_directory":false,"checksum":"abc","is_trash":null,"media_id":9}
        """
        let file = try decoder.decode(FileEntry.self, from: Data(json.utf8))
        XCTAssertEqual(file.path, "/a/b.txt")
        XCTAssertEqual(file.size, 42)
        XCTAssertEqual(file.createdAtMs, 1)
        XCTAssertEqual(file.modifiedAtMs, 2)
        XCTAssertFalse(file.isDirectory)
        XCTAssertEqual(file.checksum, "abc")
        XCTAssertNil(file.isTrash)
        XCTAssertEqual(file.mediaID, 9)
    }

    // MARK: - TransferSnapshot (tests.rs:1661 full + tests.rs:1699 legacy)

    func testTransferSnapshotFullJSONDecodes() throws {
        // Full snapshot incl. the Phase E batch fields (tests.rs
        // `transfer_snapshot_json_contract_is_stable`).
        let json = """
        {"id":7,"session_id":1,"direction":"download","source":"/remote/f.bin",\
        "destination":"/local/f.bin","state":"running","transferred_bytes":12,\
        "total_bytes":100,"started_at_ms":1,"finished_at_ms":null,"error":null,\
        "item_count":3,"completed_items":1,"failed_items":0,\
        "current_item":"/remote/g.bin",\
        "batch_result":{"ok":[{"source":"/remote/g.bin","target":"/local/g.bin"}],"failures":[]}}
        """
        let snapshot = try decoder.decode(TransferSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snapshot.id, 7)
        XCTAssertEqual(snapshot.sessionID, 1)
        XCTAssertEqual(snapshot.direction, .download)
        XCTAssertEqual(snapshot.state, .running)
        XCTAssertEqual(snapshot.transferredBytes, 12)
        XCTAssertEqual(snapshot.totalBytes, 100)
        XCTAssertEqual(snapshot.itemCount, 3)
        XCTAssertEqual(snapshot.completedItems, 1)
        XCTAssertEqual(snapshot.failedItems, 0)
        XCTAssertEqual(snapshot.currentItem, "/remote/g.bin")
        XCTAssertEqual(snapshot.batchResult?.ok.first?.source, "/remote/g.bin")
        XCTAssertEqual(snapshot.batchResult?.failures.count, 0)
    }

    func testTransferSnapshotLegacyJSONDecodes() throws {
        // Legacy JSON without the Phase E fields decodes with defaults.
        let json = """
        {"id":7,"session_id":1,"direction":"download","source":"/remote/f.bin",\
        "destination":"/local/f.bin","state":"running","transferred_bytes":12,\
        "total_bytes":100,"started_at_ms":1,"finished_at_ms":null,"error":null}
        """
        let snapshot = try decoder.decode(TransferSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snapshot.itemCount, 0)
        XCTAssertEqual(snapshot.completedItems, 0)
        XCTAssertEqual(snapshot.failedItems, 0)
        XCTAssertNil(snapshot.currentItem)
        XCTAssertNil(snapshot.batchResult)
    }

    func testTransferSnapshotWithFailureDecodes() throws {
        // TransferFailureDto shape (tests.rs:1721 `batch_result_to_dto_preserves_ok_and_failures`).
        let json = """
        {"id":8,"session_id":1,"direction":"upload","source":"/local/a.bin",\
        "destination":"/remote/a.bin","state":"failed","transferred_bytes":0,\
        "total_bytes":null,"started_at_ms":1,"finished_at_ms":2,\
        "error":{"code":"remote_io","message":"write failed","detail":"disk full",\
        "retryable":true,"operation":"download"},\
        "item_count":2,"completed_items":1,"failed_items":1,"current_item":"/remote/b.bin",\
        "batch_result":{"ok":[{"source":"/local/a.bin","target":"/remote/a.bin"}],\
        "failures":[{"source":"/remote/b.txt","target":"/local/b.txt","message":"remote io"}]}}
        """
        let snapshot = try decoder.decode(TransferSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snapshot.state, .failed)
        XCTAssertEqual(snapshot.error?.code, "remote_io")
        XCTAssertEqual(snapshot.error?.retryable, true)
        XCTAssertEqual(snapshot.batchResult?.failures.first?.message, "remote io")
    }

    // MARK: - EventEnvelope (event.rs internally tagged `kind`)

    func testEventEnvelopeTransferUpdatedDecodes() throws {
        // TransferUpdated is a newtype variant: the TransferSnapshot fields
        // are inlined next to "kind":"transfer_updated" (serde internally
        // tagged; see tests.rs:2334-2381 round-trips).
        let json = """
        {"sequence":5,"timestamp_ms":1700000000000,"event":{"kind":"transfer_updated",\
        "id":7,"session_id":1,"direction":"download","source":"/remote/f.bin",\
        "destination":"/local/f.bin","state":"completed","transferred_bytes":100,\
        "total_bytes":100,"started_at_ms":1,"finished_at_ms":2,"error":null,\
        "item_count":0,"completed_items":0,"failed_items":0,"current_item":null,"batch_result":null}}
        """
        let envelope = try decoder.decode(EventEnvelope.self, from: Data(json.utf8))
        XCTAssertEqual(envelope.sequence, 5)
        XCTAssertEqual(envelope.timestampMs, 1_700_000_000_000)
        guard case .transferUpdated(let snapshot) = envelope.event else {
            return XCTFail("expected .transferUpdated, got \(envelope.event)")
        }
        XCTAssertEqual(snapshot.id, 7)
        XCTAssertEqual(snapshot.state, .completed)
    }

    func testUnknownEventDecodesAsUnknown() throws {
        // Unknown event kinds must decode safely as .unknown(String).
        let json = """
        {"sequence":9,"timestamp_ms":1700000000001,"event":{"kind":"quantum_entangled","foo":1}}
        """
        let envelope = try decoder.decode(EventEnvelope.self, from: Data(json.utf8))
        guard case .unknown(let kind) = envelope.event else {
            return XCTFail("expected .unknown, got \(envelope.event)")
        }
        XCTAssertEqual(kind, "quantum_entangled")
    }

    func testEventEnvelopeMediaChangedDecodes() throws {
        // tests.rs:2305-2317 media_changed shape; MediaChangeDto entries
        // from tests.rs:2006-2018 (bridge_client_event_maps_known_core_events).
        let json = """
        {"sequence":6,"timestamp_ms":1700000000002,"event":{"kind":"media_changed",\
        "session_id":3,"change":{"media_kind":"photo",\
        "added":[{"media_id":7,"path":"/DCIM/a.jpg","size":1024}],"deleted":[],"updated":[]}}}
        """
        let envelope = try decoder.decode(EventEnvelope.self, from: Data(json.utf8))
        guard case .mediaChanged(let sessionID, let change) = envelope.event else {
            return XCTFail("expected .mediaChanged, got \(envelope.event)")
        }
        XCTAssertEqual(sessionID, 3)
        XCTAssertEqual(change.mediaKind, .photo)
        XCTAssertEqual(change.added.first?.mediaID, 7)
        XCTAssertEqual(change.added.first?.path, "/DCIM/a.jpg")
        XCTAssertEqual(change.added.first?.size, 1024)
        XCTAssertTrue(change.deleted.isEmpty)
        XCTAssertTrue(change.updated.isEmpty)
    }

    func testEventEnvelopeRemoteFileChangedLegacyDecodes() throws {
        // Legacy JSON without the optional files/statuses keys
        // (event.rs `remote_file_change_accepts_legacy_json_without_metadata`).
        let json = """
        {"sequence":7,"timestamp_ms":1700000000003,"event":{"kind":"remote_file_changed",\
        "session_id":3,"change":{"change_kind":"file_changed",\
        "paths":["/storage/emulated/0/a.txt"]}}}
        """
        let envelope = try decoder.decode(EventEnvelope.self, from: Data(json.utf8))
        guard case .remoteFileChanged(_, let change) = envelope.event else {
            return XCTFail("expected .remoteFileChanged, got \(envelope.event)")
        }
        XCTAssertEqual(change.changeKind, .fileChanged)
        XCTAssertEqual(change.paths, ["/storage/emulated/0/a.txt"])
        XCTAssertNil(change.files)
        XCTAssertNil(change.statuses)
    }

    // MARK: - Other DTO shapes

    func testClipboardEntryDecodes() throws {
        // tests.rs:2292-2303 clipboard shape; ClipboardEntryDto has i64
        // timestamp_ms.
        let json = #"{"text":"hi","timestamp_ms":1}"#
        let entry = try decoder.decode(ClipboardEntry.self, from: Data(json.utf8))
        XCTAssertEqual(entry.text, "hi")
        XCTAssertEqual(entry.timestampMs, 1)
    }

    func testMediaLibraryShapeDecodes() throws {
        // PhotoLibraryDto shape (ffi/src/media.rs:43): thumbnail bytes
        // travel as a JSON number array (Rust Vec<u8> serde output).
        let json = """
        {"images":[{"path":"/a.jpg","size":100,"starred":true,"thumbnail_error":false, \
        "thumbnail":[1,2,3]}],"albums":[{"path":"/alb","album_id":1,"name":"A", \
        "cover_image":{"path":"/a.jpg","size":0,"starred":false,"thumbnail_error":false}}],\
        "camera_album_id":5}
        """
        let library = try decoder.decode(PhotoLibrary.self, from: Data(json.utf8))
        XCTAssertEqual(library.images.count, 1)
        XCTAssertEqual(library.images[0].thumbnailData, Data([1, 2, 3]))
        XCTAssertEqual(library.images[0].starred, true)
        XCTAssertEqual(library.albums[0].name, "A")
        XCTAssertEqual(library.albums[0].coverImage?.path, "/a.jpg")
        XCTAssertEqual(library.cameraAlbumID, 5)
    }

    func testThumbnailResultDecodesFailuresAndOlderShape() throws {
        let json = """
        {"images":[{"path":"/a.jpg","cache_path":"/tmp/a.thumb","size":42}],\
        "videos":[],"audio_albums":[],\
        "failed_images":[{"path":"/b.jpg","media_id":7,"reason":"thumbnail_error"}],\
        "failed_videos":[],"failed_audio_albums":[]}
        """
        let result = try decoder.decode(ThumbnailResult.self, from: Data(json.utf8))
        XCTAssertEqual(result.images.first?.path, "/a.jpg")
        XCTAssertEqual(result.failedImages.first?.path, "/b.jpg")
        XCTAssertEqual(result.failedImages.first?.mediaID, 7)
        XCTAssertEqual(result.failedImages.first?.reason, "thumbnail_error")

        let legacy = #"{"images":[],"videos":[],"audio_albums":[]}"#
        let legacyResult = try decoder.decode(ThumbnailResult.self, from: Data(legacy.utf8))
        XCTAssertTrue(legacyResult.failedImages.isEmpty)
        XCTAssertTrue(legacyResult.failedVideos.isEmpty)
        XCTAssertTrue(legacyResult.failedAudioAlbums.isEmpty)
    }

    func testSyncStatusDecodes() throws {
        // SyncStatusDto shape (sync.rs:75).
        let json = """
        {"profile_id":"photos","running":false,"monitoring":true,"last_run_at_ms":1234,\
        "last_error":null}
        """
        let status = try decoder.decode(SyncStatus.self, from: Data(json.utf8))
        XCTAssertEqual(status.profileID, "photos")
        XCTAssertFalse(status.running)
        XCTAssertTrue(status.monitoring)
        XCTAssertEqual(status.lastRunAtMs, 1234)
        XCTAssertNil(status.lastError)
    }

    func testRuntimeDiagnosticsDecodes() throws {
        // ffi/src/diagnostics.rs result JSON (P1-7: json_contract field).
        let json = """
        {"abi":"1.5.0","application_api":"1.0.0","json_contract":1,\
        "crate_version":"0.6.0",\
        "platform":"macos","arch":"aarch64","adb_path":"adb","adb_available":false,\
        "adb_version":null,"state_dir":null,"wire_log_enabled":false,\
        "active_sessions":0,"active_transfers":0,\
        "capabilities":["files","clipboard","trust","media","batch","sync",\
        "monitor","events","discovery","diagnostics","update_file_info","media_merge"]}
        """
        let diagnostics = try decoder.decode(RuntimeDiagnostics.self, from: Data(json.utf8))
        XCTAssertEqual(diagnostics.abi, "1.5.0")
        XCTAssertEqual(diagnostics.jsonContract, 1)
        XCTAssertEqual(diagnostics.platform, "macos")
        XCTAssertFalse(diagnostics.adbAvailable)
        XCTAssertNil(diagnostics.adbVersion)
        XCTAssertNil(diagnostics.stateDir)
        XCTAssertEqual(diagnostics.activeSessions, 0)
        XCTAssertTrue(diagnostics.capabilities.contains("sync"))
        // An older library without json_contract decodes as 0 (the
        // runtime-creation check then refuses it with .unsupported).
        let legacy = json.replacingOccurrences(
            of: #""json_contract":1,"#, with: ""
        )
        let legacyDiagnostics = try decoder.decode(
            RuntimeDiagnostics.self, from: Data(legacy.utf8)
        )
        XCTAssertEqual(legacyDiagnostics.jsonContract, 0)
        XCTAssertTrue(diagnostics.capabilities.contains("media_merge"))
    }

    func testTrustRecordDecodes() throws {
        // TrustRecordDto shape (trust.rs:16).
        let json = #"{"device_id":"phone:9a3f-77ee","device_name":"My Phone","updated_at_ms":1700000000000}"#
        let record = try decoder.decode(TrustRecord.self, from: Data(json.utf8))
        XCTAssertEqual(record.deviceID, "phone:9a3f-77ee")
        XCTAssertEqual(record.deviceName, "My Phone")
        XCTAssertEqual(record.updatedAtMs, 1_700_000_000_000)
    }

    // MARK: - Rust-generated event fixtures (P0-3)

    /// Reads a JSON fixture generated by the Rust side. The authoritative
    /// copy lives at crates/handshaker-application/tests/fixtures/ and is
    /// asserted byte-for-byte by the Rust test
    /// `probe_device_updated_fixture_json`; this copy must stay in sync.
    private func rustFixture(_ name: String) throws -> Data {
        guard let url = Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Fixtures") else {
            throw NSError(
                domain: "HandShakerCoreTests",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "missing fixture \(name).json — check Fixtures/ resources"]
            )
        }
        return try Data(contentsOf: url)
    }

    func testDeviceUpdatedEventDecodesFromRustFixture() throws {
        // Regression for P0-3: the Rust DeviceUpdated variant is a struct
        // payload — the descriptor lives under the nested "device" key.
        // Decoding from the Rust-generated fixture must succeed and the
        // re-encoded JSON must keep the same nested shape.
        let data = try rustFixture("event_device_updated")
        let envelope = try decoder.decode(EventEnvelope.self, from: data)
        guard case .deviceUpdated(let sessionID, let device) = envelope.event else {
            return XCTFail("expected deviceUpdated event, got \(envelope.event)")
        }
        XCTAssertEqual(sessionID, 7)
        XCTAssertEqual(device.id, "phone:9a3f-77ee")
        XCTAssertEqual(device.stableID, "phone:9a3f-77ee")
        XCTAssertEqual(device.displayName, "U2 Pro")
        XCTAssertEqual(device.transport, .wifi)

        // Re-encode: the shape must match the Rust fixture structurally
        // (nested "device" object, "session_id" key).
        let reencoded = try JSONEncoder().encode(envelope)
        guard let object = try JSONSerialization.jsonObject(with: reencoded) as? [String: Any] else {
            return XCTFail("re-encoded envelope is not a JSON object")
        }
        guard let event = object["event"] as? [String: Any] else {
            return XCTFail("re-encoded envelope has no event object")
        }
        XCTAssertEqual(event["kind"] as? String, "device_updated")
        let sessionIDValue = event["session_id"] as? Int
        XCTAssertEqual(sessionIDValue, 7)
        guard let deviceObject = event["device"] as? [String: Any] else {
            return XCTFail("re-encoded event must keep the nested device object")
        }
        let idValue = deviceObject["id"] as? String
        XCTAssertEqual(idValue, "phone:9a3f-77ee")
        let transportValue = deviceObject["transport"] as? String
        XCTAssertEqual(transportValue, "wifi")
    }
    /// DoD item 6 (stable release): every BackendEvent variant has an
    /// authoritative Rust-generated fixture (examples/gen_event_fixtures.rs).
    /// Decoding every fixture must succeed; the kind token and a few
    /// variant-specific fields must survive a decode → re-encode round trip.
    func testAllEventFixturesDecodeAndReencode() throws {
        let cases: [(String, String, [String: Any])] = [
            ("event_runtime_started", "runtime_started", [:]),
            ("event_runtime_stopping", "runtime_stopping", [:]),
            ("event_device_added", "device_added", ["id": "phone:9a3f-77ee"]),
            ("event_device_removed", "device_removed", ["device_id": "phone:9a3f-77ee"]),
            ("event_connection_lost", "connection_lost", ["session_id": 7]),
            ("event_clipboard_changed", "clipboard_changed", ["session_id": 7]),
            ("event_media_changed", "media_changed", ["session_id": 7]),
            ("event_remote_file_changed", "remote_file_changed", ["session_id": 7]),
            ("event_sync_watch_applied", "sync_watch_applied", ["profile_id": "photos", "session_id": 7]),
            ("event_warning", "warning", ["code": "remote_io"]),
            ("event_transfer_updated", "transfer_updated", ["id": 7, "direction": "download", "state": "running"]),
            ("event_session_state_changed", "session_state_changed", ["id": 7]),
        ]
        for (name, kind, fields) in cases {
            let data = try rustFixture(name)
            let envelope = try decoder.decode(EventEnvelope.self, from: data)
            XCTAssertEqual(envelope.sequence, 1, "\(name): envelope sequence")
            XCTAssertEqual(envelope.timestampMs, 1_700_000_000_000, "\(name): envelope timestamp")

            let reencoded = try JSONEncoder().encode(envelope)
            guard let object = try JSONSerialization.jsonObject(with: reencoded) as? [String: Any],
                  let event = object["event"] as? [String: Any] else {
                return XCTFail("\(name): re-encoded envelope is not a JSON object")
            }
            XCTAssertEqual(event["kind"] as? String, kind, "\(name): kind token")
            for (key, expected) in fields {
                let actual = event[key]
                switch expected {
                case let string as String:
                    XCTAssertEqual(actual as? String, string, "\(name): \(key)")
                case let int as Int:
                    XCTAssertEqual(actual as? Int, int, "\(name): \(key)")
                default:
                    XCTFail("\(name): unsupported expectation for \(key)")
                }
            }
        }
    }
}
