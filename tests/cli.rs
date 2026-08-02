use std::process::Command;

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
    assert!(help.contains(handshaker_rust::i18n::text("cli.command.ls")));
    assert!(help.contains(handshaker_rust::i18n::text("cli.arg.path")));
    assert!(!help.contains("Print this message"));
    assert!(!help.contains("[default:"));
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
    assert_eq!(envelope["data"][0]["serial"], "ABC123");
    assert_eq!(envelope["data"][0]["model"], "DE106");

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
    assert_eq!(
        String::from_utf8(human.stdout).expect("UTF-8 output"),
        format!(
            "{}\nABC123\tdevice\tDE106\tosborn\n",
            handshaker_rust::i18n::text("device.list_header")
        )
    );
    assert_eq!(
        std::fs::read_to_string(calls).expect("calls"),
        "devices -l\ndevices -l\n",
        "device list must not start a service or create a forward"
    );
}
