# OpenLRLab

A browser-based diagnostic decoder **and encoder** for [OpenLR](https://www.openlr-association.com/) location references. The Rust core compiles to WebAssembly and runs the full codec, graph, A\* path search, and encoder entirely client-side. A MapLibre GL JS front end renders the decoded/encoded path and step-by-step diagnostics.

Two formats are supported, both for decode and encode:

- **OpenLR binary v3** (TomTom) — 11.25° bearing buckets, ~58.6 m DNP buckets
- **TPEG-OLR / ISO 21219-22** — full-precision intervals

## Architecture

```
BUILD TIME  (a few times per year, separate repo: openlr-pmtiles)
  Road network source data ──▶ openlr-pmtiles-build ──▶ PMTiles archive ──▶ R2 / CDN

RUNTIME  (browser, no server)
  PMTiles (range reads) ──▶ TileLoader ──▶ OpenLRDataProvider ──▶ in-memory graph
                                                    │
  decode:  OpenLR string ──▶ codec (v3 / TPEG) ──▶ unified LRP model
                                                    │
                             engine: candidate selection + A* + validation
                                                    │
  encode:  waypoints (map clicks) ──▶ snap + route ──▶ encoder (Rule-1/Rule-4)
                                                    │
                             codec (v3 / TPEG) ──▶ round-trip verify (decode)
                                                    │
                                         diagnostics + MapLibre UI
```

All map I/O stays in JavaScript. WASM receives pre-fetched tile bytes and operates synchronously over an in-memory cache, avoiding async-trait across the FFI boundary.

### Rust crates

| Crate | Role |
|---|---|
| `openlr-codec` | v3 / TPEG-OLR binary parsing and serialization ↔ unified `Lrp` model |
| `openlr-graph` | Tile format, segment/node tables, geometry pool |
| `openlr-engine` | Decode: candidate selection, A\* (`state = (node, incoming_segment)`), scoring, diagnostics |
| `openlr-encoder` | Encode: Rule-1/Rule-4 Line and PointAlongLine encoding, boundary expansion, coverage sweep |
| `openlr-provider` | `OpenLRDataProvider` trait + `PmtilesProvider` implementation |
| `openlr-wasm` | `wasm-bindgen` glue exposing `Decoder` (decode) and `Encoder` (waypoint snapping, route preview, encode) to JS |

The PMTiles builder (`openlr-pmtiles-build`, ingesting Overture, OSM, generic
GeoJSONL, or a canonical DuckDB source) lives in a separate repo,
[openlr-pmtiles](https://github.com/nw31304/openlr-pmtiles) — this repo is a
consumer of the archives it produces, not the producer. Only the tile
**format** (magic, header layout, segment/node/restriction records — see
`CLAUDE.md §4–5`) is a contract shared between the two repos; a format change
must land in openlr-pmtiles first, then propagate here to `openlr-provider`'s
decoder.

### Web frontend

Vite + React + MapLibre GL JS + Zustand. Source lives in `web/`.

## Diagnostics

The UI is a stepped debugger, not just a result renderer:

- **Candidate panel** — per-LRP candidate table with bearing wedge, DNP band, and per-term scores. Each candidate shows whether it snapped to an interior point, start endpoint, or end endpoint.
- **A\* replay** — step-forward/backward through the search frontier.
- **Forced-decode mode** — pin any candidate per LRP and re-run A\* to see why the encoder's intended path was accepted or rejected.
- **Encode mode** — draw waypoints directly on the map (click to append, drag to insert/move) for a Line or PointAlongLine location; a live route preview snaps and routes between them as you go. Confirming the last waypoint automatically encodes to both binary v3 and TPEG-OLR and immediately decodes the result back, so every encode is round-trip-verified against the exact same engine a consumer would use.
- **LLM chat** — optional AI assistant with full access to the decode trace, encode/verify state, candidate scores, and graph geometry. Bring your own key (OpenAI / Anthropic).

## Prerequisites

- Rust toolchain + `wasm-pack`
- Node.js ≥ 18

## Build

### 1. Compile the WASM module

```sh
cd crates/openlr-wasm
wasm-pack build --target web --out-dir ../../web/src/wasm
```

### 2. Run the web dev server

```sh
cd web
npm install
npm run dev
```

`npm run dev` starts both the Vite dev server (default `localhost:5173`) and a built-in tile server at `http://localhost:5176` (see the `tile-server` plugin in `vite.config.js`). By default it serves range requests out of `../out`; set `OPENLR_TILES_DIR` to point it at wherever [openlr-pmtiles](https://github.com/nw31304/openlr-pmtiles) built its archives instead (e.g. `OPENLR_TILES_DIR=../../openlr-pmtiles/out npm run dev`). Override the tile source in the **Tile source** menu if you're pointing at a different archive or host.

### 3. Build a tile archive (optional — if you have road network data)

Building PMTiles archives is a separate repo now:
[openlr-pmtiles](https://github.com/nw31304/openlr-pmtiles). See its README
for build commands. Point this repo's dev server at its output via
`OPENLR_TILES_DIR` (step 2), or serve the archive from any PMTiles-compatible
host (e.g. [`pmtiles serve`](https://github.com/protomaps/go-pmtiles), or
R2/CDN with range-request support) and point the app at it via the **Tile
source** menu.

## Tile format

Custom binary payload (magic `OLRL`, version 3). All integers little-endian, single zoom level (default z12). Segments are post-split at every interior junction — junctions are never elided. Each segment and node carries a provider-defined opaque stable ID (UTF-8 string, stored in a per-tile string pool). See `CLAUDE.md §4–5` for the full layout.

## License

Web frontend: MIT. Derived tile data license depends on the source data used to build it: OSM-derived sources (OSM directly, or any provider whose road-network theme is OSM-derived, e.g. Overture) carry **ODbL** — any served output must preserve attribution and honour share-alike obligations.
