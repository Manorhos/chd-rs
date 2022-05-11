use crate::compression::{CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::error::{ChdError, Result};
use crate::header::CodecType;
use lzma_rs_headerless::decode::lzma::LzmaParams;
use lzma_rs_headerless::lzma_decompress_with_params;
use std::io::Cursor;

/// LZMA codec with default CHD parameters
pub struct LzmaCodec {
    params: LzmaParams,
}

impl CompressionCodec for LzmaCodec {}

/// MAME/libchdr uses an ancient LZMA 19.00.
///
/// To match the proper dictionary size, we copy the algorithm from
/// [`LzmaEnc::LzmaEncProps_Normalize`](https://github.com/rtissera/libchdr/blob/cdcb714235b9ff7d207b703260706a364282b063/deps/lzma-19.00/src/LzmaEnc.c#L52).
fn get_lzma_dict_size(level: u32, reduce_size: u32) -> u32 {
    let mut dict_size = if level <= 5 {
        1 << (level * 2 + 14)
    } else if level <= 7 {
        1 << 25
    } else {
        1 << 26
    };

    // this does the same thing as LzmaEnc.c when determining dict_size
    if dict_size > reduce_size {
        // might be worth converting this to a while loop for const
        // depends if we can const-propagate hunk_size.
        for i in 11..=30 {
            if reduce_size <= (2u32 << i) {
                dict_size = 2u32 << i;
                break;
            }
            if reduce_size <= (3u32 << i) {
                dict_size = 3u32 << i;
                break;
            }
        }
    }

    dict_size
}

impl CompressionCodecType for LzmaCodec {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized,
    {
        CodecType::LzmaV5
    }
}

impl InternalCodec for LzmaCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(hunk_size: u32) -> Result<Self> {
        // The LZMA codec for CHD uses raw LZMA chunks without a stream header. The result
        // is that the chunks are encoded with the defaults used in LZMA 19.0.
        // These defaults are lc = 3, lp = 0, pb = 2.
        let params = LzmaParams::new(3, 0, 2, get_lzma_dict_size(9, hunk_size), None);

        Ok(LzmaCodec { params })
    }

    fn decompress(&mut self, input: &[u8], mut output: &mut [u8]) -> Result<DecompressLength> {
        let mut read = Cursor::new(input);
        let len = output.len();
        if let Ok(_) = lzma_decompress_with_params(
            &mut read,
            &mut output,
            None,
            self.params.with_size(len as u64),
        ) {
            Ok(DecompressLength::new(len, read.position() as usize))
        } else {
            Err(ChdError::DecompressionError)
        }
    }
}
