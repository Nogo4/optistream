//! Geometry reduction of a model above the virtual memory budget.
//!
//! In `.yft` files the vertex / index data lives in the system segment (virtual memory).
//! Steps from the least to the most visible are tried in turn (far LODs halved first, the
//! High LOD last and only down to the preset's floor); each step starts again from the
//! original geometry and the declared size is measured with the real system packer
//! ([`optistream_rage::sys`]). The decimator keeps original vertices (skinning intact),
//! locks borders, seams and bone boundaries, and stops at the error tolerance.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Result};
use optistream_core::mib;
use optistream_mesh::{decimate, Decimated, GeometryData};
use optistream_rage::drawable::{self, component, geom_field, ib_field, semantic, vb_field, DrawableInfo, GeometryInfo};
use optistream_rage::sys::{self, RawBuffer};
use optistream_rage::{Resource, Segment};
use rayon::prelude::*;

use crate::config::Config;

/// Smallest share of the High LOD a lower LOD may keep (med, low, vlow).
const LOD_FLOOR: [f64; 4] = [1.0, 0.15, 0.05, 0.02];
/// Error tolerance of the far LODs (fraction of the geometry extent).
const LOD_ERROR: [f32; 4] = [0.0, 0.02, 0.03, 0.05];

/// Reduction steps: `(lod, share of the original triangles)`, least visible first.
fn steps(cfg: &Config) -> Vec<(usize, f64)> {
    let mut s = vec![(3, 0.5), (2, 0.5), (1, 0.5), (3, 0.25), (2, 0.25), (1, 0.35)];
    let mut r = 0.9;
    while r + 1e-9 >= cfg.geo_high_floor {
        s.push((0, r));
        r -= 0.1;
    }
    s
}

pub struct GeoPlan {
    /// Geometries rewritten (structure offset) and their new data.
    pub writes: Vec<(GeometryInfo, Decimated)>,
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
    pub virtual_after: u64,
}

fn vertex_ptr(g: &GeometryInfo) -> u64 {
    if g.vertex_ptr != 0 {
        g.vertex_ptr
    } else {
        g.vertex_ptr2
    }
}

fn sys_off(res: &Resource, ptr: u64) -> Option<usize> {
    match res.resolve(ptr) {
        Some((Segment::System, o)) if ptr != 0 => Some(o),
        _ => None,
    }
}

/// Every geometry once (by structure offset), with its drawable and LOD.
fn geometries(drawables: &[DrawableInfo]) -> Vec<(usize, usize, GeometryInfo)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (di, d) in drawables.iter().enumerate() {
        for (lod, gs) in d.lods.iter().enumerate() {
            for g in gs {
                if seen.insert(g.offset) {
                    out.push((di, lod, g.clone()));
                }
            }
        }
    }
    out
}

/// Raw buffers of every geometry: `(offset → length)`; a buffer shared by two geometries
/// (or referenced twice) is listed once.
fn raw_buffers(res: &Resource, geoms: &[(usize, usize, GeometryInfo)]) -> Result<BTreeMap<usize, usize>> {
    let mut out = BTreeMap::new();
    for (_, _, g) in geoms {
        let vlen = g.vertex_count as usize * g.vertex_stride as usize;
        let mut ptrs = vec![(g.vertex_ptr, vlen), (g.vertex_ptr2, vlen), (g.index_ptr, g.index_count as usize * 2)];
        ptrs.retain(|&(p, _)| p != 0);
        for (p, len) in ptrs {
            let Some(o) = sys_off(res, p) else { bail!("données de géométrie hors du segment système") };
            let e = out.entry(o).or_insert(len);
            *e = (*e).max(len);
        }
    }
    Ok(out)
}

fn decimable(g: &GeometryInfo, shared: &Shared) -> bool {
    !g.decl.is_cloth()
        && !shared.vertex.contains(&g.vb_offset)
        && !shared.index.contains(&g.index_ptr)
        && g.triangles() >= 64
        && g.decl.has(semantic::POSITION)
        && g.decl.component_type(semantic::POSITION) == component::FLOAT3
}

/// Vertex buffers (structure offset) and index data (pointer) used by more than one geometry.
struct Shared {
    vertex: HashSet<usize>,
    index: HashSet<u64>,
}

fn shared_buffers(geoms: &[(usize, usize, GeometryInfo)]) -> Shared {
    let mut vb: HashMap<usize, usize> = HashMap::new();
    let mut ib: HashMap<u64, usize> = HashMap::new();
    for (_, _, g) in geoms {
        *vb.entry(g.vb_offset).or_default() += 1;
        *ib.entry(g.index_ptr).or_default() += 1;
    }
    Shared {
        vertex: vb.into_iter().filter(|&(_, c)| c > 1).map(|(o, _)| o).collect(),
        index: ib.into_iter().filter(|&(_, c)| c > 1).map(|(o, _)| o).collect(),
    }
}

fn read(res: &Resource, g: &GeometryInfo) -> Result<(Vec<u8>, Vec<u16>)> {
    let v = res.slice(vertex_ptr(g), g.vertex_count as usize * g.vertex_stride as usize)?.to_vec();
    let i = res.slice(g.index_ptr, g.index_count as usize * 2)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Ok((v, i))
}

/// Raw buffer list for [`sys`] with the decimated geometries' new content.
fn raw_list(res: &Resource, buffers: &BTreeMap<usize, usize>, writes: &[(&GeometryInfo, &Decimated)]) -> Result<Vec<RawBuffer>> {
    let mut new_data: HashMap<usize, Vec<u8>> = HashMap::new();
    for (g, d) in writes {
        for p in [g.vertex_ptr, g.vertex_ptr2] {
            if let Some(o) = sys_off(res, p) {
                new_data.insert(o, d.vertices.clone());
            }
        }
        if let Some(o) = sys_off(res, g.index_ptr) {
            new_data.insert(o, d.indices.iter().flat_map(|i| i.to_le_bytes()).collect());
        }
    }
    Ok(buffers
        .iter()
        .map(|(&off, &len)| RawBuffer { off, old_len: len, data: new_data.remove(&off).unwrap_or_else(|| res.system[off..off + len].to_vec()) })
        .collect())
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Plans the geometry reduction of a file whose declared virtual memory is above the budget.
pub fn plan(res: &Resource, drawables: &[DrawableInfo], cfg: &Config) -> Result<Option<GeoPlan>> {
    let before = res.system_flags.size();
    if !cfg.geo_enabled || before <= cfg.budget {
        return Ok(None);
    }
    let geoms = geometries(drawables);
    let buffers = raw_buffers(res, &geoms)?;
    let shared = shared_buffers(&geoms);
    let high: Vec<u64> = drawables.iter().map(|d| d.lod_triangles(0)).collect();
    let original: Vec<u64> = (0..geoms.len()).map(|i| geoms[i].2.triangles() as u64).collect();
    let lod_tris = |di: usize, lod: usize| -> u64 { geoms.iter().filter(|(d, l, _)| *d == di && *l == lod).map(|(_, _, g)| g.triangles() as u64).sum() };

    // Share kept per (drawable, lod), and the decimations it gives (cached per geometry and share).
    let mut share: HashMap<(usize, usize), f64> = HashMap::new();
    let mut cache: HashMap<(usize, u64), Option<Decimated>> = HashMap::new();
    let mut warnings = Vec::new();
    let mut size = sys::packed_size(res, &raw_list(res, &buffers, &[])?)?;
    let mut applied: Vec<usize> = Vec::new();

    for (lod, ratio) in steps(cfg) {
        if size <= cfg.budget {
            break;
        }
        let max_error = if lod == 0 { cfg.geo_high_error } else { LOD_ERROR[lod] };
        // Share of this step per drawable, raised to the floor of the LOD; a drawable whose
        // LOD would barely change is left out.
        let mut todo: Vec<(usize, u64)> = Vec::new();
        for di in 0..drawables.len() {
            let orig = lod_tris(di, lod);
            if orig == 0 {
                continue;
            }
            let floor = if lod == 0 { 0.0 } else { LOD_FLOOR[lod] * high[di] as f64 / orig as f64 };
            let eff = ratio.max(floor).min(1.0);
            if eff > share.get(&(di, lod)).copied().unwrap_or(1.0) - 0.05 {
                continue;
            }
            share.insert((di, lod), eff);
            let key = (eff * 1000.0).round() as u64;
            todo.extend((0..geoms.len()).filter(|&i| geoms[i].0 == di && geoms[i].1 == lod && decimable(&geoms[i].2, &shared)).map(|i| (i, key)));
        }
        if todo.is_empty() {
            continue;
        }
        let fresh: Vec<((usize, u64), Option<Decimated>)> = todo
            .par_iter()
            .filter(|k| !cache.contains_key(k))
            .map(|&(i, key)| -> Result<((usize, u64), Option<Decimated>)> {
                let g = &geoms[i].2;
                let (v, idx) = read(res, g)?;
                let data = GeometryData { decl: g.decl, stride: g.vertex_stride as usize, vertices: &v, indices: &idx };
                let target = ((original[i] as f64) * key as f64 / 1000.0).ceil() as usize;
                // Vehicle shaders are matched by hash only: keep the triangle order (safe for alpha).
                Ok(((i, key), decimate(&data, target, max_error, true)?))
            })
            .collect::<Result<_>>()?;
        cache.extend(fresh);
        let todo: Vec<usize> = todo.into_iter().map(|(i, _)| i).collect();
        for &i in &todo {
            if !applied.contains(&i) {
                applied.push(i);
            }
        }
        let writes = current(&geoms, &applied, &share, &cache);
        size = sys::packed_size(res, &raw_list(res, &buffers, &writes)?)?;
    }

    let writes: Vec<(GeometryInfo, Decimated)> =
        current(&geoms, &applied, &share, &cache).into_iter().map(|(g, d)| (g.clone(), d.clone())).collect();
    if writes.is_empty() {
        warnings.push(format!("mémoire virtuelle {} (budget {}) : aucune géométrie réductible dans la tolérance", mib(before), mib(cfg.budget)));
        return Ok(Some(GeoPlan { writes, changes: Vec::new(), warnings, virtual_after: before }));
    }

    // Changes per drawable and LOD.
    let mut changes = Vec::new();
    for (di, _) in drawables.iter().enumerate() {
        for lod in 0..4 {
            let before_t = lod_tris(di, lod);
            if before_t == 0 {
                continue;
            }
            let (mut after_t, mut err, mut touched) = (0u64, 0f32, false);
            for (_, _, g) in geoms.iter().filter(|(gd, gl, _)| *gd == di && *gl == lod) {
                match writes.iter().find(|(w, _)| w.offset == g.offset) {
                    Some((_, d)) => {
                        after_t += (d.indices.len() / 3) as u64;
                        err = err.max(d.error);
                        touched = true;
                    }
                    None => after_t += g.triangles() as u64,
                }
            }
            if !touched {
                continue;
            }
            changes.push(format!(
                "géométrie {} LOD {} : {} → {} triangles (écart max {:.2} %)",
                if di == 0 { "principale".to_owned() } else { format!("pièce {di}") },
                drawable::LOD_NAMES[lod],
                group(before_t),
                group(after_t),
                err * 100.0
            ));
        }
    }
    if size > cfg.budget {
        warnings.push(format!(
            "mémoire virtuelle : reste à {} (budget {}) : tolérance ou plancher du LOD High atteints",
            mib(size),
            mib(cfg.budget)
        ));
    }
    Ok(Some(GeoPlan { writes, changes, warnings, virtual_after: size }))
}

/// Decimations in force: the latest share of each geometry's LOD.
fn current<'a>(
    geoms: &'a [(usize, usize, GeometryInfo)],
    applied: &[usize],
    share: &HashMap<(usize, usize), f64>,
    cache: &'a HashMap<(usize, u64), Option<Decimated>>,
) -> Vec<(&'a GeometryInfo, &'a Decimated)> {
    applied
        .iter()
        .filter_map(|&i| {
            let (di, lod, g) = &geoms[i];
            let key = (share.get(&(*di, *lod))? * 1000.0).round() as u64;
            cache.get(&(i, key))?.as_ref().map(|d| (g, d))
        })
        .collect()
}

/// Writes the plan: counts patched in the structures, system segment re-laid out.
pub fn apply(res: &mut Resource, drawables: &[DrawableInfo], plan: &GeoPlan) -> Result<()> {
    let geoms = geometries(drawables);
    let buffers = raw_buffers(res, &geoms)?;
    let writes: Vec<(&GeometryInfo, &Decimated)> = plan.writes.iter().map(|(g, d)| (g, d)).collect();
    let raw = raw_list(res, &buffers, &writes)?;
    for (g, d) in &plan.writes {
        let vcount = d.vertex_count(g.vertex_stride as usize);
        res.set_u32(g.vb_offset + vb_field::VERTEX_COUNT, vcount as u32);
        res.set_u16(g.offset + geom_field::VERTICES_COUNT, vcount as u16);
        res.set_u32(g.ib_offset + ib_field::INDICES_COUNT, d.indices.len() as u32);
        res.set_u32(g.offset + geom_field::INDICES_COUNT, d.indices.len() as u32);
        res.set_u32(g.offset + geom_field::TRIANGLES_COUNT, (d.indices.len() / 3) as u32);
    }
    sys::rebuild(res, &raw)
}
