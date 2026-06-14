use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use osmpbf::{Element, ElementReader, RelMemberType};
use tracing::info;

use crate::extent::Bbox;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OsmWay {
    pub id: i64,
    pub node_ids: Vec<i64>,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy)]
pub struct OsmNodeCoord {
    pub lon: f64,
    pub lat: f64,
}

/// A simple (via=single-node) prohibited-turn restriction.
/// Complex via-way restrictions and "only_*" restrictions are skipped for v1.
#[derive(Debug, Clone)]
pub struct OsmRestriction {
    pub from_way_id: i64,
    pub via_node_id: i64,
    pub to_way_id: i64,
}

pub struct OsmData {
    pub ways: Vec<OsmWay>,
    pub nodes: HashMap<i64, OsmNodeCoord>,
    pub restrictions: Vec<OsmRestriction>,
}

// ── Parallel map-reduce accumulator ──────────────────────────────────────────

#[derive(Default)]
struct Partial {
    nodes:        HashMap<i64, OsmNodeCoord>,
    ways:         Vec<OsmWay>,
    restrictions: Vec<OsmRestriction>,
}

impl Partial {
    fn merge(mut self, other: Partial) -> Partial {
        self.nodes.extend(other.nodes);
        self.ways.extend(other.ways);
        self.restrictions.extend(other.restrictions);
        self
    }
}

fn process_element(el: Element<'_>) -> Partial {
    let mut p = Partial::default();
    match el {
        Element::Node(node) => {
            p.nodes.insert(node.id(), OsmNodeCoord { lon: node.lon(), lat: node.lat() });
        }
        Element::DenseNode(node) => {
            p.nodes.insert(node.id(), OsmNodeCoord { lon: node.lon(), lat: node.lat() });
        }
        Element::Way(way) => {
            let tags: HashMap<String, String> = way.tags()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if !tags.contains_key("highway") {
                return p;
            }
            let node_ids: Vec<i64> = way.refs().collect();
            if node_ids.len() >= 2 {
                p.ways.push(OsmWay { id: way.id(), node_ids, tags });
            }
        }
        Element::Relation(relation) => {
            let tags: HashMap<String, String> = relation.tags()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if tags.get("type").map(|s| s.as_str()) != Some("restriction") {
                return p;
            }
            // Only "no_*" prohibitions; skip "only_*" and conditional restrictions.
            let restriction_tag = tags.get("restriction").map(|s| s.as_str()).unwrap_or("");
            if !restriction_tag.starts_with("no_") {
                return p;
            }
            let mut from_way  = None;
            let mut via_node  = None;
            let mut to_way    = None;
            for member in relation.members() {
                let role = member.role().unwrap_or("");
                match (member.member_type, role) {
                    (RelMemberType::Way,  "from") => from_way  = Some(member.member_id),
                    (RelMemberType::Node, "via")  => via_node  = Some(member.member_id),
                    (RelMemberType::Way,  "to")   => to_way    = Some(member.member_id),
                    _ => {}
                }
            }
            if let (Some(f), Some(v), Some(t)) = (from_way, via_node, to_way) {
                p.restrictions.push(OsmRestriction {
                    from_way_id: f,
                    via_node_id: v,
                    to_way_id:   t,
                });
            }
        }
    }
    p
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Read an OSM PBF file and extract highway ways, node coordinates, and turn restrictions.
///
/// If `bbox` is given, only ways that have at least one node inside the bbox are kept;
/// all nodes referenced by kept ways are retained (including nodes slightly outside the bbox
/// that are part of roads crossing the boundary).
pub fn extract(path: &Path, bbox: Option<Bbox>) -> Result<OsmData> {
    let reader = ElementReader::from_path(path)?;

    let partial: Partial = reader.par_map_reduce(
        process_element,
        Partial::default,
        Partial::merge,
    )?;

    let Partial { mut nodes, mut ways, restrictions } = partial;

    info!(
        nodes        = nodes.len(),
        ways         = ways.len(),
        restrictions = restrictions.len(),
        "OSM PBF loaded"
    );

    if let Some(b) = bbox {
        // Find nodes inside bbox, then keep ways that touch the bbox.
        let bbox_node_set: HashSet<i64> = nodes
            .iter()
            .filter(|(_, c)| c.lon >= b.west && c.lon <= b.east && c.lat >= b.south && c.lat <= b.north)
            .map(|(id, _)| *id)
            .collect();

        ways.retain(|w| w.node_ids.iter().any(|id| bbox_node_set.contains(id)));

        // Keep all nodes referenced by kept ways (some may lie just outside the bbox).
        let referenced: HashSet<i64> = ways.iter()
            .flat_map(|w| w.node_ids.iter().copied())
            .collect();
        nodes.retain(|id, _| referenced.contains(id));

        info!(nodes = nodes.len(), ways = ways.len(), "after bbox filter");
    }

    Ok(OsmData { ways, nodes, restrictions })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_coord(lon: f64, lat: f64) -> OsmNodeCoord {
        OsmNodeCoord { lon, lat }
    }

    #[test]
    fn bbox_filter_keeps_ways_with_any_node_inside() {
        let bbox = Bbox { west: 170.0, south: -40.0, east: 175.0, north: -35.0 };
        let mut nodes = HashMap::new();
        nodes.insert(1, make_coord(172.0, -38.0)); // inside
        nodes.insert(2, make_coord(180.0, -38.0)); // outside
        nodes.insert(3, make_coord(173.0, -39.0)); // inside
        nodes.insert(4, make_coord(169.0, -38.0)); // outside

        let mut ways = vec![
            OsmWay { id: 10, node_ids: vec![1, 2], tags: [("highway".to_string(), "primary".to_string())].into() },
            OsmWay { id: 11, node_ids: vec![2, 4], tags: [("highway".to_string(), "primary".to_string())].into() },
            OsmWay { id: 12, node_ids: vec![1, 3], tags: [("highway".to_string(), "secondary".to_string())].into() },
        ];

        let bbox_node_set: std::collections::HashSet<i64> = nodes.iter()
            .filter(|(_, c)| c.lon >= bbox.west && c.lon <= bbox.east && c.lat >= bbox.south && c.lat <= bbox.north)
            .map(|(id, _)| *id)
            .collect();
        ways.retain(|w| w.node_ids.iter().any(|id| bbox_node_set.contains(id)));

        // Ways 10 and 12 have node 1 (inside bbox); way 11 has no nodes in bbox.
        assert_eq!(ways.len(), 2);
        assert!(ways.iter().any(|w| w.id == 10));
        assert!(ways.iter().any(|w| w.id == 12));
        assert!(ways.iter().all(|w| w.id != 11));
    }

    #[test]
    fn bbox_filter_retains_boundary_crossing_nodes() {
        // Way with one node inside, one outside: both nodes must be kept after filter.
        let bbox = Bbox { west: 170.0, south: -40.0, east: 175.0, north: -35.0 };
        let mut nodes = HashMap::new();
        nodes.insert(1, make_coord(172.0, -38.0)); // inside
        nodes.insert(2, make_coord(180.0, -38.0)); // outside

        let ways = vec![
            OsmWay { id: 10, node_ids: vec![1, 2], tags: [("highway".to_string(), "primary".to_string())].into() },
        ];

        let bbox_node_set: HashSet<i64> = nodes.iter()
            .filter(|(_, c)| c.lon >= bbox.west && c.lon <= bbox.east && c.lat >= bbox.south && c.lat <= bbox.north)
            .map(|(id, _)| *id)
            .collect();

        let kept_ways: Vec<_> = ways.iter()
            .filter(|w| w.node_ids.iter().any(|id| bbox_node_set.contains(id)))
            .collect();
        let referenced: HashSet<i64> = kept_ways.iter()
            .flat_map(|w| w.node_ids.iter().copied())
            .collect();
        nodes.retain(|id, _| referenced.contains(id));

        assert_eq!(nodes.len(), 2, "boundary node 2 must be retained");
        assert!(nodes.contains_key(&1));
        assert!(nodes.contains_key(&2));
    }
}
