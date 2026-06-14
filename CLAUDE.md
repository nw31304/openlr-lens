# CLAUDE.md — OpenLRLens

> A browser-based, WebAssembly OpenLR **diagnostic decoder** (encoder
> stubbed for now) with **global** coverage, backed by a preprocessed, version-pinned
> map store derived from Overture Maps. The headline differentiator is not "another
> decoder" — it is the ability to explain **why** a reference decodes the way it does,
> including a verdict on whether a misdecode is decoder-tunable or a deficiency in the
> encoded reference itself.

This file is the authoritative context for Claude Code. Read the **Critical Invariants**
section before writing any code; several of the constraints here fail *silently* (wrong
output, not a crash) if ignored.

---

## 1. What this is

OpenLRLens decodes OpenLR location references against a global road network and visually
diagnoses the result. It runs entirely client-side: a Rust core compiled to WASM does the
codec + graph + search work; a JS/MapLibre front end drives it and renders the diagnostics.
The map data is **not** queried live — it is preprocessed once per adopted Overture release
into a static, edge-cached tile store. See §3 for why.

Two OpenLR physical formats are supported on the **decode** path from day one:
- **OpenLR binary v3** (the "TomTom" binary format) — quantizes bearing (5-bit, 32 × 11.25°
  sectors) and distance (1-byte DNP, ~58.6 m buckets over a 15 km range).
- **TISA / TPEG-OLR** (ISO 21219-22) — represents bearing and distance at full precision.

The **encode** path (producing binary strings) is **out of scope for v1** but must be
present as traits/stubs so it can be filled in later without refactoring (see §7).

---

## 2. Critical Invariants (read first — these fail silently)

1. **Split Overture segments at *every* interior connector during the build.** Overture
   segments may carry connectors at interior positions, not just endpoints. The runtime
   graph model is strictly node-to-node edges with a `start_node` and `end_node`. If you do
   not split at interior connectors, real junctions silently vanish from the graph and A*
   routes around junctions that exist. This passes casual testing in sparse rural areas and
   fails in dense urban ones.

2. **Anchor all segment and node identifiers to stable, deterministic ids (derived from
   Overture/GERS), never to build/row order.** The turn-restriction table and cross-tile
   stitching both reference segments/nodes by id; a later rebuild must produce the same ids
   or every restriction and boundary link breaks. Determinism here is what makes turn
   restrictions and future rebuilds cheap.

3. **A* node state is `(node, incoming_segment)` from day one**, even though turn
   restrictions are applied via a check that may initially be permissive. Retrofitting this
   state into a plain-node search later is surgery on the most correctness-critical code.
   The closed/visited set is keyed on the pair, not the bare node.

4. **Store geometry at full fidelity — do NOT lossily simplify.** Two decode-time consumers both
   read the stored geometry: bearing is *derived* from it over any 20 m window (an LRP can
   project anywhere along a segment, §8), and the decoded path is *drawn over an OSM-derived
   slippy basemap*, so it must overlay the rendered roads. Lossy simplification (Douglas-Peucker
   et al.) breaks both at once — it permanently floors bearing precision and makes the highlighted
   path visibly cut corners off the road, worst exactly when the user zooms in to diagnose. The
   only thing it buys is storage, which is ~$1/month for the entire planet, so the trade is
   lopsided. Crucially, the **only** overlay divergence you want to see is *genuine map
   divergence* (the signal τ absorbs and the diagnostic exists to surface); self-inflicted
   simplification error is noise contaminating that very measurement. Therefore: keep geometry at
   source fidelity. The only reduction allowed is **lossless removal of exactly-collinear
   vertices** (zero heading change → identical line, no cost). The single build-time precision
   knob is **coordinate quantization**: make the tile-local grid fine enough to be sub-pixel at
   max display zoom (sub-meter is a safe absolute proxy) — well under both bearing and overlay
   needs. (This supersedes any earlier "~2–3° simplification bound"; with no lossy simplification
   there is no simplification floor, and 11.25° was only ever the v3 bucket width, not a geometry
   budget.)

5. **The match window is `encoding_interval ⊕ map_tolerance`, and the map tolerance term is
   mandatory.** The `[LB, UB]` interval captures *encoding* quantization (wide for v3, zero
   for TPEG). A separate, decode-time-configurable tolerance `τ` captures *map divergence*.
   For TPEG (`LB == UB`) the bare interval is a point; without `τ` the decoder demands an
   exact match its own map cannot reproduce and rejects every real candidate.

6. **Bearing intervals are circular (mod 360°); distance intervals are linear.** Containment
   and overlap tests for bearing must handle wraparound. Build one circular-interval type and
   use it only for bearing.

7. **The cost function must stay additive and decomposable** per term and per LRP/edge. The
   "explain why" diagnostic attributes a score gap to specific terms at specific LRPs; a
   non-additive cost destroys explainability.

8. **License compliance is a build-time obligation, not an afterthought.** Overture
   transportation is OSM-derived (ODbL). See §13.

9. **FRC fetch coverage is bounded by per-leg LFRCNP, not by the LRP's candidate tolerance.**
   The candidate-FRC tolerance (`±t` around an LRP's FRC) governs which lines may *match an LRP*;
   it does **not** bound which FRCs the connecting route uses. The route between two LRPs can dip
   to that leg's LFRCNP (lowest / least-important FRC on the path), which may sit well below the
   candidate band. So the segments available to A* for a leg must cover bands from the top down
   to LFRCNP; fetching only `[frc−t, frc+t]` silently drops the low-FRC connectors (ramps, links)
   the route needs and A* fails to find the path. In v1 this is automatic (every tile carries all
   FRCs); it becomes a live constraint only if/when FRC stratification is introduced (see §5).

---

## 3. Architecture

Three layers, decoupled. The map store is the only thing that ever touched a server, and
it does so only at build time.

```
            BUILD TIME (run a few times/year, per adopted Overture release)
  Overture transportation parquet ──▶ [Offline Build Pipeline] ──▶ PMTiles archive ──▶ R2 + CDN
                                          (extract, adapt, split,
                                           assign stable ids, tile)

            RUNTIME (entirely in the browser, no server, no live queries)
  PMTiles on R2/CDN ──range reads──▶ [Tile Reader] ──▶ [OpenLRDataProvider] ──▶ in-memory graph
                                                              │
   OpenLR string ──▶ [Codec: v3 / TPEG decode] ──▶ unified LRP model ([LB,UB] intervals)
                                                              │
                                                       [Decode Engine: candidate
                                                        selection + A* + validation]
                                                              │
                                          [Diagnostics + MapLibre UI]
```

### Why preprocessed, not live Overture
Coverage is **global**. The schema adapter (Overture rules → FRC/FOW/direction/turn
restrictions, plus interior-connector splitting) has to run *somewhere*; at global scale it
must run **once at build time over the planet**, not per-decode in every browser against raw
450 MB+ parquet. Live-direct also has no edge cache (single us-west-2 bucket → global users),
and a live-"latest" map shifts monthly, which is fatal for a *diagnostic* tool that must be
deterministic and reproducible. The preprocessing cost is small: the transportation segment
theme is ~46 GB of source parquet (vs. 363 GB buildings we never touch); the lean derived
store is well under that; R2 storage runs ~$1/month; the build is an I/O-bound batch job of a
few hours on a laptop, with free egress from Overture's AWS Open Data buckets.

**Consequence for I/O and CORS:** only the **preprocessor** ever reads Overture, and it runs
natively (DuckDB/Rust) — browser CORS does not apply to native clients, so Overture's bucket
CORS is irrelevant to the build. At runtime the browser reads **only** the R2 PMTiles store,
whose CORS you configure yourself. So the core architecture never depends on browser access to
Overture; the one place that would is the optional `LiveOvertureProvider` below, which is not
on the v1 path.

### Decoupling: the `OpenLRDataProvider` trait
All map access goes through one async trait so the engine is storage-agnostic:

```rust
#[async_trait]
pub trait OpenLRDataProvider {
    /// Segments whose geometry comes within `radius_m` of (lat, lon). Coarse bbox prune in
    /// the provider; exact distance filtering happens in the engine.
    async fn segments_near(&self, lat: f64, lon: f64, radius_m: f64) -> SpatialMapChunk;
    /// Resolve a segment by stable id (for cross-tile expansion / boundary stitching).
    async fn segment_by_id(&self, id: SegmentId) -> Option<NetworkSegment>;
}
```

- **Primary driver (v1):** `PmtilesProvider` — reads the preprocessed PMTiles store.
- **Optional secondary driver (later, not v1):** `LiveOvertureProvider` — range-reads
  Overture parquet directly *from the browser* for ad-hoc/fresh spot-checks. This is the **only**
  component for which browser-CORS-against-Overture matters; the core architecture does not
  depend on it. If browser-direct Overture access is never wanted, this driver can simply be
  dropped. (For the record: CORS was confirmed feasible during design via a browser-console
  `fetch()` against a `theme=transportation/type=segment` object — GET+Range returned `206` with
  a readable body, HEAD returned `206` with a readable `Content-Length` — but **re-confirm before
  relying on it**, as bucket CORS config can change.)
- **Enterprise escape hatch (not v1):** a remote/gRPC driver. Never the default.

### Crossing the WASM boundary: who does the I/O
`wasm32-unknown-unknown` has no native HTTP, so the provider cannot fetch on its own. The
recommended split: **JS owns all I/O** — the PMTiles range reads and HTTP fetches live in JS;
the Rust/WASM side operates over an **in-memory tile cache** that JS populates. Concretely,
the decode loop is steppable (§12): when the engine needs a tile it isn't holding, it yields a
**tile-key request** to JS; JS fetches the tile bytes (async, via the PMTiles reader) and
resumes the engine with the bytes injected. This keeps the Rust `OpenLRDataProvider`
implementation **synchronous over the cache** and avoids `async-trait` across the FFI boundary
and `web_sys::fetch` inside Rust. (The async signatures above describe the *logical* contract;
the WASM realization fulfills them through the JS-driven request/resume protocol, not by
awaiting inside Rust.) Alternative, if a fully self-contained WASM provider is ever wanted:
call `web_sys::fetch` via `wasm-bindgen-futures` — but that couples the core to web APIs and is
not the v1 path.

---

## 4. Data model

### Segment (runtime edge — post-split, node-to-node)
Logical fields required by OpenLR:
- `geometry`: ordered WGS84 vertices (LineString), stored at **source fidelity** (no lossy
  simplification, Invariant 4). On disk: tile-local quantized/delta-coded coordinates in a
  per-tile geometry pool; quantization grid must satisfy Invariant 4 (sub-pixel at max display
  zoom).
- `start_node_id`, `end_node_id`: connectivity (see node table).
- `length_m`: precomputed and stored — carry Overture's length (matches the encoder's notion and
  avoids drift from recomputing over quantized coordinates). With lossy simplification dropped
  (Invariant 4), there's no simplification floor to dodge here anymore; storing the scalar is
  still preferred so DNP validation uses one canonical length rather than re-deriving it. Same
  v3-bucket / TPEG-exact distinction as DNP applies at decode time via the `LinearInterval`
  model, not here.
- `frc`: u8, values 0–7 (3 bits used). Derived from Overture `class` (+ `subclass`).
- `fow`: u8, values 0–7 (3 bits used): undefined/motorway/dual/single/roundabout/
  trafficsquare/sliproad/other. Derived from Overture `class`/`subclass`/`road_flags`.
- `direction`: 2 bits — `BOTH` (S↔E) / `FORWARD` (S→E) / `BACKWARD` (S←E). Derived from
  Overture access rules + heading qualifiers.

A representative **fixed 32-byte record** (final layout is the agent's to settle; geometry is
*not* inline — records index into the tile geometry pool):

| field | type | bytes |
|---|---|---|
| start_node | u32 (tile-local) | 4 |
| end_node | u32 (tile-local) | 4 |
| geom_offset | u32 | 4 |
| geom_len | u16 | 2 |
| length_cm | u32 | 4 |
| frc/fow/direction packed | u8 | 1 |
| flags | u8 | 1 |
| reserved | — | 12 |

**Identity (Invariant 2):** the record does **not** carry a global id, and **never a hash** — a
hash can collide and a collision is a silent Invariant-2 violation. A segment's identity inside
a tile is its array index; a node's is its local index. The **lossless** stable global id
(the full GERS id — ~128 bit, so it does *not* fit a `u64`) lives in side tables: a per-tile
segment-id table (`local index → GERS id`) and the node table below. Cross-tile references —
turn restrictions spanning tiles, boundary stitching — use the **global GERS id**; intra-tile
references may use local indices for compactness.

### Node table (per tile)
`local node index → { lat, lon, gers_id }`, where `gers_id` is the full GERS id stored
losslessly (not hashed). Required at least for **boundary** nodes so adjacent tiles resolve to
the same node.

### Turn-restriction table (per tile, separate from segment records)
A turn restriction is a property of a **(from_segment, node, to_segment)** triple — it cannot
live in a per-segment record. Built from Overture `prohibited_transitions`. References use local
indices when all three are in-tile, and **global GERS ids** when the triple crosses a tile
boundary; boundary resolution is the same mechanism as topology.

### Inverted adjacency (built in memory at load)
Overture stores connectivity on the segment side only. After loading a tile region, build
`connector_id → [segment_id]` so traversal from a segment's end node is an O(1) lookup.

---

## 5. Tile / store format (DECISION: PMTiles + custom payload)

- **Container:** a single **PMTiles** archive. Range-addressable, Hilbert-clustered for
  spatial locality, baked-in directory, immutable, CDN-cacheable. Avoids the millions-of-tiny-
  objects problem of raw slippy tiles on R2.
- **Tile payload:** custom binary blob per `z/x/y` tile = `{ header, segment record array,
  geometry pool, node table, turn-restriction table, cross-tile restriction table }`. Not MVT.
  All integers little-endian. Layout:

  ```
  Header (40 bytes)
    magic:               [u8; 4]  = b"OLRL"
    version:             u8       = 1
    flags:               u8       = 0 (reserved)
    _pad:                [u8; 2]
    segment_count:       u32
    node_count:          u32
    restriction_count:   u32      // intra-tile restrictions
    geom_vertex_count:   u32
    xrestriction_count:  u32      // cross-tile restrictions
    _reserved:           [u8; 12]

  Segment array: segment_count × 32 bytes   (layout per §4)

  Geometry pool: geom_vertex_count × 8 bytes
    Each vertex: lon_e7: i32, lat_e7: i32   // WGS84 × 1e7, absolute (not delta-coded in v1)

  Node table: node_count × 28 bytes
    lon_e7:   i32
    lat_e7:   i32
    gers_id:  [u8; 16]   // full GERS UUID, little-endian bytes
    flags:    u8         // bit 0: boundary node (requires cross-tile stitching)
    _pad:     [u8; 3]

  Intra-tile restriction table: restriction_count × 16 bytes
    from_seg:  u32       // local segment index
    via_node:  u32       // local node index
    to_seg:    u32       // local segment index
    flags:     u8        // reserved
    _pad:      [u8; 3]

  Cross-tile restriction table: xrestriction_count × 40 bytes
    from_gers:      [u8; 16]   // GERS id of from-segment (may be in adjacent tile)
    via_node_local: u32        // via-node is always in this tile (restrictions keyed here)
    to_gers:        [u8; 16]   // GERS id of to-segment (may be in adjacent tile)
    flags:          u8
    _pad:           [u8; 3]
  ```

  Coordinate precision: 1e-7 degrees ≈ 1 cm at equator. No delta-coding in v1 (optimize later
  if tile sizes warrant). `geom_len` in the segment record counts **vertices**, not bytes;
  `geom_offset` is a vertex index into the pool (not a byte offset).
- **Single zoom level — NOT a pyramid.** Generate tiles at exactly **one** fixed spatial
  resolution. Do **not** build a multi-zoom z0–zN level-of-detail pyramid. The `z/x/y` here is
  purely the slippy *addressing convention* that lets the client compute a tile key from
  `(lat, lon)` with no server — it is **not** a level-of-detail or road-importance filter. Every
  tile at that single resolution carries **all** road classes (all FRCs) within its cell.
  Decoding always queries at one real-world scale (candidate radius + inter-LRP corridor), so
  only one level is ever read; coarser/finer levels would be storage that is produced and never
  touched.
- **Resolution choice:** default **z12** (~10 km cells), configurable via `--tile-zoom`
  (valid range 8–15). Tune empirically by measuring real fetch sizes against the ≤250 m
  candidate radius. This is a fetch-granularity vs. request-count tradeoff (bigger tiles
  over-fetch a small query; smaller tiles mean more requests and more boundary stitching)
  — nothing to do with road importance. The chosen zoom is recorded in `manifest.json`
  so the browser client knows which level to address.
- **Neighborhood + corridor fetch:** a query usually hits one tile; fetch the **3×3
  neighborhood** around an LRP to cover the bearing window and tile-boundary candidates. Long
  inter-LRP legs (DNP up to ~15 km) are served by fetching **more tiles at the same single
  resolution** along the corridor — never by dropping to a coarse zoom level.
- **FRC is a record field, not the zoom axis.** In v1, FRC is a field on each record
  (algorithmic use) and every tile holds all bands; there is **no** storage stratification. Do
  **not** repurpose the slippy `z` to encode FRC: `z` defines the x/y resolution (2^z tiles per
  side), so it is entangled with spatial granularity and cannot double as an importance band. If
  FRC stratification is added later (v2 optimization), it is an **orthogonal key dimension** — an
  `(frc, x, y)` namespace or one single-zoom archive per band — layered on the single spatial
  resolution, and its per-leg fetch must cover bands down to **LFRCNP**, not just the LRP's
  candidate-FRC tolerance (Invariant 9).
- **Boundary stitching:** stitch adjacent tiles by `stable_global_id` at boundary nodes
  (cheaper than a halo of duplicated segments; halo is an alternative if profiling prefers it).
- **Versioning:** filename carries the Overture release, e.g. `openlrlens-2026-05-20.pmtiles`; a
  tiny `manifest.json` names the current archive. Deploy = upload new archive, flip manifest
  (atomic, reproducible).
- **Basemap is separate:** the multi-zoom tiles MapLibre draws as the visual background are an
  ordinary external basemap source, entirely distinct from this single-resolution
  routing-data archive.

---

## 6. Offline build pipeline — **SCAFFOLD THIS FIRST**

This is milestone one. It produces the data everything else consumes and is independently
testable on a single region.

**Inputs:** a pinned Overture release; `theme=transportation/type=segment` (and `connector` if
needed — connector coordinates are otherwise derivable from segment endpoints); an **extent
specification** and a **schema mapping file** (see below).

### Pipeline CLI flags

| Flag | Default | Description |
|---|---|---|
| `--release <ver>` | (required) | Overture release, e.g. `2026-05-20.0`. Validated against available releases fetched from S3 at startup (see below). |
| `--extent <spec>` | (required) | See §Extent specification below. |
| `--schema <path>` | `pipeline/schema/overture-default.toml` | Overture → OpenLR attribute mapping file. See §Schema mapping file. |
| `--output <dir>` | `./out` | Directory to write the PMTiles archive and manifest into. |

A `list-releases` subcommand (no other flags) queries S3 and prints available releases:

```
openlrlens-build list-releases
openlrlens-build build --release 2026-05-20.0 --extent NZ
```

**Release validation:** at startup, query the Overture S3 bucket for available releases:
```
GET https://overturemaps-us-west-2.s3.us-west-2.amazonaws.com/
    ?list-type=2&prefix=release/&delimiter=/
```
Parse the `<Prefix>` entries from the XML response. If `--release` is given, confirm it appears
in the list and abort with a helpful message if not. `list-releases` just prints the list.

### Extent specification

The pipeline CLI accepts a single `--extent` argument that resolves to a bounding box used as
the spatial filter on the Overture parquet extract. Accepted forms, in order of precedence:

| Form | Example | Notes |
|---|---|---|
| `world` | `--extent world` | No spatial filter; full planet. |
| Continent name | `--extent oceania` | One of: `africa`, `antarctica`, `asia`, `europe`, `north-america`, `oceania`, `south-america`. |
| ISO 3166-1 alpha-2 | `--extent NZ` | Case-insensitive. Resolved to a bbox from a small static lookup table embedded in the binary (~250 entries). |
| Explicit bbox | `--extent 166.0,-47.5,178.5,-34.0` | `west,south,east,north` in WGS84 degrees. Escape hatch for sub-country regions or custom extents. |

Resolution is **bbox only** — polygon-based clipping is not worth the complexity given that
the slight over-fetch for irregularly shaped countries is cheap (Overture's Hilbert-partitioned
parquet skips non-intersecting row-groups, so the filter is efficient regardless).

The resolved bbox is passed directly as a spatial predicate to DuckDB (or the Rust parquet
reader) and is also embedded in the manifest and in the output archive filename:
`openlrlens-{extent_slug}-{overture_release}.pmtiles`, e.g.
`openlrlens-nz-2026-05-20.pmtiles` or `openlrlens-world-2026-05-20.pmtiles`. The slug is the
lower-cased `--extent` argument with non-alphanumeric characters replaced by `-`.

**Development target:** start with `--extent NZ` (New Zealand, ~5 M road segments, fast
end-to-end cycle) before attempting larger extents.

### Schema mapping file

The adapter step (step 2 below) is version-coupled and **must not be hardcoded**. The mapping
from Overture `class`/`subclass`/`road_flags` → FRC/FOW/direction is loaded from a TOML file
at pipeline startup, defaulting to `pipeline/schema/overture-default.toml`. Pass `--schema
<path>` to override (e.g. when Overture revises its taxonomy).

**TOML format:** `[[rules]]` entries are matched in order; first match wins. `class = ""`
is a catch-all; `subclass = ""` matches any subclass (including absent). `[flag_overrides]`
entries apply *after* the class/subclass match and override only the named attribute.

```toml
# pipeline/schema/overture-default.toml — proposed mapping, validate with Overture team.
# FRC: 0 (motorway/most important) … 7 (least important / non-vehicle)
# FOW: 0=undefined  1=motorway  2=multiple_carriageway  3=single_carriageway
#       4=roundabout  5=traffic_square  6=slip_road  7=other

[[rules]]
class = "motorway"; subclass = "link"; frc = 0; fow = 6   # slip road
[[rules]]
class = "motorway"; subclass = ""; frc = 0; fow = 1

[[rules]]
class = "trunk"; subclass = "link"; frc = 1; fow = 6
[[rules]]
class = "trunk"; subclass = ""; frc = 1; fow = 3          # flag_overrides.is_dual_carriageway → FOW 2

[[rules]]
class = "primary"; subclass = "link"; frc = 1; fow = 6
[[rules]]
class = "primary"; subclass = ""; frc = 1; fow = 3

[[rules]]
class = "secondary"; subclass = "link"; frc = 2; fow = 6
[[rules]]
class = "secondary"; subclass = ""; frc = 2; fow = 3

[[rules]]
class = "tertiary"; subclass = "link"; frc = 3; fow = 6
[[rules]]
class = "tertiary"; subclass = ""; frc = 3; fow = 3

[[rules]]
class = "unclassified"; subclass = ""; frc = 4; fow = 3
[[rules]]
class = "residential"; subclass = ""; frc = 5; fow = 3
[[rules]]
class = "living_street"; subclass = ""; frc = 6; fow = 3
[[rules]]
class = "service"; subclass = ""; frc = 6; fow = 7

[[rules]]
class = "pedestrian"; subclass = ""; frc = 7; fow = 5
[[rules]]
class = "track"; subclass = ""; frc = 7; fow = 7
[[rules]]
class = "footway"; subclass = ""; frc = 7; fow = 7
[[rules]]
class = "cycleway"; subclass = ""; frc = 7; fow = 7
[[rules]]
class = "path"; subclass = ""; frc = 7; fow = 7
[[rules]]
class = "steps"; subclass = ""; frc = 7; fow = 7
[[rules]]
class = "bridleway"; subclass = ""; frc = 7; fow = 7
[[rules]]
class = "construction"; subclass = ""; frc = 7; fow = 0

[[rules]]
class = ""; subclass = ""; frc = 7; fow = 0               # catch-all → undefined

# Road-flag overrides. Flag names must match the Overture road_flags field for this release.
[flag_overrides]
is_roundabout       = { fow = 4 }
is_dual_carriageway = { fow = 2 }
```

**Direction derivation** is not in the TOML (it follows a fixed algorithm, not a lookup):
derive from `access_restrictions` — an entry `{ access_type: "denied", when: { heading:
"backward" } }` → `FORWARD`; `heading: "forward"` → `BACKWARD`; no directional restriction
→ `BOTH`. The `heading` field names and `access_type` values must be confirmed against the
target release schema.

### Pipeline runtime model

- **Async runtime:** `tokio` throughout. All HTTP (S3 listing, parquet fetches) is async via
  `reqwest`. CPU-bound stages (adapt, split, quantize, tile) run via
  `tokio::task::spawn_blocking` so they don't starve the executor; within each blocking task
  use `rayon` to parallelise over segment batches.
- **Logging:** `tracing` + `tracing-subscriber`. Verbosity is set with `-v` / `-vv` flags
  (0 = info, 1 = debug, 2+ = trace); `RUST_LOG` overrides when set. Conventions:
  - `ERROR` — fatal, pipeline aborts.
  - `WARN` — retried operations, unexpected-but-handled conditions, schema catch-all hits.
  - `INFO` — top-level step entry/exit, segment and tile counts, elapsed time.
  - `DEBUG` — per-file/per-tile progress, individual retry attempts.
  - `TRACE` — per-segment detail (very verbose; only for targeted debugging).
- **Retry logic:** all HTTP calls go through a `RetryConfig`-driven helper. All parameters
  are CLI-configurable with sensible defaults: `--retry-max` (attempts, default 5),
  `--retry-base-ms` (initial backoff, default 200 ms), `--retry-max-ms` (backoff cap,
  default 30 000 ms), `--retry-factor` (multiplier, default 2.0). Retries on network
  failures, HTTP 429 / 5xx; fails immediately on 4xx (except 429) and parse errors.
  Each retry logged at `WARN` with attempt, URL, error, and next delay.
- **Parallelism:** two independently configurable axes — `-j / --jobs` sets rayon's worker
  thread count for CPU-bound stages (default: logical CPU count); `--fetch-concurrency`
  bounds concurrent HTTP downloads via a `tokio::sync::Semaphore` (default 8). Both sit on
  `BuildArgs`, not top-level, since they are irrelevant to `list-releases`.

**Steps:**
1. **Extract** segments for the resolved extent. Native DuckDB or a Rust parquet reader; **not**
   DuckDB-Wasm — it is browser-only regardless, and v1.29.0 specifically faulted on a 456 MB
   Overture segment file during design (`function signature mismatch`, then
   `memory access out of bounds`). Newer DuckDB-Wasm builds weren't tested; the build runs
   natively either way, so this doesn't gate anything.
2. **Adapt** each segment: `class`/`subclass` → `frc`; `class`/`subclass`/`road_flags` →
   `fow`; access rules + heading → `direction`. This adapter is the version-coupled layer;
   isolate it and confirm field names against the *current* Overture transportation schema
   (it evolves per release).
3. **Split** every segment at its interior connectors into node-to-node edges; recompute
   `geometry`, `length_m`, and node ids per piece (Invariant 1).
4. **Assign stable ids** to segments and nodes, derived deterministically from GERS ids
   (Invariant 2).
5. **Flatten** `prohibited_transitions` into the per-node turn-restriction table.
6. **Quantize** geometry into tile-local coordinates at a grid fine enough to be sub-pixel at
   max display zoom (sub-meter is a safe proxy). Do **not** lossily simplify (Invariant 4); at
   most drop exactly-collinear vertices losslessly. `length_m` was fixed in step 3 from full
   geometry and is unaffected — never recompute it from the stored geometry.
7. **Tile**: bin edges into `z/x/y`, build per-tile payloads, write the PMTiles archive +
   manifest.

**Output:** a versioned PMTiles archive + manifest, uploadable to R2.

**Tooling:** Rust (preferred — shares geometry/codec types with the engine), or DuckDB-native
for the extract + a Rust transform for the rest.

**Verification:** load a built tile region into the engine and confirm segment counts, that
junctions are present (no dropped interior connectors), that stored geometry overlays the
OSM-derived basemap roads (no lossy simplification crept in) and that quantization snapping is
sub-pixel at max display zoom, and that boundary nodes resolve across tiles.

---

## 7. Codec layer

- **Decode (v1):** parsers for OpenLR binary v3 and TPEG-OLR, each emitting the **unified LRP
  model**. Verify bit-layouts with assertions.
- **Unified LRP model:** every distance and bearing is a **`[LB, UB]` interval**. v3 fills the
  interval with the quantization bucket; TPEG sets `LB == UB`. Downstream engine code is
  **format-agnostic** — the only format-specific code is bit unpacking.
- **Encode (stubbed for v1):** define an `OpenLrEncoder` trait and the LRP→binary surface, but
  the implementation may be `unimplemented!()`. Do not let encoder concerns leak into the
  decode/engine types.

```rust
// Distinct types so the mod-360 wraparound logic can never be applied to a linear
// quantity, nor forgotten on a bearing (Invariant 6). Do NOT collapse into one type.
pub struct CircularInterval { pub lb_deg: f64, pub ub_deg: f64 } // containment/overlap mod 360
pub struct LinearInterval   { pub lb: f64,     pub ub: f64 }     // meters; ordinary containment

pub struct Lrp {
    pub coord: (f64, f64),
    pub bearing: CircularInterval,           // degrees
    pub frc: u8, pub fow: u8,
    pub lfrcnp: u8,                          // lowest FRC to next point
    pub dnp: Option<LinearInterval>,         // distance to next point, meters (None on last LRP)
    pub pos_offset: Option<LinearInterval>,  // offsets are linear distances, where present
    pub neg_offset: Option<LinearInterval>,
}
```

---

## 8. Decode engine

- **Candidate selection (any-point projection):** for each LRP, coarse-fetch segments via the
  provider, then for each candidate project the LRP coordinate onto the segment polyline
  (nearest point + arc-length), and compute the local bearing over the next 20 m from that arc
  position. An LRP may match **anywhere along a segment**, not only at line-start nodes — maps
  diverge and the encoder may force mid-segment intermediate LRPs. Start-type LRPs use the
  forward 20 m bearing; the final LRP uses the 20 m *preceding* the projection (backward). For
  bidirectional segments both directions are candidates; `direction` gates legality.
- **Matching — each acceptance criterion is BOTH a hard gate and a soft term; be explicit
  about which is which.** For bearing and for length/DNP the rule is the same two-stage shape:
  - **Hard gate (feasibility):** the candidate value must fall within the widened window
    `[LB − τ, UB + τ]` (bearing) or `[LB − δ, UB + δ]` (distance), where `⊕ τ`/`⊕ δ` is the
    map-divergence tolerance (Invariant 5). Outside the window → the candidate is **rejected**,
    not penalized. Search radius and the DNP window are likewise hard gates.
  - **Soft penalty (ranking):** among candidates that pass the gate, the score adds a penalty
    that is **zero inside `[LB, UB]`** and grows with distance from the nearest interval bound
    out to the widened edge. So a value sitting anywhere in the encoding interval is "free"
    (correct for v3 buckets and for TPEG points alike); only the residual map-divergence offset
    is penalized.
  - The total score is `positional distance + bearing penalty + FRC penalty + FOW penalty`,
    additive and decomposable (Invariant 7). FRC/FOW penalties are soft ranking terms; the
    LFRCNP floor (below) is a hard gate.
- **A\*** between consecutive LRP candidates over the in-memory graph: state is
  `(node, incoming_segment)` (Invariant 3); honor `direction`, the LFRCNP floor, and the
  turn-restriction table; bound expansion by `max_path_search_factor`. The search runs
  **point-on-edge → point-on-edge** with partial first/last edges; the partial start edge is
  the initial `incoming_segment`, so the first turn-restriction check fires normally at its
  end node.
- **Validation:** route length must fall within the DNP **hard** window = `dnp_interval ⊕ δ`,
  where the half-width is `max(bucket_half, pct · length)` (absolute v3 bucket vs. percentage
  tolerance). Apply positive/negative offsets to trim the partial first/last edges; offsets are
  `LinearInterval`s carrying the **same** v3-bucket / TPEG-exact distinction as DNP (v3 buckets
  the offset, TPEG gives it exactly), so the trim point inherits that interval's width — treat
  the trimmed location as a range, not a single point, when it matters for diagnostics.

---

## 9. Decode parameters (decode-time configurable)

Exposed to the UI; **permissive defaults**, tuned interactively.
- `candidate_search_radius_m` (positional tolerance)
- `bearing_tolerance_deg` (`τ`) — the map-divergence term; combines with the interval
- `dnp_tolerance_pct` (`δ`) — combines with the absolute v3 bucket
- `frc_weight_penalty`, `fow_weight_penalty` (soft ranking)
- `max_path_search_factor` (A* expansion cap)
- Optional master "strictness" slider scaling the vector, with individual overrides.

Notes for the implementer:
- Keep **hard tolerances** (feasibility cutoffs) and **soft penalties** (ranking) distinct in
  the model. Loosening a hard tolerance changes the candidate set discontinuously, so the
  decoded path can **flip** as a slider moves, not morph — the UI should show *which* candidate
  entered/left.
- The same `τ` behaves differently per format (v3's bucket dominates; for TPEG `τ` is the whole
  window). Show the **effective window** per LRP, not the raw `τ`.
- **Emit the parameter set with every result.** A decode is `string + tolerance profile →
  path`. This is the reproducibility record and the basis of the regression corpus.

---

## 10. Diagnostics (the differentiator)

1. **Stepped debugger:** visualize the candidate radius per LRP; color candidates pass/fail
   with the specific reason (e.g. "bearing outside [LB−τ, UB+τ]", "FRC below LFRCNP",
   "direction"); animate A* frontier expansion; badge the node where a path breaks.
2. **Interval visualization:** draw the bearing interval as a wedge (wide for v3, thin for
   TPEG) and the distance interval as a band, plus the `τ`/`δ` halo. Show why a candidate sits
   inside/outside.
3. **Desired-vs-actual explanation** (reuses the encoder click-canvas to let the user assert
   the expected edge path):
   - Run the user's desired path through the **same** feasibility + cost functions
     (forced-decode mode), then diff against the chosen path at the **divergence node**.
   - Classify: was the desired branch **infeasible** (direction / turn restriction /
     LFRCNP / DNP window / candidate-not-generated / search-limit) or **feasible-but-outscored**
     (then attribute the margin per term, per LRP)?
   - Emit named diagnoses, including **"attribute rendered non-discriminating by projection
     displacement"** (e.g. the LRP projected N m into a motorway interior, M m from the actual
     motorway/sliproad junction, so its FOW had no purchase).
   - **Root-cause verdict** (decoder-tunable vs. encoder-deficient). This is **not** a black-box
     continuous search over all parameters — that would be intractable and the spec must not
     pretend otherwise. Decompose it, exploiting structure:
     - **Hard gates are monotonic.** Loosening a tolerance only ever *admits* more candidates,
       so the minimal tolerances needed to make the desired path **feasible** are computed in
       closed form: for each failing edge on the desired path, take the margin by which it
       misses each hard gate (bearing, distance, radius, LFRCNP); the required tolerance is the
       max of those margins. No search.
     - **The soft-ranking flip is a linear program.** Because the cost is additive
       (Invariant 7), the score *difference* between the desired path and a competitor is a
       linear function of the tunable weights (FRC, FOW, bearing, positional). "Does any allowed
       weight vector make the desired path outscore every admissible competitor" is therefore a
       linear-feasibility check over the bounded weight box — not a grid search.
     - **Require strict dominance, not mere admissibility.** The verdict is *tunable* only if
       some (minimal-feasibility tolerance) + (weight vector from the LP) makes the desired path
       the **strict unique winner** over the admissible set at that setting; otherwise it is
       **encoder-deficient** (e.g. needs an intermediate LRP at junction J), reported with the
       proof that no setting recovers it.
     - **Residual complexity to acknowledge:** the competitor set is discrete and *changes* as
       gates loosen (new admissible paths appear), so the LP must be re-checked at each
       breakpoint where the admissible set changes — a finite, enumerable set of breakpoints, not
       a continuum. Start with the closed-form-margins + per-breakpoint-LP approach; fall back to
       a coarse grid search only if the breakpoint enumeration proves unwieldy in practice.
   - Handle the degenerate case where the desired path is not traversable on the decoder graph
     (missing/illegal connection) with its own clear explanation.

---

## 11. Repository structure (proposed)

```
openlrlens/
  crates/
    openlr-codec/        # v3 + TPEG decoders, unified LRP model, encoder traits (stubbed)
    openlr-graph/        # segment/node/restriction types, in-memory graph, geometry primitives
    openlr-engine/       # candidate selection, A*, validation, diagnostics (forced-decode)
    openlr-provider/     # OpenLRDataProvider trait + PmtilesProvider (+ later LiveOverture)
    openlr-wasm/         # wasm-bindgen glue; exposes config + a steppable decode loop
  pipeline/              # offline build pipeline (Overture -> PMTiles)  <-- BUILD FIRST
  web/                   # Vite SPA, MapLibre GL JS, PMTiles reader, diagnostic UI
  fixtures/              # regression corpus: {string, params, expected path}
  AGENTS.md -> CLAUDE.md # symlink; this project's agent context standard
```

---

## 12. Tech stack

- **Core:** Rust → `wasm32-unknown-unknown` via `wasm-bindgen`. No GC; deterministic.
- **Pipeline:** Rust. Async runtime: `tokio`. HTTP: `reqwest`. Parallel CPU: `rayon`.
  Logging: `tracing` + `tracing-subscriber`. DuckDB called via `spawn_blocking`.
- **Front end:** Vite + **React** (recommended default; overridable to Svelte) + **MapLibre
  GL JS**. Pass routing-tree/candidate data out of WASM as typed arrays → GeoJSON sources /
  custom layers (note: MapLibre ingests GeoJSON, not raw WASM memory — convert at the boundary).
- **Basemap / overlay fidelity:** the decoded path is drawn over a slippy basemap, and the
  highlighted geometry must sit on the rendered roads. Since stored geometry is full-fidelity
  (Invariant 4), the only residual misalignment is *genuine map divergence* — which is itself
  diagnostic signal, not a bug. For the tightest overlay, prefer an **OSM/Overture-lineage
  basemap** (ideally derived from the *same* Overture release you built the tiles from) so your
  highlight and the rendered road share a source; a different-lineage basemap will show inherent
  divergence, which is acceptable but should be read as map difference, not as your error.
- **Tile reader:** a PMTiles reader in the browser (JS) feeding the WASM provider; or a Rust
  PMTiles reader inside WASM.
- The WASM boundary must expose a **steppable** decode (pause/resume) so the UI can animate
  candidate evaluation and A* expansion.

---

## 13. Licensing & attribution (non-negotiable)

Overture transportation is derived from OpenStreetMap and carried under **ODbL**. The derived
tile store and any served output must comply: preserve required **attribution** (OSM + Overture)
in the app and data artifacts, and account for ODbL **share-alike** obligations on the derived
database. Document the exact attribution text and license posture before public release. Treat
this as a build-pipeline output requirement, not a footnote.

---

## 14. Implementation roadmap

1. **Offline build pipeline** (this milestone): one region end-to-end → a valid PMTiles archive
   with split edges, stable ids, restrictions, and full-fidelity quantized geometry. Verify per §6.
2. **Codec + decode engine core**: v3 + TPEG decoders → unified intervals; candidate
   selection with any-point projection; A* with `(node, incoming_segment)` state, restrictions,
   LFRCNP, partial edges; DNP validation. Assertion-tested against the fixtures corpus.
3. **Provider + tile reader + WASM bindings**: `PmtilesProvider`, the steppable WASM decode
   loop, config plumbing.
4. **Front end + diagnostics**: MapLibre rendering, parameter UI, stepped debugger, interval
   visualization, and the desired-vs-actual explanation with the root-cause verdict.

Then: scale the pipeline to planet; add the optional `LiveOvertureProvider`; fill in the
encoder behind its trait.

---

## 15. Agent conventions

- Match the user's stack fluency (polyglot; primary here is Rust). Prefer small, well-typed
  crates with clear boundaries; the codec must not leak format specifics past the unified LRP
  model, and the engine must not know which provider backs it.
- Keep the cost function additive/decomposable; keep hard tolerances and soft penalties
  separate types.
- Maintain the **fixtures** regression corpus (`string + params + expected path`) and run it
  as tests; add a fixture whenever a decode behavior is pinned down or a release is adopted.
- When a decision is genuinely open, state the assumption inline and proceed; do not stall on
  TBDs — but never silently violate a Critical Invariant to make something compile.

