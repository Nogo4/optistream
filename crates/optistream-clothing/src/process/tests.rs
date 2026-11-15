use std::collections::HashSet;

use optistream_rage::drawable::{self, DrawableInfo, GeometryInfo, VertexDecl};

use crate::process::analyze::heaviest_high;
use crate::process::meshes::{is_decimable, lod_ratio};

fn geometry(offset: usize, triangles: u32) -> GeometryInfo {
    GeometryInfo {
        offset,
        vb_offset: offset + 1,
        ib_offset: offset + 2,
        decl: VertexDecl { flags: 1, stride: 12, count: 1, types: drawable::component::FLOAT3 as u64 },
        shader_index: Some(0),
        vertex_stride: 12,
        vertex_count: triangles * 3,
        vertex_ptr: 0,
        vertex_ptr2: 0,
        index_ptr: 0,
        index_count: triangles * 3,
    }
}

fn drawable_with_high(offset: usize, triangles: u32) -> DrawableInfo {
    DrawableInfo {
        offset,
        hash: 0,
        has_skeleton: true,
        shaders: Vec::new(),
        texture_dict: None,
        lods: [vec![geometry(offset + 16, triangles)], Vec::new(), Vec::new(), Vec::new()],
    }
}

#[test]
fn high_budget_is_checked_per_drawable() {
    // Two drawables at 0.6 x budget each: the optimiser leaves both alone, so must the analysis.
    let budget = 40_000u64;
    let ds = [drawable_with_high(0, 24_000), drawable_with_high(1000, 24_000)];
    let (_, worst) = heaviest_high(&ds).unwrap();
    assert!(worst <= budget);
    let ds = [drawable_with_high(0, 24_000), drawable_with_high(1000, 45_000)];
    assert_eq!(heaviest_high(&ds), Some((1, 45_000)));
}

#[test]
fn ratio_compensates_for_fixed_geometries() {
    // 50k tris, 35k of them not decimable, target 40k: the 15k left must give 10k.
    let r = lod_ratio(40_000, 15_000, 35_000);
    assert!((r - 5_000.0 / 15_000.0).abs() < 1e-9);
    assert_eq!(lod_ratio(40_000, 10_000, 45_000), 0.0);
    assert_eq!(lod_ratio(40_000, 0, 50_000), 1.0);
    assert_eq!(lod_ratio(60_000, 50_000, 0), 1.0);
}

#[test]
fn small_and_shared_geometries_are_not_decimable() {
    let shared: HashSet<usize> = [101].into_iter().collect();
    assert!(is_decimable(&geometry(0, 64), &shared));
    assert!(!is_decimable(&geometry(0, 63), &shared));
    assert!(!is_decimable(&geometry(100, 1000), &shared));
}

#[test]
fn cloth_and_non_float3_geometries_are_not_decimable() {
    let none = HashSet::new();
    let mut cloth = geometry(0, 1000);
    cloth.decl.flags |= 0x8000_0000;
    assert!(!is_decimable(&cloth, &none));
    let mut half = geometry(0, 1000);
    half.decl.types = drawable::component::HALF4 as u64;
    assert!(!is_decimable(&half, &none));
}

#[test]
fn lod_ratio_reaches_the_target_and_stays_in_unit_range() {
    for target in [0u64, 1, 5_000, 40_000, 100_000] {
        for decimable in [1u64, 64, 10_000, 60_000] {
            for fixed in [0u64, 1_000, 30_000, 80_000] {
                let r = lod_ratio(target, decimable, fixed);
                assert!((0.0..=1.0).contains(&r), "{target} {decimable} {fixed}: {r}");
                let total = decimable as f64 * r + fixed as f64;
                if fixed <= target && target <= decimable + fixed {
                    // Reachable: the LOD lands exactly on the target.
                    assert!((total - target as f64).abs() < 1e-6, "{target} {decimable} {fixed}");
                } else if fixed > target {
                    // The fixed geometries alone exceed it: decimate everything to the floor.
                    assert_eq!(r, 0.0);
                } else {
                    assert_eq!(r, 1.0);
                }
            }
        }
    }
}

#[test]
fn shared_vertex_buffers_are_the_ones_used_twice() {
    let mut a = drawable_with_high(0, 100);
    let mut b = drawable_with_high(1000, 100);
    // The med LOD of `a` reuses its high geometry; `b` reuses a vertex buffer of `a`.
    let reused = a.lods[0][0].clone();
    a.lods[1].push(reused);
    let mut foreign = geometry(5000, 10);
    foreign.vb_offset = a.lods[0][0].vb_offset;
    b.lods[2].push(foreign);
    b.lods[3].push(geometry(6000, 10));
    let vb = a.lods[0][0].vb_offset;
    let shared = crate::process::meshes::shared_vertex_buffers(&[a, b]);
    assert_eq!(shared, [vb].into_iter().collect());
    assert!(crate::process::meshes::shared_vertex_buffers(&[drawable_with_high(0, 100)]).is_empty());
}

#[test]
fn group_separates_thousands() {
    assert_eq!(crate::process::group(0), "0");
    assert_eq!(crate::process::group(999), "999");
    assert_eq!(crate::process::group(1_000), "1 000");
    assert_eq!(crate::process::group(45_000), "45 000");
    assert_eq!(crate::process::group(1_234_567), "1 234 567");
}
