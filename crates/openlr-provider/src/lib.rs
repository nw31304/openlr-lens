use openlr_graph::{NetworkSegment, SegmentId};

/// A coarse spatial result returned by the provider before exact engine filtering.
pub struct SpatialMapChunk {
    pub segments: Vec<NetworkSegment>,
}

/// All map access goes through this trait so the engine is storage-agnostic.
/// The async signatures describe the logical contract; the WASM realization
/// fulfills them via the JS-driven tile-request/resume protocol (see CLAUDE.md §3).
pub trait OpenLrDataProvider {
    type Error;

    /// Segments whose geometry comes within `radius_m` of (lon, lat).
    /// Coarse bbox prune in the provider; exact filtering in the engine.
    fn segments_near(
        &self,
        lon: f64,
        lat: f64,
        radius_m: f64,
    ) -> Result<SpatialMapChunk, Self::Error>;

    /// Resolve a segment by stable id (cross-tile expansion / boundary stitching).
    fn segment_by_id(&self, id: SegmentId) -> Result<Option<NetworkSegment>, Self::Error>;
}
