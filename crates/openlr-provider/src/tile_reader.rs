//! Parse OLRL v2 binary tile payloads into the in-memory Graph.
//!
//! Binary layout (all integers little-endian):
//!
//! Header       40 bytes
//! Segment array   segment_count × 32 bytes
//! Stable-id table segment_count × 16 bytes   (v2 only; OSM: way-id LE i64 + 8 zero bytes)
//! Geometry pool   geom_vertex_count × 8 bytes
//! Node table      node_count × 28 bytes
//! Intra restrictions  restriction_count × 16 bytes
//! Cross-tile restrictions  xrestriction_count × 40 bytes

use std::collections::HashMap;

use openlr_graph::{
    Direction, Graph, NetworkNode, NetworkSegment, NodeId, SegmentId, TurnRestriction,
};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum TileReadError {
    #[error("bad magic: expected OLRL, got {0:?}")]
    BadMagic([u8; 4]),
    #[error("unsupported tile version {0} (expected 2)")]
    UnsupportedVersion(u8),
    #[error("tile payload too short: need at least {need} bytes, have {have}")]
    TooShort { need: usize, have: usize },
    #[error("geometry index out of range: offset {offset} + len {len} > pool size {pool}")]
    GeomOutOfRange { offset: usize, len: usize, pool: usize },
    #[error("segment {index} has {len} geometry vertices (minimum 2)")]
    GeomTooShort { index: usize, len: usize },
    #[error("local node index {0} out of range")]
    NodeIndexOob(usize),
    #[error("local segment index {0} out of range")]
    SegIndexOob(usize),
}

// ── Cross-tile restriction pending entry ──────────────────────────────────────

/// A cross-tile turn restriction that cannot be fully resolved at parse time because
/// one or both segments live in a tile that may not yet be loaded.
/// `via_node` is always in the tile where the restriction was stored and is resolved
/// immediately; `from_gers`/`to_gers` are resolved in a post-load stitch pass.
#[derive(Debug, Clone)]
struct PendingXRestr {
    from_gers: [u8; 16],
    via_node:  NodeId,
    to_gers:   [u8; 16],
}

// ── Tile loader (multi-tile, boundary-node stitching) ─────────────────────────

/// Accumulates tiles into an in-memory `Graph`, stitching boundary nodes and
/// cross-tile turn restrictions across tiles.
pub struct TileLoader {
    pub graph: Graph,
    /// Stable source ID → global NodeId, universal dedup map (all nodes, not just boundary).
    boundary_nodes: HashMap<[u8; 16], NodeId>,
    next_node_id: u32,
    next_seg_id: u32,
    /// Maps each SegmentId → (tile_z, tile_x, tile_y, local_segment_index_within_tile).
    pub seg_tile: HashMap<SegmentId, (u8, u32, u32, u32)>,
    /// Cross-tile restrictions waiting for their from/to segments to be loaded.
    pending_xrestr: Vec<PendingXRestr>,
}

impl Default for TileLoader {
    fn default() -> Self { Self::new() }
}

impl TileLoader {
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
            boundary_nodes: HashMap::new(),
            next_node_id: 0,
            next_seg_id: 0,
            seg_tile: HashMap::new(),
            pending_xrestr: Vec::new(),
        }
    }

    /// Parse one OLRL v2 tile payload and merge it into the graph.
    /// Cross-tile restrictions are collected and stitched immediately against already-loaded
    /// segments; any that reference not-yet-loaded segments stay pending and will be resolved
    /// when the next tile is loaded.
    pub fn load_tile(&mut self, bytes: &[u8]) -> Result<(), TileReadError> {
        parse_tile(
            bytes,
            &mut self.graph,
            &mut self.boundary_nodes,
            &mut self.next_node_id,
            &mut self.next_seg_id,
            &mut self.pending_xrestr,
        )?;
        self.stitch_cross_tile();
        Ok(())
    }

    /// Like `load_tile`, but also records the tile key and local index for each
    /// ingested segment so callers can map a `SegmentId` back to its tile origin.
    ///
    /// An empty `bytes` slice means the tile is not present in the archive.  The
    /// tile is still marked as loaded so A* does not keep requesting it — boundary
    /// nodes that home to this tile are treated as genuine dead ends.
    pub fn load_tile_at(&mut self, z: u8, x: u32, y: u32, bytes: &[u8]) -> Result<(), TileReadError> {
        if bytes.is_empty() {
            self.graph.mark_tile_loaded(z, x, y);
            return Ok(());
        }
        let first_seg = self.next_seg_id;
        self.load_tile(bytes)?;
        for local_idx in 0..(self.next_seg_id - first_seg) {
            self.seg_tile.insert(SegmentId(first_seg + local_idx), (z, x, y, local_idx));
        }
        self.graph.mark_tile_loaded(z, x, y);
        Ok(())
    }

    /// Resolve pending cross-tile restrictions against currently loaded segments.
    /// Called automatically after each `load_tile`; exposed publicly so callers can
    /// trigger an extra pass after on-demand tile fetches if needed.
    pub fn stitch_cross_tile(&mut self) {
        if self.pending_xrestr.is_empty() { return; }

        // Build stable_id → Vec<SegmentId> reverse map over all loaded segments.
        let mut by_stable: HashMap<[u8; 16], Vec<SegmentId>> = HashMap::new();
        for (&seg_id, seg) in &self.graph.segments {
            by_stable.entry(seg.stable_id).or_default().push(seg_id);
        }

        let pending = std::mem::take(&mut self.pending_xrestr);
        let mut still_pending = Vec::new();

        for p in pending {
            let froms = by_stable.get(&p.from_gers);
            let tos   = by_stable.get(&p.to_gers);

            match (froms, tos) {
                (Some(froms), Some(tos)) => {
                    // Register a restriction for every (from_seg, to_seg) pair that is
                    // incident to via_node.  With endpoint-based tile assignment the same
                    // physical segment may appear under two SegmentIds, so we cover all copies.
                    for &from_seg in froms {
                        let fs = match self.graph.segments.get(&from_seg) { Some(s) => s, None => continue };
                        if fs.start_node != p.via_node && fs.end_node != p.via_node { continue; }
                        for &to_seg in tos {
                            let ts = match self.graph.segments.get(&to_seg) { Some(s) => s, None => continue };
                            if ts.start_node != p.via_node && ts.end_node != p.via_node { continue; }
                            self.graph.add_restriction(TurnRestriction { from_seg, via_node: p.via_node, to_seg });
                        }
                    }
                }
                _ => still_pending.push(p),
            }
        }

        self.pending_xrestr = still_pending;
    }
}

// ── Parser ────────────────────────────────────────────────────────────────────

fn parse_tile(
    b: &[u8],
    graph: &mut Graph,
    boundary_nodes: &mut HashMap<[u8; 16], NodeId>,
    next_node: &mut u32,
    next_seg: &mut u32,
    pending_xrestr: &mut Vec<PendingXRestr>,
) -> Result<(), TileReadError> {
    require(b, 40)?;

    let magic: [u8; 4] = b[0..4].try_into().expect("slice is exactly 4 bytes");
    if &magic != b"OLRL" {
        return Err(TileReadError::BadMagic(magic));
    }
    if b[4] != 2 {
        return Err(TileReadError::UnsupportedVersion(b[4]));
    }

    let seg_count   = u32_le(b, 8)  as usize;
    let node_count  = u32_le(b, 12) as usize;
    let restr_count = u32_le(b, 16) as usize;
    let geom_count  = u32_le(b, 20) as usize;
    let xrestr_count= u32_le(b, 24) as usize;

    // Compute section offsets with overflow checks (counts come from untrusted tile data).
    let checked = (|| -> Option<(usize,usize,usize,usize,usize,usize,usize)> {
        let seg_off    = 40usize;
        let sid_off    = seg_off   .checked_add(seg_count   .checked_mul(32)?)?;
        let geom_off   = sid_off   .checked_add(seg_count   .checked_mul(16)?)?;
        let node_off   = geom_off  .checked_add(geom_count  .checked_mul(8)?)?;
        let restr_off  = node_off  .checked_add(node_count  .checked_mul(28)?)?;
        let xrestr_off = restr_off .checked_add(restr_count .checked_mul(16)?)?;
        let min_len    = xrestr_off.checked_add(xrestr_count.checked_mul(40)?)?;
        Some((seg_off, sid_off, geom_off, node_off, restr_off, xrestr_off, min_len))
    })().ok_or(TileReadError::TooShort { need: usize::MAX, have: b.len() })?;
    let (seg_off, sid_off, geom_off, node_off, restr_off, xrestr_off, min_len) = checked;

    require(b, min_len)?;

    // ── Geometry pool ────────────────────────────────────────────────────────
    let geom_pool: Vec<(f64, f64)> = (0..geom_count)
        .map(|i| {
            let o = geom_off + i * 8;
            let lon = i32_le(b, o)     as f64 / 1e7;
            let lat = i32_le(b, o + 4) as f64 / 1e7;
            (lon, lat)
        })
        .collect();

    // ── Node table ───────────────────────────────────────────────────────────
    let mut local_node: Vec<NodeId> = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let o = node_off + i * 28;
        let lon = i32_le(b, o)     as f64 / 1e7;
        let lat = i32_le(b, o + 4) as f64 / 1e7;
        let stable_id: [u8; 16] = b[o+8..o+24].try_into().expect("slice is exactly 16 bytes");
        let is_boundary = b[o + 24] & 0x01 != 0;

        // Register every node in the global dedup map, not just boundary-flagged ones.
        // With endpoint-based tile assignment a segment may appear in two tiles (one per
        // endpoint); the shared endpoint is flagged is_boundary=false in its home tile but
        // is_boundary=true in the other tile.  Without universal registration, the home-tile
        // occurrence gets a fresh NodeId that is never stored, so the foreign-tile lookup
        // produces a second, different NodeId — silently disconnecting the graph.
        let node_id = *boundary_nodes.entry(stable_id).or_insert_with(|| {
            let id = NodeId(*next_node);
            *next_node += 1;
            id
        });

        if !graph.nodes.contains_key(&node_id) {
            graph.add_node(NetworkNode { id: node_id, lon, lat, stable_id, is_boundary });
        }
        local_node.push(node_id);
    }

    // ── Segment array ────────────────────────────────────────────────────────
    let mut local_seg: Vec<SegmentId> = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let o = seg_off + i * 32;
        let start_local = u32_le(b, o)     as usize;
        let end_local   = u32_le(b, o + 4) as usize;
        let geom_idx    = u32_le(b, o + 8) as usize;
        let geom_len    = u16_le(b, o + 12) as usize;
        let length_cm   = u32_le(b, o + 14);
        let attrs       = b[o + 18];

        if start_local >= node_count { return Err(TileReadError::NodeIndexOob(start_local)); }
        if end_local   >= node_count { return Err(TileReadError::NodeIndexOob(end_local)); }
        if geom_idx + geom_len > geom_count {
            return Err(TileReadError::GeomOutOfRange { offset: geom_idx, len: geom_len, pool: geom_count });
        }
        if geom_len < 2 {
            return Err(TileReadError::GeomTooShort { index: i, len: geom_len });
        }

        let frc = attrs & 0x07;
        let fow = (attrs >> 3) & 0x07;
        let direction = match (attrs >> 6) & 0x03 {
            1 => Direction::Forward,
            2 => Direction::Backward,
            _ => Direction::Both,
        };
        let geometry = geom_pool[geom_idx..geom_idx + geom_len].to_vec();

        let stable_id: [u8; 16] = b[sid_off + i * 16..sid_off + i * 16 + 16]
            .try_into()
            .expect("slice is exactly 16 bytes");

        let seg_id = SegmentId(*next_seg);
        *next_seg += 1;
        local_seg.push(seg_id);

        graph.add_segment(NetworkSegment {
            id: seg_id,
            start_node: local_node[start_local],
            end_node:   local_node[end_local],
            geometry,
            length_m: length_cm as f64 / 100.0,
            frc,
            fow,
            direction,
            stable_id,
        });
    }

    // ── Intra-tile restrictions ───────────────────────────────────────────────
    for i in 0..restr_count {
        let o = restr_off + i * 16;
        let from = u32_le(b, o)     as usize;
        let via  = u32_le(b, o + 4) as usize;
        let to   = u32_le(b, o + 8) as usize;

        if from >= seg_count  { return Err(TileReadError::SegIndexOob(from)); }
        if to   >= seg_count  { return Err(TileReadError::SegIndexOob(to)); }
        if via  >= node_count { return Err(TileReadError::NodeIndexOob(via)); }

        graph.add_restriction(TurnRestriction {
            from_seg: local_seg[from],
            via_node: local_node[via],
            to_seg:   local_seg[to],
        });
    }

    // ── Cross-tile restriction table ─────────────────────────────────────────
    // via_node is always in this tile and can be resolved immediately.
    // from/to segments may be in adjacent tiles; they are resolved in stitch_cross_tile().
    for i in 0..xrestr_count {
        let o = xrestr_off + i * 40;
        let from_gers: [u8; 16]  = b[o..o+16].try_into().expect("slice is exactly 16 bytes");
        let via_local             = u32_le(b, o + 16) as usize;
        let to_gers: [u8; 16]    = b[o+20..o+36].try_into().expect("slice is exactly 16 bytes");
        // flags at o+36, _pad at o+37..40 — reserved, ignored.

        if via_local >= node_count { return Err(TileReadError::NodeIndexOob(via_local)); }
        pending_xrestr.push(PendingXRestr {
            from_gers,
            via_node: local_node[via_local],
            to_gers,
        });
    }

    Ok(())
}

// ── Byte helpers ──────────────────────────────────────────────────────────────

fn require(b: &[u8], n: usize) -> Result<(), TileReadError> {
    if b.len() < n { Err(TileReadError::TooShort { need: n, have: b.len() }) } else { Ok(()) }
}

fn u32_le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]])
}
fn i32_le(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]])
}
fn u16_le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o+1]])
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 2-segment, 3-node tile payload for testing.
    fn minimal_tile() -> Vec<u8> {
        let seg_count: u32 = 2;
        let node_count: u32 = 3;
        let restr_count: u32 = 0;
        let xrestr_count: u32 = 0;
        // geometry: 2 vertices per segment = 4 total
        let geom_count: u32 = 4;

        let mut buf = vec![0u8; 0];

        // Header (40 bytes)
        buf.extend_from_slice(b"OLRL");     // magic
        buf.push(2);                        // version
        buf.push(0);                        // flags
        buf.extend_from_slice(&[0u8; 2]);   // pad
        buf.extend_from_slice(&seg_count.to_le_bytes());
        buf.extend_from_slice(&node_count.to_le_bytes());
        buf.extend_from_slice(&restr_count.to_le_bytes());
        buf.extend_from_slice(&geom_count.to_le_bytes());
        buf.extend_from_slice(&xrestr_count.to_le_bytes());
        buf.extend_from_slice(&[0u8; 12]); // reserved
        assert_eq!(buf.len(), 40);

        // Segment array (2 × 32 bytes)
        // Seg 0: nodes 0→1, geom [0,2), len 100 m (10000 cm), FRC=3, FOW=3, Both
        let attrs0: u8 = 3 | (3 << 3) | (0 << 6); // frc=3, fow=3, dir=Both
        let mut seg0 = [0u8; 32];
        seg0[0..4].copy_from_slice(&0u32.to_le_bytes());  // start_node local 0
        seg0[4..8].copy_from_slice(&1u32.to_le_bytes());  // end_node local 1
        seg0[8..12].copy_from_slice(&0u32.to_le_bytes()); // geom_offset 0
        seg0[12..14].copy_from_slice(&2u16.to_le_bytes());// geom_len 2
        seg0[14..18].copy_from_slice(&10_000u32.to_le_bytes()); // 100m
        seg0[18] = attrs0;
        buf.extend_from_slice(&seg0);

        // Seg 1: nodes 1→2, geom [2,4), len 150 m (15000 cm), FRC=3, FOW=3, Forward
        let attrs1: u8 = 3 | (3 << 3) | (1 << 6);
        let mut seg1 = [0u8; 32];
        seg1[0..4].copy_from_slice(&1u32.to_le_bytes());
        seg1[4..8].copy_from_slice(&2u32.to_le_bytes());
        seg1[8..12].copy_from_slice(&2u32.to_le_bytes());
        seg1[12..14].copy_from_slice(&2u16.to_le_bytes());
        seg1[14..18].copy_from_slice(&15_000u32.to_le_bytes());
        seg1[18] = attrs1;
        buf.extend_from_slice(&seg1);

        // GERS-id table (2 × 16 bytes = 32 bytes), all zeros fine for test
        buf.extend_from_slice(&[0u8; 32]);

        // Geometry pool (4 × 8 bytes)
        // Vertex 0: lon=174.0 lat=-36.0 → 1740000000, -360000000
        let lon0: i32 = 1_740_000_000;
        let lat0: i32 = -360_000_000;
        let lon1: i32 = 1_740_010_000;
        let lat1: i32 = -360_000_000;
        let lon2: i32 = 1_740_010_000;
        let lat2: i32 = -360_010_000;
        let lon3: i32 = 1_740_020_000;
        let lat3: i32 = -360_010_000;
        for &(lon, lat) in &[(lon0,lat0),(lon1,lat1),(lon2,lat2),(lon3,lat3)] {
            buf.extend_from_slice(&lon.to_le_bytes());
            buf.extend_from_slice(&lat.to_le_bytes());
        }

        // Node table (3 × 28 bytes)
        // Each node must have a unique stable_id; all-zero IDs would merge under universal dedup.
        for (idx, (lon_e7, lat_e7)) in [(lon0,lat0),(lon1,lat1),(lon2,lat2)].iter().enumerate() {
            buf.extend_from_slice(&lon_e7.to_le_bytes());
            buf.extend_from_slice(&lat_e7.to_le_bytes());
            let mut sid = [0u8; 16];
            sid[0] = idx as u8 + 1; // node 0→0x01, node 1→0x02, node 2→0x03
            buf.extend_from_slice(&sid);
            buf.push(0); // flags: not boundary
            buf.extend_from_slice(&[0u8; 3]); // pad
        }

        buf
    }

    #[test]
    fn parse_minimal_tile() {
        let bytes = minimal_tile();
        let mut loader = TileLoader::new();
        loader.load_tile(&bytes).unwrap();
        let g = &loader.graph;
        assert_eq!(g.segments.len(), 2, "segment count");
        assert_eq!(g.nodes.len(), 3, "node count");
    }

    #[test]
    fn segment_lengths_correct() {
        let bytes = minimal_tile();
        let mut loader = TileLoader::new();
        loader.load_tile(&bytes).unwrap();
        let segs: Vec<_> = loader.graph.segments.values().collect();
        let lengths: std::collections::HashSet<u32> =
            segs.iter().map(|s| s.length_m as u32).collect();
        assert!(lengths.contains(&100), "100 m segment");
        assert!(lengths.contains(&150), "150 m segment");
    }

    #[test]
    fn direction_decoded_correctly() {
        let bytes = minimal_tile();
        let mut loader = TileLoader::new();
        loader.load_tile(&bytes).unwrap();
        let segs: Vec<_> = loader.graph.segments.values().collect();
        let dirs: Vec<Direction> = segs.iter().map(|s| s.direction).collect();
        assert!(dirs.contains(&Direction::Both));
        assert!(dirs.contains(&Direction::Forward));
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = minimal_tile();
        bytes[0] = b'X';
        let mut loader = TileLoader::new();
        assert!(matches!(loader.load_tile(&bytes), Err(TileReadError::BadMagic(_))));
    }

    #[test]
    fn wrong_version_rejected() {
        let mut bytes = minimal_tile();
        bytes[4] = 1;
        let mut loader = TileLoader::new();
        assert!(matches!(loader.load_tile(&bytes), Err(TileReadError::UnsupportedVersion(1))));
    }

    // Header=40, segs=2*32=64, stable-id table=2*16=32, geom=4*8=32 → node_off=168
    const NODE_OFF: usize = 40 + 2*32 + 2*16 + 4*8;

    fn set_node_stable_id(tile: &mut Vec<u8>, node_idx: usize, id: [u8; 16]) {
        let o = NODE_OFF + node_idx * 28;
        tile[o+8..o+24].copy_from_slice(&id);
    }

    fn set_node_boundary(tile: &mut Vec<u8>, node_idx: usize, boundary: bool) {
        tile[NODE_OFF + node_idx * 28 + 24] = u8::from(boundary);
    }

    /// Both endpoints flagged is_boundary — the original stitching case.
    #[test]
    fn boundary_nodes_stitched_across_tiles() {
        let mut tile1 = minimal_tile();
        let mut tile2 = minimal_tile();

        let shared_id = [0xAB; 16];

        // Shared node: node2 of tile1 (boundary) = node0 of tile2 (boundary).
        set_node_stable_id(&mut tile1, 2, shared_id);
        set_node_boundary(&mut tile1, 2, true);
        set_node_stable_id(&mut tile2, 0, shared_id);
        set_node_boundary(&mut tile2, 0, true);

        // Give tile2's non-shared nodes IDs that don't collide with tile1's (0x01, 0x02).
        let mut id21 = [0u8; 16]; id21[0] = 0x21;
        let mut id22 = [0u8; 16]; id22[0] = 0x22;
        set_node_stable_id(&mut tile2, 1, id21);
        set_node_stable_id(&mut tile2, 2, id22);

        let mut loader = TileLoader::new();
        loader.load_tile(&tile1).unwrap();
        loader.load_tile(&tile2).unwrap();

        // tile1: 3 nodes (0x01, 0x02, 0xAB); tile2: (0xAB, 0x21, 0x22) → 5 unique.
        assert_eq!(loader.graph.nodes.len(), 5, "boundary node stitched: 3+3-1=5");
    }

    /// The endpoint-based-assignment bug: tile1 carries the node as non-boundary (home tile),
    /// tile2 carries it as boundary (foreign tile).  Without universal dedup, tile1's home
    /// occurrence was never registered, so tile2's boundary lookup created a second NodeId.
    #[test]
    fn home_tile_node_stitches_with_foreign_occurrence() {
        let mut tile1 = minimal_tile();
        let mut tile2 = minimal_tile();

        let shared_id = [0xCD; 16];

        // Node2 of tile1: NOT boundary (this is its home tile).
        set_node_stable_id(&mut tile1, 2, shared_id);
        set_node_boundary(&mut tile1, 2, false);

        // Node0 of tile2: boundary (home tile is tile1).
        set_node_stable_id(&mut tile2, 0, shared_id);
        set_node_boundary(&mut tile2, 0, true);

        let mut id21 = [0u8; 16]; id21[0] = 0x21;
        let mut id22 = [0u8; 16]; id22[0] = 0x22;
        set_node_stable_id(&mut tile2, 1, id21);
        set_node_stable_id(&mut tile2, 2, id22);

        let mut loader = TileLoader::new();
        loader.load_tile(&tile1).unwrap();
        loader.load_tile(&tile2).unwrap();

        // Same expected result: 5 unique nodes.
        assert_eq!(loader.graph.nodes.len(), 5,
            "home-tile non-boundary node must stitch with foreign-tile boundary occurrence");

        // The shared node must have exactly one NodeId across both tiles.
        let shared_nodes: Vec<_> = loader.graph.nodes.values()
            .filter(|n| n.stable_id == shared_id)
            .collect();
        assert_eq!(shared_nodes.len(), 1, "shared node should appear exactly once in the graph");
    }

    /// Build a tile that has one cross-tile restriction referencing segments in another tile.
    ///
    /// Tile layout:
    ///   Nodes: A (local 0), B (local 1, boundary / shared via), C (local 2)
    ///   Segments: seg_AB (from=A to=B), seg_BC (from=B to=C)
    ///   Cross-tile restriction: from=FROM_GERS (segment in other tile), via=B, to=TO_GERS
    ///
    /// The restriction means: arriving at B via the foreign segment is prohibited from
    /// turning onto seg_BC.  After loading a second tile that contains FROM_GERS and
    /// TO_GERS segments incident to the shared B node, the restriction should be stitched.
    #[test]
    fn cross_tile_restriction_stitched() {
        // ── Tile 1: has the via-node and the cross-tile restriction entry ────────
        // We build it manually rather than patching minimal_tile(), because we need a
        // cross-tile restriction table (xrestriction_count = 1).

        let from_gers_id: [u8; 16] = { let mut v = [0u8;16]; v[0] = 0xF1; v };
        let to_gers_id:   [u8; 16] = { let mut v = [0u8;16]; v[0] = 0xF2; v };
        let shared_node_id: [u8; 16] = { let mut v = [0u8;16]; v[0] = 0xBB; v };

        let seg_count:   u32 = 2;
        let node_count:  u32 = 3;
        let restr_count: u32 = 0;
        let xrestr_count:u32 = 1;
        let geom_count:  u32 = 6; // 2 verts × 3 segs, but only 2 segs in tile1

        let mut t1: Vec<u8> = Vec::new();
        // Header
        t1.extend_from_slice(b"OLRL");
        t1.push(2); t1.push(0); t1.extend_from_slice(&[0u8;2]);
        t1.extend_from_slice(&seg_count.to_le_bytes());
        t1.extend_from_slice(&node_count.to_le_bytes());
        t1.extend_from_slice(&restr_count.to_le_bytes());
        t1.extend_from_slice(&geom_count.to_le_bytes());
        t1.extend_from_slice(&xrestr_count.to_le_bytes());
        t1.extend_from_slice(&[0u8; 12]);
        assert_eq!(t1.len(), 40);

        // 2 segments × 32 bytes
        for (start, end, geom_off, geom_len) in [(0u32,1u32,0u32,2u16),(1u32,2u32,2u32,2u16)] {
            let mut s = [0u8; 32];
            s[0..4].copy_from_slice(&start.to_le_bytes());
            s[4..8].copy_from_slice(&end.to_le_bytes());
            s[8..12].copy_from_slice(&geom_off.to_le_bytes());
            s[12..14].copy_from_slice(&geom_len.to_le_bytes());
            s[14..18].copy_from_slice(&10_000u32.to_le_bytes()); // 100m
            s[18] = 3 | (3 << 3); // frc=3, fow=3, Both
            t1.extend_from_slice(&s);
        }

        // Stable-id table for 2 segments (unique IDs)
        let mut seg0_id = [0u8;16]; seg0_id[0] = 0xA0;
        let mut seg1_id = [0u8;16]; seg1_id[0] = 0xA1;
        t1.extend_from_slice(&seg0_id);
        t1.extend_from_slice(&seg1_id);

        // Geometry pool: 6 vertices (3 pairs)
        for lon_e7 in [1_740_000_000i32, 1_740_010_000, 1_740_010_000,
                        1_740_020_000, 1_740_020_000, 1_740_030_000] {
            t1.extend_from_slice(&lon_e7.to_le_bytes());
            t1.extend_from_slice(&(-360_000_000i32).to_le_bytes());
        }

        // Node table: 3 nodes × 28 bytes
        // node 0: regular
        // node 1: boundary (shared via node)
        // node 2: regular
        let nodes = [
            ([0u8;16].iter().cloned().enumerate().map(|(i,_)| if i==0 {0x01} else {0}).collect::<Vec<_>>(), false),
            (shared_node_id.to_vec(), true),
            ([0u8;16].iter().cloned().enumerate().map(|(i,_)| if i==0 {0x03} else {0}).collect::<Vec<_>>(), false),
        ];
        for (i, (sid, is_b)) in nodes.iter().enumerate() {
            let lon = 1_740_000_000i32 + (i as i32) * 10_000_000;
            t1.extend_from_slice(&lon.to_le_bytes());
            t1.extend_from_slice(&(-360_000_000i32).to_le_bytes());
            t1.extend_from_slice(sid.as_slice());
            t1.push(u8::from(*is_b));
            t1.extend_from_slice(&[0u8;3]);
        }

        // No intra-tile restrictions.

        // Cross-tile restriction: from=from_gers_id, via=node_local_1, to=to_gers_id
        t1.extend_from_slice(&from_gers_id);
        t1.extend_from_slice(&1u32.to_le_bytes()); // via_node_local = 1
        t1.extend_from_slice(&to_gers_id);
        t1.push(0); // flags
        t1.extend_from_slice(&[0u8;3]);

        // ── Tile 2: contains the from and to segments referenced by the restriction ─
        // Two segments: from_seg (end_node = shared B), to_seg (start_node = shared B)
        let seg_count2:   u32 = 2;
        let node_count2:  u32 = 3; // X → B → Y
        let geom_count2:  u32 = 4;

        let mut t2: Vec<u8> = Vec::new();
        t2.extend_from_slice(b"OLRL");
        t2.push(2); t2.push(0); t2.extend_from_slice(&[0u8;2]);
        t2.extend_from_slice(&seg_count2.to_le_bytes());
        t2.extend_from_slice(&node_count2.to_le_bytes());
        t2.extend_from_slice(&0u32.to_le_bytes()); // restr
        t2.extend_from_slice(&geom_count2.to_le_bytes());
        t2.extend_from_slice(&0u32.to_le_bytes()); // xrestr
        t2.extend_from_slice(&[0u8; 12]);

        // seg X→B (from_seg): start=node0(X), end=node1(B)
        // seg B→Y (to_seg):   start=node1(B), end=node2(Y)
        for (start, end, geom_off) in [(0u32,1u32,0u32),(1u32,2u32,2u32)] {
            let mut s = [0u8; 32];
            s[0..4].copy_from_slice(&start.to_le_bytes());
            s[4..8].copy_from_slice(&end.to_le_bytes());
            s[8..12].copy_from_slice(&geom_off.to_le_bytes());
            s[12..14].copy_from_slice(&2u16.to_le_bytes());
            s[14..18].copy_from_slice(&10_000u32.to_le_bytes());
            s[18] = 3 | (3 << 3);
            t2.extend_from_slice(&s);
        }

        // Stable IDs for the 2 segments in tile2: from_gers_id and to_gers_id
        t2.extend_from_slice(&from_gers_id);
        t2.extend_from_slice(&to_gers_id);

        // Geometry: 4 vertices
        for lon_e7 in [1_750_000_000i32, 1_750_010_000, 1_750_010_000, 1_750_020_000] {
            t2.extend_from_slice(&lon_e7.to_le_bytes());
            t2.extend_from_slice(&(-360_000_000i32).to_le_bytes());
        }

        // Nodes in tile2: X (0), B (1, boundary, shared stable_id), Y (2)
        let mut x_id = [0u8;16]; x_id[0] = 0xE1;
        let mut y_id = [0u8;16]; y_id[0] = 0xE2;
        for (sid, is_b) in [(&x_id[..], false), (&shared_node_id[..], true), (&y_id[..], false)] {
            t2.extend_from_slice(&1_750_000_000i32.to_le_bytes());
            t2.extend_from_slice(&(-360_000_000i32).to_le_bytes());
            t2.extend_from_slice(sid);
            t2.push(u8::from(is_b));
            t2.extend_from_slice(&[0u8;3]);
        }

        // ── Load both tiles and verify ────────────────────────────────────────
        let mut loader = TileLoader::new();
        loader.load_tile(&t1).unwrap();

        // After tile 1 only: restriction is pending (from/to segs not yet loaded).
        assert_eq!(loader.pending_xrestr.len(), 1, "restriction should be pending");
        assert_eq!(loader.graph.restrictions_count(), 0, "not yet stitched");

        loader.load_tile(&t2).unwrap();

        // After tile 2: both from_gers and to_gers are loaded; restriction should be stitched.
        assert_eq!(loader.pending_xrestr.len(), 0, "no remaining pending restrictions");
        assert!(loader.graph.restrictions_count() > 0, "restriction should have been stitched");
    }
}
