/// DuckDB-backed canonical-ingestion pipeline, activated by `build --canonical-db <path>`.
///
/// Unlike every other low-memory path, there is no extraction step at all here —
/// an external producer (a SQL transform, or any language with DuckDB bindings)
/// has already populated `canonical_edges` / `canonical_nodes` /
/// `canonical_restrictions` in the given DuckDB file, per
/// `pipeline/schema/canonical_schema.sql`. This module attaches that database
/// read-only, transforms its rows into the shared `q_edges` / `q_nodes` /
/// `restriction_triples` scratch schema, and hands off to the shared
/// `lowmem_tile::tile_from_duckdb`.
///
/// Producer ids are opaque UTF-8 strings up to 255 bytes (see the schema file),
/// not necessarily integers or 32-hex-digit UUIDs the way OSM/Overture ids are.
/// The internal 16-byte dedup key used everywhere else in this pipeline can't
/// carry an arbitrary string losslessly, so it's derived here via MD5
/// (`unhex(md5(id))`, computed in DuckDB) purely as a join/dedup key — the real
/// string is carried separately through the `stable_id` column and is what
/// actually gets persisted in the tile, so this hash never touches Invariant 2.
/// Two different producer ids hashing to the same digest is checked for and
/// hard-errors rather than silently merging two segments/nodes.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use duckdb::{params, Connection};
use tracing::info;

use crate::lowmem_tile::{geom_to_blob, make_bar, remove_collinear_lm, tile_from_duckdb};
use crate::partition::available_ram_bytes;
use crate::quantize::quantize_coord;
use crate::restrictions::{encode_restriction_flags, HEADING_ANY, HEADING_BACKWARD, HEADING_FORWARD};
use crate::split::polyline_length_m;
use crate::tile::lon_lat_to_tile_xy;

// ── Direction / heading vocabulary ────────────────────────────────────────────

/// canonical_edges.direction ('fwd'/'rev'/'both') -> the 0=Both/1=Forward/2=Backward
/// convention used throughout the rest of this pipeline (see WayRecord in
/// osm_low_memory.rs and tile::pack_attrs).
fn map_direction(direction: &str) -> Result<u8> {
    match direction {
        "both" => Ok(0),
        "fwd"  => Ok(1),
        "rev"  => Ok(2),
        other  => anyhow::bail!("invalid canonical_edges.direction value: {other:?} (expected fwd/rev/both)"),
    }
}

/// canonical_restrictions.{from,to}_heading ('fwd'/'rev'/'both') -> HEADING_* bits.
fn map_heading(heading: &str) -> Result<u8> {
    match heading {
        "both" => Ok(HEADING_ANY),
        "fwd"  => Ok(HEADING_FORWARD),
        "rev"  => Ok(HEADING_BACKWARD),
        other  => anyhow::bail!("invalid canonical_restrictions heading value: {other:?} (expected fwd/rev/both)"),
    }
}

// ── WKT parsing ───────────────────────────────────────────────────────────────

/// Parse a WKT `LINESTRING (lon lat, lon lat, ...)` into vertex pairs.
/// canonical_schema.sql requires at least 2 points; that's checked by the caller.
fn parse_wkt_linestring(s: &str) -> Result<Vec<(f64, f64)>> {
    let inner = s
        .trim()
        .strip_prefix("LINESTRING")
        .map(str::trim)
        .and_then(|s| s.strip_prefix('('))
        .and_then(|s| s.strip_suffix(')'))
        .with_context(|| format!("not a WKT LINESTRING: {s:?}"))?;

    inner
        .split(',')
        .map(|pair| {
            let mut it = pair.trim().split_whitespace();
            let lon: f64 = it
                .next()
                .with_context(|| format!("missing longitude in {pair:?}"))?
                .parse()
                .with_context(|| format!("bad longitude in {pair:?}"))?;
            let lat: f64 = it
                .next()
                .with_context(|| format!("missing latitude in {pair:?}"))?
                .parse()
                .with_context(|| format!("bad latitude in {pair:?}"))?;
            Ok((lon, lat))
        })
        .collect()
}

// ── Hash-collision guard ──────────────────────────────────────────────────────

/// Tracks which producer id string a given 16-byte MD5 digest belongs to.
/// A second, different string hashing to the same digest hard-errors instead
/// of silently merging two distinct segments/nodes — see module doc comment.
#[derive(Default)]
struct KeyRegistry(HashMap<[u8; 16], String>);

impl KeyRegistry {
    fn check(&mut self, key: [u8; 16], id: &str) -> Result<()> {
        match self.0.get(&key) {
            Some(existing) if existing != id => anyhow::bail!(
                "MD5 key collision: producer ids {existing:?} and {id:?} both hash to {}. \
                 This is astronomically unlikely by chance — check for a producer bug \
                 before assuming it's a genuine collision.",
                hex::encode(key)
            ),
            _ => {
                self.0.insert(key, id.to_string());
                Ok(())
            }
        }
    }
}

fn blob_to_key(blob: &[u8]) -> [u8; 16] {
    blob.try_into().expect("MD5 digest is always 16 bytes")
}

// ── DuckDB setup ──────────────────────────────────────────────────────────────

fn setup_duckdb(canonical_db_path: &Path, memory_mb_override: Option<u64>, temp_dir: &Path) -> Result<Connection> {
    let limit_mb = match memory_mb_override {
        Some(mb) => mb,
        None => {
            let avail = available_ram_bytes();
            ((avail as f64 * 0.40) / 1_048_576.0) as u64
        }
    }
    .max(1_024);

    std::fs::create_dir_all(temp_dir).context("create DuckDB temp dir")?;
    let db_file = temp_dir.join("pipeline.duckdb");
    let conn = Connection::open(&db_file).context("open DuckDB")?;

    conn.execute_batch(&format!(
        "PRAGMA threads={threads}; \
         SET memory_limit='{limit_mb}MB'; \
         SET preserve_insertion_order=false; \
         CREATE TABLE q_edges ( \
             edge_idx INTEGER, split_idx INTEGER, \
             start_id BLOB, end_id BLOB, parent_id BLOB, \
             geom_blob BLOB, length_cm INTEGER, \
             frc INTEGER, fow INTEGER, direction INTEGER, \
             tile_x INTEGER, tile_y INTEGER, tile_id BIGINT, stable_id VARCHAR); \
         CREATE TABLE q_nodes (node_id BLOB, lon_e7 INTEGER, lat_e7 INTEGER, tile_x INTEGER, tile_y INTEGER, stable_id VARCHAR); \
         CREATE TABLE restriction_triples (from_id BLOB, via_id BLOB, to_id BLOB, flags INTEGER);",
        threads = rayon::current_num_threads().min(8),
    ))
    .context("DuckDB scratch schema")?;

    let canonical_path_str = canonical_db_path
        .to_str()
        .context("canonical DB path is not valid UTF-8")?;
    conn.execute_batch(&format!(
        "ATTACH '{}' AS src (READ_ONLY);",
        canonical_path_str.replace('\'', "''"),
    ))
    .with_context(|| format!("attach canonical DB {}", canonical_db_path.display()))?;

    info!(limit_mb, canonical_db = %canonical_db_path.display(), "DuckDB scratch database ready");
    Ok(conn)
}

// ── Phase 1: nodes ────────────────────────────────────────────────────────────

fn extract_nodes(conn: &Connection, tile_zoom: u8, show_progress: bool) -> Result<()> {
    let node_count: i64 = conn.query_row("SELECT COUNT(*) FROM src.canonical_nodes", [], |r| r.get(0))?;
    let pb = make_bar(show_progress, node_count.max(0) as u64, "Canonical nodes         ");

    let mut registry = KeyRegistry::default();
    let mut node_app = conn.appender("q_nodes").context("appender q_nodes")?;
    let mut stmt = conn
        .prepare("SELECT id, lon, lat, unhex(md5(id)) FROM src.canonical_nodes")
        .context("prepare canonical_nodes scan")?;
    let mut rows = stmt.query([]).context("query canonical_nodes")?;

    let mut n = 0usize;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let lon: f64 = row.get(1)?;
        let lat: f64 = row.get(2)?;
        let key_blob: Vec<u8> = row.get(3)?;
        let key = blob_to_key(&key_blob);
        registry.check(key, &id)?;

        let lon_e7 = quantize_coord(lon);
        let lat_e7 = quantize_coord(lat);
        let (tx, ty) = lon_lat_to_tile_xy(lon, lat, tile_zoom);
        node_app
            .append_row(params![&key[..], lon_e7, lat_e7, tx as i64, ty as i64, id.as_str()])
            .context("append q_node")?;
        n += 1;
        pb.inc(1);
    }
    node_app.flush().context("flush q_nodes")?;
    pb.finish_and_clear();
    info!(nodes = n, "canonical nodes extracted");
    Ok(())
}

// ── Phase 2: edges ────────────────────────────────────────────────────────────

fn extract_edges(conn: &Connection, tile_zoom: u8, show_progress: bool) -> Result<()> {
    let edge_count: i64 = conn.query_row("SELECT COUNT(*) FROM src.canonical_edges", [], |r| r.get(0))?;
    let pb = make_bar(show_progress, edge_count.max(0) as u64, "Canonical edges         ");

    let mut registry = KeyRegistry::default();
    let mut edge_app = conn.appender("q_edges").context("appender q_edges")?;
    let mut stmt = conn
        .prepare(
            "SELECT id, geometry, frc::BIGINT, fow::BIGINT, direction, \
                    unhex(md5(id)), unhex(md5(start_node_id)), unhex(md5(end_node_id)) \
             FROM src.canonical_edges",
        )
        .context("prepare canonical_edges scan")?;
    let mut rows = stmt.query([]).context("query canonical_edges")?;

    let mut edge_idx: u32 = 0;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let geometry: String = row.get(1)?;
        let frc: u8 = row.get::<_, i64>(2)? as u8;
        let fow: u8 = row.get::<_, i64>(3)? as u8;
        let direction_str: String = row.get(4)?;
        let parent_blob: Vec<u8> = row.get(5)?;
        let start_blob:  Vec<u8> = row.get(6)?;
        let end_blob:    Vec<u8> = row.get(7)?;

        let parent_id = blob_to_key(&parent_blob);
        let start_id  = blob_to_key(&start_blob);
        let end_id    = blob_to_key(&end_blob);
        registry.check(parent_id, &id)?;

        let direction = map_direction(&direction_str)?;

        let float_geom = parse_wkt_linestring(&geometry)
            .with_context(|| format!("edge {id:?} has invalid geometry"))?;
        anyhow::ensure!(float_geom.len() >= 2, "edge {id:?} geometry has fewer than 2 points");

        let length_m  = polyline_length_m(&float_geom);
        let length_cm = (length_m * 100.0).round() as u32;

        let q_geom_raw: Vec<(i32, i32)> = float_geom
            .iter()
            .map(|&(lon, lat)| (quantize_coord(lon), quantize_coord(lat)))
            .collect();
        let q_geom = remove_collinear_lm(q_geom_raw);
        anyhow::ensure!(q_geom.len() >= 2, "edge {id:?} degenerate after collinear removal");
        let geom_blob = geom_to_blob(&q_geom);

        let (slon, slat) = float_geom[0];
        let (elon, elat) = *float_geom.last().unwrap();
        let (stx, sty) = lon_lat_to_tile_xy(slon, slat, tile_zoom);
        let (etx, ety) = lon_lat_to_tile_xy(elon, elat, tile_zoom);

        // Canonical edges arrive already fully split (Invariant 1 is the
        // producer's responsibility, per canonical_schema.sql), so — exactly
        // like the generic GeoJSONL path — split_idx is always 0 and the
        // persisted stable_id is just the producer's own id, unmodified.
        edge_app
            .append_row(params![
                edge_idx as i64, 0i64,
                &start_id[..], &end_id[..], &parent_id[..],
                geom_blob.as_slice(), length_cm as i64,
                frc as i64, fow as i64, direction as i64,
                stx as i64, sty as i64,
                crate::tile::xyz_to_tile_id(tile_zoom, stx, sty) as i64,
                id.as_str(),
            ])
            .context("append q_edge start-tile")?;
        if (etx, ety) != (stx, sty) {
            edge_app
                .append_row(params![
                    edge_idx as i64, 0i64,
                    &start_id[..], &end_id[..], &parent_id[..],
                    geom_blob.as_slice(), length_cm as i64,
                    frc as i64, fow as i64, direction as i64,
                    etx as i64, ety as i64,
                    crate::tile::xyz_to_tile_id(tile_zoom, etx, ety) as i64,
                    id.as_str(),
                ])
                .context("append q_edge end-tile")?;
        }
        edge_idx += 1;
        pb.inc(1);
    }
    edge_app.flush().context("flush q_edges")?;
    pb.finish_and_clear();
    info!(edges = edge_idx, "canonical edges extracted");
    Ok(())
}

// ── Phase 3: restrictions ─────────────────────────────────────────────────────

fn load_restrictions(conn: &Connection) -> Result<usize> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM src.canonical_restrictions", [], |r| r.get(0))?;
    if count == 0 {
        return Ok(0);
    }

    let mut stmt = conn
        .prepare(
            "SELECT unhex(md5(from_id)), unhex(md5(via_id)), unhex(md5(to_id)), from_heading, to_heading \
             FROM src.canonical_restrictions",
        )
        .context("prepare canonical_restrictions scan")?;
    let mut rows = stmt.query([]).context("query canonical_restrictions")?;
    let mut insert = conn
        .prepare("INSERT INTO restriction_triples VALUES (?, ?, ?, ?)")
        .context("prepare INSERT restriction_triples")?;

    let mut n = 0usize;
    while let Some(row) = rows.next()? {
        let from_blob: Vec<u8> = row.get(0)?;
        let via_blob:  Vec<u8> = row.get(1)?;
        let to_blob:   Vec<u8> = row.get(2)?;
        let from_heading: String = row.get(3)?;
        let to_heading:   String = row.get(4)?;

        let flags = encode_restriction_flags(map_heading(&from_heading)?, map_heading(&to_heading)?);
        insert
            .execute(params![from_blob, via_blob, to_blob, flags as i64])
            .context("insert restriction_triple")?;
        n += 1;
    }
    Ok(n)
}

// ── Public entry point ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_pipeline(
    canonical_db_path: &Path,
    output_dir:        &Path,
    extent_slug:       &str,
    release_label:     &str,
    tile_zoom:         u8,
    duckdb_memory_mb:  Option<u64>,
    duckdb_temp_dir:   Option<&Path>,
    show_progress:     bool,
    compress:          bool,
) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let default_tmp = output_dir.join(format!(".duckdb_tmp_{}", std::process::id()));
    let temp_dir    = duckdb_temp_dir.unwrap_or(&default_tmp);
    let _tmp_guard = duckdb_temp_dir.is_none().then(|| crate::build::TempDirGuard(default_tmp.clone()));

    let conn = setup_duckdb(canonical_db_path, duckdb_memory_mb, temp_dir)?;

    info!("canonical: extracting nodes");
    extract_nodes(&conn, tile_zoom, show_progress)?;

    info!("canonical: extracting edges");
    extract_edges(&conn, tile_zoom, show_progress)?;

    info!("canonical: loading restrictions");
    let n = load_restrictions(&conn)?;
    info!(restrictions = n, "restrictions loaded");

    info!("canonical: tiling");
    tile_from_duckdb(&conn, tile_zoom, output_dir, extent_slug, release_label, show_progress, compress)?;

    drop(conn);
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_linestring() {
        let pts = parse_wkt_linestring("LINESTRING (4.9 51.4, 4.91 51.41)").unwrap();
        assert_eq!(pts, vec![(4.9, 51.4), (4.91, 51.41)]);
    }

    #[test]
    fn parses_linestring_with_many_points_and_negatives() {
        let pts = parse_wkt_linestring("LINESTRING (-4.9 -51.4, 0 0, 4.91 51.41)").unwrap();
        assert_eq!(pts, vec![(-4.9, -51.4), (0.0, 0.0), (4.91, 51.41)]);
    }

    #[test]
    fn rejects_non_linestring_wkt() {
        assert!(parse_wkt_linestring("POINT (4.9 51.4)").is_err());
        assert!(parse_wkt_linestring("not wkt at all").is_err());
        assert!(parse_wkt_linestring("LINESTRING (4.9)").is_err());
    }

    #[test]
    fn direction_mapping_matches_pack_attrs_convention() {
        // 0=Both 1=Forward 2=Backward, matching tile::pack_attrs / WayRecord.
        assert_eq!(map_direction("both").unwrap(), 0);
        assert_eq!(map_direction("fwd").unwrap(), 1);
        assert_eq!(map_direction("rev").unwrap(), 2);
        assert!(map_direction("forward").is_err(), "old pre-schema-update vocabulary must not silently work");
    }

    #[test]
    fn heading_mapping_matches_restriction_flag_bits() {
        assert_eq!(map_heading("both").unwrap(), HEADING_ANY);
        assert_eq!(map_heading("fwd").unwrap(), HEADING_FORWARD);
        assert_eq!(map_heading("rev").unwrap(), HEADING_BACKWARD);
        assert!(map_heading("any").is_err(), "the dropped 'any' vocabulary must not silently work");
    }

    #[test]
    fn key_registry_allows_repeated_lookups_of_the_same_id() {
        let mut reg = KeyRegistry::default();
        let key = [1u8; 16];
        reg.check(key, "same-id").unwrap();
        reg.check(key, "same-id").unwrap();
        reg.check(key, "same-id").unwrap();
    }

    #[test]
    fn key_registry_hard_errors_on_collision() {
        // Can't construct a real MD5 collision, so test the guard logic directly:
        // two different ids must never be allowed to share one internal key.
        let mut reg = KeyRegistry::default();
        let key = [7u8; 16];
        reg.check(key, "id-a").unwrap();
        let err = reg.check(key, "id-b");
        assert!(err.is_err(), "two different producer ids must never silently share a dedup key");
    }
}
