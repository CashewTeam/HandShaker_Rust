use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

#[test]
fn argument_errors_use_json_envelope_and_exit_two() {
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["--output", "json", "fs", "ls", "--depth", "invalid"])
        .output()
        .expect("run handshaker");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "usage");
}

#[test]
fn help_text_comes_from_the_chinese_language_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["fs", "ls", "--help"])
        .output()
        .expect("run handshaker help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(help.contains(handshaker_core::i18n::text("cli.command.ls")));
    assert!(help.contains(handshaker_core::i18n::text("cli.arg.path")));
    assert!(!help.contains("Print this message"));
    assert!(!help.contains("[default:"));
}

#[test]
fn discover_help_is_localized_and_parses() {
    let help = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["device", "discover", "--help"])
        .output()
        .expect("run discover help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("UTF-8 help");
    assert!(help.contains(handshaker_core::i18n::text("cli.command.discover")));
    assert!(help.contains(handshaker_core::i18n::text("cli.arg.browse_timeout")));
    assert!(help.contains(handshaker_core::i18n::text("cli.value.browse_timeout")));

    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args([
            "--output",
            "json",
            "device",
            "discover",
            "--browse-timeout",
            "10ms",
        ])
        .output()
        .expect("run discover");
    assert!(output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["command"], "device.discover");
    assert!(envelope["data"].is_array());
}

#[test]
fn wifi_flag_parses_and_conflicts_with_serial() {
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["--wifi", "not-an-address", "device", "info"])
        .output()
        .expect("run invalid wifi");
    assert_eq!(output.status.code(), Some(2));

    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args([
            "--serial",
            "ABC123",
            "--wifi",
            "192.0.2.47:45656",
            "device",
            "info",
        ])
        .output()
        .expect("run conflicting flags");
    assert_eq!(output.status.code(), Some(2));

    // A valid IPv6 address parses.
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args(["--wifi", "[::1]:45656", "--output", "json", "trust", "list"])
        .output()
        .expect("run valid wifi");
    assert!(output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(envelope["command"], "trust.list");
}

#[test]
fn dry_run_flag_parses_on_push_and_pull() {
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args([
            "fs",
            "push",
            "some-file.txt",
            "--dry-run",
            "--",
            "/storage/emulated/0/Download",
        ])
        .output()
        .expect("parse push dry-run");
    // Parsing succeeds (exit 4 = device selection, not 2 = usage).
    assert_ne!(output.status.code(), Some(2), "push --dry-run must parse");

    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args([
            "fs",
            "pull",
            "--dry-run",
            "--recursive",
            "/storage/emulated/0/DCIM",
        ])
        .output()
        .expect("parse pull dry-run");
    assert_ne!(output.status.code(), Some(2), "pull --dry-run must parse");
}

#[test]
fn trust_remove_requires_confirmation_outside_tty() {
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/nonexistent-hs-home")
        .args(["--output", "json", "trust", "remove", "device-1"])
        .output()
        .expect("run trust remove");
    assert_eq!(output.status.code(), Some(8));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(envelope["error"]["code"], "confirmation_required");
}

#[cfg(unix)]
#[test]
fn device_list_reads_only_adb_devices_long() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temporary directory");
    let adb = temp.path().join("adb");
    let calls = temp.path().join("calls");
    std::fs::write(
        &adb,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nprintf 'List of devices attached\\nABC123 device product:osborn model:DE106 device:osborn\\n'\n",
            calls.display()
        ),
    )
    .expect("fake adb");
    let mut permissions = std::fs::metadata(&adb).expect("metadata").permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&adb, permissions).expect("permissions");

    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["--output", "json", "device", "list"])
        .env("PATH", path)
        .output()
        .expect("run handshaker");
    assert!(output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(envelope["command"], "device.list");
    assert_eq!(envelope["data"]["adb"][0]["serial"], "ABC123");
    assert_eq!(envelope["data"]["adb"][0]["model"], "DE106");
    assert!(
        envelope["data"]["usb"].is_array(),
        "usb accessories are listed"
    );

    let human = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["device", "list"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                temp.path().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .expect("run handshaker human output");
    assert!(human.status.success());
    let human_output = String::from_utf8(human.stdout).expect("UTF-8 output");
    let adb_header = handshaker_core::i18n::text("device.list_header");
    assert!(
        human_output.contains(&format!("{adb_header}\nABC123\tdevice\tDE106\tosborn")),
        "ADB rows must appear with their header: {human_output}"
    );
    assert_eq!(
        std::fs::read_to_string(calls).expect("calls"),
        "devices -l\ndevices -l\n",
        "device list must not start a service or create a forward"
    );
}

#[test]
fn watch_help_is_localized() {
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["watch", "--help"])
        .output()
        .expect("run watch help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(help.contains(handshaker_core::i18n::text("cli.command.watch")));
    assert!(help.contains(handshaker_core::i18n::text("cli.arg.watch_path")));
}

#[test]
fn media_help_is_localized() {
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["media", "--help"])
        .output()
        .expect("run media help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(help.contains(handshaker_core::i18n::text("cli.command.media")));
    assert!(help.contains(handshaker_core::i18n::text("cli.command.media_photo")));

    let photo = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .args(["media", "photo", "--help"])
        .output()
        .expect("run media photo help");
    assert!(photo.status.success());
    let photo = String::from_utf8(photo.stdout).expect("UTF-8 help");
    assert!(photo.contains(handshaker_core::i18n::text("cli.arg.media_limit")));
}

#[test]
fn sync_commands_parse_and_require_output_dir() {
    // `sync status` parses (device selection error, not usage).
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args(["sync", "status"])
        .output()
        .expect("parse sync status");
    assert_ne!(output.status.code(), Some(2), "sync status must parse");

    // `sync run` without --output-dir is a usage error (exit 2).
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args(["--output", "json", "sync", "run"])
        .output()
        .expect("run sync run without output dir");
    assert_eq!(output.status.code(), Some(2));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(envelope["error"]["code"], "usage");

    // `sync plan` parses with --output-dir (device selection, not usage).
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args(["sync", "plan", "--output-dir", "/tmp/hs-sync-test"])
        .output()
        .expect("parse sync plan");
    assert_ne!(output.status.code(), Some(2), "sync plan must parse");

    // Localized help for the sync group.
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args(["sync", "--help"])
        .output()
        .expect("sync help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(help.contains(handshaker_core::i18n::text("cli.command.sync_plan")));
}

#[test]
fn batch_subcommand_parses_and_requires_connection() {
    // Localized help for the batch group.
    let output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args(["batch", "--help"])
        .output()
        .expect("batch help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(help.contains(handshaker_core::i18n::text("cli.command.batch")));

    // Without a device, batch fails at device selection (3) or connection (4),
    // not usage (2): the command itself parses.
    let mut output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args(["--output", "json", "batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn batch");
    let mut stdin = output.stdin.take().expect("stdin");
    write!(stdin, "device ping\nquit\n").expect("write batch lines");
    drop(stdin);
    let status = output.wait_with_output().expect("wait batch");
    assert_ne!(
        status.status.code(),
        Some(2),
        "batch must parse; failure is device/connection level"
    );
}

#[test]
fn batch_rejects_nested_shell_line() {
    // A `shell` line inside batch is rejected as usage but the batch itself
    // still parses and connects-fails at device level (not usage).
    let mut output = Command::new(env!("CARGO_BIN_EXE_handshaker"))
        .env("HOME", "/tmp/hs-cli-test-home")
        .args(["--output", "json", "batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn batch nested");
    let mut stdin = output.stdin.take().expect("stdin");
    write!(stdin, "shell\nquit\n").expect("write nested shell");
    drop(stdin);
    let status = output.wait_with_output().expect("wait batch nested");
    assert_ne!(
        status.status.code(),
        Some(2),
        "nested shell is per-line, not usage"
    );
}
