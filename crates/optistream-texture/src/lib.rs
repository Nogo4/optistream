//! Texture processing for RAGE (Legacy) textures: decode, resample, mips, DXT encode.

pub mod codec;
pub mod format;
pub mod ops;
pub mod plan;
pub mod quality;
pub mod resource;

pub use codec::Rgba8;
pub use plan::{optimize, Encoded, TextureMeta, TexturePolicy, TextureRole};

#[cfg(test)]
mod tests {
    use super::*;
    use optistream_rage::texture::format as f;

    fn policy(halve: bool) -> TexturePolicy {
        TexturePolicy { halve, halvings: 0, convert_formats: true, fix_mips: true, force_pot: true, alpha_mask: false }
    }

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Rgba8 {
        let mut img = Rgba8::new(w, h);
        for p in img.data.chunks_exact_mut(4) {
            p.copy_from_slice(&px);
        }
        img
    }

    /// Full chain of `img` encoded in `format`.
    fn encoded_chain(format: u32, img: Rgba8) -> (TextureMeta, Vec<u8>) {
        let (w, h) = (img.width, img.height);
        let levels = format::full_levels(w, h);
        let mut data = Vec::new();
        for level in ops::mip_chain(img, levels, ops::ColorSpace::Linear) {
            data.extend(codec::encode(format, &level).unwrap());
        }
        (TextureMeta { width: w, height: h, format, levels }, data)
    }

    /// DXT1 blocks in 3-colour mode where half of the texels are transparent (index 3).
    fn punch_through_dxt1(w: u32, h: u32, levels: u32) -> Vec<u8> {
        let block = [0x00, 0x00, 0xFF, 0xFF, 0xCC, 0xCC, 0xCC, 0xCC];
        let len = format::chain_size(f::DXT1, w, h, levels);
        block.iter().copied().cycle().take(len).collect()
    }

    /// 64x64 DXT1 whose level 0 is a black/white checker and whose stored mips are `mip_px`.
    fn checker_with_mips(mip_px: [u8; 4]) -> (TextureMeta, Vec<u8>) {
        let mut top = Rgba8::new(64, 64);
        for (i, p) in top.data.chunks_exact_mut(4).enumerate() {
            let v = if (i % 64 / 4 + i / 256) % 2 == 0 { 0 } else { 255 };
            p.copy_from_slice(&[v, v, v, 255]);
        }
        let levels = format::full_levels(64, 64);
        let mut data = codec::encode(f::DXT1, &top).unwrap();
        for l in 1..levels {
            let (w, h) = format::level_dims(64, 64, l);
            data.extend(codec::encode(f::DXT1, &solid(w, h, mip_px)).unwrap());
        }
        (TextureMeta { width: 64, height: 64, format: f::DXT1, levels }, data)
    }

    fn mean_rgb(img: &Rgba8) -> f64 {
        img.data.chunks_exact(4).map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) as f64 / 3.0).sum::<f64>()
            / (img.data.len() / 4) as f64
    }

    #[test]
    fn matching_stored_mips_are_sliced() {
        let mut img = Rgba8::new(64, 64);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((i % 64) as u8, (i / 64) as u8);
            p.copy_from_slice(&[x * 4, y * 4, x * 2 + y * 2, 255]);
        }
        let (meta, data) = encoded_chain(f::DXT1, img);
        let enc = optimize(&meta, &data, TextureRole::Diffuse, &policy(true)).unwrap().unwrap();
        let start = format::chain_size(f::DXT1, 64, 64, 1);
        assert_eq!(&enc.data[..], &data[start..start + enc.data.len()], "lossless slice expected");
    }

    #[test]
    fn brighter_stored_mips_are_rebuilt_from_the_top() {
        // Mips made in linear light: 188 where the checker averages to 128.
        let (meta, data) = checker_with_mips([188, 188, 188, 255]);
        let enc = optimize(&meta, &data, TextureRole::Diffuse, &policy(true)).unwrap().unwrap();
        assert_eq!((enc.width, enc.height), (32, 32));
        let top = codec::decode(f::DXT1, 32, 32, &enc.data).unwrap();
        assert!((mean_rgb(&top) - 128.0).abs() <= 4.0, "mean {}", mean_rgb(&top));
    }

    #[test]
    fn grey_dark_areas_in_stored_mips_are_rebuilt() {
        // White 128x128 with a black 32x32 square; the stored mips turn it blue-grey. The
        // whole-texture mean only moves by ~2, the dark band by ~30.
        let dark = |x: u32, y: u32, side: u32| x < side / 4 && y < side / 4;
        let levels = format::full_levels(128, 128);
        let mut data = Vec::new();
        for l in 0..levels {
            let (w, h) = format::level_dims(128, 128, l);
            let mut img = Rgba8::new(w, h);
            for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
                let (x, y) = (i as u32 % w, i as u32 / w);
                let px = match (dark(x, y, w), l) {
                    (false, _) => [255, 255, 255, 255],
                    (true, 0) => [0, 0, 0, 255],
                    (true, _) => [24, 28, 48, 255],
                };
                p.copy_from_slice(&px);
            }
            data.extend(codec::encode(f::DXT1, &img).unwrap());
        }
        let meta = TextureMeta { width: 128, height: 128, format: f::DXT1, levels };
        let enc = optimize(&meta, &data, TextureRole::Diffuse, &policy(true)).unwrap().unwrap();
        let top = codec::decode(f::DXT1, 64, 64, &enc.data).unwrap();
        let p = &top.data[(4 * 64 + 4) * 4..][..3];
        assert!(p.iter().all(|&v| v <= 4), "dark texel {p:?}");
    }

    #[test]
    fn opaque_uncompressed_spec_becomes_dxt1() {
        // Uncompressed spec with a spread of 3: converted, never flattened.
        let meta = TextureMeta { width: 64, height: 64, format: f::A8R8G8B8, levels: 1 };
        let data: Vec<u8> = (0..64 * 64).flat_map(|i| {
            let v = 100 + (i % 4 == 0) as u8 * 3;
            [v, v, v, 255]
        }).collect();
        let enc = optimize(&meta, &data, TextureRole::Specular, &policy(false)).unwrap().unwrap();
        assert_eq!((enc.width, enc.format), (64, f::DXT1));
    }

    #[test]
    fn uniform_diffuse_is_not_flattened() {
        let (meta, data) = encoded_chain(f::DXT1, solid(512, 512, [90, 30, 20, 255]));
        assert!(optimize(&meta, &data, TextureRole::Diffuse, &policy(false)).unwrap().is_none());
    }

    #[test]
    fn uniform_4x4_is_untouched() {
        let (meta, data) = encoded_chain(f::DXT1, solid(4, 4, [90, 90, 90, 255]));
        assert_eq!(meta.levels, 1);
        assert!(optimize(&meta, &data, TextureRole::Specular, &policy(false)).unwrap().is_none());
    }

    #[test]
    fn punch_through_dxt1_missing_mips_stays_dxt1() {
        let meta = TextureMeta { width: 64, height: 64, format: f::DXT1, levels: 1 };
        let data = punch_through_dxt1(64, 64, 1);
        assert!(plan::punch_through_blocks_mips(&meta, &data));
        // Not re-encoded to DXT5 (twice the size) just to add mips.
        assert!(optimize(&meta, &data, TextureRole::Diffuse, &policy(false)).unwrap().is_none());
        // Sliced down: stays DXT1, bit-exact, with the chain it has.
        let meta = TextureMeta { width: 128, height: 128, format: f::DXT1, levels: 2 };
        let data = punch_through_dxt1(128, 128, 2);
        let enc = optimize(&meta, &data, TextureRole::Diffuse, &policy(true)).unwrap().unwrap();
        assert_eq!((enc.width, enc.format, enc.levels), (64, f::DXT1, 1));
        assert_eq!(enc.data, data[format::level_size(f::DXT1, 128, 128)..]);
    }

    #[test]
    fn opaque_dxt1_is_not_reported_as_punch_through() {
        let (mut meta, data) = encoded_chain(f::DXT1, solid(64, 64, [90, 30, 20, 255]));
        meta.levels = 1;
        assert!(!plan::punch_through_blocks_mips(&meta, &data[..format::level_size(f::DXT1, 64, 64)]));
    }

    fn gradient(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        // A8R8G8B8 memory order: B, G, R, A.
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let a = if alpha && x < w / 2 { 0 } else { 255 };
                v.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 128, a]);
            }
        }
        v
    }

    #[test]
    fn uncompressed_npot_becomes_dxt1_with_mips() {
        let meta = TextureMeta { width: 300, height: 300, format: f::A8R8G8B8, levels: 1 };
        let data = gradient(300, 300, false);
        let enc = optimize(&meta, &data, TextureRole::Diffuse, &policy(false)).unwrap().unwrap();
        assert_eq!((enc.width, enc.height, enc.format), (256, 256, f::DXT1));
        assert_eq!(enc.levels, 7); // 256 .. 4
        assert_eq!(enc.data.len(), format::chain_size(f::DXT1, 256, 256, 7));
        assert_eq!(enc.stride, 128);
    }

    #[test]
    fn alpha_keeps_dxt5() {
        let meta = TextureMeta { width: 64, height: 64, format: f::A8R8G8B8, levels: 1 };
        let data = gradient(64, 64, true);
        let enc = optimize(&meta, &data, TextureRole::Diffuse, &policy(false)).unwrap().unwrap();
        assert_eq!(enc.format, f::DXT5);
    }

    #[test]
    fn dxt1_with_mips_is_sliced_losslessly() {
        // Mips must match their top level: random bytes are now (rightly) rebuilt.
        let mut img = Rgba8::new(1024, 1024);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i % 1024, i / 1024);
            p.copy_from_slice(&[(x / 4) as u8, (y / 4) as u8, ((x ^ y) % 256) as u8, 255]);
        }
        let (meta, data) = encoded_chain(f::DXT1, img);
        assert_eq!(meta.levels, 9); // 1024 .. 4
        let enc = optimize(&meta, &data, TextureRole::Diffuse, &policy(true)).unwrap().unwrap();
        assert_eq!((enc.width, enc.levels), (512, 8));
        let skip = format::level_size(f::DXT1, 1024, 1024);
        assert_eq!(enc.data, data[skip..]);
    }

    #[test]
    fn missing_mips_keep_the_top_level_bit_exact() {
        // A texture that only lacks mips must not be decoded and re-compressed.
        let mut img = Rgba8::new(512, 512);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            let n = ((i as u32).wrapping_mul(2_654_435_761) >> 29) as u8;
            p.copy_from_slice(&[40 + n, 52 + n / 2, 90 + n, 255]);
        }
        let top = codec::encode(f::DXT1, &img).unwrap();
        let meta = TextureMeta { width: 512, height: 512, format: f::DXT1, levels: 1 };
        let enc = optimize(&meta, &top, TextureRole::Diffuse, &policy(false)).unwrap().unwrap();
        assert_eq!((enc.width, enc.format, enc.levels), (512, f::DXT1, 8));
        assert_eq!(&enc.data[..top.len()], &top[..]);
        assert_eq!(enc.data.len(), format::chain_size(f::DXT1, 512, 512, 8));
    }

    #[test]
    fn compliant_texture_is_untouched() {
        let levels = 8;
        let len = format::chain_size(f::DXT1, 512, 512, levels);
        let data = vec![0u8; len];
        let meta = TextureMeta { width: 512, height: 512, format: f::DXT1, levels };
        assert!(optimize(&meta, &data, TextureRole::Diffuse, &policy(false)).unwrap().is_none());
    }

    #[test]
    fn zeroed_normal_mips_are_rebuilt() {
        // Real case (hair_normal_007): the stored mips of a normal map are black, i.e. normals
        // pointing (-1, -1). They used to be promoted to the top unchecked.
        let mut top = Rgba8::new(64, 64);
        for (i, p) in top.data.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[100 + (i % 64) as u8, 128, 255, 255]);
        }
        let levels = format::full_levels(64, 64);
        let mut data = codec::encode(f::DXT1, &ops::renormalize_normals(&top)).unwrap();
        for l in 1..levels {
            let (w, h) = format::level_dims(64, 64, l);
            data.extend(codec::encode(f::DXT1, &solid(w, h, [0, 0, 0, 255])).unwrap());
        }
        let meta = TextureMeta { width: 64, height: 64, format: f::DXT1, levels };
        let enc = optimize(&meta, &data, TextureRole::Normal, &policy(true)).unwrap().unwrap();
        let out = codec::decode(f::DXT1, 32, 32, &enc.data).unwrap();
        let red = out.data.chunks_exact(4).map(|p| p[0] as f64).sum::<f64>() / 1024.0;
        assert!((red - 132.0).abs() <= 6.0, "mean R {red}");
    }

    #[test]
    fn matte_spec_mips_are_checked() {
        // Specular alpha is gloss: a matte spec (alpha 20) whose stored mips are glossy
        // (alpha 200) skipped the check, every texel being taken for transparent.
        let mut img = Rgba8::new(64, 64);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[(i % 64) as u8 * 2, 60, 60, 20]);
        }
        let levels = format::full_levels(64, 64);
        let mut data = Vec::new();
        for (l, mut level) in ops::mip_chain(img, levels, ops::ColorSpace::Linear).into_iter().enumerate() {
            if l > 0 {
                level.data.chunks_exact_mut(4).for_each(|p| p[3] = 200);
            }
            data.extend(codec::encode(f::DXT5, &level).unwrap());
        }
        let meta = TextureMeta { width: 64, height: 64, format: f::DXT5, levels };
        let enc = optimize(&meta, &data, TextureRole::Specular, &policy(true)).unwrap().unwrap();
        let out = codec::decode(enc.format, 32, 32, &enc.data).unwrap();
        assert!(out.data.chunks_exact(4).all(|p| p[3] <= 24), "gloss {}", out.data[3]);
    }

    #[test]
    fn cutout_edges_keep_their_colour() {
        // Red cutout on a transparent white background, rebuilt at half size: straight
        // filtering gave pink edges.
        let mut img = Rgba8::new(64, 64);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            let px = if i % 64 < 33 { [200, 20, 20, 255] } else { [255, 255, 255, 0] };
            p.copy_from_slice(&px);
        }
        let (meta, data) = encoded_chain(f::DXT5, img);
        let masked = TexturePolicy { alpha_mask: true, ..policy(true) };
        let enc = optimize(&meta, &data[..format::level_size(f::DXT5, 64, 64)], TextureRole::Diffuse, &masked)
            .unwrap()
            .unwrap();
        let out = codec::decode(f::DXT5, 32, 32, &enc.data).unwrap();
        for x in [16, 20, 31] {
            let p = &out.data[(8 * 32 + x) * 4..][..4];
            assert!(p[1] < 40 && p[2] < 40 && p[0] > 170, "x {x}: {p:?}");
        }
    }

    #[test]
    fn uncompressed_normal_becomes_dxt5() {
        let meta = TextureMeta { width: 64, height: 64, format: f::A8R8G8B8, levels: 1 };
        let data = gradient(64, 64, false);
        let enc = optimize(&meta, &data, TextureRole::Normal, &policy(false)).unwrap().unwrap();
        assert_eq!((enc.width, enc.format, enc.levels), (64, f::DXT5, format::full_levels(64, 64)));
    }

    #[test]
    fn compressed_formats_are_kept() {
        // Opaque DXT5 and DXT1 normal maps are no longer converted: complete and POT, untouched.
        let (meta, data) = encoded_chain(f::DXT5, solid(64, 64, [90, 30, 20, 255]));
        assert!(optimize(&meta, &data, TextureRole::Diffuse, &policy(false)).unwrap().is_none());
        let (meta, data) = encoded_chain(f::DXT1, solid(64, 64, [128, 128, 255, 255]));
        assert!(optimize(&meta, &data, TextureRole::Normal, &policy(false)).unwrap().is_none());
    }

    #[test]
    fn halving_keeps_the_format() {
        let (meta, data) = encoded_chain(f::DXT5, solid(64, 64, [90, 30, 20, 255]));
        let enc = optimize(&meta, &data, TextureRole::Diffuse, &policy(true)).unwrap().unwrap();
        assert_eq!((enc.width, enc.format), (32, f::DXT5));
    }
}
