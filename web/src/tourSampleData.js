// Illustrative, entirely made-up decode result used only while the
// onboarding tour is showing the Results/Trace panels -- not a real decode,
// so it works regardless of which tileset/region the user has loaded (no
// tile fetch, no dependency on the sample location actually existing in
// whatever they've configured). Shaped exactly like a real JsDecodeResult so
// it renders through the same, real ResultPanel/TracePanel components.
export const TOUR_SAMPLE_OPENLR_STRING = 'CwRcByNgWv9OAP8=';

export const TOUR_SAMPLE_DECODE_RESULT = {
  ok: true,
  format: 'TomTomV3',
  location_type: 'Line',
  wkt: 'LINESTRING (4.9000000 52.3700000, 4.9012000 52.3704000, 4.9025000 52.3709000)',
  segments: [
    {
      frc: 3, fow: 2, direction: 'Forward', length_m: 145.2,
      stable_id: 'tour-demo-1', tile: '12/2107/1338', local_index: 0, segment_id: 900001,
      geometry: [[4.9000000, 52.3700000], [4.9012000, 52.3704000]],
    },
    {
      frc: 3, fow: 2, direction: 'Forward', length_m: 118.7,
      stable_id: 'tour-demo-2', tile: '12/2107/1338', local_index: 1, segment_id: 900002,
      geometry: [[4.9012000, 52.3704000], [4.9025000, 52.3709000]],
    },
  ],
  lrps: [
    {
      lon: 4.9000000, lat: 52.3700000, frc: 3, fow: 2, lfrcnp: 4,
      bearing_lb: 42.0, bearing_ub: 42.0, dnp_lb: 264.0, dnp_ub: 264.0,
    },
    {
      lon: 4.9025000, lat: 52.3709000, frc: 3, fow: 2, lfrcnp: null,
      bearing_lb: 45.5, bearing_ub: 45.5, dnp_lb: null, dnp_ub: null,
    },
  ],
  pos_offset_lb: 12.0, pos_offset_ub: 12.0,
  neg_offset_lb: 8.0,  neg_offset_ub: 8.0,
  covered_start_idx: 0, covered_end_idx: 1,
  covered_pos_offset_lb: 12.0, covered_pos_offset_ub: 12.0,
  covered_neg_offset_lb: 8.0,  covered_neg_offset_ub: 8.0,
  offsets_approximate: false,
};
