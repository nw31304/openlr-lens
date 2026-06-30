// Tool definitions (OpenAI function-calling format) and executor.
// llmClient.js converts these to Anthropic format when needed.

const FOW_LABELS = [
  'undefined', 'motorway', 'dual carriageway', 'single carriageway',
  'roundabout', 'traffic square', 'slip road', 'other',
];
const FRC_LABELS = [
  'FRC0 motorway', 'FRC1 trunk', 'FRC2 secondary', 'FRC3 tertiary',
  'FRC4 unclassified', 'FRC5 residential', 'FRC6 service', 'FRC7 other',
];

const FOW_LABELS_FULL = [
  'Form of Way undefined', 'Motorway', 'Multiple carriageway', 'Single carriageway',
  'Roundabout', 'Traffic square', 'Slip road', 'Other / non-vehicle',
];

export const TOOL_DEFINITIONS = [
  {
    type: 'function',
    function: {
      name: 'get_decode_summary',
      description:
        'Top-level decode outcome: success/failure, segment count, format, offset ranges, current decode parameters, and the full ordered path segment list with per-segment length_m, FRC, FOW, and direction. Also returns path_total_length_m — the sum of all segment lengths in the untrimmed decoded location. Call this first before any other tool.',
      parameters: { type: 'object', properties: {}, required: [] },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_parsed_reference',
      description:
        'Full parsed LRP chain: coordinates, bearing interval, FRC, FOW, LFRCNP, and DNP for each LRP.',
      parameters: { type: 'object', properties: {}, required: [] },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_lrp_candidates',
      description:
        'Ranked candidate segments for one LRP, with projection geometry and score breakdown. Set include_rejected=true to see rejection reasons.',
      parameters: {
        type: 'object',
        properties: {
          lrp_index: {
            type: 'integer',
            description: 'Zero-based LRP index.',
          },
          include_rejected: {
            type: 'boolean',
            description: 'Include rejected candidates with their rejection verdict. Default false.',
          },
        },
        required: ['lrp_index'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_leg_summary',
      description:
        'Routing outcome for one inter-LRP leg: whether a route was found, its length, A* expansion statistics (nodes expanded, edges skipped by reason), and DNP validation result.',
      parameters: {
        type: 'object',
        properties: {
          leg_index: {
            type: 'integer',
            description: 'Zero-based leg index (leg 0 = LRP 0 → LRP 1).',
          },
        },
        required: ['leg_index'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_route_segments',
      description:
        'Ordered segment list for a successfully routed leg, with per-segment length_m, FRC, FOW, direction, and cumulative distance. Also returns segment_sum_m and snap coordinates at each end.',
      parameters: {
        type: 'object',
        properties: {
          leg_index: {
            type: 'integer',
            description: 'Zero-based leg index.',
          },
        },
        required: ['leg_index'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_segment',
      description:
        'Full attributes and geometry for one segment by its internal segment ID. Returns FRC, FOW, direction, length, geometry, tile location, and source_key (the human-readable stable ID such as "372358612-1"). Use this to inspect any segment seen in candidate lists, path breakdowns, or rejection reasons.',
      parameters: {
        type: 'object',
        properties: {
          segment_id: {
            type: 'integer',
            description: 'Internal graph segment ID (as seen in candidate or path data).',
          },
        },
        required: ['segment_id'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_segments_near',
      description:
        'Find all loaded road segments within radius_m of a coordinate. Returns up to 20 segments sorted by distance, each with source_key (stable ID like "372358612-1"), FRC, FOW, direction, and length. Useful for understanding what roads are available near an LRP that produced no or few candidates.',
      parameters: {
        type: 'object',
        properties: {
          lat:      { type: 'number',  description: 'Latitude in decimal degrees.' },
          lon:      { type: 'number',  description: 'Longitude in decimal degrees.' },
          radius_m: { type: 'number',  description: 'Search radius in metres (max 500). Default 100.' },
        },
        required: ['lat', 'lon'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_segment_neighbors',
      description:
        'Returns all segments connected at each endpoint of a given segment. '
        + 'Reports two groups — at_start_node and at_end_node — each listing every other '
        + 'segment that shares that node, with can_arrive/can_depart flags and turn-restriction flags. '
        + 'For bidirectional (Both) segments each endpoint is simultaneously entry and exit, '
        + 'so both groups show full connectivity. '
        + 'Each neighbour includes source_key (the human-readable stable ID such as "372358612-1"), '
        + 'internal segment_id, FRC, FOW, direction, and length. '
        + 'Use this to understand junction topology, diagnose why A* took or avoided a turn, '
        + 'or explore the road network around a candidate segment.',
      parameters: {
        type: 'object',
        properties: {
          segment_id: {
            type: 'integer',
            description: 'Internal graph segment ID.',
          },
        },
        required: ['segment_id'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'retry_decode',
      description:
        'Re-run the decode with a partial parameter override merged over the current params. Returns ok/fail, segment count, and total path length so you can immediately compare with the original result. Tiles must already be loaded (always true after a normal decode). Example: {"max_bearing_deviation_deg": 30} to test a wider bearing window.',
      parameters: {
        type: 'object',
        properties: {
          params_override: {
            type: 'object',
            description: 'Partial DecodeParams as a JSON object — only the fields you want to change. All other params inherit from the current values.',
            additionalProperties: true,
          },
        },
        required: ['params_override'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'highlight_segments',
      description:
        'Highlight one or more road segments on the map immediately. '
        + 'Use this to visually direct the user\'s attention to specific segments you are discussing. '
        + 'Replaces any previous highlight. Pass an empty array to clear.',
      parameters: {
        type: 'object',
        properties: {
          segment_ids: {
            type: 'array',
            items: { type: 'integer' },
            description: 'Internal segment IDs to highlight (from candidate lists, path breakdowns, or get_segment).',
          },
        },
        required: ['segment_ids'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'set_map_view',
      description:
        'Pan and zoom the map to a specific coordinate. '
        + 'Use this to focus the user\'s view on the area you are discussing.',
      parameters: {
        type: 'object',
        properties: {
          lat:  { type: 'number',  description: 'Latitude in decimal degrees.' },
          lon:  { type: 'number',  description: 'Longitude in decimal degrees.' },
          zoom: { type: 'number',  description: 'Map zoom level (12–18 typical; 15 for street-level detail, 17 for junction-level).' },
        },
        required: ['lat', 'lon', 'zoom'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'focus_lrp',
      description:
        'Pan and zoom the map to an LRP coordinate at street-level zoom. '
        + 'Convenience wrapper around set_map_view — no need to look up coordinates manually.',
      parameters: {
        type: 'object',
        properties: {
          lrp_index: { type: 'integer', description: 'Zero-based LRP index.' },
        },
        required: ['lrp_index'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_astar_skipped_edges',
      description:
        'List every edge skipped by A* on a specific leg, with the skip reason. '
        + 'Requires Full trace level — returns an error if only Summary trace is available. '
        + 'Optionally filter to a specific segment_id to check whether A* skipped it and why. '
        + 'Use this to diagnose why a specific road was not used despite being reachable.',
      parameters: {
        type: 'object',
        properties: {
          leg_index: {
            type: 'integer',
            description: 'Zero-based leg index.',
          },
          segment_id: {
            type: 'integer',
            description: 'Optional: filter to this specific segment. If omitted, returns all skipped edges.',
          },
        },
        required: ['leg_index'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_forced_leg_summary',
      description:
        'Routing outcome for one leg of the most recent forced decode: A* stats and DNP validation. '
        + 'Use this after run_forced_decode fails to diagnose why a specific leg did not route. '
        + 'Returns an error if no forced decode has been run yet.',
      parameters: {
        type: 'object',
        properties: {
          leg_index: { type: 'integer', description: 'Zero-based leg index.' },
        },
        required: ['leg_index'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'get_attempted_combinations',
      description:
        'List every candidate combination the original decode attempted, with per-combination outcome. '
        + 'Each row shows which from/to candidates were tried for each leg and whether routing succeeded. '
        + 'Use this to understand the full search space and identify which combinations were never tried.',
      parameters: { type: 'object', properties: {}, required: [] },
    },
  },
  {
    type: 'function',
    function: {
      name: 'set_pinned_candidates',
      description:
        'Pin one specific accepted candidate per LRP so that run_forced_decode routes through exactly those snap points. '
        + 'Clears any existing pins first. Every LRP in the reference must be covered — pass one entry per LRP index. '
        + 'Snap geometry is resolved automatically from the trace; you only need segment_id and traversal from get_lrp_candidates. '
        + 'Returns the number of LRPs pinned or an error if a specified candidate was not in the accepted list.',
      parameters: {
        type: 'object',
        properties: {
          snaps: {
            type: 'array',
            description: 'One entry per LRP, in LRP order.',
            items: {
              type: 'object',
              properties: {
                lrp_index:  { type: 'integer', description: 'Zero-based LRP index.' },
                segment_id: { type: 'integer', description: 'Internal segment ID from get_lrp_candidates.' },
                traversal:  { type: 'string',  description: '"Forward" or "Backward".' },
              },
              required: ['lrp_index', 'segment_id', 'traversal'],
            },
          },
        },
        required: ['snaps'],
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'run_forced_decode',
      description:
        'Run A* using the currently pinned candidates (set via set_pinned_candidates). '
        + 'Skips candidate selection entirely — only routes between the pinned snap points. '
        + 'Returns ok/fail, segment count, total path length, and per-leg DNP outcome. '
        + 'All LRPs must be pinned before calling; returns an error otherwise.',
      parameters: { type: 'object', properties: {}, required: [] },
    },
  },
];

// ── TOON (Token-Oriented Object Notation) helpers ─────────────────────────────
// Compact tabular format: field names appear once in a header; data rows contain
// only values. Vendor-neutral — any capable LLM can parse it from the header.
//
// Format:  label[N]{col1,col2,...}:\n  v1,v2,...\n  v1,v2,...

function fmtVal(v) {
  if (v === null || v === undefined) return 'null';
  if (typeof v === 'number') return Number.isFinite(v) ? String(v) : 'null';
  return String(v);
}

// Render one TOON table: "label[N]{col,...}:\n  r1c1,r1c2,...\n  ..."
function toToon(label, rows, fields) {
  if (!rows || !rows.length) return `${label}[0]{${fields.join(',')}}:`;
  const header = `${label}[${rows.length}]{${fields.join(',')}}:`;
  const dataRows = rows.map(r => '  ' + fields.map(f => fmtVal(r[f])).join(','));
  return [header, ...dataRows].join('\n');
}

// Build a complete tool response from scalar key-value pairs and TOON tables.
// scalars: plain object — rendered as "key: value" lines (nulls omitted).
// tables:  [{ label, rows, fields }] — rendered as TOON blocks separated by blank lines.
function toonResponse(scalars, tables = []) {
  const parts = [];
  for (const [k, v] of Object.entries(scalars)) {
    if (v === null || v === undefined) continue;
    parts.push(`${k}: ${typeof v === 'object' ? JSON.stringify(v) : v}`);
  }
  for (const { label, rows, fields } of tables) {
    parts.push('');
    parts.push(toToon(label, rows, fields));
  }
  return parts.join('\n');
}

// Round to N decimal places (avoids floating-point noise in output).
function r1(v) { return v != null && Number.isFinite(v) ? Math.round(v * 10)    / 10    : null; }
function r3(v) { return v != null && Number.isFinite(v) ? Math.round(v * 1000)  / 1000  : null; }

// Extract verdict key and numeric excess from a Rust externally-tagged enum value.
// e.g. "FailBearing" → { verdict: "FailBearing", excess: null }
//      { FailBearing: { excess_deg: 33.2 } } → { verdict: "FailBearing", excess: 33.2 }
function parseVerdict(raw) {
  if (!raw || raw === 'Pass') return { verdict: 'Pass', excess: null };
  if (typeof raw === 'string') return { verdict: raw, excess: null };
  const key = Object.keys(raw)[0];
  const data = raw[key];
  const excess = data?.excess_deg ?? data?.score ?? null;
  return { verdict: key, excess: excess != null ? r1(excess) : null };
}

// ── Trace event extractor ─────────────────────────────────────────────────────

function getTraceEvents(events, variant) {
  return (events ?? [])
    .filter(e => e[variant] !== undefined)
    .map(e => e[variant]);
}

// ── Tool executor ─────────────────────────────────────────────────────────────

// storeActions: { setPinnedCandidates, runForcedDecodeAndGet, highlightSegments, flyTo }
export async function executeTool(name, args, { decodeResult, params, decoder, storeActions, forcedDecodeResult }) {
  if (!decodeResult) return JSON.stringify({ error: 'No decode result available.' });

  // When a forced decode is active, routing tools (leg summary, route segments, skipped edges)
  // should reflect the forced route, not the original multi-attempt trace.
  const isForced = !!forcedDecodeResult?.ok;
  const activeResult = isForced ? forcedDecodeResult : decodeResult;
  const routingEvents = activeResult.trace?.events ?? [];
  // Original trace is still needed for candidate tools and attempted-combinations.
  const events = decodeResult.trace?.events ?? [];

  switch (name) {

    case 'get_decode_summary': {
      const segs = activeResult.segments ?? [];
      const totalLengthM = segs.reduce((sum, s) => sum + (s.length_m ?? 0), 0);

      const scalars = {
        ok:                    activeResult.ok,
        forced_decode_active:  isForced,
        format:                decodeResult.format ?? null,
        error:                 activeResult.error  ?? null,
        segment_count:         segs.length,
        lrp_count:             decodeResult.lrps?.length ?? 0,
        path_total_length_m:   r1(totalLengthM),
        pos_offset_m:          decodeResult.pos_offset_lb != null
          ? `[${decodeResult.pos_offset_lb},${decodeResult.pos_offset_ub}]` : null,
        neg_offset_m:          decodeResult.neg_offset_lb != null
          ? `[${decodeResult.neg_offset_lb},${decodeResult.neg_offset_ub}]` : null,
        search_radius_m:       params?.candidate_search_radius_m,
        bearing_tolerance_deg: params?.max_bearing_deviation_deg,
        dnp_tolerance_pct:     params?.dnp_tolerance_pct,
        lfrcnp_tolerance:      params?.lfrcnp_tolerance,
        max_candidate_score:   params?.max_candidate_score,
        max_candidates_per_lrp: params?.max_candidates_per_lrp,
      };

      const pathRows = segs.map(s => ({
        seg_id:    s.segment_id,
        frc:       s.frc,
        fow:       s.fow,
        direction: s.direction,
        length_m:  r1(s.length_m),
      }));

      return toonResponse(scalars,
        pathRows.length
          ? [{ label: 'path', rows: pathRows, fields: ['seg_id','frc','fow','direction','length_m'] }]
          : []
      );
    }

    case 'get_parsed_reference': {
      const lrps = (decodeResult.lrps ?? []).map((l, i) => {
        const isLast = i === decodeResult.lrps.length - 1;
        return {
          index: i,
          lat: l.lat,
          lon: l.lon,
          bearing: { lb: l.bearing_lb, ub: l.bearing_ub },
          frc: l.frc,
          frc_label: FRC_LABELS[l.frc] ?? null,
          fow: l.fow,
          fow_label: FOW_LABELS[l.fow] ?? null,
          lfrcnp: isLast ? null : l.lfrcnp,
          dnp_m: isLast ? null
            : l.dnp_lb != null ? { lb: l.dnp_lb, ub: l.dnp_ub ?? l.dnp_lb }
            : null,
        };
      });
      return JSON.stringify({ lrps });
    }

    case 'get_lrp_candidates': {
      const { lrp_index, include_rejected = false } = args;
      const ranked = getTraceEvents(events, 'CandidatesRanked');
      const data = ranked.find(e => e.lrp_idx === lrp_index);
      if (!data) return JSON.stringify({ error: `No candidate trace data for LRP ${lrp_index}.` });

      const acceptedRows = (data.accepted ?? []).map(c => ({
        seg_id:      c.segment_id,
        traversal:   c.traversal,
        dist_m:      r1(c.projection?.distance_m),
        bearing_deg: r1(c.projection?.bearing_deg),
        dist_sc:     r3(c.score?.distance_score),
        bear_sc:     r3(c.score?.bearing_score),
        frc_sc:      r3(c.score?.frc_score),
        fow_sc:      r3(c.score?.fow_score),
        total:       r3(c.score?.total),
      }));

      const scalars = {
        lrp_index,
        accepted_count: acceptedRows.length,
        rejected_count: data.rejected_count ?? data.rejected?.length ?? 0,
      };

      const tables = [{
        label:  'accepted',
        rows:   acceptedRows,
        fields: ['seg_id','traversal','dist_m','bearing_deg','dist_sc','bear_sc','frc_sc','fow_sc','total'],
      }];

      if (include_rejected) {
        const rejectedRows = (data.rejected ?? []).map(r => {
          const { verdict, excess } = parseVerdict(r.verdict);
          return {
            seg_id:      r.segment_id,
            dist_m:      r.projection?.distance_m != null ? r1(r.projection.distance_m) : null,
            bearing_deg: r.projection?.bearing_deg != null ? r1(r.projection.bearing_deg) : null,
            verdict,
            excess,
          };
        });
        tables.push({
          label:  'rejected',
          rows:   rejectedRows,
          fields: ['seg_id','dist_m','bearing_deg','verdict','excess'],
        });
      }

      return toonResponse(scalars, tables);
    }

    case 'get_leg_summary': {
      const { leg_index } = args;
      const routing = {};
      for (const ev of routingEvents) {
        const [type, data] = Object.entries(ev)[0];
        if (data.leg !== leg_index) continue;
        switch (type) {
          case 'RouteFound':      routing.result = { found: true,  ...data }; break;
          case 'RouteFailed':     routing.result = { found: false, ...data }; break;
          case 'DnpChecked':      routing.dnp    = data;                      break;
          case 'AStarTerminated': routing.astar  = data;                      break;
          default: break;
        }
      }
      if (!Object.keys(routing).length) {
        return JSON.stringify({ error: `No routing trace data for leg ${leg_index}.` });
      }
      const r = routing.result;
      const d = routing.dnp;
      const a = routing.astar;
      return JSON.stringify({
        leg_index,
        route_found:       r?.found ?? null,
        route_length_m:    r?.found ? r.length_m : null,
        route_fail_reason: r?.found === false ? r.reason : null,
        dnp: d ? {
          actual_m:  d.actual_m,
          window_lb: d.interval?.lb,
          window_ub: d.interval?.ub,
          passed:    d.passed,
        } : null,
        astar: a ? {
          nodes_expanded:          a.nodes_expanded,
          edges_skipped_frc:       a.edges_skipped_frc,
          edges_skipped_direction: a.edges_skipped_direction,
          edges_skipped_turn:      a.edges_skipped_turn,
          edges_skipped_distance:  a.edges_skipped_distance,
          reason: a.reason,
        } : null,
      });
    }

    case 'get_route_segments': {
      const { leg_index } = args;
      const found = getTraceEvents(routingEvents, 'RouteFound');
      const data = found.find(e => e.leg === leg_index);
      if (!data) return JSON.stringify({ error: `No successful route found for leg ${leg_index}.` });

      const segById = new Map((activeResult.segments ?? []).map(s => [s.segment_id, s]));
      let cumul = 0;
      const segRows = (data.path ?? []).map(id => {
        const info = segById.get(id);
        const len = info?.length_m ?? null;
        if (len != null) cumul += len;
        return {
          seg_id:    id,
          frc:       info?.frc       ?? null,
          fow:       info?.fow       ?? null,
          direction: info?.direction ?? null,
          length_m:  r1(len),
          cumul_m:   r1(cumul),
        };
      });
      const sumLengthM = segRows.reduce((s, seg) => s + (seg.length_m ?? 0), 0);

      const fromSnap = data.from_snap;
      const toSnap   = data.to_snap;

      return toonResponse(
        {
          leg_index,
          segment_count: segRows.length,
          length_m:      r1(data.length_m),
          segment_sum_m: r1(sumLengthM),
          from_snap: fromSnap ? `${fromSnap[0]},${fromSnap[1]}` : null,
          to_snap:   toSnap   ? `${toSnap[0]},${toSnap[1]}`     : null,
        },
        [{ label: 'segments', rows: segRows, fields: ['seg_id','frc','fow','direction','length_m','cumul_m'] }]
      );
    }

    case 'get_segment_neighbors': {
      const { segment_id } = args;
      if (!decoder) return JSON.stringify({ error: 'Decoder not available.' });
      const raw = decoder.get_segment_neighbors(segment_id);
      const data = JSON.parse(raw);
      if (data.error) return raw;

      const neighborFields = ['seg_id','source_key','frc','fow','direction','length_m','can_arrive','can_depart'];
      const mapNeighbor = s => ({
        seg_id:     s.segment_id,
        source_key: s.source_key ?? null,
        frc:        s.frc,
        fow:        s.fow,
        direction:  s.direction,
        length_m:   r1(s.length_m),
        can_arrive: s.can_arrive,
        can_depart: s.can_depart,
      });

      return toonResponse(
        {
          segment_id,
          direction:  data.direction,
          start_node: data.start_node?.node_id,
          end_node:   data.end_node?.node_id,
        },
        [
          { label: 'at_start_node', rows: (data.start_node?.segments ?? []).map(mapNeighbor), fields: neighborFields },
          { label: 'at_end_node',   rows: (data.end_node?.segments   ?? []).map(mapNeighbor), fields: neighborFields },
        ]
      );
    }

    case 'get_segment': {
      const { segment_id } = args;
      if (!decoder) return JSON.stringify({ error: 'Decoder not available.' });
      const raw = decoder.get_segment(segment_id);
      const data = JSON.parse(raw);
      if (data.error) return raw;
      data.frc_label = FRC_LABELS[data.frc] ?? null;
      data.fow_label = FOW_LABELS_FULL[data.fow] ?? null;
      return JSON.stringify(data);
    }

    case 'get_segments_near': {
      const { lat, lon, radius_m = 100 } = args;
      if (!decoder) return JSON.stringify({ error: 'Decoder not available.' });
      const raw = decoder.get_segments_near(lat, lon, radius_m);
      const data = JSON.parse(raw);
      if (!data.segments) return raw;

      const segRows = data.segments.map(s => ({
        seg_id:     s.segment_id,
        source_key: s.source_key ?? null,
        frc:        s.frc,
        fow:        s.fow,
        direction:  s.direction,
        length_m:   r1(s.length_m),
        dist_m:     r1(s.distance_m),
      }));

      return toonResponse(
        { lat, lon, radius_m: data.query?.radius_m ?? radius_m, count: segRows.length },
        [{ label: 'segments', rows: segRows, fields: ['seg_id','source_key','frc','fow','direction','length_m','dist_m'] }]
      );
    }

    case 'highlight_segments': {
      const { segment_ids } = args;
      if (!storeActions) return JSON.stringify({ error: 'Store actions not available.' });
      storeActions.highlightSegments(segment_ids?.length ? segment_ids : null);
      return JSON.stringify({ ok: true, highlighted: segment_ids?.length ?? 0 });
    }

    case 'set_map_view': {
      const { lat, lon, zoom } = args;
      if (!storeActions) return JSON.stringify({ error: 'Store actions not available.' });
      storeActions.flyTo(lat, lon, zoom);
      return JSON.stringify({ ok: true });
    }

    case 'focus_lrp': {
      const { lrp_index } = args;
      if (!storeActions) return JSON.stringify({ error: 'Store actions not available.' });
      const lrp = decodeResult.lrps?.[lrp_index];
      if (!lrp) return JSON.stringify({ error: `LRP ${lrp_index} not found.` });
      storeActions.flyTo(lrp.lat, lrp.lon, 16);
      return JSON.stringify({ ok: true, lat: lrp.lat, lon: lrp.lon });
    }

    case 'get_astar_skipped_edges': {
      const { leg_index, segment_id } = args;
      const skipped = getTraceEvents(routingEvents, 'AStarEdgeSkipped').filter(e => e.leg === leg_index);
      if (!skipped.length) {
        const hasFullTrace = routingEvents.some(e => e.AStarEdgeSkipped !== undefined || e.AStarNodeExpanded !== undefined);
        if (!hasFullTrace) return JSON.stringify({ error: 'Full trace required. Set Trace Level → Full and decode again.' });
        return JSON.stringify({ leg_index, count: 0, note: 'No edges skipped on this leg.' });
      }
      const filtered = segment_id != null ? skipped.filter(e => e.segment_id === segment_id) : skipped;
      const rows = filtered.map(e => {
        const r = e.reason ?? {};
        const rKey = typeof r === 'string' ? r : Object.keys(r)[0] ?? 'Unknown';
        const rData = typeof r === 'object' ? r[rKey] : null;
        return {
          seg_id:  e.segment_id,
          reason:  rKey,
          seg_frc: rData?.seg_frc ?? null,
          lfrcnp:  rData?.lfrcnp  ?? null,
          dist_m:  rData?.distance_m != null ? r1(rData.distance_m) : null,
          max_m:   rData?.max_m    != null ? r1(rData.max_m)     : null,
        };
      });
      return toonResponse(
        { leg_index, total_skipped: skipped.length, shown: rows.length },
        [{ label: 'skipped', rows, fields: ['seg_id','reason','seg_frc','lfrcnp','dist_m','max_m'] }]
      );
    }

    case 'get_forced_leg_summary': {
      const { leg_index } = args;
      if (!forcedDecodeResult) return JSON.stringify({ error: 'No forced decode result. Call run_forced_decode first.' });
      const fEvents = forcedDecodeResult.trace?.events ?? [];
      const routing = {};
      for (const ev of fEvents) {
        const [type, data] = Object.entries(ev)[0];
        if (data.leg !== leg_index) continue;
        switch (type) {
          case 'RouteFound':      routing.result = { found: true,  ...data }; break;
          case 'RouteFailed':     routing.result = { found: false, ...data }; break;
          case 'DnpChecked':      routing.dnp    = data;                      break;
          case 'AStarTerminated': routing.astar  = data;                      break;
          default: break;
        }
      }
      if (!Object.keys(routing).length) return JSON.stringify({ error: `No routing trace for forced leg ${leg_index}.` });
      const r = routing.result, d = routing.dnp, a = routing.astar;
      return JSON.stringify({
        leg_index,
        route_found:       r?.found ?? null,
        route_length_m:    r?.found ? r.length_m : null,
        route_fail_reason: r?.found === false ? r.reason : null,
        dnp: d ? { actual_m: d.actual_m, window_lb: d.interval?.lb, window_ub: d.interval?.ub, passed: d.passed } : null,
        astar: a ? {
          nodes_expanded:          a.nodes_expanded,
          edges_skipped_frc:       a.edges_skipped_frc,
          edges_skipped_direction: a.edges_skipped_direction,
          edges_skipped_turn:      a.edges_skipped_turn,
          edges_skipped_distance:  a.edges_skipped_distance,
          reason: a.reason,
        } : null,
      });
    }

    case 'get_attempted_combinations': {
      const started = getTraceEvents(events, 'RouteSearchStarted');
      const exhausted = getTraceEvents(events, 'RouteAttemptsExhausted')[0] ?? null;
      // Group by combination: a new combination starts each time leg resets to 0 (or first event)
      const combos = [];
      let current = null;
      for (const s of started) {
        if (s.leg === 0 || current === null) { current = { legs: [] }; combos.push(current); }
        // Find outcome event for this specific leg search (next RouteFound/RouteFailed for this leg after this start)
        current.legs.push({ leg: s.leg, from_seg: s.from.segment_id, from_trav: s.from.traversal, to_seg: s.to.segment_id, to_trav: s.to.traversal });
      }
      // Match outcomes: RouteFound/RouteFailed events per leg
      const foundEvts  = getTraceEvents(events, 'RouteFound');
      const failedEvts = getTraceEvents(events, 'RouteFailed');
      const rows = started.map((s, idx) => {
        // Find matching outcome by leg — simplified: same leg order in events
        const rf = foundEvts.find((e, i) => e.leg === s.leg && i === started.slice(0, idx + 1).filter(x => x.leg === s.leg).length - 1);
        const fail = failedEvts.find((e, i) => e.leg === s.leg && i === started.slice(0, idx + 1).filter(x => x.leg === s.leg).length - 1);
        const outcome = rf ? `found(${r1(rf.length_m)}m)` : fail ? `failed` : 'incomplete';
        return { leg: s.leg, from_seg: s.from.segment_id, from_trav: s.from.traversal, to_seg: s.to.segment_id, to_trav: s.to.traversal, outcome };
      });
      return toonResponse(
        { total_attempts: started.length, cap_hit: exhausted != null, cap_limit: exhausted?.limit ?? null },
        [{ label: 'attempts', rows, fields: ['leg','from_seg','from_trav','to_seg','to_trav','outcome'] }]
      );
    }

    case 'retry_decode': {
      const { params_override } = args;
      if (!decoder) return JSON.stringify({ error: 'Decoder not available.' });
      const overrideStr = typeof params_override === 'string'
        ? params_override
        : JSON.stringify(params_override);
      return decoder.retry_decode(overrideStr);
    }

    case 'set_pinned_candidates': {
      const { snaps } = args;
      if (!storeActions) return JSON.stringify({ error: 'Store actions not available.' });
      const ranked = getTraceEvents(events, 'CandidatesRanked');
      const resolved = [];
      const errs = [];
      for (const { lrp_index, segment_id, traversal } of snaps) {
        const lrpData = ranked.find(e => e.lrp_idx === lrp_index);
        if (!lrpData) { errs.push(`LRP ${lrp_index}: no candidate trace data`); continue; }
        const c = (lrpData.accepted ?? []).find(
          a => a.segment_id === segment_id && a.traversal === traversal
        );
        if (!c) { errs.push(`LRP ${lrp_index}: segment ${segment_id} (${traversal}) not in accepted list`); continue; }
        resolved.push({
          lrp_index,
          segment_id:   c.segment_id,
          traversal:    c.traversal,
          arc_offset_m: c.projection.arc_offset_m,
          snap_lon:     c.projection.point[0],
          snap_lat:     c.projection.point[1],
        });
      }
      if (errs.length) return JSON.stringify({ error: errs.join('; ') });
      await storeActions.setPinnedCandidates(resolved);
      return JSON.stringify({ ok: true, pinned_count: resolved.length });
    }

    case 'run_forced_decode': {
      if (!storeActions) return JSON.stringify({ error: 'Store actions not available.' });
      const result = await storeActions.runForcedDecodeAndGet();
      if (!result) return JSON.stringify({ error: 'Not all LRPs pinned. Call set_pinned_candidates first.' });
      if (!result.ok) return JSON.stringify({ ok: false, error: result.error ?? 'Forced decode failed.' });
      const segs = result.segments ?? [];
      const totalLengthM = segs.reduce((s, seg) => s + (seg.length_m ?? 0), 0);
      const forcedEvents = result.trace?.events ?? [];
      const legResults = getTraceEvents(forcedEvents, 'DnpChecked').map(d => ({
        leg:        d.leg,
        actual_m:   r1(d.actual_m),
        window_lb:  r1(d.interval?.lb),
        window_ub:  r1(d.interval?.ub),
        dnp_passed: d.passed,
      }));
      return toonResponse(
        { ok: true, segment_count: segs.length, path_total_length_m: r1(totalLengthM) },
        legResults.length
          ? [{ label: 'legs', rows: legResults, fields: ['leg','actual_m','window_lb','window_ub','dnp_passed'] }]
          : []
      );
    }

    default:
      return JSON.stringify({ error: `Tool "${name}" is not yet implemented in this version.` });
  }
}
