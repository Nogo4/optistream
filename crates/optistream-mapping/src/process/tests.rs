use super::*;

#[test]
fn kinds_from_names() {
    assert_eq!(Kind::of("a.ytd"), Some(Kind::Ytd));
    assert_eq!(Kind::of("b.ydr"), Some(Kind::Ydr));
    assert_eq!(Kind::of("c.ydd"), Some(Kind::Ydd));
    assert_eq!(Kind::of("d.yft"), None);
}

fn entry(name: &str, width: u16, height: u16, format: u32) -> optistream_rage::texture::TextureEntry {
    optistream_rage::texture::TextureEntry { struct_offset: 0, name: name.into(), width, height, depth: 1, stride: width, format, levels: 1, data_ptr: 0 }
}

#[test]
fn usage_prefers_own_shaders_then_resource_then_shape_then_name() {
    use optistream_rage::drawable::ShaderInfo;
    use optistream_rage::joaat::joaat;
    use optistream_rage::texture::format as tf;
    use optistream_texture::TextureRole;

    use crate::roles::RoleIndex;
    use parse::usage;

    let shader = |sampler: &str, name: &str| ShaderInfo { name_hash: 0, file_hash: 0, textures: vec![(joaat(sampler), name.into())] };
    let mut own = RoleIndex::default();
    own.add_shaders(&[shader("DiffuseSampler", "Wall_01")]);
    let mut resource = RoleIndex::default();
    resource.add_shaders(&[shader("BumpSampler", "wall_01"), shader("SpecSampler", "floor_01")]);

    let wall = entry("wall_01", 1024, 1024, tf::DXT5);
    // The drawable's own shaders win, even over a stronger usage elsewhere in the resource.
    assert_eq!(usage(&wall, Some(&own), &resource), Usage::Role(TextureRole::Diffuse));
    assert_eq!(usage(&wall, None, &resource), Usage::Role(TextureRole::Normal));
    assert_eq!(usage(&entry("floor_01", 512, 512, tf::DXT5), Some(&own), &resource), Usage::Role(TextureRole::Specular));
    // Unknown to every shader: an uncompressed strip is a lookup table, else the name decides.
    let empty = RoleIndex::default();
    assert_eq!(usage(&entry("grad", 256, 1, tf::A8R8G8B8), None, &empty), Usage::Protected);
    assert_eq!(usage(&entry("grad", 256, 1, tf::DXT1), None, &empty), Usage::Role(TextureRole::Diffuse));
    assert_eq!(usage(&entry("brick_n", 512, 512, tf::DXT5), None, &empty), Usage::Role(TextureRole::Normal));
}
