use std::env;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use handshaker_core::i18n;
use handshaker_core::{
    ClientEvent, ClientOptions, ConnectionTarget, EventCallbacks, EventFilter, HandShakerClient,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let serial = env::var("HANDSHAKER_M1_SERIAL")?;
    let marker = format!(
        "HandShaker_M1_{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    );
    let client = HandShakerClient::connect_with_event_callbacks(
        ConnectionTarget::Adb {
            serial: Some(serial),
        },
        ClientOptions::default(),
        EventCallbacks {
            device_info: true,
            photo_library: true,
            audio_library: true,
            video_library: true,
        },
    )
    .await?;
    let mut events = client.subscribe_events(EventFilter::all());
    let mut device_event = false;
    let device_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < device_deadline {
        let remaining = device_deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Ok(ClientEvent::DeviceInfoChanged(_))) => {
                device_event = true;
                break;
            }
            Ok(Ok(_)) | Ok(Err(_)) => {}
            Err(_) => break,
        }
    }

    client.clipboard_set(&marker).await?;
    let mut event_timestamp = None;
    let clipboard_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < clipboard_deadline {
        let remaining = clipboard_deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Ok(ClientEvent::ClipboardChanged(entries))) => {
                event_timestamp = entries
                    .into_iter()
                    .find(|entry| entry.text == marker)
                    .map(|entry| entry.timestamp_ms);
                if event_timestamp.is_some() {
                    break;
                }
            }
            Ok(Ok(_)) | Ok(Err(_)) => {}
            Err(_) => break,
        }
    }

    let timestamp = match event_timestamp {
        Some(timestamp) => timestamp,
        None => client
            .clipboard_list()
            .await?
            .into_iter()
            .find(|entry| entry.text == marker)
            .map(|entry| entry.timestamp_ms)
            .ok_or(i18n::text("m1.marker_missing"))?,
    };
    client.clipboard_delete(timestamp).await?;
    client.close().await?;
    println!(
        "{}",
        i18n::format("m1.device_event", &[&device_event.to_string()])
    );
    println!(
        "{}",
        i18n::format(
            "m1.clipboard_event",
            &[&event_timestamp.is_some().to_string()]
        )
    );
    println!("{}", i18n::text("m1.clipboard_deleted"));
    Ok(())
}
