use tracing::{debug, warn};

use crate::adapt::AdaptedSegment;
use crate::split::parse_gers_id;

/// Heading constraint packed into 2 bits (used in the tile restriction `flags` byte).
/// `HEADING_ANY` means the restriction fires regardless of which direction the segment is traversed.
pub const HEADING_ANY:      u8 = 0b00;
pub const HEADING_FORWARD:  u8 = 0b01;
pub const HEADING_BACKWARD: u8 = 0b10;

fn parse_heading(s: Option<&str>) -> u8 {
    match s {
        Some("forward")  => HEADING_FORWARD,
        Some("backward") => HEADING_BACKWARD,
        _                => HEADING_ANY,
    }
}

/// Encode from_heading (bits [1:0]) and to_heading (bits [3:2]) into the restriction flags byte.
/// Bits [7:4] are reserved (zero).
pub fn encode_restriction_flags(from_heading: u8, to_heading: u8) -> u8 {
    (from_heading & 0x03) | ((to_heading & 0x03) << 2)
}

/// A turn restriction expressed as global GERS ids (parsed to bytes at collection time).
/// Resolved to tile-local indices during the tile-write step.
///
/// `flags` encodes optional direction constraints so the A* engine can skip restrictions
/// that don't apply to the current traversal direction:
///   bits [1:0] = from_heading: direction on the FROM segment that triggers this ban
///                (`HEADING_ANY` = fires for both directions, e.g. on one-way segments).
///   bits [3:2] = to_heading:   direction on the TO segment (`final_heading` in Overture).
///                (`HEADING_ANY` = not constrained).
///   bits [7:4] = reserved (zero).
#[derive(Debug, Clone)]
pub struct RestrictionTriple {
    pub from_segment_gers: [u8; 16],
    pub via_connector_gers: [u8; 16],
    pub to_segment_gers: [u8; 16],
    pub flags: u8,
}

/// Flatten `prohibited_transitions` from every adapted segment into a list of
/// (from_segment, via_connector, to_segment) triples.
///
/// Overture format: each `ProhibitedTransition` lives on its "from" segment and
/// carries a `sequence` of `{connector_id, segment_id}` hops.  For the common
/// single-hop case: from = parent segment, via = sequence[0].connector_id,
/// to = sequence[0].segment_id.  Multi-hop sequences (length > 1) represent
/// complex restrictions not yet modelled; they are logged and skipped.
///
/// The `when.heading` field (which direction on the FROM segment triggers the ban)
/// and `final_heading` (required direction on the TO segment) are captured in `flags`
/// so the A* engine can evaluate direction-conditioned restrictions correctly.
pub fn flatten(segments: &[AdaptedSegment]) -> Vec<RestrictionTriple> {
    let mut out = Vec::new();

    for seg in segments {
        let from_gers = match parse_gers_id(&seg.gers_id) {
            Ok(id) => id,
            Err(_) => {
                warn!(id = %seg.gers_id, "segment has invalid GERS id, skipping its restrictions");
                continue;
            }
        };

        for pt in &seg.prohibited_transitions {
            if pt.sequence.is_empty() {
                warn!(parent = %seg.gers_id, "prohibited_transition has empty sequence, skipped");
                continue;
            }
            if pt.sequence.len() > 1 {
                warn!(
                    parent = %seg.gers_id,
                    hops = pt.sequence.len(),
                    "multi-hop prohibited_transition not yet supported, skipped"
                );
                continue;
            }

            let from_heading = parse_heading(
                pt.when_condition.as_ref().and_then(|w| w.heading.as_deref()),
            );
            let to_heading = parse_heading(pt.final_heading.as_deref());
            let flags = encode_restriction_flags(from_heading, to_heading);

            let hop = &pt.sequence[0];
            match (parse_gers_id(&hop.connector_id), parse_gers_id(&hop.segment_id)) {
                (Ok(via), Ok(to)) => {
                    out.push(RestrictionTriple {
                        from_segment_gers: from_gers,
                        via_connector_gers: via,
                        to_segment_gers: to,
                        flags,
                    });
                }
                _ => {
                    warn!(
                        parent = %seg.gers_id,
                        connector = %hop.connector_id,
                        segment   = %hop.segment_id,
                        "prohibited_transition hop has invalid GERS id, skipped"
                    );
                }
            }
        }
    }

    debug!(count = out.len(), "turn restrictions extracted");
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapt::AdaptedSegment;
    use crate::extract::{ConnectorRef, ProhibitedTransition, SequenceEntry};
    use openlr_graph::Direction;

    fn bare_segment(id: &str, pt: Vec<ProhibitedTransition>) -> AdaptedSegment {
        AdaptedSegment {
            gers_id: id.to_string(),
            geometry: vec![(0.0, 0.0), (1.0, 0.0)],
            connectors: vec![
                ConnectorRef { connector_id: "start".into(), at: 0.0 },
                ConnectorRef { connector_id: "end".into(),   at: 1.0 },
            ],
            frc: 3,
            fow: 3,
            direction: Direction::Both,
            vehicular: true,
            prohibited_transitions: pt,
        }
    }

    #[test]
    fn empty_when_no_restrictions() {
        let segs = vec![bare_segment("seg1", vec![])];
        assert!(flatten(&segs).is_empty());
    }

    // Valid 32-hex-char GERS ids for use as the parent "from" segment.
    const SEG1_GERS: &str = "00000000000000000000000000000001";
    const VIA_GERS:  &str = "00000000000000000000000000000002";
    const TO_GERS:   &str = "00000000000000000000000000000003";

    fn one_hop(via: &str, to: &str) -> ProhibitedTransition {
        ProhibitedTransition {
            sequence: vec![SequenceEntry { connector_id: via.into(), segment_id: to.into() }],
            final_heading: None,
            when_condition: None,
        }
    }

    fn one_hop_headed(via: &str, to: &str, from_h: &str, to_h: &str) -> ProhibitedTransition {
        use crate::extract::AccessWhen;
        ProhibitedTransition {
            sequence: vec![SequenceEntry { connector_id: via.into(), segment_id: to.into() }],
            final_heading: Some(to_h.into()),
            when_condition: Some(AccessWhen {
                heading: Some(from_h.into()),
                during: None, vehicle: None, mode: None,
            }),
        }
    }

    #[test]
    fn extracts_triple_from_single_hop() {
        let segs = vec![bare_segment(SEG1_GERS, vec![one_hop(VIA_GERS, TO_GERS)])];
        let triples = flatten(&segs);
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].from_segment_gers,  parse_gers_id(SEG1_GERS).unwrap());
        assert_eq!(triples[0].via_connector_gers, parse_gers_id(VIA_GERS).unwrap());
        assert_eq!(triples[0].to_segment_gers,    parse_gers_id(TO_GERS).unwrap());
        assert_eq!(triples[0].flags, encode_restriction_flags(HEADING_ANY, HEADING_ANY));
    }

    #[test]
    fn captures_heading_conditions() {
        let segs = vec![bare_segment(SEG1_GERS, vec![
            one_hop_headed(VIA_GERS, TO_GERS, "forward", "backward"),
        ])];
        let triples = flatten(&segs);
        assert_eq!(triples.len(), 1);
        let expected = encode_restriction_flags(HEADING_FORWARD, HEADING_BACKWARD);
        assert_eq!(triples[0].flags, expected);
    }

    #[test]
    fn heading_any_when_no_condition() {
        let segs = vec![bare_segment(SEG1_GERS, vec![one_hop(VIA_GERS, TO_GERS)])];
        let triples = flatten(&segs);
        assert_eq!(triples[0].flags, 0x00); // both fields = HEADING_ANY
    }

    #[test]
    fn skips_multi_hop() {
        let segs = vec![bare_segment(SEG1_GERS, vec![ProhibitedTransition {
            sequence: vec![
                SequenceEntry { connector_id: VIA_GERS.into(), segment_id: TO_GERS.into() },
                SequenceEntry { connector_id: VIA_GERS.into(), segment_id: TO_GERS.into() },
            ],
            final_heading: None,
            when_condition: None,
        }])];
        assert!(flatten(&segs).is_empty());
    }

    #[test]
    fn skips_invalid_gers_ids() {
        let segs = vec![bare_segment(SEG1_GERS, vec![one_hop("not-a-gers-id", TO_GERS)])];
        assert!(flatten(&segs).is_empty());
    }

    #[test]
    fn skips_empty_sequence() {
        let segs = vec![bare_segment(SEG1_GERS, vec![ProhibitedTransition {
            sequence: vec![],
            final_heading: None,
            when_condition: None,
        }])];
        assert!(flatten(&segs).is_empty());
    }
}
