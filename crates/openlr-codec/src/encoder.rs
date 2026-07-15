use crate::lrp::LocationReference;

pub mod v3;
pub mod tpeg;

/// Physical-format encoding is the exact dual of `decoder::{v3,tpeg}`: same bit
/// layout, write instead of read. `Lrp`s passed in are expected to carry *exact*
/// point values (`lb == ub`) for `bearing`/`dnp`/`pos_offset`/`neg_offset` — the
/// caller (the graph-algorithm side that builds `LocationReference` from a real
/// path) knows these precisely; only the physical formats' own bucket/sector
/// quantization is lossy, applied here, not upstream.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("line/closed-line location needs at least {min} LRPs, got {got}")]
    TooFewLrps { min: usize, got: usize },
    #[error("point-on-line location must have exactly {expected} LRPs, got {got}")]
    WrongLrpCount { expected: usize, got: usize },
    #[error("missing required field '{0}' on an Lrp being encoded")]
    MissingField(&'static str),
    #[error("offset {offset_m}m is not less than its bracketing leg length {leg_m}m (Rule-5)")]
    OffsetExceedsLeg { offset_m: f64, leg_m: f64 },
    #[error("location type {0} is not yet supported by the encoder")]
    UnsupportedLocationType(&'static str),
    #[error("message needs {needed} bytes for a length field the physical format only reserves 1 byte for (max 127)")]
    MessageTooLarge { needed: usize },
}

pub trait Encoder {
    fn encode(&self, reference: &LocationReference) -> Result<Vec<u8>, EncodeError>;
}
