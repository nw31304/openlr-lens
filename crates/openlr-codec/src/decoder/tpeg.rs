use crate::lrp::LocationReference;
use super::{DecodeError, Decoder};

pub struct TpegDecoder;

impl Decoder for TpegDecoder {
    fn decode(&self, _bytes: &[u8]) -> Result<LocationReference, DecodeError> {
        todo!("TISA/TPEG-OLR decoder")
    }
}
