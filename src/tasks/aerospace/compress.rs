use crate::core::limits::read_bounded;
use anyhow::Result;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::io::Write;
pub fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}
pub fn decompress(bytes: &[u8], max: u64) -> Result<Vec<u8>> {
    read_bounded(GzDecoder::new(bytes), max)
}
