//! WebAssembly bindings for the OpenLRLens decode engine.
//!
//! # JS usage pattern
//!
//! ```js
//! import init, { Decoder } from './openlr_wasm.js';
//! await init();
//!
//! const dec = new Decoder();
//!
//! // 1. Parse the reference and learn which tiles are needed.
//! const { tiles } = JSON.parse(dec.start("CwRbnh...", JSON.stringify(params), 12));
//! // tiles: [[z, x, y], ...]
//!
//! // 2. Fetch each tile from the PMTiles archive and inject it.
//! for (const [z, x, y] of tiles) {
//!     const bytes = await pmtilesSource.getZxy(z, x, y);
//!     if (bytes) dec.load_tile(z, x, y, new Uint8Array(bytes));
//! }
//!
//! // 3. Run the decode.
//! const result = JSON.parse(dec.decode());
//! if (result.ok) {
//!     console.log(result.wkt);          // "LINESTRING (...)"
//!     console.log(result.segments);     // [{ frc, fow, osm_way_id }, ...]
//! }
//! ```

use wasm_bindgen::prelude::*;

use openlr_codec::{decode_v3_base64, decode_tpeg_hex};
use openlr_codec::lrp::LocationReference;
use openlr_engine::{decode as engine_decode, DecodeParams, prefetch_tile_keys, path_to_wkt};
use openlr_provider::TileLoader;
use serde::Serialize;

// ── Module init ───────────────────────────────────────────────────────────────

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

// ── JS-visible result types ───────────────────────────────────────────────────

/// Returned by `Decoder.start()` as a JSON string.
#[derive(Serialize)]
struct StartResult {
    /// Tiles to fetch before calling `decode()`.  Each entry is `[z, x, y]`.
    tiles: Vec<[u32; 3]>,
}

/// Per-segment metadata included in a successful `DecodeResult`.
#[derive(Serialize)]
struct SegmentInfo {
    frc: u8,
    fow: u8,
    /// OSM way ID, present when the tile was built from OSM data.
    #[serde(skip_serializing_if = "Option::is_none")]
    osm_way_id: Option<i64>,
}

/// Returned by `Decoder.decode()` as a JSON string.
#[derive(Serialize)]
struct DecodeResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    wkt: Option<String>,
    segments: Vec<SegmentInfo>,
    pos_offset_m: f64,
    neg_offset_m: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Full decode trace; null when `trace_level` is `Off` or on error.
    #[serde(skip_serializing_if = "Option::is_none")]
    trace: Option<serde_json::Value>,
}

impl DecodeResult {
    fn err(msg: impl Into<String>) -> Self {
        DecodeResult {
            ok: false,
            wkt: None,
            segments: vec![],
            pos_offset_m: 0.0,
            neg_offset_m: 0.0,
            error: Some(msg.into()),
            trace: None,
        }
    }
}

// ── Decoder ───────────────────────────────────────────────────────────────────

/// Stateful decode session.  Create one per reference string, or call `reset()`
/// between decodes if you want to reuse the loaded tile cache.
#[wasm_bindgen]
pub struct Decoder {
    loader: TileLoader,
    location_ref: Option<LocationReference>,
    params: DecodeParams,
    zoom: u8,
}

#[wasm_bindgen]
impl Decoder {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Decoder {
        Decoder {
            loader: TileLoader::new(),
            location_ref: None,
            params: DecodeParams::default(),
            zoom: 12,
        }
    }

    /// Parse `openlr_string` (auto-detects OpenLR binary v3 base64 or TPEG-OLR hex),
    /// store the decode parameters, and compute the set of tiles that must be loaded.
    ///
    /// `params_json`: JSON-serialized `DecodeParams`, or `""` / `"null"` for defaults.
    /// `zoom`: tile zoom level (must match the PMTiles archive; typically 12).
    ///
    /// Returns a JSON string: `{ "tiles": [[z, x, y], ...] }`.
    /// Throws a JS error string on parse failure.
    pub fn start(&mut self, openlr_string: &str, params_json: &str, zoom: u8) -> Result<String, JsValue> {
        let params: DecodeParams = if params_json.is_empty() || params_json == "null" {
            DecodeParams::default()
        } else {
            serde_json::from_str(params_json)
                .map_err(|e| JsValue::from_str(&format!("invalid params: {e}")))?
        };

        let loc_ref = parse_openlr(openlr_string)
            .map_err(|e| JsValue::from_str(&e))?;

        let tile_keys = prefetch_tile_keys(&loc_ref.lrps, &params, zoom);
        let tiles: Vec<[u32; 3]> = tile_keys
            .iter()
            .map(|k| [k.z as u32, k.x, k.y])
            .collect();

        self.location_ref = Some(loc_ref);
        self.params = params;
        self.zoom = zoom;

        Ok(serde_json::to_string(&StartResult { tiles }).unwrap())
    }

    /// Inject one tile's raw OLRL bytes into the graph.  Call once per tile
    /// returned by `start()`.  Missing tiles are silently skipped — decode will
    /// simply have fewer candidates near those coordinates.
    ///
    /// Throws a JS error string if the tile payload is malformed.
    pub fn load_tile(&mut self, _z: u8, _x: u32, _y: u32, data: &[u8]) -> Result<(), JsValue> {
        self.loader
            .load_tile(data)
            .map_err(|e| JsValue::from_str(&format!("tile parse error: {e}")))
    }

    /// Run the decode against the loaded graph.
    ///
    /// Returns a JSON string.  On success:
    /// ```json
    /// { "ok": true, "wkt": "LINESTRING (...)", "segments": [...],
    ///   "pos_offset_m": 0.0, "neg_offset_m": 0.0, "trace": {...} }
    /// ```
    /// On failure:
    /// ```json
    /// { "ok": false, "error": "LRP 0: no candidate segments found", "segments": [] }
    /// ```
    pub fn decode(&self) -> String {
        let loc_ref = match &self.location_ref {
            Some(r) => r,
            None => return serde_json::to_string(&DecodeResult::err("call start() first")).unwrap(),
        };

        let result = match engine_decode(loc_ref, &self.loader.graph, &self.params) {
            Err(e) => return serde_json::to_string(&DecodeResult::err(e.to_string())).unwrap(),
            Ok(r)  => r,
        };

        let wkt = path_to_wkt(
            &result.path,
            result.pos_offset_m,
            result.neg_offset_m,
            result.first_lrp_arc_m,
            result.last_lrp_arc_m,
            &self.loader.graph,
        );

        let segments: Vec<SegmentInfo> = result.path.iter().filter_map(|seg_id| {
            self.loader.graph.segments.get(seg_id).map(|seg| SegmentInfo {
                frc: seg.frc,
                fow: seg.fow,
                osm_way_id: seg.osm_way_id(),
            })
        }).collect();

        let trace_value = result.trace.and_then(|t| serde_json::to_value(t).ok());

        serde_json::to_string(&DecodeResult {
            ok: true,
            wkt,
            segments,
            pos_offset_m: result.pos_offset_m,
            neg_offset_m: result.neg_offset_m,
            error: None,
            trace: trace_value,
        }).unwrap()
    }

    /// Clear the stored location reference.  The loaded tile graph is kept so
    /// nearby re-decodes can reuse the cached tiles — call `reset_tiles()` too
    /// if you want to start completely fresh.
    pub fn reset(&mut self) {
        self.location_ref = None;
    }

    /// Drop all loaded tiles and the stored location reference.
    pub fn reset_tiles(&mut self) {
        self.loader = TileLoader::new();
        self.location_ref = None;
    }

    /// Tile zoom level in use (set by `start()`).
    pub fn zoom(&self) -> u8 {
        self.zoom
    }

    /// Number of segments in the loaded graph.
    pub fn loaded_segment_count(&self) -> usize {
        self.loader.graph.segments.len()
    }

    /// Number of nodes in the loaded graph.
    pub fn loaded_node_count(&self) -> usize {
        self.loader.graph.nodes.len()
    }
}

// ── Format auto-detection ─────────────────────────────────────────────────────

/// Try OpenLR binary v3 (base64) then TPEG-OLR (hex).  Returns the first that parses.
fn parse_openlr(s: &str) -> Result<LocationReference, String> {
    // v3 base64 strings contain '+', '/', or '=' — characters that are not valid hex.
    // Use that as a fast discriminator before trying both parsers.
    let looks_like_base64 = s.chars().any(|c| c == '+' || c == '/' || c == '=');

    if looks_like_base64 {
        decode_v3_base64(s).map_err(|e| format!("OpenLR v3 parse error: {e}"))
    } else {
        // Could be v3 (url-safe base64 without padding) or TPEG hex — try both.
        decode_v3_base64(s)
            .or_else(|_| decode_tpeg_hex(s))
            .map_err(|e| format!("OpenLR parse error (tried v3 and TPEG): {e}"))
    }
}
