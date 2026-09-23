//! MIME type detection from magic bytes (file signatures).

/// Detect MIME type from the leading bytes of a file.
///
/// Returns `Some(mime_type)` for recognized signatures, `None` otherwise.
/// Based on well-known magic byte sequences for common media, document, and archive formats.
pub fn detect_mime_type(data: &[u8]) -> Option<&'static str> {
    if data.len() < 4 {
        return None;
    }

    // Images
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // BMP
    if data.starts_with(b"BM") && data.len() >= 6 {
        return Some("image/bmp");
    }
    // TIFF (little-endian and big-endian)
    if data.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || data.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        return Some("image/tiff");
    }
    // PSD
    if data.starts_with(b"8BPS") {
        return Some("image/vnd.adobe.photoshop");
    }
    // JPEG XL
    if data.starts_with(&[0xFF, 0x0A])
        || (data.len() >= 12 && data[0..4] == [0x00, 0x00, 0x00, 0x0C] && &data[4..8] == b"JXL ")
    {
        return Some("image/jxl");
    }
    // JPEG 2000
    if data.len() >= 12 && data[0..4] == [0x00, 0x00, 0x00, 0x0C] && &data[4..8] == b"jP  " {
        return Some("image/jp2");
    }

    // ftyp-based formats (HEIC/HEIF, AVIF, MP4, MOV)
    // Must be checked before ICO: an ftyp box with size 0x100 shares ICO's magic bytes
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        let brand = &data[8..12];
        // HEIC/HEIF
        if brand == b"heic" || brand == b"heix" || brand == b"hevc" || brand == b"hevx" {
            return Some("image/heic");
        }
        if brand == b"mif1" || brand == b"msf1" {
            return Some("image/heif");
        }
        // AVIF
        if brand == b"avif" || brand == b"avis" {
            return Some("image/avif");
        }
        // MP4
        if brand == b"mp41" || brand == b"mp42" || brand == b"isom" || brand == b"M4V " {
            return Some("video/mp4");
        }
        // M4A (audio in MP4 container)
        if brand == b"M4A " {
            return Some("audio/mp4");
        }
        // QuickTime MOV
        if brand == b"qt  " {
            return Some("video/quicktime");
        }
    }

    // ICO (after ftyp to avoid false positive on ftyp boxes with size 0x100)
    if data.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return Some("image/x-icon");
    }

    // Audio
    // MP3: ID3 tag or frame sync
    if data.starts_with(&[0x49, 0x44, 0x33]) || data.starts_with(&[0xFF, 0xFB]) {
        return Some("audio/mpeg");
    }
    // WAV
    if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WAVE" {
        return Some("audio/wav");
    }
    // OGG (audio/video — default to audio)
    if data.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    // FLAC
    if data.starts_with(b"fLaC") {
        return Some("audio/flac");
    }
    // MIDI
    if data.starts_with(b"MThd") {
        return Some("audio/midi");
    }

    // Video
    // WebM / MKV (EBML header)
    if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("video/webm");
    }
    // AVI
    if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"AVI " {
        return Some("video/x-msvideo");
    }

    // Documents
    // PDF
    if data.starts_with(b"%PDF") {
        return Some("application/pdf");
    }
    // ZIP (and ZIP-based: DOCX, XLSX, PPTX)
    if data.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
        return Some("application/zip");
    }

    None
}

/// Check if a string is a valid MIME type (e.g., "image/png", "application/octet-stream").
/// Requires exactly one `/`, non-empty type and subtype, and only valid MIME characters.
pub fn is_valid_mime_type(s: &str) -> bool {
    let Some((type_part, subtype)) = s.split_once('/') else {
        return false;
    };
    if type_part.is_empty() || subtype.is_empty() {
        return false;
    }
    // Reject multiple slashes
    if subtype.contains('/') {
        return false;
    }
    s.bytes().all(|b| {
        b.is_ascii_alphanumeric() || b == b'/' || b == b'.' || b == b'-' || b == b'+' || b == b'_'
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_data(header: &[u8], total: usize) -> Vec<u8> {
        let mut d = header.to_vec();
        d.resize(total, 0);
        d
    }

    #[test]
    fn test_jpeg() {
        assert_eq!(
            detect_mime_type(&make_data(&[0xFF, 0xD8, 0xFF, 0xE0], 16)),
            Some("image/jpeg")
        );
    }

    #[test]
    fn test_png() {
        assert_eq!(
            detect_mime_type(&make_data(
                &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
                16
            )),
            Some("image/png")
        );
    }

    #[test]
    fn test_gif() {
        assert_eq!(
            detect_mime_type(b"GIF89a\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00"),
            Some("image/gif")
        );
        assert_eq!(
            detect_mime_type(b"GIF87a\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00"),
            Some("image/gif")
        );
    }

    #[test]
    fn test_webp() {
        let mut d = b"RIFF".to_vec();
        d.extend_from_slice(&[0; 4]); // size
        d.extend_from_slice(b"WEBP");
        assert_eq!(detect_mime_type(&d), Some("image/webp"));
    }

    #[test]
    fn test_pdf() {
        assert_eq!(
            detect_mime_type(b"%PDF-1.7\x00\x00\x00\x00"),
            Some("application/pdf")
        );
    }

    #[test]
    fn test_mp3_id3() {
        assert_eq!(
            detect_mime_type(&make_data(&[0x49, 0x44, 0x33], 16)),
            Some("audio/mpeg")
        );
    }

    #[test]
    fn test_mp4() {
        let mut d = vec![0x00, 0x00, 0x00, 0x18];
        d.extend_from_slice(b"ftypisom");
        d.extend_from_slice(&[0; 4]);
        assert_eq!(detect_mime_type(&d), Some("video/mp4"));
    }

    #[test]
    fn test_zip() {
        assert_eq!(
            detect_mime_type(&make_data(&[0x50, 0x4B, 0x03, 0x04], 16)),
            Some("application/zip")
        );
    }

    #[test]
    fn test_wav() {
        let mut d = b"RIFF".to_vec();
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(b"WAVE");
        assert_eq!(detect_mime_type(&d), Some("audio/wav"));
    }

    #[test]
    fn test_flac() {
        assert_eq!(
            detect_mime_type(b"fLaC\x00\x00\x00\x00"),
            Some("audio/flac")
        );
    }

    #[test]
    fn test_heic() {
        let mut d = vec![0x00, 0x00, 0x00, 0x18];
        d.extend_from_slice(b"ftypheic");
        d.extend_from_slice(&[0; 4]);
        assert_eq!(detect_mime_type(&d), Some("image/heic"));
    }

    #[test]
    fn test_avif() {
        let mut d = vec![0x00, 0x00, 0x00, 0x18];
        d.extend_from_slice(b"ftypavif");
        d.extend_from_slice(&[0; 4]);
        assert_eq!(detect_mime_type(&d), Some("image/avif"));
    }

    #[test]
    fn test_webm() {
        assert_eq!(
            detect_mime_type(&make_data(&[0x1A, 0x45, 0xDF, 0xA3], 16)),
            Some("video/webm")
        );
    }

    #[test]
    fn test_unknown_data() {
        let data: Vec<u8> = (0..64).map(|i| ((i * 0x12) % 256) as u8).collect();
        assert_eq!(detect_mime_type(&data), None);
    }

    #[test]
    fn test_short_data() {
        assert_eq!(detect_mime_type(&[0xFF, 0xD8]), None);
        assert_eq!(detect_mime_type(&[0xFF, 0xD8, 0xFF]), None);
        assert_eq!(detect_mime_type(&[0xFF, 0xFB, 0x90]), None);
        assert_eq!(detect_mime_type(&[]), None);
    }

    #[test]
    fn test_bmp() {
        assert_eq!(
            detect_mime_type(&make_data(b"BM\x00\x00\x00\x00", 16)),
            Some("image/bmp")
        );
    }

    #[test]
    fn test_tiff_le() {
        assert_eq!(
            detect_mime_type(&make_data(&[0x49, 0x49, 0x2A, 0x00], 16)),
            Some("image/tiff")
        );
    }

    #[test]
    fn test_tiff_be() {
        assert_eq!(
            detect_mime_type(&make_data(&[0x4D, 0x4D, 0x00, 0x2A], 16)),
            Some("image/tiff")
        );
    }

    #[test]
    fn test_ico() {
        assert_eq!(
            detect_mime_type(&make_data(&[0x00, 0x00, 0x01, 0x00], 16)),
            Some("image/x-icon")
        );
    }

    #[test]
    fn test_ogg() {
        assert_eq!(detect_mime_type(b"OggS\x00\x00\x00\x00"), Some("audio/ogg"));
    }

    #[test]
    fn test_midi() {
        assert_eq!(
            detect_mime_type(b"MThd\x00\x00\x00\x00"),
            Some("audio/midi")
        );
    }

    #[test]
    fn test_avi() {
        let mut d = b"RIFF".to_vec();
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(b"AVI ");
        assert_eq!(detect_mime_type(&d), Some("video/x-msvideo"));
    }

    #[test]
    fn test_mov() {
        let mut d = vec![0x00, 0x00, 0x00, 0x18];
        d.extend_from_slice(b"ftypqt  ");
        d.extend_from_slice(&[0; 4]);
        assert_eq!(detect_mime_type(&d), Some("video/quicktime"));
    }

    #[test]
    fn test_psd() {
        assert_eq!(
            detect_mime_type(b"8BPS\x00\x00\x00\x00"),
            Some("image/vnd.adobe.photoshop")
        );
    }

    #[test]
    fn test_m4a() {
        let mut d = vec![0x00, 0x00, 0x00, 0x18];
        d.extend_from_slice(b"ftypM4A ");
        d.extend_from_slice(&[0; 4]);
        assert_eq!(detect_mime_type(&d), Some("audio/mp4"));
    }

    // ========================================================================
    // is_valid_mime_type tests
    // ========================================================================

    #[test]
    fn test_valid_mime_types() {
        assert!(is_valid_mime_type("image/png"));
        assert!(is_valid_mime_type("image/jpeg"));
        assert!(is_valid_mime_type("application/octet-stream"));
        assert!(is_valid_mime_type(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        ));
        assert!(is_valid_mime_type("text/plain"));
        assert!(is_valid_mime_type("audio/mpeg"));
        assert!(is_valid_mime_type("video/mp4"));
        assert!(is_valid_mime_type("application/json"));
        assert!(is_valid_mime_type("image/svg+xml"));
    }

    #[test]
    fn test_invalid_mime_empty() {
        assert!(!is_valid_mime_type(""));
    }

    #[test]
    fn test_invalid_mime_no_slash() {
        assert!(!is_valid_mime_type("image"));
        assert!(!is_valid_mime_type("png"));
    }

    #[test]
    fn test_invalid_mime_multiple_slashes() {
        assert!(!is_valid_mime_type("image/png/extra"));
        assert!(!is_valid_mime_type("a/b/c"));
    }

    #[test]
    fn test_invalid_mime_special_chars() {
        assert!(!is_valid_mime_type("image png"));
        assert!(!is_valid_mime_type("image/png; charset=utf-8"));
        assert!(!is_valid_mime_type("../etc/passwd"));
        assert!(!is_valid_mime_type("image\0/png"));
    }

    #[test]
    fn test_valid_mime_with_special_allowed_chars() {
        // Dots, hyphens, plus, underscores are valid
        assert!(is_valid_mime_type("application/x-tar"));
        assert!(is_valid_mime_type("application/vnd.ms-excel"));
        assert!(is_valid_mime_type("application/atom+xml"));
        assert!(is_valid_mime_type("application/x_custom"));
    }

    #[test]
    fn test_invalid_mime_empty_type_or_subtype() {
        // Empty type part
        assert!(!is_valid_mime_type("/png"));
        // Empty subtype part
        assert!(!is_valid_mime_type("image/"));
        // Both empty
        assert!(!is_valid_mime_type("/"));
    }

    #[test]
    fn test_valid_mime_case_insensitive() {
        // MIME types are case-insensitive per RFC; our validator accepts uppercase
        assert!(is_valid_mime_type("IMAGE/PNG"));
        assert!(is_valid_mime_type("Application/JSON"));
    }
}
