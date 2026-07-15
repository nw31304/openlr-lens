//! Line location encoding: the full Table 54 pipeline — expand to valid
//! nodes, sweep coverage into legs, compute attributes, assemble offsets.

use openlr_codec::{CircularInterval, LinearInterval, LocationReference, Lrp};
use openlr_graph::{Graph, NodeId, SegmentId};

use crate::{attributes, coverage, expansion, EncodeError};

/// Rule-1: maximum distance between two consecutive LRPs, meters.
pub const MAX_LEG_M: f64 = 15_000.0;

/// A concrete path on the road network — e.g. Layer 1's waypoint-routing
/// output — plus where within the first/last segment the user's *true*
/// intended start/end point falls (it may be mid-segment, not at a node).
pub struct LineLocationInput {
    pub path: Vec<SegmentId>,
    /// The node `path[0]` is entered from (disambiguates travel direction —
    /// a bare segment list alone doesn't say which end is "first").
    pub start_node: NodeId,
    /// Distance from `start_node` to the true intended start, meters.
    pub start_offset_m: f64,
    /// Distance from the true intended end to the path's exit node, meters.
    pub end_offset_m: f64,
}

pub fn encode_line(graph: &Graph, input: &LineLocationInput) -> Result<LocationReference, EncodeError> {
    if input.path.is_empty() {
        return Err(EncodeError::EmptyPath);
    }
    let first_seg_id = input.path[0];
    let last_seg_id = *input.path.last().unwrap();

    let end_node = coverage::trace_end_node(graph, input.start_node, &input.path)
        .ok_or(EncodeError::Disconnected { index: 0 })?;

    // Step 2: expand both ends outward to valid nodes (Rule-4), tracking the
    // segments walked so they can be spliced into the full path.
    let start_exp = expansion::expand_to_valid_node(graph, input.start_node, first_seg_id, MAX_LEG_M);
    let end_exp = expansion::expand_to_valid_node(graph, end_node, last_seg_id, MAX_LEG_M);

    let mut full_path = Vec::with_capacity(start_exp.segments.len() + input.path.len() + end_exp.segments.len());
    full_path.extend(start_exp.segments.iter().rev().copied());
    full_path.extend(input.path.iter().copied());
    full_path.extend(end_exp.segments.iter().copied());

    let expanded_start_node = start_exp.node;
    let expanded_end_node = end_exp.node;

    // Figure 27: final offset = original within-leg offset + expansion distance.
    let pos_offset_m = input.start_offset_m + start_exp.distance_m;
    let neg_offset_m = input.end_offset_m + end_exp.distance_m;

    // Steps 3-6: sweep the expanded path into legs.
    let legs = coverage::sweep_coverage(graph, &full_path, expanded_start_node, expanded_end_node, MAX_LEG_M)?;
    if legs.is_empty() {
        return Err(EncodeError::EmptyPath);
    }

    // Step 7: attributes per LRP.
    let mut lrps = Vec::with_capacity(legs.len() + 1);
    for leg in &legs {
        let attrs = attributes::leg_attributes(graph, leg.start_node, &leg.segments)
            .ok_or(EncodeError::UnknownSegment(leg.segments[0]))?;
        lrps.push(Lrp {
            coord: node_coord(graph, leg.start_node)?,
            bearing: CircularInterval::point(attrs.bearing_deg),
            frc: attrs.frc,
            fow: attrs.fow,
            lfrcnp: Some(attrs.lfrcnp),
            dnp: Some(LinearInterval::point(attrs.dnp_m)),
            pos_offset: None,
            neg_offset: None,
            pos_offset_raw: None,
            neg_offset_raw: None,
        });
    }

    let last_leg_seg = *legs.last().unwrap().segments.last().unwrap();
    let last_seg = graph.segments.get(&last_leg_seg).ok_or(EncodeError::UnknownSegment(last_leg_seg))?;
    let last_bearing = attributes::last_lrp_bearing_deg(graph, expanded_end_node, last_leg_seg)
        .ok_or(EncodeError::UnknownSegment(last_leg_seg))?;
    lrps.push(Lrp {
        coord: node_coord(graph, expanded_end_node)?,
        bearing: CircularInterval::point(last_bearing),
        frc: last_seg.frc,
        fow: last_seg.fow,
        lfrcnp: None,
        dnp: None,
        pos_offset: None,
        neg_offset: None,
        pos_offset_raw: None,
        neg_offset_raw: None,
    });

    // Offsets, bounded per Rule-5 (must be strictly less than the bracketing
    // leg). v1 scope: error out rather than the spec's full cascade of
    // dropping the boundary LRP and re-deriving against the next leg.
    let first_leg_m = lrps[0].dnp.unwrap().lb;
    if pos_offset_m > 0.0 {
        if pos_offset_m >= first_leg_m {
            return Err(EncodeError::Codec(openlr_codec::EncodeError::OffsetExceedsLeg {
                offset_m: pos_offset_m, leg_m: first_leg_m,
            }));
        }
        lrps[0].pos_offset = Some(LinearInterval::point(pos_offset_m));
    }
    let last_leg_m = lrps[lrps.len() - 2].dnp.unwrap().lb;
    if neg_offset_m > 0.0 {
        if neg_offset_m >= last_leg_m {
            return Err(EncodeError::Codec(openlr_codec::EncodeError::OffsetExceedsLeg {
                offset_m: neg_offset_m, leg_m: last_leg_m,
            }));
        }
        let n = lrps.len() - 1;
        lrps[n].neg_offset = Some(LinearInterval::point(neg_offset_m));
    }

    Ok(LocationReference::Line { lrps })
}

fn node_coord(graph: &Graph, node: NodeId) -> Result<(f64, f64), EncodeError> {
    graph.nodes.get(&node).map(|n| (n.lon, n.lat)).ok_or(EncodeError::UnknownNode(node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlr_graph::{Direction, NetworkNode, NetworkSegment};

    fn node(id: u32, lon: f64, lat: f64) -> NetworkNode {
        NetworkNode { id: NodeId(id), lon, lat, stable_id: String::new(), is_boundary: false }
    }
    fn seg(id: u32, s: u32, e: u32, len_deg: f64) -> NetworkSegment {
        let lon0 = s as f64 * 0.001;
        NetworkSegment {
            id: SegmentId(id),
            start_node: NodeId(s),
            end_node: NodeId(e),
            geometry: vec![(lon0, 0.0), (lon0 + len_deg, 0.0)],
            length_m: len_deg * 111_000.0, // rough, matches the geometry's own extent closely enough for tests
            frc: 3, fow: 2,
            direction: Direction::Both,
            stable_id: String::new(),
        }
    }

    /// A simple 3-node straight line; both ends are dead ends (degree 1),
    /// already valid per Rule-4 — no expansion, no intermediates.
    #[test]
    fn simple_straight_line_encodes_two_lrps() {
        let mut g = Graph::new();
        for i in 0..=2u32 { g.add_node(node(i, i as f64 * 0.001, 0.0)); }
        g.add_segment(seg(1, 0, 1, 0.001));
        g.add_segment(seg(2, 1, 2, 0.001));
        // Nodes 0 and 2 each touch exactly one segment — dead ends, already
        // valid (Rule-4 only invalidates pass-throughs, not dead ends).

        let input = LineLocationInput {
            path: vec![SegmentId(1), SegmentId(2)],
            start_node: NodeId(0),
            start_offset_m: 0.0,
            end_offset_m: 0.0,
        };
        let loc = encode_line(&g, &input).unwrap();
        let lrps = loc.lrps().unwrap();
        assert_eq!(lrps.len(), 2);
        assert!(lrps[0].dnp.is_some());
        assert!(lrps[1].dnp.is_none());
        assert!(lrps[0].pos_offset.is_none());
        assert!(lrps[1].neg_offset.is_none());
    }

    #[test]
    fn nonzero_offsets_are_carried_through() {
        let mut g = Graph::new();
        for i in 0..=2u32 { g.add_node(node(i, i as f64 * 0.001, 0.0)); }
        g.add_segment(seg(1, 0, 1, 0.001));
        g.add_segment(seg(2, 1, 2, 0.001));

        let input = LineLocationInput {
            path: vec![SegmentId(1), SegmentId(2)],
            start_node: NodeId(0),
            start_offset_m: 20.0,
            end_offset_m: 15.0,
        };
        let loc = encode_line(&g, &input).unwrap();
        let lrps = loc.lrps().unwrap();
        assert!((lrps[0].pos_offset.unwrap().lb - 20.0).abs() < 1e-6);
        assert!((lrps[1].neg_offset.unwrap().lb - 15.0).abs() < 1e-6);
    }

    /// End-to-end through openlr_codec: encode, then serialize to both
    /// physical formats, and confirm each round-trips via its own decoder.
    #[test]
    fn encoded_line_round_trips_through_both_physical_formats() {
        let mut g = Graph::new();
        for i in 0..=2u32 { g.add_node(node(i, i as f64 * 0.001, 0.0)); }
        g.add_segment(seg(1, 0, 1, 0.001));
        g.add_segment(seg(2, 1, 2, 0.001));

        let input = LineLocationInput {
            path: vec![SegmentId(1), SegmentId(2)],
            start_node: NodeId(0),
            start_offset_m: 0.0,
            end_offset_m: 0.0,
        };
        let loc = encode_line(&g, &input).unwrap();

        let v3 = openlr_codec::encode_v3_base64(&loc).unwrap();
        let redecoded_v3 = openlr_codec::decode_v3_base64(&v3).unwrap();
        assert_eq!(redecoded_v3.lrps().unwrap().len(), 2);

        let tpeg = openlr_codec::encode_tpeg_hex(&loc).unwrap();
        let redecoded_tpeg = openlr_codec::decode_tpeg_hex(&tpeg).unwrap();
        assert_eq!(redecoded_tpeg.lrps().unwrap().len(), 2);
    }
}
