use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use tokio::time::timeout;

use crate::domain::WifiDevice;
use crate::error::{Error, Result};
use crate::i18n;

/// mDNS service type advertised by the HandShaker phone app.
pub(crate) const HANDSHAKER_SERVICE_TYPE: &str = "_handshaker_ssp._tcp.local.";

/// Browse for HandShaker WiFi devices for up to `browse_timeout`, then return
/// the unique resolved devices.
///
/// SRV/TXT/A/AAAA resolution is handled by the mdns-sd daemon; the advertised
/// TCP port is dynamic and must always be read fresh from the resolution.
pub(crate) async fn discover_wifi_devices(browse_timeout: Duration) -> Result<Vec<WifiDevice>> {
    let daemon = ServiceDaemon::new().map_err(|error| {
        Error::Configuration(i18n::format("wifi.mdns_init_failed", &[&error.to_string()]))
    })?;
    let receiver = daemon.browse(HANDSHAKER_SERVICE_TYPE).map_err(|error| {
        Error::Configuration(i18n::format(
            "wifi.mdns_browse_failed",
            &[&error.to_string()],
        ))
    })?;

    // Key by host name: the app registers the fixed instance name
    // `handshaker_ssp_` (docs/02 §2.1), so fullname alone would collapse
    // every phone into one entry; the SRV port is dynamic, so the latest
    // resolution for each host wins.
    let mut devices: std::collections::BTreeMap<String, WifiDevice> =
        std::collections::BTreeMap::new();
    let deadline = tokio::time::Instant::now() + browse_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let event = match timeout(remaining, receiver.recv_async()).await {
            Ok(Ok(event)) => event,
            Ok(Err(_)) | Err(_) => break, // channel closed or browse deadline
        };
        if let ServiceEvent::ServiceResolved(service) = event {
            let key = service.host.trim_end_matches('.').to_string();
            devices.insert(key, resolved_to_device(&service));
        }
    }
    drop(receiver);
    let _ = daemon.shutdown();
    Ok(devices.into_values().collect())
}

/// Convert a resolved mDNS service into the public domain model.
fn resolved_to_device(service: &ResolvedService) -> WifiDevice {
    let mut addresses: Vec<IpAddr> = service
        .addresses
        .iter()
        .map(|scoped| scoped.to_ip_addr())
        .collect();
    // IPv4 first, then IPv6, in a stable order for deterministic output.
    addresses.sort_by_key(|address| match address {
        IpAddr::V4(_) => 0,
        IpAddr::V6(_) => 1,
    });
    let mut txt = BTreeMap::new();
    for property in service.txt_properties.iter() {
        txt.insert(property.key().to_string(), property.val_str().to_string());
    }
    WifiDevice {
        instance: instance_name(&service.fullname),
        host: service.host.trim_end_matches('.').to_string(),
        addresses: addresses.into_iter().map(|ip| ip.to_string()).collect(),
        port: service.port,
        txt,
    }
}

/// Extract the instance name from a full service name such as
/// `handshaker_ssp_._handshaker_ssp._tcp.local.`.
fn instance_name(fullname: &str) -> String {
    fullname
        .split_once('.')
        .map(|(instance, _)| instance.to_string())
        .unwrap_or_else(|| fullname.to_string())
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use mdns_sd::ServiceInfo;

    use super::*;

    fn resolved(fullname: &str, host: &str, port: u16, addresses: Vec<IpAddr>) -> ResolvedService {
        let service_type = "_handshaker_ssp._tcp.local.";
        let info = ServiceInfo::new(
            fullname.trim_end_matches(service_type),
            service_type,
            host,
            &addresses[..],
            port,
            &[("note", "ssp")][..],
        )
        .expect("service info");
        let mut resolved = info.as_resolved_service();
        resolved.fullname = fullname.to_string();
        resolved
    }

    #[test]
    fn resolved_service_maps_to_public_model() {
        let service = resolved(
            "handshaker_ssp_._handshaker_ssp._tcp.local.",
            "fixture-phone.local.",
            45656,
            vec![
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 47)),
            ],
        );
        let device = resolved_to_device(&service);
        assert_eq!(device.instance, "handshaker_ssp_");
        assert_eq!(device.host, "fixture-phone.local");
        assert_eq!(device.port, 45656);
        // IPv4 first, then IPv6.
        assert_eq!(
            device.addresses,
            vec!["192.0.2.47".to_string(), "::1".to_string()]
        );
        assert_eq!(device.txt.get("note").map(String::as_str), Some("ssp"));
    }

    #[test]
    fn instance_name_splits_at_first_dot() {
        assert_eq!(
            instance_name("handshaker_ssp_._handshaker_ssp._tcp.local."),
            "handshaker_ssp_"
        );
        assert_eq!(instance_name("no-dot-here"), "no-dot-here");
    }

    #[test]
    fn latest_resolution_per_host_wins_and_distinct_hosts_are_kept() {
        // The SRV port is dynamic; a re-resolution for the same host replaces
        // the earlier one, while distinct hosts (multiple phones) each stay.
        let early = resolved(
            "handshaker_ssp_._handshaker_ssp._tcp.local.",
            "fixture-phone.local.",
            1000,
            vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 47))],
        );
        let late = resolved(
            "handshaker_ssp_._handshaker_ssp._tcp.local.",
            "fixture-phone.local.",
            2000,
            vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 47))],
        );
        let other = resolved(
            "handshaker_ssp_._handshaker_ssp._tcp.local.",
            "fixture-phone-b.local.",
            3000,
            vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 48))],
        );
        let mut devices = BTreeMap::new();
        devices.insert(
            early.host.trim_end_matches('.').to_string(),
            resolved_to_device(&early),
        );
        devices.insert(
            late.host.trim_end_matches('.').to_string(),
            resolved_to_device(&late),
        );
        devices.insert(
            other.host.trim_end_matches('.').to_string(),
            resolved_to_device(&other),
        );
        assert_eq!(devices.len(), 2, "two distinct phones must both appear");
        assert_eq!(
            devices.get("fixture-phone.local").expect("device").port,
            2000
        );
        assert!(
            devices
                .values()
                .any(|device| device.host == "fixture-phone-b.local")
        );
    }
}
