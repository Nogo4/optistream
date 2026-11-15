use optistream_rage::pages;
use optistream_rage::texture::{self as rtex, field, format as tf, TextureEntry, TEXTURE_STRUCT_SIZE};
use optistream_rage::{gfx, PageFlags, Resource, GRAPHICS_BASE, SYSTEM_BASE};

use super::*;
use crate::format::{chain_size, full_levels, stride};
use crate::{codec, ops, resource as texres, Rgba8, TextureRole};

#[test]
fn candidate_sizes_follow_halvings() {
    let c = Candidate { block: 0, format: tf::DXT1, width: 2048, height: 2048, levels: 12, loss: vec![0.0], key: vec![0] };
    let full = c.size(0, true);
    let half = c.size(1, true);
    // DXT1 2048² full chain ≈ 2.67 Mio; one halving keeps about a quarter.
    assert!(full > 2_700_000 && full < 2_900_000, "{full}");
    assert!(half * 4 >= full - 64 && half * 4 <= full + 64, "{half}");
}

#[test]
fn candidate_size_without_mip_fix_keeps_the_stored_levels() {
    let c = Candidate { block: 0, format: tf::DXT1, width: 256, height: 256, levels: 1, loss: vec![0.0], key: vec![0] };
    assert_eq!(c.size(0, false), chain_size(tf::DXT1, 256, 256, 1) as u64);
    assert_eq!(c.size(1, false), chain_size(tf::DXT1, 128, 128, 1) as u64);
    assert_eq!(c.size(1, true), chain_size(tf::DXT1, 128, 128, full_levels(128, 128)) as u64);
}

#[test]
fn past_steps_come_after_every_tier_within() {
    // Tiers within the limits (0..=2) all sort before any step past them.
    for tier in 0..=2u8 {
        assert!(tier < PAST);
    }
    assert!(PAST + 2 > PAST);
}

// --- Synthetic resources -------------------------------------------------------------

/// Deterministic high-frequency image: every halving loses detail.
fn noise(side: u32, seed: u32) -> Rgba8 {
    let mut img = Rgba8::new(side, side);
    for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
        let mut v = (i as u32).wrapping_mul(2_654_435_761).wrapping_add(seed.wrapping_mul(40_503));
        v ^= v >> 13;
        p.copy_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]);
    }
    img
}

/// DXT1 full mip chain of `img`.
fn dxt1_chain(img: Rgba8) -> Vec<u8> {
    let levels = full_levels(img.width, img.height);
    ops::mip_chain(img, levels, ops::ColorSpace::Linear)
        .iter()
        .flat_map(|l| codec::encode(tf::DXT1, l).unwrap())
        .collect()
}

struct Synth {
    res: Resource,
    entries: Vec<TextureEntry>,
}

/// Builds a resource holding one `grcTexture` per entry of `textures` (name, side, index
/// of the pixel block it uses), the blocks laid out with the real packer.
fn synth(blocks: &[Vec<u8>], textures: &[(&str, u32, usize)]) -> Synth {
    let sizes: Vec<u64> = blocks.iter().map(|b| b.len() as u64).collect();
    let layout = pages::pack(&sizes, 128, false).unwrap();
    let mut graphics = vec![0u8; layout.size() as usize];
    for (b, &off) in blocks.iter().zip(&layout.offsets) {
        graphics[off as usize..off as usize + b.len()].copy_from_slice(b);
    }

    // System: 0x10 header (no ResourcePagesInfo), the structures, then the names.
    let structs = 0x10;
    let names = structs + textures.len() * TEXTURE_STRUCT_SIZE;
    let sys_len = names + textures.iter().map(|(n, ..)| n.len() + 1).sum::<usize>();
    let sys_layout = pages::pack(&[sys_len as u64], 128, true).unwrap();
    let mut res = Resource {
        version: 13,
        system_flags: sys_layout.flags,
        graphics_flags: PageFlags(layout.flags.0),
        system: vec![0u8; sys_layout.size() as usize],
        graphics,
    };
    let mut name_off = names;
    for (i, &(name, side, block)) in textures.iter().enumerate() {
        let t = structs + i * TEXTURE_STRUCT_SIZE;
        res.system[name_off..name_off + name.len()].copy_from_slice(name.as_bytes());
        res.set_u64(t + field::NAME_PTR, SYSTEM_BASE + name_off as u64);
        name_off += name.len() + 1;
        let levels = full_levels(side, side) as u8;
        rtex::write_texture_header(&mut res, t, side as u16, side as u16, stride(tf::DXT1, side, side), tf::DXT1, levels);
        res.set_u64(t + field::DATA_PTR, GRAPHICS_BASE + layout.offsets[block]);
    }
    let entries = (0..textures.len()).map(|i| rtex::read_texture(&res, structs + i * TEXTURE_STRUCT_SIZE).unwrap()).collect();
    Synth { res, entries }
}

/// `n` textures of `side`², each with its own block.
fn distinct(n: usize, side: u32) -> Synth {
    let blocks: Vec<Vec<u8>> = (0..n).map(|i| dxt1_chain(noise(side, i as u32))).collect();
    let names: Vec<String> = (0..n).map(|i| format!("tex_{i}")).collect();
    let textures: Vec<(&str, u32, usize)> = names.iter().enumerate().map(|(i, n)| (n.as_str(), side, i)).collect();
    synth(&blocks, &textures)
}

fn rule() -> Rule {
    Rule { access: Access::Normal, tier: 0, min_size: 4, max_steps: 8, hard_floor: 0, hard_floor_hold: None, ignore_loss: true }
}

fn limits(budget: u64) -> Limits {
    Limits {
        enabled: true,
        budget,
        max_loss: 100.0,
        force: false,
        force_min_size: 4,
        force_max_steps: 8,
        policy: TexturePolicy { halve: false, halvings: 0, convert_formats: true, fix_mips: true, force_pot: true, alpha_mask: false },
    }
}

fn budget_texs<'a>(s: &'a Synth, rules: &[Rule]) -> Vec<BudgetTex<'a>> {
    s.entries
        .iter()
        .zip(rules)
        .map(|(e, &rule)| {
            let loc = texres::locate(&s.res, e);
            BudgetTex {
                entry: e,
                data_off: loc.map(|l| l.0),
                data_len: loc.map(|l| l.1).unwrap_or(0),
                role: Some(TextureRole::Diffuse),
                rule,
            }
        })
        .collect()
}

fn plan(s: &Synth, rules: &[Rule], limits: &Limits) -> FilePlan {
    plan_file(&s.res, &[], &budget_texs(s, rules), limits).unwrap()
}

/// Halvings planned for each texture (by texture index).
fn steps_by_texture(fp: &FilePlan, n: usize) -> Vec<u32> {
    let mut out = vec![0; n];
    for b in &fp.blocks {
        for &u in &b.users {
            out[u] = b.steps;
        }
    }
    out
}

const HOLD_LOGO: Hold = Hold { label: "logos", rank: 1 };

#[test]
fn compliant_file_under_budget_is_left_alone() {
    let s = distinct(3, 128);
    let before = s.res.graphics_flags.size();
    let fp = plan(&s, &[rule(); 3], &limits(u64::MAX));
    assert_eq!(fp.blocks.len(), 3);
    assert!(fp.blocks.iter().all(|b| b.enc.is_none() && b.steps == 0 && !b.forced));
    assert!(fp.warnings.is_empty(), "{:?}", fp.warnings);
    assert_eq!(fp.physical_after, before);
}

#[test]
fn disabled_budget_never_halves() {
    let s = distinct(2, 128);
    let mut l = limits(0);
    l.enabled = false;
    let fp = plan(&s, &[rule(); 2], &l);
    assert_eq!(steps_by_texture(&fp, 2), [0, 0]);
    assert!(fp.warnings.is_empty());
}

#[test]
fn over_budget_file_is_reduced_until_it_fits() {
    let s = distinct(4, 256);
    let before = s.res.graphics_flags.size();
    let fp = plan(&s, &[rule(); 4], &limits(before / 2));
    assert!(fp.physical_after <= before / 2, "{} > {}", fp.physical_after, before / 2);
    assert!(fp.warnings.is_empty(), "{:?}", fp.warnings);
    for b in fp.blocks.iter().filter(|b| b.steps > 0) {
        let enc = b.enc.as_ref().expect("a halved block is re-encoded");
        assert_eq!(enc.width, 256 >> b.steps);
        assert_eq!(enc.levels, full_levels(enc.width, enc.height));
        assert!(b.loss > 0.0 && !b.forced);
    }
    // Only what is needed: some textures keep their size.
    assert!(steps_by_texture(&fp, 4).contains(&0));
}

#[test]
fn applied_plan_matches_the_estimate_and_patches_every_texture() {
    let mut s = distinct(4, 256);
    let before = s.res.graphics_flags.size();
    let data_len: Vec<usize> = budget_texs(&s, &[rule(); 4]).iter().map(|t| t.data_len).collect();
    let fp = plan(&s, &[rule(); 4], &limits(before / 2));
    let expected: Vec<(usize, Option<Encoded>)> =
        fp.blocks.iter().map(|b| (b.users[0], b.enc.clone())).collect();
    let physical = fp.physical_after;
    let originals: Vec<Vec<u8>> = (0..4)
        .map(|i| {
            let off = texres::locate(&s.res, &s.entries[i]).unwrap().0;
            s.res.graphics[off..off + data_len[i]].to_vec()
        })
        .collect();

    apply(&mut s.res, &s.entries, &data_len, fp.blocks).unwrap();
    assert_eq!(s.res.graphics_flags.size(), physical);
    assert_eq!(s.res.graphics.len() as u64, physical);
    assert_eq!(s.res.graphics_flags.version_nibble(), 0);
    for (user, enc) in expected {
        let e = rtex::read_texture(&s.res, s.entries[user].struct_offset).unwrap();
        let (off, _) = texres::locate(&s.res, &e).expect("data pointer resolves into the graphics segment");
        assert_eq!(off % 16, 0);
        match enc {
            Some(enc) => {
                assert_eq!((e.width as u32, e.height as u32, e.format, e.levels as u32), (enc.width, enc.height, enc.format, enc.levels));
                assert_eq!(e.stride, enc.stride);
                assert_eq!(&s.res.graphics[off..off + enc.data.len()], &enc.data[..]);
            }
            None => {
                assert_eq!(e.width, 256);
                assert_eq!(&s.res.graphics[off..off + originals[user].len()], &originals[user][..]);
            }
        }
    }
}

#[test]
fn lower_tiers_are_spent_first() {
    let s = distinct(2, 256);
    let full = chain_size(tf::DXT1, 256, 256, full_levels(256, 256)) as u64;
    let half = chain_size(tf::DXT1, 128, 128, full_levels(128, 128)) as u64;
    let budget = gfx::packed_size(&s.res, &[full, half]).unwrap();
    assert!(budget < s.res.graphics_flags.size(), "one halving must be needed");
    // Texture 1 is in the lowest tier: it is halved, texture 0 is kept.
    let fp = plan(&s, &[Rule { tier: 2, ..rule() }, Rule { tier: 0, ..rule() }], &limits(budget));
    assert_eq!(steps_by_texture(&fp, 2), [0, 1]);
    assert!(fp.physical_after <= budget);
}

#[test]
fn never_access_keeps_the_texture_and_explains_why() {
    let s = distinct(2, 256);
    let fp = plan(&s, &[Rule { access: Access::Never(HOLD_LOGO), ..rule() }, rule()], &limits(0));
    let steps = steps_by_texture(&fp, 2);
    assert_eq!(steps[0], 0);
    assert!(steps[1] > 0);
    assert_eq!(fp.warnings.len(), 1);
    let w = &fp.warnings[0];
    assert!(w.starts_with("reste à ") && w.contains("(budget 0.0 Mio)"), "{w}");
    assert!(w.contains("logos : "), "{w}");
}

#[test]
fn force_only_textures_wait_for_forcing() {
    let s = distinct(1, 256);
    let rules = [Rule { access: Access::ForceOnly(HOLD_LOGO), ..rule() }];
    let fp = plan(&s, &rules, &limits(0));
    assert_eq!(steps_by_texture(&fp, 1), [0]);
    assert!(fp.warnings[0].contains("logos : "), "{:?}", fp.warnings);

    let forced = plan(&s, &rules, &Limits { force: true, ..limits(0) });
    let b = &forced.blocks[0];
    assert!(b.steps > 0);
    // Every step of a force-only texture is past the limits.
    assert!(b.forced);
}

#[test]
fn min_size_and_max_steps_bound_the_reduction() {
    let s = distinct(2, 256);
    let rules = [Rule { min_size: 128, ..rule() }, Rule { max_steps: 2, ..rule() }];
    let fp = plan(&s, &rules, &limits(0));
    assert_eq!(steps_by_texture(&fp, 2), [1, 2]);
    let w = &fp.warnings[0];
    assert!(w.contains("à la taille minimale") && w.contains("divisions max atteintes"), "{w}");
}

#[test]
fn forcing_goes_past_the_limits_down_to_the_forced_floor() {
    let s = distinct(1, 256);
    let rules = [Rule { min_size: 128, max_steps: 1, ..rule() }];
    let l = Limits { force: true, force_min_size: 32, force_max_steps: 3, ..limits(0) };
    let b = &plan(&s, &rules, &l).blocks[0];
    assert_eq!(b.steps, 3);
    assert_eq!(b.enc.as_ref().unwrap().width, 32);
    assert!(b.forced);
}

#[test]
fn hard_floor_holds_even_when_forcing() {
    let s = distinct(1, 256);
    let hold = Hold { label: "plancher", rank: 2 };
    let rules = [Rule { hard_floor: 128, hard_floor_hold: Some(hold), ..rule() }];
    let fp = plan(&s, &rules, &Limits { force: true, ..limits(0) });
    assert_eq!(fp.blocks[0].steps, 1);
    assert!(fp.warnings[0].contains("plancher : "), "{:?}", fp.warnings);
}

#[test]
fn max_loss_stops_lossy_steps() {
    let s = distinct(1, 256);
    let rules = [Rule { ignore_loss: false, ..rule() }];
    let fp = plan(&s, &rules, &Limits { max_loss: 0.0, ..limits(0) });
    assert_eq!(fp.blocks[0].steps, 0);
    assert!(fp.warnings[0].contains("perte maximale atteinte"), "{:?}", fp.warnings);
}

#[test]
fn textures_without_a_role_are_protected() {
    let s = distinct(1, 256);
    let mut texs = budget_texs(&s, &[rule()]);
    texs[0].role = None;
    let fp = plan_file(&s.res, &[], &texs, &limits(0)).unwrap();
    assert_eq!(fp.blocks[0].steps, 0);
    assert!(fp.blocks[0].enc.is_none());
    assert!(fp.warnings[0].contains(HOLD_PROTECTED.label), "{:?}", fp.warnings);
}

#[test]
fn shared_blocks_are_planned_once_and_patched_for_every_user() {
    let blocks = vec![dxt1_chain(noise(256, 7))];
    let mut s = synth(&blocks, &[("a", 256, 0), ("b", 256, 0)]);
    let data_len: Vec<usize> = budget_texs(&s, &[rule(); 2]).iter().map(|t| t.data_len).collect();
    let fp = plan(&s, &[rule(); 2], &limits(0));
    assert_eq!(fp.blocks.len(), 1);
    assert_eq!(fp.blocks[0].users, [0, 1]);
    let width = fp.blocks[0].enc.as_ref().unwrap().width;
    apply(&mut s.res, &s.entries, &data_len, fp.blocks).unwrap();
    let a = rtex::read_texture(&s.res, s.entries[0].struct_offset).unwrap();
    let b = rtex::read_texture(&s.res, s.entries[1].struct_offset).unwrap();
    assert_eq!(a.data_ptr, b.data_ptr);
    assert_eq!((a.width as u32, b.width as u32), (width, width));
}

#[test]
fn unknown_graphics_pointer_leaves_the_file_untouched() {
    let mut s = distinct(2, 256);
    // A pointer to the graphics segment from a field no texture owns.
    s.res.set_u64(0x00, GRAPHICS_BASE);
    let before = s.res.graphics_flags.size();
    let fp = plan(&s, &[rule(); 2], &limits(0));
    assert!(fp.blocks.is_empty());
    assert_eq!(fp.physical_after, before);
    assert!(fp.warnings[0].contains("référence(s) graphique(s) inconnue(s)"), "{:?}", fp.warnings);
}

#[test]
fn raw_ranges_are_not_scanned_for_pointers() {
    let mut s = distinct(1, 256);
    s.res.set_u64(0x00, GRAPHICS_BASE);
    let fp = plan_file(&s.res, &[(0, 8)], &budget_texs(&s, &[rule()]), &limits(0)).unwrap();
    assert_eq!(fp.blocks.len(), 1);
    assert!(fp.blocks[0].steps > 0);
}

#[test]
fn completing_mips_never_makes_a_file_heavier() {
    // Textures stored without mips: completing them costs about a third more.
    let blocks: Vec<Vec<u8>> = (0..2).map(|i| codec::encode(tf::DXT1, &noise(256, i)).unwrap()).collect();
    let mut s = synth(&blocks, &[("a", 256, 0), ("b", 256, 1)]);
    for e in &mut s.entries {
        rtex::write_texture_header(&mut s.res, e.struct_offset, 256, 256, e.stride, tf::DXT1, 1);
        e.levels = 1;
    }
    let before = s.res.graphics_flags.size();
    let fp = plan(&s, &[rule(); 2], &limits(u64::MAX));
    assert!(fp.physical_after <= before, "{} > {before}", fp.physical_after);
}
