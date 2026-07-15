use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

use crate::lrp::{LocationReference, Lrp, Orientation, SideOfRoad};
use super::EncodeError;

// ── Constants (mirror decoder::v3 exactly) ────────────────────────────────────

const BEARING_SECTOR_DEG: f64 = 360.0 / 32.0; // 11.25°
const DNP_BUCKET_M: f64       = 15_000.0 / 256.0; // ≈58.59375 m

// ── Public entry points ────────────────────────────────────────────────────────

pub fn encode_v3(loc: &LocationReference) -> Result<Vec<u8>, EncodeError> {
    match loc {
        LocationReference::Line { lrps } => encode_line(lrps),
        LocationReference::PointAlongLine { lrps, orientation, side_of_road } =>
            encode_pal(lrps, *orientation, *side_of_road),
        _ => Err(EncodeError::UnsupportedLocationType(loc.type_str())),
    }
}

pub fn encode_v3_base64(loc: &LocationReference) -> Result<String, EncodeError> {
    Ok(B64.encode(encode_v3(loc)?))
}

// ── Line location ──────────────────────────────────────────────────────────────

fn encode_line(lrps: &[Lrp]) -> Result<Vec<u8>, EncodeError> {
    if lrps.len() < 2 {
        return Err(EncodeError::TooFewLrps { min: 2, got: lrps.len() });
    }

    let mut out = Vec::new();
    out.push(0x0Bu8); // status: version=3, attr_flag=1, point=0, area=0

    let first = &lrps[0];
    out.extend(encode_abs_coord(first.coord.0));
    out.extend(encode_abs_coord(first.coord.1));
    out.push(encode_attr1(first.frc, first.fow));
    let lfrcnp0 = first.lfrcnp.ok_or(EncodeError::MissingField("lfrcnp"))?;
    let dnp0 = first.dnp.ok_or(EncodeError::MissingField("dnp"))?;
    out.push(encode_attr2(lfrcnp0, first.bearing.lb_deg));
    out.push(encode_dnp(dnp0.lb));

    let mut prev = first.coord;
    for lrp in &lrps[1..lrps.len() - 1] {
        out.extend(encode_rel_coord(lrp.coord.0, prev.0));
        out.extend(encode_rel_coord(lrp.coord.1, prev.1));
        out.push(encode_attr1(lrp.frc, lrp.fow));
        let lfrcnp = lrp.lfrcnp.ok_or(EncodeError::MissingField("lfrcnp"))?;
        let dnp = lrp.dnp.ok_or(EncodeError::MissingField("dnp"))?;
        out.push(encode_attr2(lfrcnp, lrp.bearing.lb_deg));
        out.push(encode_dnp(dnp.lb));
        prev = lrp.coord;
    }

    let last = &lrps[lrps.len() - 1];
    out.extend(encode_rel_coord(last.coord.0, prev.0));
    out.extend(encode_rel_coord(last.coord.1, prev.1));
    out.push(encode_attr1(last.frc, last.fow));

    let has_pos = first.pos_offset.is_some();
    let has_neg = last.neg_offset.is_some();
    out.push(encode_attr4(has_pos, has_neg, last.bearing.lb_deg));

    if has_pos {
        let poff_m = first.pos_offset.unwrap().lb;
        out.push(encode_offset_raw(poff_m, dnp0.lb)?);
    }
    if has_neg {
        let last_leg_m = lrps[lrps.len() - 2].dnp.ok_or(EncodeError::MissingField("dnp"))?.lb;
        let noff_m = last.neg_offset.unwrap().lb;
        out.push(encode_offset_raw(noff_m, last_leg_m)?);
    }

    Ok(out)
}

// ── PointAlongLine (PAL) location ──────────────────────────────────────────────

fn encode_pal(lrps: &[Lrp], orientation: Orientation, side_of_road: SideOfRoad) -> Result<Vec<u8>, EncodeError> {
    if lrps.len() != 2 {
        return Err(EncodeError::WrongLrpCount { expected: 2, got: lrps.len() });
    }
    let first = &lrps[0];
    let last = &lrps[1];

    let mut out = Vec::new();
    out.push(0x2Bu8); // status: version=3, attr_flag=1, point=1, area=0

    out.extend(encode_abs_coord(first.coord.0));
    out.extend(encode_abs_coord(first.coord.1));

    out.push(((orientation as u8) << 6) | encode_attr1(first.frc, first.fow));
    let lfrcnp0 = first.lfrcnp.ok_or(EncodeError::MissingField("lfrcnp"))?;
    let dnp0 = first.dnp.ok_or(EncodeError::MissingField("dnp"))?;
    out.push(encode_attr2(lfrcnp0, first.bearing.lb_deg));
    out.push(encode_dnp(dnp0.lb));

    out.extend(encode_rel_coord(last.coord.0, first.coord.0));
    out.extend(encode_rel_coord(last.coord.1, first.coord.1));

    out.push(((side_of_road as u8) << 6) | encode_attr1(last.frc, last.fow));
    out.push(bearing_sector(last.bearing.lb_deg) & 0x1F);

    if let Some(poff) = first.pos_offset {
        out.push(encode_offset_raw(poff.lb, dnp0.lb)?);
    }

    Ok(out)
}

// ── Byte-level helpers (exact duals of decoder::v3's) ─────────────────────────

/// Encode WGS84 degrees to a big-endian signed 24-bit integer.
/// Whitepaper §7.2.1 Equation 1: int = sgn(deg)·0.5 + deg·2^Resolution/360°.
pub fn encode_abs_coord(deg: f64) -> [u8; 3] {
    const K: f64 = 16_777_216.0 / 360.0; // 2^24/360
    let sgn = if deg > 0.0 { 0.5 } else if deg < 0.0 { -0.5 } else { 0.0 };
    let i = (sgn + deg * K).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
    let u = (i as u32) & 0x00FF_FFFF;
    [(u >> 16) as u8, (u >> 8) as u8, u as u8]
}

/// Encode a coordinate delta to a big-endian signed 16-bit relative offset
/// (decamicrodegrees). Whitepaper §7.2.2 Equation 3, inverted.
pub fn encode_rel_coord(current: f64, prev: f64) -> [u8; 2] {
    let i = ((current - prev) * 100_000.0).round().clamp(-32_768.0, 32_767.0) as i16;
    i.to_be_bytes()
}

fn encode_attr1(frc: u8, fow: u8) -> u8 {
    ((frc & 0x07) << 3) | (fow & 0x07)
}

fn encode_attr2(lfrcnp: u8, bearing_deg: f64) -> u8 {
    ((lfrcnp & 0x07) << 5) | (bearing_sector(bearing_deg) & 0x1F)
}

fn encode_attr4(has_pos_off: bool, has_neg_off: bool, bearing_deg: f64) -> u8 {
    ((has_pos_off as u8) << 6) | ((has_neg_off as u8) << 5) | (bearing_sector(bearing_deg) & 0x1F)
}

/// Which of the 32 11.25° sectors `bearing_deg` (expected in [0,360)) falls into.
fn bearing_sector(bearing_deg: f64) -> u8 {
    ((bearing_deg / BEARING_SECTOR_DEG).floor() as i64).clamp(0, 31) as u8
}

/// Which of the 256 fixed 58.59375m buckets `dnp_m` falls into.
fn encode_dnp(dnp_m: f64) -> u8 {
    ((dnp_m / DNP_BUCKET_M).floor() as i64).clamp(0, 255) as u8
}

/// Which of the 256 buckets of `leg_m` (the *actual* bracketing leg length)
/// `offset_m` falls into. Errors if `offset_m` isn't strictly less than `leg_m`
/// (Rule-5) — the encoder pipeline upstream is responsible for that invariant;
/// this function only catches it, it doesn't silently clamp it away.
fn encode_offset_raw(offset_m: f64, leg_m: f64) -> Result<u8, EncodeError> {
    if leg_m <= 0.0 || offset_m >= leg_m || offset_m < 0.0 {
        return Err(EncodeError::OffsetExceedsLeg { offset_m, leg_m });
    }
    Ok(((offset_m / leg_m * 256.0).floor() as i64).clamp(0, 255) as u8)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::v3::decode_v3_base64;
    use crate::interval::LinearInterval;

    /// Round-trip: decode a known-good fixture, re-encode it, and expect the
    /// exact same bytes back. Uses the same test vector as decoder::v3's own
    /// `v3_two_lrp_no_offsets`.
    #[test]
    fn round_trip_two_lrp_no_offsets() {
        let b64 = "C/+zGCZJgyuvBAAh/x8rHw==";
        let loc = decode_v3_base64(b64).unwrap();
        let re_encoded = encode_v3_base64(&loc).unwrap();
        assert_eq!(re_encoded, b64);
    }

    #[test]
    fn round_trip_three_lrp_no_offsets() {
        let b64 = "C/4bnSaa4yu5Af91ACAruQT+r/+9Kwc=";
        let loc = decode_v3_base64(b64).unwrap();
        let re_encoded = encode_v3_base64(&loc).unwrap();
        assert_eq!(re_encoded, b64);
    }

    #[test]
    fn round_trip_pal_header_detection() {
        let mut b = [0u8; 16];
        b[0] = 0x2B;
        let loc = decode_v3(&b).unwrap();
        let re_encoded = encode_v3(&loc).unwrap();
        assert_eq!(re_encoded, b.to_vec());
    }

    fn decode_v3(bytes: &[u8]) -> Result<LocationReference, crate::decoder::DecodeError> {
        crate::decoder::v3::decode_v3(bytes)
    }

    #[test]
    fn abs_coord_round_trips_within_lsb() {
        for lon in [13.41, -122.4, 0.0, -0.4224485, 179.999, -179.999] {
            let bytes = encode_abs_coord(lon);
            let back = crate::decoder::v3::decode_abs_coord(bytes[0], bytes[1], bytes[2]);
            assert!((back - lon).abs() < 2e-5, "lon={lon} back={back}");
        }
    }

    #[test]
    fn rel_coord_round_trips() {
        let prev = 13.41;
        let current = 13.4105;
        let bytes = encode_rel_coord(current, prev);
        let back = crate::decoder::v3::decode_rel_coord(bytes[0], bytes[1], prev);
        assert!((back - current).abs() < 1e-9);
    }

    #[test]
    fn dnp_bucket_round_trips() {
        // Bucket 4: [234.375, 292.96875)
        assert_eq!(encode_dnp(234.375), 4);
        assert_eq!(encode_dnp(280.0), 4);
        assert_eq!(encode_dnp(0.0), 0);
        assert_eq!(encode_dnp(14_999.0), 255);
    }

    #[test]
    fn bearing_sector_round_trips() {
        assert_eq!(bearing_sector(0.0), 0);
        assert_eq!(bearing_sector(168.75), 15);
        assert_eq!(bearing_sector(359.9), 31);
    }

    #[test]
    fn offset_raw_matches_manual_pal_fixture() {
        // Mirrors decoder::v3's pal_with_offset test: DNP raw=4 (leg=234.375m),
        // offset raw=128 -> offset_m = 128/256*234.375 = 117.1875.
        let leg_m = 4.0 * DNP_BUCKET_M;
        let raw = encode_offset_raw(117.1875, leg_m).unwrap();
        assert_eq!(raw, 128);
    }

    #[test]
    fn offset_at_or_past_leg_length_errors() {
        assert!(encode_offset_raw(100.0, 100.0).is_err());
        assert!(encode_offset_raw(150.0, 100.0).is_err());
    }

    /// 3 LRPs so the first leg (LRP0->LRP1, carries POFF) and last leg
    /// (LRP1->LRP2, carries NOFF) are genuinely independent lengths.
    #[test]
    fn line_with_both_offsets_round_trips_bytes() {
        use crate::CircularInterval;
        let first_leg_m = 300.0;
        let last_leg_m = 400.0;
        let lrps = vec![
            Lrp {
                coord: (10.0, 50.0),
                bearing: CircularInterval::point(90.0),
                frc: 3, fow: 2,
                lfrcnp: Some(3),
                dnp: Some(LinearInterval::point(first_leg_m)),
                pos_offset: Some(LinearInterval::point(50.0)),
                neg_offset: None,
                pos_offset_raw: None, neg_offset_raw: None,
            },
            Lrp {
                coord: (10.005, 50.005),
                bearing: CircularInterval::point(95.0),
                frc: 3, fow: 2,
                lfrcnp: Some(3),
                dnp: Some(LinearInterval::point(last_leg_m)),
                pos_offset: None,
                neg_offset: None,
                pos_offset_raw: None, neg_offset_raw: None,
            },
            Lrp {
                coord: (10.01, 50.01),
                bearing: CircularInterval::point(270.0),
                frc: 3, fow: 2,
                lfrcnp: None,
                dnp: None,
                pos_offset: None,
                neg_offset: Some(LinearInterval::point(75.0)),
                pos_offset_raw: None, neg_offset_raw: None,
            },
        ];
        let loc = LocationReference::Line { lrps };
        let bytes = encode_v3(&loc).unwrap();
        let expected_pos = ((50.0_f64 / first_leg_m * 256.0).floor()) as u8;
        let expected_neg = ((75.0_f64 / last_leg_m * 256.0).floor()) as u8;
        assert_eq!(*bytes.last().unwrap(), expected_neg);
        assert_eq!(bytes[bytes.len() - 2], expected_pos);

        // And it should decode back into a Line with 3 LRPs.
        let redecoded = crate::decoder::v3::decode_v3(&bytes).unwrap();
        assert_eq!(redecoded.lrps().unwrap().len(), 3);
    }
}
