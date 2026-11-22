//! RFC 4648 base64 encoding and permissive/strict decoding with prospective limit gates.

use crate::mem::{Budget, KVec, MemError};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base64DecodeMode {
    /// Node `Buffer.from(cleaned, "base64")` behavior required for `base64 -d` parity:
    /// strips whitespace, ignores non-alphabet bytes outside `[A-Za-z0-9+/=]`, stops at first `=`,
    /// decodes 2-char and 3-char unpadded tail groups, and drops a trailing single-sextet orphan.
    Permissive,
    /// Strict RFC 4648 validation (after stripping ASCII whitespace): rejects non-alphabet bytes,
    /// `len % 4 == 1`, mid-stream `=`, or >2 `=` pad bytes with `Base64Error::InvalidInput`.
    Strict,
}

#[derive(Clone, Copy, Debug)]
pub struct Base64Limits {
    pub max_string_length: usize,
    pub max_output_size: usize,
    pub max_array_elements: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Base64Error {
    /// `base64: invalid input\n` (exit 1)
    InvalidInput,
    /// `base64: wrapped line limit exceeded (<limit>)\n` (exit 126, `maxArrayElements`)
    WrappedLineLimit { limit: usize },
    /// `base64: encoded size limit exceeded (<limit> bytes)\n` (exit 126, `maxStringLength`)
    EncodedSizeLimit { limit: usize },
    /// `base64: decoded size limit exceeded (<limit> bytes)\n` (exit 126, `maxStringLength`)
    DecodedSizeLimit { limit: usize },
    /// `base64: output size limit exceeded (<limit> bytes)\n` (exit 126, `maxOutputSize`)
    OutputSizeLimit { limit: usize },
    FuelExhausted,
    Mem(MemError),
}

impl From<MemError> for Base64Error {
    fn from(e: MemError) -> Self {
        Self::Mem(e)
    }
}

/// Returns `(total_bytes_with_newlines, newline_count)` for base64 encoding `input_len` bytes.
pub fn encoded_output_len(input_len: usize, wrap_cols: usize) -> Option<(usize, usize)> {
    let encoded_len = input_len.div_ceil(3).checked_mul(4)?;
    let newline_count = if wrap_cols > 0 && encoded_len > 0 {
        encoded_len.div_ceil(wrap_cols)
    } else {
        0
    };
    let total = encoded_len.checked_add(newline_count)?;
    Some((total, newline_count))
}

pub fn encode(
    input: &[u8],
    wrap_cols: usize,
    limits: Base64Limits,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, Base64Error> {
    let (total_len, newline_count) =
        encoded_output_len(input.len(), wrap_cols).ok_or(Base64Error::Mem(MemError::Overflow))?;
    let encoded_len = total_len - newline_count;

    if newline_count > limits.max_array_elements {
        return Err(Base64Error::WrappedLineLimit { limit: limits.max_array_elements });
    }
    if encoded_len > limits.max_string_length {
        return Err(Base64Error::EncodedSizeLimit { limit: limits.max_string_length });
    }
    if total_len > limits.max_output_size {
        return Err(Base64Error::OutputSizeLimit { limit: limits.max_output_size });
    }

    if input.is_empty() {
        return Ok(KVec::new_in(b));
    }

    let mut out = KVec::with_capacity_in(total_len, b)?;
    let mut col = 0usize;
    let mut push_b64 = |byte: u8| -> Result<(), Base64Error> {
        out.try_push(byte)?;
        if wrap_cols > 0 {
            col += 1;
            if col == wrap_cols {
                out.try_push(b'\n')?;
                col = 0;
            }
        }
        Ok(())
    };

    let mut processed = 0usize;
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;

        push_b64(ALPHABET[b0 >> 2])?;
        push_b64(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)])?;
        if chunk.len() >= 2 {
            push_b64(ALPHABET[((b1 & 0x0f) << 2) | (b2 >> 6)])?;
        } else {
            push_b64(b'=')?;
        }
        if chunk.len() == 3 {
            push_b64(ALPHABET[b2 & 0x3f])?;
        } else {
            push_b64(b'=')?;
        }

        processed += chunk.len();
        if processed >= 1024 {
            let units = (processed / 1024) as u64;
            processed %= 1024;
            work(units).map_err(|()| Base64Error::FuelExhausted)?;
        }
    }
    if processed > 0 {
        work(1).map_err(|()| Base64Error::FuelExhausted)?;
    }

    if wrap_cols > 0 && col > 0 {
        out.try_push(b'\n')?;
    }
    Ok(out)
}

#[inline]
fn decode_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[inline]
fn is_ascii_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

pub fn decode(
    input: &[u8],
    mode: Base64DecodeMode,
    limits: Base64Limits,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, Base64Error> {
    let mut cleaned_len = 0usize;
    let mut pad_count = 0usize;
    let mut saw_pad = false;

    for &byte in input {
        if is_ascii_ws(byte) {
            continue;
        }
        match mode {
            Base64DecodeMode::Permissive => {
                if saw_pad {
                    continue;
                }
                if byte == b'=' {
                    saw_pad = true;
                    pad_count += 1;
                    cleaned_len += 1;
                } else if decode_sextet(byte).is_some() {
                    cleaned_len += 1;
                }
            }
            Base64DecodeMode::Strict => {
                if byte == b'=' {
                    saw_pad = true;
                    pad_count += 1;
                    if pad_count > 2 {
                        return Err(Base64Error::InvalidInput);
                    }
                    cleaned_len += 1;
                } else if decode_sextet(byte).is_some() {
                    if saw_pad {
                        return Err(Base64Error::InvalidInput);
                    }
                    cleaned_len += 1;
                } else {
                    return Err(Base64Error::InvalidInput);
                }
            }
        }
    }

    if mode == Base64DecodeMode::Strict {
        let data_len = cleaned_len - pad_count;
        if data_len % 4 == 1 || (pad_count > 0 && cleaned_len % 4 != 0) {
            return Err(Base64Error::InvalidInput);
        }
    }

    let est_len = if mode == Base64DecodeMode::Permissive {
        let data_len = cleaned_len.saturating_sub(pad_count);
        (data_len / 4) * 3 + match data_len % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        }
    } else {
        cleaned_len
            .div_ceil(4)
            .saturating_mul(3)
            .saturating_sub(pad_count)
    };

    if est_len > limits.max_string_length {
        return Err(Base64Error::DecodedSizeLimit { limit: limits.max_string_length });
    }
    if est_len > limits.max_output_size {
        return Err(Base64Error::OutputSizeLimit { limit: limits.max_output_size });
    }

    if est_len == 0 {
        if !input.is_empty() {
            work(1).map_err(|()| Base64Error::FuelExhausted)?;
        }
        return Ok(KVec::new_in(b));
    }

    let mut out = KVec::with_capacity_in(est_len, b)?;
    let mut group = [0u8; 4];
    let mut g_len = 0usize;
    let mut processed = 0usize;

    for &byte in input {
        processed += 1;
        if processed >= 1024 {
            work((processed / 1024) as u64).map_err(|()| Base64Error::FuelExhausted)?;
            processed %= 1024;
        }
        if byte == b'=' {
            break;
        }
        if let Some(s) = decode_sextet(byte) {
            group[g_len] = s;
            g_len += 1;
            if g_len == 4 {
                out.try_push((group[0] << 2) | (group[1] >> 4))?;
                out.try_push((group[1] << 4) | (group[2] >> 2))?;
                out.try_push((group[2] << 6) | group[3])?;
                g_len = 0;
            }
        }
    }
    if processed > 0 {
        work(1).map_err(|()| Base64Error::FuelExhausted)?;
    }

    match g_len {
        2 => {
            out.try_push((group[0] << 2) | (group[1] >> 4))?;
        }
        3 => {
            out.try_push((group[0] << 2) | (group[1] >> 4))?;
            out.try_push((group[1] << 4) | (group[2] >> 2))?;
        }
        _ => {}
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::LimitId;

    fn unlimited_limits() -> Base64Limits {
        Base64Limits {
            max_string_length: 16 << 20,
            max_output_size: 32 << 20,
            max_array_elements: 1_000_000,
        }
    }

    fn budget() -> Budget {
        crate::mem::testing::reset(1 << 30);
        Budget::root(LimitId(30), u64::MAX).unwrap()
    }

    #[test]
    fn rfc4648_section10_vectors() {
        let b = budget();
        let lim = unlimited_limits();
        let mut noop = |_: u64| Ok(());
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"f", b"Zg=="),
            (b"fo", b"Zm8="),
            (b"foo", b"Zm9v"),
            (b"foob", b"Zm9vYg=="),
            (b"fooba", b"Zm9vYmE="),
            (b"foobar", b"Zm9vYmFy"),
        ];
        for (plain, enc) in cases {
            let out = encode(plain, 0, lim, &b, &mut noop).unwrap();
            assert_eq!(out.as_slice(), *enc);
            let dec_s = decode(out.as_slice(), Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap();
            assert_eq!(dec_s.as_slice(), *plain);
            let dec_p = decode(out.as_slice(), Base64DecodeMode::Permissive, lim, &b, &mut noop).unwrap();
            assert_eq!(dec_p.as_slice(), *plain);
        }
    }

    #[test]
    fn wrap_cols_and_limits() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let input = [b'a'; 80];
        let out76 = encode(&input, 76, unlimited_limits(), &b, &mut noop).unwrap();
        assert_eq!(
            out76.as_slice(),
            b"YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFh\nYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=\n"
        );
        let lim_lines = Base64Limits {
            max_string_length: 1000,
            max_output_size: 1000,
            max_array_elements: 1,
        };
        assert_eq!(
            encode(&input, 76, lim_lines, &b, &mut noop).unwrap_err(),
            Base64Error::WrappedLineLimit { limit: 1 }
        );
    }

    #[test]
    fn permissive_vs_strict_decoding() {
        let b = budget();
        let lim = unlimited_limits();
        let mut noop = |_: u64| Ok(());
        assert_eq!(decode(b"Y", Base64DecodeMode::Permissive, lim, &b, &mut noop).unwrap().as_slice(), b"");
        assert_eq!(decode(b"YW", Base64DecodeMode::Permissive, lim, &b, &mut noop).unwrap().as_slice(), b"a");
        assert_eq!(decode(b"YWJ", Base64DecodeMode::Permissive, lim, &b, &mut noop).unwrap().as_slice(), b"ab");
        assert_eq!(decode(b"YW!Jj", Base64DecodeMode::Permissive, lim, &b, &mut noop).unwrap().as_slice(), b"abc");
        assert_eq!(decode(b"YW=Jj", Base64DecodeMode::Permissive, lim, &b, &mut noop).unwrap().as_slice(), b"a");

        assert_eq!(decode(b"Y", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap_err(), Base64Error::InvalidInput);
        assert_eq!(decode(b"YW!Jj", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap_err(), Base64Error::InvalidInput);
        assert_eq!(decode(b"YW=Jj", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap_err(), Base64Error::InvalidInput);
        assert_eq!(decode(b"YW=", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap_err(), Base64Error::InvalidInput);
        assert_eq!(decode(b"YW==", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap().as_slice(), b"a");
        assert_eq!(decode(b"YW", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap().as_slice(), b"a");
        assert_eq!(decode(b"YWJ", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap().as_slice(), b"ab");
    }

    #[test]
    fn one_mib_round_trip() {
        let b = budget();
        let lim = unlimited_limits();
        let mut noop = |_: u64| Ok(());
        let mut raw = KVec::with_capacity_in(1 << 20, &b).unwrap();
        for i in 0..(1 << 20) {
            raw.try_push(((i * 31 + 7) & 0xff) as u8).unwrap();
        }
        let enc = encode(raw.as_slice(), 76, lim, &b, &mut noop).unwrap();
        let dec = decode(enc.as_slice(), Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap();
        assert_eq!(dec.as_slice(), raw.as_slice());
    }

    #[test]
    fn encoded_output_len_and_all_limit_errors() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        assert_eq!(encoded_output_len(0, 76), Some((0, 0)));
        assert_eq!(encoded_output_len(3, 0), Some((4, 0)));
        assert_eq!(encoded_output_len(3, 76), Some((5, 1)));
        assert_eq!(encoded_output_len(usize::MAX, 76), None);

        let enc_lim = Base64Limits {
            max_string_length: 3,
            max_output_size: 100,
            max_array_elements: 100,
        };
        assert_eq!(
            encode(b"a", 0, enc_lim, &b, &mut noop).unwrap_err(),
            Base64Error::EncodedSizeLimit { limit: 3 }
        );

        let out_lim = Base64Limits {
            max_string_length: 100,
            max_output_size: 4,
            max_array_elements: 100,
        };
        assert_eq!(
            encode(b"a", 76, out_lim, &b, &mut noop).unwrap_err(),
            Base64Error::OutputSizeLimit { limit: 4 }
        );

        let dec_lim = Base64Limits {
            max_string_length: 2,
            max_output_size: 100,
            max_array_elements: 100,
        };
        assert_eq!(
            decode(b"Zm9v", Base64DecodeMode::Permissive, dec_lim, &b, &mut noop).unwrap_err(),
            Base64Error::DecodedSizeLimit { limit: 2 }
        );

        let mut fail_work = |_: u64| Err(());
        assert_eq!(
            encode(b"abc", 0, unlimited_limits(), &b, &mut fail_work).unwrap_err(),
            Base64Error::FuelExhausted
        );
        assert_eq!(
            decode(b"YWJj", Base64DecodeMode::Strict, unlimited_limits(), &b, &mut fail_work).unwrap_err(),
            Base64Error::FuelExhausted
        );
    }

    #[test]
    fn decode_strict_whitespace_and_bad_padding() {
        let b = budget();
        let lim = unlimited_limits();
        let mut noop = |_: u64| Ok(());
        assert_eq!(decode(b" \t\r\n ", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap().as_slice(), b"");
        assert_eq!(decode(b"YW===", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap_err(), Base64Error::InvalidInput);
        assert_eq!(decode(b"Y=Jj", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap_err(), Base64Error::InvalidInput);
        assert_eq!(decode(b"YWJj\nZg==", Base64DecodeMode::Strict, lim, &b, &mut noop).unwrap().as_slice(), b"abcf");
    }

    #[test]
    fn decode_permissive_ignores_invalid_and_stops_at_pad() {
        let b = budget();
        let lim = unlimited_limits();
        let mut noop = |_: u64| Ok(());
        assert_eq!(decode(b"***", Base64DecodeMode::Permissive, lim, &b, &mut noop).unwrap().as_slice(), b"");
        assert_eq!(decode(b"Zg==YWJj", Base64DecodeMode::Permissive, lim, &b, &mut noop).unwrap().as_slice(), b"f");
    }

    #[test]
    fn encode_empty_with_wrap_has_no_newline() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        assert_eq!(encode(b"", 76, unlimited_limits(), &b, &mut noop).unwrap().as_slice(), b"");
        assert_eq!(encode(b"", 0, unlimited_limits(), &b, &mut noop).unwrap().as_slice(), b"");
    }
}
