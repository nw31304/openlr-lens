/// Decode-time configuration. Permissive defaults; tunable via UI sliders.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecodeParams {
    /// Candidate search radius in meters.
    pub candidate_search_radius_m: f64,
    /// Map-divergence bearing tolerance τ (degrees). Combined with encoding interval.
    pub bearing_tolerance_deg: f64,
    /// DNP tolerance δ as a fraction of path length. Combined with v3 bucket half-width.
    pub dnp_tolerance_pct: f64,
    /// Soft penalty weight for FRC mismatch (per FRC step).
    pub frc_penalty_per_step: f64,
    /// Soft penalty weight for FOW mismatch.
    pub fow_penalty: f64,
    /// A* expansion cap: max ratio of expanded path length to DNP.
    pub max_path_search_factor: f64,
}

impl Default for DecodeParams {
    fn default() -> Self {
        Self {
            candidate_search_radius_m: 100.0,
            bearing_tolerance_deg:     30.0,
            dnp_tolerance_pct:         0.25,
            frc_penalty_per_step:      25.0,
            fow_penalty:               25.0,
            max_path_search_factor:    5.0,
        }
    }
}
