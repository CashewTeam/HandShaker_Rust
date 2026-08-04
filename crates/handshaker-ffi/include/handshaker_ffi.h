/* handshaker_ffi.h — stable C ABI for handshaker-application (M8 v1.5).
 *
 * ABI version: 1.5.0 (independent of the Rust crate version). 1.5 adds
 * update file info (hs_update_file_info) and the pure media incremental
 * merge (hs_media_merge_change); 1.4 added the photo-sync surface
 * (hs_sync_plan/start/status/stop/start_watch/stop_watch); 1.3 added file
 * stat/count/move/delete, clipboard, trust, device discovery, directory
 * monitor, batch transfers, media libraries/thumbnail/exif and runtime
 * diagnostics; 1.2 added hs_create_directory and hs_ping; 1.1 added the
 * transfer surface (hs_transfer_*); 1.0 symbols are unchanged.
 *
 * Ownership rules:
 *  - Rust allocates; Rust frees. Buffers returned in HsCallResult must be
 *    released with hs_call_result_free (or hs_byte_buffer_free).
 *  - An empty buffer is { NULL, 0, 0 }; freeing it is safe.
 *  - NULL handles are accepted by destroy/free functions and rejected with a
 *    stable InvalidArgument error by call functions.
 *  - Opaque handles (HsRuntime*, HsSubscription*) are owned by the caller and
 *    must be destroyed exactly once.
 *
 * Threading:
 *  - Short calls block the calling thread on the runtime's executor; call
 *    them from a background thread, never from a UI main thread.
 *  - hs_subscription_next blocks up to timeout_ms.
 *  - No unwind ever crosses the ABI: internal panics map to status != 0.
 *
 * Concurrency contract (P1-8):
 *  - hs_runtime_destroy MUST NOT run concurrently with any other call on
 *    the same runtime handle, and hs_subscription_destroy MUST NOT run
 *    concurrently with hs_subscription_next on the same subscription
 *    handle. The host is responsible for that synchronization; violating
 *    it is use-after-free and undefined behavior.
 *  - Ordinary calls (anything except destroy) MAY run concurrently on the
 *    same handle: the runtime serializes internally and every call takes
 *    a shared reference. The bundled Swift SDK satisfies the destroy
 *    rule with a lifecycle lease (in-flight counter; destroy drains and
 *    then frees), which is the reference pattern for other bindings.
 *  - A handle destroyed once is invalid for all further calls: call
 *    functions return a stable RuntimeClosed error. Destroying twice is
 *    a caller error (double-free).
 */
#ifndef HANDSHAKER_FFI_H
#define HANDSHAKER_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Owned byte buffer (Rust-allocated). */
typedef struct {
    uint8_t *ptr;
    size_t len;
    size_t capacity;
} HsByteBuffer;

/* Unified result: status == 0 means success (value holds UTF-8 JSON).
 * On failure error holds PublicError JSON and value is empty. */
typedef struct {
    int32_t status;
    HsByteBuffer value;
    HsByteBuffer error;
} HsCallResult;

typedef struct HsRuntime HsRuntime;
typedef struct HsSubscription HsSubscription;

/* ABI version. */
uint32_t hs_abi_version_major(void);
uint32_t hs_abi_version_minor(void);
uint32_t hs_abi_version_patch(void);

/* Buffers and results. */
void hs_byte_buffer_free(HsByteBuffer buffer);
void hs_call_result_free(HsCallResult result);

/* Runtime lifecycle. config_json: FfiRuntimeConfig JSON, e.g.
 * {"adb_path_utf8":"adb","default_timeout_ms":30000,
 *  "heartbeat_interval_ms":10000,"event_capacity":1024} (all optional).
 * out_runtime is written only on success. */
HsCallResult hs_runtime_create(const uint8_t *config_json, size_t config_len,
                               HsRuntime **out_runtime);
HsCallResult hs_runtime_shutdown(HsRuntime *runtime); /* idempotent; NULL -> ok */
void hs_runtime_destroy(HsRuntime *runtime);          /* NULL safe */

/* Devices. request_json: {"include_adb":true,"include_wifi":true,
 * "include_usb":true,"wifi_browse_timeout_ms":3000} (all optional).
 * Result: JSON array of DeviceDescriptor. */
HsCallResult hs_list_devices(HsRuntime *runtime, const uint8_t *request_json,
                             size_t request_len);
/* Discovery (ABI 1.3). No request body. Result: DeviceDiscoveryResult
 * {"devices":[...],"warnings":[...]} — per-channel failures are reported
 * as warnings instead of an empty-array lie. */
HsCallResult hs_discover_devices(HsRuntime *runtime);

/* Sessions. connect request_json: a full DeviceDescriptor (as returned by
 * hs_list_devices). Result: {"session_id": N}. */
HsCallResult hs_connect(HsRuntime *runtime, const uint8_t *request_json,
                        size_t request_len);
HsCallResult hs_disconnect(HsRuntime *runtime, uint64_t session_id);
HsCallResult hs_get_session(HsRuntime *runtime, uint64_t session_id);

/* Files. request_json: {"path":"/storage/emulated/0","depth":1} (optional).
 * Result: JSON array of FileEntryDto. */
HsCallResult hs_list_files(HsRuntime *runtime, uint64_t session_id,
                           const uint8_t *request_json, size_t request_len);

/* Files (ABI 1.2). request_json: {"path":"/sdcard/new"}.
 * Result: {"created":true}. */
HsCallResult hs_create_directory(HsRuntime *runtime, uint64_t session_id,
                                 const uint8_t *request_json, size_t request_len);

/* Session (ABI 1.2). Result: {"round_trip_ms": N}. */
HsCallResult hs_ping(HsRuntime *runtime, uint64_t session_id);

/* Files (ABI 1.3). stat request_json: {"path":"/sdcard/a.txt"} (path
 * optional, default "."). Result: {"file": FileEntryDto|null}.
 * count request_json: {"path":"/sdcard","depth":1,"exclusions":[...]}
 * (all optional, depth default 1). Result: {"count": N}.
 * move request_json: {"source":"/a","target":"/b"}.
 * Result: {"moved":true}.
 * delete request_json: {"paths":["/a","/b"],"trash":false,"sync":false}
 * (trash/sync optional, default false). Result: DeleteResultDto. */
HsCallResult hs_stat_file(HsRuntime *runtime, uint64_t session_id,
                          const uint8_t *request_json, size_t request_len);
HsCallResult hs_count_files(HsRuntime *runtime, uint64_t session_id,
                            const uint8_t *request_json, size_t request_len);
HsCallResult hs_move_path(HsRuntime *runtime, uint64_t session_id,
                          const uint8_t *request_json, size_t request_len);
HsCallResult hs_delete_paths(HsRuntime *runtime, uint64_t session_id,
                             const uint8_t *request_json, size_t request_len);
/* update file info (ABI 1.5). request_json:
 * {"files":[{"path":"/sdcard/a.jpg","size":1024,"is_directory":false,
 *   "created_at":123,"modified_at":456,"checksum":null,"is_trash":null,
 *   "id":7,"ext_data":null}],"is_sync":false}
 * (files/is_sync optional; session_id always comes from the call
 * argument). The phone writes the reported fields back into its media
 * store. Result: {"updated":true}. */
HsCallResult hs_update_file_info(HsRuntime *runtime, uint64_t session_id,
                                 const uint8_t *request_json, size_t request_len);

/* Directory monitor (ABI 1.3). request_json:
 * {"path":"/sdcard/DCIM","enabled":true} (enabled optional, default true).
 * Result: {"registered":true}. Events arrive as RemoteFileChanged. */
HsCallResult hs_monitor_folder(HsRuntime *runtime, uint64_t session_id,
                               const uint8_t *request_json, size_t request_len);

/* Clipboard (ABI 1.3). hs_clipboard_list takes no request body; result:
 * JSON array of ClipboardEntryDto. set request_json: {"text":"..."},
 * result: {"set":true}. delete request_json: {"timestamp_ms":123},
 * result: {"deleted":true}. clear result: {"cleared":true}. */
HsCallResult hs_clipboard_list(HsRuntime *runtime, uint64_t session_id);
HsCallResult hs_clipboard_set(HsRuntime *runtime, uint64_t session_id,
                              const uint8_t *request_json, size_t request_len);
HsCallResult hs_clipboard_delete(HsRuntime *runtime, uint64_t session_id,
                                 const uint8_t *request_json, size_t request_len);
HsCallResult hs_clipboard_clear(HsRuntime *runtime, uint64_t session_id);

/* Trust (ABI 1.3, no session). hs_trust_list result: JSON array of
 * TrustRecordDto. remove request_json: {"device_id":"phone:xxx"},
 * result: {"removed":true}. reset request_json:
 * {"endpoint":"192.168.1.5:5555","expected_device_id":"phone:xxx"},
 * result: {"reset":true}. */
HsCallResult hs_trust_list(HsRuntime *runtime);
HsCallResult hs_trust_remove(HsRuntime *runtime, const uint8_t *request_json,
                             size_t request_len);
HsCallResult hs_trust_reset(HsRuntime *runtime, const uint8_t *request_json,
                            size_t request_len);

/* Transfers (ABI 1.1). request_json (start_*):
 * {"remote_path":"/sdcard/a.bin","local_path":"/tmp/a.bin","overwrite":false}
 * (overwrite optional, default false). Result: {"transfer_id": N}.
 * hs_transfer_get/list result JSON: TransferSnapshot (array for list).
 * hs_transfer_cancel result: {"cancelled": true}. */
HsCallResult hs_transfer_start_download(HsRuntime *runtime, uint64_t session_id,
                                        const uint8_t *request_json, size_t request_len);
HsCallResult hs_transfer_start_upload(HsRuntime *runtime, uint64_t session_id,
                                      const uint8_t *request_json, size_t request_len);
HsCallResult hs_transfer_cancel(HsRuntime *runtime, uint64_t transfer_id);
HsCallResult hs_transfer_get(HsRuntime *runtime, uint64_t transfer_id);
HsCallResult hs_transfer_list(HsRuntime *runtime);

/* Batch transfers (ABI 1.3). request_json (start_batch_*):
 * {"files":[{"source":"/a","target":"/b"}],
 *  "trees":[{"source":"/dir","target":"/dir"}],"overwrite":false}
 * (files/trees/overwrite optional). Result: {"transfer_id": N}.
 * Progress, cancellation and the final result ride the existing
 * hs_transfer_get/list/cancel: TransferSnapshot carries
 * item_count/completed_items/failed_items/current_item/batch_result. */
HsCallResult hs_transfer_start_batch_download(HsRuntime *runtime,
                                              uint64_t session_id,
                                              const uint8_t *request_json,
                                              size_t request_len);
HsCallResult hs_transfer_start_batch_upload(HsRuntime *runtime,
                                            uint64_t session_id,
                                            const uint8_t *request_json,
                                            size_t request_len);

/* Media (ABI 1.3). Library calls take request_json "{}" (reserved for
 * future paging); results are the PhotoLibraryDto/VideoLibraryDto/
 * AudioLibraryDto JSON. Thumbnail request_json:
 * {"images":[{"path":"/a.jpg"}],"videos":[...],"audio_albums":[...]}
 * (all optional); thumbnail bytes are cached on disk under
 * <state_dir>/thumbnails/ and the result carries cache paths:
 * {"images":[{"path":"/a.jpg","cache_path":"/abs/path","size":N}],...}
 * EXIF request_json: {"path":"/a.jpg"}; result: ExifDataDto. */
HsCallResult hs_media_photo_library(HsRuntime *runtime, uint64_t session_id,
                                    const uint8_t *request_json, size_t request_len);
HsCallResult hs_media_video_library(HsRuntime *runtime, uint64_t session_id,
                                    const uint8_t *request_json, size_t request_len);
HsCallResult hs_media_audio_library(HsRuntime *runtime, uint64_t session_id,
                                    const uint8_t *request_json, size_t request_len);
HsCallResult hs_media_thumbnail(HsRuntime *runtime, uint64_t session_id,
                                const uint8_t *request_json, size_t request_len);
HsCallResult hs_media_fetch_exif(HsRuntime *runtime, uint64_t session_id,
                                 const uint8_t *request_json, size_t request_len);
/* Media incremental merge (ABI 1.5, pure function, no session/device).
 * kind is "photo"|"video"|"audio"; library_json is the current
 * PhotoLibraryDto/VideoLibraryDto/AudioLibraryDto; change_json is a
 * MediaChangeDto
 * {"media_kind":"photo","added":[...],"deleted":[...],"updated":[...]}
 * whose media_kind must match kind. Entries are upserted by media_id
 * (fallback path), preserving snapshot-only fields (thumbnail/star/GPS);
 * deleted entries are removed by the same key. Result: the merged library
 * DTO JSON. Errors: InvalidArgument (bad kind/JSON), InvalidState (kind
 * mismatch). */
HsCallResult hs_media_merge_change(HsRuntime *runtime, const uint8_t *kind,
                                   size_t kind_len, const uint8_t *library_json,
                                   size_t library_len, const uint8_t *change_json,
                                   size_t change_len);

/* Diagnostics (ABI 1.3, no session). Result JSON: abi/application_api/
 * crate_version/platform/arch/adb_path/adb_available/adb_version/
 * state_dir/wire_log_enabled/active_sessions/active_transfers/
 * capabilities. adb probing never fails the call. */
HsCallResult hs_runtime_diagnostics(HsRuntime *runtime);

/* Photo sync (ABI 1.4). hs_sync_plan/start take a SyncProfileDto request
 * {"id":"<optional, default device_uuid>",
 *  "device_uuid":"<uuid, optional 'phone:' prefix stripped>",
 *  "remote_root":"<optional, default camera folder>",
 *  "local_root":"/abs/path","enabled":true}
 * (session id always comes from the call argument; device_uuid must be
 * [A-Za-z0-9_-]+, ids and paths are length-capped). hs_sync_plan result:
 * SyncPlanDto. hs_sync_start launches the run in the background and
 * returns {"profile_id":"<id>"}; progress is polled with hs_sync_status
 * (SyncStatusDto) or observed via events (SyncWatchApplied/Transfer-
 * Updated/Warning). hs_sync_stop result: {"stopped":true}.
 * hs_sync_start_watch requires a finished run (phone in SYNCING state;
 * poll hs_sync_status until running:false), then applies debounced
 * batches as SyncWatchApplied events; result: {"started":true}.
 * hs_sync_stop_watch result: {"stopped":true}. Errors: NotFound for an
 * unknown profile id. */
HsCallResult hs_sync_plan(HsRuntime *runtime, uint64_t session_id,
                          const uint8_t *request_json, size_t request_len);
HsCallResult hs_sync_start(HsRuntime *runtime, uint64_t session_id,
                           const uint8_t *request_json, size_t request_len);
HsCallResult hs_sync_status(HsRuntime *runtime, const uint8_t *profile_id,
                            size_t profile_id_len);
HsCallResult hs_sync_stop(HsRuntime *runtime, const uint8_t *profile_id,
                          size_t profile_id_len);
HsCallResult hs_sync_start_watch(HsRuntime *runtime, const uint8_t *profile_id,
                                 size_t profile_id_len);
HsCallResult hs_sync_stop_watch(HsRuntime *runtime, const uint8_t *profile_id,
                                size_t profile_id_len);

/* Events (queue-pull). hs_subscription_next returns EventEnvelope JSON;
 * on timeout: {"timeout":true}; after runtime shutdown: {"closed":true}.
 * Lagged subscribers get a status != 0 result. */
HsCallResult hs_subscribe_events(HsRuntime *runtime,
                                 HsSubscription **out_subscription);
HsCallResult hs_subscription_next(HsSubscription *subscription,
                                  uint32_t timeout_ms);
void hs_subscription_destroy(HsSubscription *subscription); /* NULL safe */

#ifdef __cplusplus
}
#endif

#endif /* HANDSHAKER_FFI_H */
