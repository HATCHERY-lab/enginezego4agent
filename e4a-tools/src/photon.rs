//! engine4agent photon plan. photon-rs is linked only with the `photon` feature.

/// One Photon image op. Planning does not decode pixels or link the wasm runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhotonOp {
    /// Scale to `w` by `h` pixels. Either axis at `0` is refused.
    Resize { w: u32, h: u32 },
    /// Add `value` to each channel. Negative values darken. `0` is allowed.
    Brighten { value: i16 },
}

/// Accept `op` when `byte_len` would pass [`crate::image_refusal`] as a `.png`.
///
/// `0` and lengths above `5_000_000` reuse that function's errors. A
/// [`PhotonOp::Resize`] with `w` or `h` of `0` is an error. The size rule runs
/// first. Pixels are not decoded.
pub fn plan(op: PhotonOp, byte_len: u64) -> Result<(), String> {
    // `.png` passes the extension check, so only the shared size rule remains.
    crate::image_refusal("photon.png", byte_len)?;
    if let PhotonOp::Resize { w, h } = op {
        if w == 0 || h == 0 {
            return Err("resize dimension is zero".to_string());
        }
    }
    Ok(())
}

/// Name the Photon runtime this build links.
///
/// `"photon-rs"` when the `photon` feature is on. Otherwise the wasm module
/// is not linked.
pub fn runtime() -> &'static str {
    if cfg!(feature = "photon") {
        "photon-rs"
    } else {
        "photon wasm not linked"
    }
}

/// Open `bytes` with photon-rs, resize to `w` by `h`, and return PNG bytes.
///
/// `w` or `h` of `0` is an error before decode. A photon-rs failure is returned
/// as that error's text. Sampling is Lanczos3.
#[cfg(feature = "photon")]
pub fn resize_png(bytes: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    if w == 0 || h == 0 {
        return Err("resize dimension is zero".to_string());
    }
    let image =
        photon_rs::native::open_image_from_bytes(bytes).map_err(|error| error.to_string())?;
    let resized = photon_rs::transform::resize(
        &image,
        w,
        h,
        photon_rs::transform::SamplingFilter::Lanczos3,
    );
    Ok(resized.get_bytes())
}

#[cfg(test)]
mod tests {
    use super::{plan, runtime, PhotonOp};

    #[test]
    fn accepts_a_png_sized_resize_and_brighten() {
        plan(PhotonOp::Resize { w: 1, h: 1 }, 1).unwrap();
        plan(PhotonOp::Resize { w: 32, h: 32 }, 5_000_000).unwrap();
        plan(PhotonOp::Brighten { value: 0 }, 12).unwrap();
        plan(PhotonOp::Brighten { value: i16::MIN }, 12).unwrap();
        plan(PhotonOp::Brighten { value: i16::MAX }, 12).unwrap();
    }

    #[test]
    fn refuses_an_empty_or_oversized_buffer() {
        assert_eq!(
            plan(PhotonOp::Brighten { value: 1 }, 0).unwrap_err(),
            "empty image"
        );
        assert_eq!(
            plan(PhotonOp::Resize { w: 8, h: 8 }, 5_000_001).unwrap_err(),
            "too large (5000001 bytes)"
        );
        assert_eq!(
            plan(PhotonOp::Resize { w: 0, h: 0 }, 0).unwrap_err(),
            "empty image"
        );
    }

    #[test]
    fn refuses_a_zero_resize_dimension() {
        assert_eq!(
            plan(PhotonOp::Resize { w: 0, h: 8 }, 12).unwrap_err(),
            "resize dimension is zero"
        );
        assert_eq!(
            plan(PhotonOp::Resize { w: 8, h: 0 }, 12).unwrap_err(),
            "resize dimension is zero"
        );
        assert_eq!(
            plan(PhotonOp::Resize { w: 0, h: 0 }, 12).unwrap_err(),
            "resize dimension is zero"
        );
    }

    #[cfg(not(feature = "photon"))]
    #[test]
    fn runtime_is_unlinked() {
        assert_eq!(runtime(), "photon wasm not linked");
    }

    #[cfg(feature = "photon")]
    #[test]
    fn runtime_names_photon_rs() {
        assert_eq!(runtime(), "photon-rs");
    }

    #[cfg(feature = "photon")]
    #[test]
    fn resize_png_refuses_a_zero_dimension_before_decode() {
        assert_eq!(
            super::resize_png(&[], 0, 8).unwrap_err(),
            "resize dimension is zero"
        );
        assert_eq!(
            super::resize_png(&[1, 2, 3], 8, 0).unwrap_err(),
            "resize dimension is zero"
        );
    }
}
