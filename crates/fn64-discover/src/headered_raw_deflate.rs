//! Bounded candidate-only materialization of headered raw-DEFLATE sequences.
//!
//! The caller supplies both the exact source slice and the cursor of its first
//! stream. This module never scans for a signature and never infers how many
//! streams a sequence contains. A successful decode proves only the bytes
//! described by the container; it does not prove that guest code loads,
//! executes, or otherwise consumes them.

use flate2::{Decompress, FlushDecompress, Status};
use sha2::{Digest, Sha256};

const HEADER_LEN: usize = 6;
const MAGIC: [u8; 2] = [0x11, 0x72];

/// Half-open byte range relative to the caller-supplied source or output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelativeByteRange {
    pub start: usize,
    pub end: usize,
}

impl RelativeByteRange {
    pub fn len(self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Resource envelope checked before allocating or decoding each stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderedRawDeflateLimits {
    pub max_input_bytes: usize,
    pub max_stream_output_bytes: usize,
    pub max_aggregate_output_bytes: usize,
    pub max_streams: usize,
}

impl Default for HeaderedRawDeflateLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_stream_output_bytes: 64 * 1024 * 1024,
            max_aggregate_output_bytes: 64 * 1024 * 1024,
            max_streams: 4096,
        }
    }
}

/// Exact materialization evidence for one requested stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderedRawDeflateStream {
    /// Header and compressed payload consumed by this stream.
    pub source_range: RelativeByteRange,
    /// Raw-DEFLATE payload only, excluding the six-byte header.
    pub deflate_range: RelativeByteRange,
    /// This stream's bytes within the returned aggregate output.
    pub output_range: RelativeByteRange,
    pub declared_output_len: usize,
    pub source_sha256: String,
    pub output_sha256: String,
}

/// Metadata for source bytes remaining after the explicitly requested count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderedRawDeflateSuffix {
    pub offset: usize,
    pub len: usize,
    pub sha256: String,
}

/// Candidate materialization result. Suffix bytes are deliberately not copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderedRawDeflateSequence {
    pub streams: Vec<HeaderedRawDeflateStream>,
    pub bytes: Vec<u8>,
    pub trailing_suffix: HeaderedRawDeflateSuffix,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderedRawDeflateError {
    InputLimitExceeded {
        bytes: usize,
        limit: usize,
    },
    CursorOutsideSource {
        cursor: usize,
        source_len: usize,
    },
    ZeroStreamCount,
    StreamCountLimitExceeded {
        streams: usize,
        limit: usize,
    },
    TruncatedHeader {
        stream: usize,
        cursor: usize,
    },
    InvalidMagic {
        stream: usize,
        cursor: usize,
        found: [u8; 2],
    },
    ZeroDeclaredOutput {
        stream: usize,
    },
    StreamOutputLimitExceeded {
        stream: usize,
        bytes: usize,
        limit: usize,
    },
    AggregateOutputOverflow,
    AggregateOutputLimitExceeded {
        bytes: usize,
        limit: usize,
    },
    OutputAllocationOverflow {
        stream: usize,
    },
    DeflateRejected {
        stream: usize,
        message: String,
    },
    MissingStreamEnd {
        stream: usize,
    },
    NoForwardProgress {
        stream: usize,
    },
    OutputLengthMismatch {
        stream: usize,
        declared: usize,
        actual: usize,
    },
    DecoderCounterOverflow {
        stream: usize,
    },
    SourceRangeOverflow {
        stream: usize,
    },
}

/// Decode exactly `stream_count` streams beginning at `source_cursor`.
///
/// The byte immediately following the last stream begins the explicit suffix;
/// it is never probed for another header. Raw-DEFLATE counters are taken from
/// the decoder only after `StreamEnd` and must agree exactly with the retained
/// source and declared output ranges.
pub fn materialize_headered_raw_deflate_sequence(
    source: &[u8],
    source_cursor: usize,
    stream_count: usize,
    limits: HeaderedRawDeflateLimits,
) -> Result<HeaderedRawDeflateSequence, HeaderedRawDeflateError> {
    if source.len() > limits.max_input_bytes {
        return Err(HeaderedRawDeflateError::InputLimitExceeded {
            bytes: source.len(),
            limit: limits.max_input_bytes,
        });
    }
    if source_cursor > source.len() {
        return Err(HeaderedRawDeflateError::CursorOutsideSource {
            cursor: source_cursor,
            source_len: source.len(),
        });
    }
    if stream_count == 0 {
        return Err(HeaderedRawDeflateError::ZeroStreamCount);
    }
    if stream_count > limits.max_streams {
        return Err(HeaderedRawDeflateError::StreamCountLimitExceeded {
            streams: stream_count,
            limit: limits.max_streams,
        });
    }

    let mut cursor = source_cursor;
    let mut bytes = Vec::new();
    let mut streams = Vec::with_capacity(stream_count);
    for stream in 0..stream_count {
        let header_end = cursor
            .checked_add(HEADER_LEN)
            .ok_or(HeaderedRawDeflateError::SourceRangeOverflow { stream })?;
        let header = source
            .get(cursor..header_end)
            .ok_or(HeaderedRawDeflateError::TruncatedHeader { stream, cursor })?;
        let found = [header[0], header[1]];
        if found != MAGIC {
            return Err(HeaderedRawDeflateError::InvalidMagic {
                stream,
                cursor,
                found,
            });
        }
        let declared_output_len = u32::from_be_bytes(header[2..6].try_into().unwrap()) as usize;
        if declared_output_len == 0 {
            return Err(HeaderedRawDeflateError::ZeroDeclaredOutput { stream });
        }
        if declared_output_len > limits.max_stream_output_bytes {
            return Err(HeaderedRawDeflateError::StreamOutputLimitExceeded {
                stream,
                bytes: declared_output_len,
                limit: limits.max_stream_output_bytes,
            });
        }
        let aggregate_end = bytes
            .len()
            .checked_add(declared_output_len)
            .ok_or(HeaderedRawDeflateError::AggregateOutputOverflow)?;
        if aggregate_end > limits.max_aggregate_output_bytes {
            return Err(HeaderedRawDeflateError::AggregateOutputLimitExceeded {
                bytes: aggregate_end,
                limit: limits.max_aggregate_output_bytes,
            });
        }

        let payload = &source[header_end..];
        let output_capacity = declared_output_len
            .checked_add(1)
            .ok_or(HeaderedRawDeflateError::OutputAllocationOverflow { stream })?;
        let mut output = vec![0; output_capacity];
        let mut decoder = Decompress::new(false);
        loop {
            let before_in = decoder.total_in();
            let before_out = decoder.total_out();
            let input_offset = usize::try_from(before_in)
                .map_err(|_| HeaderedRawDeflateError::DecoderCounterOverflow { stream })?;
            let output_offset = usize::try_from(before_out)
                .map_err(|_| HeaderedRawDeflateError::DecoderCounterOverflow { stream })?;
            let input = payload
                .get(input_offset..)
                .ok_or(HeaderedRawDeflateError::DecoderCounterOverflow { stream })?;
            let destination = output.get_mut(output_offset..).ok_or(
                HeaderedRawDeflateError::OutputLengthMismatch {
                    stream,
                    declared: declared_output_len,
                    actual: output_offset,
                },
            )?;
            let status = decoder
                .decompress(input, destination, FlushDecompress::Finish)
                .map_err(|error| HeaderedRawDeflateError::DeflateRejected {
                    stream,
                    message: error.to_string(),
                })?;
            let after_in = decoder.total_in();
            let after_out = decoder.total_out();
            if status == Status::StreamEnd {
                break;
            }
            if after_out > declared_output_len as u64 {
                return Err(HeaderedRawDeflateError::OutputLengthMismatch {
                    stream,
                    declared: declared_output_len,
                    actual: usize::try_from(after_out).unwrap_or(usize::MAX),
                });
            }
            if after_in == before_in && after_out == before_out {
                if usize::try_from(after_in).ok() == Some(payload.len()) {
                    return Err(HeaderedRawDeflateError::MissingStreamEnd { stream });
                }
                return Err(HeaderedRawDeflateError::NoForwardProgress { stream });
            }
            if usize::try_from(after_in).ok() == Some(payload.len()) {
                return Err(HeaderedRawDeflateError::MissingStreamEnd { stream });
            }
        }

        let consumed = usize::try_from(decoder.total_in())
            .map_err(|_| HeaderedRawDeflateError::DecoderCounterOverflow { stream })?;
        let actual_output_len = usize::try_from(decoder.total_out())
            .map_err(|_| HeaderedRawDeflateError::DecoderCounterOverflow { stream })?;
        if actual_output_len != declared_output_len {
            return Err(HeaderedRawDeflateError::OutputLengthMismatch {
                stream,
                declared: declared_output_len,
                actual: actual_output_len,
            });
        }
        if consumed == 0 {
            return Err(HeaderedRawDeflateError::NoForwardProgress { stream });
        }
        let source_end = header_end
            .checked_add(consumed)
            .ok_or(HeaderedRawDeflateError::SourceRangeOverflow { stream })?;
        if source_end > source.len() {
            return Err(HeaderedRawDeflateError::DecoderCounterOverflow { stream });
        }
        output.truncate(actual_output_len);
        let output_start = bytes.len();
        let output_end = output_start
            .checked_add(output.len())
            .ok_or(HeaderedRawDeflateError::AggregateOutputOverflow)?;
        let source_range = RelativeByteRange {
            start: cursor,
            end: source_end,
        };
        let deflate_range = RelativeByteRange {
            start: header_end,
            end: source_end,
        };
        let output_range = RelativeByteRange {
            start: output_start,
            end: output_end,
        };
        streams.push(HeaderedRawDeflateStream {
            source_range,
            deflate_range,
            output_range,
            declared_output_len,
            source_sha256: sha256(&source[source_range.start..source_range.end]),
            output_sha256: sha256(&output),
        });
        bytes.extend_from_slice(&output);
        cursor = source_end;
    }

    let suffix = &source[cursor..];
    Ok(HeaderedRawDeflateSequence {
        streams,
        bytes,
        trailing_suffix: HeaderedRawDeflateSuffix {
            offset: cursor,
            len: suffix.len(),
            sha256: sha256(suffix),
        },
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::DeflateEncoder, Compression};
    use std::io::Write;

    fn stream(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut result = Vec::with_capacity(HEADER_LEN + compressed.len());
        result.extend_from_slice(&MAGIC);
        result.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        result.extend_from_slice(&compressed);
        result
    }

    fn limits() -> HeaderedRawDeflateLimits {
        HeaderedRawDeflateLimits {
            max_input_bytes: 4096,
            max_stream_output_bytes: 1024,
            max_aggregate_output_bytes: 2048,
            max_streams: 4,
        }
    }

    #[test]
    fn materializes_two_explicit_streams_and_retains_suffix_metadata() {
        let first = b"first synthetic payload";
        let second = b"second payload with a distinct length";
        let prefix = [0xaa, 0xbb, 0xcc];
        let suffix = [0xde, 0xad, 0xbe, 0xef];
        let first_stream = stream(first);
        let second_stream = stream(second);
        let mut source = prefix.to_vec();
        source.extend_from_slice(&first_stream);
        source.extend_from_slice(&second_stream);
        source.extend_from_slice(&suffix);

        let result =
            materialize_headered_raw_deflate_sequence(&source, prefix.len(), 2, limits()).unwrap();

        assert_eq!(result.streams.len(), 2);
        assert_eq!(result.bytes, [first.as_slice(), second.as_slice()].concat());
        assert_eq!(result.streams[0].source_range.start, prefix.len());
        assert_eq!(result.streams[0].source_range.len(), first_stream.len());
        assert_eq!(
            result.streams[0].output_range,
            RelativeByteRange {
                start: 0,
                end: first.len()
            }
        );
        assert_eq!(
            result.streams[1].source_range.start,
            prefix.len() + first_stream.len()
        );
        assert_eq!(
            result.streams[1].output_range,
            RelativeByteRange {
                start: first.len(),
                end: first.len() + second.len()
            }
        );
        assert_eq!(result.trailing_suffix.offset, source.len() - suffix.len());
        assert_eq!(result.trailing_suffix.len, suffix.len());
        assert_eq!(result.trailing_suffix.sha256, sha256(&suffix));
        assert_eq!(result.streams[0].source_sha256, sha256(&first_stream));
        assert_eq!(result.streams[1].output_sha256, sha256(second));
    }

    #[test]
    fn rejects_malformed_deflate() {
        let mut source = vec![0x11, 0x72, 0, 0, 0, 8];
        source.extend_from_slice(&[0xff; 16]);
        assert!(matches!(
            materialize_headered_raw_deflate_sequence(&source, 0, 1, limits()),
            Err(HeaderedRawDeflateError::DeflateRejected { stream: 0, .. })
        ));
    }

    #[test]
    fn rejects_truncated_header_and_stream() {
        assert_eq!(
            materialize_headered_raw_deflate_sequence(&[0x11, 0x72, 0], 0, 1, limits()),
            Err(HeaderedRawDeflateError::TruncatedHeader {
                stream: 0,
                cursor: 0
            })
        );

        let mut truncated = stream(b"a payload long enough to require compressed bytes");
        truncated.truncate(HEADER_LEN + 1);
        assert!(matches!(
            materialize_headered_raw_deflate_sequence(&truncated, 0, 1, limits()),
            Err(HeaderedRawDeflateError::MissingStreamEnd { stream: 0 })
                | Err(HeaderedRawDeflateError::DeflateRejected { stream: 0, .. })
        ));
    }

    #[test]
    fn rejects_declared_output_over_resource_limits() {
        let mut source = stream(b"small");
        source[2..6].copy_from_slice(&1025u32.to_be_bytes());
        assert_eq!(
            materialize_headered_raw_deflate_sequence(&source, 0, 1, limits()),
            Err(HeaderedRawDeflateError::StreamOutputLimitExceeded {
                stream: 0,
                bytes: 1025,
                limit: 1024,
            })
        );

        let one = stream(&vec![1; 700]);
        let two = stream(&vec![2; 700]);
        let source = [one, two].concat();
        let mut aggregate_limited = limits();
        aggregate_limited.max_aggregate_output_bytes = 1000;
        assert_eq!(
            materialize_headered_raw_deflate_sequence(&source, 0, 2, aggregate_limited),
            Err(HeaderedRawDeflateError::AggregateOutputLimitExceeded {
                bytes: 1400,
                limit: 1000,
            })
        );
    }

    #[test]
    fn rejects_missing_end_marker() {
        let mut source = stream(&vec![0x5a; 512]);
        source.pop();
        assert!(matches!(
            materialize_headered_raw_deflate_sequence(&source, 0, 1, limits()),
            Err(HeaderedRawDeflateError::MissingStreamEnd { stream: 0 })
                | Err(HeaderedRawDeflateError::DeflateRejected { stream: 0, .. })
        ));
    }

    #[test]
    fn rejects_declared_output_mismatch() {
        let mut source = stream(b"known output");
        source[2..6].copy_from_slice(&11u32.to_be_bytes());
        assert_eq!(
            materialize_headered_raw_deflate_sequence(&source, 0, 1, limits()),
            Err(HeaderedRawDeflateError::OutputLengthMismatch {
                stream: 0,
                declared: 11,
                actual: 12,
            })
        );
    }
}
