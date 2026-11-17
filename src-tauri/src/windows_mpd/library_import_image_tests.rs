use super::*;
use base64::Engine;
fn pixel() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP4sE/uPwAHOgLM1IPMwgAAAABJRU5ErkJggg==").unwrap()
}
fn jpeg_header(width: u16, height: u16) -> Vec<u8> {
    let mut b = vec![0xff, 0xd8, 0xff, 0xc0, 0, 8, 8];
    b.extend(height.to_be_bytes());
    b.extend(width.to_be_bytes());
    b.push(1);
    b
}
#[test]
fn real_png_structure_and_output_limit_are_distinct_from_native_decode_claim() {
    let b = pixel();
    let p = plan(&b).unwrap();
    assert_eq!(
        (p.width, p.height, p.thumb_width, p.thumb_height),
        (1, 1, 1, 1)
    );
    let mut fixed = vec![0u8; LIMIT];
    fixed[..b.len()].copy_from_slice(&b);
    assert_eq!(png(&fixed), Some((b.len(), 1, 1)));
    assert!(
        plan(&fixed).is_none(),
        "input trailing padding is not accepted"
    );
}
#[test]
fn declared_source_pixels_and_aspect_ratio_are_bounded_before_any_decoder() {
    for (w, h, tw, th) in [
        (4096, 4096, 256, 256),
        (4096, 1, 256, 1),
        (1, 4096, 1, 256),
        (200, 100, 200, 100),
        (1000, 500, 256, 128),
    ] {
        let p = plan(&jpeg_header(w, h)).unwrap();
        assert_eq!((p.thumb_width, p.thumb_height), (tw, th));
    }
    for (w, h) in [(0, 1), (1, 0), (4097, 1), (1, 4097), (65535, 65535)] {
        assert!(plan(&jpeg_header(w, h)).is_none());
    }
}
#[test]
fn animation_truncation_chunk_overflow_duplicate_headers_and_bad_signatures_are_rejected() {
    let b = pixel();
    let mut animated = b[..33].to_vec();
    animated.extend(0u32.to_be_bytes());
    animated.extend(b"acTL");
    animated.extend([0; 4]);
    animated.extend(&b[33..]);
    assert!(plan(&animated).is_none());
    for n in 0..b.len() {
        assert!(plan(&b[..n]).is_none());
    }
    let mut huge = b.clone();
    huge[33..37].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(plan(&huge).is_none());
    let mut duplicate = b[..33].to_vec();
    duplicate.extend(&b[8..33]);
    duplicate.extend(&b[33..]);
    assert!(plan(&duplicate).is_none());
    for bad in [
        b"GIF89a".as_slice(),
        b"RIFFxxxxWEBP",
        b"https://example.invalid/image",
        b"data:image/png;base64,xxxx",
        &[0xff, 0xd8, 0xff],
    ] {
        assert!(plan(bad).is_none());
    }
}
#[test]
fn compressed_input_byte_limit_and_png_integer_bounds_are_hard_limits() {
    let mut b = pixel();
    b.resize(LIMIT + 1, 0);
    assert!(plan(&b).is_none());
    let mut b = pixel();
    b[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(plan(&b).is_none());
}
#[cfg(not(windows))]
#[test]
fn portable_host_never_reports_a_simulated_os_thumbnail_as_native_success() {
    assert_eq!(thumbnail(&pixel()), Err("windows_feature_unavailable"));
}
