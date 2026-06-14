use crate::lrp::LocationReference;

pub mod v3;
pub mod tpeg;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("input too short: need at least {min} bytes, got {got}")]
    TooShort { min: usize, got: usize },
    #[error("invalid magic / version byte: {0:#04x}")]
    InvalidHeader(u8),
    #[error("trailing bytes after valid payload ({0} bytes)")]
    TrailingBytes(usize),
}

pub trait Decoder {
    fn decode(&self, bytes: &[u8]) -> Result<LocationReference, DecodeError>;
}
