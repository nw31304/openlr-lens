use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use tracing::{trace, warn};

use crate::osm_extract::{OsmData, OsmNodeCoord, OsmWay};
use crate::restrictions::{encode_restriction_flags, RestrictionTriple, HEADING_ANY};
use crate::split::{polyline_length_m, NodeRecord, SplitEdge};
use openlr_graph::Direction;

// ── OSM ID encoding ───────────────────────────────────────────────────────────
//
// 16-byte stable IDs derived from OSM numeric IDs (Invariant 2).
//
// Node ID:   [0u8; 8]  ++ (node_id as u64).to_le_bytes()
// Way ID:    (way_id as u64).to_le_bytes() ++ [0u8; 8]
//
// The two spaces are disjoint: a node ID always has zeroes in bytes 0–7 and a
// way ID always has zeroes in bytes 8–15, so they can never accidentally collide.
// The tile writer uses `(parent_gers_id, end_node_gers)` as the restriction lookup
// key, which requires from_segment_gers == parent_gers_id (way encoding) and
// via_connector_gers == end/start_node_gers (node encoding).

pub(crate) fn encode_node_id(id: i64) -> [u8; 16] {
    let mut buf = [0u8; 16];
    buf[8..16].copy_from_slice(&(id as u64).to_le_bytes());
    buf
}

pub(crate) fn encode_way_id(id: i64) -> [u8; 16] {
    let mut buf = [0u8; 16];
    buf[0..8].copy_from_slice(&(id as u64).to_le_bytes());
    buf
}

// ── FRC / FOW / direction from OSM tags ──────────────────────────────────────
//
// FRC and FOW values follow TomTom's osm_tag_mapper reference implementation
// (github.com/tomtom-international/osm_tag_mapper, Apache 2.0).
//
// FRC: motorway=0, {motorway_link,trunk,trunk_link}=1, {primary,primary_link}=2,
//      {secondary,secondary_link}=3, {tertiary,tertiary_link}=4,
//      yes=5, unclassified=6,
//      {residential,service,living_street,road,track}=7.
//
// FOW base values come from FowMapping; junction/dual_carriageway overrides
// are applied afterwards by derive_fow().

/// Map OSM `highway` value to `(frc, base_fow, vehicular)`.
/// `base_fow` is the default before junction/dual_carriageway overrides.
/// Returns `None` for unknown tag values.
fn highway_attrs(highway: &str) -> Option<(u8, u8, bool)> {
    let attrs = match highway {
        // FRC0 — motorway gets FOW=1 directly; no dual-carriageway override applies
        "motorway"        => (0, 1, true),
        // FRC1 — link variants get FOW=6 (slip road)
        "motorway_link"   => (1, 6, true),
        "trunk"           => (1, 3, true),
        "trunk_link"      => (1, 6, true),
        // FRC2
        "primary"         => (2, 3, true),
        "primary_link"    => (2, 6, true),
        // FRC3
        "secondary"       => (3, 3, true),
        "secondary_link"  => (3, 6, true),
        // FRC4
        "tertiary"        => (4, 3, true),
        "tertiary_link"   => (4, 6, true),
        // FRC5 — generic highway=yes (important local road)
        "yes"             => (5, 3, true),
        // FRC6
        "unclassified"    => (6, 3, true),
        // FRC7 vehicular
        "residential"     => (7, 3, true),
        "living_street"   => (7, 3, true),
        "road"            => (7, 3, true),
        "track"           => (7, 3, true),
        "service"         => (7, 7, true),   // FOW=OTHER (service_road)
        // FRC7 non-vehicular
        "pedestrian"      => (7, 7, false),
        "footway"         => (7, 7, false),
        "cycleway"        => (7, 7, false),
        "path"            => (7, 7, false),
        "steps"           => (7, 7, false),
        "bridleway"       => (7, 7, false),
        _                 => return None,
    };
    Some(attrs)
}

/// Return true if the way should be excluded from routing (area polygon or private access).
fn is_excluded(tags: &HashMap<String, String>) -> bool {
    // Exclude area=yes (pedestrian plazas etc. tagged as highway=pedestrian + area=yes)
    matches!(tags.get("area").map(|s| s.as_str()), Some("yes") | Some("true") | Some("1"))
    // Exclude access=private or access=no (private driveways, gated communities)
    || matches!(tags.get("access").map(|s| s.as_str()), Some("private") | Some("no"))
}

fn derive_direction(tags: &HashMap<String, String>) -> Direction {
    // junction=roundabout implicitly means oneway=yes (OSM standard).
    if tags.get("junction").map(|s| s.as_str()) == Some("roundabout") {
        return Direction::Forward;
    }
    match tags.get("oneway").map(|s| s.as_str()) {
        Some("yes") | Some("true") | Some("1") => Direction::Forward,
        Some("-1") | Some("reverse")           => Direction::Backward,
        _                                      => Direction::Both,
    }
}

fn derive_fow(mut fow: u8, tags: &HashMap<String, String>) -> u8 {
    match tags.get("junction").map(|s| s.as_str()) {
        Some("roundabout") | Some("mini_roundabout") => fow = 4,
        _ => {}
    }
    if fow != 4 {
        if tags.get("dual_carriageway").map(|s| s.as_str()) == Some("yes") {
            fow = 2;
        }
    }
    fow
}

// ── Way splitting ─────────────────────────────────────────────────────────────

fn split_way(
    way: &OsmWay,
    intersection_nodes: &HashSet<i64>,
    node_coords: &HashMap<i64, OsmNodeCoord>,
    frc: u8,
    fow: u8,
    direction: Direction,
) -> (Vec<SplitEdge>, Vec<NodeRecord>) {
    if way.node_ids.len() < 2 {
        return (vec![], vec![]);
    }

    let parent_gers = encode_way_id(way.id);

    // Collect the start-indices of each sub-edge: always 0, plus every interior
    // node that is a road intersection (shared by 2+ ways).
    let mut split_starts: Vec<usize> = vec![0];
    let last = way.node_ids.len() - 1;
    for (i, &nid) in way.node_ids[1..last].iter().enumerate() {
        if intersection_nodes.contains(&nid) {
            split_starts.push(i + 1); // convert slice index to way-node index
        }
    }

    let mut edges: Vec<SplitEdge>  = Vec::with_capacity(split_starts.len());
    let mut nodes: Vec<NodeRecord> = Vec::with_capacity(split_starts.len() + 1);

    for (k, &start_idx) in split_starts.iter().enumerate() {
        let end_idx = if k + 1 < split_starts.len() { split_starts[k + 1] } else { last };

        // Collect geometry for this sub-edge.
        let mut geom: Vec<(f64, f64)> = Vec::with_capacity(end_idx - start_idx + 1);
        let mut ok = true;
        for &nid in &way.node_ids[start_idx..=end_idx] {
            if let Some(c) = node_coords.get(&nid) {
                geom.push((c.lon, c.lat));
            } else {
                warn!(way = way.id, node = nid, "missing node coordinates, sub-edge skipped");
                ok = false;
                break;
            }
        }
        if !ok || geom.len() < 2 {
            continue;
        }

        let start_nid = way.node_ids[start_idx];
        let end_nid   = way.node_ids[end_idx];
        let start_gers = encode_node_id(start_nid);
        let end_gers   = encode_node_id(end_nid);
        let length_m   = polyline_length_m(&geom);

        trace!(way = way.id, start = start_nid, end = end_nid, length_m, "sub-edge");

        nodes.push(NodeRecord { gers_id: start_gers, lon: geom[0].0,              lat: geom[0].1 });
        nodes.push(NodeRecord { gers_id: end_gers,   lon: geom.last().unwrap().0, lat: geom.last().unwrap().1 });

        edges.push(SplitEdge {
            start_node_gers: start_gers,
            end_node_gers:   end_gers,
            geometry:        geom,
            length_m,
            frc,
            fow,
            direction,
            parent_gers_id:  parent_gers,
        });
    }

    (edges, nodes)
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Convert raw OSM data into the tile-pipeline's edge/node/restriction types.
///
/// This replaces the Overture `adapt` + `split` + `restrictions::flatten` steps
/// with a single OSM-native pass.
pub fn adapt(data: OsmData) -> (Vec<SplitEdge>, Vec<NodeRecord>, Vec<RestrictionTriple>) {
    let OsmData { ways, nodes, restrictions } = data;

    // ── Step 1: filter to vehicular ways, compute per-way attributes ──────────

    struct WayAttrs {
        way: OsmWay,
        frc: u8,
        fow: u8,
        direction: Direction,
    }

    let vehicular: Vec<WayAttrs> = ways
        .into_iter()
        .filter_map(|way| {
            let highway = way.tags.get("highway")?.as_str();
            let (frc, base_fow, is_vehicular) = highway_attrs(highway)?;
            if !is_vehicular || is_excluded(&way.tags) {
                return None;
            }
            let fow       = derive_fow(base_fow, &way.tags);
            let direction = derive_direction(&way.tags);
            Some(WayAttrs { way, frc, fow, direction })
        })
        .collect();

    // ── Step 2: find intersection nodes ──────────────────────────────────────
    //
    // A node is an intersection if it appears in 2+ vehicular ways, OR if it is
    // a way endpoint (always a split point regardless of connectivity).

    let mut node_ref_count: HashMap<i64, u32> = HashMap::new();
    for wa in &vehicular {
        for &nid in &wa.way.node_ids {
            *node_ref_count.entry(nid).or_insert(0) += 1;
        }
    }

    let mut intersection_nodes: HashSet<i64> = node_ref_count
        .iter()
        .filter(|(_, &cnt)| cnt >= 2)
        .map(|(&id, _)| id)
        .collect();

    // Always split at way endpoints (first and last node of each way).
    for wa in &vehicular {
        if let (Some(&f), Some(&l)) = (wa.way.node_ids.first(), wa.way.node_ids.last()) {
            intersection_nodes.insert(f);
            intersection_nodes.insert(l);
        }
    }

    // ── Step 3: split ways in parallel ───────────────────────────────────────

    let results: Vec<(Vec<SplitEdge>, Vec<NodeRecord>)> = vehicular
        .par_iter()
        .map(|wa| split_way(&wa.way, &intersection_nodes, &nodes, wa.frc, wa.fow, wa.direction))
        .collect();

    let mut all_edges: Vec<SplitEdge>              = Vec::new();
    let mut node_map:  HashMap<[u8; 16], NodeRecord> = HashMap::new();
    for (edges, node_records) in results {
        all_edges.extend(edges);
        for n in node_records {
            node_map.insert(n.gers_id, n); // last writer wins; coords should agree
        }
    }
    let all_nodes: Vec<NodeRecord> = node_map.into_values().collect();

    // ── Step 4: convert turn restrictions ────────────────────────────────────
    //
    // from_segment_gers = encode_way_id(from_way_id)  → matches parent_gers_id of FROM sub-edge
    // via_connector_gers = encode_node_id(via_node_id) → matches end_node_gers of FROM sub-edge
    //                                                    and start_node_gers of TO sub-edge
    // to_segment_gers   = encode_way_id(to_way_id)   → matches parent_gers_id of TO sub-edge
    //
    // No heading conditions in basic OSM restrictions (those are in restriction:conditional).

    let all_restrictions: Vec<RestrictionTriple> = restrictions
        .iter()
        .map(|r| RestrictionTriple {
            from_segment_gers: encode_way_id(r.from_way_id),
            via_connector_gers: encode_node_id(r.via_node_id),
            to_segment_gers:   encode_way_id(r.to_way_id),
            flags: encode_restriction_flags(HEADING_ANY, HEADING_ANY),
        })
        .collect();

    (all_edges, all_nodes, all_restrictions)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn coord(lon: f64, lat: f64) -> OsmNodeCoord {
        OsmNodeCoord { lon, lat }
    }

    fn make_nodes(pairs: &[(i64, f64, f64)]) -> HashMap<i64, OsmNodeCoord> {
        pairs.iter().map(|&(id, lon, lat)| (id, coord(lon, lat))).collect()
    }

    fn primary(id: i64, node_ids: Vec<i64>) -> OsmWay {
        OsmWay {
            id,
            node_ids,
            tags: [("highway".to_string(), "primary".to_string())].into(),
        }
    }

    fn roundabout_way(id: i64, node_ids: Vec<i64>) -> OsmWay {
        OsmWay {
            id,
            node_ids,
            tags: [
                ("highway".to_string(), "secondary".to_string()),
                ("junction".to_string(), "roundabout".to_string()),
            ]
            .into(),
        }
    }

    // ── FRC / FOW / direction ─────────────────────────────────────────────────

    #[test]
    fn motorway_frc_fow() {
        let (frc, fow, veh) = highway_attrs("motorway").unwrap();
        assert_eq!(frc, 0);
        assert_eq!(fow, 1); // FOW=MOTORWAY
        assert!(veh);
    }

    #[test]
    fn motorway_link_is_frc1_slip_road() {
        let (frc, fow, veh) = highway_attrs("motorway_link").unwrap();
        assert_eq!(frc, 1); // INTERNATIONAL_ROAD, not MOTORWAY
        assert_eq!(fow, 6); // FOW=SLIP_ROAD
        assert!(veh);
    }

    #[test]
    fn primary_is_frc2() {
        let (frc, _, _) = highway_attrs("primary").unwrap();
        assert_eq!(frc, 2);
    }

    #[test]
    fn secondary_is_frc3() {
        let (frc, _, _) = highway_attrs("secondary").unwrap();
        assert_eq!(frc, 3);
    }

    #[test]
    fn unclassified_is_frc6() {
        let (frc, _, _) = highway_attrs("unclassified").unwrap();
        assert_eq!(frc, 6);
    }

    #[test]
    fn residential_is_frc7_vehicular() {
        let (frc, _, veh) = highway_attrs("residential").unwrap();
        assert_eq!(frc, 7);
        assert!(veh);
    }

    #[test]
    fn pedestrian_non_vehicular() {
        let (_, _, veh) = highway_attrs("pedestrian").unwrap();
        assert!(!veh);
    }

    #[test]
    fn unknown_highway_returns_none() {
        assert!(highway_attrs("proposed").is_none());
    }

    #[test]
    fn area_excluded() {
        let tags: HashMap<String, String> =
            [("area".to_string(), "yes".to_string())].into();
        assert!(is_excluded(&tags));
    }

    #[test]
    fn private_access_excluded() {
        let tags: HashMap<String, String> =
            [("access".to_string(), "private".to_string())].into();
        assert!(is_excluded(&tags));
    }

    #[test]
    fn roundabout_fow_and_direction() {
        let tags: HashMap<String, String> = [
            ("highway".to_string(), "secondary".to_string()),
            ("junction".to_string(), "roundabout".to_string()),
        ]
        .into();
        assert_eq!(derive_fow(3, &tags), 4);
        assert_eq!(derive_direction(&tags), Direction::Forward);
    }

    #[test]
    fn oneway_yes_gives_forward() {
        let tags: HashMap<String, String> =
            [("oneway".to_string(), "yes".to_string())].into();
        assert_eq!(derive_direction(&tags), Direction::Forward);
    }

    #[test]
    fn oneway_minus1_gives_backward() {
        let tags: HashMap<String, String> =
            [("oneway".to_string(), "-1".to_string())].into();
        assert_eq!(derive_direction(&tags), Direction::Backward);
    }

    // ── Way splitting ─────────────────────────────────────────────────────────

    #[test]
    fn simple_way_no_interior_intersection_gives_one_edge() {
        // Way A–B–C; B is not in any other way.
        let way = primary(1, vec![1, 2, 3]);
        let nodes = make_nodes(&[(1, 174.0, -36.0), (2, 174.5, -36.0), (3, 175.0, -36.0)]);
        let mut intersections = HashSet::new();
        intersections.insert(1i64); // endpoints
        intersections.insert(3i64);

        let (edges, node_records) = split_way(&way, &intersections, &nodes, 1, 3, Direction::Both);
        assert_eq!(edges.len(), 1);
        assert_eq!(node_records.len(), 2);
        assert_eq!(edges[0].geometry.len(), 3); // all original vertices kept
        assert!(edges[0].length_m > 0.0);
    }

    #[test]
    fn interior_intersection_splits_into_two_edges() {
        // Way A–B–C–D; B is shared with another way.
        let way = primary(1, vec![10, 20, 30, 40]);
        let nodes = make_nodes(&[
            (10, 174.0, -36.0),
            (20, 174.25, -36.0),
            (30, 174.5, -36.0),
            (40, 175.0, -36.0),
        ]);
        let mut intersections = HashSet::new();
        intersections.insert(10i64);
        intersections.insert(20i64); // interior intersection
        intersections.insert(40i64);

        let (edges, _) = split_way(&way, &intersections, &nodes, 1, 3, Direction::Both);
        assert_eq!(edges.len(), 2);
        // Sub-edge 1: nodes 10,20 → 2 geometry points
        assert_eq!(edges[0].geometry.len(), 2);
        // Sub-edge 2: nodes 20,30,40 → 3 geometry points
        assert_eq!(edges[1].geometry.len(), 3);
    }

    #[test]
    fn roundabout_edges_are_forward_fow4() {
        let way = roundabout_way(99, vec![1, 2, 3]);
        let nodes = make_nodes(&[
            (1, 174.0, -36.0),
            (2, 174.1, -36.0),
            (3, 174.2, -36.0),
        ]);
        let fow = derive_fow(3, &way.tags); // secondary base fow=3, overridden to 4
        let dir = derive_direction(&way.tags);
        assert_eq!(fow, 4);
        assert_eq!(dir, Direction::Forward);

        let mut intersections = HashSet::new();
        intersections.insert(1i64);
        intersections.insert(3i64);
        let (edges, _) = split_way(&way, &intersections, &nodes, 2, fow, dir);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].fow, 4);
        assert_eq!(edges[0].direction, Direction::Forward);
    }

    // ── ID encoding ───────────────────────────────────────────────────────────

    #[test]
    fn node_id_encoding_is_stable() {
        let id = 123_456_789i64;
        let enc = encode_node_id(id);
        assert_eq!(&enc[0..8], &[0u8; 8]); // zeroes in first 8 bytes
        let back = i64::from_le_bytes(enc[8..16].try_into().unwrap());
        assert_eq!(back, id);
    }

    #[test]
    fn way_id_encoding_is_stable() {
        let id = 987_654_321i64;
        let enc = encode_way_id(id);
        assert_eq!(&enc[8..16], &[0u8; 8]); // zeroes in last 8 bytes
        let back = i64::from_le_bytes(enc[0..8].try_into().unwrap());
        assert_eq!(back, id);
    }

    #[test]
    fn node_and_way_encodings_are_disjoint() {
        // A node ID that equals a way ID must produce different 16-byte values.
        let same_numeric = 42i64;
        assert_ne!(encode_node_id(same_numeric), encode_way_id(same_numeric));
    }

    // ── Full adapt pass ───────────────────────────────────────────────────────

    #[test]
    fn adapt_filters_pedestrian_ways() {
        use crate::osm_extract::OsmData;
        let data = OsmData {
            ways: vec![
                OsmWay {
                    id: 1,
                    node_ids: vec![1, 2],
                    tags: [("highway".to_string(), "footway".to_string())].into(),
                },
                OsmWay {
                    id: 2,
                    node_ids: vec![1, 2],
                    tags: [("highway".to_string(), "primary".to_string())].into(),
                },
            ],
            nodes: make_nodes(&[(1, 174.0, -36.0), (2, 175.0, -36.0)]),
            restrictions: vec![],
        };
        let (edges, _, _) = adapt(data);
        assert_eq!(edges.len(), 1, "only the primary way produces an edge");
        assert_eq!(edges[0].frc, 2); // primary → FRC2 (MAJOR_ROAD)
    }

    #[test]
    fn adapt_produces_restriction_triple() {
        use crate::osm_extract::{OsmData, OsmRestriction as Restriction};
        let data = OsmData {
            ways: vec![
                primary(100, vec![1, 5, 2]),
                primary(200, vec![5, 3]),
            ],
            nodes: make_nodes(&[
                (1, 174.0, -36.0),
                (5, 174.5, -36.0),
                (2, 175.0, -36.0),
                (3, 174.5, -36.5),
            ]),
            restrictions: vec![Restriction {
                from_way_id: 100,
                via_node_id: 5,
                to_way_id: 200,
            }],
        };
        let (_, _, restrictions) = adapt(data);
        assert_eq!(restrictions.len(), 1);
        assert_eq!(restrictions[0].from_segment_gers, encode_way_id(100));
        assert_eq!(restrictions[0].via_connector_gers, encode_node_id(5));
        assert_eq!(restrictions[0].to_segment_gers, encode_way_id(200));
    }
}
