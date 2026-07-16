//! Rule-4 expansion: walk outward from a location's start/end node to the
//! nearest valid node, per the whitepaper's Figure 27 composition
//! (`final_offset = original_within-leg_offset + expansion_distance`).
//! Expansion never shrinks the location — only ever walks away from it.

use openlr_graph::{Graph, NodeId, SegmentId};

/// Why `expand_to_valid_node` stopped where it did — the LLM-diagnostic
/// counterpart to the plain `Expansion` fields, naming which of the walk's
/// four possible stopping conditions actually fired.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ExpansionStopReason {
    /// `start` was already a valid node — no walk needed.
    AlreadyValid,
    /// Walked to a node with more than one continuation (a real junction)
    /// or a dead end recognized by `Graph::is_valid_node` itself.
    ReachedValidNode,
    /// The walk ran out of topology (no segment other than the one just
    /// arrived on touches the current node) while that node was still
    /// invalid — a defensive case; in practice `Graph::is_valid_node`
    /// already treats a true topological dead end as valid, so this
    /// shouldn't fire on well-formed graphs.
    DeadEnd,
    /// Hit `max_leg_m` before reaching a valid node.
    BudgetExhausted,
    /// The next hop's turn angle exceeded `max_turn_deviation_deg` — see the
    /// function doc comment for why this stops the walk rather than being a
    /// gate with an alternative to fall back on.
    SharpTurn { deviation_deg: f64 },
    /// The only other segment touching the current node exists in the raw
    /// (direction-agnostic) topology, but can't actually be travelled in the
    /// direction this expansion needs — see `expand_to_valid_node`'s
    /// `end_side` parameter. Distinct from `DeadEnd` (no other segment at
    /// all) so a caller can tell "no further road" from "a road, but the
    /// wrong way down it" apart.
    WrongDirection,
}

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
    /// Why the walk stopped — `node` is still invalid unless this is
    /// `AlreadyValid` or `ReachedValidNode`.
    pub stopped: ExpansionStopReason,
}

/// Walk outward from `start` until reaching a valid node, the `max_leg_m` cap
/// (Rule-1), a dead end (no further neighbor), or a turn sharper than
/// `max_turn_deviation_deg` — each accepted as the spec's explicit escape
/// hatch for "no valid node reachable" (see below for why the turn-angle
/// case belongs in that same bucket).
///
/// `skip_seg` is the segment the location continues into from `start` — i.e.
/// the direction *not* to walk (that's the location's own interior). Each
/// subsequent hop then skips whichever segment was just traversed.
///
/// `end_side` says which way the assembled final path travels relative to
/// this walk: `true` for end-side expansion, where the walked segments are
/// appended as-is (the final path travels the same direction as the walk —
/// `node` toward `next_node`); `false` for start-side expansion, where the
/// walked segments are reversed before being prepended (the final path
/// travels `next_node` toward `node`, the *opposite* of the walk direction).
/// This determines which endpoint's permitted travel direction each
/// candidate hop is checked against — see `next_hop`.
///
/// A pass-through node has, by construction, exactly one continuation — no
/// alternative to choose between, so there's nothing for a turn-angle *gate*
/// to protect against here (unlike A*/`sweep_coverage`, where the same check
/// rejects a sharp turn in favor of a better-angled alternative route). But
/// walking through one anyway when its continuation is a genuinely sharp
/// real-world kink is worse than pointless: `sweep_coverage` re-verifies this
/// exact stretch afterward with a real turn-angle search, and *would* reject
/// it there — with no alternative to fall back on, since it's the only
/// physical continuation — surfacing as a confusing generic `NoRoute` with no
/// indication the boundary expansion was the actual cause. Stopping here
/// instead, and accepting the current (possibly still-invalid) node, keeps
/// the failure mode honest: "no valid node reachable without an unnavigable
/// turn" rather than an opaque downstream routing failure.
pub fn expand_to_valid_node(
    graph: &Graph,
    start: NodeId,
    skip_seg: SegmentId,
    end_side: bool,
    max_leg_m: f64,
    max_turn_deviation_deg: f64,
) -> Expansion {
    if graph.is_valid_node(start) {
        return Expansion {
            node: start, distance_m: 0.0, segments: Vec::new(),
            stopped: ExpansionStopReason::AlreadyValid,
        };
    }

    let mut node = start;
    let mut skip = skip_seg;
    let mut distance_m = 0.0;
    let mut segments = Vec::new();
    let stopped;

    loop {
        let hop = next_hop(graph, node, skip, end_side);
        let Some((next_node, next_seg, seg_len)) = hop else {
            // Distinguish "no other segment at all" from "a segment exists
            // but can't be travelled the way this expansion needs" — see
            // `next_hop`.
            stopped = if graph.topology_neighbors(node).iter().any(|(_, s)| *s != skip) {
                ExpansionStopReason::WrongDirection
            } else {
                ExpansionStopReason::DeadEnd
            };
            break;
        };
        if distance_m + seg_len > max_leg_m {
            stopped = ExpansionStopReason::BudgetExhausted;
            break;
        }
        if let Some(dev) = graph.turn_deviation_deg(skip, node, next_seg) {
            if dev > max_turn_deviation_deg {
                stopped = ExpansionStopReason::SharpTurn { deviation_deg: dev };
                break;
            }
        }
        distance_m += seg_len;
        node = next_node;
        skip = next_seg;
        segments.push(next_seg);
        if graph.is_valid_node(node) {
            stopped = ExpansionStopReason::ReachedValidNode;
            break;
        }
    }

    Expansion { node, distance_m, segments, stopped }
}

/// The one segment touching `node` other than `skip`, if any — filtered to
/// one that can actually be *travelled* the direction this expansion needs.
/// `Graph::is_valid_node`/`topology_neighbors` are direction-agnostic (real
/// topology, ignoring one-way restrictions), so a pass-through node's "one
/// other continuation" can be a one-way segment oriented the wrong way for
/// this walk — exactly the bug class fixed in `snap_point` (see
/// `Graph::outgoing_segments`'s doc comment). End-side expansion (`end_side`)
/// needs the segment departable from `node` itself (the final path travels
/// the same direction as the walk); start-side expansion needs it departable
/// from the *other* node (the final path travels the walk in reverse — see
/// `expand_to_valid_node`'s doc comment).
fn next_hop(graph: &Graph, node: NodeId, skip: SegmentId, end_side: bool) -> Option<(NodeId, SegmentId, f64)> {
    graph.topology_neighbors(node)
        .iter()
        .find(|(other_node, seg)| {
            *seg != skip && {
                let departable_from = if end_side { node } else { *other_node };
                graph.outgoing_segments(departable_from).contains(seg)
            }
        })
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
        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), true, 15_000.0, 180.0);
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
        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), true, 15_000.0, 180.0);
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
        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), true, 15_000.0, 180.0);
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
        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), true, 10.0, 180.0); // cap way below 50m
        assert_eq!(exp.node, NodeId(1), "should stay put rather than exceed the cap");
        assert_eq!(exp.distance_m, 0.0);
    }

    #[test]
    fn stops_before_an_unnavigable_turn_at_a_pass_through_node() {
        // 0 -> 1 (heading due east) -> 2 (doubling straight back west from 1,
        // a literal reversal) -> node 2 is a real 3-way branch (valid), but
        // the only way to reach it from node 1 requires a 180° turn. With a
        // sub-180 cap, expansion must stop at node 1 (still invalid) rather
        // than walk through a turn `sweep_coverage` would reject anyway.
        let mut g = Graph::new();
        g.add_segment(NetworkSegment {
            id: SegmentId(1), start_node: NodeId(0), end_node: NodeId(1),
            geometry: vec![(0.0, 0.0), (0.002, 0.0)], length_m: 100.0,
            frc: 3, fow: 3, direction: Direction::Both, stable_id: String::new(),
        });
        g.add_segment(NetworkSegment {
            id: SegmentId(2), start_node: NodeId(1), end_node: NodeId(2),
            geometry: vec![(0.002, 0.0), (0.0005, 0.0)], length_m: 50.0,
            frc: 3, fow: 3, direction: Direction::Both, stable_id: String::new(),
        });
        g.add_segment(seg(3, 2, 3, 100.0)); // makes node 2 a real 3-way branch
        g.add_segment(seg(4, 2, 4, 100.0));

        let permissive = expand_to_valid_node(&g, NodeId(1), SegmentId(1), true, 15_000.0, 180.0);
        assert_eq!(permissive.node, NodeId(2), "an unrestricted cap should walk through to the valid node");
        assert!((permissive.distance_m - 50.0).abs() < 1e-9);

        let strict = expand_to_valid_node(&g, NodeId(1), SegmentId(1), true, 15_000.0, 150.0);
        assert_eq!(strict.node, NodeId(1), "a 150° cap should refuse the 180° reversal and stay put");
        assert_eq!(strict.distance_m, 0.0);
        assert!(strict.segments.is_empty());
    }

    #[test]
    fn end_side_rejects_wrong_direction_one_way_continuation() {
        // Node 1 is a pass-through node (2 distinct neighbors: 0 via the
        // skip segment, 2 via seg 2) but seg 2 is one-way and can only be
        // departed from node 2, not node 1. End-side expansion (end_side =
        // true) needs to depart *from* node 1 in the walk direction, so it
        // must refuse this continuation rather than silently walking onto a
        // segment the final path couldn't actually traverse.
        let mut g = Graph::new();
        g.add_segment(seg(1, 0, 1, 100.0));
        g.add_segment(NetworkSegment {
            id: SegmentId(2), start_node: NodeId(1), end_node: NodeId(2),
            geometry: vec![(0.001, 0.0), (0.002, 0.0)], length_m: 50.0,
            frc: 3, fow: 3, direction: Direction::Backward, stable_id: String::new(),
        });
        g.add_segment(seg(3, 2, 3, 100.0));
        g.add_segment(seg(4, 2, 4, 100.0)); // node 2 a real 3-way branch (moot — must be unreachable)

        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), true, 15_000.0, 180.0);
        assert_eq!(exp.node, NodeId(1), "must not walk onto a segment it can't depart from node 1");
        assert_eq!(exp.distance_m, 0.0);
        assert!(exp.segments.is_empty());
        assert_eq!(exp.stopped, ExpansionStopReason::WrongDirection);
    }

    #[test]
    fn end_side_walks_correctly_directioned_one_way_continuation() {
        // Same shape as above, but seg 2 is departable from node 1 (its
        // start_node) — the fix must not reject a continuation just because
        // it's one-way, only one oriented the wrong way.
        let mut g = Graph::new();
        g.add_segment(seg(1, 0, 1, 100.0));
        g.add_segment(NetworkSegment {
            id: SegmentId(2), start_node: NodeId(1), end_node: NodeId(2),
            geometry: vec![(0.001, 0.0), (0.002, 0.0)], length_m: 50.0,
            frc: 3, fow: 3, direction: Direction::Forward, stable_id: String::new(),
        });
        g.add_segment(seg(3, 2, 3, 100.0));
        g.add_segment(seg(4, 2, 4, 100.0));

        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), true, 15_000.0, 180.0);
        assert_eq!(exp.node, NodeId(2));
        assert!((exp.distance_m - 50.0).abs() < 1e-9);
    }

    #[test]
    fn start_side_rejects_wrong_direction_one_way_continuation() {
        // Start-side expansion (end_side = false) walks node 1 -> node 2,
        // but the *final* assembled path travels node 2 -> node 1 (these
        // segments get reversed before being prepended). seg 2 is one-way
        // Forward (departable only from its start_node, node 1) — fine for
        // the walk itself, but the final path needs to depart from node 2,
        // which this segment doesn't allow. Must be rejected.
        let mut g = Graph::new();
        g.add_segment(seg(1, 1, 0, 100.0)); // skip_seg: location's own core
        g.add_segment(NetworkSegment {
            id: SegmentId(2), start_node: NodeId(1), end_node: NodeId(2),
            geometry: vec![(0.001, 0.0), (0.002, 0.0)], length_m: 50.0,
            frc: 3, fow: 3, direction: Direction::Forward, stable_id: String::new(),
        });
        g.add_segment(seg(3, 2, 3, 100.0));
        g.add_segment(seg(4, 2, 4, 100.0));

        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), false, 15_000.0, 180.0);
        assert_eq!(exp.node, NodeId(1), "final path travels node2->node1, which this segment can't support");
        assert_eq!(exp.distance_m, 0.0);
        assert_eq!(exp.stopped, ExpansionStopReason::WrongDirection);
    }

    #[test]
    fn start_side_walks_correctly_directioned_one_way_continuation() {
        // Same shape, but seg 2 is departable from node 2 (its start_node)
        // toward node 1 — exactly the direction the reversed final path
        // needs.
        let mut g = Graph::new();
        g.add_segment(seg(1, 1, 0, 100.0));
        g.add_segment(NetworkSegment {
            id: SegmentId(2), start_node: NodeId(2), end_node: NodeId(1),
            geometry: vec![(0.002, 0.0), (0.001, 0.0)], length_m: 50.0,
            frc: 3, fow: 3, direction: Direction::Forward, stable_id: String::new(),
        });
        g.add_segment(seg(3, 2, 3, 100.0));
        g.add_segment(seg(4, 2, 4, 100.0));

        let exp = expand_to_valid_node(&g, NodeId(1), SegmentId(1), false, 15_000.0, 180.0);
        assert_eq!(exp.node, NodeId(2));
        assert!((exp.distance_m - 50.0).abs() < 1e-9);
    }
}
