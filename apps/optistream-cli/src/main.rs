//! Command-line front-end, mainly for testing and batch runs.
//!
//! optistream-cli analyze  <resource dir> [--preset id] [--only <relative path>]...
//! optistream-cli optimize <resource dir> (--out <dir> | --in-place) [--preset id] [--set key=value]... [--overwrite] [--only <relative path>]...
//! optistream-cli crops    <resource dir>   (UV crop diagnostics)
//! optistream-cli inspect  <file>...

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use optistream_clothing::ClothingMethod;
use optistream_core::{Method, Output, Progress, Settings};
use optistream_rage::{drawable, texture, Resource, Segment};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let has = |name: &str| args.iter().any(|a| a == name);
    let method = ClothingMethod::new();
    // `--only <relative path>` (repeatable) restricts the run to some files.
    let picked: std::collections::HashSet<PathBuf> =
        args.windows(2).filter(|w| w[0] == "--only").map(|w| PathBuf::from(&w[1])).collect();
    let only = (!picked.is_empty()).then_some(picked);
    let settings = || -> Result<Settings> {
        let id = flag("--preset").unwrap_or_else(|| method.default_preset().to_owned());
        let mut s = method.presets().into_iter().find(|p| p.id == id).map(|p| p.values).context("unknown preset")?;
        // `--set key=value` (repeatable) overrides one setting of the preset.
        for w in args.windows(2).filter(|w| w[0] == "--set") {
            let (key, value) = w[1].split_once('=').context("--set expects key=value")?;
            let spec = method.params().into_iter().find(|p| p.key == key).with_context(|| format!("unknown setting {key}"))?;
            let value = match spec.kind {
                optistream_core::ParamKind::Bool => optistream_core::Value::Bool(value == "true" || value == "1"),
                optistream_core::ParamKind::Float { .. } => optistream_core::Value::Float(value.parse()?),
                _ => optistream_core::Value::Int(value.parse()?),
            };
            s = s.with(spec.key, value);
        }
        Ok(s)
    };
    match args.first().map(String::as_str) {
        Some("inspect") => {
            for p in &args[1..] {
                inspect(Path::new(p))?;
            }
        }
        Some("crops") => {
            // UV crop diagnostics: every candidate group and why it is (not) cropped.
            let root = PathBuf::from(args.get(1).context("missing resource dir")?);
            let ctxs = optistream_core::contexts(&root)?;
            let Some(first) = ctxs.first() else { return Ok(()) };
            let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
            for (main, outcome) in optistream_clothing::crop::diagnose(&root, &first.all_files) {
                match outcome {
                    Ok(Some(w)) => println!("CROP {main} : {}", w.describe()),
                    Ok(None) => *reasons.entry("rien à gagner".to_owned()).or_default() += 1,
                    Err(e) => *reasons.entry(e).or_default() += 1,
                }
            }
            for (r, n) in reasons {
                println!("{n:6}  {r}");
            }
        }
        Some("analyze") => {
            let root = PathBuf::from(args.get(1).context("missing resource dir")?);
            let a = optistream_core::analyze(&method, &root, &settings()?, only.as_ref(), &Progress::default())?;
            for f in &a.files {
                match &f.analysis {
                    Ok(an) if !an.issues.is_empty() => {
                        println!("{} [{}] {}", f.rel.display(), an.kind, an.summary);
                        for i in &an.issues {
                            println!("    {:?}: {}", i.severity, i.text);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => println!("{} ERROR {e}", f.rel.display()),
                }
            }
            for i in &a.resource_issues {
                println!("RESOURCE {:?}: {}", i.severity, i.text);
            }
            println!("{} files analysed, {} other files", a.files.len(), a.other_files);
        }
        Some("optimize") => {
            let root = PathBuf::from(args.get(1).context("missing resource dir")?);
            let output = match (flag("--out"), has("--in-place")) {
                (Some(dir), false) => Output::Export { dir: dir.into(), overwrite: has("--overwrite") },
                (None, true) => Output::InPlace,
                _ => bail!("choose exactly one of --out <dir> or --in-place"),
            };
            let report = optistream_core::optimize(&method, &root, &settings()?, &output, only.as_ref(), &Progress::default())?;
            println!("{}", report.to_text());
        }
        _ => bail!("usage: optistream-cli (analyze|optimize|crops|inspect) ..."),
    }
    Ok(())
}

fn inspect(path: &Path) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let res = Resource::parse(&bytes)?;
    println!(
        "{}: v{} sys {} ({} pages) gfx {} ({} pages)",
        path.display(),
        res.version,
        res.system_flags.size(),
        res.system_flags.page_count(),
        res.graphics_flags.size(),
        res.graphics_flags.page_count()
    );
    if let Some(o) = res.pages_info_offset()? {
        println!("  pagesinfo @0x{o:X}: sys {} gfx {}", res.u8_at(o + 8)?, res.u8_at(o + 9)?);
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "ytd" {
        for t in texture::read_dictionary(&res, 0)? {
            print_tex(&res, &t);
        }
    } else if ext == "ydd" {
        for d in drawable::read_dictionary(&res, 0)? {
            println!("  drawable 0x{:08X} skel {} shaders {}", d.hash, d.has_skeleton, d.shaders.len());
            for s in &d.shaders {
                println!("    shader 0x{:08X} file 0x{:08X} {:?}", s.name_hash, s.file_hash, s.textures);
            }
            if let Some(td) = d.texture_dict {
                for t in texture::read_dictionary(&res, td)? {
                    print_tex(&res, &t);
                }
            }
            for (lod, geoms) in d.lods.iter().enumerate() {
                for g in geoms {
                    println!(
                        "    {} geom tris {} verts {} stride {} decl {:08X}/{:016X} vptr {:X}/{:X} iptr {:X} shader {:?}",
                        drawable::LOD_NAMES[lod], g.triangles(), g.vertex_count, g.vertex_stride,
                        g.decl.flags, g.decl.types, g.vertex_ptr, g.vertex_ptr2, g.index_ptr, g.shader_index
                    );
                }
            }
        }
    }
    Ok(())
}

fn print_tex(res: &Resource, t: &texture::TextureEntry) {
    let seg = res.resolve(t.data_ptr).map(|(s, o)| (if s == Segment::System { "sys" } else { "gfx" }, o));
    println!(
        "    tex {} {}x{} fmt {:08X} levels {} stride {} data {:?}",
        t.name, t.width, t.height, t.format, t.levels, t.stride, seg
    );
}
