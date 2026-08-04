//
//  LXISHNativeBridge.h
//  LingxiCode
//
//  Minimal C ABI for the OpenMinis-derived iSH bridge.
//  This bridge is intended for Rust/host callers that prefer a JSON envelope
//  over direct Objective-C/Swift bindings.
//
//  JSON request keys:
//    config:
//      managed_root, workspace_host_path, stable_workspace_id, abi,
//      rootfs_version, archive_sha256?, authorization_file?
//    mount:
//      host_path, guest_path, read_only, purpose
//    run request:
//      command, args, cwd?, env, stdin?, timeout_ms?, mounts?
//    pty open request:
//      command, args, cwd?, env, cols, rows, mounts?
//    poll request:
//      after_sequence?, limit?
//
//  All string-returning functions return UTF-8 JSON allocated with `strdup`.
//  Callers must release them with `lx_ish_native_free_string`.
//

#pragma once

#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

bool lx_ish_native_is_available(void);

char *lx_ish_native_availability_json(void);
char *lx_ish_native_install_rootfs_json(const char *config_json);
char *lx_ish_native_repair_rootfs_json(const char *config_json);
char *lx_ish_native_reset_rootfs_json(const char *config_json);
char *lx_ish_native_boot_json(const char *config_json);
char *lx_ish_native_configure_mounts_json(const char *config_json, const char *mounts_json);
char *lx_ish_native_run_sync_json(const char *config_json, const char *request_json);
char *lx_ish_native_background_spawn_json(const char *config_json, const char *request_json);
char *lx_ish_native_background_kill_json(const char *config_json, const char *request_json);
char *lx_ish_native_background_poll_json(const char *config_json, const char *request_json);
char *lx_ish_native_probe_loopback_json(const char *config_json, const char *request_json);
char *lx_ish_native_pty_open_json(const char *config_json, const char *request_json);
char *lx_ish_native_pty_write_json(const char *config_json, const char *request_json);
char *lx_ish_native_pty_resize_json(const char *config_json, const char *request_json);
char *lx_ish_native_pty_close_json(const char *config_json, const char *request_json);
char *lx_ish_native_poll_output_json(const char *config_json, const char *request_json);

void lx_ish_native_free_string(char *value);

bool lingxi_ish_is_available(void);

char *lingxi_ish_availability_json(void);
char *lingxi_ish_install_rootfs_json(const char *config_json);
char *lingxi_ish_repair_rootfs_json(const char *config_json);
char *lingxi_ish_reset_rootfs_json(const char *config_json);
char *lingxi_ish_boot_json(const char *config_json);
char *lingxi_ish_configure_mounts_json(const char *config_json, const char *mounts_json);
char *lingxi_ish_run_json(const char *config_json, const char *request_json);
char *lingxi_ish_background_spawn_json(const char *config_json, const char *request_json);
char *lingxi_ish_background_kill_json(const char *config_json, const char *request_json);
char *lingxi_ish_background_poll_json(const char *config_json, const char *request_json);
char *lingxi_ish_probe_loopback_json(const char *config_json, const char *request_json);
char *lingxi_ish_pty_open_json(const char *config_json, const char *request_json);
char *lingxi_ish_pty_write_json(const char *config_json, const char *request_json);
char *lingxi_ish_pty_resize_json(const char *config_json, const char *request_json);
char *lingxi_ish_pty_close_json(const char *config_json, const char *request_json);
char *lingxi_ish_pty_poll_json(const char *config_json, const char *request_json);
char *lingxi_ish_pty_read_json(const char *config_json, const char *request_json);

void lingxi_ish_free_string(char *value);

#ifdef __cplusplus
}
#endif
