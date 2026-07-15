use crate::lrp::{LocationReference, Lrp, Orientation, SideOfRoad};
use super::EncodeError;

const TPEG_BEARING_FACTOR: f64 = 360.0 / 256.0; // 256 sectors ≈ 1.40625° each

// ── Public entry points ────────────────────────────────────────────────────────

pub fn encode_tpeg(loc: &LocationReference) -> Result<Vec<u8>, EncodeError> {
    match loc {
        LocationReference::Line { lrps } => encode_line(lrps),
        LocationReference::PointAlongLine { lrps, orientation, side_of_road } =>
            encode_pal(lrps, *orientation, *side_of_road),
        _ => Err(EncodeError::UnsupportedLocationType(loc.type_str())),
    }
}

pub fn encode_tpeg_hex(loc: &LocationReference) -> Result<String, EncodeError> {
    Ok(to_hex(&encode_tpeg(loc)?))
}

pub fn encode_tpeg_base64(loc: &LocationReference) -> Result<String, EncodeError> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    Ok(B64.encode(encode_tpeg(loc)?))
}

// ── Line location ──────────────────────────────────────────────────────────────

fn encode_line(lrps: &[Lrp]) -> Result<Vec<u8>, EncodeError> {
    if lrps.len() < 2 {
        return Err(EncodeError::TooFewLrps { min: 2, got: lrps.len() });
    }
    let first = &lrps[0];
    let last = &lrps[lrps.len() - 1];
    let intermediates = &lrps[1..lrps.len() - 1];

    let mut attrs = Vec::new();

    // First LRP: absolute coords + altitude selector + LP + PP.
    attrs.extend(encode_abs24(first.coord.0));
    attrs.extend(encode_abs24(first.coord.1));
    attrs.push(0x00); // no altitude
    push_lp(&mut attrs, first.frc, first.fow, first.bearing.lb_deg);
    let lfrcnp0 = first.lfrcnp.ok_or(EncodeError::MissingField("lfrcnp"))?;
    let dnp0 = first.dnp.ok_or(EncodeError::MissingField("dnp"))?;
    push_pp(&mut attrs, lfrcnp0, dnp0.lb);

    // Last LRP: relative coords (relative to the *previous* LRP, i.e. the final
    // intermediate if any, else the first LRP) + altitude selector + LP.
    let prev_for_last = intermediates.last().map(|l| l.coord).unwrap_or(first.coord);
    attrs.extend(encode_rel16(last.coord.0, prev_for_last.0));
    attrs.extend(encode_rel16(last.coord.1, prev_for_last.1));
    attrs.push(0x00);
    push_lp(&mut attrs, last.frc, last.fow, last.bearing.lb_deg);

    let has_pos_off = first.pos_offset.is_some();
    let has_neg_off = last.neg_offset.is_some();
    let has_intermediates = !intermediates.is_empty();
    attrs.push(((has_intermediates as u8) << 6) | ((has_pos_off as u8) << 5) | ((has_neg_off as u8) << 4));

    if has_intermediates {
        attrs.extend(encode_mb(intermediates.len() as u64));
        let mut prev = first.coord;
        for lrp in intermediates {
            attrs.extend(encode_rel16(lrp.coord.0, prev.0));
            attrs.extend(encode_rel16(lrp.coord.1, prev.1));
            attrs.push(0x00);
            push_lp(&mut attrs, lrp.frc, lrp.fow, lrp.bearing.lb_deg);
            let lfrcnp = lrp.lfrcnp.ok_or(EncodeError::MissingField("lfrcnp"))?;
            let dnp = lrp.dnp.ok_or(EncodeError::MissingField("dnp"))?;
            push_pp(&mut attrs, lfrcnp, dnp.lb);
            prev = lrp.coord;
        }
    }

    if has_pos_off {
        attrs.extend(encode_mb(first.pos_offset.unwrap().lb.round() as u64));
    }
    if has_neg_off {
        attrs.extend(encode_mb(last.neg_offset.unwrap().lb.round() as u64));
    }

    wrap_message(0x00, attrs)
}

// ── PointAlongLine (PAL) location ──────────────────────────────────────────────

fn encode_pal(lrps: &[Lrp], orientation: Orientation, side_of_road: SideOfRoad) -> Result<Vec<u8>, EncodeError> {
    if lrps.len() != 2 {
        return Err(EncodeError::WrongLrpCount { expected: 2, got: lrps.len() });
    }
    let first = &lrps[0];
    let last = &lrps[1];

    let mut attrs = Vec::new();
    attrs.extend(encode_abs24(first.coord.0));
    attrs.extend(encode_abs24(first.coord.1));
    attrs.push(0x00);
    push_lp(&mut attrs, first.frc, first.fow, first.bearing.lb_deg);
    let lfrcnp0 = first.lfrcnp.ok_or(EncodeError::MissingField("lfrcnp"))?;
    let dnp0 = first.dnp.ok_or(EncodeError::MissingField("dnp"))?;
    push_pp(&mut attrs, lfrcnp0, dnp0.lb);

    attrs.extend(encode_rel16(last.coord.0, first.coord.0));
    attrs.extend(encode_rel16(last.coord.1, first.coord.1));
    attrs.push(0x00);
    push_lp(&mut attrs, last.frc, last.fow, last.bearing.lb_deg);

    attrs.push(side_of_road as u8);
    attrs.push(orientation as u8);

    let has_pos_off = first.pos_offset.is_some();
    attrs.push((has_pos_off as u8) << 6);
    if has_pos_off {
        attrs.extend(encode_mb(first.pos_offset.unwrap().lb.round() as u64));
    }

    wrap_message(0x02, attrs)
}

// ── Message wrapper / component writers ────────────────────────────────────────

/// Wraps `attrs` (everything from byte 7 onward) in the fixed 7-byte header.
/// See decoder::tpeg's layout comment — this is its exact dual.
fn wrap_message(location_type: u8, attrs: Vec<u8>) -> Result<Vec<u8>, EncodeError> {
    // bytes[6] ("inner_attr_len") = attrs.len(); bytes[5] = attrs.len()+1;
    // bytes[1] = 5+attrs.len(). All three must fit the single-byte-length
    // assumption our decoder (and this encoder) make — see decoder::tpeg's
    // "(all practical messages have lengths < 128, so MB fields are 1 byte)".
    if attrs.len() + 1 >= 128 {
        return Err(EncodeError::MessageTooLarge { needed: attrs.len() + 1 });
    }
    let mut out = Vec::with_capacity(7 + attrs.len());
    out.push(0x08);
    out.push((5 + attrs.len()) as u8);
    out.push(0x01);
    out.push(0x10);
    out.push(location_type);
    out.push((attrs.len() + 1) as u8);
    out.push(attrs.len() as u8);
    out.extend(attrs);
    Ok(out)
}

/// LineProperties component (id 0x09): fixed layout observed in reference
/// vectors — [id][len=5][lp_attr=0x00][frc][fow][bearing_sector][selector=0x00].
/// `lp_attr`/`selector` are unread by our decoder and not otherwise documented
/// here (no side-of-road/orientation sub-fields are ever emitted for the main
/// road properties), so they're fixed at the values a reference encoder emits
/// for the "nothing optional present" case.
fn push_lp(out: &mut Vec<u8>, frc: u8, fow: u8, bearing_deg: f64) {
    out.push(0x09);
    out.push(0x05);
    out.push(0x04);
    out.push(frc);
    out.push(fow);
    out.push(bearing_sector(bearing_deg));
    out.push(0x00);
}

/// PathProperties component (id 0x0A): [id][len][pp_attr=0x04][lfrcnp][dnp
/// varint...][selector=0x00]. `len` varies with the DNP varint's width.
fn push_pp(out: &mut Vec<u8>, lfrcnp: u8, dnp_m: f64) {
    let dnp_varint = encode_mb(dnp_m.round().max(0.0) as u64);
    out.push(0x0A);
    out.push((3 + dnp_varint.len()) as u8);
    out.push(0x04);
    out.push(lfrcnp);
    out.extend(dnp_varint);
    out.push(0x00);
}

// ── Byte-level helpers (exact duals of decoder::tpeg's) ───────────────────────

fn encode_abs24(deg: f64) -> [u8; 3] {
    super::v3::encode_abs_coord(deg)
}

fn encode_rel16(current: f64, prev: f64) -> [u8; 2] {
    super::v3::encode_rel_coord(current, prev)
}

/// TPEG bearing: 256 sectors of 360/256°.
fn bearing_sector(bearing_deg: f64) -> u8 {
    ((bearing_deg / TPEG_BEARING_FACTOR).round() as i64).clamp(0, 255) as u8
}

/// IntUnLoMB: big-endian MSB-continuation varint (dual of decoder::tpeg::decode_mb).
/// 7 payload bits per byte, most-significant group first, continuation bit (0x80)
/// set on every byte except the last.
fn encode_mb(value: u64) -> Vec<u8> {
    let n_bits = if value == 0 { 1 } else { 64 - value.leading_zeros() };
    let n_groups = (n_bits + 6) / 7;
    let mut out = Vec::with_capacity(n_groups as usize);
    for i in (0..n_groups).rev() {
        let chunk = ((value >> (i * 7)) & 0x7F) as u8;
        out.push(if i == 0 { chunk } else { chunk | 0x80 });
    }
    out
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::tpeg::{decode_tpeg_hex, decode_tpeg_base64};

    #[test]
    fn mb_round_trips() {
        assert_eq!(encode_mb(127), vec![0x7F]);
        assert_eq!(encode_mb(1_093_567_633), vec![0x84, 0x89, 0xBA, 0x89, 0x11]);
        assert_eq!(encode_mb(0), vec![0x00]);
        assert_eq!(encode_mb(351), vec![0x82, 0x5F]);
        assert_eq!(encode_mb(15), vec![0x0F]);
    }

    #[test]
    fn round_trip_two_lrp_both_offsets() {
        let hex = "0829011000252404121724D5C800090504060321000A050406825F00FFF300030009050406030B00300F77";
        let loc = decode_tpeg_hex(hex).unwrap();
        let re_encoded = encode_tpeg_hex(&loc).unwrap();
        assert_eq!(re_encoded, hex);
    }

    #[test]
    fn round_trip_four_lrp_two_intermediates() {
        let hex = "08510110004D4C083CE62242730009050401023E000A0504018567000148F9A1000905040102FC007002038EFF1900090504010257000A05040198480006E7F7D400090504010258000A0504018F3100834655";
        let loc = decode_tpeg_hex(hex).unwrap();
        let re_encoded = encode_tpeg_hex(&loc).unwrap();
        assert_eq!(re_encoded, hex);
    }

    #[test]
    fn round_trip_pal() {
        let b64 = "CCsBEAInJgHY7yKlnwAJBQQBAnwACgUEAYYXAACs/TMACQUEAQL8AAAAQIMc";
        let loc = decode_tpeg_base64(b64).unwrap();
        let re_encoded = encode_tpeg_base64(&loc).unwrap();
        assert_eq!(re_encoded, b64);
    }
}
