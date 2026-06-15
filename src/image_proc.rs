use resize::Pixel::RGBA8;
use resize::Type::Triangle;
use rgb::FromSlice;

/// Resizes a 32-bit pixel buffer (RGBA/BGRA) from `(src_w, src_h)` to `(dst_w, dst_h)`,
/// and swaps the Red and Blue channels (RGBA <-> BGRA).
pub fn resize_and_convert(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
) -> Result<(), &'static str> {
    // 1. Validation
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Err("Dimensions cannot be zero");
    }

    let src_len = (src_w as usize)
        .checked_mul(src_h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("Overflow calculating source buffer size")?;

    let dst_len = (dst_w as usize)
        .checked_mul(dst_h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("Overflow calculating destination buffer size")?;

    if src.len() != src_len {
        return Err("Source buffer size does not match dimensions");
    }
    if dst.len() != dst_len {
        return Err("Destination buffer size does not match dimensions");
    }

    // 2. Perform resizing and channel swapping
    if src_w == dst_w && src_h == dst_h {
        // Fast path: dimensions are identical, just copy and swap channels in one pass
        for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    } else {
        // Slow path: resize first, then swap channels in-place in dst
        let src_rgba = src.as_rgba();
        let dst_rgba = dst.as_rgba_mut();

        let mut resizer = resize::new(
            src_w as usize,
            src_h as usize,
            dst_w as usize,
            dst_h as usize,
            RGBA8,
            Triangle,
        ).map_err(|_| "Failed to initialize resizer")?;

        resizer
            .resize(src_rgba, dst_rgba)
            .map_err(|_| "Resizing failed")?;

        // Swap Red and Blue channels (0 and 2) in-place efficiently
        for pixel in dst.chunks_exact_mut(4) {
            let r = pixel[0];
            let b = pixel[2];
            pixel[0] = b;
            pixel[2] = r;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_size_swap() {
        // A 2x2 image where each pixel is [R, G, B, A]
        // Pixel 0: [255, 0, 0, 255] (Red)
        // Pixel 1: [0, 255, 0, 255] (Green)
        // Pixel 2: [0, 0, 255, 255] (Blue)
        // Pixel 3: [10, 20, 30, 40]
        let src = vec![
            255, 0, 0, 255,
            0, 255, 0, 255,
            0, 0, 255, 255,
            10, 20, 30, 40,
        ];
        let mut dst = vec![0u8; 16];

        let res = resize_and_convert(&src, 2, 2, &mut dst, 2, 2);
        assert!(res.is_ok());

        // Expected output is swapped R and B:
        // Pixel 0: [0, 0, 255, 255] (Blue)
        // Pixel 1: [0, 255, 0, 255] (Green)
        // Pixel 2: [255, 0, 0, 255] (Red)
        // Pixel 3: [30, 20, 10, 40]
        let expected = vec![
            0, 0, 255, 255,
            0, 255, 0, 255,
            255, 0, 0, 255,
            30, 20, 10, 40,
        ];
        assert_eq!(dst, expected);
    }

    #[test]
    fn test_downscale_and_swap() {
        // A 4x4 image with constant solid red [255, 0, 0, 255]
        let src = vec![255, 0, 0, 255].repeat(16);
        let mut dst = vec![0u8; 4]; // 1x1 destination

        let res = resize_and_convert(&src, 4, 4, &mut dst, 1, 1);
        assert!(res.is_ok());

        // Since it's solid red, after swap it should be solid blue [0, 0, 255, 255]
        assert_eq!(dst, vec![0, 0, 255, 255]);
    }

    #[test]
    fn test_validation() {
        let src = vec![0u8; 16];
        let mut dst = vec![0u8; 16];

        // Invalid dimensions (zero)
        assert!(resize_and_convert(&src, 0, 2, &mut dst, 2, 2).is_err());
        assert!(resize_and_convert(&src, 2, 2, &mut dst, 2, 0).is_err());

        // Source buffer size mismatch
        assert!(resize_and_convert(&src[..12], 2, 2, &mut dst, 2, 2).is_err());

        // Destination buffer size mismatch
        assert!(resize_and_convert(&src, 2, 2, &mut dst[..12], 2, 2).is_err());
    }
}
