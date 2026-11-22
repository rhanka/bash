//! Complete bash `printf` engine and `awk` `sprintf`/`printf`/`OFMT` formatter.

use super::escape::{
    decode_utf8_recovery, process_b_escapes, process_format_escapes, shell_quote_printf,
    truncate_utf16, utf16_units_len,
};
use super::strftime::format_strftime;
use super::FormatLimitKind;
use crate::mem::{Budget, KString, KVec};
use crate::num::{
    exact_decimal_exp, f64_floor, f64_to_js_string, f64_trunc, fmt_f64_abs_radix, fmt_f64_exp,
    fmt_f64_fixed, fmt_f64_prec, next_utf8_recovery, parse_f64_prefix, parse_f64_strict,
    parse_printf_int, FloatFmtError, MAX_SAFE_INT_F64,
};
use core::fmt::Write;

pub struct PrintfOpts<'a> {
    /// Value of `$TZ` from the fiber environment (`None` when unset -> UTC).
    pub tz: Option<&'a [u8]>,
    /// Current Unix timestamp in seconds (virtual clock `0` unless `real-clock` granted), used for `%(...)T` on `""` or `"-1"`.
    pub now_sec: i64,
    /// Shell start Unix timestamp in seconds, used for `%(...)T` on `"-2"`.
    pub start_sec: i64,
    /// Effective cap `min(maxStringLength, maxOutputSize)` in bytes.
    pub max_output_bytes: usize,
}

pub struct PrintfOutput {
    pub bytes: KVec<u8>,
    /// `true` when any integer conversion failed or an invalid placeholder occurred (`exit 1`).
    pub had_error: bool,
    /// Diagnostic line to write to stderr when `had_error` is `true` (e.g. `printf: abc: invalid number\n`).
    pub stderr: KString,
    /// `true` when `%b` encountered `\c`.
    pub stopped: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum AwkArg<'a> {
    Num(f64),
    Str(&'a [u8]),
}

fn is_ascii_js_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Parses leading `[+-]?[0-9]+` integer (JS `parseInt(s, 10) || 0`) as `f64`.
fn parse_leading_int_f64(raw: &[u8]) -> f64 {
    let mut i = 0usize;
    while i < raw.len() && is_ascii_js_ws(raw[i]) {
        i += 1;
    }
    let s = &raw[i..];
    if s.is_empty() {
        return 0.0;
    }
    let start = 0usize;
    let mut pos = 0usize;
    if s[pos] == b'+' || s[pos] == b'-' {
        pos += 1;
    }
    let d_start = pos;
    while pos < s.len() && s[pos].is_ascii_digit() {
        pos += 1;
    }
    if pos == d_start {
        return 0.0;
    }
    core::str::from_utf8(&s[start..pos])
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .map(|v| if v == 0.0 { 0.0 } else { v })
        .unwrap_or(0.0)
}

fn parse_leading_i64(raw: &[u8]) -> i64 {
    let v = parse_leading_int_f64(raw);
    if !v.is_finite() {
        return 0;
    }
    f64_trunc(v) as i64
}

fn to_uint32_f64(val: f64) -> f64 {
    if !val.is_finite() {
        return 0.0;
    }
    let t = f64_trunc(val);
    let m = t - f64_floor(t / 4_294_967_296.0) * 4_294_967_296.0;
    let u = m as u32;
    u as f64
}

fn push_spaces(out: &mut KVec<u8>, count: usize) -> Result<(), FormatLimitKind> {
    const SPACES: &[u8] = b"                                ";
    let mut rem = count;
    while rem > 0 {
        let take = rem.min(SPACES.len());
        out.try_extend_from_slice(&SPACES[..take])?;
        rem -= take;
    }
    Ok(())
}

fn push_zeros(out: &mut KVec<u8>, count: usize) -> Result<(), FormatLimitKind> {
    const ZEROS: &[u8] = b"00000000000000000000000000000000";
    let mut rem = count;
    while rem > 0 {
        let take = rem.min(ZEROS.len());
        out.try_extend_from_slice(&ZEROS[..take])?;
        rem -= take;
    }
    Ok(())
}

fn push_padded_utf16(
    out: &mut KVec<u8>,
    val_utf8: &[u8],
    width: usize,
    left_align: bool,
    rem: usize,
) -> Result<(), FormatLimitKind> {
    let units = utf16_units_len(val_utf8);
    let pad = width.saturating_sub(units);
    if val_utf8.len().saturating_add(pad) > rem {
        return Err(FormatLimitKind::FormattedValueSize { limit_bytes: rem });
    }
    if left_align {
        out.try_extend_from_slice(val_utf8)?;
        push_spaces(out, pad)?;
    } else {
        push_spaces(out, pad)?;
        out.try_extend_from_slice(val_utf8)?;
    }
    Ok(())
}

fn push_padded_numeric(
    out: &mut KVec<u8>,
    prefix: &[u8],
    zero_pad_min: usize,
    digits: &[u8],
    width: usize,
    left_align: bool,
    zero_flag: bool,
    rem: usize,
) -> Result<(), FormatLimitKind> {
    let prec_zeros = zero_pad_min.saturating_sub(digits.len());
    let core_len = prefix.len().saturating_add(prec_zeros).saturating_add(digits.len());
    let width_pad = width.saturating_sub(core_len);
    let total = core_len.saturating_add(width_pad);
    if total > rem {
        return Err(FormatLimitKind::FormattedValueSize { limit_bytes: rem });
    }

    if left_align {
        out.try_extend_from_slice(prefix)?;
        push_zeros(out, prec_zeros)?;
        out.try_extend_from_slice(digits)?;
        push_spaces(out, width_pad)?;
    } else if zero_flag {
        out.try_extend_from_slice(prefix)?;
        push_zeros(out, width_pad)?;
        push_zeros(out, prec_zeros)?;
        out.try_extend_from_slice(digits)?;
    } else {
        push_spaces(out, width_pad)?;
        out.try_extend_from_slice(prefix)?;
        push_zeros(out, prec_zeros)?;
        out.try_extend_from_slice(digits)?;
    }
    Ok(())
}

/// Parses optional `%(fmt)T` directive starting at `fmt[pos]` (right after `'%'`).
/// Returns `Some((next_pos, width, left_align, precision, inner_fmt_slice))` if matched.
fn try_parse_strftime_directive<'a>(
    fmt: &'a [u8],
    pos: usize,
    rem: usize,
) -> Result<Option<(usize, usize, bool, Option<usize>, &'a [u8])>, FormatLimitKind> {
    let mut i = pos;
    let mut left_align = false;
    if i < fmt.len() && fmt[i] == b'-' {
        left_align = true;
        i += 1;
    }
    let mut width = 0usize;
    let mut has_width = false;
    while i < fmt.len() && fmt[i].is_ascii_digit() {
        has_width = true;
        width = width
            .checked_mul(10)
            .and_then(|w| w.checked_add((fmt[i] - b'0') as usize))
            .ok_or(FormatLimitKind::Width { limit_bytes: rem })?;
        if width > rem {
            return Err(FormatLimitKind::Width { limit_bytes: rem });
        }
        i += 1;
    }
    let mut precision = None;
    if i < fmt.len() && fmt[i] == b'.' {
        i += 1;
        let mut p = 0usize;
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            p = p
                .checked_mul(10)
                .and_then(|v| v.checked_add((fmt[i] - b'0') as usize))
                .ok_or(FormatLimitKind::Precision { limit_bytes: rem })?;
            if p > rem {
                return Err(FormatLimitKind::Precision { limit_bytes: rem });
            }
            i += 1;
        }
        precision = Some(p);
    }
    let _ = has_width;
    if i < fmt.len() && fmt[i] == b'(' {
        let inner_start = i + 1;
        if let Some(close_rel) = fmt[inner_start..].iter().position(|&b| b == b')') {
            let close = inner_start + close_rel;
            if close + 1 < fmt.len() && fmt[close + 1] == b'T' {
                return Ok(Some((
                    close + 2,
                    width,
                    left_align,
                    precision,
                    &fmt[inner_start..close],
                )));
            }
        }
    }
    Ok(None)
}

pub fn format_printf(
    format: &[u8],
    args: &[&[u8]],
    opts: &PrintfOpts<'_>,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<PrintfOutput, FormatLimitKind> {
    let esc_fmt = process_format_escapes(format, opts.max_output_bytes, b, work)?;
    let fmt = esc_fmt.as_slice();

    let mut out = KVec::new_in(b);
    let mut stderr = KString::new_in(b);
    let mut had_error = false;
    let mut stopped = false;
    let mut arg_pos = 0usize;

    loop {
        let mut args_consumed = 0usize;
        let mut i = 0usize;
        let mut steps = 0usize;

        while i < fmt.len() {
            steps += 1;
            if steps % 64 == 0 && work(1).is_err() {
                return Err(FormatLimitKind::OutputSize { limit_bytes: opts.max_output_bytes });
            }

            if fmt[i] != b'%' {
                let rem = opts.max_output_bytes.saturating_sub(out.len());
                if rem == 0 {
                    return Err(FormatLimitKind::OutputSize { limit_bytes: opts.max_output_bytes });
                }
                let (_, len) = next_utf8_recovery(&fmt[i..]);
                if len > rem {
                    return Err(FormatLimitKind::OutputSize { limit_bytes: opts.max_output_bytes });
                }
                out.try_extend_from_slice(&fmt[i..i + len])?;
                i += len;
                continue;
            }

            if i + 1 >= fmt.len() {
                let rem = opts.max_output_bytes.saturating_sub(out.len());
                if rem == 0 {
                    return Err(FormatLimitKind::OutputSize { limit_bytes: opts.max_output_bytes });
                }
                out.try_push(b'%')?;
                i += 1;
                continue;
            }

            if fmt[i + 1] == b'%' {
                let rem = opts.max_output_bytes.saturating_sub(out.len());
                if rem == 0 {
                    return Err(FormatLimitKind::OutputSize { limit_bytes: opts.max_output_bytes });
                }
                out.try_push(b'%')?;
                i += 2;
                continue;
            }

            let rem = opts.max_output_bytes.saturating_sub(out.len());

            if let Some((next_i, width, left_align, precision, tfmt)) =
                try_parse_strftime_directive(fmt, i + 1, rem)?
            {
                let arg = args.get(arg_pos).copied().unwrap_or(b"");
                arg_pos += 1;
                args_consumed += 1;

                let ts = if arg.is_empty() || arg == b"-1" {
                    opts.now_sec
                } else if arg == b"-2" {
                    opts.start_sec
                } else {
                    parse_leading_i64(arg)
                };
                let formatted = format_strftime(tfmt, ts, opts.tz, rem, rem, b, work)?;
                let truncated = if let Some(p) = precision {
                    truncate_utf16(formatted.as_slice(), p, b)?
                } else {
                    formatted
                };
                push_padded_utf16(&mut out, truncated.as_slice(), width, left_align, rem)?;
                i = next_i;
                continue;
            }

            // General specifier: %[+-0 #']*[width|*][.[precision|*]][hlL]<spec>
            let mut j = i + 1;
            let mut flag_plus = false;
            let mut flag_minus = false;
            let mut flag_zero = false;
            let mut flag_space = false;
            let mut flag_hash = false;
            while j < fmt.len() {
                match fmt[j] {
                    b'+' => flag_plus = true,
                    b'-' => flag_minus = true,
                    b'0' => flag_zero = true,
                    b' ' => flag_space = true,
                    b'#' => flag_hash = true,
                    b'\'' => {}
                    _ => break,
                }
                j += 1;
            }

            let mut width = 0usize;
            if j < fmt.len() && fmt[j] == b'*' {
                j += 1;
                let w_arg = args.get(arg_pos).copied().unwrap_or(b"");
                arg_pos += 1;
                args_consumed += 1;
                let w = parse_leading_int_f64(w_arg);
                let abs_w = w.abs();
                if abs_w > (rem as f64) || abs_w > MAX_SAFE_INT_F64 {
                    return Err(FormatLimitKind::Width { limit_bytes: rem });
                }
                if w < 0.0 {
                    flag_minus = true;
                }
                width = abs_w as usize;
            } else {
                while j < fmt.len() && fmt[j].is_ascii_digit() {
                    width = width
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((fmt[j] - b'0') as usize))
                        .ok_or(FormatLimitKind::Width { limit_bytes: rem })?;
                    if width > rem {
                        return Err(FormatLimitKind::Width { limit_bytes: rem });
                    }
                    j += 1;
                }
            }

            let mut precision: Option<usize> = None;
            if j < fmt.len() && fmt[j] == b'.' {
                j += 1;
                if j < fmt.len() && fmt[j] == b'*' {
                    j += 1;
                    let p_arg = args.get(arg_pos).copied().unwrap_or(b"");
                    arg_pos += 1;
                    args_consumed += 1;
                    let p = parse_leading_int_f64(p_arg);
                    if p >= 0.0 {
                        if p > (rem as f64) || p > MAX_SAFE_INT_F64 {
                            return Err(FormatLimitKind::Precision { limit_bytes: rem });
                        }
                        precision = Some(p as usize);
                    } else if p < -MAX_SAFE_INT_F64 {
                        return Err(FormatLimitKind::Precision { limit_bytes: rem });
                    } else {
                        // Owner decision (22/11): negative .* precision is omitted (upstream-defect).
                        precision = None;
                    }
                } else {
                    let mut p = 0usize;
                    while j < fmt.len() && fmt[j].is_ascii_digit() {
                        p = p
                            .checked_mul(10)
                            .and_then(|v| v.checked_add((fmt[j] - b'0') as usize))
                            .ok_or(FormatLimitKind::Precision { limit_bytes: rem })?;
                        if p > rem {
                            return Err(FormatLimitKind::Precision { limit_bytes: rem });
                        }
                        j += 1;
                    }
                    precision = Some(p);
                }
            }

            while j < fmt.len() && matches!(fmt[j], b'h' | b'l' | b'L') {
                j += 1;
            }

            if j >= fmt.len() {
                had_error = true;
                stderr.clear();
                stderr.try_push_str("printf: [sprintf] unexpected placeholder\n")?;
                out.clear();
                return Ok(PrintfOutput { bytes: out, had_error, stderr, stopped: false });
            }

            let spec = fmt[j];
            i = j + 1;

            if !matches!(
                spec,
                b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'f' | b'F' | b'e' | b'E' | b'g'
                    | b'G' | b'c' | b's' | b'q' | b'b'
            ) || (spec == b'q' && precision.is_some())
            {
                had_error = true;
                stderr.clear();
                stderr.try_push_str("printf: [sprintf] unexpected placeholder\n")?;
                out.clear();
                return Ok(PrintfOutput { bytes: out, had_error, stderr, stopped: false });
            }

            let arg = args.get(arg_pos).copied().unwrap_or(b"");
            arg_pos += 1;
            args_consumed += 1;

            let zero_eff = flag_zero && !flag_minus && precision.is_none();

            match spec {
                b'd' | b'i' => {
                    let parsed = parse_printf_int(arg);
                    if parsed.invalid {
                        had_error = true;
                        let arg_text = decode_utf8_recovery(arg, b)?;
                        let s_arg = core::str::from_utf8(arg_text.as_slice()).unwrap_or("");
                        if write!(stderr, "printf: {s_arg}: invalid number\n").is_err() {
                            if let Some(e) = stderr.take_fmt_error() {
                                return Err(FormatLimitKind::Mem(e));
                            }
                        }
                    }
                    let val = if parsed.value == 0.0 { 0.0 } else { parsed.value };
                    let prefix: &[u8] = if val < 0.0 {
                        b"-"
                    } else if flag_plus {
                        b"+"
                    } else if flag_space {
                        b" "
                    } else {
                        b""
                    };
                    let abs_str = f64_to_js_string(val.abs(), b)?;
                    push_padded_numeric(
                        &mut out,
                        prefix,
                        precision.unwrap_or(0),
                        abs_str.as_bytes(),
                        width,
                        flag_minus,
                        zero_eff,
                        rem,
                    )?;
                }
                b'u' => {
                    let parsed = parse_printf_int(arg);
                    if parsed.invalid {
                        had_error = true;
                        let arg_text = decode_utf8_recovery(arg, b)?;
                        let s_arg = core::str::from_utf8(arg_text.as_slice()).unwrap_or("");
                        if write!(stderr, "printf: {s_arg}: invalid number\n").is_err() {
                            if let Some(e) = stderr.take_fmt_error() {
                                return Err(FormatLimitKind::Mem(e));
                            }
                        }
                    }
                    let val = if parsed.value < 0.0 {
                        to_uint32_f64(parsed.value)
                    } else if parsed.value == 0.0 {
                        0.0
                    } else {
                        parsed.value
                    };
                    let prefix: &[u8] = if flag_plus {
                        b"+"
                    } else if flag_space {
                        b" "
                    } else {
                        b""
                    };
                    let abs_str = f64_to_js_string(val, b)?;
                    push_padded_numeric(
                        &mut out,
                        prefix,
                        precision.unwrap_or(0),
                        abs_str.as_bytes(),
                        width,
                        flag_minus,
                        zero_eff,
                        rem,
                    )?;
                }
                b'o' | b'x' | b'X' => {
                    let parsed = parse_printf_int(arg);
                    if parsed.invalid {
                        had_error = true;
                        let arg_text = decode_utf8_recovery(arg, b)?;
                        let s_arg = core::str::from_utf8(arg_text.as_slice()).unwrap_or("");
                        if write!(stderr, "printf: {s_arg}: invalid number\n").is_err() {
                            if let Some(e) = stderr.take_fmt_error() {
                                return Err(FormatLimitKind::Mem(e));
                            }
                        }
                    }
                    let mut digits = KString::new_in(b);
                    let radix = if spec == b'o' { 8 } else { 16 };
                    fmt_f64_abs_radix(parsed.value, radix, spec == b'X', &mut digits)?;
                    let prec = precision.unwrap_or(0);
                    if spec == b'o' {
                        let need_hash_zero = flag_hash
                            && !digits.as_bytes().starts_with(b"0")
                            && prec <= digits.len();
                        let prefix: &[u8] = if need_hash_zero { b"0" } else { b"" };
                        push_padded_numeric(
                            &mut out,
                            prefix,
                            prec,
                            digits.as_bytes(),
                            width,
                            flag_minus,
                            zero_eff,
                            rem,
                        )?;
                    } else {
                        let val_is_zero = parsed.value == 0.0;
                        let prefix: &[u8] = if flag_hash && !val_is_zero {
                            if spec == b'X' { b"0X" } else { b"0x" }
                        } else {
                            b""
                        };
                        push_padded_numeric(
                            &mut out,
                            prefix,
                            prec,
                            digits.as_bytes(),
                            width,
                            flag_minus,
                            zero_eff,
                            rem,
                        )?;
                    }
                }
                b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                    let raw_val = parse_f64_prefix(arg).unwrap_or(0.0);
                    let val = if raw_val.is_nan() || raw_val == 0.0 {
                        0.0
                    } else {
                        raw_val
                    };
                    let prec = precision.unwrap_or(6);
                    let body = match spec {
                        b'f' | b'F' => fmt_f64_fixed(val.abs(), prec, b),
                        b'e' | b'E' => fmt_f64_exp(val.abs(), prec, spec == b'E', true, b),
                        _ => fmt_f64_prec(
                            val.abs(),
                            prec.max(1),
                            spec == b'G',
                            flag_hash,
                            true,
                            b,
                        ),
                    };
                    if let Err(FloatFmtError::PrecisionRange) = body {
                        had_error = true;
                        stderr.clear();
                        let msg = match spec {
                            b'f' | b'F' => "printf: toFixed() digits argument must be between 0 and 100\n",
                            b'e' | b'E' => "printf: toExponential() argument must be between 0 and 100\n",
                            _ => "printf: toPrecision() argument must be between 1 and 100\n",
                        };
                        stderr.try_push_str(msg)?;
                        out.clear();
                        return Ok(PrintfOutput { bytes: out, had_error, stderr, stopped: false });
                    }
                    let mut body = match body {
                        Ok(s) => s,
                        Err(FloatFmtError::Mem(e)) => return Err(FormatLimitKind::Mem(e)),
                        Err(FloatFmtError::PrecisionRange) => unreachable!(),
                    };
                    if matches!(spec, b'f' | b'F') && flag_hash && prec == 0 && val.is_finite() {
                        body.try_push('.')?;
                    }
                    let prefix: &[u8] = if val < 0.0 {
                        b"-"
                    } else if flag_plus {
                        b"+"
                    } else if flag_space {
                        b" "
                    } else {
                        b""
                    };
                    let zero_pad = flag_zero && !flag_minus && val.is_finite();
                    push_padded_numeric(
                        &mut out,
                        prefix,
                        0,
                        body.as_bytes(),
                        width,
                        flag_minus,
                        zero_pad,
                        rem,
                    )?;
                }
                b'c' => {
                    if arg.is_empty() {
                        push_padded_utf16(&mut out, b"", width, flag_minus, rem)?;
                    } else {
                        let ch = char::from(arg[0]);
                        let mut buf = [0u8; 4];
                        let s = ch.encode_utf8(&mut buf).as_bytes();
                        push_padded_utf16(&mut out, s, width, flag_minus, rem)?;
                    }
                }
                b's' => {
                    let decoded = decode_utf8_recovery(arg, b)?;
                    let truncated = if let Some(p) = precision {
                        truncate_utf16(decoded.as_slice(), p, b)?
                    } else {
                        decoded
                    };
                    push_padded_utf16(&mut out, truncated.as_slice(), width, flag_minus, rem)?;
                }
                b'q' => {
                    let quoted = shell_quote_printf(arg, rem, b, work)?;
                    push_padded_utf16(&mut out, quoted.as_slice(), width, flag_minus, rem)?;
                }
                b'b' => {
                    let b_out = process_b_escapes(arg, rem, b, work)?;
                    if out.len().saturating_add(b_out.bytes.len()) > opts.max_output_bytes {
                        return Err(FormatLimitKind::OutputSize { limit_bytes: opts.max_output_bytes });
                    }
                    out.try_extend_from_slice(b_out.bytes.as_slice())?;
                    if b_out.stopped {
                        stopped = true;
                        break;
                    }
                }
                _ => unreachable!(),
            }
        }

        if stopped || arg_pos >= args.len() || args_consumed == 0 {
            break;
        }
    }

    Ok(PrintfOutput {
        bytes: out,
        had_error,
        stderr,
        stopped,
    })
}

fn awk_num(arg: Option<&AwkArg<'_>>) -> f64 {
    match arg {
        Some(AwkArg::Num(n)) => {
            if n.is_nan() || *n == 0.0 {
                0.0
            } else {
                *n
            }
        }
        Some(AwkArg::Str(s)) => {
            let v = parse_f64_strict(s).unwrap_or(0.0);
            if v.is_nan() || v == 0.0 { 0.0 } else { v }
        }
        None => 0.0,
    }
}

fn awk_str(arg: Option<&AwkArg<'_>>, b: &Budget) -> Result<KVec<u8>, FormatLimitKind> {
    match arg {
        Some(AwkArg::Str(s)) => decode_utf8_recovery(s, b),
        Some(AwkArg::Num(n)) => {
            let s = f64_to_js_string(*n, b)?;
            Ok(s.into_bytes())
        }
        None => Ok(KVec::new_in(b)),
    }
}

fn strip_awk_g_zeros(s: &str, b: &Budget) -> Result<KString, FormatLimitKind> {
    let bytes = s.as_bytes();
    let e_pos = bytes.iter().position(|&c| c == b'e' || c == b'E');
    let mant = match e_pos {
        Some(p) => &bytes[..p],
        None => bytes,
    };
    let exp_part = match e_pos {
        Some(p) => &bytes[p..],
        None => b"",
    };
    let kept_mant = if mant.contains(&b'.') {
        let mut end = mant.len();
        while end > 0 && mant[end - 1] == b'0' {
            end -= 1;
        }
        if end > 0 && mant[end - 1] == b'.' {
            end -= 1;
        }
        &mant[..end]
    } else {
        mant
    };
    let mut out = KString::new_in(b);
    out.try_push_str(core::str::from_utf8(kept_mant).unwrap_or(""))?;
    out.try_push_str(core::str::from_utf8(exp_part).unwrap_or(""))?;
    Ok(out)
}

pub fn format_awk_printf(
    format: &[u8],
    args: &[AwkArg<'_>],
    max_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, FormatLimitKind> {
    let cap = 10_000usize.min(max_bytes);
    let input = decode_utf8_recovery(format, b)?;
    let fmt = input.as_slice();
    let mut out = KVec::new_in(b);
    let mut value_idx = 0usize;
    let mut i = 0usize;
    let mut steps = 0usize;

    let push_bytes = |out: &mut KVec<u8>, slice: &[u8]| -> Result<(), FormatLimitKind> {
        if out.len().saturating_add(slice.len()) > max_bytes {
            return Err(FormatLimitKind::FormattedValueSize { limit_bytes: max_bytes });
        }
        out.try_extend_from_slice(slice)?;
        Ok(())
    };

    while i < fmt.len() {
        steps += 1;
        if steps % 64 == 0 && work(1).is_err() {
            return Err(FormatLimitKind::FormattedValueSize { limit_bytes: max_bytes });
        }

        if fmt[i] == b'\\' {
            if i + 1 < fmt.len() {
                let ch = match fmt[i + 1] {
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'r' => b'\r',
                    b'\\' => b'\\',
                    other => other,
                };
                push_bytes(&mut out, &[ch])?;
                i += 2;
            } else {
                push_bytes(&mut out, b"\\")?;
                i += 1;
            }
            continue;
        }

        if fmt[i] != b'%' {
            let (_, len) = next_utf8_recovery(&fmt[i..]);
            push_bytes(&mut out, &fmt[i..i + len])?;
            i += len;
            continue;
        }

        if i + 1 >= fmt.len() {
            push_bytes(&mut out, b"%")?;
            i += 1;
            continue;
        }
        if fmt[i + 1] == b'%' {
            push_bytes(&mut out, b"%")?;
            i += 2;
            continue;
        }

        let dir_start = i;
        let mut j = i + 1;

        // Optional `%n$` 1-based positional index
        let mut explicit_idx: Option<usize> = None;
        let d_start = j;
        while j < fmt.len() && fmt[j].is_ascii_digit() {
            j += 1;
        }
        if j > d_start && j < fmt.len() && fmt[j] == b'$' {
            let mut pos_num = 0usize;
            for &dig in &fmt[d_start..j] {
                pos_num = pos_num.saturating_mul(10).saturating_add((dig - b'0') as usize);
            }
            explicit_idx = Some(pos_num.saturating_sub(1));
            j += 1;
        } else {
            j = d_start;
        }

        let mut flag_plus = false;
        let mut flag_minus = false;
        let mut flag_zero = false;
        let mut flag_space = false;
        while j < fmt.len() {
            match fmt[j] {
                b'+' => flag_plus = true,
                b'-' => flag_minus = true,
                b'0' => flag_zero = true,
                b' ' => flag_space = true,
                b'#' => {}
                _ => break,
            }
            j += 1;
        }

        let mut width = 0usize;
        if j < fmt.len() && fmt[j] == b'*' {
            j += 1;
            let w_val = f64_floor(awk_num(args.get(value_idx)));
            value_idx += 1;
            let abs_w = w_val.abs();
            if abs_w > (cap as f64) {
                return Err(FormatLimitKind::Width { limit_bytes: cap });
            }
            if w_val < 0.0 {
                flag_minus = true;
            }
            width = abs_w as usize;
        } else {
            while j < fmt.len() && fmt[j].is_ascii_digit() {
                width = width
                    .checked_mul(10)
                    .and_then(|v| v.checked_add((fmt[j] - b'0') as usize))
                    .ok_or(FormatLimitKind::Width { limit_bytes: cap })?;
                if width > cap {
                    return Err(FormatLimitKind::Width { limit_bytes: cap });
                }
                j += 1;
            }
        }

        let mut precision: Option<usize> = None;
        if j < fmt.len() && fmt[j] == b'.' {
            j += 1;
            if j < fmt.len() && fmt[j] == b'*' {
                j += 1;
                let p_val = f64_floor(awk_num(args.get(value_idx)));
                value_idx += 1;
                if p_val >= 0.0 {
                    if p_val > (cap as f64) {
                        return Err(FormatLimitKind::Precision { limit_bytes: cap });
                    }
                    precision = Some(p_val as usize);
                }
            } else {
                let p_start = j;
                let mut p = 0usize;
                while j < fmt.len() && fmt[j].is_ascii_digit() {
                    p = p
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((fmt[j] - b'0') as usize))
                        .ok_or(FormatLimitKind::Precision { limit_bytes: cap })?;
                    if p > cap {
                        return Err(FormatLimitKind::Precision { limit_bytes: cap });
                    }
                    j += 1;
                }
                if j > p_start {
                    precision = Some(p);
                }
            }
        }

        // Length modifiers: hh, ll, l, z, j, h
        while j < fmt.len() && matches!(fmt[j], b'h' | b'l' | b'z' | b'j') {
            j += 1;
        }

        if j >= fmt.len() {
            push_bytes(&mut out, &fmt[dir_start..])?;
            break;
        }

        let spec = fmt[j];
        let dir_end = j + 1;

        if !matches!(
            spec,
            b's' | b'd' | b'i' | b'o' | b'x' | b'X' | b'f' | b'e' | b'E' | b'g' | b'G' | b'c'
        ) {
            push_bytes(&mut out, &fmt[dir_start..dir_end])?;
            i = dir_end;
            continue;
        }

        i = dir_end;
        let arg = if let Some(idx) = explicit_idx {
            args.get(idx)
        } else {
            let a = args.get(value_idx);
            value_idx += 1;
            a
        };

        let rem = max_bytes.saturating_sub(out.len());
        let zero_eff = flag_zero && !flag_minus && precision.is_none();

        match spec {
            b's' => {
                let s = awk_str(arg, b)?;
                let truncated = if let Some(p) = precision {
                    truncate_utf16(s.as_slice(), p, b)?
                } else {
                    s
                };
                if truncated.len().saturating_add(width.saturating_sub(utf16_units_len(truncated.as_slice()))) > rem {
                    return Err(FormatLimitKind::FormattedValueSize { limit_bytes: max_bytes });
                }
                push_padded_utf16(&mut out, truncated.as_slice(), width, flag_minus, rem)?;
            }
            b'c' => {
                let mut tmp = KVec::new_in(b);
                match arg {
                    Some(AwkArg::Num(n)) => {
                        let unit = ((f64_trunc(*n) as i64 as u32) & 0xffff) as u32;
                        let ch = char::from_u32(unit).unwrap_or('\u{FFFD}');
                        let mut buf = [0u8; 4];
                        tmp.try_extend_from_slice(ch.encode_utf8(&mut buf).as_bytes())?;
                    }
                    Some(AwkArg::Str(s)) => {
                        let decoded = decode_utf8_recovery(s, b)?;
                        if !decoded.is_empty() {
                            let (cp, _) = next_utf8_recovery(decoded.as_slice());
                            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            tmp.try_extend_from_slice(ch.encode_utf8(&mut buf).as_bytes())?;
                        }
                    }
                    None => {}
                }
                let pad = width.saturating_sub(utf16_units_len(tmp.as_slice()));
                if tmp.len().saturating_add(pad) > rem {
                    return Err(FormatLimitKind::FormattedValueSize { limit_bytes: max_bytes });
                }
                push_padded_utf16(&mut out, tmp.as_slice(), width, flag_minus, rem)?;
            }
            b'd' | b'i' => {
                let num = f64_floor(awk_num(arg));
                let val = if num == 0.0 { 0.0 } else { num };
                let prefix: &[u8] = if val < 0.0 {
                    b"-"
                } else if flag_plus {
                    b"+"
                } else if flag_space {
                    b" "
                } else {
                    b""
                };
                let abs_str = f64_to_js_string(val.abs(), b)?;
                push_padded_numeric(
                    &mut out,
                    prefix,
                    precision.unwrap_or(0),
                    abs_str.as_bytes(),
                    width,
                    flag_minus,
                    zero_eff,
                    rem,
                ).map_err(|e| match e {
                    FormatLimitKind::FormattedValueSize { .. } => {
                        FormatLimitKind::FormattedValueSize { limit_bytes: max_bytes }
                    }
                    other => other,
                })?;
            }
            b'o' | b'x' | b'X' => {
                let num = f64_floor(awk_num(arg));
                let val = if num == 0.0 { 0.0 } else { num };
                let prefix: &[u8] = if val < 0.0 { b"-" } else { b"" };
                let mut digits = KString::new_in(b);
                let radix = if spec == b'o' { 8 } else { 16 };
                fmt_f64_abs_radix(val, radix, spec == b'X', &mut digits)?;
                push_padded_numeric(
                    &mut out,
                    prefix,
                    precision.unwrap_or(0),
                    digits.as_bytes(),
                    width,
                    flag_minus,
                    zero_eff,
                    rem,
                ).map_err(|e| match e {
                    FormatLimitKind::FormattedValueSize { .. } => {
                        FormatLimitKind::FormattedValueSize { limit_bytes: max_bytes }
                    }
                    other => other,
                })?;
            }
            b'f' | b'e' | b'E' | b'g' | b'G' => {
                let prec = precision.unwrap_or(6);
                if prec > 100 {
                    return Err(FormatLimitKind::AwkFloatPrecision);
                }
                let raw_val = awk_num(arg);
                let val = if raw_val.is_nan() || raw_val == 0.0 {
                    0.0
                } else {
                    raw_val
                };
                let prefix: &[u8] = if val < 0.0 { b"-" } else { b"" };
                let body = match spec {
                    b'f' => fmt_f64_fixed(val.abs(), prec, b).map_err(|e| match e {
                        FloatFmtError::PrecisionRange => FormatLimitKind::AwkFloatPrecision,
                        FloatFmtError::Mem(m) => FormatLimitKind::Mem(m),
                    })?,
                    b'e' | b'E' => fmt_f64_exp(val.abs(), prec, spec == b'E', false, b).map_err(|e| match e {
                        FloatFmtError::PrecisionRange => FormatLimitKind::AwkFloatPrecision,
                        FloatFmtError::Mem(m) => FormatLimitKind::Mem(m),
                    })?,
                    _ => {
                        if val == 0.0 {
                            KString::try_from_str_in("0", b)?
                        } else {
                            let p = prec.max(1);
                            let exp = exact_decimal_exp(val.abs());
                            let raw_s = if exp < -4 || exp >= p as i32 {
                                fmt_f64_exp(val.abs(), p - 1, spec == b'G', false, b)
                            } else {
                                fmt_f64_prec(val.abs(), p, spec == b'G', true, false, b)
                            }
                            .map_err(|e| match e {
                                FloatFmtError::PrecisionRange => FormatLimitKind::AwkFloatPrecision,
                                FloatFmtError::Mem(m) => FormatLimitKind::Mem(m),
                            })?;
                            strip_awk_g_zeros(raw_s.as_str(), b)?
                        }
                    }
                };
                push_padded_numeric(
                    &mut out,
                    prefix,
                    0,
                    body.as_bytes(),
                    width,
                    flag_minus,
                    false,
                    rem,
                ).map_err(|e| match e {
                    FormatLimitKind::FormattedValueSize { .. } => {
                        FormatLimitKind::FormattedValueSize { limit_bytes: max_bytes }
                    }
                    other => other,
                })?;
            }
            _ => unreachable!(),
        }
    }

    Ok(out)
}
