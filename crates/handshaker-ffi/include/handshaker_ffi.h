/* handshaker_ffi.h — stable C ABI for handshaker-application (M8 v1.1).
 *
 * ABI version: 1.1.0 (independent of the Rust crate version). 1.1 adds the
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
