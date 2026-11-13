//! OptiStream method for addon clothing and EUP packs.
//!
//! Handles `.ydd` (meshes + embedded normal/spec) and `.ytd` (diffuse variations).
//! Never renames files, never renumbers drawables, never touches `.ymt`, `.yld`,
//! palettes, render targets, emissive textures or cloth geometry.

pub mod config;
pub mod crop;
pub mod naming;
pub mod process;
pub mod shaders;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use optistream_core::{FileAnalysis, FileCtx, FileOutcome, Issue, Method, ParamSpec, Preset, PreviewItem, Settings};

use crate::config::Config;
use crate::crop::{Group, RunPlan};
use crate::process::{FileInfo, Kind};

/// Drawables above this index per component are invisible to other players.
pub const MAX_DRAWABLES_PER_COMPONENT: usize = 128;

/// Clothing method. Keeps the UV crop plan of the current run (see [`crop`]), taken in
/// [`Method::prepare`] from the files as they were before anything was written.
#[derive(Default)]
pub struct ClothingMethod {
    run: Mutex<Option<(PathBuf, Arc<RunPlan>)>>,
}

impl ClothingMethod {
    pub fn new() -> Self {
        Self::default()
    }

    /// Crop group of a file under the current settings.
    fn group_of(&self, ctx: &FileCtx, cfg: &Config) -> Option<Arc<Group>> {
        if !cfg.crop_uv || cfg.only_3d {
            return None;
        }
        let rel = ctx.rel_lower();
        if let Some((_, plan)) = self.run.lock().unwrap().as_ref().filter(|(root, _)| *root == ctx.root) {
            return plan.get(&rel).cloned();
        }
        // Outside a run (comparison of one file): plan its group alone.
        crop::plan_for(&ctx.root, &ctx.all_files, &rel)
    }
}

fn kind_of(ctx: &FileCtx) -> Option<Kind> {
    let name = ctx.file_name_lower();
    if name.ends_with(".ydd") {
        Some(Kind::Ydd)
    } else if name.ends_with(".ytd") {
        Some(Kind::Ytd)
    } else {
        None
    }
}

fn file_info(ctx: &FileCtx, group: Option<&Group>) -> (Kind, FileInfo) {
    let kind = kind_of(ctx).expect("handled file");
    let name = ctx.file_name_lower();
    let slot = naming::parse(&name).slot;
    // A cloth piece ships its simulation next to the drawable: `<name>.yld`.
    let cloth = kind == Kind::Ydd && ctx.has_sibling_with_ext("yld");
    let skin = kind == Kind::Ytd && naming::is_skin_texture(&name);
    let variation = kind == Kind::Ytd && naming::is_variation_texture(&name);
    let alpha_mask = variation && drawable_alpha_mask(ctx);
    let crop = group.map(|g| g.window);
    (kind, FileInfo { slot, cloth, skin, variation, alpha_mask, crop })
}

/// Whether the drawable of a diffuse variation only draws its diffuses with alpha shaders.
/// False when the drawable is missing or unreadable (filtering then stays as for opaque).
fn drawable_alpha_mask(ctx: &FileCtx) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<(PathBuf, u64), bool>>> = OnceLock::new();
    let Some(sibling) = crop::drawable_of(&ctx.rel_lower(), &ctx.all_files) else {
        return false;
    };
    let path = ctx.root.join(&sibling);
    let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) else {
        return false;
    };
    // Every variation of a drawable asks the same question: read and parse it once.
    let cache = CACHE.get_or_init(Default::default);
    let key = (path, len);
    if let Some(&v) = cache.lock().unwrap().get(&key) {
        return v;
    }
    let v = std::fs::read(&key.0).ok().and_then(|b| process::drawable_alpha_mask(&b).ok()).unwrap_or(false);
    cache.lock().unwrap().insert(key, v);
    v
}

impl Method for ClothingMethod {
    fn id(&self) -> &'static str {
        "clothing"
    }

    fn label(&self) -> &'static str {
        "Vêtements / EUP"
    }

    fn description(&self) -> &'static str {
        "Vêtements addon et packs EUP : textures (.ytd et embarquées) et polygones (.ydd)."
    }

    fn params(&self) -> Vec<ParamSpec> {
        config::params()
    }

    fn presets(&self) -> Vec<Preset> {
        config::presets()
    }

    fn default_preset(&self) -> &'static str {
        "equilibre"
    }

    fn handles(&self, ctx: &FileCtx) -> bool {
        kind_of(ctx).is_some()
    }

    fn analyze_file(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<FileAnalysis> {
        let cfg = Config::from_settings(settings);
        let group = self.group_of(ctx, &cfg);
        let (kind, info) = file_info(ctx, group.as_deref());
        if let Some(reason) = skip_reason(&cfg, kind, data, group.as_deref()) {
            let kind = if kind == Kind::Ytd { "ytd" } else { "ydd" }.into();
            return Ok(FileAnalysis { kind, summary: format!("ignoré ({reason})"), skipped: true, ..Default::default() });
        }
        let parsed = process::parse(kind, data, &info)?;
        let mut a = process::analyze(&parsed, &cfg, &info);
        if let Some(g) = &group {
            a.issues.push(Issue::info(format!(
                "recadrage UV prévu : textures réduites à {}, avec {} autre(s) fichier(s) du même vêtement",
                g.window.describe(),
                g.files.len() - 1
            )));
        }
        Ok(a)
    }

    fn optimize_file(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<FileOutcome> {
        let cfg = Config::from_settings(settings);
        let group = self.group_of(ctx, &cfg);
        let (kind, info) = file_info(ctx, group.as_deref());
        if skip_reason(&cfg, kind, data, group.as_deref()).is_some() {
            return Ok(FileOutcome::default());
        }
        let (bytes, changes, warnings) = process::optimize(kind, data, &cfg, &info)?;
        Ok(FileOutcome { bytes, changes, warnings })
    }

    fn prepare(&self, files: &[FileCtx], settings: &Settings) -> Result<()> {
        let cfg = Config::from_settings(settings);
        let run = files.first().filter(|_| cfg.crop_uv && !cfg.only_3d).map(|f| {
            let plan = crop::plan_run(&f.root, &f.all_files);
            (f.root.clone(), Arc::new(plan))
        });
        // No crop: no plan either (`group_of` never plans anything with these settings). A
        // plan kept from this run would hide the groups from a later comparison made with
        // the crop switched on.
        *self.run.lock().unwrap() = run;
        Ok(())
    }

    fn linked_files(&self, ctx: &FileCtx) -> Vec<String> {
        let rel = ctx.rel_lower();
        match self.run.lock().unwrap().as_ref() {
            Some((root, plan)) if *root == ctx.root => {
                plan.get(&rel).map(|g| g.files.iter().filter(|f| **f != rel).cloned().collect()).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn resource_checks(&self, files: &[FileCtx]) -> Vec<Issue> {
        // Distinct drawable indices per (collection, component).
        let mut indices: BTreeMap<(String, String), BTreeSet<u32>> = BTreeMap::new();
        for ctx in files {
            if kind_of(ctx) != Some(Kind::Ydd) {
                continue;
            }
            // First-person variants (`jbib_000_u_1.ydd`) share the index of their drawable.
            let n = naming::parse(&ctx.file_name_lower());
            if let Some(i) = n.index {
                indices.entry((n.prefix, n.slot)).or_default().insert(i);
            }
        }
        let mut out = Vec::new();
        for ((prefix, slot), set) in indices {
            let max = set.iter().max().copied().unwrap_or(0) as usize;
            if set.len() > MAX_DRAWABLES_PER_COMPONENT || max >= MAX_DRAWABLES_PER_COMPONENT {
                out.push(Issue::error(format!(
                    "{prefix} / {slot} : {} drawables (index max {max}) — au-delà de {MAX_DRAWABLES_PER_COMPONENT}, invisibles pour les autres joueurs",
                    set.len()
                )));
            }
        }
        out
    }

    fn preview(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<Vec<PreviewItem>> {
        let cfg = Config::from_settings(settings);
        let group = self.group_of(ctx, &cfg);
        let (kind, info) = file_info(ctx, group.as_deref());
        if let Some(reason) = skip_reason(&cfg, kind, data, group.as_deref()) {
            anyhow::bail!("fichier ignoré avec les réglages actuels ({reason}) : rien à comparer. Change ce réglage dans « Général » pour le traiter.");
        }
        process::preview(kind, data, &cfg, &info)
    }
}

/// Why the current settings leave a file untouched, if they do. The files of a crop group
/// are processed together: the memory threshold applies to the heaviest of them.
fn skip_reason(cfg: &Config, kind: Kind, data: &[u8], group: Option<&Group>) -> Option<String> {
    if cfg.only_3d && kind == Kind::Ytd {
        return Some("« Fichiers 3D uniquement » : .ytd non traités".to_owned());
    }
    let below = match group {
        Some(g) => cfg.min_physical > 0 && g.max_physical <= cfg.min_physical,
        None => cfg.below_min_physical(data),
    };
    below.then(|| format!("mémoire physique ≤ {} Mio, seuil « Mémoire physique minimale »", cfg.min_physical / (1024 * 1024)))
}
