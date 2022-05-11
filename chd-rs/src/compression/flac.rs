use std::io::Cursor;
use std::marker::PhantomData;
use std::mem;

use byteorder::{BigEndian, ByteOrder, LittleEndian, WriteBytesExt};
use cfg_if::cfg_if;
use claxon::frame::FrameReader;

use crate::cdrom::{CD_FRAME_SIZE, CD_MAX_SECTOR_DATA, CD_MAX_SUBCODE_DATA};
use crate::compression::zlib::ZlibCodec;
use crate::compression::{CompressionCodec, CompressionCodecType, DecompressLength, InternalCodec};
use crate::error::{ChdError, Result};
use crate::header::CodecType;

/// Generic block decoder for FLAC.
///
/// Defaults assume 2 channel interleaved FLAC.
struct FlacCodec<T: ByteOrder, const CHANNELS: usize = 2> {
    buffer: Vec<i32>,
    _ordering: PhantomData<T>,
}

impl<T: ByteOrder, const CHANNELS: usize> InternalCodec for FlacCodec<T, CHANNELS> {
    fn is_lossy(&self) -> bool
    where
        Self: Sized,
    {
        return false;
    }

    fn new(hunk_bytes: u32) -> Result<Self>
    where
        Self: Sized,
    {
        if hunk_bytes % (CHANNELS * mem::size_of::<i16>()) as u32 != 0 {
            return Err(ChdError::CodecError);
        }

        Ok(FlacCodec {
            buffer: Vec::new(),
            _ordering: PhantomData::default(),
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        let comp_buf = Cursor::new(input);

        // Number of samples to write to the buffer.
        let sample_len = output.len() / (CHANNELS * mem::size_of::<i16>());

        // We don't need to create a fake header since claxon will read raw FLAC frames just fine.
        // We just need to be careful not to read past the number of blocks in the input buffer.
        let mut frame_read = FrameReader::new(comp_buf);

        let mut cursor = Cursor::new(output);

        // Buffer to hold decompressed FLAC block data.
        let mut block_buf = mem::take(&mut self.buffer);

        // A little bit of a misnomer. 1 'sample' refers to a sample for all channels.
        let mut samples_written = 0;

        while samples_written < sample_len {
            // Loop through all blocks until we have enough samples written.
            match frame_read.read_next_or_eof(block_buf) {
                Ok(Some(block)) => {
                    // We assume 2 channels (by default), so we can use claxon's stereo_samples
                    // iterator for slightly better performance.
                    #[cfg(not(feature = "nonstandard_channel_count"))]
                    for (l, r) in block.stereo_samples() {
                        cursor.write_i16::<T>(l as i16)?;
                        cursor.write_i16::<T>(r as i16)?;
                        samples_written += 1;
                    }

                    // This is generic over number of assumed channels, but is broken effectively
                    // for any value other than 2.
                    // What we really want here is specialization for CHANNELS = 2 ...
                    #[cfg(feature = "nonstandard_channel_count")]
                    for sample in 0..block.len() / block.channels() {
                        for channel in 0..block.channels() {
                            let sample_data = block.sample(channel, sample) as u16;
                            cursor.write_i16::<T>(sample_data as i16)?;
                        }
                        samples_written += 1;
                    }

                    block_buf = block.into_buffer();
                }
                _ => {
                    // If frame_read dies our buffer just gets eaten. The Error return for a failed
                    // read does not expose the inner buffer.
                    return Err(ChdError::DecompressionError);
                }
            }
        }

        self.buffer = block_buf;
        let bytes_in = frame_read.into_inner().position();
        Ok(DecompressLength::new(
            samples_written * 4,
            bytes_in as usize,
        ))
    }
}

/// Codec for Raw FLAC (flac) compressed hunks.
///
/// MAME Raw FLAC has the first byte as either 'L' or 'B' depending
/// on the endianness of the output data.
pub struct RawFlacCodec {
    be: FlacCodec<BigEndian>,
    le: FlacCodec<LittleEndian>,
}

impl CompressionCodec for RawFlacCodec {}

impl CompressionCodecType for RawFlacCodec {
    fn codec_type(&self) -> CodecType
    where
        Self: Sized,
    {
        CodecType::FlacV5
    }
}

impl InternalCodec for RawFlacCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(hunk_bytes: u32) -> Result<Self> {
        Ok(RawFlacCodec {
            be: FlacCodec::new(hunk_bytes)?,
            le: FlacCodec::new(hunk_bytes)?,
        })
    }

    /// Decompress CD FLAC data from MAME RAW FLAC data.
    ///
    /// The first byte indicates the endianness of the data to be written, and not
    ///
    /// FLAC data is assumed to be 2-channel interleaved 16-bit PCM. Thus the length of the output
    /// buffer must be a multiple of 4 to hold 2 bytes per sample, for 2 channels.
    ///
    /// The input buffer must also contain enough compressed samples to fill the length of the
    /// output buffer.
    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        match input[0] {
            b'L' => self.le.decompress(&input[1..], output),
            b'B' => self.be.decompress(&input[1..], output),
            _ => Err(ChdError::DecompressionError),
        }
    }
}

/// Codec for CD Flac
pub struct CdFlCodec {
    // cdfl always writes in big endian.
    engine: FlacCodec<BigEndian>,
    sub_engine: ZlibCodec,
    buffer: Vec<u8>,
}

impl CompressionCodec for CdFlCodec {}

impl CompressionCodecType for CdFlCodec {
    fn codec_type(&self) -> CodecType {
        CodecType::FlacCdV5
    }
}

impl InternalCodec for CdFlCodec {
    fn is_lossy(&self) -> bool {
        false
    }

    fn new(hunk_size: u32) -> Result<Self>
    where
        Self: Sized,
    {
        if hunk_size % CD_FRAME_SIZE != 0 {
            return Err(ChdError::CodecError);
        }

        // The size of the FLAC data in each cdfl hunk, excluding the subcode data.
        let max_frames = hunk_size / CD_FRAME_SIZE;
        let flac_data_size = max_frames * CD_MAX_SECTOR_DATA;

        // neither FlacCodec nor ZlibCodec actually make use of hunk_size.
        Ok(CdFlCodec {
            engine: FlacCodec::new(flac_data_size)?,
            sub_engine: ZlibCodec::new(hunk_size)?,
            buffer: vec![0u8; hunk_size as usize],
        })
    }

    fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<DecompressLength> {
        let total_frames = output.len() / CD_FRAME_SIZE as usize;
        let frame_res = self.engine.decompress(
            input,
            &mut self.buffer[..total_frames * CD_MAX_SECTOR_DATA as usize],
        )?;

        cfg_if! {
            if #[cfg(feature = "want_subcode")] {
                let sub_res = self.sub_engine.decompress(
                    &input[frame_res.total_in()..],
                    &mut self.buffer[total_frames * CD_MAX_SECTOR_DATA as usize..]
                        [..total_frames * CD_MAX_SUBCODE_DATA as usize],
                )?;
            } else {
              let sub_res =  DecompressLength::default();
            }
        };

        // Decompressed FLAC data has layout
        // [Frame0, Frame1, ..., FrameN, Subcode0, Subcode1, ..., SubcodeN]
        // We need to reassemble the data to be
        // [Frame0, Subcode0, Frame1, Subcode1, ..., FrameN, SubcodeN]

        // Reassemble frame data to expected layout.
        for (frame_num, chunk) in self.buffer[..total_frames * CD_MAX_SECTOR_DATA as usize]
            .chunks_exact(CD_MAX_SECTOR_DATA as usize)
            .enumerate()
        {
            output[frame_num * CD_FRAME_SIZE as usize..][..CD_MAX_SECTOR_DATA as usize]
                .copy_from_slice(chunk);
        }

        // Reassemble subcode data to expected layout.
        #[cfg(feature = "want_subcode")]
        for (frame_num, chunk) in self.buffer[total_frames * CD_MAX_SECTOR_DATA as usize..]
            .chunks_exact(CD_MAX_SUBCODE_DATA as usize)
            .enumerate()
        {
            output[frame_num * CD_FRAME_SIZE as usize + CD_MAX_SECTOR_DATA as usize..]
                [..CD_MAX_SUBCODE_DATA as usize]
                .copy_from_slice(chunk);
        }

        Ok(frame_res + sub_res)
    }
}
