//! USB AOA (Android Open Accessory) transport.
//!
//! The phone's Smartisan accessory presents a bulk byte stream that carries
//! the exact same SSP framing as ADB/WiFi (docs/03 §3.1, verified against the
//! original 1.2.0 APK: `service/a.java` opens the accessory ParcelFileDescriptor
//! and wraps it in FileInputStream/FileOutputStream). The host side mirrors the
//! Mac client's `SFUSBDevice` (libusb handle + bulkIn/bulkOut endpoints).
//!
//! Accessory-mode devices enumerate as VID 0x18d1 with a vendor-specific
//! interface (class 0xff) exposing one bulk IN and one bulk OUT endpoint.
//! The Smartisan ROM keeps the phone in accessory mode by default
//! (functions=accessory,ffs; PID 0x2d01 observed on OD103), so no
//! ACCESSORY identification/START control transfer is needed for this
//! milestone. See docs/23-m7-usb-aoa.md for the known limitations.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::error::{Error, Result};
use crate::i18n;
use crate::transport::{ConnectedTransport, TransportCleanup};

/// Accessory-mode vendor id (Google's accessory VID, per AOA spec).
pub(crate) const ACCESSORY_VID: u16 = 0x18d1;
/// Accessory-mode product ids: 0x2d00 accessory, 0x2d01 accessory+adb,
/// 0x2d02 accessory+audio, 0x2d03 accessory+audio+adb.
const ACCESSORY_PIDS: &[u16] = &[0x2d00, 0x2d01, 0x2d02, 0x2d03];
/// Smartisan's plain (non-accessory) USB vendor id; the host must send the
/// AOA identification + ACCESSORY_START to flip the device into accessory
/// mode (VID 0x18d1). Observed on OD103 as VID 0x29a9 / PID 0x7020.
pub(crate) const SMARTISAN_VID: u16 = 0x29a9;
/// AOA control-transfer requests (see the AOA spec / mac SFUSBDevice).
/// Note: these are *decimal* request numbers (0x33=51 GET_PROTOCOL,
/// 0x34=52 SEND_STRING, 0x35=53 START), matching libusb_control_transfer's
/// bRequest parameter as used by the Mac client's sendAOAStartupRequest.
const ACC_GET_PROTOCOL: u8 = 0x33;
const ACC_SEND_STRING: u8 = 0x34;
const ACC_START: u8 = 0x35;
/// Identification strings matching the phone's accessory_filter.xml
/// (manufacturer=Smartisan model=HandShaker version=1) and the Mac client.
/// The Mac client (sendAOAStartupRequest) sends these at *zero-based*
/// indexes 0..=4 (manufacturer/model/description/version/host-uuid); the
/// optional URI slot (index 5) is not used.
const ACC_MANUFACTURER: &str = "Smartisan";
const ACC_MODEL: &str = "HandShaker";
const ACC_DESCRIPTION: &str = "HandShaker";
const ACC_VERSION: &str = "1.0";
/// AOA bulk interface is vendor-specific (class 0xff, subclass 0xff,
/// protocol 0x00 on this ROM).
const ACCESSORY_IFACE_CLASS: u8 = 0xff;
const ACCESSORY_IFACE_SUBCLASS: u8 = 0xff;
/// Bulk transfer timeout used for both directions.
const BULK_TIMEOUT: Duration = Duration::from_secs(5);
/// Read chunk size; mirrors the phone's 16 KiB BufferedInputStream.
const READ_CHUNK: usize = 16 * 1024;

/// Whether the device is already in accessory mode or still in plain
/// Smartisan mode (needs identification + ACCESSORY_START).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum AccessoryMode {
    /// Already at VID 0x18d1, ready for bulk transfers.
    Accessory,
    /// Plain Smartisan VID; the connector switches it before connecting.
    Plain,
}

/// A discovered USB device that can host a HandShaker SSP connection.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UsbAccessory {
    /// Stable-ish identifier: `bus-port1[-port2]` (macOS USB location path).
    pub location: String,
    pub bus_number: u8,
    pub serial: Option<String>,
    pub vendor_id: u16,
    pub product_id: u16,
    pub mode: AccessoryMode,
}

/// Enumerate accessory-mode USB devices.
pub fn list_accessories() -> Result<Vec<UsbAccessory>> {
    let devices = rusb::devices().map_err(|error| {
        Error::LocalIo(i18n::format("usb.enumerate_failed", &[&error.to_string()]))
    })?;
    let mut out = Vec::new();
    for device in devices.iter() {
        let descriptor = match device.device_descriptor() {
            Ok(descriptor) => descriptor,
            Err(_) => continue,
        };
        if !(descriptor.vendor_id() == ACCESSORY_VID
            && ACCESSORY_PIDS.contains(&descriptor.product_id())
            || descriptor.vendor_id() == SMARTISAN_VID)
        {
            continue;
        }
        let mode = if descriptor.vendor_id() == ACCESSORY_VID {
            AccessoryMode::Accessory
        } else {
            // Any Smartisan VID device needs the AOA identification +
            // ACCESSORY_START flow: even though the ROM keeps the accessory
            // data interface active in its default configuration (observed on
            // OD103 as VID 0x29a9 / PID 0x7020), no accessory *session*
            // exists until identification makes the device re-enumerate as
            // 0x18d1 and the phone app open the accessory.
            AccessoryMode::Plain
        };
        let ports = device.port_numbers().unwrap_or_default();
        let location = format!(
            "{}-{}",
            device.bus_number(),
            ports
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join("-")
        );
        let serial = device
            .open()
            .ok()
            .and_then(|handle| handle.read_serial_number_string_ascii(&descriptor).ok())
            .filter(|value| !value.is_empty());
        out.push(UsbAccessory {
            location,
            bus_number: device.bus_number(),
            serial,
            vendor_id: descriptor.vendor_id(),
            product_id: descriptor.product_id(),
            mode,
        });
    }
    Ok(out)
}

/// Find the bulk IN/OUT endpoints of the accessory data interface.
///
/// The Smartisan ROM presents several vendor interfaces (0xff/0xff) in its
/// composite gadget; the live accessory interface is the one with exactly one
/// bulk IN and one bulk OUT endpoint (observed on OD103: interface 1,
/// endpoints 0x83 IN / 0x02 OUT; the 3-endpoint interface 0 accepts no bulk
/// traffic). Candidates are collected across all interfaces, preferring the
/// 1-IN/1-OUT shape, and falling back to the first vendor interface that has
/// both directions.
fn accessory_endpoints<T: rusb::UsbContext>(device: &rusb::Device<T>) -> Result<(u8, u8, u8)> {
    let config = device.active_config_descriptor().map_err(|error| {
        Error::LocalIo(i18n::format("usb.config_failed", &[&error.to_string()]))
    })?;
    let mut candidates: Vec<(u8, u8, u8)> = Vec::new();
    for interface in config.interfaces() {
        for alt in interface.descriptors() {
            if alt.class_code() != ACCESSORY_IFACE_CLASS
                || alt.sub_class_code() != ACCESSORY_IFACE_SUBCLASS
            {
                continue;
            }
            let mut bulk_in = None;
            let mut bulk_out = None;
            let mut extra_in = false;
            for endpoint in alt.endpoint_descriptors() {
                if endpoint.transfer_type() != rusb::TransferType::Bulk {
                    continue;
                }
                match endpoint.direction() {
                    rusb::Direction::In => {
                        if bulk_in.is_some() {
                            extra_in = true;
                        } else {
                            bulk_in = Some(endpoint.address());
                        }
                    }
                    rusb::Direction::Out => {
                        if bulk_out.is_none() {
                            bulk_out = Some(endpoint.address());
                        }
                    }
                }
            }
            if let (Some(bulk_in), Some(bulk_out)) = (bulk_in, bulk_out) {
                if !extra_in {
                    // Exactly one IN + one OUT: the accessory data shape.
                    return Ok((interface.number(), bulk_in, bulk_out));
                }
                candidates.push((interface.number(), bulk_in, bulk_out));
            }
        }
    }
    if let Some((interface_number, bulk_in, bulk_out)) = candidates.first() {
        return Ok((*interface_number, *bulk_in, *bulk_out));
    }
    Err(Error::LocalIo(
        i18n::text("usb.no_bulk_endpoints").to_string(),
    ))
}

/// Send the AOA identification strings + ACCESSORY_START so the phone
/// re-enumerates as VID 0x18d1 accessory mode. Mirrors the Mac client's
/// `sendAOAStartupRequest` (SmartFinderCore): GET_PROTOCOL failures are
/// logged but do not abort (observed STALLing on the OD103 ROM), strings are
/// sent at zero-based indexes 0..=4 with 1 ms spacing, and `host_uuid`
/// becomes the accessory serial (index 4). UTF-16LE per the AOA spec.
fn send_accessory_identification(
    handle: &rusb::DeviceHandle<rusb::GlobalContext>,
    host_uuid: &str,
) -> Result<()> {
    let timeout = Duration::from_secs(2);
    let mut protocol = [0_u8; 2];
    // Best-effort like the Mac client: a STALL here (the Smartisan ROM
    // STALLs GET_PROTOCOL in composite mode) must not abort the handshake.
    let _ = handle.read_control(0xc0, ACC_GET_PROTOCOL, 0, 0, &mut protocol, timeout);
    let strings: [(u16, &str); 5] = [
        (0, ACC_MANUFACTURER),
        (1, ACC_MODEL),
        (2, ACC_DESCRIPTION),
        (3, ACC_VERSION),
        (4, host_uuid),
    ];
    for (index, value) in strings {
        // The Mac client encodes identification strings as UTF-8
        // (cStringUsingEncoding:NSUTF8StringEncoding in sendControlString),
        // and the Android f_accessory driver stores them byte-wise; UTF-16LE
        // would truncate every string at the first NUL (observed as
        // mManufacturer="S" on the device).
        let encoded = value.as_bytes();
        handle
            .write_control(0x40, ACC_SEND_STRING, 0, index, encoded, timeout)
            .map_err(|error| {
                Error::LocalIo(i18n::format("usb.aoa_string_failed", &[&error.to_string()]))
            })?;
        std::thread::sleep(Duration::from_millis(1));
    }
    handle
        .write_control(0x40, ACC_START, 0, 0, &[], timeout)
        .map_err(|error| {
            Error::LocalIo(i18n::format("usb.aoa_start_failed", &[&error.to_string()]))
        })?;
    Ok(())
}

/// Wait for the phone to re-enumerate in accessory mode at the same USB
/// location (it briefly disappears after ACCESSORY_START).
fn wait_for_accessory(location: &str) -> Result<rusb::Device<rusb::GlobalContext>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while std::time::Instant::now() < deadline {
        if let Ok(devices) = rusb::devices() {
            for device in devices.iter() {
                if let Ok(descriptor) = device.device_descriptor()
                    && descriptor.vendor_id() == ACCESSORY_VID
                    && ACCESSORY_PIDS.contains(&descriptor.product_id())
                {
                    let ports = device.port_numbers().unwrap_or_default();
                    let candidate = format!(
                        "{}-{}",
                        device.bus_number(),
                        ports
                            .iter()
                            .map(u8::to_string)
                            .collect::<Vec<_>>()
                            .join("-")
                    );
                    if candidate == location {
                        return Ok(device);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(Error::LocalIo(i18n::format(
        "usb.aoa_switch_failed",
        &[location],
    )))
}

/// Open, claim, and wrap an accessory device as an async byte stream.
pub(crate) struct UsbConnector {
    location: Option<String>,
    host_uuid: String,
    timeout: Duration,
}

impl UsbConnector {
    pub fn new(location: Option<String>, host_uuid: &str, timeout: Duration) -> Self {
        Self {
            location,
            host_uuid: host_uuid.to_string(),
            timeout,
        }
    }

    pub fn connect(self) -> Result<ConnectedTransport> {
        let accessories = list_accessories()?;
        let selected = match &self.location {
            Some(location) => accessories
                .iter()
                .find(|accessory| &accessory.location == location)
                .ok_or_else(|| Error::LocalIo(i18n::format("usb.device_not_found", &[location])))?,
            None => {
                if accessories.len() != 1 {
                    return Err(Error::LocalIo(i18n::format(
                        "usb.ambiguous_devices",
                        &[&accessories.len().to_string()],
                    )));
                }
                accessories
                    .first()
                    .expect("one accessory after the length check")
            }
        };
        let selected_location = selected.location.clone();
        let mut device = find_device(&selected_location)?;
        if selected.mode == AccessoryMode::Plain {
            let handle = device.open().map_err(|error| {
                Error::LocalIo(i18n::format(
                    "usb.open_failed",
                    &[&selected_location, &error.to_string()],
                ))
            })?;
            // macOS binds kernel drivers (e.g. mass storage) to plain-mode
            // devices; detach best-effort so control transfers reach the
            // gadget. macOS returns NotSupported when nothing is bound.
            let _ = handle.set_auto_detach_kernel_driver(true);
            send_accessory_identification(&handle, &self.host_uuid)?;
            drop(handle);
            device = wait_for_accessory(&selected_location)?;
            // After ACCESSORY_START the device re-enumerates as 0x18d1 and
            // Android broadcasts USB_ACCESSORY_ATTACHED; the phone app then
            // opens the accessory asynchronously (permission was granted on
            // first pairing). Give it a moment before the first bulk frames.
            std::thread::sleep(Duration::from_secs(2));
        }
        let handle = device.open().map_err(|error| {
            Error::LocalIo(i18n::format(
                "usb.open_failed",
                &[&selected_location, &error.to_string()],
            ))
        })?;
        // Best-effort on platforms with kernel drivers (Linux); macOS returns
        // NotSupported which is fine.
        let _ = handle.set_auto_detach_kernel_driver(true);
        let (interface_number, bulk_in, bulk_out) = accessory_endpoints(&device)?;
        handle.claim_interface(interface_number).map_err(|error| {
            Error::LocalIo(i18n::format(
                "usb.claim_failed",
                &[&selected_location, &error.to_string()],
            ))
        })?;
        let label = selected
            .serial
            .clone()
            .unwrap_or_else(|| selected.location.clone());
        let stream = UsbStream::new(handle, interface_number, bulk_in, bulk_out, self.timeout)?;
        Ok(ConnectedTransport {
            stream: Box::new(stream),
            label,
            cleanup: TransportCleanup::None,
        })
    }
}

fn find_device(location: &str) -> Result<rusb::Device<rusb::GlobalContext>> {
    let devices = rusb::devices().map_err(|error| {
        Error::LocalIo(i18n::format("usb.enumerate_failed", &[&error.to_string()]))
    })?;
    devices
        .iter()
        .find(|device| {
            let ports = device.port_numbers().unwrap_or_default();
            let candidate = format!(
                "{}-{}",
                device.bus_number(),
                ports
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join("-")
            );
            candidate == location
        })
        .ok_or_else(|| Error::LocalIo(i18n::format("usb.device_not_found", &[location])))
}

fn usb_error(error: rusb::Error) -> Error {
    match error {
        rusb::Error::NoDevice | rusb::Error::NotFound => {
            Error::Transport(i18n::text("usb.device_gone").to_string())
        }
        rusb::Error::Busy => Error::Transport(i18n::text("usb.busy").to_string()),
        rusb::Error::Pipe => Error::Transport(i18n::text("usb.pipe_error").to_string()),
        _ => Error::Transport(i18n::format("usb.transfer_failed", &[&error.to_string()])),
    }
}

/// Async wrapper over a libusb bulk stream: a reader thread feeds a bounded
/// channel (backpressure via Full-retry); writes go through spawn_blocking
/// with an in-flight oneshot so `poll_write` never blocks the reactor.
pub(crate) struct UsbStream {
    handle: Arc<rusb::DeviceHandle<rusb::GlobalContext>>,
    interface_number: u8,
    bulk_out: u8,
    read_rx: mpsc::Receiver<io::Result<Vec<u8>>>,
    /// Bytes received from the reader thread that did not fit the last
    /// `ReadBuf`; drained by subsequent `poll_read` calls.
    pending_read: Vec<u8>,
    write_pending: Option<tokio::sync::oneshot::Receiver<io::Result<usize>>>,
    write_timeout: Duration,
    _reader: JoinHandle<()>,
}

impl Drop for UsbStream {
    fn drop(&mut self) {
        // Cancel the blocking reader thread; it releases the interface and
        // resets the device once it exits (it owns the last handle clone,
        // so release/reset run without concurrent bulk I/O). When the reader
        // was already gone, try_unwrap succeeds and we do it here.
        self._reader.abort();
        if let Ok(handle) = Arc::try_unwrap(Arc::clone(&self.handle)) {
            let _ = handle.release_interface(self.interface_number);
            let _ = handle.reset();
        }
    }
}

impl UsbStream {
    pub fn new(
        handle: rusb::DeviceHandle<rusb::GlobalContext>,
        interface_number: u8,
        bulk_in: u8,
        bulk_out: u8,
        timeout: Duration,
    ) -> Result<Self> {
        let handle = Arc::new(handle);
        let (tx, rx) = mpsc::channel(128);
        let reader_handle = Arc::clone(&handle);
        let reader = tokio::task::spawn_blocking(move || {
            let mut buffer = vec![0_u8; READ_CHUNK];
            loop {
                let read = reader_handle.read_bulk(bulk_in, &mut buffer, BULK_TIMEOUT);
                match read {
                    Ok(0) => {
                        let _ = tx.try_send(Ok(Vec::new()));
                        break;
                    }
                    Ok(count) => {
                        // Bounded channel provides backpressure; retry when
                        // full, but terminate when the stream was dropped
                        // (Closed) so the reader thread never becomes a zombie.
                        match tx.try_send(Ok(buffer[..count].to_vec())) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => break,
                        }
                    }
                    Err(rusb::Error::Timeout) => {
                        // The stream may have been dropped while the phone
                        // keeps the endpoint open: bail out instead of
                        // looping forever on 5 s timeouts holding the last
                        // handle clone (which would block a later claim).
                        if tx.is_closed() {
                            break;
                        }
                        continue;
                    }
                    Err(error) => {
                        let _ = tx.try_send(Err(io::Error::other(usb_error(error).to_string())));
                        break;
                    }
                }
            }
            // Reader thread exits owning the last handle reference (no
            // concurrent bulk I/O): release the interface and reset the
            // device so the phone re-enumerates and, when physically
            // unplugged, exits accessory mode. Mirrors the Mac client's
            // SFUSBDevice close. Both are best-effort; Drop's try_unwrap is
            // a fallback when this thread was aborted before reaching here.
            let _ = reader_handle.release_interface(interface_number);
            let _ = reader_handle.reset();
        });
        Ok(Self {
            handle,
            interface_number,
            bulk_out,
            read_rx: rx,
            pending_read: Vec::new(),
            write_pending: None,
            write_timeout: timeout,
            _reader: reader,
        })
    }
}

impl AsyncRead for UsbStream {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // Serve leftover bytes first: bulk reads are 16 KiB but callers (e.g.
        // the 6-byte frame header) request less; never let the surplus exceed
        // the caller's ReadBuf (put_slice would panic).
        if !this.pending_read.is_empty() {
            fill_buffer(buffer, &mut this.pending_read, &[]);
            return Poll::Ready(Ok(()));
        }
        match Pin::new(&mut this.read_rx).poll_recv(context) {
            Poll::Ready(Some(Ok(chunk))) => {
                fill_buffer(buffer, &mut this.pending_read, &chunk);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Err(error)),
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Copy as much of `incoming` (after any `pending` leftovers) as fits
/// `buffer`, retaining overflow in `pending` for the next poll.
fn fill_buffer(buffer: &mut ReadBuf<'_>, pending: &mut Vec<u8>, incoming: &[u8]) {
    if !pending.is_empty() {
        let take = pending.len().min(buffer.remaining());
        buffer.put_slice(&pending[..take]);
        pending.drain(..take);
        return;
    }
    let take = incoming.len().min(buffer.remaining());
    buffer.put_slice(&incoming[..take]);
    if take < incoming.len() {
        pending.extend_from_slice(&incoming[take..]);
    }
}

impl AsyncWrite for UsbStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_pending.is_none() {
            let handle = Arc::clone(&self.handle);
            let bulk_out = self.bulk_out;
            let timeout = self.write_timeout;
            let data = buffer.to_vec();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            tokio::task::spawn_blocking(move || {
                let result = handle
                    .write_bulk(bulk_out, &data, timeout)
                    .map_err(transport_io_error);
                let _ = sender.send(result);
            });
            self.write_pending = Some(receiver);
        }
        let receiver = self
            .write_pending
            .as_mut()
            .expect("write_pending set above");
        match Pin::new(receiver).poll(context) {
            Poll::Ready(Ok(Ok(count))) => {
                self.write_pending = None;
                Poll::Ready(Ok(count))
            }
            Poll::Ready(Ok(Err(error))) => {
                self.write_pending = None;
                Poll::Ready(Err(error))
            }
            Poll::Ready(Err(_)) => {
                self.write_pending = None;
                Poll::Ready(Err(io::Error::other("usb write task dropped")))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write_pending.is_some() {
            // Wait for the in-flight write before reporting flushed.
            let receiver = self.write_pending.as_mut().expect("write_pending");
            match Pin::new(receiver).poll(context) {
                Poll::Ready(Ok(Ok(_))) => {
                    self.write_pending = None;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Ok(Err(error))) => {
                    self.write_pending = None;
                    Poll::Ready(Err(error))
                }
                Poll::Ready(Err(_)) => {
                    self.write_pending = None;
                    Poll::Ready(Err(io::Error::other("usb write task dropped")))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn transport_io_error(error: rusb::Error) -> io::Error {
    io::Error::other(usb_error(error).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_constants_match_accessory_spec() {
        assert_eq!(ACCESSORY_VID, 0x18d1);
        assert_eq!(ACCESSORY_PIDS, &[0x2d00, 0x2d01, 0x2d02, 0x2d03]);
        assert_eq!(ACCESSORY_IFACE_CLASS, 0xff);
        assert_eq!(ACCESSORY_IFACE_SUBCLASS, 0xff);
    }

    #[test]
    fn usb_error_maps_device_gone_to_transport() {
        match usb_error(rusb::Error::NoDevice) {
            Error::Transport(_) => {}
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    #[test]
    fn location_format_is_stable() {
        let location = format!(
            "{}-{}",
            1_u8,
            [2_u8, 3]
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join("-")
        );
        assert_eq!(location, "1-2-3");
    }

    /// Regression for the security-review HIGH: a 16 KiB bulk chunk must not
    /// panic when the caller's ReadBuf is smaller (e.g. the 6-byte frame
    /// header read); the surplus must be buffered for the next poll.
    #[test]
    fn fill_buffer_retains_overflow_for_small_reads() {
        let mut pending = Vec::new();
        let mut buf = [0_u8; 6];
        let mut read = tokio::io::ReadBuf::new(&mut buf);
        let chunk = vec![0x5a_u8; 16 * 1024];
        fill_buffer(&mut read, &mut pending, &chunk);
        assert_eq!(&buf, &[0x5a; 6], "first 6 bytes served");
        assert_eq!(pending.len(), 16 * 1024 - 6, "surplus retained");
        assert!(pending.iter().all(|byte| *byte == 0x5a));

        // Next poll serves the surplus without a new incoming chunk.
        let mut buf2 = [0_u8; 4];
        let mut read2 = tokio::io::ReadBuf::new(&mut buf2);
        fill_buffer(&mut read2, &mut pending, &[]);
        assert_eq!(&buf2, &[0x5a; 4]);
        assert_eq!(pending.len(), 16 * 1024 - 10);
    }
}
