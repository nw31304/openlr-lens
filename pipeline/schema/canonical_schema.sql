-- OpenLRLens canonical ingestion schema.
--
-- This is the interchange contract between a format-specific producer (a SQL
-- transform, or a program in any language with DuckDB bindings) and the
-- openlr-lens pipeline binary. A producer populates these three tables in a
-- DuckDB database file; the pipeline's "ingest from existing DuckDB" mode
-- reads them directly and runs the same split/quantize/tile logic used by
-- every other source (OSM, Overture, generic GeoJSONL).
--
-- Preconditions a producer MUST satisfy before writing rows:
--   * The road network is already split at every interior junction — one row
--     in canonical_edges per final node-to-node graph edge, not per raw
--     source way/segment that may span multiple junctions. (Sources that
--     cannot guarantee this, like raw OSM ways, are handled by the
--     pipeline's own native importers instead of this path.)
--   * Only vehicular, routable segments are present — non-vehicular segments
--     (footpaths, cycleways, etc.) must already be filtered out.
--   * All three tables must exist, even if canonical_restrictions is empty.
--
-- IDs are opaque UTF-8 strings, never surrogate/sequential integers (see
-- CLAUDE.md Invariant 2: stable IDs must be deterministic and derived from
-- the source data, never build/row order). Use whatever the source format's
-- own persistent identifier is — an OSM node/way id, a MultiNet-R FEAT_ID, a
-- database primary key — as long as it is stable across rebuilds of the same
-- source data. IDs are stored in the tile's string pool with a 1-byte length
-- prefix, so every id MUST be at most 255 bytes (UTF-8 byte length, not
-- character count) — enforced below.

CREATE TABLE IF NOT EXISTS canonical_nodes (
    -- Persistent, source-defined identifier. Becomes the tile's stable_id
    -- for this node. Never a hash, never a row number.
    id  TEXT NOT NULL PRIMARY KEY CHECK (octet_length(encode(id)) BETWEEN 1 AND 255),
    lon DOUBLE NOT NULL,  -- WGS84 degrees
    lat DOUBLE NOT NULL   -- WGS84 degrees
);

CREATE TABLE IF NOT EXISTS canonical_edges (
    -- Persistent, source-defined identifier for this final (already-split)
    -- edge. Becomes the tile's stable_id for this segment.
    id             TEXT NOT NULL PRIMARY KEY CHECK (octet_length(encode(id)) BETWEEN 1 AND 255),

    -- Foreign keys into canonical_nodes.id — NOT surrogate integers.
    start_node_id  TEXT NOT NULL REFERENCES canonical_nodes(id),
    end_node_id    TEXT NOT NULL REFERENCES canonical_nodes(id),

    -- WGS84 LineString as WKT text, e.g. "LINESTRING (lon lat, lon lat, ...)".
    -- At least 2 points. First point must equal the start node's coordinate,
    -- last point must equal the end node's coordinate (within source
    -- precision) — the pipeline does not re-snap geometry to node coords.
    -- Full fidelity only: no simplification beyond exact-collinear removal,
    -- which the pipeline itself performs downstream (Invariant 4).
    geometry       TEXT NOT NULL,

    -- OpenLR Functional Road Class, 0 (Motorway) .. 7 (Other/Local).
    frc            UTINYINT NOT NULL CHECK (frc BETWEEN 0 AND 7),

    -- OpenLR Form Of Way: 0=Undefined 1=Motorway 2=MultipleCarriageway
    -- 3=SingleCarriageway 4=Roundabout 5=TrafficSquare 6=SlipRoad 7=Other.
    fow            UTINYINT NOT NULL CHECK (fow BETWEEN 0 AND 7),

    -- Direction of legal travel relative to the geometry's own vertex order
    -- (start_node_id -> end_node_id is 'fwd').
    direction      TEXT NOT NULL CHECK (direction IN ('fwd', 'rev', 'both'))

    -- No length column: the pipeline always computes edge length itself from
    -- `geometry`, for consistency with every other source rather than trusting
    -- a producer-supplied value that may use a different great-circle model.
);

CREATE TABLE IF NOT EXISTS canonical_restrictions (
    -- A turn restriction: travel from `from_id` through node `via_id` onto
    -- `to_id` is prohibited. Every id must resolve within this same dataset —
    -- from_id/to_id into canonical_edges.id, via_id into canonical_nodes.id,
    -- and via_id must equal an endpoint shared by both edges (from_id's
    -- end_node_id / to_id's start_node_id in the common case).
    from_id       TEXT NOT NULL REFERENCES canonical_edges(id),
    via_id        TEXT NOT NULL REFERENCES canonical_nodes(id),
    to_id         TEXT NOT NULL REFERENCES canonical_edges(id),

    -- Optional direction constraints, using the same vocabulary as
    -- canonical_edges.direction. 'both' (default) means the restriction
    -- applies regardless of which direction the segment is traversed —
    -- correct for one-way segments and the common case.
    from_heading  TEXT NOT NULL DEFAULT 'both' CHECK (from_heading IN ('fwd', 'rev', 'both')),
    to_heading    TEXT NOT NULL DEFAULT 'both' CHECK (to_heading   IN ('fwd', 'rev', 'both'))
);
