// Shared formatting/labeling for the "Reference" (location-reference-point)
// display used by both ResultPanel.jsx and TracePanel.jsx. Kept in one place
// so wording (units, precision, range notation, FRC/FOW/orientation labels)
// can't silently drift between the two panels the way it had before this
// was consolidated -- e.g. "TomTomV3 (binary v3)" vs "TomTom v3" for the
// same format, or DNP/LFRCNP shown on the last LRP in one panel but not the
// other (that field is meaningless there -- no "next point" after the last).

// Same "single source of truth" reasoning as the labels below -- rendered
// both as the `?` tooltips next to each field in ResultPanel's Reference
// section, and (unmodified) as the Decode Parameters section of the in-app
// Documentation panel, so the two can't drift apart from each other.
export const HELP = {
  frc:     'Functional Road Class (0–7): how important the road is (0 = motorway, 7 = local path). Candidates must match within the configured FRC tolerance.',
  fow:     'Form of Way: the geometric road type (motorway, dual carriageway, roundabout, slip road, etc.).',
  bearing: 'Direction of travel at this LRP, clockwise from North. TomTomV3 uses an 11.25° sector (32 sectors); TPEG-OLR uses a 1.41° sector (256 sectors). Decoded against the interval ± the map tolerance.',
  dnp:     'Distance to Next Point: encoded path length from this LRP to the next (meters). TomTomV3 quantises into ~58.6 m buckets (max ~14,901 m); TPEG-OLR is exact. The found route length must fall within this interval ± tolerance.',
  lfrcnp:  'Lowest FRC to Next Point: the least-important road class the A* path between this LRP and the next may use. Prevents re-routing via minor roads when the encoder used a motorway.',
  offset:  'Trim distance applied after route validation — positive from the path start, negative from the path end.',
};

export const FRC_LABEL = [
  'FRC0 · Motorway', 'FRC1 · Trunk', 'FRC2 · Secondary', 'FRC3 · Tertiary',
  'FRC4 · Unclassified', 'FRC5 · Residential', 'FRC6 · Service/Link', 'FRC7 · Other/Path',
];

export const FOW_LABEL = [
  'Undefined', 'Motorway', 'Dual Carriageway', 'Single Carriageway',
  'Roundabout', 'Traffic Square', 'Slip Road', 'Other',
];

export const ORIENTATION_LABEL = {
  NoOrientation:     'No orientation',
  FirstTowardSecond: 'First → Second',
  SecondTowardFirst: 'Second → First',
  BothDirections:    'Both directions',
};

export const SIDE_OF_ROAD_LABEL = {
  DirectlyOnOrNA: 'Directly on / N/A',
  Right:          'Right',
  Left:           'Left',
  Both:           'Both sides',
};

export function isPointAlongLine(locationType) {
  return locationType === 'PointAlongLine' || locationType === 'PoiWithAccessPoint';
}

export function formatOpenlrFormat(format) {
  if (format === 'TomTomV3') return 'TomTomV3 (binary v3)';
  if (format === 'Tpeg')     return 'TPEG-OLR (ISO 21219-22)';
  return '(unknown)';
}

export function frcLabel(frc) {
  return FRC_LABEL[frc] ?? `FRC${frc}`;
}

export function fowLabel(fow) {
  return FOW_LABEL[fow] != null ? `FOW${fow} · ${FOW_LABEL[fow]}` : `FOW${fow}`;
}

export function fmtBearing(lb, ub) {
  return Math.abs(ub - lb) < 0.1 ? `${lb.toFixed(1)}°` : `${lb.toFixed(1)}°–${ub.toFixed(1)}°`;
}

// General meter-range formatter (DNP, offsets) -- null only means "no data",
// callers that need to suppress a legitimate all-zero interval (e.g. "no
// offset configured") gate on that themselves before calling this.
export function fmtInterval(lb, ub) {
  if (lb == null) return null;
  return Math.abs(ub - lb) < 0.1 ? `${lb.toFixed(0)} m` : `${lb.toFixed(0)}–${ub.toFixed(0)} m`;
}

export function fmtOffsetValue(lb, ub, approximate) {
  const str = fmtInterval(lb, ub);
  if (str == null) return null;
  return approximate ? `${str} *` : str;
}

// For Line locations, the Pos/Neg Offset rows are always shown (explicitly
// "N/A" when the reference didn't encode one) rather than the row silently
// disappearing -- makes clear the field was considered, not just missing
// from the display.
export function offsetRowValue(hasValue, lb, ub, approximate) {
  return hasValue ? fmtOffsetValue(lb, ub, approximate) : 'N/A';
}

// Compact form for collapsed/summary rows -- just the number(s), no label text.
export function lfrcnpCompact(lfrcnp, tolerance = 0) {
  if (lfrcnp == null) return '—';
  return tolerance > 0 ? `${lfrcnp} → ${Math.min(lfrcnp + tolerance, 7)}` : `${lfrcnp}`;
}

// Full descriptive form for expanded/detail rows.
export function lfrcnpFull(lfrcnp, tolerance = 0) {
  if (lfrcnp == null) return '—';
  if (tolerance > 0) {
    return `${frcLabel(lfrcnp)} → ${frcLabel(Math.min(lfrcnp + tolerance, 7))}`;
  }
  return frcLabel(lfrcnp);
}
