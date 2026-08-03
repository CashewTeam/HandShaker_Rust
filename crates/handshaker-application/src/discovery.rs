//! Device discovery with per-transport diagnostics (Phase D / D1).
//!
//! A discovery sweep never fails as a whole just because one transport is
//! broken: ADB missing, Wi-Fi mDNS down, or USB permission errors surface as
//! structured [`DeviceDiscoveryWarning`] entries next to the devices that
//! were found. Whole-request failures (runtime closed, invalid request) are
//! still returned as `Err` by the runtime methods.

use serde::{Deserialize, Serialize};

use crate::dto::{AdbDetailDto, DeviceDescriptor, DeviceId, TransportKind, UsbDetailDto};
use crate::error::PublicError;

/// Result of a multi-transport device discovery sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceDiscoveryResult {
    pub devices: Vec<DeviceDescriptor>,
    pub warnings: Vec<DeviceDiscoveryWarning>,
}

/// A per-transport failure during discovery. `transport` names the channel
/// that failed; `error` carries the stable public error. A warning never
/// aborts the devices that other transports reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceDiscoveryWarning {
    pub transport: TransportKind,
    pub error: PublicError,
}

/// Map one core ADB device entry onto the stable descriptor.
pub(crate) fn adb_device_to_descriptor(device: handshaker_core::AdbDevice) -> DeviceDescriptor {
    DeviceDescriptor {
        id: DeviceId(device.serial.clone()),
        display_name: Some(device.serial.clone()),
        model: device.model.clone(),
        transport: TransportKind::Adb,
        transport_address: device.serial.clone(),
        available: device.state == "device",
        adb: Some(AdbDetailDto {
            state: device.state.clone(),
            product: device.product.clone(),
            model: device.model.clone(),
            device: device.device.clone(),
        }),
        usb: None,
    }
}

/// Map one core Wi-Fi device entry onto the stable descriptor.
///
/// The id is a *discovery endpoint* id only: the mDNS SRV port is dynamic,
/// so the id embeds it and must never be treated as a stable device identity
/// (Phase D / D2 reconciles the stable id after connection).
pub(crate) fn wifi_device_to_descriptor(device: handshaker_core::WifiDevice) -> DeviceDescriptor {
    let address = device
        .addresses
        .first()
        .cloned()
        .unwrap_or_else(|| device.host.clone());
    DeviceDescriptor {
        id: DeviceId(format!(
            "wifi-endpoint:{}:{}:{}",
            device.instance, address, device.port
        )),
        display_name: Some(device.host.clone()),
        model: None,
        transport: TransportKind::Wifi,
        transport_address: format!("{address}:{}", device.port),
        available: true,
        adb: None,
        usb: None,
    }
}

/// Map one core USB AOA accessory entry onto the stable descriptor.
pub(crate) fn usb_device_to_descriptor(device: handshaker_core::UsbAccessory) -> DeviceDescriptor {
    DeviceDescriptor {
        id: DeviceId(device.location.clone()),
        display_name: device.serial.clone().or(Some(device.location.clone())),
        model: None,
        transport: TransportKind::UsbAccessory,
        transport_address: format!("0x{:04x}:0x{:04x}", device.vendor_id, device.product_id),
        available: true,
        adb: None,
        usb: Some(UsbDetailDto {
            bus_number: device.bus_number,
            serial: device.serial.clone(),
            vendor_id: device.vendor_id,
            product_id: device.product_id,
            mode: format!("{:?}", device.mode),
        }),
    }
}

/// Remove duplicate entries after merging transports: the first occurrence of
/// a (transport, transport id) pair wins, later duplicates are dropped.
/// Ordering of the survivors is left to the caller's sort.
pub(crate) fn deduplicate_discovered_devices(devices: &mut Vec<DeviceDescriptor>) {
    let mut seen = std::collections::HashSet::new();
    devices.retain(|device| seen.insert((device.transport, device.id.clone())));
}

/// Stable presentation order: ADB, then Wi-Fi, then USB; ties broken by the
/// transport address, then by display name so the output is deterministic.
pub(crate) fn device_sort_key(device: &DeviceDescriptor) -> (u8, &str, &str) {
    let transport = match device.transport {
        TransportKind::Adb => 0,
        TransportKind::Wifi => 1,
        TransportKind::UsbAccessory => 2,
    };
    (
        transport,
        device.transport_address.as_str(),
        device.display_name.as_deref().unwrap_or(""),
    )
}

/// Sort devices in place by [`device_sort_key`].
pub(crate) fn sort_discovered_devices(devices: &mut [DeviceDescriptor]) {
    devices.sort_by(|left, right| device_sort_key(left).cmp(&device_sort_key(right)));
}

#[cfg(test)]
mod tests {
    use handshaker_core::{AdbDevice, UsbAccessory};

    use super::*;

    fn adb_device() -> handshaker_core::AdbDevice {
        AdbDevice {
            serial: "serial-1".to_string(),
            state: "device".to_string(),
            product: Some("product-1".to_string()),
            model: Some("model-1".to_string()),
            device: Some("device-1".to_string()),
        }
    }

    fn wifi_device(port: u16) -> handshaker_core::WifiDevice {
        handshaker_core::WifiDevice {
            instance: "handshaker_ssp_".to_string(),
            host: "Android-2.local".to_string(),
            addresses: vec!["192.168.2.47".to_string()],
            port,
            txt: Default::default(),
        }
    }

    fn usb_device() -> UsbAccessory {
        UsbAccessory {
            location: "20-1".to_string(),
            bus_number: 1,
            serial: Some("usb-serial".to_string()),
            vendor_id: 0x18d1,
            product_id: 0x2d01,
            mode: handshaker_core::AccessoryMode::Accessory,
        }
    }

    #[test]
    fn adb_device_maps_to_descriptor() {
        let descriptor = adb_device_to_descriptor(adb_device());
        assert_eq!(descriptor.id, DeviceId("serial-1".to_string()));
        assert_eq!(descriptor.transport, TransportKind::Adb);
        assert!(descriptor.available);
        let adb = descriptor.adb.expect("adb detail");
        assert_eq!(adb.state, "device");
        assert_eq!(adb.product.as_deref(), Some("product-1"));
    }

    #[test]
    fn adb_unauthorized_device_is_not_available() {
        let mut device = adb_device();
        device.state = "unauthorized".to_string();
        let descriptor = adb_device_to_descriptor(device);
        assert!(!descriptor.available);
    }

    #[test]
    fn wifi_endpoint_id_embeds_the_dynamic_port() {
        let descriptor = wifi_device_to_descriptor(wifi_device(45656));
        assert_eq!(
            descriptor.id,
            DeviceId("wifi-endpoint:handshaker_ssp_:192.168.2.47:45656".to_string())
        );
        assert_eq!(descriptor.transport, TransportKind::Wifi);
        assert_eq!(descriptor.transport_address, "192.168.2.47:45656");
        // The endpoint id must change with the port: the port is dynamic and
        // must never be treated as a stable device identity.
        let other = wifi_device_to_descriptor(wifi_device(9999));
        assert_ne!(descriptor.id, other.id);
    }

    #[test]
    fn usb_device_maps_to_descriptor() {
        let descriptor = usb_device_to_descriptor(usb_device());
        assert_eq!(descriptor.id, DeviceId("20-1".to_string()));
        assert_eq!(descriptor.transport, TransportKind::UsbAccessory);
        assert_eq!(descriptor.transport_address, "0x18d1:0x2d01");
        let usb = descriptor.usb.expect("usb detail");
        assert_eq!(usb.serial.as_deref(), Some("usb-serial"));
        assert_eq!(usb.mode, "Accessory");
    }

    #[test]
    fn deduplicate_keeps_first_occurrence_per_transport_id() {
        let mut devices = vec![
            adb_device_to_descriptor(adb_device()),
            adb_device_to_descriptor(adb_device()),
            wifi_device_to_descriptor(wifi_device(45656)),
            usb_device_to_descriptor(usb_device()),
        ];
        deduplicate_discovered_devices(&mut devices);
        assert_eq!(devices.len(), 3);
    }

    #[test]
    fn sort_is_stable_and_transport_ordered() {
        let wifi = wifi_device_to_descriptor(wifi_device(45656));
        let usb = usb_device_to_descriptor(usb_device());
        let mut adb2 = adb_device();
        adb2.serial = "serial-2".to_string();
        let mut devices = vec![
            usb,
            wifi,
            adb_device_to_descriptor(adb2),
            adb_device_to_descriptor(adb_device()),
        ];
        sort_discovered_devices(&mut devices);
        let transports: Vec<TransportKind> = devices.iter().map(|d| d.transport).collect();
        assert_eq!(
            transports,
            vec![
                TransportKind::Adb,
                TransportKind::Adb,
                TransportKind::Wifi,
                TransportKind::UsbAccessory
            ]
        );
        // Ties break on the transport address (serial-1 < serial-2).
        assert_eq!(devices[0].id, DeviceId("serial-1".to_string()));
        assert_eq!(devices[1].id, DeviceId("serial-2".to_string()));
    }

    #[test]
    fn warning_json_uses_stable_tokens() {
        let warning = DeviceDiscoveryWarning {
            transport: TransportKind::Wifi,
            error: PublicError::new(
                crate::error::PublicErrorCode::WifiDiscoveryFailed,
                "mDNS unavailable",
            )
            .operation("discover_devices.wifi"),
        };
        let value = serde_json::to_value(&warning).expect("serialize");
        let object = value.as_object().expect("object");
        assert_eq!(
            object.get("transport").and_then(|v| v.as_str()),
            Some("wifi")
        );
        let error = object
            .get("error")
            .expect("error")
            .as_object()
            .expect("object");
        assert_eq!(
            error.get("code").and_then(|v| v.as_str()),
            Some("wifi_discovery_failed")
        );
        // Round-trip keeps the warning intact.
        let decoded: DeviceDiscoveryWarning = serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded, warning);
    }
}
