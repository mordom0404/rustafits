//! Contract tests for the EXIF block: it must be readable, and it must not
//! disturb a single byte of the image data.
//!
//! The image data is checked with `jpeg-decoder`, an *independent* pure-Rust
//! decoder, so a symmetric bug in the encoder cannot hide behind a
//! self-round-trip.

use astroimage::{encode_jpeg, encode_jpeg_with_exif, ObservationalMetadata};

/// Deterministic content with enough structure that a damaged image shows up.
fn synth(w: usize, h: usize) -> Vec<u8> {
    let mut v = vec![0u8; w * h * 3];
    let mut s: u32 = 0x9e37_79b9;
    let mut rng = move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    for px in v.chunks_exact_mut(3) {
        let n = (rng() % 40) as u8 + 10;
        px[0] = n;
        px[1] = n / 2;
        px[2] = 255 - n;
    }
    v
}

fn metadata() -> ObservationalMetadata {
    ObservationalMetadata {
        date_obs: Some("2024-01-15".to_string()),
        time_obs: Some("20:30:00".to_string()),
        exposure: Some(0.5),
        iso: Some(1600),
        f_number: Some(2.8),
        focal_length: Some(24.0),
        ccd_temp: Some(-10.5),
        instrument: Some("QHY268M".to_string()),
        telescope: Some("William Optics".to_string()),
        object: Some("M31".to_string()),
        observer: Some("someone".to_string()),
        filter: Some("Ha".to_string()),
        gain: Some(120.0),
        ..Default::default()
    }
}

/// The byte offset where the APP1 segment ends and the original JPEG starts.
fn app1_end(jpeg: &[u8]) -> usize {
    assert_eq!(&jpeg[..2], &[0xff, 0xd8], "SOI marker");
    assert_eq!(&jpeg[2..4], &[0xff, 0xe1], "APP1 marker");
    let length = u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
    assert!(length >= 2);
    4 + length
}

/// The EXIF block of a JPEG, without the `Exif\0\0` header.
fn exif_block(jpeg: &[u8]) -> Vec<u8> {
    let start = 6;
    let end = app1_end(jpeg);
    assert_eq!(&jpeg[start..start + 6], b"Exif\0\0");
    jpeg[start + 6..end].to_vec()
}

fn read_tag(exif_tiff: &[u8], hex: u16, group: little_exif::ifd::ExifTagGroup) -> Option<little_exif::exif_tag::ExifTag> {
    let metadata =
        little_exif::metadata::Metadata::new_from_vec(&exif_tiff.to_vec(), little_exif::filetype::FileExtension::TIFF)
            .ok()?;
    metadata.get_tag_by_hex(hex, Some(group)).next().cloned()
}

/// The EXIF block must not change the image data: everything behind the APP1
/// segment has to be byte-identical to the plain encode.
#[test]
fn the_exif_block_leaves_the_image_data_untouched() {
    let (w, h) = (96usize, 64usize);
    let src = synth(w, h);

    let plain = encode_jpeg(&src, w, h, 3, 90).expect("encode failed");
    let with_exif = encode_jpeg_with_exif(&src, w, h, 3, 90, &metadata()).expect("encode failed");

    // The plain JPEG has no EXIF to begin with.
    assert!(
        !plain.windows(4).any(|w| w == b"Exif"),
        "the plain encoder already wrote an EXIF header"
    );

    let split = app1_end(&with_exif);
    assert_eq!(
        &with_exif[split..],
        &plain[2..],
        "the bytes behind the APP1 segment differ from the plain JPEG"
    );
}

/// An independent decoder must still accept the file and see the same pixels.
#[test]
fn a_jpeg_with_exif_still_decodes_to_the_same_pixels() {
    let (w, h) = (96usize, 64usize);
    let src = synth(w, h);

    let plain = encode_jpeg(&src, w, h, 3, 90).expect("encode failed");
    let with_exif = encode_jpeg_with_exif(&src, w, h, 3, 90, &metadata()).expect("encode failed");

    let decode = |jpeg: &[u8]| {
        let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(jpeg));
        let pixels = d.decode().expect("independent decoder rejected our JPEG");
        let info = d.info().expect("no frame info");
        assert_eq!((info.width as usize, info.height as usize), (w, h));
        pixels
    };

    assert_eq!(decode(&plain), decode(&with_exif), "the pixels changed");
}

/// The block must carry the header's keywords, with the exposure keeping its
/// fraction and the focal length staying separate from the f-number.
#[test]
fn the_block_carries_the_header_keywords() {
    let (w, h) = (32usize, 24usize);
    let src = synth(w, h);
    let jpeg = encode_jpeg_with_exif(&src, w, h, 3, 90, &metadata()).expect("encode failed");
    let block = exif_block(&jpeg);

    match read_tag(&block, 0x829a, little_exif::ifd::ExifTagGroup::EXIF) {
        Some(little_exif::exif_tag::ExifTag::ExposureTime(value)) => {
            assert_eq!(value, vec![little_exif::rational::uR64 { nominator: 1, denominator: 2 }]);
        }
        other => panic!("exposure time missing: {other:?}"),
    }
    match read_tag(&block, 0x920a, little_exif::ifd::ExifTagGroup::EXIF) {
        Some(little_exif::exif_tag::ExifTag::FocalLength(value)) => {
            assert_eq!(value, vec![little_exif::rational::uR64 { nominator: 24, denominator: 1 }]);
        }
        other => panic!("focal length missing: {other:?}"),
    }
    match read_tag(&block, 0x829d, little_exif::ifd::ExifTagGroup::EXIF) {
        Some(little_exif::exif_tag::ExifTag::FNumber(value)) => {
            assert_eq!(value, vec![little_exif::rational::uR64 { nominator: 14, denominator: 5 }]);
        }
        other => panic!("f-number missing: {other:?}"),
    }
    match read_tag(&block, 0x0110, little_exif::ifd::ExifTagGroup::GENERIC) {
        Some(little_exif::exif_tag::ExifTag::Model(value)) => assert_eq!(value, "QHY268M"),
        other => panic!("model missing: {other:?}"),
    }
}

/// A PNG must not grow an EXIF block — the metadata has no home there.
#[test]
fn png_output_stays_free_of_exif() {
    let dir = std::env::temp_dir().join(format!("rustafits-exif-png-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.png");

    let image = astroimage::ProcessedImage {
        data: synth(16, 16),
        width: 16,
        height: 16,
        is_color: true,
        channels: 3,
        flip_vertical: false,
    };
    astroimage::ImageConverter::save_processed_with_exif(&image, &path, 90, &metadata())
        .expect("png save failed");

    let bytes = std::fs::read(&path).unwrap();
    assert!(
        !bytes.windows(4).any(|w| w == b"Exif"),
        "the PNG carries an EXIF header"
    );
    std::fs::remove_dir_all(&dir).ok();
}
