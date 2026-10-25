//! Clothing file naming: `<ped>_<collection>^<slot>_<index>_<u|r>.ydd`,
//! `<ped>_<collection>^<slot>_diff_<index>_<variant>_<race>.ytd`, props `p_<slot>_<index>.ydd`.

/// Parsed clothing file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClothingName {
    /// Part before `^` (ped model + collection), empty if there is none.
    pub prefix: String,
    /// Component slot (`jbib`, `lowr`, ...) or prop slot (`p_head`, ...).
    pub slot: String,
    pub index: Option<u32>,
}

pub fn parse(file_name: &str) -> ClothingName {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
    let (prefix, rest) = match stem.split_once('^') {
        Some((p, r)) => (p.to_owned(), r),
        None => (String::new(), stem),
    };
    let parts: Vec<&str> = rest.split('_').collect();
    let (slot, tail) = if parts.first() == Some(&"p") && parts.len() > 1 {
        (format!("p_{}", parts[1]), &parts[2..])
    } else {
        (parts.first().copied().unwrap_or("").to_owned(), &parts[1.min(parts.len())..])
    };
    let index = tail.iter().find_map(|p| p.parse::<u32>().ok());
    ClothingName { prefix, slot, index }
}

/// Race tags of the textures of skin-dependent (`_r`) drawables (`uni` = universal).
const RACE_TAGS: [&str; 7] = ["whi", "bla", "chi", "lat", "ara", "kor", "pak"];

/// Diffuse variation of a skin-dependent drawable (`jbib_diff_000_a_whi.ytd`): it must
/// keep the dimensions of the original MP skin.
pub fn is_skin_texture(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
    let rest = stem.split_once('^').map(|(_, r)| r).unwrap_or(stem);
    let parts: Vec<&str> = rest.split('_').collect();
    parts.contains(&"diff") && parts.last().is_some_and(|p| RACE_TAGS.contains(p))
}

fn texture_parts(file_name: &str) -> (String, Vec<String>) {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
    let (prefix, rest) = match stem.split_once('^') {
        Some((p, r)) => (format!("{p}^"), r),
        None => (String::new(), stem),
    };
    (prefix, rest.split('_').map(str::to_owned).collect())
}

/// Diffuse variation of a drawable (`jbib_diff_048_a_uni.ytd`, `p_head_diff_010_h.ytd`).
pub fn is_variation_texture(file_name: &str) -> bool {
    texture_parts(file_name).1.iter().any(|p| p == "diff")
}

/// Possible file names of the drawable a diffuse variation belongs to:
/// `jbib_diff_048_a_uni.ytd` → `jbib_048_u.ydd` / `jbib_048_r.ydd`, props `p_head_010.ydd`.
pub fn drawable_names(file_name: &str) -> Vec<String> {
    let (prefix, parts) = texture_parts(file_name);
    let Some(i) = parts.iter().position(|p| p == "diff") else {
        return Vec::new();
    };
    let Some(index) = parts.get(i + 1) else {
        return Vec::new();
    };
    let base = format!("{prefix}{}_{index}", parts[..i].join("_"));
    ["_u", "_r", ""].iter().map(|suffix| format!("{base}{suffix}.ydd")).collect()
}

/// Pieces with a large visual footprint get the "large" polygon budget.
pub fn is_large_slot(slot: &str) -> bool {
    matches!(slot, "uppr" | "lowr" | "jbib" | "accs" | "task" | "hair" | "feet" | "berd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_and_texture_names() {
        let n = parse("mp_m_freemode_01_oreneupfinal^task_031_u.ydd");
        assert_eq!((n.prefix.as_str(), n.slot.as_str(), n.index), ("mp_m_freemode_01_oreneupfinal", "task", Some(31)));
        let t = parse("mp_f_freemode_01_x^jbib_diff_048_a_uni.ytd");
        assert_eq!((t.slot.as_str(), t.index), ("jbib", Some(48)));
        let p = parse("mp_m_freemode_01_p_x^p_head_003.ydd");
        assert_eq!((p.slot.as_str(), p.index), ("p_head", Some(3)));
    }

    #[test]
    fn skin_textures() {
        assert!(is_skin_texture("mp_m_freemode_01_x^uppr_diff_012_a_whi.ytd"));
        assert!(is_skin_texture("uppr_diff_012_c_bla.ytd"));
        assert!(!is_skin_texture("mp_f_freemode_01_x^jbib_diff_048_a_uni.ytd"));
        assert!(!is_skin_texture("mp_m_freemode_01_p_x^p_head_diff_010_h.ytd"));
        assert!(!is_skin_texture("mp_m_freemode_01_x^uppr_012_r.ydd"));
    }

    #[test]
    fn variations_and_their_drawable() {
        assert!(is_variation_texture("mp_m_freemode_01_p_x^p_head_diff_000_n.ytd"));
        assert!(!is_variation_texture("mp_m_freemode_01_x^jbib_048_u.ydd"));
        assert_eq!(
            drawable_names("mp_f_freemode_01_x^jbib_diff_048_a_uni.ytd"),
            ["mp_f_freemode_01_x^jbib_048_u.ydd", "mp_f_freemode_01_x^jbib_048_r.ydd", "mp_f_freemode_01_x^jbib_048.ydd"]
        );
        assert_eq!(drawable_names("mp_m_freemode_01_p_x^p_head_diff_010_h.ytd")[2], "mp_m_freemode_01_p_x^p_head_010.ydd");
        assert!(drawable_names("hair_normal_007.ytd").is_empty());
    }
}
