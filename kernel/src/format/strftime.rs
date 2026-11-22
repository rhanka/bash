//! Civil-calendar `strftime` engine and S11-compliant timezone parser.

use super::FormatLimitKind;
use crate::mem::{Budget, KVec};
use core::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TzSpec {
    /// Offset east of UTC in seconds (e.g. `-18_000` for `-0500`, `0` for UTC).
    pub offset_east_sec: i32,
    /// Abbreviated zone label (`"UTC"` by default, up to 8 ASCII bytes).
    pub abbr: [u8; 8],
    pub abbr_len: u8,
}

impl TzSpec {
    pub const fn utc() -> Self {
        Self {
            offset_east_sec: 0,
            abbr: *b"UTC\0\0\0\0\0",
            abbr_len: 3,
        }
    }

    pub fn abbr_str(&self) -> &str {
        let len = (self.abbr_len as usize).min(self.abbr.len());
        core::str::from_utf8(&self.abbr[..len]).unwrap_or("UTC")
    }
}

fn make_abbr(bytes: &[u8]) -> ([u8; 8], u8) {
    let mut abbr = [0u8; 8];
    let len = bytes.len().min(8);
    abbr[..len].copy_from_slice(&bytes[..len]);
    (abbr, len as u8)
}

fn parse_u32_exact(s: &[u8]) -> Option<u32> {
    if s.is_empty() || !s.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut v = 0u32;
    for &b in s {
        v = v.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(v)
}

/// Parses `h[:m[:s]]` or `HH[MM]` POSIX/ISO offset components into non-negative seconds.
fn parse_hms_offset(s: &[u8], allow_compact_hhmm: bool) -> Option<i32> {
    if s.is_empty() {
        return None;
    }
    if s.contains(&b':') {
        let mut parts = s.split(|&b| b == b':');
        let h_bytes = parts.next()?;
        let m_bytes = parts.next()?;
        let sec_bytes = parts.next();
        if parts.next().is_some() {
            return None;
        }
        if !(1..=2).contains(&h_bytes.len()) || m_bytes.len() != 2 {
            return None;
        }
        let h = parse_u32_exact(h_bytes)?;
        let m = parse_u32_exact(m_bytes)?;
        let sec = if let Some(sb) = sec_bytes {
            if sb.len() != 2 {
                return None;
            }
            parse_u32_exact(sb)?
        } else {
            0
        };
        if h > 23 || m > 59 || sec > 59 {
            return None;
        }
        return Some((h * 3600 + m * 60 + sec) as i32);
    }
    if allow_compact_hhmm && s.len() == 4 && s.iter().all(|b| b.is_ascii_digit()) {
        let h = parse_u32_exact(&s[..2])?;
        let m = parse_u32_exact(&s[2..4])?;
        if h > 23 || m > 59 {
            return None;
        }
        return Some((h * 3600 + m * 60) as i32);
    }
    if (1..=2).contains(&s.len()) {
        let h = parse_u32_exact(s)?;
        if h > 23 {
            return None;
        }
        return Some((h * 3600) as i32);
    }
    None
}

/// Resolves `$TZ` into a fixed offset and abbreviation without ever consulting the host timezone (EVOL S11).
pub fn parse_tz_spec(tz: Option<&[u8]>) -> TzSpec {
    let Some(raw) = tz else {
        return TzSpec::utc();
    };
    if raw.is_empty()
        || matches!(
            raw,
            b"UTC"
                | b"GMT"
                | b"UCT"
                | b"Universal"
                | b"Zulu"
                | b"Etc/UTC"
                | b"Etc/GMT"
                | b"Etc/GMT+0"
                | b"Etc/GMT-0"
                | b"Etc/GMT0"
                | b"Z"
        )
    {
        return TzSpec::utc();
    }

    // `Etc/GMT+H` or `Etc/GMT-H` (0 <= H <= 14): POSIX-inverted sign.
    if let Some(rest) = raw.strip_prefix(b"Etc/GMT") {
        if rest.len() >= 2 && (rest[0] == b'+' || rest[0] == b'-') {
            if let Some(h) = parse_u32_exact(&rest[1..]) {
                if h <= 14 {
                    if h == 0 {
                        return TzSpec::utc();
                    }
                    // Etc/GMT+5 => -5h east of UTC, abbr "GMT-5"
                    let sign_east = if rest[0] == b'+' { -1 } else { 1 };
                    let mut abbr = [0u8; 8];
                    abbr[..3].copy_from_slice(b"GMT");
                    abbr[3] = if sign_east < 0 { b'-' } else { b'+' };
                    let mut len = 4usize;
                    if h >= 10 {
                        abbr[len] = b'0' + (h / 10) as u8;
                        len += 1;
                    }
                    abbr[len] = b'0' + (h % 10) as u8;
                    len += 1;
                    return TzSpec {
                        offset_east_sec: sign_east * (h as i32) * 3600,
                        abbr,
                        abbr_len: len as u8,
                    };
                }
            }
        }
        return TzSpec::utc();
    }

    // Pure ISO offset `[+-]HH[:MM]` or `[+-]HHMM`
    if raw[0] == b'+' || raw[0] == b'-' {
        let sign_east = if raw[0] == b'+' { 1 } else { -1 };
        if let Some(secs) = parse_hms_offset(&raw[1..], true) {
            if secs == 0 {
                return TzSpec::utc();
            }
            let (abbr, abbr_len) = make_abbr(raw);
            return TzSpec {
                offset_east_sec: sign_east * secs,
                abbr,
                abbr_len,
            };
        }
        return TzSpec::utc();
    }

    // Bracketed POSIX `<ABBR>[+-]h[:m[:s]]`
    if raw[0] == b'<' {
        if let Some(close) = raw[1..].iter().position(|&b| b == b'>') {
            let name = &raw[1..1 + close];
            let rest = &raw[2 + close..];
            if !name.is_empty() && name.len() <= 8 && !rest.is_empty() {
                let (sign_east, digits) = if rest[0] == b'+' {
                    (-1, &rest[1..])
                } else if rest[0] == b'-' {
                    (1, &rest[1..])
                } else {
                    (-1, rest)
                };
                if let Some(secs) = parse_hms_offset(digits, false) {
                    let (abbr, abbr_len) = make_abbr(name);
                    return TzSpec {
                        offset_east_sec: sign_east * secs,
                        abbr,
                        abbr_len,
                    };
                }
            }
        }
        return TzSpec::utc();
    }

    // Unbracketed POSIX `STD[+-]h[:m[:s]]` (`^[A-Za-z]{3,6}[+-]?\d{1,2}(:\d{2}(:\d{2})?)?$`)
    let alpha_len = raw.iter().take_while(|b| b.is_ascii_alphabetic()).count();
    if (3..=6).contains(&alpha_len) && alpha_len < raw.len() {
        let name = &raw[..alpha_len];
        let rest = &raw[alpha_len..];
        let (sign_east, digits) = if rest[0] == b'+' {
            (-1, &rest[1..])
        } else if rest[0] == b'-' {
            (1, &rest[1..])
        } else {
            (-1, rest)
        };
        if let Some(secs) = parse_hms_offset(digits, false) {
            let (abbr, abbr_len) = make_abbr(name);
            return TzSpec {
                offset_east_sec: sign_east * secs,
                abbr,
                abbr_len,
            };
        }
    }

    TzSpec::utc()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilDateTime {
    pub year: i64,
    pub month: u8,   // 1..=12
    pub day: u8,     // 1..=31
    pub hour: u8,    // 0..=23
    pub minute: u8,  // 0..=59
    pub second: u8,  // 0..=59
    pub weekday: u8, // 0 = Sunday .. 6 = Saturday
    pub yday: u16,   // 1..=366
}

#[inline]
fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[inline]
fn days_in_year(y: i64) -> i64 {
    if is_leap_year(y) { 366 } else { 365 }
}

/// Converts Unix epoch seconds and east-of-UTC offset into proleptic Gregorian civil date/time.
pub fn civil_from_timestamp(epoch_sec: i64, offset_east_sec: i32) -> CivilDateTime {
    let local_sec = epoch_sec.saturating_add(offset_east_sec as i64);
    let days = local_sec.div_euclid(86_400);
    let rem = local_sec.rem_euclid(86_400);
    let hour = (rem / 3600) as u8;
    let minute = ((rem % 3600) / 60) as u8;
    let second = (rem % 60) as u8;
    let weekday = (days + 4).rem_euclid(7) as u8;

    // Howard Hinnant's civil_from_days algorithm (proleptic Gregorian, exact for all i64).
    let z = days.saturating_add(719_468);
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = y + i64::from(month <= 2);

    const CUM_DAYS: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let yday = CUM_DAYS[(month - 1) as usize]
        + (day as u16)
        + u16::from(is_leap_year(year) && month > 2);

    CivilDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        weekday,
        yday,
    }
}

const ABBR_DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const FULL_DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const ABBR_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const FULL_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn iso_week_and_year(dt: &CivilDateTime) -> (i64, u8) {
    let iso_wday = if dt.weekday == 0 { 7 } else { dt.weekday };
    let yday_thu = (dt.yday as i64) + 4 - (iso_wday as i64);
    let (iso_year, iso_yday) = if yday_thu < 1 {
        let prev = dt.year - 1;
        (prev, yday_thu + days_in_year(prev))
    } else {
        let cur_days = days_in_year(dt.year);
        if yday_thu > cur_days {
            (dt.year + 1, yday_thu - cur_days)
        } else {
            (dt.year, yday_thu)
        }
    };
    let week = ((iso_yday - 1) / 7 + 1) as u8;
    (iso_year, week)
}

struct SmallBuf {
    bytes: [u8; 64],
    len: usize,
}

impl SmallBuf {
    const fn new() -> Self {
        Self { bytes: [0; 64], len: 0 }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for SmallBuf {
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

fn push_checked(
    out: &mut KVec<u8>,
    bytes: &[u8],
    max_output_bytes: usize,
) -> Result<(), FormatLimitKind> {
    if out.len().saturating_add(bytes.len()) > max_output_bytes {
        return Err(FormatLimitKind::StrftimeOutputSize { limit_bytes: max_output_bytes });
    }
    out.try_extend_from_slice(bytes)?;
    Ok(())
}

pub fn format_strftime(
    format: &[u8],
    epoch_sec: i64,
    tz: Option<&[u8]>,
    max_operations: usize,
    max_output_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, FormatLimitKind> {
    let tz_spec = parse_tz_spec(tz);
    let dt = civil_from_timestamp(epoch_sec, tz_spec.offset_east_sec);
    let mut out = KVec::new_in(b);
    let mut ops = 0usize;
    let mut i = 0usize;

    let jan1_wday = ((dt.weekday as i32) + 7 - (((dt.yday as i32) - 1) % 7)).rem_euclid(7);
    let first_sun = (7 - jan1_wday) % 7;
    let jan1_mon = (jan1_wday + 6) % 7;
    let first_mon = (7 - jan1_mon) % 7;

    while i < format.len() {
        ops += 1;
        if ops > max_operations {
            return Err(FormatLimitKind::StrftimeIterations { max_ops: max_operations });
        }
        if ops % 64 == 0 && work(1).is_err() {
            return Err(FormatLimitKind::StrftimeIterations { max_ops: max_operations });
        }

        if format[i] != b'%' {
            push_checked(&mut out, &format[i..i + 1], max_output_bytes)?;
            i += 1;
            continue;
        }
        if i + 1 >= format.len() {
            push_checked(&mut out, b"%", max_output_bytes)?;
            i += 1;
            continue;
        }

        let spec = format[i + 1];
        let mut buf = SmallBuf::new();
        let mut handled = true;

        match spec {
            b'%' => {
                let _ = buf.write_str("%");
            }
            b'a' => {
                let _ = buf.write_str(ABBR_DAYS[dt.weekday as usize]);
            }
            b'A' => {
                let _ = buf.write_str(FULL_DAYS[dt.weekday as usize]);
            }
            b'b' | b'h' => {
                let _ = buf.write_str(ABBR_MONTHS[(dt.month - 1) as usize]);
            }
            b'B' => {
                let _ = buf.write_str(FULL_MONTHS[(dt.month - 1) as usize]);
            }
            b'c' => {
                let _ = write!(
                    buf,
                    "{} {} {:2} {:02}:{:02}:{:02} {}",
                    ABBR_DAYS[dt.weekday as usize],
                    ABBR_MONTHS[(dt.month - 1) as usize],
                    dt.day,
                    dt.hour,
                    dt.minute,
                    dt.second,
                    dt.year
                );
            }
            b'C' => {
                let _ = write!(buf, "{:02}", dt.year.div_euclid(100));
            }
            b'd' => {
                let _ = write!(buf, "{:02}", dt.day);
            }
            b'D' | b'x' => {
                let _ = write!(
                    buf,
                    "{:02}/{:02}/{:02}",
                    dt.month,
                    dt.day,
                    dt.year.rem_euclid(100)
                );
            }
            b'e' => {
                let _ = write!(buf, "{:2}", dt.day);
            }
            b'F' => {
                let _ = write!(buf, "{}-{:02}-{:02}", dt.year, dt.month, dt.day);
            }
            b'G' => {
                let (iso_y, _) = iso_week_and_year(&dt);
                let _ = write!(buf, "{iso_y}");
            }
            b'g' => {
                let (iso_y, _) = iso_week_and_year(&dt);
                let _ = write!(buf, "{:02}", iso_y.rem_euclid(100));
            }
            b'H' => {
                let _ = write!(buf, "{:02}", dt.hour);
            }
            b'I' => {
                let h12 = match dt.hour % 12 {
                    0 => 12,
                    h => h,
                };
                let _ = write!(buf, "{h12:02}");
            }
            b'j' => {
                let _ = write!(buf, "{:03}", dt.yday);
            }
            b'k' => {
                let _ = write!(buf, "{:2}", dt.hour);
            }
            b'l' => {
                let h12 = match dt.hour % 12 {
                    0 => 12,
                    h => h,
                };
                let _ = write!(buf, "{h12:2}");
            }
            b'm' => {
                let _ = write!(buf, "{:02}", dt.month);
            }
            b'M' => {
                let _ = write!(buf, "{:02}", dt.minute);
            }
            b'n' => {
                let _ = buf.write_str("\n");
            }
            b'N' => {
                let _ = buf.write_str("000000000");
            }
            b'p' => {
                let _ = buf.write_str(if dt.hour < 12 { "AM" } else { "PM" });
            }
            b'P' => {
                let _ = buf.write_str(if dt.hour < 12 { "am" } else { "pm" });
            }
            b'r' => {
                let h12 = match dt.hour % 12 {
                    0 => 12,
                    h => h,
                };
                let ampm = if dt.hour < 12 { "AM" } else { "PM" };
                let _ = write!(buf, "{h12:02}:{:02}:{:02} {ampm}", dt.minute, dt.second);
            }
            b'R' => {
                let _ = write!(buf, "{:02}:{:02}", dt.hour, dt.minute);
            }
            b's' => {
                let _ = write!(buf, "{epoch_sec}");
            }
            b'S' => {
                let _ = write!(buf, "{:02}", dt.second);
            }
            b't' => {
                let _ = buf.write_str("\t");
            }
            b'T' | b'X' => {
                let _ = write!(buf, "{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second);
            }
            b'u' => {
                let iso_w = if dt.weekday == 0 { 7 } else { dt.weekday };
                let _ = write!(buf, "{iso_w}");
            }
            b'U' => {
                let u = if (dt.yday as i32) - 1 < first_sun {
                    0
                } else {
                    1 + ((dt.yday as i32) - 1 - first_sun) / 7
                };
                let _ = write!(buf, "{u:02}");
            }
            b'V' => {
                let (_, week) = iso_week_and_year(&dt);
                let _ = write!(buf, "{week:02}");
            }
            b'w' => {
                let _ = write!(buf, "{}", dt.weekday);
            }
            b'W' => {
                let w = if (dt.yday as i32) - 1 < first_mon {
                    0
                } else {
                    1 + ((dt.yday as i32) - 1 - first_mon) / 7
                };
                let _ = write!(buf, "{w:02}");
            }
            b'y' => {
                let _ = write!(buf, "{:02}", dt.year.rem_euclid(100));
            }
            b'Y' => {
                if dt.year >= 0 && dt.year < 1000 {
                    let _ = write!(buf, "{:04}", dt.year);
                } else {
                    let _ = write!(buf, "{}", dt.year);
                }
            }
            b'z' => {
                let off = tz_spec.offset_east_sec;
                let sign = if off < 0 { '-' } else { '+' };
                let abs = off.unsigned_abs();
                let hh = abs / 3600;
                let mm = (abs % 3600) / 60;
                let _ = write!(buf, "{sign}{hh:02}{mm:02}");
            }
            b'Z' => {
                let _ = buf.write_str(tz_spec.abbr_str());
            }
            _ => {
                handled = false;
            }
        }

        if handled {
            push_checked(&mut out, buf.as_bytes(), max_output_bytes)?;
            i += 2;
        } else {
            push_checked(&mut out, b"%", max_output_bytes)?;
            i += 1;
        }
    }

    Ok(out)
}
