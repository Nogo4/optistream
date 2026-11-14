use crate::ops::coverage::coverage;
use super::*;

fn solid(w: u32, h: u32, px: [u8; 4]) -> Rgba8 {
    let mut img = Rgba8::new(w, h);
    for p in img.data.chunks_exact_mut(4) {
        p.copy_from_slice(&px);
    }
    img
}

#[test]
fn normal_z_is_rebuilt_from_rg() {
    // x = 0.6, y = 0 with a stored B of 0 (z = -1): renormalising with B shrank x to 0.51.
    let r = ((0.6f32 * 0.5 + 0.5) * 255.0).round() as i32;
    let img = solid(4, 4, [r as u8, 128, 0, 255]);
    let out = half(&img, ColorSpace::Normal);
    assert!((out.data[0] as i32 - r).abs() <= 1, "R = {}", out.data[0]);
    // Rebuilt z = 0.8, stored as 0.9 * 255.
    assert!((out.data[2] as i32 - 230).abs() <= 1, "B = {}", out.data[2]);
}

#[test]
fn well_formed_normal_is_unchanged() {
    // (0.6, 0, 0.8) stored with its correct Z.
    let px = [204, 128, 230, 255];
    let out = half(&solid(8, 8, px), ColorSpace::Normal);
    for (a, b) in out.data[..4].iter().zip(px) {
        assert!((*a as i32 - b as i32).abs() <= 1, "{:?}", &out.data[..4]);
    }
}

#[test]
fn fine_dark_detail_keeps_its_brightness() {
    // One-texel black/white checker: linear-light averaging gave 188, i.e. lighter clothes.
    let mut img = Rgba8::new(8, 8);
    for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
        let v = if (i % 8 + i / 8) % 2 == 0 { 0 } else { 255 };
        p.copy_from_slice(&[v, v, v, 255]);
    }
    for out in [half(&img, ColorSpace::Linear), resize(&img, 4, 4, ColorSpace::Linear)] {
        for p in out.data.chunks_exact(4) {
            // CatmullRom lands a few levels off at the image border.
            assert!((p[0] as i32 - 128).abs() <= 4, "{:?}", &p[..4]);
        }
    }
}

#[test]
fn float_mip_chain_matches_direct_average() {
    let (w, h) = (256u32, 256u32);
    let mut img = Rgba8::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            img.data[i..i + 4].copy_from_slice(&[x as u8, y as u8, ((x + y) / 2) as u8, 255]);
        }
    }
    let chain = mip_chain(img.clone(), 7, ColorSpace::Linear); // 256 .. 4
    let last = chain.last().unwrap();
    assert_eq!((last.width, last.height), (4, 4));
    let block = w / 4;
    for by in 0..4 {
        for bx in 0..4 {
            for c in 0..3 {
                let mut acc = 0f64;
                for y in by * block..(by + 1) * block {
                    for x in bx * block..(bx + 1) * block {
                        acc += img.data[((y * w + x) * 4) as usize + c] as f64;
                    }
                }
                let direct = acc / (block * block) as f64;
                let got = last.data[((by * 4 + bx) * 4) as usize + c] as f64;
                assert!((got - direct).abs() <= 1.0, "4x4 ({bx},{by}) c{c}: {got} vs {direct}");
            }
        }
    }
}

/// One opaque column out of four (hair strands): 25 % of the texels pass the alpha test.
fn strands(side: u32) -> Rgba8 {
    let mut img = Rgba8::new(side, side);
    for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
        let a = if i as u32 % side % 4 == 0 { 255 } else { 0 };
        p.copy_from_slice(&[60, 40, 20, a]);
    }
    img
}

fn passing(img: &Rgba8) -> f32 {
    coverage(img.data.chunks_exact(4).map(|p| p[3] as f32 / 255.0), 1.0)
}

#[test]
fn thin_strands_keep_their_coverage() {
    // Box filtering alone: alpha 0.25 at level 2, nothing passes the test any more.
    let plain = mip_chain(strands(64), 4, ColorSpace::Linear);
    assert_eq!(passing(&plain[2]), 0.0);
    let kept = mip_chain(strands(64), 4, ColorSpace::AlphaMask);
    for level in &kept[1..] {
        assert!(passing(level) >= 0.25, "{}x{}: {}", level.width, level.height, passing(level));
    }
    // Colour is untouched by the alpha scale.
    assert_eq!(&kept[2].data[..3], &[60, 40, 20]);
}

#[test]
fn coverage_is_never_lowered() {
    // A solid cutout already covers everything: alpha stays as filtered.
    let chain = mip_chain(solid(16, 16, [10, 20, 30, 200]), 3, ColorSpace::AlphaMask);
    assert!(chain.iter().all(|l| l.data.chunks_exact(4).all(|p| p[3] == 200)));
}
