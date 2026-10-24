//! EXIF metadata for the JPEG output, built from a FITS/XISF header.
//!
//! The header is read separately — [`crate::converter::ImageConverter::read_header`]
//! hands back every keyword without touching the pixels — and is then passed
//! to the encoder as its own argument. Nothing is added to
//! [`crate::types::ProcessedImage`], so library users who build that struct
//! literally keep compiling.
//!
//! The finished block is injected into the encoded JPEG as an APP1 segment,
//! which leaves every other byte of the stream untouched.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use little_exif::exif_tag::ExifTag;
use little_exif::ifd::ExifTagGroup;
use little_exif::metadata::Metadata;
use little_exif::rational::{iR64, uR64};

use crate::types::{HeaderKeyword, ImageHeader};

/// The observational keywords of a FITS/XISF header, in the shape the EXIF
/// writer wants them.
///
/// Every field is optional: a header that carries only `EXPTIME` gives a
/// metadata with only [`exposure`](Self::exposure) set.
#[derive(Clone, Debug, Default)]
pub struct ObservationalMetadata {
    /// Acquisition date, `YYYY-MM-DD` or `YYYY-MM-DDThh:mm:ss`.
    pub date_obs: Option<String>,
    /// Acquisition time, `hh:mm:ss[.sss]`.
    pub time_obs: Option<String>,
    /// Exposure time in seconds.
    pub exposure: Option<f64>,
    /// ISO as recorded by the camera.
    pub iso: Option<u32>,
    /// Detector gain.
    pub gain: Option<f64>,
    /// Aperture (f-number).
    pub f_number: Option<f64>,
    /// Focal length in millimetres.
    pub focal_length: Option<f64>,
    /// Sensor temperature in degrees Celsius.
    pub ccd_temp: Option<f64>,
    /// The camera or imager.
    pub instrument: Option<String>,
    /// The telescope or lens system.
    pub telescope: Option<String>,
    /// The lens, when the header names one separately.
    pub lens: Option<String>,
    /// The filter in use.
    pub filter: Option<String>,
    /// The target name.
    pub object: Option<String>,
    /// Right ascension, sexagesimal (`hh:mm:ss`).
    pub ra: Option<String>,
    /// Declination, sexagesimal (`±dd:mm:ss`).
    pub dec: Option<String>,
    /// Altitude in degrees.
    pub alt: Option<f64>,
    /// Azimuth in degrees.
    pub az: Option<f64>,
    /// Who took the frame.
    pub observer: Option<String>,
    /// Where it was taken.
    pub site: Option<String>,
    /// Free-form notes.
    pub notes: Option<String>,
    /// The Bayer pattern of a colour frame.
    pub bayerpat: Option<String>,
}

impl ObservationalMetadata {
    /// Collect the observational keywords of a FITS/XISF header.
    ///
    /// FITS keywords are plain names (`EXPTIME`); XISF properties are
    /// namespaced (`Instrument:ExposureTime`). Both are understood, and the
    /// keyword written verbatim always wins over the namespaced one.
    pub fn from_header(header: &ImageHeader) -> Self {
        let index = KeywordIndex::new(&header.keywords);
        Self {
            date_obs: index.text(&["DATE-OBS", "DATE_OBS", "DATEOBS", "DATE", "OBSERVATION:TIME:START"]),
            time_obs: index.text(&[
                "TIME-OBS",
                "TIME_OBS",
                "TIME-UTC",
                "UT",
                "UTSTART",
                "OBSERVATION:TIME:START",
            ]),
            exposure: index.number(&[
                "EXPTIME",
                "EXPOSURE",
                "EXPOSURETIME",
                "TOTALEXP",
                "INSTRUMENT:EXPOSURETIME",
            ]),
            iso: index
                .number(&["ISO", "ISOSPEED", "ISOSPEEDRATINGS", "ISO-SPEED"])
                .map(|value| value as u32),
            gain: index.number(&["GAIN", "EGAIN", "ELECTRONICGAIN", "CGAIN", "INSTRUMENT:CAMERA:GAIN"]),
            f_number: index.number(&["FNUMBER", "F-NUMBER", "APERTURE", "APTDIA", "FNUM"]),
            focal_length: index.number(&["FOCALLEN", "FOCUSLEN", "FLENGTH", "FOCALLENGTH"]),
            ccd_temp: index.number(&[
                "CCD-TEMP",
                "CCDTEMP",
                "CCD_TEMP",
                "SENSOR-TEMP",
                "TEMPERATURE",
                "INSTRUMENT:SENSOR:TEMPERATURE",
            ]),
            instrument: index.text(&[
                "INSTRUME",
                "INSTRUMENT",
                "CAMERA",
                "INSTRUMENT:CAMERA:MODEL",
                "INSTRUMENT:CAMERA:MAKE",
            ]),
            telescope: index.text(&["TELESCOP", "TELESCOPE", "INSTRUMENT:SCOPE:NAME"]),
            lens: index.text(&["LENS", "OPTICS", "INSTRUMENT:LENS:NAME"]),
            filter: index.text(&["FILTER", "FILTER-NAME", "FILTERNAME", "INSTRUMENT:FILTER:NAME"]),
            object: index.text(&[
                "OBJECT",
                "OBJCT",
                "TARGET",
                "OBJECTNAME",
                "OBJECT:NAME",
                "OBSERVATION:OBJECT:NAME",
            ]),
            ra: index.text(&["RA", "RA--OBS", "OBJCTRA", "OBJCT-RA", "OBJECT:RA", "OBSERVATION:OBJECT:RA"]),
            dec: index.text(&["DEC", "DEC-OBS", "OBJCTDEC", "OBJCT-DEC", "OBJECT:DEC", "OBSERVATION:OBJECT:DEC"]),
            alt: index.number(&["ALT", "EALT", "ALTI"]),
            az: index.number(&["AZ", "EAZ", "AZI"]),
            observer: index.text(&["OBSERVER", "OBSERVER:NAME", "OBSERVATION:OBSERVER:NAME"]),
            site: index.text(&["SITE", "SITE:NAME", "OBSERVATION:SITE:NAME"]),
            notes: index.text(&["NOTES", "COMMENT", "HISTORY"]),
            bayerpat: index.text(&["BAYERPAT", "BAYERPATTERN"]),
        }
    }

    /// Build the EXIF block — the TIFF part, without the `Exif\0\0` header
    /// that the JPEG segment carries.
    pub fn exif_payload(&self) -> Result<Vec<u8>> {
        let mut metadata = Metadata::new();

        if let Some(datetime) = self.exif_datetime() {
            metadata.set_tag(ExifTag::DateTimeOriginal(datetime.clone()));
            metadata.set_tag(ExifTag::CreateDate(datetime));
        }

        if let Some(exposure) = self.exposure {
            if let Some(rational) = exposure_rational(exposure) {
                metadata.set_tag(ExifTag::ExposureTime(vec![rational]));
                metadata.set_tag(ExifTag::ShutterSpeedValue(vec![apex_shutter(exposure)]));
            }
        }

        if let Some(f_number) = self.f_number {
            if let Some(rational) = scalar_rational(f_number) {
                metadata.set_tag(ExifTag::FNumber(vec![rational]));
                metadata.set_tag(ExifTag::ApertureValue(vec![apex_aperture(f_number)]));
            }
        }

        if let Some(focal_length) = self.focal_length {
            if let Some(rational) = scalar_rational(focal_length) {
                metadata.set_tag(ExifTag::FocalLength(vec![rational]));
            }
        }

        if let Some(iso) = self.iso {
            if iso > 0 && iso <= u16::MAX as u32 {
                metadata.set_tag(ExifTag::ISO(vec![iso as u16]));
            }
        }

        if let Some(ccd_temp) = self.ccd_temp {
            if let Some(rational) = signed_rational(ccd_temp) {
                metadata.set_tag(ExifTag::AmbientTemperature(vec![rational]));
            }
        }

        if let Some(instrument) = &self.instrument {
            metadata.set_tag(ExifTag::Model(instrument.clone()));
        }
        if let Some(telescope) = &self.telescope {
            metadata.set_tag(ExifTag::Make(telescope.clone()));
        }
        if let Some(lens) = &self.lens {
            metadata.set_tag(ExifTag::LensMake(lens.clone()));
            metadata.set_tag(ExifTag::LensModel(lens.clone()));
        }
        if let Some(object) = &self.object {
            metadata.set_tag(ExifTag::ImageDescription(object.clone()));
        }
        if let Some(observer) = &self.observer {
            metadata.set_tag(ExifTag::Artist(observer.clone()));
        }
        metadata.set_tag(ExifTag::Software(format!(
            "rustafits {}",
            env!("CARGO_PKG_VERSION")
        )));

        if let Some((ra_deg, dec_deg)) = self.coordinates_degrees() {
            metadata.create_ifd(ExifTagGroup::GPS, 0);
            metadata.set_tag(ExifTag::GPSVersionID(vec![2, 3, 0, 0]));
            let (latitude, south) = decimal_to_dms(dec_deg);
            metadata.set_tag(ExifTag::GPSLatitudeRef(if south { "S" } else { "N" }.to_string()));
            metadata.set_tag(ExifTag::GPSLatitude(latitude));
            let (longitude, west) = decimal_to_dms(ra_deg);
            metadata.set_tag(ExifTag::GPSLongitudeRef(if west { "W" } else { "E" }.to_string()));
            metadata.set_tag(ExifTag::GPSLongitude(longitude));
        }

        if let Some(comment) = self.user_comment() {
            metadata.set_tag(ExifTag::UserComment(comment));
        }

        metadata.encode().context("failed to encode EXIF")
    }

    /// `YYYY:MM:DD HH:MM:SS`, the shape EXIF asks for.
    fn exif_datetime(&self) -> Option<String> {
        let raw_date = self.date_obs.as_deref()?.trim();
        if raw_date.is_empty() {
            return None;
        }
        // A FITS date may carry the time: `2024-01-15T20:30:00.000`.
        let (date_part, inline_time) = match raw_date.split_once(['T', 't']) {
            Some((date, time)) => (date, Some(time)),
            None => (raw_date, None),
        };
        let date = normalize_exif_date(date_part)?;
        let time = inline_time
            .or(self.time_obs.as_deref())
            .and_then(normalize_exif_time)
            .unwrap_or_else(|| "00:00:00".to_string());
        Some(format!("{date} {time}"))
    }

    /// Right ascension and declination in degrees, if both are present and
    /// parseable.
    fn coordinates_degrees(&self) -> Option<(f64, f64)> {
        let ra = parse_sexagesimal(self.ra.as_deref()?, 15.0)?;
        let dec = parse_sexagesimal(self.dec.as_deref()?, 1.0)?;
        Some((ra, dec))
    }

    /// The observational fields that have no home in the EXIF standard, as a
    /// `key=value` list in the user comment.
    fn user_comment(&self) -> Option<Vec<u8>> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(gain) = self.gain {
            parts.push(format!("Gain={gain}"));
        }
        if let Some(filter) = &self.filter {
            parts.push(format!("Filter={filter}"));
        }
        if let Some(site) = &self.site {
            parts.push(format!("Site={site}"));
        }
        if let Some(alt) = self.alt {
            parts.push(format!("Alt={alt}"));
        }
        if let Some(az) = self.az {
            parts.push(format!("Az={az}"));
        }
        if let Some(bayerpat) = &self.bayerpat {
            parts.push(format!("BayerPattern={bayerpat}"));
        }
        if let Some(notes) = &self.notes {
            parts.push(format!("Notes={notes}"));
        }
        if parts.is_empty() {
            return None;
        }

        let text = parts.join("; ");
        // The user comment starts with an eight byte character code.
        let mut bytes = Vec::with_capacity(8 + text.len());
        bytes.extend_from_slice(b"ASCII\0\0\0");
        bytes.extend_from_slice(text.as_bytes());
        // TIFF values start on an even offset, so an odd length needs a pad.
        if bytes.len() % 2 != 0 {
            bytes.push(0);
        }
        Some(bytes)
    }
}

/// Insert an EXIF block into a JPEG byte stream as an APP1 segment.
///
/// The Exif specification wants the APP1 segment to be the first marker
/// segment of the file, so it goes in right behind the SOI marker. Every
/// other byte of the stream is left exactly as it was.
pub fn inject_exif(jpeg: &[u8], exif_tiff: &[u8]) -> Result<Vec<u8>> {
    if jpeg.len() < 2 || jpeg[0] != 0xff || jpeg[1] != 0xd8 {
        anyhow::bail!("not a JPEG stream: missing SOI marker");
    }

    const EXIF_HEADER: &[u8] = b"Exif\0\0";
    // The segment length counts itself but not the marker.
    let length = 2 + EXIF_HEADER.len() + exif_tiff.len();
    if length > u16::MAX as usize {
        anyhow::bail!("EXIF block too large for a JPEG APP1 segment");
    }

    let mut out = Vec::with_capacity(jpeg.len() + length);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xff, 0xe1]);
    out.extend_from_slice(&(length as u16).to_be_bytes());
    out.extend_from_slice(EXIF_HEADER);
    out.extend_from_slice(exif_tiff);
    out.extend_from_slice(&jpeg[2..]);
    Ok(out)
}

/// Case-insensitive lookup over the keywords of a header.
///
/// FITS keywords are plain names (`EXPTIME`); XISF properties are namespaced
/// (`Instrument:ExposureTime`). Both are indexed — the full name always wins,
/// the last path segment is only a fallback.
struct KeywordIndex {
    exact: BTreeMap<String, String>,
    namespaced: BTreeMap<String, String>,
}

impl KeywordIndex {
    fn new(keywords: &[HeaderKeyword]) -> Self {
        let mut exact: BTreeMap<String, String> = BTreeMap::new();
        let mut namespaced: BTreeMap<String, String> = BTreeMap::new();

        for keyword in keywords {
            let key = keyword.key.trim().to_ascii_uppercase();
            if key.is_empty() {
                continue;
            }
            exact.entry(key.clone()).or_insert_with(|| keyword.value.clone());
            if let Some((_, tail)) = key.rsplit_once(':') {
                let tail = tail.trim().to_ascii_uppercase();
                if !tail.is_empty() {
                    namespaced.entry(tail).or_insert_with(|| keyword.value.clone());
                }
            }
        }
        // A keyword written verbatim beats the same name inside a namespace.
        for (key, value) in &exact {
            namespaced.insert(key.clone(), value.clone());
        }

        Self { exact, namespaced }
    }

    fn raw(&self, candidates: &[&str]) -> Option<&str> {
        for candidate in candidates {
            let key = candidate.to_ascii_uppercase();
            for map in [&self.exact, &self.namespaced] {
                if let Some(value) = map.get(&key) {
                    if !value.trim().is_empty() {
                        return Some(value.as_str());
                    }
                }
            }
        }
        None
    }

    fn text(&self, candidates: &[&str]) -> Option<String> {
        self.raw(candidates).map(|value| value.trim().to_string())
    }

    fn number(&self, candidates: &[&str]) -> Option<f64> {
        self.raw(candidates).and_then(parse_number)
    }
}

/// Parse a header value as a number: plain decimals, and the `a/b` form some
/// writers use for a rational.
fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(value) = text.parse::<f64>() {
        return value.is_finite().then_some(value);
    }
    let (numerator, denominator) = text.split_once('/')?;
    let numerator = numerator.trim().parse::<f64>().ok()?;
    let denominator = denominator.trim().parse::<f64>().ok()?;
    if denominator == 0.0 {
        return None;
    }
    let value = numerator / denominator;
    value.is_finite().then_some(value)
}

/// `2024-01-15` → `2024:01:15`. Also accepts `2024:01:15` and `20240115`.
fn normalize_exif_date(date: &str) -> Option<String> {
    let date = date.trim();
    if date.is_empty() {
        return None;
    }
    if date.contains(':') {
        // Already in the EXIF shape; leave it alone.
        return Some(date.to_string());
    }
    if date.contains('-') {
        let parts: Vec<&str> = date.split('-').collect();
        if parts.len() != 3 {
            return None;
        }
        let (year, month, day) = (parts[0].trim(), parts[1].trim(), parts[2].trim());
        if year.is_empty() || month.is_empty() || day.is_empty() {
            return None;
        }
        return Some(format!("{year}:{month}:{day}"));
    }
    if date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("{}:{}:{}", &date[0..4], &date[4..6], &date[6..8]));
    }
    None
}

/// `20:30:00.000` → `20:30:00`.
fn normalize_exif_time(time: &str) -> Option<String> {
    let time = time.trim();
    if time.is_empty() {
        return None;
    }
    // The fractional seconds are dropped.
    let time = time.split_once('.').map_or(time, |(head, _)| head);
    let mut parts = time.split(':');
    let hour = parts.next()?.trim().parse::<u32>().ok()?;
    let minute = parts.next().map_or(0, |part| part.trim().parse().unwrap_or(0));
    let second = parts.next().map_or(0, |part| part.trim().parse().unwrap_or(0));
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(format!("{hour:02}:{minute:02}:{second:02}"))
}

/// Parse a sexagesimal coordinate into degrees.
///
/// `hours_per_unit` is 15 for right ascension (given in hours) and 1 for
/// declination (given in degrees).
fn parse_sexagesimal(text: &str, hours_per_unit: f64) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // The sign sits in front of the first part.
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, text.strip_prefix('+').unwrap_or(text)),
    };
    let parts: Vec<&str> = rest
        .split(|c: char| {
            c == ':' || c == ' ' || c == 'h' || c == 'm' || c == 's' || c == 'd' || c == '\u{00b0}'
        })
        .filter(|part| !part.is_empty())
        .collect();

    let first: f64 = match parts.first() {
        Some(part) => part.parse().ok()?,
        None => return None,
    };
    let second: f64 = match parts.get(1) {
        Some(part) => part.parse().ok()?,
        None => 0.0,
    };
    let third: f64 = match parts.get(2) {
        Some(part) => part.parse().ok()?,
        None => 0.0,
    };

    let value = sign * (first + second / 60.0 + third / 3600.0) * hours_per_unit;
    value.is_finite().then_some(value)
}

/// Decimal degrees → degrees, minutes and seconds as rationals, plus whether
/// the value is on the negative side of its axis.
fn decimal_to_dms(degrees: f64) -> (Vec<uR64>, bool) {
    let negative = degrees < 0.0;
    // Milliseconds first, so that a rounding carry cannot produce 60 seconds.
    let total_millis = (degrees.abs() * 3600.0 * 1000.0).round() as u64;
    let millis = total_millis % 1000;
    let total_seconds = total_millis / 1000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let whole_degrees = total_minutes / 60;

    (
        vec![
            uR64 { nominator: whole_degrees as u32, denominator: 1 },
            uR64 { nominator: minutes as u32, denominator: 1 },
            // The seconds carry the millisecond part with them.
            rational(seconds * 1000 + millis, 1000),
        ],
        negative,
    )
}

/// Exposure time as an EXIF rational, in seconds.
///
/// A long exposure keeps its decimal part — 30.7 s becomes 307/10. A short
/// one uses the camera convention 1/N — 0.5 s becomes 1/2, 0.0005 s becomes
/// 1/2000 — which is what every reader expects.
fn exposure_rational(seconds: f64) -> Option<uR64> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    if seconds >= 1.0 {
        // Millisecond precision, then reduced to lowest terms.
        let scaled = (seconds * 1000.0).round() as u64;
        let (numerator, denominator) = reduce(scaled, 1000);
        if numerator > u32::MAX as u64 {
            return None;
        }
        Some(uR64 {
            nominator: numerator as u32,
            denominator: denominator as u32,
        })
    } else {
        let denominator = (1.0 / seconds).round() as u64;
        if denominator == 0 || denominator > u32::MAX as u64 {
            return None;
        }
        Some(uR64 {
            nominator: 1,
            denominator: denominator as u32,
        })
    }
}

/// A plain decimal as an EXIF rational: 2.8 becomes 14/5, 24 becomes 24/1.
///
/// Only for positive values — a negative one needs the signed form.
fn scalar_rational(value: f64) -> Option<uR64> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let scaled = (value * 1000.0).round() as u64;
    let (numerator, denominator) = reduce(scaled, 1000);
    if numerator > u32::MAX as u64 || denominator > u32::MAX as u64 {
        return None;
    }
    Some(uR64 {
        nominator: numerator as u32,
        denominator: denominator as u32,
    })
}

/// A decimal as a signed EXIF rational: -10.5 becomes -21/2.
///
/// The sensor temperature is regularly below zero, so the unsigned form would
/// clamp it away.
fn signed_rational(value: f64) -> Option<iR64> {
    if !value.is_finite() {
        return None;
    }
    let scaled = (value * 1000.0).round() as i64;
    let (numerator, denominator) = reduce(scaled.unsigned_abs(), 1000);
    if numerator > i32::MAX as u64 || denominator > i32::MAX as u64 {
        return None;
    }
    let numerator = if scaled < 0 {
        -(numerator as i64)
    } else {
        numerator as i64
    };
    Some(iR64 {
        nominator: numerator as i32,
        denominator: denominator as i32,
    })
}

/// APEX shutter speed: `2^tv` with `tv = log2(t)`. A 1/250 s exposure is
/// tv = -7.97, so the stored value is -7966/1000.
fn apex_shutter(seconds: f64) -> iR64 {
    let tv = seconds.log2();
    signed_rational(tv).unwrap_or(iR64 { nominator: 0, denominator: 1 })
}

/// APEX aperture: `2 * log2(N)`. f/2.8 is 2.971.
fn apex_aperture(f_number: f64) -> uR64 {
    let av = 2.0 * f_number.log2();
    let scaled = (av * 1000.0).round() as u64;
    let (numerator, denominator) = reduce(scaled, 1000);
    uR64 {
        nominator: numerator as u32,
        denominator: denominator as u32,
    }
}

/// A rational reduced to lowest terms.
fn rational(numerator: u64, denominator: u64) -> uR64 {
    let (numerator, denominator) = reduce(numerator, denominator);
    uR64 {
        nominator: numerator as u32,
        denominator: denominator as u32,
    }
}

fn reduce(numerator: u64, denominator: u64) -> (u64, u64) {
    if numerator == 0 {
        return (0, 1);
    }
    let divisor = gcd(numerator, denominator);
    (numerator / divisor, denominator / divisor)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_exif::filetype::FileExtension;

    fn keyword(key: &str, value: &str) -> HeaderKeyword {
        HeaderKeyword {
            key: key.to_string(),
            value: value.to_string(),
            comment: String::new(),
        }
    }

    fn header_with(keywords: &[HeaderKeyword]) -> ImageHeader {
        ImageHeader {
            format: crate::types::FileFormat::Fits,
            width: 4,
            height: 3,
            channels: 1,
            sample_type: "u16",
            bayer_pattern: crate::types::BayerPattern::None,
            flip_vertical: true,
            keywords: keywords.to_vec(),
        }
    }

    /// Read a tag back out of an encoded EXIF block, so the tests check what a
    /// reader would actually see rather than what was put in.
    ///
    /// The block is wrapped in a minimal JPEG first, so the read-back goes
    /// through the same path a real file takes.
    fn read_tag(exif_tiff: &[u8], hex: u16, group: ExifTagGroup) -> Option<ExifTag> {
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.extend_from_slice(&[0xff, 0xe1]);
        jpeg.extend_from_slice(&((2 + 6 + exif_tiff.len()) as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(exif_tiff);
        jpeg.extend_from_slice(&[0xff, 0xd9]);

        let metadata = Metadata::new_from_vec(&jpeg, FileExtension::JPEG).ok()?;
        metadata.get_tag_by_hex(hex, Some(group)).next().cloned()
    }

    fn read_exif_tag(exif_tiff: &[u8], hex: u16) -> Option<ExifTag> {
        read_tag(exif_tiff, hex, ExifTagGroup::EXIF)
    }

    fn read_gps_tag(exif_tiff: &[u8], hex: u16) -> Option<ExifTag> {
        read_tag(exif_tiff, hex, ExifTagGroup::GPS)
    }

    #[test]
    fn reads_the_common_fits_keywords() {
        let header = header_with(&[
            keyword("EXPTIME", "120.0"),
            keyword("ISO", "800"),
            keyword("FOCALLEN", "24.00"),
            keyword("FNUMBER", "2.8"),
            keyword("INSTRUME", "QHY268M"),
            keyword("TELESCOP", "William Optics"),
            keyword("OBJECT", "M31"),
            keyword("OBSERVER", "someone"),
            keyword("DATE-OBS", "2024-01-15"),
            keyword("TIME-OBS", "20:30:00.123"),
            keyword("CCD-TEMP", "-10.5"),
            keyword("FILTER", "Ha"),
            keyword("GAIN", "120"),
        ]);

        let meta = ObservationalMetadata::from_header(&header);
        assert_eq!(meta.exposure, Some(120.0));
        assert_eq!(meta.iso, Some(800));
        // The focal length and the f-number are two different things.
        assert_eq!(meta.focal_length, Some(24.0));
        assert_eq!(meta.f_number, Some(2.8));
        assert_eq!(meta.instrument.as_deref(), Some("QHY268M"));
        assert_eq!(meta.telescope.as_deref(), Some("William Optics"));
        assert_eq!(meta.object.as_deref(), Some("M31"));
        assert_eq!(meta.observer.as_deref(), Some("someone"));
        assert_eq!(meta.date_obs.as_deref(), Some("2024-01-15"));
        assert_eq!(meta.time_obs.as_deref(), Some("20:30:00.123"));
        assert_eq!(meta.ccd_temp, Some(-10.5));
        assert_eq!(meta.filter.as_deref(), Some("Ha"));
        assert_eq!(meta.gain, Some(120.0));
    }

    #[test]
    fn a_namespaced_xisf_property_is_understood() {
        let header = header_with(&[
            keyword("Instrument:ExposureTime", "30.0"),
            keyword("Instrument:Gain", "3.5"),
            keyword("Object:Name", "NGC 7000"),
        ]);
        let meta = ObservationalMetadata::from_header(&header);
        assert_eq!(meta.exposure, Some(30.0));
        assert_eq!(meta.gain, Some(3.5));
        assert_eq!(meta.object.as_deref(), Some("NGC 7000"));
    }

    /// A keyword written verbatim wins over the same name inside a namespace.
    #[test]
    fn a_verbatim_keyword_wins_over_a_namespaced_one() {
        let header = header_with(&[
            keyword("Instrument:ExposureTime", "1.0"),
            keyword("EXPTIME", "300.0"),
        ]);
        let meta = ObservationalMetadata::from_header(&header);
        assert_eq!(meta.exposure, Some(300.0));
    }

    /// The whole point of the rational exposure time: no truncation.
    #[test]
    fn a_short_exposure_keeps_its_fraction() {
        assert_eq!(exposure_rational(0.5), Some(uR64 { nominator: 1, denominator: 2 }));
        assert_eq!(exposure_rational(0.0005), Some(uR64 { nominator: 1, denominator: 2000 }));
        assert_eq!(exposure_rational(0.016), Some(uR64 { nominator: 1, denominator: 63 }));
    }

    #[test]
    fn a_long_exposure_keeps_its_decimal_part() {
        assert_eq!(exposure_rational(30.7), Some(uR64 { nominator: 307, denominator: 10 }));
        assert_eq!(exposure_rational(120.0), Some(uR64 { nominator: 120, denominator: 1 }));
        assert_eq!(exposure_rational(1.0), Some(uR64 { nominator: 1, denominator: 1 }));
    }

    #[test]
    fn a_nonsense_exposure_is_dropped() {
        assert_eq!(exposure_rational(0.0), None);
        assert_eq!(exposure_rational(-1.0), None);
        assert_eq!(exposure_rational(f64::NAN), None);
        assert_eq!(exposure_rational(f64::INFINITY), None);
    }

    #[test]
    fn a_decimal_becomes_a_reduced_rational() {
        assert_eq!(scalar_rational(2.8), Some(uR64 { nominator: 14, denominator: 5 }));
        assert_eq!(scalar_rational(24.0), Some(uR64 { nominator: 24, denominator: 1 }));
        assert_eq!(scalar_rational(14.4), Some(uR64 { nominator: 72, denominator: 5 }));
    }

    #[test]
    fn the_date_becomes_the_exif_shape() {
        let meta = ObservationalMetadata {
            date_obs: Some("2024-01-15".to_string()),
            time_obs: Some("20:30:00.123".to_string()),
            ..Default::default()
        };
        assert_eq!(meta.exif_datetime().as_deref(), Some("2024:01:15 20:30:00"));
    }

    #[test]
    fn a_date_that_carries_its_own_time_is_used() {
        let meta = ObservationalMetadata {
            date_obs: Some("2024-01-15T20:30:00".to_string()),
            ..Default::default()
        };
        assert_eq!(meta.exif_datetime().as_deref(), Some("2024:01:15 20:30:00"));
    }

    #[test]
    fn a_missing_time_becomes_midnight() {
        let meta = ObservationalMetadata {
            date_obs: Some("2024-01-15".to_string()),
            ..Default::default()
        };
        assert_eq!(meta.exif_datetime().as_deref(), Some("2024:01:15 00:00:00"));
    }

    #[test]
    fn sexagesimal_coordinates_become_degrees() {
        // 05h 35m 17.3s → 83.8221°
        let ra = parse_sexagesimal("05:35:17.3", 15.0).unwrap();
        assert!((ra - 83.82208).abs() < 1e-4, "{ra}");
        // -00° 30' 00" → -0.5°
        let dec = parse_sexagesimal("-00:30:00", 1.0).unwrap();
        assert!((dec + 0.5).abs() < 1e-9, "{dec}");
    }

    #[test]
    fn the_encoded_block_carries_the_expected_tags() {
        let header = header_with(&[
            keyword("EXPTIME", "0.5"),
            keyword("ISO", "1600"),
            keyword("FOCALLEN", "24.00"),
            keyword("FNUMBER", "2.8"),
            keyword("INSTRUME", "QHY268M"),
            keyword("TELESCOP", "William Optics"),
            keyword("OBJECT", "M31"),
            keyword("OBSERVER", "someone"),
            keyword("DATE-OBS", "2024-01-15"),
            keyword("TIME-OBS", "20:30:00"),
        ]);
        let meta = ObservationalMetadata::from_header(&header);
        let payload = meta.exif_payload().unwrap();

        // The exposure keeps its fraction: 0.5 s is 1/2.
        match read_exif_tag(&payload, 0x829a) {
            Some(ExifTag::ExposureTime(value)) => {
                assert_eq!(value, vec![uR64 { nominator: 1, denominator: 2 }]);
            }
            other => panic!("exposure time missing: {other:?}"),
        }
        // The focal length is the focal length, not the f-number.
        match read_exif_tag(&payload, 0x920a) {
            Some(ExifTag::FocalLength(value)) => {
                assert_eq!(value, vec![uR64 { nominator: 24, denominator: 1 }]);
            }
            other => panic!("focal length missing: {other:?}"),
        }
        match read_exif_tag(&payload, 0x829d) {
            Some(ExifTag::FNumber(value)) => {
                assert_eq!(value, vec![uR64 { nominator: 14, denominator: 5 }]);
            }
            other => panic!("f-number missing: {other:?}"),
        }
        match read_exif_tag(&payload, 0x8827) {
            Some(ExifTag::ISO(value)) => assert_eq!(value, vec![1600]),
            other => panic!("ISO missing: {other:?}"),
        }
        match read_exif_tag(&payload, 0x9003) {
            Some(ExifTag::DateTimeOriginal(value)) => {
                assert_eq!(value, "2024:01:15 20:30:00");
            }
            other => panic!("date missing: {other:?}"),
        }
        match read_tag(&payload, 0x0110, ExifTagGroup::GENERIC) {
            Some(ExifTag::Model(value)) => assert_eq!(value, "QHY268M"),
            other => panic!("model missing: {other:?}"),
        }
        match read_tag(&payload, 0x010f, ExifTagGroup::GENERIC) {
            Some(ExifTag::Make(value)) => assert_eq!(value, "William Optics"),
            other => panic!("make missing: {other:?}"),
        }
        match read_tag(&payload, 0x010e, ExifTagGroup::GENERIC) {
            Some(ExifTag::ImageDescription(value)) => assert_eq!(value, "M31"),
            other => panic!("object missing: {other:?}"),
        }
        match read_tag(&payload, 0x013b, ExifTagGroup::GENERIC) {
            Some(ExifTag::Artist(value)) => assert_eq!(value, "someone"),
            other => panic!("observer missing: {other:?}"),
        }
    }

    #[test]
    fn the_coordinates_become_gps_tags() {
        let header = header_with(&[
            keyword("RA", "05:35:17.3"),
            keyword("DEC", "-05:23:28"),
        ]);
        let meta = ObservationalMetadata::from_header(&header);
        let payload = meta.exif_payload().unwrap();

        match read_gps_tag(&payload, 0x0001) {
            Some(ExifTag::GPSLatitudeRef(value)) => assert_eq!(value, "S"),
            other => panic!("latitude ref missing: {other:?}"),
        }
        match read_gps_tag(&payload, 0x0003) {
            Some(ExifTag::GPSLongitudeRef(value)) => assert_eq!(value, "E"),
            other => panic!("longitude ref missing: {other:?}"),
        }
        match read_gps_tag(&payload, 0x0002) {
            Some(ExifTag::GPSLatitude(value)) => {
                assert_eq!(value.len(), 3);
                // -5° 23' 28"
                assert_eq!(value[0], uR64 { nominator: 5, denominator: 1 });
                assert_eq!(value[1], uR64 { nominator: 23, denominator: 1 });
            }
            other => panic!("latitude missing: {other:?}"),
        }
    }

    #[test]
    fn the_sensor_temperature_is_written_in_celsius() {
        let header = header_with(&[keyword("CCD-TEMP", "-10.5")]);
        let meta = ObservationalMetadata::from_header(&header);
        let payload = meta.exif_payload().unwrap();
        match read_exif_tag(&payload, 0x9400) {
            Some(ExifTag::AmbientTemperature(value)) => {
                assert_eq!(value, vec![iR64 { nominator: -21, denominator: 2 }]);
            }
            other => panic!("temperature missing: {other:?}"),
        }
    }

    #[test]
    fn the_fields_without_a_standard_tag_go_into_the_user_comment() {
        let header = header_with(&[
            keyword("GAIN", "120"),
            keyword("FILTER", "Ha"),
            keyword("SITE", "Beijing"),
        ]);
        let meta = ObservationalMetadata::from_header(&header);
        let payload = meta.exif_payload().unwrap();
        match read_exif_tag(&payload, 0x9286) {
            Some(ExifTag::UserComment(value)) => {
                assert_eq!(&value[..8], b"ASCII\0\0\0");
                let text = String::from_utf8_lossy(&value[8..]);
                assert!(text.contains("Gain=120"), "{text}");
                assert!(text.contains("Filter=Ha"), "{text}");
                assert!(text.contains("Site=Beijing"), "{text}");
            }
            other => panic!("user comment missing: {other:?}"),
        }
    }

    /// The injection must not disturb a single byte of the image data.
    #[test]
    fn the_injection_leaves_the_rest_of_the_stream_alone() {
        let jpeg = vec![0xff, 0xd8, 0xff, 0xdb, 0x00, 0x01, 0x42, 0xff, 0xd9];
        let exif = vec![0x49, 0x49, 0x2a, 0x00];
        let out = inject_exif(&jpeg, &exif).unwrap();

        assert_eq!(&out[..2], &[0xff, 0xd8]);
        assert_eq!(&out[2..4], &[0xff, 0xe1]);
        // The length field counts itself and the payload.
        let length = u16::from_be_bytes([out[4], out[5]]) as usize;
        assert_eq!(length, 2 + 6 + exif.len());
        assert_eq!(&out[6..12], b"Exif\0\0");
        assert_eq!(&out[12..16], &exif[..]);
        // The original bytes follow unchanged.
        assert_eq!(&out[16..], &jpeg[2..]);
    }

    #[test]
    fn a_stream_that_is_not_a_jpeg_is_refused() {
        assert!(inject_exif(&[0x00, 0x01, 0x02], &[0x00]).is_err());
        assert!(inject_exif(&[0xff], &[0x00]).is_err());
    }
}
