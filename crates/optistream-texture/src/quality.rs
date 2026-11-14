//! Visual loss of a texture reduction, used to decide which textures can be halved first.
//!
//! The loss of `k` halvings is measured against the full-size original: the image is
//! halved `k` times (same filter as the mips), scaled back up to the original size and
//! compared with it. The measure is the root mean square difference of the channels the
//! role cares about, in percent of the full range (100 % = black against white).
//!
//! Calibrated on prompt_mrpd interiors (2026-11-14): SSIM ranked a flat plaster wall
//! (identical to the eye after ÷2) as lossy as a wood floor whose grain softens, and a
//! texture of printed names barely worse. The RMS difference follows the eye: plaster
//! 0.9 %, wood floor 1.5 %, slatted wood wall 3.0 %, printed names 14 %.

use crate::codec::Rgba8;
use crate::ops;
use crate::plan::TextureRole;

type Channel = fn(&[u8]) -> f64;

/// Channels compared for a role. Diffuse-like textures: luma, plus the alpha when it varies
/// (cutouts). Normal maps: X and Y (the shader rebuilds Z). Speculars: luma and gloss (alpha).
fn channels(role: TextureRole, alpha_used: bool) -> Vec<Channel> {
    let luma: Channel = |p| 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
    let alpha: Channel = |p| p[3] as f64;
    match role {
        TextureRole::Normal => vec![|p| p[0] as f64, |p| p[1] as f64],
        TextureRole::Specular => vec![luma, alpha],
        TextureRole::Diffuse | TextureRole::Other if alpha_used => vec![luma, alpha],
        TextureRole::Diffuse | TextureRole::Other => vec![luma],
    }
}

/// Root mean square difference of one channel, in percent of the full range.
fn rms_percent(a: &Rgba8, b: &Rgba8, channel: Channel) -> f64 {
    let n = (a.width as usize * a.height as usize).max(1);
    let sum: f64 = a
        .data
        .chunks_exact(4)
        .zip(b.data.chunks_exact(4))
        .map(|(p, q)| {
            let d = channel(p) - channel(q);
            d * d
        })
        .sum();
    (sum / n as f64).sqrt() / 255.0 * 100.0
}

/// Loss in percent (0 = nothing lost) of halving `top` `times` times, against `top` itself.
pub fn reduction_loss(top: &Rgba8, times: u32, role: TextureRole, alpha_used: bool) -> f64 {
    if times == 0 || top.width < 2 || top.height < 2 {
        return 0.0;
    }
    let cs = role.color_space();
    let small = ops::half_n(top, times, cs);
    let back = ops::resize(&small, top.width, top.height, cs);
    channels(role, alpha_used).into_iter().map(|c| rms_percent(top, &back, c)).fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Rgba8 {
        let mut img = Rgba8::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                img.data[i..i + 4].copy_from_slice(&f(x, y));
            }
        }
        img
    }

    #[test]
    fn flat_texture_loses_nothing() {
        let img = image(64, 64, |_, _| [120, 80, 40, 255]);
        assert!(reduction_loss(&img, 2, TextureRole::Diffuse, false) < 0.01);
    }

    #[test]
    fn fine_detail_loses_more_than_a_gradient() {
        let checker = image(64, 64, |x, y| if (x + y) % 2 == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        let gradient = image(64, 64, |x, _| [(x * 4) as u8, (x * 4) as u8, (x * 4) as u8, 255]);
        let lc = reduction_loss(&checker, 1, TextureRole::Diffuse, false);
        let lg = reduction_loss(&gradient, 1, TextureRole::Diffuse, false);
        assert!(lc > 30.0, "checker {lc}");
        assert!(lg < 1.0, "gradient {lg}");
    }

    #[test]
    fn more_halvings_never_lose_less() {
        let noise = image(64, 64, |x, y| {
            let v = ((x * 7 + y * 13) % 17 * 15) as u8;
            [v, v, v, 255]
        });
        let l1 = reduction_loss(&noise, 1, TextureRole::Diffuse, false);
        let l2 = reduction_loss(&noise, 2, TextureRole::Diffuse, false);
        assert!(l2 >= l1, "{l1} {l2}");
    }
}
