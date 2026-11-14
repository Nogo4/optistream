//! Mesh optimisation: per-LOD decimation under the slot budget and lossless index reordering.

use std::collections::{BTreeMap, HashSet};

use anyhow::{bail, ensure, Result};
use optistream_mesh::{decimate, reorder_indices, Decimated, GeometryData};
use optistream_rage::drawable::{self, geom_field, ib_field, vb_field, DrawableInfo, GeometryInfo, ShaderInfo};
use optistream_rage::{Resource, Segment};
use rayon::prelude::*;

use crate::config::Config;
use crate::process::analyze::has_cloth;
use crate::process::{group, Parsed};
use crate::shaders;

pub(crate) fn vertex_ptr(g: &GeometryInfo) -> u64 {
    if g.vertex_ptr != 0 {
        g.vertex_ptr
    } else {
        g.vertex_ptr2
    }
}

pub(super) struct GeomData {
    pub(super) vertices: Vec<u8>,
    pub(super) indices: Vec<u16>,
}

pub(super) fn read_geometry(res: &Resource, g: &GeometryInfo) -> Result<GeomData> {
    let vlen = g.vertex_count as usize * g.vertex_stride as usize;
    let ilen = g.index_count as usize * 2;
    for ptr in [vertex_ptr(g), g.index_ptr] {
        if !matches!(res.resolve(ptr), Some((Segment::System, _))) {
            bail!("vertex/index data outside the system segment");
        }
    }
    let vertices = res.slice(vertex_ptr(g), vlen)?.to_vec();
    let indices = res.slice(g.index_ptr, ilen)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Ok(GeomData { vertices, indices })
}

/// Rewrites the index buffer of a geometry (never longer than the original one).
pub(super) fn write_indices(res: &mut Resource, g: &GeometryInfo, indices: &[u16]) -> Result<()> {
    ensure!(indices.len() <= g.index_count as usize, "index buffer cannot grow");
    let old_ilen = g.index_count as usize * 2;
    let ioff = res.sys_offset(g.index_ptr)?;
    let ibuf = &mut res.system[ioff..ioff + old_ilen];
    for (dst, &i) in ibuf.chunks_exact_mut(2).zip(indices) {
        dst.copy_from_slice(&i.to_le_bytes());
    }
    ibuf[indices.len() * 2..].fill(0);
    res.set_u32(g.ib_offset + ib_field::INDICES_COUNT, indices.len() as u32);
    res.set_u32(g.offset + geom_field::INDICES_COUNT, indices.len() as u32);
    res.set_u32(g.offset + geom_field::TRIANGLES_COUNT, (indices.len() / 3) as u32);
    Ok(())
}

fn write_geometry(res: &mut Resource, g: &GeometryInfo, d: &Decimated) -> Result<()> {
    let stride = g.vertex_stride as usize;
    let old_vlen = g.vertex_count as usize * stride;
    let mut targets: Vec<u64> = vec![g.vertex_ptr, g.vertex_ptr2];
    targets.retain(|&p| p != 0);
    targets.dedup();
    for ptr in targets {
        let off = res.sys_offset(ptr)?;
        let buf = &mut res.system[off..off + old_vlen];
        buf[..d.vertices.len()].copy_from_slice(&d.vertices);
        buf[d.vertices.len()..].fill(0);
    }
    let vcount = d.vertex_count(stride);
    res.set_u32(g.vb_offset + vb_field::VERTEX_COUNT, vcount as u32);
    res.set_u16(g.offset + geom_field::VERTICES_COUNT, vcount as u16);
    write_indices(res, g, &d.indices)
}

/// Shader drawing a geometry.
fn geometry_shader<'a>(d: &'a DrawableInfo, g: &GeometryInfo) -> Option<&'a ShaderInfo> {
    g.shader_index.and_then(|i| d.shaders.get(i as usize))
}

/// Geometries the decimation may touch (the others keep their triangle count).
pub(super) fn is_decimable(g: &GeometryInfo, shared_vbs: &HashSet<usize>) -> bool {
    !g.decl.is_cloth()
        && !shared_vbs.contains(&g.vb_offset)
        && g.triangles() >= 64
        && g.decl.component_type(drawable::semantic::POSITION) == drawable::component::FLOAT3
}

/// Ratio applied to every decimable geometry so that the whole LOD reaches `target`
/// although the other geometries (`fixed` triangles) are left as they are.
pub(super) fn lod_ratio(target: u64, decimable: u64, fixed: u64) -> f64 {
    if decimable == 0 {
        return 1.0;
    }
    (target.saturating_sub(fixed) as f64 / decimable as f64).clamp(0.0, 1.0)
}

/// Decimation plan of one LOD: per geometry, the decimated data (or `None` if kept).
pub(super) fn decimate_lod(
    res: &Resource,
    d: &DrawableInfo,
    geoms: &[GeometryInfo],
    shared_vbs: &HashSet<usize>,
    target_total: u64,
    max_error: f32,
) -> Result<Vec<Option<Decimated>>> {
    let total: u64 = geoms.iter().map(|g| g.triangles() as u64).sum();
    if total == 0 || target_total >= total {
        return Ok(geoms.iter().map(|_| None).collect());
    }
    let decimable: u64 =
        geoms.iter().filter(|g| is_decimable(g, shared_vbs)).map(|g| g.triangles() as u64).sum();
    let ratio = lod_ratio(target_total, decimable, total - decimable);
    // Geometries are independent: decimate them in parallel.
    geoms
        .par_iter()
        .map(|g| {
            if !is_decimable(g, shared_vbs) {
                return Ok(None);
            }
            let data = read_geometry(res, g)?;
            let geom = GeometryData {
                decl: g.decl,
                stride: g.vertex_stride as usize,
                vertices: &data.vertices,
                indices: &data.indices,
            };
            let target = ((g.triangles() as f64) * ratio).ceil() as usize;
            decimate(&geom, target, max_error, shaders::is_order_sensitive(geometry_shader(d, g)))
        })
        .collect()
}

/// Lossless index reordering planned for the geometries left untouched by the decimation.
pub(super) struct ReorderPlan {
    pub(super) writes: Vec<(GeometryInfo, Vec<u16>)>,
    removed_triangles: usize,
    /// Vertex shader invocations (ACMR x triangles) of every candidate geometry.
    cost_before: f64,
    cost_after: f64,
    triangles_before: u64,
    triangles_after: u64,
}

impl ReorderPlan {
    /// A reordering alone is only worth rewriting the file for a clear gain.
    pub(super) fn worth_alone(&self) -> bool {
        self.removed_triangles > 0 || self.cost_after <= self.cost_before * 0.9
    }

    pub(super) fn describe(&self) -> String {
        let acmr = |cost: f64, tris: u64| if tris == 0 { 0.0 } else { cost / tris as f64 };
        let mut s = format!(
            "ordre des triangles optimisé : {} géométrie(s), ACMR {:.2} → {:.2}",
            self.writes.len(),
            acmr(self.cost_before, self.triangles_before),
            acmr(self.cost_after, self.triangles_after)
        );
        if self.removed_triangles > 0 {
            s += &format!(", {} triangle(s) dégénéré(s) ou en double retiré(s)", group(self.removed_triangles as u64));
        }
        s
    }
}

pub(super) fn plan_reorder(p: &Parsed, decimated: &HashSet<usize>) -> Result<ReorderPlan> {
    // Index buffers referenced by more than one geometry are left alone.
    let mut ib_users: BTreeMap<u64, usize> = BTreeMap::new();
    for d in &p.drawables {
        for g in d.geometries() {
            *ib_users.entry(g.index_ptr).or_default() += 1;
        }
    }
    let mut candidates: Vec<(GeometryInfo, bool)> = Vec::new();
    for d in p.drawables.iter().filter(|d| !has_cloth(d)) {
        for g in d.geometries() {
            if g.decl.is_cloth() || decimated.contains(&g.offset) || ib_users[&g.index_ptr] > 1 {
                continue;
            }
            candidates.push((g.clone(), shaders::is_order_sensitive(geometry_shader(d, g))));
        }
    }
    let results: Vec<(GeometryInfo, Option<optistream_mesh::Reordered>, f32)> = candidates
        .into_par_iter()
        .map(|(g, order_sensitive)| {
            let data = read_geometry(&p.res, &g)?;
            let geom = GeometryData {
                decl: g.decl,
                stride: g.vertex_stride as usize,
                vertices: &data.vertices,
                indices: &data.indices,
            };
            let r = reorder_indices(&geom, order_sensitive)?;
            let acmr = if r.is_none() { optistream_mesh::index_acmr(&geom) } else { 0.0 };
            Ok((g, r, acmr))
        })
        .collect::<Result<_>>()?;

    let mut plan = ReorderPlan {
        writes: Vec::new(),
        removed_triangles: 0,
        cost_before: 0.0,
        cost_after: 0.0,
        triangles_before: 0,
        triangles_after: 0,
    };
    for (g, r, acmr) in results {
        let tris = g.triangles() as u64;
        plan.triangles_before += tris;
        match r {
            Some(r) => {
                let after = (r.indices.len() / 3) as u64;
                plan.triangles_after += after;
                plan.cost_before += r.acmr_before as f64 * tris as f64;
                plan.cost_after += r.acmr_after as f64 * after as f64;
                plan.removed_triangles += r.removed_triangles;
                plan.writes.push((g, r.indices));
            }
            None => {
                plan.triangles_after += tris;
                plan.cost_before += acmr as f64 * tris as f64;
                plan.cost_after += acmr as f64 * tris as f64;
            }
        }
    }
    Ok(plan)
}

pub(super) fn shared_vertex_buffers(drawables: &[DrawableInfo]) -> HashSet<usize> {
    let mut count: BTreeMap<usize, usize> = BTreeMap::new();
    for d in drawables {
        for g in d.geometries() {
            *count.entry(g.vb_offset).or_default() += 1;
        }
    }
    count.into_iter().filter(|&(_, c)| c > 1).map(|(o, _)| o).collect()
}

/// Decimates the drawables in place. Returns the change descriptions; the offsets of the
/// rewritten geometries are added to `decimated`.
pub(super) fn optimize_meshes(
    p: &mut Parsed,
    cfg: &Config,
    slot: &str,
    warnings: &mut Vec<String>,
    decimated: &mut HashSet<usize>,
) -> Result<Vec<String>> {
    let mut changes = Vec::new();
    let budget = cfg.budget_for_slot(slot);
    let shared = shared_vertex_buffers(&p.drawables);
    let drawables = p.drawables.clone();
    for d in &drawables {
        if has_cloth(d) {
            continue;
        }
        let high_before = d.lod_triangles(0);
        let mut high_after = high_before;
        for lod in 0..4 {
            let before = d.lod_triangles(lod);
            let target = if lod == 0 {
                budget
            } else if cfg.lower_lods {
                high_after
            } else {
                continue;
            };
            if before <= target {
                continue;
            }
            let plan = decimate_lod(&p.res, d, &d.lods[lod], &shared, target, cfg.max_error)?;
            let mut after = 0u64;
            let mut max_err = 0f32;
            for (g, dec) in d.lods[lod].iter().zip(&plan) {
                match dec {
                    Some(dec) => {
                        write_geometry(&mut p.res, g, dec)?;
                        decimated.insert(g.offset);
                        after += (dec.indices.len() / 3) as u64;
                        max_err = max_err.max(dec.error);
                    }
                    None => after += g.triangles() as u64,
                }
            }
            if lod == 0 {
                high_after = after;
            }
            if after < before {
                let mut c = format!(
                    "LOD {} : {} → {} triangles (écart {:.2} %)",
                    drawable::LOD_NAMES[lod],
                    group(before),
                    group(after),
                    max_err * 100.0
                );
                if after > target {
                    c += " — limité par la tolérance";
                }
                changes.push(c);
            } else if lod == 0 {
                warnings.push(format!(
                    "LOD High {} triangles : aucune réduction possible sans dépasser la tolérance",
                    group(before)
                ));
            }
        }
    }
    Ok(changes)
}

// ---------------------------------------------------------------- textures
