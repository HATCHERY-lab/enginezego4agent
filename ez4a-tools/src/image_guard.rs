// Portions derived from pi-durable 1.5.2 (packages/durable/src/tools/image.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 tools/edit-diff and tools/image, MIT, see LICENSES/pi-MIT.txt.

//! Refuse an image by extension and byte length. Pixels are not decoded.

/// Largest accepted image. One byte through this length is allowed.
const MAX_IMAGE_BYTES: u64 = 5_000_000;

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// Accept `png`, `jpg`, `jpeg`, `gif`, and `webp` when `byte_len` is `1..=5_000_000`.
///
/// The extension match is ASCII case-insensitive and uses the final path
/// segment. Any other extension, a missing extension, an empty file, or a file
/// past the byte cap is an error that says why. This does not read or decode
/// the file.
pub fn image_refusal(path: &str, byte_len: u64) -> Result<(), String> {
    if !supported_extension(path) {
        return Err("bad extension".to_string());
    }
    if byte_len == 0 {
        return Err("empty image".to_string());
    }
    if byte_len > MAX_IMAGE_BYTES {
        return Err(format!("too large ({byte_len} bytes)"));
    }
    Ok(())
}

fn supported_extension(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || extension.is_empty() {
        return false;
    }
    IMAGE_EXTENSIONS
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
mod tests {
    use super::image_refusal;

    #[test]
    fn accepts_inline_extensions_inside_the_byte_cap() {
        for extension in ["png", "jpg", "jpeg", "gif", "webp", "PNG", "JpEg"] {
            let path = format!("dir/sub\\shot.{extension}");
            image_refusal(&path, 1).unwrap();
            image_refusal(&path, 5_000_000).unwrap();
        }
        image_refusal(r"C:\pics\a.b.webp", 32).unwrap();
    }

    #[test]
    fn refuses_a_bad_extension_without_decoding() {
        for path in ["shot.bmp", "shot", "shot.png.txt", ".png", "shot.png."] {
            let error = image_refusal(path, 12).unwrap_err();
            assert_eq!(error, "bad extension");
        }
    }

    #[test]
    fn refuses_an_empty_or_oversized_image() {
        assert_eq!(image_refusal("shot.png", 0).unwrap_err(), "empty image");
        let error = image_refusal(r"C:\pics\shot.webp", 5_000_001).unwrap_err();
        assert_eq!(error, "too large (5000001 bytes)");
        assert_eq!(
            image_refusal("notes.txt", 9_000_000).unwrap_err(),
            "bad extension"
        );
    }
}
