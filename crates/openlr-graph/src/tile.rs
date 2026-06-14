/// Magic bytes for the tile payload header.
pub const TILE_MAGIC: [u8; 4] = *b"OLRL";
pub const TILE_VERSION: u8 = 1;

/// Tile header — all integers little-endian.
#[repr(C)]
pub struct TileHeader {
    pub magic:              [u8; 4],
    pub version:            u8,
    pub flags:              u8,
    pub _pad:               [u8; 2],
    pub segment_count:      u32,
    pub node_count:         u32,
    pub restriction_count:  u32,
    pub geom_vertex_count:  u32,
    pub xrestriction_count: u32,
    pub _reserved:          [u8; 12],
}
const _: () = assert!(std::mem::size_of::<TileHeader>() == 40);
