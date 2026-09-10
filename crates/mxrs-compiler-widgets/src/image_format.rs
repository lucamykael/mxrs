//! Magic-byte image-format sniffing, ported from mxrb's
//! `compiler/model_values.rb#image_format`/`#image_bytes`.

use mxrs_bson::{Bson, Document};

use crate::support::{get, to_s};
use crate::CompilerError;

fn image_bytes(value: &Bson) -> Vec<u8> {
    match value {
        Bson::Binary(binary) => binary.bytes.clone(),
        Bson::Null => Vec::new(),
        Bson::String(s) => s.clone().into_bytes(),
        other => other.to_string().into_bytes(),
    }
}

/// `source['ImageFormat']` wins if present (case-insensitively lowercased);
/// otherwise sniffs magic bytes for PNG/GIF/JPG/BMP/ICO/WEBP, falling back
/// to a plain `<svg` substring scan (case-insensitive, word-bounded) over
/// the first 1024 bytes for SVG. Errors loudly — never guesses — when
/// nothing matches, mirroring the "fail loud rather than silently produce
/// wrong output" rule the rest of this workspace's compilers already
/// follow (see e.g. `mxrs-compiler-flow`'s `CompilerError` doc comments).
pub fn image_format(source: &Document) -> Result<String, CompilerError> {
    let explicit = to_s(&get(source, "ImageFormat")).to_lowercase();
    if !explicit.is_empty() {
        return Ok(explicit);
    }

    let bytes = image_bytes(&get(source, "Image"));
    if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        return Ok("png".to_string());
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok("gif".to_string());
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Ok("jpg".to_string());
    }
    if bytes.starts_with(b"BM") {
        return Ok("bmp".to_string());
    }
    if bytes.starts_with(b"\x00\x00\x01\x00") {
        return Ok("ico".to_string());
    }
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Ok("webp".to_string());
    }
    let head_len = bytes.len().min(1024);
    if contains_svg_tag(&bytes[..head_len]) {
        return Ok("svg".to_string());
    }

    Err(CompilerError::CannotDetermineImageFormat {
        name: to_s(&get(source, "Name")),
    })
}

/// Case-insensitive, word-bounded search for `<svg`, mirroring Ruby's
/// `/<svg\b/i` — a plain byte scan rather than pulling in a `regex`
/// dependency for one substring check.
fn contains_svg_tag(head: &[u8]) -> bool {
    let needle = b"<svg";
    if head.len() < needle.len() {
        return false;
    }
    head.windows(needle.len()).enumerate().any(|(i, window)| {
        window.eq_ignore_ascii_case(needle)
            && head
                .get(i + needle.len())
                .map(|&b| !(b.is_ascii_alphanumeric() || b == b'_'))
                .unwrap_or(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::{doc, Binary, BinarySubtype};

    fn img(bytes: &[u8]) -> Document {
        doc! {
            "Name": "Pic",
            "Image": Binary { subtype: BinarySubtype::Generic, bytes: bytes.to_vec() },
        }
    }

    #[test]
    fn sniffs_every_known_signature() {
        assert_eq!(image_format(&img(b"\x89PNG\r\n\x1A\nrest")).unwrap(), "png");
        assert_eq!(image_format(&img(b"GIF89arest")).unwrap(), "gif");
        assert_eq!(image_format(&img(b"\xFF\xD8\xFFrest")).unwrap(), "jpg");
        assert_eq!(image_format(&img(b"BMrest")).unwrap(), "bmp");
        assert_eq!(image_format(&img(b"\x00\x00\x01\x00rest")).unwrap(), "ico");
        assert_eq!(image_format(&img(b"RIFF1234WEBPrest")).unwrap(), "webp");
        assert_eq!(
            image_format(&img(b"prefix <svg xmlns=\"foo\">")).unwrap(),
            "svg"
        );
        assert_eq!(image_format(&img(b"  <SVG>")).unwrap(), "svg");
    }

    #[test]
    fn explicit_image_format_field_wins_over_sniffing() {
        let mut source = img(b"not an image at all");
        source.insert("ImageFormat", "SVG");
        assert_eq!(image_format(&source).unwrap(), "svg");
    }

    #[test]
    fn unknown_bytes_are_a_loud_error_not_a_guess() {
        let err = image_format(&img(b"nope, not a known format")).unwrap_err();
        assert!(matches!(
            err,
            CompilerError::CannotDetermineImageFormat { name } if name == "Pic"
        ));
    }

    #[test]
    fn svg_tag_followed_by_identifier_char_does_not_match() {
        // "<svgx" isn't a real tag open — the \b word-boundary check must reject it.
        assert!(!contains_svg_tag(b"<svgxmlns"));
    }
}
