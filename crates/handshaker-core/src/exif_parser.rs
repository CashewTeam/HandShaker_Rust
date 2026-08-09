//! Local EXIF parsing for `HandShakerClient::fetch_exif`.
//!
//! The remote file is pulled over the SSP download channel (WiFi or ADB) and
//! parsed here with `kamadak-exif`; no EXIF fields are added to the SSP
//! schema. All error text comes from the locale files via `i18n::text`.

use crate::domain::ExifData;
use crate::error::{Error, Result};
use crate::i18n;

/// Maximum bytes pulled for EXIF parsing (covers all sane photos).
pub(crate) const EXIF_FETCH_LIMIT: u64 = 32 * 1024 * 1024;

/// Parse EXIF metadata from JPEG bytes.
pub(crate) fn exif_from_bytes(bytes: &[u8]) -> Result<ExifData> {
    let mut cursor = std::io::Cursor::new(bytes);
    let exif = exif::Reader::new()
        .read_from_container(&mut cursor)
        .map_err(|_| Error::Protocol(i18n::text("exif.parse_failed").to_string()))?;
    let mut data = ExifData::default();
    let field = |tag: exif::Tag| exif.get_field(tag, exif::In::PRIMARY);

    if let Some(value) = field(exif::Tag::Orientation) {
        data.orientation = short_value(value).map(|v| v as u32);
    }
    data.make = field(exif::Tag::Make).and_then(display_string);
    data.model = field(exif::Tag::Model).and_then(display_string);
    data.software = field(exif::Tag::Software).and_then(display_string);
    data.lens_model = field(exif::Tag::LensModel).and_then(display_string);
    data.focal_length = rational_value(field(exif::Tag::FocalLength));
    data.exposure_time = rational_value(field(exif::Tag::ExposureTime));
    data.f_number = rational_value(field(exif::Tag::FNumber));
    if let Some(value) = field(exif::Tag::PhotographicSensitivity) {
        data.iso = short_value(value).map(|v| v as u32);
    }
    data.date_taken = field(exif::Tag::DateTimeOriginal)
        .and_then(display_string)
        .and_then(|text| exif_datetime_to_unix(&text));
    data.latitude = gps_coordinate(exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef, &exif);
    data.longitude = gps_coordinate(exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef, &exif);
    Ok(data)
}

fn short_value(field: &exif::Field) -> Option<u16> {
    match &field.value {
        exif::Value::Short(values) => values.first().copied(),
        _ => None,
    }
}

fn rational_value(field: Option<&exif::Field>) -> Option<f64> {
    let field = field?;
    match &field.value {
        exif::Value::Rational(values) => values.first().map(|r| r.to_f64()),
        _ => None,
    }
}

fn display_string(field: &exif::Field) -> Option<String> {
    match &field.value {
        // kamadak's DisplayValue quotes ASCII strings; take the raw bytes
        // (ASCII values are stored as one or more byte strings).
        exif::Value::Ascii(components) => {
            let joined = components
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes))
                .collect::<String>();
            Some(joined.trim_end_matches('\0').trim().to_string())
        }
        _ => Some(field.display_value().to_string()),
    }
}

/// Convert an EXIF "YYYY:MM:DD HH:MM:SS" timestamp (interpreted as UTC, since
/// EXIF carries no timezone) to Unix seconds.
fn exif_datetime_to_unix(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split([':', ' ']).collect();
    if parts.len() < 6 {
        return None;
    }
    let year: i64 = parts[0].parse().ok()?;
    let month: i64 = parts[1].parse().ok()?;
    let day: i64 = parts[2].parse().ok()?;
    let hour: i64 = parts[3].parse().ok()?;
    let minute: i64 = parts[4].parse().ok()?;
    let second: i64 = parts[5].parse().ok()?;
    if year < 1970 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) as u64)
}

/// Days since the Unix epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// GPS degrees/minutes/seconds rationals plus an N/S/E/W reference become a
/// signed decimal-degrees string (same shape as the media-library query).
fn gps_coordinate(value_tag: exif::Tag, ref_tag: exif::Tag, exif: &exif::Exif) -> Option<String> {
    let values = exif.get_field(value_tag, exif::In::PRIMARY)?;
    let reference = exif
        .get_field(ref_tag, exif::In::PRIMARY)
        .and_then(display_string)
        .unwrap_or_default();
    let rationals = match &values.value {
        exif::Value::Rational(values) if values.len() == 3 => values,
        _ => return None,
    };
    let degrees = rationals[0].to_f64();
    let minutes = rationals[1].to_f64();
    let seconds = rationals[2].to_f64();
    let mut decimal = degrees + minutes / 60.0 + seconds / 3600.0;
    if reference.starts_with('S') || reference.starts_with('W') {
        decimal = -decimal;
    }
    Some(format!("{decimal:.6}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_datetime_is_parsed_as_utc_seconds() {
        assert_eq!(exif_datetime_to_unix("1970:01:01 00:00:00"), Some(0));
        assert_eq!(exif_datetime_to_unix("1970:01:01 00:00:01"), Some(1));
        assert_eq!(
            exif_datetime_to_unix("2023:06:03 01:53:20"),
            Some(1_685_757_200)
        );
        assert_eq!(exif_datetime_to_unix("1969:12:31 23:59:59"), None);
        assert_eq!(exif_datetime_to_unix("garbage"), None);
        assert_eq!(exif_datetime_to_unix("2023:13:01 00:00:00"), None);
    }

    #[test]
    fn days_from_civil_matches_known_epoch_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        // Verified against `date -j` on macOS: 2023-06-03 is day 19511.
        assert_eq!(days_from_civil(2023, 6, 3), 19_511);
    }

    #[test]
    fn non_jpeg_bytes_yield_clear_protocol_error() {
        let error = exif_from_bytes(b"not a jpeg at all").unwrap_err();
        assert!(matches!(error, Error::Protocol(_)));
    }

    #[test]
    fn parses_fixture_from_test_support() {
        let bytes = crate::test_support::exif_jpeg_fixture();
        let data = exif_from_bytes(&bytes).expect("parse fixture");
        assert_eq!(data.orientation, Some(6));
        assert_eq!(data.make.as_deref(), Some("Fixture"));
        assert_eq!(data.model.as_deref(), Some("TestCam"));
        assert_eq!(data.date_taken, Some(1_577_934_245));
        assert_eq!(data.f_number, Some(1.8));
        assert_eq!(data.focal_length, Some(4.28));
        assert_eq!(data.iso, Some(100));
        assert_eq!(data.latitude.as_deref(), Some("1.034167"));
        assert_eq!(data.longitude.as_deref(), Some("4.085000"));
    }
}
