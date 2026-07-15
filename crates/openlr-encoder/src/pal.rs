//! PointAlongLine (PAL) encoding: Table 56's 3-step pipeline. Always exactly
//! 2 LRPs bracketing a single line — no coverage sweep, since there's only
//! ever one line to encode.

use openlr_codec::{CircularInterval, LinearInterval, LocationReference, Lrp, Orientation, SideOfRoad};
use openlr_graph::{Graph, NodeId, SegmentId};

use crate::{attributes, expansion, line::MAX_LEG_M, EncodeError};

pub struct PalLocationInput {
    pub line: SegmentId,
    /// The endpoint of `line` on the "first LRP" side.
    pub start_node: NodeId,
    /// Distance from `start_node` to the actual point, meters.
    pub point_offset_m: f64,
    pub orientation: Orientation,
    pub side_of_road: SideOfRoad,
}

pub fn encode_pal(graph: &Graph, input: &PalLocationInput) -> Result<LocationReference, EncodeError> {
    let seg = graph.segments.get(&input.line).ok_or(EncodeError::UnknownSegment(input.line))?;
    let end_node = if seg.start_node == input.start_node {
        seg.end_node
    } else if seg.end_node == input.start_node {
        seg.start_node
    } else {
        return Err(EncodeError::UnknownNode(input.start_node));
    };

    // Table 56 Step 2: both ends expand independently — but only POFF is
    // ever tracked; there's no NOFF concept for a single point.
    let start_exp = expansion::expand_to_valid_node(graph, input.start_node, input.line, MAX_LEG_M);
    let end_exp = expansion::expand_to_valid_node(graph, end_node, input.line, MAX_LEG_M);

    let poff_m = input.point_offset_m + start_exp.distance_m;

    let mut leg_segments = Vec::with_capacity(start_exp.segments.len() + 1 + end_exp.segments.len());
    leg_segments.extend(start_exp.segments.iter().rev().copied());
    leg_segments.push(input.line);
    leg_segments.extend(end_exp.segments.iter().copied());

    let attrs = attributes::leg_attributes(graph, start_exp.node, &leg_segments)
        .ok_or(EncodeError::UnknownSegment(input.line))?;

    let last_seg_id = end_exp.segments.last().copied().unwrap_or(input.line);
    let last_seg = graph.segments.get(&last_seg_id).ok_or(EncodeError::UnknownSegment(last_seg_id))?;
    let last_bearing = attributes::last_lrp_bearing_deg(graph, end_exp.node, last_seg_id)
        .ok_or(EncodeError::UnknownSegment(last_seg_id))?;

    if poff_m >= attrs.dnp_m {
        return Err(EncodeError::Codec(openlr_codec::EncodeError::OffsetExceedsLeg {
            offset_m: poff_m, leg_m: attrs.dnp_m,
        }));
    }

    let first = Lrp {
        coord: node_coord(graph, start_exp.node)?,
        bearing: CircularInterval::point(attrs.bearing_deg),
        frc: attrs.frc,
        fow: attrs.fow,
        lfrcnp: Some(attrs.lfrcnp),
        dnp: Some(LinearInterval::point(attrs.dnp_m)),
        pos_offset: if poff_m > 0.0 { Some(LinearInterval::point(poff_m)) } else { None },
        neg_offset: None,
        pos_offset_raw: None,
        neg_offset_raw: None,
    };
    let last = Lrp {
        coord: node_coord(graph, end_exp.node)?,
        bearing: CircularInterval::point(last_bearing),
        frc: last_seg.frc,
        fow: last_seg.fow,
        lfrcnp: None,
        dnp: None,
        pos_offset: None,
        neg_offset: None,
        pos_offset_raw: None,
        neg_offset_raw: None,
    };

    Ok(LocationReference::PointAlongLine {
        lrps: vec![first, last],
        orientation: input.orientation,
        side_of_road: input.side_of_road,
    })
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
            length_m: len_deg * 111_000.0,
            frc: 4, fow: 3,
            direction: Direction::Both,
            stable_id: String::new(),
        }
    }

    #[test]
    fn pal_encodes_exactly_two_lrps_with_positive_offset() {
        let mut g = Graph::new();
        g.add_node(node(0, 0.0, 0.0));
        g.add_node(node(1, 0.001, 0.0));
        g.add_segment(seg(1, 0, 1, 0.001));
        // Nodes 0 and 1 each touch only this one segment — dead ends, already
        // valid (Rule-4 only invalidates pass-throughs, not dead ends).

        let input = PalLocationInput {
            line: SegmentId(1),
            start_node: NodeId(0),
            point_offset_m: 40.0,
            orientation: Orientation::NoOrientation,
            side_of_road: SideOfRoad::DirectlyOnOrNA,
        };
        let loc = encode_pal(&g, &input).unwrap();
        let lrps = loc.lrps().unwrap();
        assert_eq!(lrps.len(), 2);
        assert!(lrps[0].dnp.is_some());
        assert!(lrps[1].dnp.is_none());
        assert!((lrps[0].pos_offset.unwrap().lb - 40.0).abs() < 1e-6);

        // Round-trips through both physical formats.
        let v3 = openlr_codec::encode_v3_base64(&loc).unwrap();
        let redecoded = openlr_codec::decode_v3_base64(&v3).unwrap();
        assert!(redecoded.is_point_on_line());

        let tpeg = openlr_codec::encode_tpeg_hex(&loc).unwrap();
        let redecoded_tpeg = openlr_codec::decode_tpeg_hex(&tpeg).unwrap();
        assert!(redecoded_tpeg.is_point_on_line());
    }
}
