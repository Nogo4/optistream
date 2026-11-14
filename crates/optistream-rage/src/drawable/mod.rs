//! Drawables (`rmcDrawable`) and drawable dictionaries (YDD roots), Legacy layout.
//!
//! Only what OptiStream patches is decoded: shaders (name + texture parameters), the
//! embedded texture dictionary and, per LOD, every geometry with its vertex/index buffers.

mod read;

pub use read::{read_dictionary, read_drawable};

pub const LOD_NAMES: [&str; 4] = ["high", "med", "low", "vlow"];

/// Vertex component semantics, in declaration bit order.
pub mod semantic {
    pub const POSITION: u32 = 0;
    pub const BLEND_WEIGHTS: u32 = 1;
    pub const BLEND_INDICES: u32 = 2;
    pub const NORMAL: u32 = 3;
    pub const COLOUR0: u32 = 4;
    pub const COLOUR1: u32 = 5;
    pub const TEXCOORD0: u32 = 6;
    pub const TANGENT: u32 = 14;
    pub const BINORMAL: u32 = 15;
}

/// Vertex component storage types (4 bits per semantic in `types`).
pub mod component {
    pub const NOTHING: u8 = 0;
    pub const HALF2: u8 = 1;
    pub const FLOAT: u8 = 2;
    pub const HALF4: u8 = 3;
    pub const FLOAT2: u8 = 5;
    pub const FLOAT3: u8 = 6;
    pub const FLOAT4: u8 = 7;
    pub const UBYTE4: u8 = 8;
    pub const COLOUR: u8 = 9;
    pub const RGBA8_SNORM: u8 = 10;

    pub fn size(t: u8) -> usize {
        match t {
            HALF2 | FLOAT | UBYTE4 | COLOUR | RGBA8_SNORM => 4,
            HALF4 | FLOAT2 => 8,
            FLOAT3 => 12,
            FLOAT4 => 16,
            _ => 0,
        }
    }
}

/// `grcVertexDeclaration` (a.k.a. vertex format info).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexDecl {
    pub flags: u32,
    pub stride: u16,
    pub count: u8,
    pub types: u64,
}

impl VertexDecl {
    /// Cloth vertex formats set the high bit; their blend data binds to the cloth proxy.
    pub fn is_cloth(&self) -> bool {
        self.flags & 0x8000_0000 != 0
    }

    pub fn has(&self, sem: u32) -> bool {
        (self.flags >> sem) & 1 == 1
    }

    pub fn component_type(&self, sem: u32) -> u8 {
        ((self.types >> (sem * 4)) & 0xF) as u8
    }

    pub fn component_offset(&self, sem: u32) -> usize {
        (0..sem)
            .filter(|&k| self.has(k))
            .map(|k| component::size(self.component_type(k)))
            .sum()
    }
}

#[derive(Clone, Debug)]
pub struct ShaderInfo {
    pub name_hash: u32,
    pub file_hash: u32,
    /// `(parameter name hash, texture name)` for every texture parameter.
    pub textures: Vec<(u32, String)>,
}

#[derive(Clone, Debug)]
pub struct GeometryInfo {
    /// `grmGeometryQB` structure offset in the system segment.
    pub offset: usize,
    pub vb_offset: usize,
    pub ib_offset: usize,
    pub decl: VertexDecl,
    pub shader_index: Option<u16>,
    pub vertex_stride: u16,
    pub vertex_count: u32,
    pub vertex_ptr: u64,
    /// Second vertex data pointer of the vertex buffer (usually identical to `vertex_ptr`).
    pub vertex_ptr2: u64,
    pub index_ptr: u64,
    pub index_count: u32,
}

impl GeometryInfo {
    pub fn triangles(&self) -> u32 {
        self.index_count / 3
    }
}

#[derive(Clone, Debug)]
pub struct DrawableInfo {
    pub offset: usize,
    /// Hash from the parent dictionary, 0 for a standalone drawable.
    pub hash: u32,
    pub has_skeleton: bool,
    pub shaders: Vec<ShaderInfo>,
    /// Offset of the embedded texture dictionary, if any.
    pub texture_dict: Option<usize>,
    /// Geometries per LOD (high, med, low, vlow). A geometry is listed once even if shared.
    pub lods: [Vec<GeometryInfo>; 4],
}

impl DrawableInfo {
    pub fn lod_triangles(&self, lod: usize) -> u64 {
        self.lods[lod].iter().map(|g| g.triangles() as u64).sum()
    }

    pub fn lod_vertices(&self, lod: usize) -> u64 {
        self.lods[lod].iter().map(|g| g.vertex_count as u64).sum()
    }

    pub fn geometries(&self) -> impl Iterator<Item = &GeometryInfo> {
        self.lods.iter().flatten()
    }
}

pub mod geom_field {
    pub const VERTEX_BUFFER_PTR: usize = 0x18;
    pub const INDEX_BUFFER_PTR: usize = 0x38;
    pub const INDICES_COUNT: usize = 0x58;
    pub const TRIANGLES_COUNT: usize = 0x5C;
    pub const VERTICES_COUNT: usize = 0x60;
    pub const VERTEX_STRIDE: usize = 0x70;
    pub const VERTEX_DATA_PTR: usize = 0x78;
}

pub mod vb_field {
    pub const STRIDE: usize = 0x08;
    pub const DATA_PTR1: usize = 0x10;
    pub const VERTEX_COUNT: usize = 0x18;
    pub const DATA_PTR2: usize = 0x20;
    pub const INFO_PTR: usize = 0x30;
}

pub mod ib_field {
    pub const INDICES_COUNT: usize = 0x08;
    pub const INDICES_PTR: usize = 0x10;
}
