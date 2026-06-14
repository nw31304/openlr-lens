use crate::{CircularInterval, LinearInterval};
use crate::lrp::LocationReference;
use super::{DecodeError, Decoder};

pub struct V3Decoder;

/// OpenLR binary v3 constants.
const BEARING_SECTOR_DEG: f64 = 11.25; // 360 / 32
const DNP_BUCKET_M: f64 = 58.6;        // ~15 km / 255 buckets

impl Decoder for V3Decoder {
    fn decode(&self, _bytes: &[u8]) -> Result<LocationReference, DecodeError> {
        todo!("OpenLR binary v3 decoder")
    }
}

/// Convert a 5-bit bearing sector (0–31) to a [LB, UB] CircularInterval.
pub fn bearing_sector_to_interval(sector: u8) -> CircularInterval {
    assert!(sector < 32, "bearing sector must be 0–31");
    let lb = sector as f64 * BEARING_SECTOR_DEG;
    CircularInterval { lb_deg: lb, ub_deg: lb + BEARING_SECTOR_DEG }
}

/// Convert a 1-byte DNP value to a LinearInterval (meters).
pub fn dnp_byte_to_interval(raw: u8) -> LinearInterval {
    let centre = raw as f64 * DNP_BUCKET_M;
    LinearInterval { lb: centre, ub: centre + DNP_BUCKET_M }
}
