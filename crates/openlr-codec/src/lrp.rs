use crate::{CircularInterval, LinearInterval};

/// A Location Reference Point in the unified interval model.
/// Format-specific bit-unpacking (v3 / TPEG) lives in the decoder modules;
/// everything downstream of the decoder is format-agnostic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Lrp {
    /// WGS84 (longitude, latitude).
    pub coord: (f64, f64),
    /// Forward bearing from the projection point (or backward for the last LRP).
    pub bearing: CircularInterval,
    pub frc: u8,
    pub fow: u8,
    /// Lowest FRC permitted on the path to the next LRP. None on the last LRP.
    pub lfrcnp: Option<u8>,
    /// Distance to next point in meters. None on the last LRP.
    pub dnp: Option<LinearInterval>,
    /// Positive offset (meters from start of first edge). None if absent.
    pub pos_offset: Option<LinearInterval>,
    /// Negative offset (meters from end of last edge). None if absent.
    pub neg_offset: Option<LinearInterval>,
}

/// A decoded location reference: an ordered sequence of LRPs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LocationReference {
    pub lrps: Vec<Lrp>,
}
