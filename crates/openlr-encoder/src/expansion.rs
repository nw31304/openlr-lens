//! Rule-4 expansion: walk outward from a location's start/end node to the
//! nearest valid node, per the whitepaper's Figure 27 composition
//! (`final_offset = original_within-leg_offset + expansion_distance`).
//! Expansion never shrinks the location — only ever walks away from it.

use openlr_graph::{Graph, NodeId, SegmentId};

/// Result of walking outward from one end of a location to a valid node.
pub struct Expansion {
    pub node: NodeId,
    /// Distance walked past the original boundary node. Zero if it was
    /// already valid.
    pub distance_m: f64,
    /// Segments walked, in the order traversed (from the original boundary
    /// node outward) — empty if no expansion was needed. The caller splices
    /// these into the full path: reversed and prepended for a start-side
    /// expansion, appended as-is for an end-side expansion.
    pub segments: Vec<SegmentId>,
}

/// Walk outward from `start` until reaching a valid node, the `max_leg_m` cap
/// (Rule-1), or a dead end (no further neighbor — accepted as the spec's
/// explicit escape hatch for "no valid node reachable").
///
/// `skip_seg` is the segment the location continues into from `start` — i.e.
/// the direction *not* to walk (that's the location's own interior). Each
/// subsequent hop then skips whichever segment was just traversed.
pub fn expand_to_valid_node(
    graph: &Graph,
    start: NodeId,
    skip_seg: SegmentId,
    max_leg_m: f64,
) -> Expansion {
    let mut node = start;
    let mut skip = skip_seg;
    let mut distance_m = 0.0;
    let mut segments = Vec::new();

    while !graph.is_valid_node(node) {
        match next_hop(graph, node, skip) {
            Some((next_node, next_seg, seg_len)) if distance_m + seg_len <= max_leg_m => {
                distance_m += seg_len;
                node = next_node;
                skip = next_seg;
                segments.push(next_seg);
            }
            _ => break, // cap reached, or no further neighbor — accept the invalid node.
        }
    }

    Expansion { node, distance_m, segments }
}

/// The one segment touching `node` other than `skip`, if any. At an invalid
/// node (a pass-through, per `Graph::is_valid_node`) there is by construction
/// exactly one such neighbor to continue the walk into.
fn next_hop(graph: &Graph, node: NodeId, skip: SegmentId) -> Option<(NodeId, SegmentId, f64)> {
    graph.topology_neighbors(node)
        .iter()
        .find(|(_, seg)| *seg != skip)
        .and_then(|(other_node, seg_id)| {
            graph.segments.get(seg_id).map(|seg| (*other_node, *seg_id, seg.length_m))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlr_graph::{Direction, NetworkSegment};

    fn seg(id: u32, s: u32, e: u32, len: f64) -> NetworkSegment {
        NetworkSegment {
            id: SegmentId(id),
            start_node: NodeId(s),
            end_node: NodeId(e),
            geometry: vec![(0.0, 0.0), (0.001, 0.0)],
            length_m: len,
            frc: 3, fow: 3,
            direction: Direction::Both,
            stable_id: String::new(),
        }
    }

    #[test]
    fn already_valid_node_does_not_expand() {
        let mut g = Graph::new();
        // Node 1 has three distinct neighbors — a real branch, already valid.
        g.add_segment(seg(1, 0, 1, 100.0));
        g.add_segment(seg(2, 1, 2, 100.0));
        g.add_segment(seg(3, 1, 3, 100.0));
        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), 15_000.0);
        assert_eq!(exp.node, NodeId(1));
        assert_eq!(exp.distance_m, 0.0);
    }

    #[test]
    fn walks_past_one_pass_through_node() {
        // Location's boundary is at node 1 (invalid: pass-through). The real
        // junction is at node 2. Arrived via seg 1 (0->1); must walk seg 2
        // (1->2) to reach it.
        let mut g = Graph::new();
        g.add_segment(seg(1, 0, 1, 100.0));
        g.add_segment(seg(2, 1, 2, 50.0));
        g.add_segment(seg(3, 2, 3, 100.0));
        g.add_segment(seg(4, 2, 4, 100.0)); // makes node 2 a real 3-way branch
        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), 15_000.0);
        assert_eq!(exp.node, NodeId(2));
        assert!((exp.distance_m - 50.0).abs() < 1e-9);
    }

    #[test]
    fn stops_at_dead_end_when_no_valid_node_reachable() {
        // 0 -> 1 -> 2, and node 2 is a true dead end (degree 1) — so node 2
        // itself counts as valid, and expansion from node 1 should reach it.
        let mut g = Graph::new();
        g.add_segment(seg(1, 0, 1, 100.0));
        g.add_segment(seg(2, 1, 2, 50.0));
        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), 15_000.0);
        assert_eq!(exp.node, NodeId(2));
        assert!((exp.distance_m - 50.0).abs() < 1e-9);
    }

    #[test]
    fn respects_max_leg_cap() {
        let mut g = Graph::new();
        g.add_segment(seg(1, 0, 1, 100.0));
        g.add_segment(seg(2, 1, 2, 50.0));
        g.add_segment(seg(3, 2, 3, 100.0));
        g.add_segment(seg(4, 2, 4, 100.0)); // node 2 would be valid, but out of budget
        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), 10.0); // cap way below 50m
        assert_eq!(exp.node, NodeId(1), "should stay put rather than exceed the cap");
        assert_eq!(exp.distance_m, 0.0);
    }
}
