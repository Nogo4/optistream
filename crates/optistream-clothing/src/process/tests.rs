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
