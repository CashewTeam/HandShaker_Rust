//! USB AOA end-to-end acceptance: one connection, full business sequence.
//! Run: cargo run --release --example usb_accept
use std::path::Path;
use std::time::Duration;

use handshaker_core::{
    ClientOptions, ConnectionTarget, DeleteOptions, HandShakerClient, TransferOptions,
};

const TEST_DIR: &str = "/storage/emulated/0/hs_m7_test";

fn check(name: &str, ok: bool) {
    println!("{name}: {}", if ok { "PASS" } else { "FAIL" });
}

#[tokio::main]
async fn main() {
    let client = match HandShakerClient::connect(
        ConnectionTarget::Usb { location_id: None },
        ClientOptions {
            timeout: Duration::from_secs(15),
            ..Default::default()
        },
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            println!("connect FAILED: {e}");
            return;
        }
    };
    println!(
        "connect: PASS ({} serial={} root={})",
        client.device_info().name.clone().unwrap_or_default(),
        client.device_info().serial,
        client.root_path()
    );

    check("ping", client.ping().await.is_ok());

    check(
        "list_dir",
        matches!(client.list_dir("/storage/emulated/0", 1).await, Ok(f) if !f.is_empty()),
    );

    check("mkdir", client.create_dir(TEST_DIR).await.is_ok());

    // Upload 8 KiB deterministic payload from a temp file.
    let payload: Vec<u8> = (0..8192u32).map(|i| (i * 31 % 251) as u8).collect();
    std::fs::write("/tmp/hs-m7-payload.bin", &payload).expect("write payload");
    let remote = format!("{TEST_DIR}/u.bin");
    check(
        "upload",
        client
            .upload(
                Path::new("/tmp/hs-m7-payload.bin"),
                &remote,
                TransferOptions::default(),
            )
            .await
            .is_ok(),
    );

    // Download back and verify byte equality.
    let local = "/tmp/hs-m7-downloaded.bin";
    match client
        .download(&remote, Path::new(local), TransferOptions::default())
        .await
    {
        Ok(n) => println!("download: PASS ({n} bytes)"),
        Err(e) => println!("download: FAIL ({e})"),
    }
    let downloaded = std::fs::read(local).unwrap_or_default();
    check("verify_md5", downloaded == payload);
    println!(
        "  downloaded {} bytes, equal={}",
        downloaded.len(),
        downloaded == payload
    );

    check(
        "rename",
        client
            .rename(&remote, &format!("{TEST_DIR}/u2.bin"))
            .await
            .is_ok(),
    );

    check(
        "clipboard_set",
        client.clipboard_set("hello-usb").await.is_ok(),
    );
    let clip = client.clipboard_list().await;
    check(
        "clipboard_get",
        matches!(&clip, Ok(list) if list.iter().any(|e| e.text == "hello-usb")),
    );
    if let Ok(list) = &clip {
        println!(
            "  clipboard entries: {:?}",
            list.iter().map(|e| e.text.clone()).collect::<Vec<_>>()
        );
    }

    check(
        "cleanup",
        client
            .delete(
                &[TEST_DIR.to_string()],
                DeleteOptions {
                    trash: false,
                    sync: false,
                },
            )
            .await
            .is_ok(),
    );

    match client.close().await {
        Ok(()) => println!("quit: PASS"),
        Err(e) => println!("quit err {e} (tolerated)"),
    }
}
