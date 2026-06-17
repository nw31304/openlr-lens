/**
 * Parse a WKT LINESTRING into a GeoJSON Feature.
 * Returns null if the WKT is missing or malformed.
 */
export function wktToGeoJSON(wkt) {
  const m = wkt?.match(/^LINESTRING \((.+)\)$/);
  if (!m) return null;
  const coordinates = m[1].split(',').map(p => {
    const [lon, lat] = p.trim().split(' ').map(Number);
    return [lon, lat];
  });
  return { type: 'Feature', geometry: { type: 'LineString', coordinates }, properties: {} };
}
