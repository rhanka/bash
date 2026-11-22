//! Escape processors (`process_format_escapes`, `process_b_escapes`), `%q` shell quoting,
//! and UTF-8 / UTF-16 scalar helpers.

use super::FormatLimitKind;
use crate::mem::{Budget, KVec};
use crate::num::next_utf8_recovery;

pub struct BEscapeOut {
    pub bytes: KVec<u8>,
    pub stopped: bool,
}

#[inline]
fn hex_digit(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some((b - b'0') as u32),
        b'a'..=b'f' => Some((10 + b - b'a') as u32),
        b'A'..=b'F' => Some((10 + b - b'A') as u32),
        _ => None,
    }
}

/// Computes the UTF-8 byte length of `raw` after applying Latin-1 recovery (`0x80..=0xFF` -> 2-byte UTF-8).
pub(crate) fn decoded_utf8_len(raw: &[u8]) -> usize {
    let mut i = 0usize;
    let mut total = 0usize;
    while i < raw.len() {
        let (cp, len) = next_utf8_recovery(&raw[i..]);
        total += char::from_u32(cp).map_or(3, char::len_utf8);
        i += len;
    }
    total
}

/// Decodes `raw` using UTF-8 with Latin-1 recovery into a UTF-8 `KVec<u8>`.
pub(crate) fn decode_utf8_recovery(raw: &[u8], b: &Budget) -> Result<KVec<u8>, FormatLimitKind> {
    let mut out = KVec::with_capacity_in(raw.len(), b)?;
    let mut i = 0usize;
    while i < raw.len() {
        let (cp, len) = next_utf8_recovery(&raw[i..]);
        let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
        let mut buf = [0u8; 4];
        out.try_extend_from_slice(ch.encode_utf8(&mut buf).as_bytes())?;
        i += len;
    }
    Ok(out)
}

#[inline]
pub(crate) fn utf16_units_len(utf8: &[u8]) -> usize {
    let s = core::str::from_utf8(utf8).unwrap_or("");
    s.chars().map(char::len_utf16).sum()
}

/// Truncates valid UTF-8 `utf8` to `max_units` UTF-16 code units, replacing a split astral pair
/// with `U+FFFD` (`0xEF 0xBF 0xBD`).
pub(crate) fn truncate_utf16(utf8: &[u8], max_units: usize, b: &Budget) -> Result<KVec<u8>, FormatLimitKind> {
    let s = core::str::from_utf8(utf8).unwrap_or("");
    let mut units = 0usize;
    let mut out = KVec::new_in(b);
    for ch in s.chars() {
        let need = ch.len_utf16();
        if units + need <= max_units {
            let mut buf = [0u8; 4];
            out.try_extend_from_slice(ch.encode_utf8(&mut buf).as_bytes())?;
            units += need;
        } else {
            if need == 2 && units + 1 == max_units {
                out.try_extend_from_slice("\u{FFFD}".as_bytes())?;
            }
            break;
        }
    }
    Ok(out)
}

fn push_char_checked(
    out: &mut KVec<u8>,
    ch: char,
    max_bytes: usize,
    limit_err: fn(usize) -> FormatLimitKind,
) -> Result<(), FormatLimitKind> {
    let mut buf = [0u8; 4];
    let enc = ch.encode_utf8(&mut buf).as_bytes();
    if out.len().saturating_add(enc.len()) > max_bytes {
        return Err(limit_err(max_bytes));
    }
    out.try_extend_from_slice(enc)?;
    Ok(())
}

fn push_slice_checked(
    out: &mut KVec<u8>,
    slice: &[u8],
    max_bytes: usize,
    limit_err: fn(usize) -> FormatLimitKind,
) -> Result<(), FormatLimitKind> {
    if out.len().saturating_add(slice.len()) > max_bytes {
        return Err(limit_err(max_bytes));
    }
    out.try_extend_from_slice(slice)?;
    Ok(())
}

fn esc_limit(limit_bytes: usize) -> FormatLimitKind {
    FormatLimitKind::EscapedStringSize { limit_bytes }
}

fn out_limit(limit_bytes: usize) -> FormatLimitKind {
    FormatLimitKind::OutputSize { limit_bytes }
}

fn parse_unicode_escape(s: &[u8], max_digits: usize) -> Option<(char, usize)> {
    let mut val = 0u32;
    let mut digits = 0usize;
    while digits < max_digits && digits < s.len() {
        let Some(d) = hex_digit(s[digits]) else {
            break;
        };
        val = (val << 4) | d;
        digits += 1;
    }
    if digits == 0 {
        return None;
    }
    let ch = char::from_u32(val).unwrap_or('\u{FFFD}');
    Some((ch, digits))
}

pub fn process_format_escapes(
    raw: &[u8],
    max_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, FormatLimitKind> {
    if decoded_utf8_len(raw) > max_bytes {
        return Err(FormatLimitKind::EscapedStringSize { limit_bytes: max_bytes });
    }
    let input = decode_utf8_recovery(raw, b)?;
    let s = input.as_slice();
    let mut out = KVec::with_capacity_in(s.len(), b)?;
    let mut hex_run = KVec::new_in(b);
    let mut i = 0usize;
    let mut steps = 0usize;

    while i < s.len() {
        steps += 1;
        if steps % 64 == 0 && work(1).is_err() {
            return Err(FormatLimitKind::EscapedStringSize { limit_bytes: max_bytes });
        }

        if s[i] != b'\\' {
            let (cp, len) = next_utf8_recovery(&s[i..]);
            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
            push_char_checked(&mut out, ch, max_bytes, esc_limit)?;
            i += len;
            continue;
        }
        if i + 1 >= s.len() {
            push_char_checked(&mut out, '\\', max_bytes, esc_limit)?;
            i += 1;
            continue;
        }

        match s[i + 1] {
            b'a' => {
                push_char_checked(&mut out, '\x07', max_bytes, esc_limit)?;
                i += 2;
            }
            b'b' => {
                push_char_checked(&mut out, '\x08', max_bytes, esc_limit)?;
                i += 2;
            }
            b'e' | b'E' => {
                push_char_checked(&mut out, '\x1b', max_bytes, esc_limit)?;
                i += 2;
            }
            b'f' => {
                push_char_checked(&mut out, '\x0c', max_bytes, esc_limit)?;
                i += 2;
            }
            b'n' => {
                push_char_checked(&mut out, '\n', max_bytes, esc_limit)?;
                i += 2;
            }
            b'r' => {
                push_char_checked(&mut out, '\r', max_bytes, esc_limit)?;
                i += 2;
            }
            b't' => {
                push_char_checked(&mut out, '\t', max_bytes, esc_limit)?;
                i += 2;
            }
            b'v' => {
                push_char_checked(&mut out, '\x0b', max_bytes, esc_limit)?;
                i += 2;
            }
            b'\\' => {
                push_char_checked(&mut out, '\\', max_bytes, esc_limit)?;
                i += 2;
            }
            b'0'..=b'7' => {
                let mut val = 0u32;
                let mut digits = 0usize;
                while digits < 3 && i + 1 + digits < s.len() {
                    let b_dig = s[i + 1 + digits];
                    if !(b'0'..=b'7').contains(&b_dig) {
                        break;
                    }
                    val = (val << 3) | ((b_dig - b'0') as u32);
                    digits += 1;
                }
                let ch = char::from_u32(val).unwrap_or('\u{FFFD}');
                push_char_checked(&mut out, ch, max_bytes, esc_limit)?;
                i += 1 + digits;
            }
            b'x' => {
                // Collect consecutive `\xHH` escapes (requiring 2 hex digits each).
                hex_run.clear();
                let mut j = i;
                while j + 3 < s.len() && s[j] == b'\\' && s[j + 1] == b'x' {
                    if let (Some(h), Some(l)) = (hex_digit(s[j + 2]), hex_digit(s[j + 3])) {
                        hex_run.try_push(((h << 4) | l) as u8)?;
                        j += 4;
                    } else {
                        break;
                    }
                }
                if hex_run.is_empty() {
                    push_slice_checked(&mut out, b"\\x", max_bytes, esc_limit)?;
                    i += 2;
                } else {
                    if let Ok(valid) = core::str::from_utf8(hex_run.as_slice()) {
                        push_slice_checked(&mut out, valid.as_bytes(), max_bytes, esc_limit)?;
                    } else {
                        for &byte in hex_run.as_slice() {
                            push_char_checked(&mut out, char::from(byte), max_bytes, esc_limit)?;
                        }
                    }
                    i = j;
                }
            }
            b'u' => {
                if let Some((ch, digits)) = parse_unicode_escape(&s[i + 2..], 4) {
                    push_char_checked(&mut out, ch, max_bytes, esc_limit)?;
                    i += 2 + digits;
                } else {
                    push_slice_checked(&mut out, b"\\u", max_bytes, esc_limit)?;
                    i += 2;
                }
            }
            b'U' => {
                if let Some((ch, digits)) = parse_unicode_escape(&s[i + 2..], 8) {
                    push_char_checked(&mut out, ch, max_bytes, esc_limit)?;
                    i += 2 + digits;
                } else {
                    push_slice_checked(&mut out, b"\\U", max_bytes, esc_limit)?;
                    i += 2;
                }
            }
            _ => {
                push_char_checked(&mut out, '\\', max_bytes, esc_limit)?;
                i += 1;
            }
        }
    }

    Ok(out)
}

pub fn process_b_escapes(
    raw: &[u8],
    max_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<BEscapeOut, FormatLimitKind> {
    if decoded_utf8_len(raw) > max_bytes {
        return Err(FormatLimitKind::OutputSize { limit_bytes: max_bytes });
    }
    let input = decode_utf8_recovery(raw, b)?;
    let s = input.as_slice();
    let mut out = KVec::with_capacity_in(s.len(), b)?;
    let mut hex_run = KVec::new_in(b);
    let mut i = 0usize;
    let mut steps = 0usize;

    while i < s.len() {
        steps += 1;
        if steps % 64 == 0 && work(1).is_err() {
            return Err(FormatLimitKind::OutputSize { limit_bytes: max_bytes });
        }

        if s[i] != b'\\' {
            let (cp, len) = next_utf8_recovery(&s[i..]);
            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
            push_char_checked(&mut out, ch, max_bytes, out_limit)?;
            i += len;
            continue;
        }
        if i + 1 >= s.len() {
            push_char_checked(&mut out, '\\', max_bytes, out_limit)?;
            i += 1;
            continue;
        }

        match s[i + 1] {
            b'a' => {
                push_char_checked(&mut out, '\x07', max_bytes, out_limit)?;
                i += 2;
            }
            b'b' => {
                push_char_checked(&mut out, '\x08', max_bytes, out_limit)?;
                i += 2;
            }
            b'c' => {
                return Ok(BEscapeOut { bytes: out, stopped: true });
            }
            b'f' => {
                push_char_checked(&mut out, '\x0c', max_bytes, out_limit)?;
                i += 2;
            }
            b'n' => {
                push_char_checked(&mut out, '\n', max_bytes, out_limit)?;
                i += 2;
            }
            b'r' => {
                push_char_checked(&mut out, '\r', max_bytes, out_limit)?;
                i += 2;
            }
            b't' => {
                push_char_checked(&mut out, '\t', max_bytes, out_limit)?;
                i += 2;
            }
            b'v' => {
                push_char_checked(&mut out, '\x0b', max_bytes, out_limit)?;
                i += 2;
            }
            b'\\' => {
                push_char_checked(&mut out, '\\', max_bytes, out_limit)?;
                i += 2;
            }
            b'0' => {
                // `\0[0-7]{0,3}`
                let mut val = 0u32;
                let mut digits = 0usize;
                while digits < 3 && i + 2 + digits < s.len() {
                    let b_dig = s[i + 2 + digits];
                    if !(b'0'..=b'7').contains(&b_dig) {
                        break;
                    }
                    val = (val << 3) | ((b_dig - b'0') as u32);
                    digits += 1;
                }
                let ch = char::from_u32(val).unwrap_or('\u{FFFD}');
                push_char_checked(&mut out, ch, max_bytes, out_limit)?;
                i += 2 + digits;
            }
            b'1'..=b'7' => {
                let mut val = 0u32;
                let mut digits = 0usize;
                while digits < 3 && i + 1 + digits < s.len() {
                    let b_dig = s[i + 1 + digits];
                    if !(b'0'..=b'7').contains(&b_dig) {
                        break;
                    }
                    val = (val << 3) | ((b_dig - b'0') as u32);
                    digits += 1;
                }
                let ch = char::from_u32(val).unwrap_or('\u{FFFD}');
                push_char_checked(&mut out, ch, max_bytes, out_limit)?;
                i += 1 + digits;
            }
            b'x' => {
                // Collect consecutive `\xH` or `\xHH` escapes and decode with UTF-8 recovery.
                hex_run.clear();
                let mut j = i;
                while j + 2 < s.len() && s[j] == b'\\' && s[j + 1] == b'x' {
                    let Some(h0) = hex_digit(s[j + 2]) else {
                        break;
                    };
                    if j + 3 < s.len() {
                        if let Some(h1) = hex_digit(s[j + 3]) {
                            hex_run.try_push(((h0 << 4) | h1) as u8)?;
                            j += 4;
                            continue;
                        }
                    }
                    hex_run.try_push(h0 as u8)?;
                    j += 3;
                }
                if hex_run.is_empty() {
                    push_slice_checked(&mut out, b"\\x", max_bytes, out_limit)?;
                    i += 2;
                } else {
                    let mut k = 0usize;
                    let run = hex_run.as_slice();
                    while k < run.len() {
                        let (cp, len) = next_utf8_recovery(&run[k..]);
                        let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                        push_char_checked(&mut out, ch, max_bytes, out_limit)?;
                        k += len;
                    }
                    i = j;
                }
            }
            b'u' => {
                if let Some((ch, digits)) = parse_unicode_escape(&s[i + 2..], 4) {
                    push_char_checked(&mut out, ch, max_bytes, out_limit)?;
                    i += 2 + digits;
                } else {
                    push_slice_checked(&mut out, b"\\u", max_bytes, out_limit)?;
                    i += 2;
                }
            }
            _ => {
                // `\e`, `\E`, `\U`, or any other `\X` stays literal.
                push_char_checked(&mut out, '\\', max_bytes, out_limit)?;
                i += 1;
            }
        }
    }

    Ok(BEscapeOut { bytes: out, stopped: false })
}

#[inline]
fn is_safe_q_ascii(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-')
}

#[inline]
fn needs_backslash_q(cp: u32) -> bool {
    matches!(
        cp,
        0x20 | 0x09 | 0x7c | 0x26 | 0x3b | 0x3c | 0x3e | 0x28 | 0x29 | 0x24 | 0x60 | 0x5c | 0x22
            | 0x27 | 0x2a | 0x3f | 0x5b | 0x23 | 0x7e | 0x3d | 0x25 | 0x21 | 0x7b | 0x7d
    )
}

pub fn shell_quote_printf(
    raw: &[u8],
    max_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, FormatLimitKind> {
    if raw.is_empty() {
        if max_bytes < 2 {
            return Err(FormatLimitKind::OutputSize { limit_bytes: max_bytes });
        }
        let mut out = KVec::with_capacity_in(2, b)?;
        out.try_extend_from_slice(b"''")?;
        return Ok(out);
    }

    if raw.iter().all(|&c| is_safe_q_ascii(c)) {
        if raw.len() > max_bytes {
            return Err(FormatLimitKind::OutputSize { limit_bytes: max_bytes });
        }
        let mut out = KVec::with_capacity_in(raw.len(), b)?;
        out.try_extend_from_slice(raw)?;
        return Ok(out);
    }

    let mut needs_dollar = false;
    let mut i = 0usize;
    let mut steps = 0usize;
    while i < raw.len() {
        steps += 1;
        if steps % 64 == 0 && work(1).is_err() {
            return Err(FormatLimitKind::OutputSize { limit_bytes: max_bytes });
        }
        let (cp, len) = next_utf8_recovery(&raw[i..]);
        if cp <= 0x1f || (0x7f..=0xff).contains(&cp) {
            needs_dollar = true;
            break;
        }
        i += len;
    }

    let mut out = KVec::new_in(b);
    if needs_dollar {
        push_slice_checked(&mut out, b"$'", max_bytes, out_limit)?;
        let mut pos = 0usize;
        let mut nsteps = 0usize;
        while pos < raw.len() {
            nsteps += 1;
            if nsteps % 64 == 0 && work(1).is_err() {
                return Err(FormatLimitKind::OutputSize { limit_bytes: max_bytes });
            }
            let (cp, len) = next_utf8_recovery(&raw[pos..]);
            pos += len;
            match cp {
                0x27 => push_slice_checked(&mut out, b"\\'", max_bytes, out_limit)?,
                0x5c => push_slice_checked(&mut out, b"\\\\", max_bytes, out_limit)?,
                0x0a => push_slice_checked(&mut out, b"\\n", max_bytes, out_limit)?,
                0x09 => push_slice_checked(&mut out, b"\\t", max_bytes, out_limit)?,
                0x0d => push_slice_checked(&mut out, b"\\r", max_bytes, out_limit)?,
                0x07 => push_slice_checked(&mut out, b"\\a", max_bytes, out_limit)?,
                0x08 => push_slice_checked(&mut out, b"\\b", max_bytes, out_limit)?,
                0x0c => push_slice_checked(&mut out, b"\\f", max_bytes, out_limit)?,
                0x0b => push_slice_checked(&mut out, b"\\v", max_bytes, out_limit)?,
                0x1b => push_slice_checked(&mut out, b"\\E", max_bytes, out_limit)?,
                0x22 => push_slice_checked(&mut out, b"\\\"", max_bytes, out_limit)?,
                c if c <= 0x1f || (0x7f..=0xff).contains(&c) => {
                    let oct = [
                        b'\\',
                        b'0' + ((c >> 6) & 7) as u8,
                        b'0' + ((c >> 3) & 7) as u8,
                        b'0' + (c & 7) as u8,
                    ];
                    push_slice_checked(&mut out, &oct, max_bytes, out_limit)?;
                }
                c => {
                    let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
                    push_char_checked(&mut out, ch, max_bytes, out_limit)?;
                }
            }
        }
        push_char_checked(&mut out, '\'', max_bytes, out_limit)?;
    } else {
        let mut pos = 0usize;
        let mut nsteps = 0usize;
        while pos < raw.len() {
            nsteps += 1;
            if nsteps % 64 == 0 && work(1).is_err() {
                return Err(FormatLimitKind::OutputSize { limit_bytes: max_bytes });
            }
            let (cp, len) = next_utf8_recovery(&raw[pos..]);
            pos += len;
            if needs_backslash_q(cp) {
                push_char_checked(&mut out, '\\', max_bytes, out_limit)?;
            }
            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
            push_char_checked(&mut out, ch, max_bytes, out_limit)?;
        }
    }

    Ok(out)
}
