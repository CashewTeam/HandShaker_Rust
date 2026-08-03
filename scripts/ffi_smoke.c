/* Minimal C smoke test for the handshaker-ffi C ABI.
 * Build: clang -I crates/handshaker-ffi/include scripts/ffi_smoke.c \
 *        -L target/release -lhandshaker_ffi -o /tmp/ffi_smoke \
 *        -Wl,-rpath,$PWD/target/release
 * Run:   /tmp/ffi_smoke  (expect: "ffi smoke ok"; exit 0) */
#include "handshaker_ffi.h"
#include <stdio.h>
#include <string.h>

int main(void) {
    if (hs_abi_version_major() != 1 || hs_abi_version_minor() != 5) {
        fprintf(stderr, "unexpected ABI version\n");
        return 1;
    }

    HsRuntime *rt = NULL;
    const char *cfg = "{}";
    HsCallResult r = hs_runtime_create((const uint8_t *)cfg, strlen(cfg), &rt);
    if (r.status != 0 || rt == NULL) {
        fprintf(stderr, "runtime create failed\n");
        return 2;
    }
    hs_call_result_free(r);

    const char *req =
        "{\"include_adb\":false,\"include_wifi\":false,\"include_usb\":false}";
    r = hs_list_devices(rt, (const uint8_t *)req, strlen(req));
    if (r.status != 0 || r.value.len == 0) {
        fprintf(stderr, "list devices failed\n");
        return 3;
    }
    hs_call_result_free(r);

    /* ABI 1.3 surface: diagnostics and trust work without a device. */
    r = hs_runtime_diagnostics(rt);
    if (r.status != 0 || r.value.len == 0) {
        fprintf(stderr, "diagnostics failed\n");
        return 30;
    }
    hs_call_result_free(r);

    /* ABI 1.4 surface: sync status on an unknown profile is a stable
     * NotFound, and a NULL runtime yields InvalidArgument. */
    r = hs_sync_status(rt, (const uint8_t *)"phone:nope", 10);
    if (r.status == 0) {
        fprintf(stderr, "sync status must fail for unknown profile\n");
        return 34;
    }
    hs_call_result_free(r);

    r = hs_sync_plan(NULL, 1, (const uint8_t *)"{}", 2);
    if (r.status == 0) {
        fprintf(stderr, "sync plan with NULL runtime must fail\n");
        return 35;
    }
    hs_call_result_free(r);

    r = hs_trust_list(rt);
    if (r.status != 0 || r.value.len == 0) {
        fprintf(stderr, "trust list failed\n");
        return 31;
    }
    hs_call_result_free(r);

    /* ABI 1.3 surface: NULL runtime yields a stable InvalidArgument. */
    r = hs_stat_file(NULL, 1, (const uint8_t *)"{}", 2);
    if (r.status == 0) {
        fprintf(stderr, "stat with NULL runtime must fail\n");
        return 32;
    }
    hs_call_result_free(r);

    r = hs_runtime_shutdown(rt);
    if (r.status != 0) {
        fprintf(stderr, "shutdown failed\n");
        return 4;
    }
    hs_call_result_free(r);

    /* Idempotent shutdown. */
    r = hs_runtime_shutdown(rt);
    if (r.status != 0) {
        fprintf(stderr, "second shutdown failed\n");
        return 5;
    }
    hs_call_result_free(r);

    hs_runtime_destroy(rt);
    hs_runtime_destroy(NULL);
    hs_byte_buffer_free((HsByteBuffer){0, 0, 0});

    printf("ffi smoke ok\n");
    return 0;
}
