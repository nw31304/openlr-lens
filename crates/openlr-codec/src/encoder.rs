use crate::lrp::LocationReference;

/// Stub trait for the encode path. Implementations are out of scope for v1.
pub trait Encoder {
    type Error;
    fn encode(&self, reference: &LocationReference) -> Result<Vec<u8>, Self::Error>;
}
