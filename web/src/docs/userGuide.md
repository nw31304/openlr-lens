# OpenLRLab User Guide

A browser-based, visual diagnostic tool for decoding and encoding [OpenLR](https://www.openlr-association.com/)
location references — both TomTomV3 (binary) and TPEG-OLR (ISO 21219-22) formats.
Everything runs client-side; the only network activity is fetching map tiles.

## Decode vs Encode

The mode toggle at the top-left switches between two workflows:

- **Decode** — paste an existing OpenLR string (TomTomV3 or TPEG-OLR, both
  base64-encoded) into the input at the bottom of the screen and press
  Decode. The format is detected automatically.
- **Encode** — draw waypoints on the map to build a new Line or
  Point-Along-Line location, then encode it to both binary formats. The
  result is automatically decoded back (a round-trip verify) so you can
  confirm it's correct before using it.

## The four views

Toggle these independently from the menu bar — any combination can be open
at once:

- **Segments** — an overlay showing the raw road-segment graph on the map,
  independent of any decode.
- **Results** (left panel) — the at-a-glance answer: what a reference
  decoded to, its constituent segments, and the Location Reference Points
  (LRPs) that produced it. Each LRP row is collapsible — click the arrow to
  expand FRC/FOW/bearing/DNP/LFRCNP detail.
- **Trace** (right panel) — the deep-dive: which candidates were considered
  at each LRP and why, the A\* routing result for each leg, and the final
  offset trim. This is where you find out *why* the decoder chose what it
  chose, not just what it chose.
- **Replay** — step through the decode (or an encode's verify) one decision
  at a time: candidate search, A\* routing, offset trimming, all animated
  live on the map exactly as the engine experienced it. Step forward, back,
  or auto-play the whole sequence.

## Understanding a decode result

A decoded location is trimmed by its positive/negative offsets — the actual
covered extent can be shorter than the full routed path between LRPs.
Segments entirely outside that trimmed extent still appear in the segment
list (useful context), marked with a small **\*** and a caption explaining
they're bypassed by the offsets and not part of the final location.

The **Export GeoJSON** button (Results panel) and **Copy WKT** / **Copy
GeoJSON** buttons (Trace panel, Result section) both reflect the
conservatively-trimmed extent — segments entirely outside it are excluded
from the exported geometry, and the exported offset values are re-expressed
relative to the exported segment list's own boundary, not the original LRP
position.

## Forced re-decode ("what if?")

From the Trace panel, pin specific candidates at each LRP (instead of
whatever the decoder picked automatically) and re-run routing with exactly
those choices. Useful for exploring "what if the decoder had picked this
other candidate instead?" without needing a different input string.

## Bringing your own map

This tool isn't tied to one map provider. The **Tile source** menu points
the app at any [PMTiles](https://protomaps.com/) archive you build or host
yourself — TomTom, OpenStreetMap, Overture, ESRI, or anything else — for
both decoding and encoding. There's no server-side component beyond serving
that one archive.

## AI Chat

If configured (AI / LLM settings), an AI chat assistant can answer
questions about the current decode — referencing the same trace data shown
in the Trace panel — using tool calls into the live decoder rather than
guessing.

## Decode Parameters reference

Every tunable value the decoder uses to score candidates and validate
routes:
