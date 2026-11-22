//! ECMA-262 number formatting (`fmt_f64_js`, `toFixed`, `toExponential`, `toPrecision`)
//! and consumer-specific number parsers (`parse_arith_literal`, `parse_printf_int`,
//! `parse_f64_prefix`, `parse_f64_strict`).

mod bigint;

use crate::mem::{Budget, KString, MemError};
use bigint::BigUint;
use core::fmt::Write;

/// `Number.MAX_SAFE_INTEGER` (`2^53 - 1`), the literal ceiling in `just-bash@3.6.0` arithmetic (EVOL D1).
pub const MAX_SAFE_INT: u64 = 9_007_199_254_740_991;
pub const MAX_SAFE_INT_F64: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithNumError {
    /// Base outside `2..=64`, digit `>= base` (`08`, `2#102`, `16#g`), or malformed `#` literal.
    ValueTooGreatForBase,
    /// Empty or non-numeric token.
    InvalidNumber,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintfInt {
    pub value: f64,
    /// `true` when `printf` must emit `printf: <raw>: invalid number\n` and exit `1` while still formatting `value`.
    pub invalid: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FloatFmtError {
    /// Precision outside ECMA-262 bounds (`> 100` for `fmt_f64_fixed`/`fmt_f64_exp`, or `0` / `> 100` for `fmt_f64_prec`).
    PrecisionRange,
    Mem(MemError),
}

impl From<MemError> for FloatFmtError {
    fn from(e: MemError) -> Self {
        Self::Mem(e)
    }
}

struct StackBuf {
    bytes: [u8; 64],
    len: usize,
}

impl StackBuf {
    const fn new() -> Self {
        Self { bytes: [0; 64], len: 0 }
    }
    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for StackBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(core::fmt::Error)?;
        self.bytes
            .get_mut(self.len..end)
            .ok_or(core::fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn push_ascii_bytes(out: &mut KString, bytes: &[u8]) -> Result<(), MemError> {
    let s = core::str::from_utf8(bytes).expect("ASCII decimal output");
    out.try_push_str(s)
}

fn push_zeros(out: &mut KString, count: usize) -> Result<(), MemError> {
    const ZEROS: &str = "00000000000000000000000000000000";
    let mut rem = count;
    while rem > 0 {
        let take = rem.min(ZEROS.len());
        out.try_push_str(&ZEROS[..take])?;
        rem -= take;
    }
    Ok(())
}

fn push_exponent(out: &mut KString, exp: i32, upper: bool, two_digit_exp: bool) -> Result<(), MemError> {
    out.try_push(if upper { 'E' } else { 'e' })?;
    out.try_push(if exp >= 0 { '+' } else { '-' })?;
    let abs_exp = exp.unsigned_abs();
    let mut buf = StackBuf::new();
    if two_digit_exp && abs_exp < 10 {
        let _ = write!(buf, "0{abs_exp}");
    } else {
        let _ = write!(buf, "{abs_exp}");
    }
    push_ascii_bytes(out, buf.as_slice())
}

fn append_non_finite(val: f64, upper: bool, out: &mut KString) -> Result<bool, MemError> {
    if val.is_nan() {
        out.try_push_str(if upper { "NAN" } else { "NaN" })?;
        return Ok(true);
    }
    if val.is_infinite() {
        if val.is_sign_negative() {
            out.try_push('-')?;
        }
        out.try_push_str(if upper { "INFINITY" } else { "Infinity" })?;
        return Ok(true);
    }
    Ok(false)
}

/// Appends the exact ECMA-262 `Number::toString(val, 10)` representation to `out`.
pub fn fmt_f64_js(val: f64, out: &mut KString) -> Result<(), MemError> {
    if val.is_nan() {
        return out.try_push_str("NaN");
    }
    if val == 0.0 {
        return out.try_push('0');
    }
    if val == f64::INFINITY {
        return out.try_push_str("Infinity");
    }
    if val == f64::NEG_INFINITY {
        return out.try_push_str("-Infinity");
    }

    let abs = if val < 0.0 {
        out.try_push('-')?;
        -val
    } else {
        val
    };

    // Exact stack-BigUint Dragon4 (Burger-Dybvig) shortest round-trip with ECMA-262 even-tie breaking.
    let bits = abs.to_bits();
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (m, e) = if exp_bits == 0 {
        (frac, -1074)
    } else {
        ((1u64 << 52) | frac, exp_bits - 1075)
    };
    let even = (m & 1) == 0;
    let lower_boundary = exp_bits > 1 && frac == 0;

    let mut r = BigUint::from_u64(m);
    let mut s = BigUint::from_u64(1);
    let mut m_plus = BigUint::from_u64(1);
    let mut m_minus = BigUint::from_u64(1);

    if e >= 0 {
        if lower_boundary {
            r.shl((e as u32) + 2);
            s.shl(2);
            m_plus.shl((e as u32) + 1);
            m_minus.shl(e as u32);
        } else {
            r.shl((e as u32) + 1);
            s.shl(1);
            m_plus.shl(e as u32);
            m_minus.shl(e as u32);
        }
    } else if lower_boundary {
        r.shl(2);
        s.shl(((-e) as u32) + 2);
        m_plus.shl(1);
    } else {
        r.shl(1);
        s.shl(((-e) as u32) + 1);
    }

    // Estimate n such that x ~ 0.d1... * 10^n, i.e., x in [10^(n-1), 10^n).
    let bit_pos = e + (63 - m.leading_zeros() as i32);
    let mut n = (((bit_pos as i64 * 78_913) >> 18) as i32) + 1;
    if n >= 0 {
        s.mul_pow10(n as u32);
    } else {
        let p = (-n) as u32;
        r.mul_pow10(p);
        m_plus.mul_pow10(p);
        m_minus.mul_pow10(p);
    }

    // Scale so r + m_plus < s (or <= s depending on parity): guarantees 1 <= d1 <= 9 and no carry past d1.
    loop {
        let r_plus = r.add(&m_plus);
        let too_big = if even {
            r_plus.cmp(&s) != core::cmp::Ordering::Less
        } else {
            r_plus.cmp(&s) == core::cmp::Ordering::Greater
        };
        if too_big {
            s.mul_u32(10);
            n += 1;
            continue;
        }
        let mut r10_plus = r_plus;
        r10_plus.mul_u32(10);
        let too_small = if even {
            r10_plus.cmp(&s) == core::cmp::Ordering::Less
        } else {
            r10_plus.cmp(&s) != core::cmp::Ordering::Greater
        };
        if too_small {
            r.mul_u32(10);
            m_plus.mul_u32(10);
            m_minus.mul_u32(10);
            n -= 1;
            continue;
        }
        break;
    }

    let mut digits = [0u8; 24];
    let mut k = 0usize;
    loop {
        r.mul_u32(10);
        m_plus.mul_u32(10);
        m_minus.mul_u32(10);
        let q = r.div_digit(&s);

        let low_ok = if even {
            r.cmp(&m_minus) != core::cmp::Ordering::Greater
        } else {
            r.cmp(&m_minus) == core::cmp::Ordering::Less
        };
        let r_plus = r.add(&m_plus);
        let high_ok = if even {
            r_plus.cmp(&s) != core::cmp::Ordering::Less
        } else {
            r_plus.cmp(&s) == core::cmp::Ordering::Greater
        };

        let digit = match (low_ok, high_ok) {
            (false, false) => {
                digits[k] = b'0' + q;
                k += 1;
                continue;
            }
            (true, false) => q,
            (false, true) => q + 1,
            (true, true) => {
                let mut r2 = r;
                r2.shl(1);
                match r2.cmp(&s) {
                    core::cmp::Ordering::Less => q,
                    core::cmp::Ordering::Greater => q + 1,
                    core::cmp::Ordering::Equal => {
                        if q % 2 == 0 { q } else { q + 1 }
                    }
                }
            }
        };
        digits[k] = b'0' + digit;
        k += 1;
        break;
    }
    let d = &digits[..k];

    if (k as i32) <= n && n <= 21 {
        push_ascii_bytes(out, d)?;
        push_zeros(out, (n as usize) - k)?;
    } else if 0 < n && n <= 21 {
        let n_u = n as usize;
        push_ascii_bytes(out, &d[..n_u])?;
        out.try_push('.')?;
        push_ascii_bytes(out, &d[n_u..])?;
    } else if -5 <= n && n <= 0 {
        out.try_push_str("0.")?;
        push_zeros(out, (-n) as usize)?;
        push_ascii_bytes(out, d)?;
    } else {
        push_ascii_bytes(out, &d[..1])?;
        if k > 1 {
            out.try_push('.')?;
            push_ascii_bytes(out, &d[1..])?;
        }
        push_exponent(out, n - 1, false, false)?;
    }
    Ok(())
}

/// Convenience wrapper returning an owned `KString` in `b`.
pub fn f64_to_js_string(val: f64, b: &Budget) -> Result<KString, MemError> {
    let mut out = KString::new_in(b);
    fmt_f64_js(val, &mut out)?;
    Ok(out)
}

/// Decomposes positive finite `val` into `(m, e)` such that `val = m * 2^e` with `m > 0`.
fn decode_pos_f64(val: f64) -> (u64, i32) {
    let bits = val.to_bits();
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    if exp_bits == 0 {
        (frac, -1074)
    } else {
        ((1u64 << 52) | frac, exp_bits - 1075)
    }
}

/// Aligns positive finite `val = m * 2^e` to `(num, den, exp10)` such that `1 <= num/den < 10`
/// and `val = (num / den) * 10^exp10`.
fn dragon4_align(val: f64) -> (BigUint, BigUint, i32) {
    let (m, e) = decode_pos_f64(val);
    let bit_pos = e + (63 - m.leading_zeros() as i32);
    let mut exp10 = ((bit_pos as i64 * 78_913) >> 18) as i32;

    let mut num = BigUint::from_u64(m);
    let mut den = BigUint::from_u64(1);
    if e >= 0 {
        num.shl(e as u32);
    } else {
        den.shl((-e) as u32);
    }
    if exp10 >= 0 {
        den.mul_pow10(exp10 as u32);
    } else {
        num.mul_pow10((-exp10) as u32);
    }

    loop {
        let mut den10 = den.clone();
        den10.mul_u32(10);
        if num.cmp(&den10) != core::cmp::Ordering::Less {
            den = den10;
            exp10 += 1;
            continue;
        }
        if num.cmp(&den) == core::cmp::Ordering::Less {
            num.mul_u32(10);
            exp10 -= 1;
            continue;
        }
        break;
    }

    (num, den, exp10)
}

/// Computes the exact decimal exponent `e` with `10^e <= |val| < 10^(e + 1)` for non-zero finite `val`.
pub(crate) fn exact_decimal_exp(val: f64) -> i32 {
    let (_, _, exp10) = dragon4_align(val.abs());
    exp10
}

/// Extracts `count` (`1..=128`) exact decimal digits for positive finite `val` with
/// ECMA-262 ties-away-from-zero rounding, returning `(exp10_after_carry, digits_len)`.
fn extract_rounded_digits(val: f64, count: usize, digits: &mut [u8; 128]) -> (i32, usize) {
    let (mut num, den, mut exp10) = dragon4_align(val);
    for i in 0..count {
        digits[i] = b'0' + num.div_digit(&den);
        if i + 1 < count {
            num.mul_u32(10);
        }
    }
    // Round half away from zero: check `2 * rem >= den`.
    num.shl(1);
    if num.cmp(&den) != core::cmp::Ordering::Less {
        let mut idx = count;
        let mut carry = true;
        while idx > 0 && carry {
            idx -= 1;
            if digits[idx] == b'9' {
                digits[idx] = b'0';
            } else {
                digits[idx] += 1;
                carry = false;
            }
        }
        if carry {
            digits[0] = b'1';
            digits[1..count].fill(b'0');
            exp10 += 1;
        }
    }
    (exp10, count)
}

/// ECMA-262 `Number.prototype.toFixed(precision)` (`0 <= precision <= 100`).
pub fn fmt_f64_fixed(val: f64, precision: usize, b: &Budget) -> Result<KString, FloatFmtError> {
    if precision > 100 {
        return Err(FloatFmtError::PrecisionRange);
    }
    let mut out = KString::new_in(b);
    if append_non_finite(val, false, &mut out)? {
        return Ok(out);
    }
    if val == 0.0 {
        out.try_push('0')?;
        if precision > 0 {
            out.try_push('.')?;
            push_zeros(&mut out, precision)?;
        }
        return Ok(out);
    }
    let abs = if val < 0.0 {
        out.try_push('-')?;
        -val
    } else {
        val
    };
    if abs >= 1e21 {
        fmt_f64_js(abs, &mut out)?;
        return Ok(out);
    }

    let (num, den, exp10) = dragon4_align(abs);
    let total_sig = exp10 + 1 + precision as i32;
    if total_sig < 0 {
        out.try_push('0')?;
        if precision > 0 {
            out.try_push('.')?;
            push_zeros(&mut out, precision)?;
        }
        return Ok(out);
    }
    if total_sig == 0 {
        // First non-zero digit is at 10^-(precision + 1): round up iff num >= 5 * den (2 * num >= 10 * den).
        let mut den5 = den;
        den5.mul_u32(5);
        let round_up = num.cmp(&den5) != core::cmp::Ordering::Less;
        if precision == 0 {
            out.try_push(if round_up { '1' } else { '0' })?;
        } else {
            out.try_push_str("0.")?;
            push_zeros(&mut out, precision - 1)?;
            out.try_push(if round_up { '1' } else { '0' })?;
        }
        return Ok(out);
    }

    let count = total_sig as usize;
    let mut digits = [0u8; 128];
    let (exp_after, _) = extract_rounded_digits(abs, count, &mut digits);
    if exp_after > exp10 {
        if exp_after >= 0 {
            let int_len = (exp_after + 1) as usize;
            out.try_push('1')?;
            push_zeros(&mut out, int_len - 1)?;
            if precision > 0 {
                out.try_push('.')?;
                push_zeros(&mut out, precision)?;
            }
        } else {
            let lead_zeros = (-exp_after - 1) as usize;
            out.try_push_str("0.")?;
            push_zeros(&mut out, lead_zeros)?;
            out.try_push('1')?;
            push_zeros(&mut out, precision - lead_zeros - 1)?;
        }
        return Ok(out);
    }
    let int_digits = if exp_after >= 0 {
        (exp_after + 1) as usize
    } else {
        0
    };

    if int_digits == 0 {
        out.try_push('0')?;
        if precision > 0 {
            out.try_push('.')?;
            let lead_zeros = (-exp_after - 1) as usize;
            push_zeros(&mut out, lead_zeros)?;
            push_ascii_bytes(&mut out, &digits[..count])?;
        }
    } else {
        push_ascii_bytes(&mut out, &digits[..int_digits])?;
        if precision > 0 {
            out.try_push('.')?;
            push_ascii_bytes(&mut out, &digits[int_digits..count])?;
        }
    }
    Ok(out)
}

/// ECMA-262 `Number.prototype.toExponential(precision)` (`0 <= precision <= 100`).
pub fn fmt_f64_exp(
    val: f64,
    precision: usize,
    upper: bool,
    two_digit_exp: bool,
    b: &Budget,
) -> Result<KString, FloatFmtError> {
    let mut out = KString::new_in(b);
    if append_non_finite(val, upper, &mut out)? {
        return Ok(out);
    }
    if precision > 100 {
        return Err(FloatFmtError::PrecisionRange);
    }
    if val == 0.0 {
        out.try_push('0')?;
        if precision > 0 {
            out.try_push('.')?;
            push_zeros(&mut out, precision)?;
        }
        push_exponent(&mut out, 0, upper, two_digit_exp)?;
        return Ok(out);
    }
    let abs = if val < 0.0 {
        out.try_push('-')?;
        -val
    } else {
        val
    };
    let mut digits = [0u8; 128];
    let (exp10, count) = extract_rounded_digits(abs, precision + 1, &mut digits);
    out.try_push(digits[0] as char)?;
    if count > 1 {
        out.try_push('.')?;
        push_ascii_bytes(&mut out, &digits[1..count])?;
    }
    push_exponent(&mut out, exp10, upper, two_digit_exp)?;
    Ok(out)
}

/// ECMA-262 `Number.prototype.toPrecision(significant)` (`1 <= significant <= 100`).
pub fn fmt_f64_prec(
    val: f64,
    significant: usize,
    upper: bool,
    keep_zeros: bool,
    two_digit_exp: bool,
    b: &Budget,
) -> Result<KString, FloatFmtError> {
    let mut out = KString::new_in(b);
    if append_non_finite(val, upper, &mut out)? {
        return Ok(out);
    }
    if significant == 0 || significant > 100 {
        return Err(FloatFmtError::PrecisionRange);
    }
    if val == 0.0 {
        out.try_push('0')?;
        if keep_zeros && significant > 1 {
            out.try_push('.')?;
            push_zeros(&mut out, significant - 1)?;
        }
        return Ok(out);
    }
    let abs = if val < 0.0 {
        out.try_push('-')?;
        -val
    } else {
        val
    };

    let mut digits = [0u8; 128];
    let (exp10, count) = extract_rounded_digits(abs, significant, &mut digits);
    let sig_digits = &digits[..count];

    if exp10 < -6 || exp10 >= significant as i32 {
        // Exponential form
        let mut frac_len = count - 1;
        if !keep_zeros {
            while frac_len > 0 && sig_digits[frac_len] == b'0' {
                frac_len -= 1;
            }
        }
        out.try_push(sig_digits[0] as char)?;
        if frac_len > 0 {
            out.try_push('.')?;
            push_ascii_bytes(&mut out, &sig_digits[1..=frac_len])?;
        }
        push_exponent(&mut out, exp10, upper, two_digit_exp)?;
    } else if exp10 < 0 {
        // 0.000...digits
        let lead_zeros = (-exp10 - 1) as usize;
        let mut sig_len = count;
        if !keep_zeros {
            while sig_len > 0 && sig_digits[sig_len - 1] == b'0' {
                sig_len -= 1;
            }
        }
        if sig_len == 0 {
            out.try_push('0')?;
        } else {
            out.try_push_str("0.")?;
            push_zeros(&mut out, lead_zeros)?;
            push_ascii_bytes(&mut out, &sig_digits[..sig_len])?;
        }
    } else {
        let int_len = (exp10 + 1) as usize;
        if int_len == count {
            push_ascii_bytes(&mut out, sig_digits)?;
        } else {
            let mut frac_len = count - int_len;
            if !keep_zeros {
                while frac_len > 0 && sig_digits[int_len + frac_len - 1] == b'0' {
                    frac_len -= 1;
                }
            }
            push_ascii_bytes(&mut out, &sig_digits[..int_len])?;
            if frac_len > 0 {
                out.try_push('.')?;
                push_ascii_bytes(&mut out, &sig_digits[int_len..int_len + frac_len])?;
            }
        }
    }
    Ok(out)
}

/// `no_std` IEEE-754 truncation toward zero.
#[inline]
pub(crate) fn f64_trunc(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32 - 1023;
    if exp < 0 {
        return f64::from_bits(bits & (1u64 << 63));
    }
    if exp >= 52 {
        return x;
    }
    let frac_mask = (1u64 << (52 - exp)) - 1;
    f64::from_bits(bits & !frac_mask)
}

/// `no_std` IEEE-754 floor toward `-Infinity`.
#[inline]
pub(crate) fn f64_floor(x: f64) -> f64 {
    let t = f64_trunc(x);
    if x < 0.0 && t != x { t - 1.0 } else { t }
}

/// Formats `|val|` as an IEEE-754 integer double in base 8 or 16, matching JS `Math.abs(val).toString(radix)`.
/// Only integer part is meaningful (`printf`/`awk` pass integerized/floored values; handles `> u64::MAX` via exact binary shifts).
pub(crate) fn fmt_f64_abs_radix(val: f64, radix: u8, upper: bool, out: &mut KString) -> Result<(), MemError> {
    debug_assert!(radix == 8 || radix == 16);
    if val.is_nan() {
        return out.try_push_str(if upper { "NAN" } else { "NaN" });
    }
    if val.is_infinite() {
        return out.try_push_str(if upper { "INFINITY" } else { "Infinity" });
    }
    let abs = f64_trunc(val.abs());
    if abs == 0.0 {
        return out.try_push('0');
    }

    let (m, e) = decode_pos_f64(abs);
    if e <= 0 {
        let int_val = m >> ((-e) as u32);
        return push_u64_radix(int_val, radix, upper, out);
    }

    let shift_bits = e as usize;
    let group_bits = if radix == 8 { 3usize } else { 4usize };
    let rem_bits = shift_bits % group_bits;
    let trailing_zeros = shift_bits / group_bits;
    let lead = m << rem_bits;
    push_u64_radix(lead, radix, upper, out)?;
    push_zeros(out, trailing_zeros)
}

fn push_u64_radix(mut v: u64, radix: u8, upper: bool, out: &mut KString) -> Result<(), MemError> {
    if v == 0 {
        return out.try_push('0');
    }
    let alpha = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let mut buf = [0u8; 24];
    let mut i = buf.len();
    let r = radix as u64;
    while v > 0 {
        i -= 1;
        buf[i] = alpha[(v % r) as usize];
        v /= r;
    }
    push_ascii_bytes(out, &buf[i..])
}

// ----------------------------------------------------------------------------------------- Parsers

/// Parses an unsigned arithmetic literal token (`10`, `010`, `0x1f`, `64#@_`) per EVOL D1,
/// clamping values above `MAX_SAFE_INT` to `MAX_SAFE_INT_F64`.
pub fn parse_arith_literal(raw: &[u8]) -> Result<f64, ArithNumError> {
    if raw.is_empty() {
        return Err(ArithNumError::InvalidNumber);
    }
    if let Some(hash_pos) = raw.iter().position(|&b| b == b'#') {
        let base_bytes = &raw[..hash_pos];
        let digits = &raw[hash_pos + 1..];
        if base_bytes.is_empty() || digits.is_empty() || !base_bytes.iter().all(|b| b.is_ascii_digit()) {
            return Err(ArithNumError::ValueTooGreatForBase);
        }
        let mut base = 0u32;
        for &b in base_bytes {
            base = base.saturating_mul(10).saturating_add((b - b'0') as u32);
            if base > 64 {
                return Err(ArithNumError::ValueTooGreatForBase);
            }
        }
        if !(2..=64).contains(&base) {
            return Err(ArithNumError::ValueTooGreatForBase);
        }
        let mut acc = 0.0f64;
        for &ch in digits {
            let digit = match ch {
                b'0'..=b'9' => (ch - b'0') as u32,
                b'a'..=b'z' => (10 + ch - b'a') as u32,
                b'A'..=b'Z' => {
                    if base <= 36 {
                        (10 + ch - b'A') as u32
                    } else {
                        (36 + ch - b'A') as u32
                    }
                }
                b'@' => 62,
                b'_' => 63,
                _ => return Err(ArithNumError::ValueTooGreatForBase),
            };
            if digit >= base {
                return Err(ArithNumError::ValueTooGreatForBase);
            }
            acc = acc * (base as f64) + (digit as f64);
            if acc > MAX_SAFE_INT_F64 {
                return Ok(MAX_SAFE_INT_F64);
            }
        }
        return Ok(acc);
    }

    if raw.len() >= 2 && raw[0] == b'0' && (raw[1] == b'x' || raw[1] == b'X') {
        let rest = &raw[2..];
        if rest.is_empty() || !rest.iter().all(|b| b.is_ascii_hexdigit()) {
            return Err(ArithNumError::InvalidNumber);
        }
        let mut acc = 0.0f64;
        for &ch in rest {
            let d = hex_val(ch).unwrap() as f64;
            acc = acc * 16.0 + d;
            if acc > MAX_SAFE_INT_F64 {
                return Ok(MAX_SAFE_INT_F64);
            }
        }
        return Ok(acc);
    }

    if raw.len() > 1 && raw[0] == b'0' && raw.iter().all(|b| b.is_ascii_digit()) {
        if raw.iter().any(|&b| b == b'8' || b == b'9') {
            return Err(ArithNumError::ValueTooGreatForBase);
        }
        let mut acc = 0.0f64;
        for &ch in raw {
            let d = (ch - b'0') as f64;
            acc = acc * 8.0 + d;
            if acc > MAX_SAFE_INT_F64 {
                return Ok(MAX_SAFE_INT_F64);
            }
        }
        return Ok(acc);
    }

    if !raw.iter().all(|b| b.is_ascii_digit()) {
        return Err(ArithNumError::InvalidNumber);
    }
    let mut acc = 0.0f64;
    for &ch in raw {
        let d = (ch - b'0') as f64;
        acc = acc * 10.0 + d;
        if acc > MAX_SAFE_INT_F64 {
            return Ok(MAX_SAFE_INT_F64);
        }
    }
    Ok(acc)
}

#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + b - b'a'),
        b'A'..=b'F' => Some(10 + b - b'A'),
        _ => None,
    }
}

/// Decodes one UTF-8 scalar with Latin-1 byte recovery (`0x80..=0xFF` -> `U+00XX`),
/// returning `(scalar_u32, byte_len)`.
#[inline]
pub(crate) fn next_utf8_recovery(bytes: &[u8]) -> (u32, usize) {
    let b0 = bytes[0];
    if b0 < 0x80 {
        return (b0 as u32, 1);
    }
    if (0xc2..=0xdf).contains(&b0) && bytes.len() >= 2 {
        let b1 = bytes[1];
        if (b1 & 0xc0) == 0x80 {
            let cp = (((b0 & 0x1f) as u32) << 6) | ((b1 & 0x3f) as u32);
            return (cp, 2);
        }
    } else if (0xe0..=0xef).contains(&b0) && bytes.len() >= 3 {
        let b1 = bytes[1];
        let b2 = bytes[2];
        if (b1 & 0xc0) == 0x80
            && (b2 & 0xc0) == 0x80
            && !(b0 == 0xe0 && b1 < 0xa0)
            && !(b0 == 0xed && b1 >= 0xa0)
        {
            let cp = (((b0 & 0x0f) as u32) << 12) | (((b1 & 0x3f) as u32) << 6) | ((b2 & 0x3f) as u32);
            return (cp, 3);
        }
    } else if (0xf0..=0xf4).contains(&b0) && bytes.len() >= 4 {
        let b1 = bytes[1];
        let b2 = bytes[2];
        let b3 = bytes[3];
        if (b1 & 0xc0) == 0x80
            && (b2 & 0xc0) == 0x80
            && (b3 & 0xc0) == 0x80
            && !(b0 == 0xf0 && b1 < 0x90)
            && !(b0 == 0xf4 && b1 >= 0x90)
        {
            let cp = (((b0 & 0x07) as u32) << 18)
                | (((b1 & 0x3f) as u32) << 12)
                | (((b2 & 0x3f) as u32) << 6)
                | ((b3 & 0x3f) as u32);
            return (cp, 4);
        }
    }
    (b0 as u32, 1)
}

#[inline]
fn is_js_ws_cp(cp: u32) -> bool {
    matches!(
        cp,
        0x0009..=0x000d
            | 0x0020
            | 0x00a0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
            | 0xfeff
    )
}

/// Trims leading and trailing JS whitespace (in UTF-8 recovery space) from `raw` in O(N) with zero allocation.
/// Returns `(trimmed_slice, had_trailing_ws_after_non_ws)`.
fn trim_js_ws(raw: &[u8]) -> (&[u8], bool) {
    let mut start = 0usize;
    while start < raw.len() {
        let (cp, len) = next_utf8_recovery(&raw[start..]);
        if !is_js_ws_cp(cp) {
            break;
        }
        start += len;
    }
    let mut end = start;
    let mut pos = start;
    while pos < raw.len() {
        let (cp, len) = next_utf8_recovery(&raw[pos..]);
        pos += len;
        if !is_js_ws_cp(cp) {
            end = pos;
        }
    }
    (&raw[start..end], end < raw.len() && end > start)
}

#[inline]
fn first_utf16_unit(cp: u32) -> u16 {
    if cp <= 0xffff {
        cp as u16
    } else {
        (0xd800 + ((cp - 0x10000) >> 10)) as u16
    }
}

fn parse_dec_ascii_f64(digits: &[u8]) -> f64 {
    if let Ok(s) = core::str::from_utf8(digits) {
        if let Ok(v) = s.parse::<f64>() {
            return if v == 0.0 { 0.0 } else { v };
        }
    }
    0.0
}

fn parse_radix_digits_f64(digits: &[u8], radix_bits: u32) -> f64 {
    let mut acc = 0.0f64;
    let base = (1u32 << radix_bits) as f64;
    for &b in digits {
        let d = match radix_bits {
            1 => (b - b'0') as f64,
            3 => (b - b'0') as f64,
            4 => hex_val(b).unwrap_or(0) as f64,
            _ => 0.0,
        };
        acc = acc * base + d;
    }
    acc
}

/// Parses a `printf` integer argument (`%d`, `%i`, `%o`, `%u`, `%x`, `%X`), reproducing
/// character constants, whitespace rules, and partial-parse error recovery.
pub fn parse_printf_int(raw: &[u8]) -> PrintfInt {
    let (trimmed, has_trailing_ws) = trim_js_ws(raw);
    if trimmed.is_empty() {
        return PrintfInt { value: 0.0, invalid: false };
    }

    // Character constants: 'c, "c, \'c, \"c
    if trimmed[0] == b'\'' || trimmed[0] == b'"' {
        if trimmed.len() >= 2 {
            let (cp, _) = next_utf8_recovery(&trimmed[1..]);
            return PrintfInt { value: first_utf16_unit(cp) as f64, invalid: false };
        }
    } else if trimmed.len() >= 3 && trimmed[0] == b'\\' && (trimmed[1] == b'\'' || trimmed[1] == b'"') {
        let (cp, _) = next_utf8_recovery(&trimmed[2..]);
        return PrintfInt { value: first_utf16_unit(cp) as f64, invalid: false };
    }

    // Strip a single leading '+'
    let s = if trimmed[0] == b'+' { &trimmed[1..] } else { trimmed };
    if s.is_empty() {
        return PrintfInt { value: 0.0, invalid: true };
    }

    // Hexadecimal 0x / 0X
    if s.len() >= 2 && s[0] == b'0' && (s[1] == b'x' || s[1] == b'X') {
        let hex_end = 2 + s[2..].iter().take_while(|b| b.is_ascii_hexdigit()).count();
        if hex_end > 2 {
            let val = parse_radix_digits_f64(&s[2..hex_end], 4);
            return PrintfInt { value: if val == 0.0 { 0.0 } else { val }, invalid: has_trailing_ws };
        }
    }

    // Octal ^0[0-7]+$
    if s.len() > 1 && s[0] == b'0' && s[1..].iter().all(|b| matches!(b, b'0'..=b'7')) {
        let val = parse_radix_digits_f64(&s[1..], 3);
        return PrintfInt { value: if val == 0.0 { 0.0 } else { val }, invalid: has_trailing_ws };
    }

    // Arbitrary-base prefix ^\d+#
    let digit_run = s.iter().take_while(|b| b.is_ascii_digit()).count();
    if digit_run > 0 && s.get(digit_run) == Some(&b'#') {
        let val = parse_dec_ascii_f64(&s[..digit_run]);
        return PrintfInt { value: val, invalid: true };
    }

    // Exact decimal ^-?[0-9]+$
    let neg = s[0] == b'-';
    let dec_start = usize::from(neg);
    if dec_start < s.len() && s[dec_start..].iter().all(|b| b.is_ascii_digit()) {
        let val = parse_dec_ascii_f64(s);
        return PrintfInt { value: val, invalid: has_trailing_ws };
    }

    // Partial decimal fallback ^[+-]?[0-9]+
    let sign_len = usize::from(s[0] == b'+' || s[0] == b'-');
    let digits_len = s[sign_len..].iter().take_while(|b| b.is_ascii_digit()).count();
    let val = if digits_len > 0 {
        parse_dec_ascii_f64(&s[..sign_len + digits_len])
    } else {
        0.0
    };
    PrintfInt { value: val, invalid: true }
}

/// Scans the longest JS float prefix (optional sign, digits/dot, optional exponent) starting at `s[0]`.
fn scan_decimal_float_prefix(s: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    let int_digits = s[i..].iter().take_while(|b| b.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0usize;
    if i < s.len() && s[i] == b'.' {
        i += 1;
        frac_digits = s[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        i += frac_digits;
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let mut j = i + 1;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            j += 1;
        }
        let exp_digits = s[j..].iter().take_while(|b| b.is_ascii_digit()).count();
        if exp_digits > 0 {
            i = j + exp_digits;
        }
    }
    Some(i)
}

/// JavaScript `Number.parseFloat` prefix parser.
pub fn parse_f64_prefix(raw: &[u8]) -> Option<f64> {
    let mut start = 0usize;
    while start < raw.len() {
        let (cp, len) = next_utf8_recovery(&raw[start..]);
        if !is_js_ws_cp(cp) {
            break;
        }
        start += len;
    }
    let s = &raw[start..];
    if s.is_empty() {
        return None;
    }
    if s.starts_with(b"Infinity") || s.starts_with(b"+Infinity") {
        return Some(f64::INFINITY);
    }
    if s.starts_with(b"-Infinity") {
        return Some(f64::NEG_INFINITY);
    }
    let len = scan_decimal_float_prefix(s)?;
    let slice = core::str::from_utf8(&s[..len]).ok()?;
    slice.parse::<f64>().ok()
}

/// JavaScript `Number(s)` full-string parser.
pub fn parse_f64_strict(raw: &[u8]) -> Option<f64> {
    let (s, _) = trim_js_ws(raw);
    if s.is_empty() {
        return Some(0.0);
    }
    if s == b"Infinity" || s == b"+Infinity" {
        return Some(f64::INFINITY);
    }
    if s == b"-Infinity" {
        return Some(f64::NEG_INFINITY);
    }
    if s.len() > 2 && s[0] == b'0' {
        if (s[1] == b'x' || s[1] == b'X') && s[2..].iter().all(|b| b.is_ascii_hexdigit()) {
            return Some(parse_radix_digits_f64(&s[2..], 4));
        }
        if (s[1] == b'b' || s[1] == b'B') && s[2..].iter().all(|&b| b == b'0' || b == b'1') {
            return Some(parse_radix_digits_f64(&s[2..], 1));
        }
        if (s[1] == b'o' || s[1] == b'O') && s[2..].iter().all(|b| matches!(b, b'0'..=b'7')) {
            return Some(parse_radix_digits_f64(&s[2..], 3));
        }
    }
    let len = scan_decimal_float_prefix(s)?;
    if len != s.len() {
        return None;
    }
    let slice = core::str::from_utf8(s).ok()?;
    slice.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::LimitId;

    fn budget() -> Budget {
        Budget::root(LimitId(30), u64::MAX).unwrap()
    }

    #[test]
    fn arith_literals_and_bases() {
        assert_eq!(parse_arith_literal(b"0"), Ok(0.0));
        assert_eq!(parse_arith_literal(b"42"), Ok(42.0));
        assert_eq!(parse_arith_literal(b"010"), Ok(8.0));
        assert_eq!(parse_arith_literal(b"0x1f"), Ok(31.0));
        assert_eq!(parse_arith_literal(b"0XFF"), Ok(255.0));
        assert_eq!(parse_arith_literal(b"08"), Err(ArithNumError::ValueTooGreatForBase));
        assert_eq!(parse_arith_literal(b"2#1010"), Ok(10.0));
        assert_eq!(parse_arith_literal(b"2#102"), Err(ArithNumError::ValueTooGreatForBase));
        assert_eq!(parse_arith_literal(b"36#Z"), Ok(35.0));
        assert_eq!(parse_arith_literal(b"37#Z"), Err(ArithNumError::ValueTooGreatForBase));
        assert_eq!(parse_arith_literal(b"37#A"), Ok(36.0));
        assert_eq!(parse_arith_literal(b"62#Z"), Ok(61.0));
        assert_eq!(parse_arith_literal(b"64#@_"), Ok(4031.0));
        assert_eq!(parse_arith_literal(b"65#1"), Err(ArithNumError::ValueTooGreatForBase));
        assert_eq!(parse_arith_literal(b"9007199254740992"), Ok(MAX_SAFE_INT_F64));
        assert_eq!(parse_arith_literal(b"abc"), Err(ArithNumError::InvalidNumber));
    }

    #[test]
    fn printf_int_rules() {
        assert_eq!(parse_printf_int(b"+42"), PrintfInt { value: 42.0, invalid: false });
        assert_eq!(parse_printf_int(b"-42"), PrintfInt { value: -42.0, invalid: false });
        assert_eq!(parse_printf_int(b"0x10zz"), PrintfInt { value: 16.0, invalid: false });
        assert_eq!(parse_printf_int(b"-0x10"), PrintfInt { value: 0.0, invalid: true });
        assert_eq!(parse_printf_int(b"+010"), PrintfInt { value: 8.0, invalid: false });
        assert_eq!(parse_printf_int(b"-010"), PrintfInt { value: -10.0, invalid: false });
        assert_eq!(parse_printf_int(b"08"), PrintfInt { value: 8.0, invalid: false });
        assert_eq!(parse_printf_int(b"64#a"), PrintfInt { value: 64.0, invalid: true });
        assert_eq!(parse_printf_int(b"3abc"), PrintfInt { value: 3.0, invalid: true });
        assert_eq!(parse_printf_int(b"42 "), PrintfInt { value: 42.0, invalid: true });
        assert_eq!(parse_printf_int(b"   "), PrintfInt { value: 0.0, invalid: false });
        assert_eq!(parse_printf_int("'é".as_bytes()), PrintfInt { value: 233.0, invalid: false });
        assert_eq!(parse_printf_int(b"'"), PrintfInt { value: 0.0, invalid: true });
    }

    #[test]
    fn float_prefix_and_strict_parsers() {
        assert_eq!(parse_f64_prefix(b"  3.5abc"), Some(3.5));
        assert_eq!(parse_f64_prefix(b"0x10"), Some(0.0));
        assert_eq!(parse_f64_prefix(b"+Infinity!"), Some(f64::INFINITY));
        assert_eq!(parse_f64_prefix(b"infinity"), None);
        assert_eq!(parse_f64_prefix(b"1ex"), Some(1.0));

        assert_eq!(parse_f64_strict(b"  \t "), Some(0.0));
        assert_eq!(parse_f64_strict(b"0x10"), Some(16.0));
        assert_eq!(parse_f64_strict(b"0b1011"), Some(11.0));
        assert_eq!(parse_f64_strict(b"0o77"), Some(63.0));
        assert_eq!(parse_f64_strict(b"-0x10"), None);
        assert_eq!(parse_f64_strict(b"3.5abc"), None);
        assert_eq!(parse_f64_strict(b"  -1.25e2  "), Some(-125.0));
    }

    #[test]
    fn js_number_fixture_replay() {
        let raw = include_str!("../../../test/fixtures/l8b-js-numbers.json");
        let b = budget();
        let mut count = 0usize;
        for chunk in raw.split("\"bits\": \"").skip(1) {
            let bits_end = chunk.find('"').unwrap();
            let bits_hex = &chunk[2..bits_end];
            let bits = u64::from_str_radix(bits_hex, 16).unwrap();
            let val = f64::from_bits(bits);

            let extract = |key: &str| -> &str {
                let idx = chunk.find(key).unwrap() + key.len();
                let rest = &chunk[idx..];
                let end = rest.find('"').unwrap();
                &rest[..end]
            };

            let exp_js = extract("\"js\": \"");
            let exp_f0 = extract("\"fixed0\": \"");
            let exp_f6 = extract("\"fixed6\": \"");
            let exp_e6 = extract("\"exp6\": \"");
            let exp_p6 = extract("\"prec6\": \"");

            let got_js = f64_to_js_string(val, &b).unwrap();
            assert_eq!(got_js.as_str(), exp_js, "js mismatch for bits=0x{bits_hex}");

            let got_f0 = fmt_f64_fixed(val, 0, &b).unwrap();
            assert_eq!(got_f0.as_str(), exp_f0, "fixed0 mismatch for bits=0x{bits_hex}");

            let got_f6 = fmt_f64_fixed(val, 6, &b).unwrap();
            assert_eq!(got_f6.as_str(), exp_f6, "fixed6 mismatch for bits=0x{bits_hex}");

            let got_e6 = fmt_f64_exp(val, 6, false, false, &b).unwrap();
            assert_eq!(got_e6.as_str(), exp_e6, "exp6 mismatch for bits=0x{bits_hex}");

            let got_p6 = fmt_f64_prec(val, 6, false, true, false, &b).unwrap();
            assert_eq!(got_p6.as_str(), exp_p6, "prec6 mismatch for bits=0x{bits_hex}");

            count += 1;
        }
        assert!(count >= 600, "expected >= 600 vectors, got {count}");
    }

    #[test]
    fn float_formatting_edges() {
        let b = budget();
        assert_eq!(fmt_f64_fixed(-0.0, 2, &b).unwrap().as_str(), "0.00");
        assert_eq!(fmt_f64_fixed(0.5, 0, &b).unwrap().as_str(), "1");
        assert_eq!(fmt_f64_fixed(2.5, 0, &b).unwrap().as_str(), "3");
        assert_eq!(fmt_f64_fixed(1.15, 1, &b).unwrap().as_str(), "1.1");
        assert_eq!(fmt_f64_fixed(1.25, 1, &b).unwrap().as_str(), "1.3");
        assert_eq!(fmt_f64_fixed(f64::NAN, 101, &b).unwrap_err(), FloatFmtError::PrecisionRange);
        assert_eq!(fmt_f64_exp(f64::INFINITY, 101, true, true, &b).unwrap().as_str(), "INFINITY");
        assert_eq!(fmt_f64_prec(100_000.0, 6, false, false, true, &b).unwrap().as_str(), "100000");
        assert_eq!(fmt_f64_prec(0.0, 1, false, false, true, &b).unwrap().as_str(), "0");
        assert_eq!(fmt_f64_prec(1.23, 0, false, false, true, &b).unwrap_err(), FloatFmtError::PrecisionRange);
    }

    #[test]
    fn js_fmt_specials_and_cutoffs() {
        let b = budget();
        assert_eq!(f64_to_js_string(f64::NAN, &b).unwrap().as_str(), "NaN");
        assert_eq!(f64_to_js_string(-0.0, &b).unwrap().as_str(), "0");
        assert_eq!(f64_to_js_string(f64::INFINITY, &b).unwrap().as_str(), "Infinity");
        assert_eq!(f64_to_js_string(f64::NEG_INFINITY, &b).unwrap().as_str(), "-Infinity");
        assert_eq!(f64_to_js_string(1e-6, &b).unwrap().as_str(), "0.000001");
        assert_eq!(f64_to_js_string(1e-7, &b).unwrap().as_str(), "1e-7");
        assert_eq!(f64_to_js_string(1e20, &b).unwrap().as_str(), "100000000000000000000");
        assert_eq!(f64_to_js_string(1e21, &b).unwrap().as_str(), "1e+21");
        assert_eq!(f64_to_js_string(5e-324, &b).unwrap().as_str(), "5e-324");
    }

    #[test]
    fn fixed_round_up_across_decimal_boundaries() {
        let b = budget();
        assert_eq!(fmt_f64_fixed(0.099, 2, &b).unwrap().as_str(), "0.10");
        assert_eq!(fmt_f64_fixed(0.0099, 3, &b).unwrap().as_str(), "0.010");
        assert_eq!(fmt_f64_fixed(0.99, 1, &b).unwrap().as_str(), "1.0");
        assert_eq!(fmt_f64_fixed(9.99, 1, &b).unwrap().as_str(), "10.0");
        assert_eq!(fmt_f64_fixed(0.04, 0, &b).unwrap().as_str(), "0");
        assert_eq!(fmt_f64_fixed(-0.5, 0, &b).unwrap().as_str(), "-1");
        assert_eq!(fmt_f64_fixed(1e21, 2, &b).unwrap().as_str(), "1e+21");
    }

    #[test]
    fn exp_and_prec_two_digit_and_keep_zeros() {
        let b = budget();
        assert_eq!(fmt_f64_exp(1.5, 2, false, true, &b).unwrap().as_str(), "1.50e+00");
        assert_eq!(fmt_f64_exp(1.5, 2, false, false, &b).unwrap().as_str(), "1.50e+0");
        assert_eq!(fmt_f64_exp(1234.5, 2, true, true, &b).unwrap().as_str(), "1.23E+03");
        assert_eq!(fmt_f64_exp(0.0, 2, false, true, &b).unwrap().as_str(), "0.00e+00");
        assert_eq!(fmt_f64_exp(f64::NAN, 101, true, false, &b).unwrap().as_str(), "NAN");
        assert_eq!(fmt_f64_prec(100.0, 6, false, true, true, &b).unwrap().as_str(), "100.000");
        assert_eq!(fmt_f64_prec(100.0, 6, false, false, true, &b).unwrap().as_str(), "100");
        assert_eq!(fmt_f64_prec(123.4, 2, false, false, true, &b).unwrap().as_str(), "1.2e+02");
    }

    #[test]
    fn radix_abs_formatting_for_large_doubles() {
        let b = budget();
        let mut s = KString::new_in(&b);
        fmt_f64_abs_radix(1e25, 16, false, &mut s).unwrap();
        let expected_hex = format!("{:x}", 1e25_f64 as u128);
        assert_eq!(s.as_str(), expected_hex);

        s.clear();
        fmt_f64_abs_radix(1e25, 8, false, &mut s).unwrap();
        let expected_oct = format!("{:o}", 1e25_f64 as u128);
        assert_eq!(s.as_str(), expected_oct);

        s.clear();
        fmt_f64_abs_radix(f64::NEG_INFINITY, 16, true, &mut s).unwrap();
        assert_eq!(s.as_str(), "INFINITY");
    }

    #[test]
    fn arith_edge_cases_and_clamping() {
        assert_eq!(parse_arith_literal(b""), Err(ArithNumError::InvalidNumber));
        assert_eq!(parse_arith_literal(b"0x"), Err(ArithNumError::InvalidNumber));
        assert_eq!(parse_arith_literal(b"0x1g"), Err(ArithNumError::InvalidNumber));
        assert_eq!(parse_arith_literal(b"1#0"), Err(ArithNumError::ValueTooGreatForBase));
        assert_eq!(parse_arith_literal(b"16#"), Err(ArithNumError::ValueTooGreatForBase));
        assert_eq!(parse_arith_literal(b"#1"), Err(ArithNumError::ValueTooGreatForBase));
        assert_eq!(parse_arith_literal(b"16#g"), Err(ArithNumError::ValueTooGreatForBase));
        assert_eq!(parse_arith_literal(b"077"), Ok(63.0));
        assert_eq!(parse_arith_literal(b"0xFFFFFFFFFFFFFFFF"), Ok(MAX_SAFE_INT_F64));
    }

    #[test]
    fn printf_int_whitespace_and_char_units() {
        assert_eq!(parse_printf_int(b""), PrintfInt { value: 0.0, invalid: false });
        assert_eq!(parse_printf_int(b"  +0x10  "), PrintfInt { value: 16.0, invalid: true });
        assert_eq!(parse_printf_int(b"\"A "), PrintfInt { value: 65.0, invalid: false });
        assert_eq!(parse_printf_int(b"\\'z"), PrintfInt { value: 122.0, invalid: false });
        assert_eq!(parse_printf_int("\'😀".as_bytes()), PrintfInt { value: 0xd83d as f64, invalid: false });
        assert_eq!(parse_printf_int(b"0x"), PrintfInt { value: 0.0, invalid: true });
        assert_eq!(parse_printf_int(b"-0"), PrintfInt { value: 0.0, invalid: false });
        assert_eq!(parse_printf_int(b"++5"), PrintfInt { value: 5.0, invalid: true });
    }

    #[test]
    fn f64_prefix_and_strict_unicode_ws() {
        assert_eq!(parse_f64_prefix("\u{00a0}  -2.5e-1xyz".as_bytes()), Some(-0.25));
        assert_eq!(parse_f64_prefix(b"."), None);
        assert_eq!(parse_f64_prefix(b"+"), None);
        assert_eq!(parse_f64_prefix(b"-InfinityAndBeyond"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_f64_strict("\u{feff} 42 \u{3000}".as_bytes()), Some(42.0));
        assert_eq!(parse_f64_strict(b"+0b10"), None);
        assert_eq!(parse_f64_strict(b"0x"), None);
        assert_eq!(parse_f64_strict(b"NaN"), None);
        assert_eq!(parse_f64_strict(b".5"), Some(0.5));
    }

    #[test]
    fn f64_trunc_and_floor_no_std() {
        assert_eq!(f64_trunc(3.9), 3.0);
        assert_eq!(f64_trunc(-3.9), -3.0);
        assert_eq!(f64_trunc(0.5), 0.0);
        assert_eq!(f64_floor(-1.5), -2.0);
        assert_eq!(f64_floor(-0.1), -1.0);
        assert_eq!(f64_floor(2.9), 2.0);
        assert_eq!(f64_floor(-2.0), -2.0);
    }

    #[test]
    fn exact_decimal_exp_powers_of_ten() {
        assert_eq!(exact_decimal_exp(1.0), 0);
        assert_eq!(exact_decimal_exp(9.999999999999998), 0);
        assert_eq!(exact_decimal_exp(10.0), 1);
        assert_eq!(exact_decimal_exp(0.0001), -4);
        assert_eq!(exact_decimal_exp(0.00009999999999999999), -5);
    }

    #[test]
    fn seeded_random_f64_round_trip_native() {
        let b = budget();
        let mut s = 0x5345_4e54_4c38_4231_u64;
        for _ in 0..500 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let v = f64::from_bits(s);
            let formatted = f64_to_js_string(v, &b).unwrap();
            if v.is_nan() {
                assert_eq!(formatted.as_str(), "NaN");
            } else if v == 0.0 {
                assert_eq!(formatted.as_str(), "0");
            } else if v.is_infinite() {
                assert!(formatted.as_str().ends_with("Infinity"));
            } else {
                let reparsed: f64 = formatted.as_str().parse().unwrap();
                assert_eq!(reparsed.to_bits(), v.to_bits());
            }
        }
    }

    #[test]
    fn to_fixed_subnormal_and_tiny() {
        let b = budget();
        assert_eq!(fmt_f64_fixed(5e-324, 6, &b).unwrap().as_str(), "0.000000");
        assert_eq!(fmt_f64_fixed(-5e-324, 6, &b).unwrap().as_str(), "-0.000000");
        assert_eq!(fmt_f64_fixed(0.05, 1, &b).unwrap().as_str(), "0.1");
    }

    #[test]
    fn to_exp_subnormal_and_negative_zero() {
        let b = budget();
        assert_eq!(fmt_f64_exp(-0.0, 3, false, false, &b).unwrap().as_str(), "0.000e+0");
        assert_eq!(fmt_f64_exp(5e-324, 2, false, false, &b).unwrap().as_str(), "4.94e-324");
        assert_eq!(fmt_f64_exp(5e-324, 2, true, true, &b).unwrap().as_str(), "4.94E-324");
    }

    #[test]
    fn to_prec_rounding_shifts_exp() {
        let b = budget();
        assert_eq!(fmt_f64_prec(9.99e-7, 2, false, true, false, &b).unwrap().as_str(), "0.0000010");
        assert_eq!(fmt_f64_prec(999_999.9, 6, false, true, false, &b).unwrap().as_str(), "1.00000e+6");
        assert_eq!(fmt_f64_prec(999_999.9, 6, false, false, true, &b).unwrap().as_str(), "1e+06");
    }

    #[test]
    fn parse_arith_all_bases_2_to_64() {
        for base in 2u32..=64 {
            let mut buf = [0u8; 8];
            let s = if base < 10 {
                buf[0] = b'0' + base as u8;
                buf[1] = b'#';
                buf[2] = b'1';
                buf[3] = b'0';
                &buf[..4]
            } else {
                buf[0] = b'0' + (base / 10) as u8;
                buf[1] = b'0' + (base % 10) as u8;
                buf[2] = b'#';
                buf[3] = b'1';
                buf[4] = b'0';
                &buf[..5]
            };
            assert_eq!(parse_arith_literal(s), Ok(base as f64));
        }
    }

    #[test]
    fn parse_printf_int_all_unicode_ws() {
        assert_eq!(parse_printf_int("\u{2003}17\u{3000}".as_bytes()), PrintfInt { value: 17.0, invalid: true });
        assert_eq!(parse_printf_int("\u{feff} 99".as_bytes()), PrintfInt { value: 99.0, invalid: false });
    }

    #[test]
    fn parse_f64_strict_extremes() {
        assert_eq!(parse_f64_strict(b"1e309"), Some(f64::INFINITY));
        assert_eq!(parse_f64_strict(b"-1e309"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_f64_strict(b"1e-400"), Some(0.0));
        assert_eq!(parse_f64_strict(b"1.7976931348623157e308"), Some(f64::MAX));
    }

    #[test]
    fn parse_f64_prefix_malformed_exponents() {
        assert_eq!(parse_f64_prefix(b"12e"), Some(12.0));
        assert_eq!(parse_f64_prefix(b"12e+"), Some(12.0));
        assert_eq!(parse_f64_prefix(b"12e-x"), Some(12.0));
        assert_eq!(parse_f64_prefix(b"+.25e2"), Some(25.0));
    }
}
