//! Trust service (Phase D / D3): WiFi trust records through the application
//! layer, always rooted at the runtime's `state_dir`.
//!
//! - DTOs never carry derived keys or state-file contents;
//! - `remove` only deletes the local record;
//! - `reset` validates the phone-reported UUID against the expected device
//!   before clearing the phone-side record, then deletes the local record.

use serde::{Deserialize, Serialize};

use crate::dto::DeviceId;
use crate::error::{AppResult, PublicError, PublicErrorCode};

/// One locally persisted WiFi trust record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRecordDto {
    /// Stable device id (`phone:<uuid>`), matching the reconciled identity
    /// used by connected sessions (Phase D / D2).
    pub device_id: DeviceId,
    pub device_name: Option<String>,
    /// Last successful trust, Unix milliseconds.
    pub updated_at_ms: u64,
}

/// Remove the local trust record for one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveTrustRequest {
    pub device_id: DeviceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveTrustResult {
    pub removed: bool,
}

/// Clear the phone-side WiFi trust for one device and the local record.
/// `endpoint` is the WiFi `IP:PORT` of the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetWifiTrustRequest {
    pub endpoint: String,
    pub expected_device_id: DeviceId,
}

/// Parse a `phone:<uuid>` device id back into the raw UUID. Anything else is
/// an invalid argument: trust records are keyed by the phone UUID and no
/// other id shape is accepted here.
pub(crate) fn parse_phone_device_id(device_id: &DeviceId) -> AppResult<&str> {
    device_id
        .0
        .strip_prefix("phone:")
        .filter(|uuid| !uuid.trim().is_empty())
        .ok_or_else(|| {
            PublicError::new(
                PublicErrorCode::InvalidArgument,
                "expected a phone device id in the form phone:<uuid>",
            )
            .operation("trust")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_phone_device_id_extracts_uuid() {
        let id = DeviceId("phone:9a3f-77ee".to_string());
        assert_eq!(parse_phone_device_id(&id).expect("valid"), "9a3f-77ee");
    }

    #[test]
    fn parse_phone_device_id_rejects_other_shapes() {
        for raw in ["9a3f-77ee", "phone:", "adb:serial-1", "wifi:x", "phone:   "] {
            let error =
                parse_phone_device_id(&DeviceId(raw.to_string())).expect_err("must be rejected");
            assert_eq!(error.code, PublicErrorCode::InvalidArgument);
        }
    }
}
