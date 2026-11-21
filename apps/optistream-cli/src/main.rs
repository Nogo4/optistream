//! Command-line front-end, mainly for testing and batch runs.
//!
//! Every command accepts `--lang en|fr` (default: the system language).
//!
//! optistream-cli analyze  <resource dir> [--method clothing|mapping|vehicle] [--preset id] [--only <relative path>]...
//! optistream-cli optimize <resource dir> (--out <dir> | --in-place) [--method clothing|mapping|vehicle] [--preset id] [--set key=value]... [--overwrite] [--only <relative path>]...
//! optistream-cli crops    <resource dir>   (UV crop diagnostics)
//! optistream-cli inspect  <file>...
//! optistream-cli verify   <file>...   (every texture level decodes, every index valid, segment sizes = flags)

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use optistream_clothing::ClothingMethod;
use optistream_core::{Method, Output, Progress, Settings};
use optistream_i18n::{t, tf, Lang};
use optistream_rage::{drawable, fragment, texture, Resource, Segment};

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--lang en|fr` is applied before anything prints, then removed from the arguments.
    optistream_i18n::set_lang(optistream_i18n::detect());
    if let Some(i) = args.iter().position(|a| a == "--lang") {
        let code = args.get(i + 1).cloned().unwrap_or_default();
        let lang = Lang::from_code(&code).with_context(|| tf!("cli.error.unknown_lang", lang = code))?;
        optistream_i18n::set_lang(lang);
        args.drain(i..(i + 2).min(args.len()));
    }
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let has = |name: &str| args.iter().any(|a| a == name);
    // `--method clothing|mapping` (clothing by default).
    let method: Box<dyn Method> = match flag("--method").as_deref() {
        None | Some("clothing") => Box::new(ClothingMethod::new()),
        Some("mapping") => Box::new(optistream_mapping::MappingMethod::new()),
        Some("vehicle") => Box::new(optistream_vehicle::VehicleMethod::new()),
        Some(other) => bail!(tf!("cli.error.unknown_method", method = other)),
    };
    let method = method.as_ref();
    // `--only <relative path>` (repeatable) restricts the run to some files.
    let picked: std::collections::HashSet<PathBuf> =
        args.windows(2).filter(|w| w[0] == "--only").map(|w| PathBuf::from(&w[1])).collect();
    let only = (!picked.is_empty()).then_some(picked);
    let settings = || -> Result<Settings> {
        let id = flag("--preset").unwrap_or_else(|| method.default_preset().to_owned());
        let mut s = method.presets().into_iter().find(|p| p.id == id).map(|p| p.values).context(t("cli.error.unknown_preset"))?;
        // `--set key=value` (repeatable) overrides one setting of the preset.
        for w in args.windows(2).filter(|w| w[0] == "--set") {
            let (key, value) = w[1].split_once('=').context(t("cli.error.set_syntax"))?;
            let spec = method.params().into_iter().find(|p| p.key == key).with_context(|| tf!("cli.error.unknown_setting", key = key))?;
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
        Some("verify") => {
            let mut bad = 0;
            for p in &args[1..] {
                if let Err(e) = verify(Path::new(p)) {
                    bad += 1;
                    println!("FAIL {p} : {e:#}");
                }
            }
            println!("{}", tf!("cli.verify.summary", count = args.len() - 1, bad = bad));
        }
        Some("crops") => {
            // UV crop diagnostics: every candidate group and why it is (not) cropped.
            let root = PathBuf::from(args.get(1).context(t("cli.error.missing_dir"))?);
            let ctxs = optistream_core::contexts(&root)?;
            let Some(first) = ctxs.first() else { return Ok(()) };
            let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
            for (main, outcome) in optistream_clothing::crop::diagnose(&root, &first.all_files) {
                match outcome {
                    Ok(Some(w)) => println!("CROP {main} : {}", w.describe()),
                    Ok(None) => *reasons.entry(t("cli.crops.nothing_to_gain").to_owned()).or_default() += 1,
                    Err(e) => *reasons.entry(e).or_default() += 1,
                }
            }
            for (r, n) in reasons {
                println!("{n:6}  {r}");
            }
        }
        Some("analyze") => {
            let root = PathBuf::from(args.get(1).context(t("cli.error.missing_dir"))?);
            let a = optistream_core::analyze(method, &root, &settings()?, only.as_ref(), &Progress::default())?;
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
            println!("{}", tf!("cli.analyze.summary", files = a.files.len(), other = a.other_files));
        }
        Some("optimize") => {
            let root = PathBuf::from(args.get(1).context(t("cli.error.missing_dir"))?);
            let output = match (flag("--out"), has("--in-place")) {
                (Some(dir), false) => Output::Export { dir: dir.into(), overwrite: has("--overwrite") },
                (None, true) => Output::InPlace,
                _ => bail!(t("cli.error.output_choice")),
            };
            let report = optistream_core::optimize(method, &root, &settings()?, &output, only.as_ref(), &Progress::default())?;
            println!("{}", report.to_text());
        }
        _ => bail!(t("cli.usage")),
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
    } else if ext == "yft" {
        let f = fragment::read_fragment(&res)?;
        for (i, d) in f.drawables().enumerate() {
            let tris: Vec<u64> = (0..4).map(|l| d.lod_triangles(l)).collect();
            println!("  drawable {i} shaders {} tris {tris:?}", d.shaders.len());
            if let Some(td) = d.texture_dict {
                for t in texture::read_dictionary(&res, td)? {
                    print_tex(&res, &t);
                }
            }
        }
    }
    Ok(())
}

/// Decodes every level of every texture of a file and checks the graphics segment size.
fn verify(path: &Path) -> Result<()> {
    use optistream_texture::format::{level_dims, level_size};
    let bytes = std::fs::read(path)?;
    let res = Resource::parse(&bytes)?;
    anyhow::ensure!(res.graphics.len() as u64 == res.graphics_flags.size(), "graphics {} != flags {}", res.graphics.len(), res.graphics_flags.size());
    anyhow::ensure!(res.system.len() as u64 == res.system_flags.size(), "system {} != flags {}", res.system.len(), res.system_flags.size());
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    let kind = optistream_vehicle::process::Kind::of(&name).context(t("cli.error.unknown_kind"))?;
    let mut entries = Vec::new();
    if kind == optistream_vehicle::process::Kind::Ytd {
        entries = texture::read_dictionary(&res, 0)?;
    } else {
        let mut seen = std::collections::HashSet::new();
        for d in optistream_vehicle::process::read_drawables(&res, kind)? {
            if let Some(td) = d.texture_dict.filter(|td| seen.insert(*td)) {
                entries.extend(texture::read_dictionary(&res, td)?);
            }
            // Geometry: buffers inside the segment, every index within its vertex buffer.
            for g in d.geometries() {
                let vlen = g.vertex_count as usize * g.vertex_stride as usize;
                for ptr in [g.vertex_ptr, g.vertex_ptr2].into_iter().filter(|&p| p != 0) {
                    res.slice(ptr, vlen).with_context(|| format!("geometry 0x{:X} vertices", g.offset))?;
                }
                let idx = res.slice(g.index_ptr, g.index_count as usize * 2).with_context(|| format!("geometry 0x{:X} indices", g.offset))?;
                anyhow::ensure!(g.index_count % 3 == 0, "geometry 0x{:X}: index count not a multiple of 3", g.offset);
                let bad = idx.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).find(|&i| i >= g.vertex_count);
                anyhow::ensure!(bad.is_none(), "geometry 0x{:X}: index {:?} >= {} vertices", g.offset, bad, g.vertex_count);
            }
        }
    }
    for e in &entries {
        let Some((Segment::Graphics, mut off)) = res.resolve(e.data_ptr) else { continue };
        if optistream_texture::format::layout(e.format).is_none() {
            continue;
        }
        for l in 0..e.levels.max(1) as u32 {
            let (w, h) = level_dims(e.width as u32, e.height as u32, l);
            let len = level_size(e.format, w, h);
            anyhow::ensure!(off + len <= res.graphics.len(), "texture {} level {l} out of the segment", e.name);
            optistream_texture::codec::decode(e.format, w, h, &res.graphics[off..off + len])
                .with_context(|| format!("texture {} level {l}", e.name))?;
            off += len;
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

