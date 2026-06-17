use openlr_codec::interval::LinearInterval;
use openlr_graph::{Graph, SegmentId};

use crate::params::DecodeParams;
use crate::trace::{DecodeEvent, DecodeTrace, RoutingFailure};

/// Validate that `path_length_m` falls within the hard DNP window.
///
/// Hard window = `[dnp.lb − δ, dnp.ub + δ]` where `δ = pct × path_length`.
/// For v3, `dnp` is the full bucket interval `[d×58.6, (d+1)×58.6]`.
/// For TPEG, `dnp` is a point interval (`lb == ub`).
/// Returns `Ok(())` on pass, `Err(RoutingFailure)` on fail.
pub fn validate_dnp(
    leg: usize,
    path_length_m: f64,
    dnp: LinearInterval,
    params: &DecodeParams,
    trace: &mut DecodeTrace,
) -> Result<(), RoutingFailure> {
    let delta = path_length_m * params.dnp_tolerance_pct;
    let window = dnp.widen(delta);

    let passed = window.contains(path_length_m);

    trace.push_summary(DecodeEvent::DnpChecked {
        leg,
        interval: window,
        actual_m: path_length_m,
        passed,
    });

    if passed {
        Ok(())
    } else {
        Err(RoutingFailure::DnpOutOfRange { actual_m: path_length_m, window })
    }
}

/// Apply a positive (head) or negative (tail) offset to the assembled path.
///
/// Returns the trimmed offset in meters (the point along the first/last segment
/// where the decoded location begins/ends).
///
/// For a positive offset: the decoded location starts `trim_m` into the first segment.
/// For a negative offset: the decoded location ends `trim_m` before the end of the last segment.
pub fn apply_offset(
    is_positive: bool,
    offset_interval: LinearInterval,
    _path: &[SegmentId],
    _graph: &Graph,
    trace: &mut DecodeTrace,
) -> f64 {
    // Use the midpoint of the offset interval as the trim point.
    let trim_m = (offset_interval.lb + offset_interval.ub) / 2.0;

    trace.push_summary(DecodeEvent::OffsetApplied {
        is_positive,
        interval: offset_interval,
        trim_m,
    });

    trim_m
}

/// Compute the total length of a path (sum of segment lengths).
pub fn path_length_m(segments: &[SegmentId], graph: &Graph) -> f64 {
    segments
        .iter()
        .filter_map(|id| graph.segments.get(id))
        .map(|s| s.length_m)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::DecodeParams;
    use crate::trace::DecodeTrace;

    fn trace() -> DecodeTrace {
        DecodeTrace::new(DecodeParams::default())
    }

    #[test]
    fn dnp_pass_inside_window() {
        let dnp = LinearInterval { lb: 500.0, ub: 558.6 };
        let mut t = trace();
        let r = validate_dnp(0, 530.0, dnp, &DecodeParams::default(), &mut t);
        assert!(r.is_ok());
    }

    #[test]
    fn dnp_fail_too_short() {
        let dnp = LinearInterval { lb: 500.0, ub: 558.6 };
        let mut t = trace();
        // delta = 0.25 * 100 = 25 m → window = [475, 583.6]
        // 100 m is outside
        let r = validate_dnp(0, 100.0, dnp, &DecodeParams::default(), &mut t);
        assert!(r.is_err());
    }

    #[test]
    fn dnp_pass_at_boundary() {
        // TPEG: exact DNP (LB == UB), only pct tolerance applies
        let dnp = LinearInterval::point(500.0);
        let mut t = trace();
        // delta = 0.25 * 600 = 150 → window [350, 650]
        let r = validate_dnp(0, 600.0, dnp, &DecodeParams::default(), &mut t);
        assert!(r.is_ok());
    }

    #[test]
    fn dnp_v3_bucket_zero_direct_match() {
        // v3 bucket 0 → [0, 58.6 m]; path_length = 0 (same-segment match)
        let dnp = LinearInterval { lb: 0.0, ub: 58.6 };
        let mut t = trace();
        // delta = 0.25 * 0 = 0 → window = [0.0, 58.6]
        let r = validate_dnp(0, 0.0, dnp, &DecodeParams::default(), &mut t);
        assert!(r.is_ok());
    }
}
