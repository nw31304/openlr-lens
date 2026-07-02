use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

use crate::{CircularInterval, LinearInterval};
use crate::lrp::{LocationReference, Orientation, SideOfRoad, Lrp};
use super::DecodeError;

// ── Constants ──────────────────────────────────────────────────────────────────

const BEARING_SECTOR_DEG: f64 = 360.0 / 32.0;  // 11.25°
const DNP_BUCKET_M: f64       = 15_000.0 / 256.0; // ≈58.59375 m

// ── Public entry points ────────────────────────────────────────────────────────

pub fn decode_v3(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    // Smallest valid type is GeoCoordinate at 7 bytes.
    if bytes.len() < 7 {
        return Err(DecodeError::TooShort { min: 7, got: bytes.len() });
    }

    // Byte 0: status.  Bits 2-0 must be 3 (v3).
    // Bit 5 = point_flag, bits [6,4] = area_flag, bit 3 = attr_flag.
    let status = bytes[0];
    if status & 0x07 != 3 {
        return Err(DecodeError::InvalidHeader(status));
    }

    let is_point  = status & 0x20 != 0;
    let area_flag = ((status & 0x40) >> 5) | ((status & 0x10) >> 4);
    let has_attrs = status & 0x08 != 0;

    match (area_flag, is_point, has_attrs) {
        // Line: area=0, point=0, attrs=1  (status=0x0B)
        (0, false, true)  => decode_line(bytes),
        // Circle: area=0, point=0, attrs=0  (status=0x03)
        (0, false, false) => decode_circle(bytes),
        // GeoCoordinate: area=0, point=1, attrs=0  (status=0x23)
        (0, true,  false) => decode_geocoord(bytes),
        // PointAlongLine or PoiWithAccessPoint: area=0, point=1, attrs=1  (status=0x2B)
        (0, true,  true)  => decode_pal_or_poi(bytes),
        // Polygon: area=1  (status=0x13)
        (1, false, _)     => decode_polygon(bytes),
        // Rectangle / Grid: area=2  (status=0x43)
        (2, false, _)     => decode_rect_or_grid(bytes),
        // ClosedLine: area=3, point=0, attrs=1  (status=0x5B)
        (3, false, true)  => decode_closed_line(bytes),
        _                 => Err(DecodeError::InvalidHeader(status)),
    }
}

pub fn decode_v3_base64(s: &str) -> Result<LocationReference, DecodeError> {
    let bytes = B64.decode(s).map_err(|e| DecodeError::Base64(e.to_string()))?;
    decode_v3(&bytes)
}

// ── Network location types ─────────────────────────────────────────────────────

/// Decode a v3 Line location reference.
fn decode_line(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    // Minimum: header(1) + first LRP(9) + last LRP(6) = 16 bytes.
    if bytes.len() < 16 {
        return Err(DecodeError::TooShort { min: 16, got: bytes.len() });
    }
    // Remainder must be: 9 + 7*(n-2) + 6 + [0,1,2] extra bytes.
    // n = bytes.len() / 7  (integer division);  offsets = (bytes.len()-16) % 7.
    let n_lrps = bytes.len() / 7;
    if n_lrps < 2 {
        return Err(DecodeError::TooShort { min: 16, got: bytes.len() });
    }
    let n_offsets = (bytes.len().wrapping_sub(16)) % 7;
    if n_offsets > 2 {
        return Err(DecodeError::TrailingBytes(n_offsets));
    }

    // ── First LRP (absolute coords) ──────────────────────────────────────────
    let lon0 = decode_abs_coord(bytes[1], bytes[2], bytes[3]);
    let lat0 = decode_abs_coord(bytes[4], bytes[5], bytes[6]);
    let (frc0, fow0)        = decode_attr1(bytes[7]);
    let (lfrcnp0, bearing0) = decode_attr2(bytes[8]);
    let dnp0                = decode_dnp(bytes[9]);

    let first_lrp = Lrp {
        coord:      (lon0, lat0),
        bearing:    bearing0,
        frc:        frc0,
        fow:        fow0,
        lfrcnp:     Some(lfrcnp0),
        dnp:        Some(dnp0),
        pos_offset: None, neg_offset: None,
        pos_offset_raw: None, neg_offset_raw: None,
    };

    let mut lrps: Vec<Lrp> = vec![first_lrp];

    // ── Intermediate LRPs (relative coords) ─────────────────────────────────
    let mut pos = 10_usize;
    for _ in 0..n_lrps - 2 {
        let (prev_lon, prev_lat) = lrps.last().map(|l| l.coord).expect("lrps starts non-empty and only grows");
        let lon = decode_rel_coord(bytes[pos],     bytes[pos + 1], prev_lon);
        let lat = decode_rel_coord(bytes[pos + 2], bytes[pos + 3], prev_lat);
        let (frc, fow)       = decode_attr1(bytes[pos + 4]);
        let (lfrcnp, bearing) = decode_attr2(bytes[pos + 5]);
        let dnp              = decode_dnp(bytes[pos + 6]);
        lrps.push(Lrp {
            coord:      (lon, lat),
            bearing,
            frc,
            fow,
            lfrcnp:     Some(lfrcnp),
            dnp:        Some(dnp),
            pos_offset: None, neg_offset: None,
            pos_offset_raw: None, neg_offset_raw: None,
        });
        pos += 7;
    }

    // ── Last LRP ─────────────────────────────────────────────────────────────
    let (prev_lon, prev_lat) = lrps.last().map(|l| l.coord).expect("lrps is non-empty after first_lrp push");
    let lon_last = decode_rel_coord(bytes[pos],     bytes[pos + 1], prev_lon);
    let lat_last = decode_rel_coord(bytes[pos + 2], bytes[pos + 3], prev_lat);
    let (frc_last, fow_last)              = decode_attr1(bytes[pos + 4]);
    let (has_pos_off, has_neg_off, brng)  = decode_attr4(bytes[pos + 5]);
    // pos+6 is the start of the optional offset bytes

    lrps.push(Lrp {
        coord:      (lon_last, lat_last),
        bearing:    brng,
        frc:        frc_last,
        fow:        fow_last,
        lfrcnp:     None,
        dnp:        None,
        pos_offset: None, neg_offset: None,
        pos_offset_raw: None, neg_offset_raw: None,
    });

    // ── Offsets (spec §7.5.2) ─────────────────────────────────────────────────
    // Raw offset bytes are stored here; the engine computes the metric interval
    // [N/256 × L, (N+1)/256 × L] once the total A*-found path length L is known.
    let offset_start = pos + 6;
    let mut off_idx = offset_start;

    if has_pos_off {
        lrps[0].pos_offset_raw = Some(bytes[off_idx]);
        off_idx += 1;
    }
    if has_neg_off {
        let last = lrps.len() - 1;
        lrps[last].neg_offset_raw = Some(bytes[off_idx]);
    }

    Ok(LocationReference::Line { lrps })
}

/// Decode a v3 PointAlongLine (PAL) location reference.
///
/// Binary layout (16 or 17 bytes):
///   byte 0:     header (already validated)
///   bytes 1-3:  lon (24-bit absolute)
///   bytes 4-6:  lat (24-bit absolute)
///   byte 7:     bits[7:6]=orientation  bits[5:3]=FRC  bits[2:0]=FOW
///   byte 8:     bits[7:5]=LFRCNP  bits[4:0]=bearing sector
///   byte 9:     DNP
///   bytes 10-11: relative lon
///   bytes 12-13: relative lat
///   byte 14:    bits[7:6]=side-of-road  bits[5:3]=FRC  bits[2:0]=FOW
///   byte 15:    bits[4:0]=bearing sector  (no offset flags)
///   byte 16:    positive offset raw (optional, present when len==17)
fn decode_pal(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    if bytes.len() != 16 && bytes.len() != 17 {
        return Err(DecodeError::TooShort { min: 16, got: bytes.len() });
    }

    let lon0 = decode_abs_coord(bytes[1], bytes[2], bytes[3]);
    let lat0 = decode_abs_coord(bytes[4], bytes[5], bytes[6]);

    let attr1_first  = bytes[7];
    let orientation  = Orientation::from_u8(attr1_first >> 6);
    let (frc0, fow0) = decode_attr1(attr1_first);
    let (lfrcnp0, bearing0) = decode_attr2(bytes[8]);
    let dnp0 = decode_dnp(bytes[9]);

    let lon1 = decode_rel_coord(bytes[10], bytes[11], lon0);
    let lat1 = decode_rel_coord(bytes[12], bytes[13], lat0);

    let attr1_last   = bytes[14];
    let side_of_road = SideOfRoad::from_u8(attr1_last >> 6);
    let (frc1, fow1) = decode_attr1(attr1_last);

    // PAL last LRP attr4: bits[4:0] = bearing sector only (no pos/neg offset flags).
    let bearing1 = bearing_sector_to_interval(bytes[15] & 0x1F);

    let pos_offset_raw = if bytes.len() == 17 { Some(bytes[16]) } else { None };

    let lrp0 = Lrp {
        coord:      (lon0, lat0),
        bearing:    bearing0,
        frc:        frc0,
        fow:        fow0,
        lfrcnp:     Some(lfrcnp0),
        dnp:        Some(dnp0),
        pos_offset: None,
        neg_offset: None,
        pos_offset_raw,
        neg_offset_raw: None,
    };
    let lrp1 = Lrp {
        coord:      (lon1, lat1),
        bearing:    bearing1,
        frc:        frc1,
        fow:        fow1,
        lfrcnp:     None,
        dnp:        None,
        pos_offset: None, neg_offset: None,
        pos_offset_raw: None, neg_offset_raw: None,
    };

    Ok(LocationReference::PointAlongLine { lrps: vec![lrp0, lrp1], orientation, side_of_road })
}

/// Dispatch to PAL (16/17 bytes) or PoiWithAccessPoint (22/23 bytes).
fn decode_pal_or_poi(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    match bytes.len() {
        16 | 17 => decode_pal(bytes),
        22 | 23 => decode_poi(bytes),
        n if n < 16 => Err(DecodeError::TooShort { min: 16, got: n }),
        _ => Err(DecodeError::TrailingBytes(bytes.len() - 17)),
    }
}

/// Decode a v3 PoiWithAccessPoint location reference (22 or 23 bytes).
///
/// Layout: same as PAL (bytes 0-15) + POI absolute coord (bytes 16-21)
///       + optional pos-offset raw byte (byte 22).
fn decode_poi(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    if bytes.len() < 22 {
        return Err(DecodeError::TooShort { min: 22, got: bytes.len() });
    }

    let lon0 = decode_abs_coord(bytes[1], bytes[2], bytes[3]);
    let lat0 = decode_abs_coord(bytes[4], bytes[5], bytes[6]);

    let attr1_first  = bytes[7];
    let orientation  = Orientation::from_u8(attr1_first >> 6);
    let (frc0, fow0) = decode_attr1(attr1_first);
    let (lfrcnp0, bearing0) = decode_attr2(bytes[8]);
    let dnp0 = decode_dnp(bytes[9]);

    let lon1 = decode_rel_coord(bytes[10], bytes[11], lon0);
    let lat1 = decode_rel_coord(bytes[12], bytes[13], lat0);

    let attr1_last   = bytes[14];
    let side_of_road = SideOfRoad::from_u8(attr1_last >> 6);
    let (frc1, fow1) = decode_attr1(attr1_last);
    let bearing1     = bearing_sector_to_interval(bytes[15] & 0x1F);

    let poi_lon = decode_abs_coord(bytes[16], bytes[17], bytes[18]);
    let poi_lat = decode_abs_coord(bytes[19], bytes[20], bytes[21]);

    let pos_offset_raw = if bytes.len() == 23 { Some(bytes[22]) } else { None };

    let lrp0 = Lrp {
        coord: (lon0, lat0), bearing: bearing0, frc: frc0, fow: fow0,
        lfrcnp: Some(lfrcnp0), dnp: Some(dnp0),
        pos_offset: None, neg_offset: None,
        pos_offset_raw, neg_offset_raw: None,
    };
    let lrp1 = Lrp {
        coord: (lon1, lat1), bearing: bearing1, frc: frc1, fow: fow1,
        lfrcnp: None, dnp: None,
        pos_offset: None, neg_offset: None,
        pos_offset_raw: None, neg_offset_raw: None,
    };

    Ok(LocationReference::PoiWithAccessPoint {
        lrps: vec![lrp0, lrp1],
        orientation,
        side_of_road,
        poi: (poi_lon, poi_lat),
    })
}

/// Decode a v3 ClosedLine location reference.
///
/// Size = 1 + 9 + (n-3)*7 + 2 for n ≥ 3.
/// The last LRP has no coordinate bytes — it shares the first LRP's coordinate.
fn decode_closed_line(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    // Minimum (n=3): 1 + 9 + 2 = 12 bytes
    if bytes.len() < 12 {
        return Err(DecodeError::TooShort { min: 12, got: bytes.len() });
    }
    let n_intermediate = if bytes.len() > 12 {
        let remainder = bytes.len() - 12;
        if remainder % 7 != 0 {
            return Err(DecodeError::TrailingBytes(remainder % 7));
        }
        remainder / 7
    } else {
        0
    };

    // First LRP (absolute)
    let lon0 = decode_abs_coord(bytes[1], bytes[2], bytes[3]);
    let lat0 = decode_abs_coord(bytes[4], bytes[5], bytes[6]);
    let (frc0, fow0)        = decode_attr1(bytes[7]);
    let (lfrcnp0, bearing0) = decode_attr2(bytes[8]);
    let dnp0                = decode_dnp(bytes[9]);

    let mut lrps = vec![Lrp {
        coord: (lon0, lat0), bearing: bearing0, frc: frc0, fow: fow0,
        lfrcnp: Some(lfrcnp0), dnp: Some(dnp0),
        pos_offset: None, neg_offset: None,
        pos_offset_raw: None, neg_offset_raw: None,
    }];

    // Intermediate LRPs (relative)
    let mut pos = 10;
    for _ in 0..n_intermediate {
        let (prev_lon, prev_lat) = lrps.last().map(|l| l.coord).unwrap();
        let lon = decode_rel_coord(bytes[pos], bytes[pos + 1], prev_lon);
        let lat = decode_rel_coord(bytes[pos + 2], bytes[pos + 3], prev_lat);
        let (frc, fow)        = decode_attr1(bytes[pos + 4]);
        let (lfrcnp, bearing) = decode_attr2(bytes[pos + 5]);
        let dnp               = decode_dnp(bytes[pos + 6]);
        lrps.push(Lrp {
            coord: (lon, lat), bearing, frc, fow,
            lfrcnp: Some(lfrcnp), dnp: Some(dnp),
            pos_offset: None, neg_offset: None,
            pos_offset_raw: None, neg_offset_raw: None,
        });
        pos += 7;
    }

    // Last/closing LRP: no coordinates (shares first LRP's), attr1 + attr4 only.
    let (frc_last, fow_last)       = decode_attr1(bytes[pos]);
    let (_, _, bearing_last)       = decode_attr4(bytes[pos + 1]);
    lrps.push(Lrp {
        coord: (lon0, lat0), // closes back to the first LRP
        bearing: bearing_last, frc: frc_last, fow: fow_last,
        lfrcnp: None, dnp: None,
        pos_offset: None, neg_offset: None,
        pos_offset_raw: None, neg_offset_raw: None,
    });

    Ok(LocationReference::ClosedLine { lrps })
}

// ── Geometry / area location types ────────────────────────────────────────────

/// Decode a v3 GeoCoordinate location reference (7 bytes).
fn decode_geocoord(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    if bytes.len() != 7 {
        return Err(DecodeError::TooShort { min: 7, got: bytes.len() });
    }
    let lon = decode_abs_coord(bytes[1], bytes[2], bytes[3]);
    let lat = decode_abs_coord(bytes[4], bytes[5], bytes[6]);
    Ok(LocationReference::GeoCoordinate { coord: (lon, lat) })
}

/// Decode a v3 Circle location reference (10 bytes).
///
/// Layout: header(1) + center_lon(3) + center_lat(3) + radius_m(3, big-endian unsigned).
fn decode_circle(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    if bytes.len() < 10 {
        return Err(DecodeError::TooShort { min: 10, got: bytes.len() });
    }
    let lon      = decode_abs_coord(bytes[1], bytes[2], bytes[3]);
    let lat      = decode_abs_coord(bytes[4], bytes[5], bytes[6]);
    let radius_m = (bytes[7] as u32) << 16 | (bytes[8] as u32) << 8 | bytes[9] as u32;
    Ok(LocationReference::Circle { center: (lon, lat), radius_m })
}

/// Decode a v3 Rectangle (13 bytes) or Grid (15 bytes) location reference.
///
/// Rectangle layout: header(1) + ll_lon(3) + ll_lat(3) + ur_lon(3) + ur_lat(3).
/// Grid adds 1 byte each for n_cols and n_rows (stored as raw value - 1 per spec).
fn decode_rect_or_grid(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    if bytes.len() < 13 {
        return Err(DecodeError::TooShort { min: 13, got: bytes.len() });
    }
    let lon_ll = decode_abs_coord(bytes[1],  bytes[2],  bytes[3]);
    let lat_ll = decode_abs_coord(bytes[4],  bytes[5],  bytes[6]);
    let lon_ur = decode_abs_coord(bytes[7],  bytes[8],  bytes[9]);
    let lat_ur = decode_abs_coord(bytes[10], bytes[11], bytes[12]);

    if bytes.len() >= 15 {
        let n_cols = bytes[13] as u16 + 1;
        let n_rows = bytes[14] as u16 + 1;
        Ok(LocationReference::Grid {
            lower_left:  (lon_ll, lat_ll),
            upper_right: (lon_ur, lat_ur),
            n_cols,
            n_rows,
        })
    } else {
        Ok(LocationReference::Rectangle {
            lower_left:  (lon_ll, lat_ll),
            upper_right: (lon_ur, lat_ur),
        })
    }
}

/// Decode a v3 Polygon location reference.
///
/// Layout: header(1) + n × (lon(3) + lat(3)), all coordinates absolute.
/// Minimum 3 vertices (19 bytes).
fn decode_polygon(bytes: &[u8]) -> Result<LocationReference, DecodeError> {
    if bytes.len() < 19 {
        return Err(DecodeError::TooShort { min: 19, got: bytes.len() });
    }
    let remaining = bytes.len() - 1;
    if remaining % 6 != 0 {
        return Err(DecodeError::TrailingBytes(remaining % 6));
    }
    let n = remaining / 6;
    let mut coords = Vec::with_capacity(n);
    let mut p = 1;
    for _ in 0..n {
        let lon = decode_abs_coord(bytes[p], bytes[p + 1], bytes[p + 2]);
        let lat = decode_abs_coord(bytes[p + 3], bytes[p + 4], bytes[p + 5]);
        coords.push((lon, lat));
        p += 6;
    }
    Ok(LocationReference::Polygon { coords })
}

// ── Byte-level helpers ─────────────────────────────────────────────────────────

/// Decode a big-endian signed 24-bit integer to WGS84 degrees.
/// Formula (OpenLR whitepaper §8): deg = (i − sgn(i)·0.5) × 360 / 2^24
pub fn decode_abs_coord(hi: u8, mi: u8, lo: u8) -> f64 {
    let u = (hi as u32) << 16 | (mi as u32) << 8 | lo as u32;
    let i = if u >= 0x80_0000 { u as i32 - 0x100_0000 } else { u as i32 } as f64;
    let half_sgn = if i > 0.0 { 0.5 } else if i < 0.0 { -0.5 } else { 0.0 };
    (i - half_sgn) * 360.0 / 16_777_216.0
}

/// Decode a big-endian signed 16-bit relative offset to degrees.
pub fn decode_rel_coord(hi: u8, lo: u8, prev: f64) -> f64 {
    let i = ((hi as u16) << 8 | lo as u16) as i16;
    prev + i as f64 / 100_000.0
}

/// Attr1 byte: bits[5:3] = FRC, bits[2:0] = FOW.
/// Note: for PAL, bits[7:6] carry Orientation or SideOfRoad — those are extracted
/// by the caller before this function.
fn decode_attr1(b: u8) -> (u8, u8) {
    ((b >> 3) & 0x07, b & 0x07)
}

/// Attr2 byte (non-last LRP): bits[7:5] = LFRCNP, bits[4:0] = bearing sector.
fn decode_attr2(b: u8) -> (u8, CircularInterval) {
    let lfrcnp = (b >> 5) & 0x07;
    let sector = b & 0x1F;
    (lfrcnp, bearing_sector_to_interval(sector))
}

/// Attr4 byte (last LRP of line location): bit6 = pos-offset flag, bit5 = neg-offset flag,
/// bits[4:0] = bearing sector.
fn decode_attr4(b: u8) -> (bool, bool, CircularInterval) {
    let has_pos = b & 0x40 != 0;
    let has_neg = b & 0x20 != 0;
    let sector  = b & 0x1F;
    (has_pos, has_neg, bearing_sector_to_interval(sector))
}

/// Convert a 5-bit sector (0–31) to a [LB, UB] CircularInterval.
pub fn bearing_sector_to_interval(sector: u8) -> CircularInterval {
    CircularInterval {
        lb_deg: sector as f64 * BEARING_SECTOR_DEG,
        ub_deg: (sector as f64 + 1.0) * BEARING_SECTOR_DEG,
    }
}

/// Convert the 1-byte DNP raw value to a LinearInterval (meters).
pub fn decode_dnp(raw: u8) -> LinearInterval {
    LinearInterval {
        lb: raw as f64 * DNP_BUCKET_M,
        ub: (raw as f64 + 1.0) * DNP_BUCKET_M,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // rustlr test_decode1: "C/+zGCZJgyuvBAAh/x8rHw=="
    // 2 LRPs, no offsets; 16 bytes.
    #[test]
    fn v3_two_lrp_no_offsets() {
        let loc = decode_v3_base64("C/+zGCZJgyuvBAAh/x8rHw==").unwrap();
        let lrps = loc.lrps().unwrap();
        assert_eq!(lrps.len(), 2);
        assert!(lrps[0].pos_offset.is_none());
        assert!(lrps[1].neg_offset.is_none());
        assert!(lrps[0].dnp.is_some());
        assert!(lrps[1].dnp.is_none());
        assert!(lrps[0].lfrcnp.is_some());
        assert!(lrps[1].lfrcnp.is_none());
        assert!(matches!(loc, LocationReference::Line { .. }));
    }

    // rustlr test_decode4: "C/7VOCaEbSu/BP+5AMUrbJEQ"
    // 2 LRPs, both offsets; 18 bytes.
    #[test]
    fn v3_two_lrp_both_offsets() {
        let loc = decode_v3_base64("C/7VOCaEbSu/BP+5AMUrbJEQ").unwrap();
        let lrps = loc.lrps().unwrap();
        assert_eq!(lrps.len(), 2);
        assert!(lrps[0].pos_offset_raw.is_some());
        assert!(lrps[1].neg_offset_raw.is_some());
    }

    // "C/4bnSaa4yu5Af91ACAruQT+r/+9Kwc=" — 3 LRPs, no offsets; 23 bytes.
    #[test]
    fn v3_three_lrp_no_offsets() {
        let loc = decode_v3_base64("C/4bnSaa4yu5Af91ACAruQT+r/+9Kwc=").unwrap();
        let lrps = loc.lrps().unwrap();
        assert_eq!(lrps.len(), 3);
        assert!(lrps[1].dnp.is_some());   // intermediate has DNP
        assert!(lrps[2].dnp.is_none());   // last does not
    }

    #[test]
    fn abs_coord_round_trip() {
        // lon ≈ 13.41 (Berlin)
        let lon = 13.41_f64;
        let u = ((lon * 16_777_216.0 / 360.0 + 0.5).round() as i32).clamp(-8_388_608, 8_388_607);
        let encoded = u.to_be_bytes();
        let decoded = decode_abs_coord(encoded[1], encoded[2], encoded[3]);
        // Theoretical max error = ½ LSB = 360/2^24/2 ≈ 1.07e-5°.
        assert!((decoded - lon).abs() < 2e-5, "delta={}", decoded - lon);
    }

    #[test]
    fn bearing_sector_0() {
        let i = bearing_sector_to_interval(0);
        assert_eq!(i.lb_deg, 0.0);
        assert!((i.ub_deg - 11.25).abs() < 1e-10);
    }

    #[test]
    fn bearing_sector_15() {
        let i = bearing_sector_to_interval(15);
        assert!((i.lb_deg - 168.75).abs() < 1e-10);
        assert!((i.ub_deg - 180.0).abs() < 1e-10);
    }

    #[test]
    fn dnp_bucket_bounds() {
        let d = decode_dnp(0);
        assert_eq!(d.lb, 0.0);
        assert!((d.ub - 15_000.0 / 256.0).abs() < 1e-9);
        let d255 = decode_dnp(255);
        assert!((d255.ub - 15_000.0).abs() < 1e-6);
    }

    // Pinned coordinate values computed from raw bytes, cross-checked against
    // the whitepaper §8 formula: deg = (i − sgn(i)·0.5) × 360 / 2^24.
    #[test]
    fn v3_two_lrp_coord_values() {
        let loc = decode_v3_base64("C/+zGCZJgyuvBAAh/x8rHw==").unwrap();
        let lrps = loc.lrps().unwrap();
        let lrp0 = &lrps[0];
        let lrp1 = &lrps[1];

        assert!((lrp0.coord.0 - -0.422_448).abs() < 1e-5, "lon0={}", lrp0.coord.0);
        assert!((lrp0.coord.1 -  53.841_301).abs() < 1e-5, "lat0={}", lrp0.coord.1);
        assert_eq!(lrp0.frc, 5);
        assert_eq!(lrp0.fow, 3);
        assert_eq!(lrp0.lfrcnp, Some(5));
        let b0 = lrp0.bearing.clone();
        assert!((b0.lb_deg - 168.75).abs() < 1e-9, "bearing lb={}", b0.lb_deg);
        assert!((b0.ub_deg - 180.0 ).abs() < 1e-9, "bearing ub={}", b0.ub_deg);
        let d0 = lrp0.dnp.as_ref().unwrap();
        assert!((d0.lb - 234.375  ).abs() < 1e-6, "dnp lb={}", d0.lb);
        assert!((d0.ub - 292.968_75).abs() < 1e-6, "dnp ub={}", d0.ub);

        assert!((lrp1.coord.0 - -0.422_118).abs() < 1e-5, "lon1={}", lrp1.coord.0);
        assert!((lrp1.coord.1 -  53.839_051).abs() < 1e-5, "lat1={}", lrp1.coord.1);
        assert_eq!(lrp1.frc, 5);
        assert_eq!(lrp1.fow, 3);
        let b1 = lrp1.bearing.clone();
        assert!((b1.lb_deg - 348.75).abs() < 1e-9, "bearing lb={}", b1.lb_deg);
        assert!((b1.ub_deg - 360.0 ).abs() < 1e-9, "bearing ub={}", b1.ub_deg);
        assert!(lrp1.dnp.is_none());
        assert!(lrp1.pos_offset.is_none());
        assert!(lrp1.neg_offset.is_none());
    }

    #[test]
    fn too_short_rejected() {
        assert!(matches!(decode_v3(&[0x0B; 15]), Err(DecodeError::TooShort { .. })));
    }

    #[test]
    fn bad_version_rejected() {
        // status byte with version != 3
        let mut b = vec![0x0B_u8; 16];
        b[0] = 0x0C; // version = 4
        assert!(matches!(decode_v3(&b), Err(DecodeError::InvalidHeader(_))));
    }

    /// Verify PAL detection from header byte.
    /// PAL header byte: version=3 (0x03), attr_flag=1 (0x08), point_flag=1 (0x20) → 0x2B.
    #[test]
    fn pal_header_detection() {
        // Build a minimal 16-byte PAL blob with header 0x2B.
        let mut b = [0u8; 16];
        b[0] = 0x2B; // version=3 | attr_flag | point_flag
        let loc = decode_v3(&b).unwrap();
        assert!(matches!(loc, LocationReference::PointAlongLine { .. }));
        assert!(loc.is_point_on_line());
        assert_eq!(loc.lrps().unwrap().len(), 2);
    }

    /// PAL with offset byte (17 bytes): verify pos_offset is parsed and orientation/side_of_road extracted.
    #[test]
    fn pal_with_offset() {
        let mut b = [0u8; 17];
        b[0] = 0x2B;
        // First LRP attr1: orientation=1 (FirstTowardSecond), FRC=2, FOW=3
        // bits[7:6]=01  bits[5:3]=010  bits[2:0]=011 → 0b0101_0011 = 0x53
        b[7] = 0x53;
        // First LRP attr2: LFRCNP=2, bearing=5 → bits[7:5]=010 bits[4:0]=00101 → 0b0100_0101 = 0x45
        b[8] = 0x45;
        // DNP = 4 → 4 * 58.6 ≈ 234.4 m
        b[9] = 4;
        // Last LRP attr1: side_of_road=1 (Right), FRC=2, FOW=3
        // bits[7:6]=01  bits[5:3]=010  bits[2:0]=011 → 0x53
        b[14] = 0x53;
        // Last LRP bearing sector = 15
        b[15] = 15;
        // Positive offset raw = 128 → 128/256 * 4*58.6 ≈ 117.2 m
        b[16] = 128;

        let loc = decode_v3(&b).unwrap();
        let (lrps, orientation, side_of_road) = match &loc {
            LocationReference::PointAlongLine { lrps, orientation, side_of_road } => {
                (lrps, *orientation, *side_of_road)
            }
            _ => panic!("expected PointAlongLine"),
        };
        assert_eq!(orientation, crate::lrp::Orientation::FirstTowardSecond);
        assert_eq!(side_of_road, crate::lrp::SideOfRoad::Right);
        assert_eq!(lrps[0].frc, 2);
        assert_eq!(lrps[0].fow, 3);
        // Raw byte stored in codec; metric interval computed in engine once path length is known.
        assert_eq!(lrps[0].pos_offset_raw, Some(128));
        assert!(lrps[0].pos_offset.is_none());
    }

    /// GeoCoordinate: 7-byte reference, area_flag=0, point=1, attrs=0 → status=0x23.
    #[test]
    fn geocoord_roundtrip() {
        // Build a 7-byte GeoCoordinate blob: status(0x23) + lon_abs(3) + lat_abs(3)
        // Encode lon=13.41, lat=52.52 (Berlin) roughly.
        let mut b = [0u8; 7];
        b[0] = 0x23;
        // lon = 13.41 → raw i = round(13.41 * 2^24 / 360 + 0.5) ≈ 624461
        let lon_raw: i32 = ((13.41_f64 * 16_777_216.0 / 360.0 + 0.5).round() as i32).clamp(-8_388_608, 8_388_607);
        let lon_be = lon_raw.to_be_bytes();
        b[1] = lon_be[1]; b[2] = lon_be[2]; b[3] = lon_be[3];
        let lat_raw: i32 = ((52.52_f64 * 16_777_216.0 / 360.0 + 0.5).round() as i32).clamp(-8_388_608, 8_388_607);
        let lat_be = lat_raw.to_be_bytes();
        b[4] = lat_be[1]; b[5] = lat_be[2]; b[6] = lat_be[3];

        let loc = decode_v3(&b).unwrap();
        match loc {
            LocationReference::GeoCoordinate { coord: (lon, lat) } => {
                assert!((lon - 13.41).abs() < 1e-4, "lon={lon}");
                assert!((lat - 52.52).abs() < 1e-4, "lat={lat}");
            }
            _ => panic!("expected GeoCoordinate"),
        }
    }
}
