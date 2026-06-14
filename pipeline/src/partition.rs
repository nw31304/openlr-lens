use tracing::info;

use crate::extent::Bbox;

/// Fraction of available RAM used as the processing budget per partition.
const RAM_UTILISATION: f64 = 0.75;

/// Conservative peak-RAM estimate per Overture segment through the full pipeline.
/// Covers: AdaptedSegment, ~2.5 SplitEdges per segment, QuantizedEdge (with geometry),
/// QuantizedNode (deduped), tile payload buffers, HashMap overhead.
/// Intentionally high — over-partitioning is always safe; OOM is not.
pub const DEFAULT_BYTES_PER_SEGMENT: u64 = 1_000;

/// Conservative estimate of total segments in the global Overture transportation dataset.
/// Used to scale per-bbox estimates by geographic area fraction.
const GLOBAL_SEGMENT_ESTIMATE: u64 = 300_000_000;

/// Whole-world bbox (Web Mercator latitude limits).
const WORLD_BBOX: Bbox = Bbox { west: -180.0, south: -85.0511, east: 180.0, north: 85.0511 };

// ── RAM detection ─────────────────────────────────────────────────────────────

/// Returns available system RAM in bytes.
/// Falls back to 4 GiB if detection fails (conservative — will cause more partitions).
pub fn available_ram_bytes() -> u64 {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    let avail = sys.available_memory();
    if avail == 0 {
        info!("RAM detection returned 0, falling back to 4 GiB");
        4 * 1024 * 1024 * 1024
    } else {
        avail
    }
}

/// Returns the RAM budget for a single partition.
/// Uses `override_gb` if supplied, otherwise 75 % of `available`.
pub fn ram_budget_bytes(available: u64, override_gb: Option<f64>) -> u64 {
    match override_gb {
        Some(gb) => (gb * 1024.0 * 1024.0 * 1024.0) as u64,
        None     => (available as f64 * RAM_UTILISATION) as u64,
    }
}

// ── Segment / RAM estimation ──────────────────────────────────────────────────

/// Estimate peak RAM required to process `bbox` in one shot, using `bytes_per_segment`
/// as the per-segment constant (default: `DEFAULT_BYTES_PER_SEGMENT`).
pub fn estimate_ram_bytes(bbox: Option<Bbox>, bytes_per_segment: u64) -> u64 {
    let fraction = match bbox {
        None    => 1.0,
        Some(b) => bbox_area_fraction(b),
    };
    let segments = (GLOBAL_SEGMENT_ESTIMATE as f64 * fraction).ceil() as u64;
    segments.saturating_mul(bytes_per_segment)
}

/// Fraction of the world's total spherical surface area covered by `bbox`.
/// Uses the exact spherical integral: ∫∫ cos φ dφ dλ, normalised by 4π.
fn bbox_area_fraction(bbox: Bbox) -> f64 {
    use std::f64::consts::PI;
    let dlon       = (bbox.east  - bbox.west).abs().to_radians();
    let sin_north  = bbox.north.to_radians().sin();
    let sin_south  = bbox.south.to_radians().sin();
    let steradians = dlon * (sin_north - sin_south).abs();
    (steradians / (4.0 * PI)).clamp(0.0, 1.0)
}

// ── Partitioning ──────────────────────────────────────────────────────────────

/// Compute the list of non-overlapping bboxes to process for `bbox`.
///
/// Returns a single element when the estimated RAM fits within `ram_budget`;
/// otherwise recursively bisects until every piece fits.
pub fn compute_partitions(
    bbox:              Option<Bbox>,
    ram_budget:        u64,
    bytes_per_segment: u64,
) -> Vec<Bbox> {
    let root = bbox.unwrap_or(WORLD_BBOX);
    let mut out = Vec::new();
    bisect_recursive(root, ram_budget, bytes_per_segment, &mut out);
    out
}

fn bisect_recursive(bbox: Bbox, budget: u64, bps: u64, out: &mut Vec<Bbox>) {
    if estimate_ram_bytes(Some(bbox), bps) <= budget {
        out.push(bbox);
        return;
    }
    let [left, right] = bisect_bbox(bbox);
    bisect_recursive(left,  budget, bps, out);
    bisect_recursive(right, budget, bps, out);
}

/// Split a bbox along its longer geographic axis.
/// Longitude differences are weighted by cos(mid_lat) to account for Mercator compression
/// near the poles, so we always cut the larger real-world dimension.
fn bisect_bbox(bbox: Bbox) -> [Bbox; 2] {
    let mid_lat      = (bbox.south + bbox.north) / 2.0;
    let effective_ew = (bbox.east - bbox.west).abs() * mid_lat.to_radians().cos();
    let ns           = (bbox.north - bbox.south).abs();

    if effective_ew >= ns {
        // Split east–west
        let mid_lon = (bbox.west + bbox.east) / 2.0;
        [
            Bbox { west: bbox.west, south: bbox.south, east: mid_lon,  north: bbox.north },
            Bbox { west: mid_lon,   south: bbox.south, east: bbox.east, north: bbox.north },
        ]
    } else {
        // Split north–south
        [
            Bbox { west: bbox.west, south: bbox.south, east: bbox.east, north: mid_lat },
            Bbox { west: bbox.west, south: mid_lat,    east: bbox.east, north: bbox.north },
        ]
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_bbox_fraction_near_one() {
        // WORLD_BBOX uses Web Mercator latitude limits (±85.0511°), not true poles,
        // so the fraction is ~0.996 rather than 1.0.
        let f = bbox_area_fraction(WORLD_BBOX);
        assert!(f > 0.99 && f <= 1.0, "world fraction = {f}");
    }

    #[test]
    fn small_bbox_fraction_is_small() {
        // NZ bbox is roughly 1/500 of the world
        let nz = Bbox { west: 166.0, south: -47.5, east: 178.5, north: -34.0 };
        let f = bbox_area_fraction(nz);
        assert!(f < 0.01, "NZ fraction = {f}");
        assert!(f > 0.0, "NZ fraction should be positive");
    }

    #[test]
    fn single_partition_when_fits() {
        // Budget is huge → should return exactly 1 partition
        let nz = Some(Bbox { west: 166.0, south: -47.5, east: 178.5, north: -34.0 });
        let parts = compute_partitions(nz, u64::MAX, DEFAULT_BYTES_PER_SEGMENT);
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn partitions_when_does_not_fit() {
        // Budget is tiny → should split into multiple partitions
        let world = None;
        let parts = compute_partitions(world, 1_000, DEFAULT_BYTES_PER_SEGMENT);
        assert!(parts.len() > 1, "expected multiple partitions, got {}", parts.len());
    }

    #[test]
    fn partitions_are_non_overlapping_on_one_axis() {
        let world = Bbox { west: -180.0, south: -90.0, east: 180.0, north: 90.0 };
        let [left, right] = bisect_bbox(world);
        // Split east–west at equator lon = 0
        assert!((left.east - right.west).abs() < 1e-9, "left.east should == right.west");
        assert_eq!(left.west, -180.0);
        assert_eq!(right.east, 180.0);
    }

    #[test]
    fn partition_bbox_coverage_equals_input() {
        // All partition bboxes together should cover the whole world bbox.
        // Check total longitude extent sums to 360 (valid for east-west splits).
        let world = None;
        let parts = compute_partitions(world, 100_000_000_000, DEFAULT_BYTES_PER_SEGMENT);
        // If single partition, trivially OK.
        if parts.len() == 1 {
            return;
        }
        // All partitions should share the same lat extent (simple east-west split).
        let total_lon: f64 = parts.iter().map(|b| b.east - b.west).sum();
        assert!((total_lon - 360.0).abs() < 1e-6, "total lon = {total_lon}");
    }

    #[test]
    fn ram_budget_override() {
        let budget = ram_budget_bytes(32 * 1024 * 1024 * 1024, Some(24.0));
        let expected = (24.0 * 1024.0 * 1024.0 * 1024.0) as u64;
        assert_eq!(budget, expected);
    }

    #[test]
    fn ram_budget_auto_is_75_percent() {
        let avail = 32u64 * 1024 * 1024 * 1024;
        let budget = ram_budget_bytes(avail, None);
        let expected = (avail as f64 * 0.75) as u64;
        assert_eq!(budget, expected);
    }
}
