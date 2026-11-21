//! Geometry reduction of a model above the virtual memory budget.
//!
//! The vertex / index data of `.yft` / `.ydr` / `.ydd` files lives in the system segment
//! (virtual memory). The segment is first re-laid out with every buffer cut to its used
//! length (a geometry decimated in place keeps a zero-filled tail until then) and
//! byte-identical vertices merged (lossless). If that is not enough, steps from the least to the most visible are tried in turn (far LODs
//! halved first, the High LOD last and only down to the floor); each step starts again
//! from the current geometry and the declared size is measured with the real system
//! packer ([`optistream_rage::sys`]). The decimator keeps original vertices (skinning
//! intact), locks borders, seams and bone boundaries, and stops at the error tolerance.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Result};
use optistream_i18n::{t, tf};
use optistream_rage::drawable::{self, component, geom_field, ib_field, semantic, vb_field, DrawableInfo, GeometryInfo};
use optistream_rage::sys::{self, RawBuffer};
use optistream_rage::{Resource, Segment};
use rayon::prelude::*;

use crate::{decimate, dedupe_vertices, Decimated, GeometryData};

/// Smallest share of the High LOD a lower LOD may keep (med, low, vlow).
const LOD_FLOOR: [f64; 4] = [1.0, 0.15, 0.05, 0.02];
/// Error tolerance of the far LODs (fraction of the geometry extent).
const LOD_ERROR: [f32; 4] = [0.0, 0.02, 0.03, 0.05];

/// Limits of the reduction.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Declared virtual memory to get under (`u64::MAX`: re-layout only, no decimation).
    pub budget: u64,
    /// Smallest share of its triangles the High LOD may keep (0.5 to 1.0).
    pub high_floor: f64,
    /// Error tolerance of the High LOD (fraction of the geometry extent).
    pub high_error: f32,
}

/// Reduction steps: `(lod, share of the current triangles)`, least visible first.
fn steps(limits: &Limits) -> Vec<(usize, f64)> {
    let mut s = vec![(3, 0.5), (2, 0.5), (1, 0.5), (3, 0.25), (2, 0.25), (1, 0.35)];
    let mut r = 0.9;
    while r + 1e-9 >= limits.high_floor {
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
    /// Declared virtual memory after the re-layout (with the writes).
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
            let Some(o) = sys_off(res, p) else { bail!(t("mesh.error.outside_system")) };
            let e = out.entry(o).or_insert(len);
            *e = (*e).max(len);
        }
    }
    Ok(out)
}

/// Buffers of the file: room allocated (from the geometry as first read) and length in use
/// (from the geometry as it is now, after an in-place decimation).
struct Buffers {
    allocated: BTreeMap<usize, usize>,
    used: BTreeMap<usize, usize>,
}

impl Buffers {
    fn new(res: &Resource, original: &[DrawableInfo], geoms: &[(usize, usize, GeometryInfo)]) -> Result<Self> {
        let used = raw_buffers(res, geoms)?;
        let mut allocated = raw_buffers(res, &geometries(original))?;
        if allocated.len() != used.len() || allocated.keys().ne(used.keys()) {
            bail!(t("mesh.error.buffers_changed"));
        }
        for (o, len) in &used {
            let a = allocated.get_mut(o).expect("same keys");
            *a = (*a).max(*len);
        }
        Ok(Buffers { allocated, used })
    }
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

/// Raw buffer list for [`sys`]: allocated room, used content or the decimated geometries' one.
fn raw_list(res: &Resource, buffers: &Buffers, writes: &[(&GeometryInfo, &Decimated)]) -> Result<Vec<RawBuffer>> {
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
        .allocated
        .iter()
        .map(|(&off, &len)| {
            let data = new_data.remove(&off).unwrap_or_else(|| res.system[off..off + buffers.used[&off]].to_vec());
            RawBuffer { off, old_len: len, data }
        })
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

pub fn mib(bytes: u64) -> String {
    tf!("core.size.mib", size = format!("{:.1}", bytes as f64 / (1024.0 * 1024.0)))
}

/// Plans the re-layout of the system segment and, if it stays above the budget, the
/// geometry reduction.
///
/// `original`: drawables as first read (room allocated to each buffer); `current`: the same
/// drawables as they are now (pass the same slice twice when nothing was edited).
/// `policy(drawable, geometry)`: `None` leaves the geometry untouched, `Some(keep_order)`
/// lets it be decimated (`keep_order`: alpha shaders, the triangle order is kept).
pub fn plan(
    res: &Resource,
    original: &[DrawableInfo],
    current: &[DrawableInfo],
    limits: &Limits,
    policy: impl Fn(usize, &GeometryInfo) -> Option<bool> + Sync,
) -> Result<GeoPlan> {
    let geoms = geometries(current);
    let buffers = Buffers::new(res, original, &geoms)?;
    let shared = shared_buffers(&geoms);
    let high: Vec<u64> = current.iter().map(|d| d.lod_triangles(0)).collect();
    let start: Vec<u64> = (0..geoms.len()).map(|i| geoms[i].2.triangles() as u64).collect();
    let lod_tris = |di: usize, lod: usize| -> u64 { geoms.iter().filter(|(d, l, _)| *d == di && *l == lod).map(|(_, _, g)| g.triangles() as u64).sum() };
    let keep_order: Vec<Option<bool>> = geoms.iter().map(|(di, _, g)| if decimable(g, &shared) { policy(*di, g) } else { None }).collect();

    // Share kept per (drawable, lod), and the decimations it gives (cached per geometry and share).
    let mut share: HashMap<(usize, usize), f64> = HashMap::new();
    let mut cache: HashMap<(usize, u64), Option<Decimated>> = HashMap::new();
    let mut warnings = Vec::new();
    // Lossless first: byte-identical vertices merged (triangle soup exports). Every step
    // then starts from this geometry.
    let base: Vec<Option<Decimated>> = (0..geoms.len())
        .into_par_iter()
        .map(|i| -> Result<Option<Decimated>> {
            if keep_order[i].is_none() {
                return Ok(None);
            }
            let g = &geoms[i].2;
            let (v, idx) = read(res, g)?;
            Ok(dedupe_vertices(&GeometryData { decl: g.decl, stride: g.vertex_stride as usize, vertices: &v, indices: &idx }))
        })
        .collect::<Result<_>>()?;
    let mut applied: Vec<usize> = Vec::new();
    let relaid = sys::packed_size(res, &raw_list(res, &buffers, &current_writes(&geoms, &applied, &share, &cache, &base))?)?;
    let mut size = relaid;

    for (lod, ratio) in steps(limits) {
        if size <= limits.budget {
            break;
        }
        let max_error = if lod == 0 { limits.high_error } else { LOD_ERROR[lod] };
        // Share of this step per drawable, raised to the floor of the LOD; a drawable whose
        // LOD would barely change is left out.
        let mut todo: Vec<(usize, u64)> = Vec::new();
        for di in 0..current.len() {
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
            todo.extend((0..geoms.len()).filter(|&i| geoms[i].0 == di && geoms[i].1 == lod && keep_order[i].is_some()).map(|i| (i, key)));
        }
        if todo.is_empty() {
            continue;
        }
        let fresh: Vec<((usize, u64), Option<Decimated>)> = todo
            .par_iter()
            .filter(|k| !cache.contains_key(k))
            .map(|&(i, key)| -> Result<((usize, u64), Option<Decimated>)> {
                let g = &geoms[i].2;
                let (v, idx) = match &base[i] {
                    Some(d) => (d.vertices.clone(), d.indices.clone()),
                    None => read(res, g)?,
                };
                let data = GeometryData { decl: g.decl, stride: g.vertex_stride as usize, vertices: &v, indices: &idx };
                let target = ((start[i] as f64) * key as f64 / 1000.0).ceil() as usize;
                Ok(((i, key), decimate(&data, target, max_error, keep_order[i].unwrap_or(true))?))
            })
            .collect::<Result<_>>()?;
        cache.extend(fresh);
        let todo: Vec<usize> = todo.into_iter().map(|(i, _)| i).collect();
        for &i in &todo {
            if !applied.contains(&i) {
                applied.push(i);
            }
        }
        let writes = current_writes(&geoms, &applied, &share, &cache, &base);
        size = sys::packed_size(res, &raw_list(res, &buffers, &writes)?)?;
    }

    let writes: Vec<(GeometryInfo, Decimated)> =
        current_writes(&geoms, &applied, &share, &cache, &base).into_iter().map(|(g, d)| (g.clone(), d.clone())).collect();
    if writes.is_empty() {
        if relaid > limits.budget {
            warnings.push(tf!("mesh.warning.nothing_reducible", size = mib(relaid), budget = mib(limits.budget)));
        }
        return Ok(GeoPlan { writes, changes: Vec::new(), warnings, virtual_after: relaid });
    }

    let mut changes = Vec::new();
    let (merged_before, merged_after) = geoms
        .iter()
        .zip(&base)
        .filter_map(|((_, _, g), b)| b.as_ref().map(|d| (g.vertex_count as u64, d.vertex_count(g.vertex_stride as usize) as u64)))
        .fold((0, 0), |(a, b), (x, y)| (a + x, b + y));
    if merged_before > 0 {
        changes.push(tf!("mesh.change.merged_vertices", before = group(merged_before), after = group(merged_after)));
    }
    // Triangle changes per drawable and LOD.
    for (di, _) in current.iter().enumerate() {
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
            if !touched || after_t == before_t {
                continue;
            }
            let part = if di == 0 { t("mesh.part.main").to_owned() } else { tf!("mesh.part.piece", index = di) };
            changes.push(tf!(
                "mesh.change.lod_triangles",
                part = part,
                lod = drawable::LOD_NAMES[lod],
                before = group(before_t),
                after = group(after_t),
                error = format!("{:.2}", err * 100.0)
            ));
        }
    }
    if size > limits.budget {
        warnings.push(tf!("mesh.warning.over_budget", size = mib(size), budget = mib(limits.budget)));
    }
    Ok(GeoPlan { writes, changes, warnings, virtual_after: size })
}

/// Geometry in force: the latest share of each geometry's LOD, else its lossless merge.
fn current_writes<'a>(
    geoms: &'a [(usize, usize, GeometryInfo)],
    applied: &[usize],
    share: &HashMap<(usize, usize), f64>,
    cache: &'a HashMap<(usize, u64), Option<Decimated>>,
    base: &'a [Option<Decimated>],
) -> Vec<(&'a GeometryInfo, &'a Decimated)> {
    (0..geoms.len())
        .filter_map(|i| {
            let (di, lod, g) = &geoms[i];
            let decimated = applied
                .contains(&i)
                .then(|| share.get(&(*di, *lod)))
                .flatten()
                .and_then(|s| cache.get(&(i, (s * 1000.0).round() as u64)))
                .and_then(Option::as_ref);
            decimated.or(base[i].as_ref()).map(|d| (g, d))
        })
        .collect()
}

/// Writes the plan: counts patched in the structures, system segment re-laid out.
/// `original` / `current`: as given to [`plan`].
pub fn apply(res: &mut Resource, original: &[DrawableInfo], current: &[DrawableInfo], plan: &GeoPlan) -> Result<()> {
    let geoms = geometries(current);
    let buffers = Buffers::new(res, original, &geoms)?;
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

#[cfg(test)]
mod tests {
    use optistream_rage::drawable::VertexDecl;

    use super::*;

    fn limits(floor: f64) -> Limits {
        Limits { budget: 16 << 20, high_floor: floor, high_error: 0.005 }
    }

    /// Geometry with float3 positions, its own buffers and the given triangle count.
    fn geometry(offset: usize, triangles: u32) -> GeometryInfo {
        GeometryInfo {
            offset,
            vb_offset: offset + 1,
            ib_offset: offset + 2,
            decl: VertexDecl { flags: 1 << semantic::POSITION, stride: 12, count: 1, types: component::FLOAT3 as u64 },
            shader_index: Some(0),
            vertex_stride: 12,
            vertex_count: triangles * 3,
            vertex_ptr: 0x5000_0000 + offset as u64,
            vertex_ptr2: 0,
            index_ptr: 0x5100_0000 + offset as u64,
            index_count: triangles * 3,
        }
    }

    fn drawable(offset: usize, lods: [Vec<GeometryInfo>; 4]) -> DrawableInfo {
        DrawableInfo { offset, hash: 0, has_skeleton: false, shaders: Vec::new(), texture_dict: None, lods }
    }

    fn entries(geoms: &[GeometryInfo]) -> Vec<(usize, usize, GeometryInfo)> {
        geoms.iter().map(|g| (0, 0, g.clone())).collect()
    }

    #[test]
    fn far_lods_are_reduced_before_the_high_lod() {
        for floor in [0.5, 0.6, 0.8, 1.0] {
            let s = steps(&limits(floor));
            let first_high = s.iter().position(|&(lod, _)| lod == 0).unwrap_or(s.len());
            assert!(s[..first_high].iter().all(|&(lod, _)| (1..=3).contains(&lod)), "{floor}");
            assert!(s[first_high..].iter().all(|&(lod, _)| lod == 0), "{floor}");
            // Each LOD only goes down from one step to the next.
            for lod in 0..4 {
                let shares: Vec<f64> = s.iter().filter(|&&(l, _)| l == lod).map(|&(_, r)| r).collect();
                assert!(shares.windows(2).all(|w| w[1] < w[0]), "{floor} lod {lod}: {shares:?}");
                assert!(shares.iter().all(|&r| r > 0.0 && r < 1.0), "{floor} lod {lod}");
            }
            // The least visible LOD (VLow) is the first one touched.
            assert_eq!(s[0].0, 3);
        }
    }

    #[test]
    fn high_lod_steps_stop_at_the_floor() {
        for (pct, count) in [(100, 0), (95, 0), (90, 1), (80, 2), (60, 4), (50, 5)] {
            let l = limits(pct as f64 / 100.0);
            let high: Vec<f64> = steps(&l).into_iter().filter(|&(l, _)| l == 0).map(|(_, r)| r).collect();
            assert_eq!(high.len(), count, "floor {pct} %: {high:?}");
            assert!(high.iter().all(|&r| r + 1e-9 >= l.high_floor), "floor {pct} %: {high:?}");
            if let Some(&last) = high.last() {
                assert!((last - l.high_floor).abs() < 1e-6, "floor {pct} %: last {last}");
            }
        }
    }

    #[test]
    fn a_lower_high_floor_only_adds_steps() {
        // A stronger preset tries the same steps first, then more of the High LOD.
        let strict = steps(&limits(0.8));
        let loose = steps(&limits(0.6));
        assert!(loose.len() > strict.len());
        for (a, b) in strict.iter().zip(&loose) {
            assert_eq!(a.0, b.0);
            assert!((a.1 - b.1).abs() < 1e-9);
        }
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1000), "1 000");
        assert_eq!(group(100_000), "100 000");
        assert_eq!(group(1_234_567), "1 234 567");
        assert_eq!(group(u64::MAX), "18 446 744 073 709 551 615");
    }

    #[test]
    fn geometries_are_listed_once_in_first_seen_order() {
        let a = geometry(100, 10);
        let b = geometry(200, 10);
        let c = geometry(300, 10);
        // `a` is referenced by two LODs of the first drawable and by the second drawable.
        let ds = [
            drawable(0, [vec![a.clone(), b.clone()], vec![a.clone()], Vec::new(), Vec::new()]),
            drawable(1, [vec![a.clone()], Vec::new(), vec![c.clone()], Vec::new()]),
        ];
        let got: Vec<(usize, usize, usize)> = geometries(&ds).into_iter().map(|(d, l, g)| (d, l, g.offset)).collect();
        assert_eq!(got, [(0, 0, 100), (0, 0, 200), (1, 2, 300)]);
        assert!(geometries(&[]).is_empty());
    }

    #[test]
    fn buffers_used_twice_are_shared() {
        let a = geometry(100, 100);
        let mut b = geometry(200, 100);
        let c = geometry(300, 100);
        let mut d = geometry(400, 100);
        b.vb_offset = a.vb_offset;
        d.index_ptr = c.index_ptr;
        let shared = shared_buffers(&entries(&[a.clone(), b, c.clone(), d]));
        assert_eq!(shared.vertex, [a.vb_offset].into_iter().collect());
        assert_eq!(shared.index, [c.index_ptr].into_iter().collect());

        let none = shared_buffers(&entries(&[geometry(100, 100), geometry(200, 100)]));
        assert!(none.vertex.is_empty() && none.index.is_empty());
    }

    #[test]
    fn only_plain_unshared_meshes_are_decimable() {
        let no_share = Shared { vertex: HashSet::new(), index: HashSet::new() };
        assert!(decimable(&geometry(0, 64), &no_share));
        // Too small to be worth it.
        assert!(!decimable(&geometry(0, 63), &no_share));
        // Cloth is simulated vertex by vertex.
        let mut cloth = geometry(0, 1000);
        cloth.decl.flags |= 0x8000_0000;
        assert!(!decimable(&cloth, &no_share));
        // No position, or positions not stored as float3.
        let mut no_pos = geometry(0, 1000);
        no_pos.decl.flags = 1 << semantic::NORMAL;
        assert!(!decimable(&no_pos, &no_share));
        let mut half = geometry(0, 1000);
        half.decl.types = component::FLOAT4 as u64;
        assert!(!decimable(&half, &no_share));
        // Shared vertex or index data.
        let g = geometry(0, 1000);
        let by_vb = Shared { vertex: [g.vb_offset].into_iter().collect(), index: HashSet::new() };
        assert!(!decimable(&g, &by_vb));
        let by_ib = Shared { vertex: HashSet::new(), index: [g.index_ptr].into_iter().collect() };
        assert!(!decimable(&g, &by_ib));
    }
}
