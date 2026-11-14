use crate::codec::refine::{block_texels, refine_grey};
use crate::format::level_size;
use super::*;

/// Colour block with c0 = pure blue < c1 = pure red, every texel on index 3.
const COLOR_C0_LT_C1: [u8; 8] = [0x1F, 0x00, 0x00, 0xF8, 0xFF, 0xFF, 0xFF, 0xFF];

#[test]
fn dxt5_colour_block_is_always_four_colour() {
    // GPUs ignore the c0/c1 order in DXT3/DXT5: index 3 is (c0 + 2·c1) / 3, never black.
    let mut block = vec![255u8, 255, 0, 0, 0, 0, 0, 0];
    block.extend_from_slice(&COLOR_C0_LT_C1);
    let img = decode(f::DXT5, 4, 4, &block).unwrap();
    assert_eq!(&img.data[..4], &[170, 0, 85, 255]);
}

#[test]
fn dxt3_colour_block_is_always_four_colour() {
    let mut block = vec![0xFFu8; 8];
    block.extend_from_slice(&COLOR_C0_LT_C1);
    let img = decode(f::DXT3, 4, 4, &block).unwrap();
    assert_eq!(&img.data[..4], &[170, 0, 85, 255]);
}

#[test]
fn sixteen_bit_channels_round_to_eight_bits() {
    // One texel: R = 0xFFFF, G = 0x8000, B = 0x0000, A = 0x00FF.
    let data = [0xFF, 0xFF, 0x00, 0x80, 0x00, 0x00, 0xFF, 0x00];
    let img = decode(f::A16B16G16R16, 1, 1, &data).unwrap();
    assert_eq!(img.data, [255, 128, 0, 1]);
    assert_eq!(level_size(f::A16B16G16R16, 4, 2), 64);
}

#[test]
fn dxt1_three_colour_mode_keeps_punch_through() {
    let img = decode(f::DXT1, 4, 4, &COLOR_C0_LT_C1).unwrap();
    assert_eq!(&img.data[..4], &[0, 0, 0, 0]);
}

#[test]
fn strip_encoding_matches_whole_image() {
    // 128 x 200: several strips, the last one shorter.
    let mut img = Rgba8::new(128, 200);
    for (i, v) in img.data.iter_mut().enumerate() {
        *v = (i.wrapping_mul(2_654_435_761) >> 7) as u8;
    }
    for (format, squish, block_len) in [(f::DXT1, texpresso::Format::Bc1, 8), (f::DXT5, texpresso::Format::Bc3, 16)] {
        let mut pixels = img.data.clone();
        if format == f::DXT1 {
            pixels.chunks_exact_mut(4).for_each(|p| p[3] = 255);
        }
        let params = texpresso::Params {
            algorithm: texpresso::Algorithm::ClusterFit,
            weights: texpresso::COLOUR_WEIGHTS_UNIFORM,
            weigh_colour_by_alpha: false,
        };
        let mut whole = vec![0u8; squish.compressed_size(128, 200)];
        squish.compress(&pixels, 128, 200, params, &mut whole);
        for (i, block) in whole.chunks_exact_mut(block_len).enumerate() {
            let texels = block_texels(&pixels, 128, i % 32 * 4, i / 32 * 4);
            refine_grey(&texels, &mut block[block_len - 8..], format == f::DXT1);
        }
        assert_eq!(encode(format, &img).unwrap(), whole);
    }
}

fn dark_grey_noise(w: u32, h: u32) -> Rgba8 {
    // Near-black fabric: greys between 10 and 30.
    let mut img = Rgba8::new(w, h);
    for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
        let v = 10 + ((i.wrapping_mul(2_654_435_761) >> 11) % 21) as u8;
        p.copy_from_slice(&[v, v, v, 255]);
    }
    img
}

#[test]
fn dark_greys_stay_grey_and_keep_their_brightness() {
    let img = dark_grey_noise(64, 64);
    for format in [f::DXT1, f::DXT5] {
        let dec = decode(format, 64, 64, &encode(format, &img).unwrap()).unwrap();
        let (mut tint, mut shift) = (0i32, 0i64);
        for (s, d) in img.data.chunks_exact(4).zip(dec.data.chunks_exact(4)) {
            tint = tint.max(*d[..3].iter().max().unwrap() as i32 - *d[..3].iter().min().unwrap() as i32);
            shift += (0..3).map(|c| d[c] as i64 - s[c] as i64).sum::<i64>();
        }
        let shift = shift as f64 / (64.0 * 64.0 * 3.0);
        // Grey 565 endpoints may differ by one level between G and R/B.
        assert!(tint <= 2, "format {format:X}: tint {tint}");
        assert!(shift.abs() <= 0.5, "format {format:X}: shift {shift}");
    }
}

#[test]
fn dxt1_ignores_alpha() {
    // Normal maps from DXT5 sources go to DXT1 whatever their alpha: never punch-through.
    let mut img = dark_grey_noise(16, 16);
    img.data.chunks_exact_mut(4).for_each(|p| p[3] = 0);
    let dec = decode(f::DXT1, 16, 16, &encode(f::DXT1, &img).unwrap()).unwrap();
    assert!(dec.data.chunks_exact(4).all(|p| p[3] == 255));
}

#[test]
fn dxt5_alpha_six_value_mode() {
    // a0 = 10 <= a1 = 200: indices 6 and 7 are 0 and 255. Texel 0 -> index 6, texel 1 -> 7.
    let mut block = vec![10u8, 200, 0b1111_1110, 0, 0, 0, 0, 0];
    block.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
    let img = decode(f::DXT5, 4, 4, &block).unwrap();
    assert_eq!((img.data[3], img.data[7]), (0, 255));
    assert_eq!(&img.data[..3], &[255, 255, 255]);
}


#[test]
fn non_multiple_of_four_levels_encode_with_the_block_grid_size() {
    let mut img = Rgba8::new(7, 3);
    img.data.iter_mut().for_each(|v| *v = 200);
    for (format, size) in [(f::DXT1, 2 * 8), (f::DXT5, 2 * 16)] {
        assert_eq!(encode(format, &img).unwrap().len(), size);
        assert_eq!(size, level_size(format, 7, 3));
    }
}


#[test]
fn slightly_tinted_dark_fabric_stays_nearly_grey() {
    // Near-black fabric with a faint, noisy tint (spread up to 12): must not turn green/magenta.
    let mut img = Rgba8::new(64, 64);
    for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
        let n = (i.wrapping_mul(2_654_435_761) >> 11) as u32;
        let v = 10 + (n % 21) as i32;
        let off = |k: u32| ((n >> k) % 13) as i32 - 6;
        p.copy_from_slice(&[(v + off(5)).clamp(0, 255) as u8, (v + off(9)).clamp(0, 255) as u8, (v + off(13)).clamp(0, 255) as u8, 255]);
    }
    let mean_tint = |im: &Rgba8| {
        let sum: i64 = im.data.chunks_exact(4).map(|p| (p[..3].iter().max().unwrap() - p[..3].iter().min().unwrap()) as i64).sum();
        sum as f64 / (im.data.len() / 4) as f64
    };
    for format in [f::DXT1, f::DXT5] {
        let dec = decode(format, 64, 64, &encode(format, &img).unwrap()).unwrap();
        // 2.85 with the grey refit limited to near-grey blocks, 1.88 with the dark-block limit.
        assert!(mean_tint(&dec) <= 2.3, "format {format:X}: mean tint {}", mean_tint(&dec));
    }
}
