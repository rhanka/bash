//! Formatting engines: bash `printf`, escape processors, `%q` shell quoting,
//! `awk` `sprintf`/`OFMT`, and proleptic-Gregorian `strftime`.

mod escape;
mod printf;
mod strftime;

use crate::mem::MemError;

pub use escape::{process_b_escapes, process_format_escapes, shell_quote_printf, BEscapeOut};
pub use printf::{format_awk_printf, format_printf, AwkArg, PrintfOpts, PrintfOutput};
pub use strftime::{civil_from_timestamp, format_strftime, parse_tz_spec, CivilDateTime, TzSpec};

#[derive(Debug, PartialEq, Eq)]
pub enum FormatLimitKind {
    /// `escaped string size limit exceeded (<rem> bytes)`
    EscapedStringSize { limit_bytes: usize },
    /// `format width limit exceeded (<rem> bytes)` (or `printf width limit exceeded (<cap> bytes)` in `awk`)
    Width { limit_bytes: usize },
    /// `format precision limit exceeded (<rem> bytes)` (or `printf precision limit exceeded (<cap> bytes)` in `awk`)
    Precision { limit_bytes: usize },
    /// `printf floating-point precision limit exceeded (100 digits)` (`awk` only)
    AwkFloatPrecision,
    /// `formatted value size limit exceeded (<rem> bytes)` (or `formatted string size limit exceeded (<max_bytes> bytes)` in `awk`)
    FormattedValueSize { limit_bytes: usize },
    /// `printf: output size limit exceeded (<max> bytes)`
    OutputSize { limit_bytes: usize },
    /// `strftime: output size limit exceeded (<rem> bytes)`
    StrftimeOutputSize { limit_bytes: usize },
    /// `strftime: iteration limit exceeded (<max_ops>)`
    StrftimeIterations { max_ops: usize },
    Mem(MemError),
}

impl From<MemError> for FormatLimitKind {
    fn from(e: MemError) -> Self {
        Self::Mem(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::{Budget, LimitId};

    fn budget() -> Budget {
        Budget::root(LimitId(30), u64::MAX).unwrap()
    }

    fn opts(max_output_bytes: usize) -> PrintfOpts<'static> {
        PrintfOpts {
            tz: Some(b"UTC"),
            now_sec: 1_700_000_000,
            start_sec: 1_699_999_000,
            max_output_bytes,
        }
    }

    #[test]
    fn printf_utf16_padding_and_truncation() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%5s|%.2s|%.1s", &[b"\xf0\x9f\x98\x80", b"\xf0\x9f\x98\x80ab", b"\xf0\x9f\x98\x80"], &opts(1024), &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), "   😀|😀|\u{FFFD}".as_bytes());
    }

    #[test]
    fn printf_ieee_oct_hex_and_g_and_neg_star_divergences() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(
            b"%o|%x|%X|%g|%.1g|%.*d|%.*s|%.*f",
            &[
                b"10000000000000000000000000",
                b"10000000000000000000000000",
                b"10000000000000000000000000",
                b"100000",
                b"0",
                b"-2",
                b"7",
                b"-2",
                b"hi",
                b"-2",
                b"1.5",
            ],
            &opts(4096),
            &b,
            &mut noop,
        )
        .unwrap();
        assert!(!out.had_error);
        let expected = format!(
            "{:o}|{:x}|{:X}|100000|0|7|hi|1.500000",
            1e25_f64 as u128, 1e25_f64 as u128, 1e25_f64 as u128
        );
        assert_eq!(core::str::from_utf8(out.bytes.as_slice()).unwrap(), expected);
    }

    #[test]
    fn printf_limits_prospective() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        assert_eq!(
            format_printf(b"%2147483647s", &[b"hi"], &opts(16 << 20), &b, &mut noop).err(),
            Some(FormatLimitKind::Width { limit_bytes: 16 << 20 })
        );
        assert_eq!(
            format_printf(b"%.2147483647d", &[b"1"], &opts(16 << 20), &b, &mut noop).err(),
            Some(FormatLimitKind::Precision { limit_bytes: 16 << 20 })
        );
        let neg = format_printf(b"%.*d", &[b"-100000000", b"7"], &opts(4 << 20), &b, &mut noop).unwrap();
        assert_eq!(neg.bytes.as_slice(), b"7");
    }

    #[test]
    fn awk_printf_rules() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let args = [
            AwkArg::Num(-1.5),
            AwkArg::Str(b"hi"),
            AwkArg::Num(-16.0),
            AwkArg::Num(1e-5),
            AwkArg::Num(0.0),
            AwkArg::Num(12.34),
        ];
        let out = format_awk_printf(
            b"%2$s:%1$d:%3$x:%4$g:%5$g:%6$.f:%F:%z",
            &args,
            10_000,
            &b,
            &mut noop,
        )
        .unwrap();
        assert_eq!(
            core::str::from_utf8(out.as_slice()).unwrap(),
            "hi:-2:-10:1e-5:0:12.340000:%F:%z"
        );

        assert_eq!(
            format_awk_printf(b"%15000s", &[AwkArg::Str(b"a")], 20_000, &b, &mut noop).unwrap_err(),
            FormatLimitKind::Width { limit_bytes: 10_000 }
        );
        assert_eq!(
            format_awk_printf(b"%.101f", &[AwkArg::Num(1.0)], 20_000, &b, &mut noop).unwrap_err(),
            FormatLimitKind::AwkFloatPrecision
        );
        assert_eq!(
            format_awk_printf(b"%s%s", &[AwkArg::Str(b"12345"), AwkArg::Str(b"67890")], 8, &b, &mut noop).unwrap_err(),
            FormatLimitKind::FormattedValueSize { limit_bytes: 8 }
        );
    }

    #[test]
    fn strftime_negative_future_and_tz() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let neg = format_strftime(b"%Y-%m-%d %H:%M:%S %V %G", -86_400, None, 1024, 1024, &b, &mut noop).unwrap();
        assert_eq!(neg.as_slice(), b"1969-12-31 00:00:00 01 1970");

        let fut = format_strftime(b"%Y-%m-%d %H:%M:%S %Z", 4_102_444_800, Some(b"Invalid/Zone"), 1024, 1024, &b, &mut noop).unwrap();
        assert_eq!(fut.as_slice(), b"2100-01-01 00:00:00 UTC");

        let etc = format_strftime(b"%H %z %Z", 0, Some(b"Etc/GMT+5"), 1024, 1024, &b, &mut noop).unwrap();
        assert_eq!(etc.as_slice(), b"19 -0500 GMT-5");

        let est = format_strftime(b"%H %z %Z", 0, Some(b"EST5"), 1024, 1024, &b, &mut noop).unwrap();
        assert_eq!(est.as_slice(), b"19 -0500 EST");
    }

    #[test]
    fn format_escapes_table_parity() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = process_format_escapes(
            b"\\a|\\b|\\e|\\E|\\f|\\n|\\r|\\t|\\v|\\\\|\\0101|\\101|\\777|\\x4|\\x41|\\xc3\\xa9|\\xc3\\x28|\\u00e9|\\U0001f600|\\cX",
            4096,
            &b,
            &mut noop,
        )
        .unwrap();
        assert_eq!(
            out.as_slice(),
            "\x07|\x08|\x1b|\x1b|\x0c|\n|\r|\t|\x0b|\\\x081|A|ǿ|\\x4|A|é|Ã(|é|😀|\\cX".replace("\\\x081", "\\|\x081").as_bytes()
        );
    }

    #[test]
    fn b_escapes_and_early_stop() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = process_b_escapes(
            b"\\a|\\e|\\0|\\0101|\\101|\\777|\\x4g|\\xc3\\xa9|\\xc3\\x28|\\u00e9|\\U0001f600",
            4096,
            &b,
            &mut noop,
        )
        .unwrap();
        assert!(!out.stopped);
        assert_eq!(
            out.bytes.as_slice(),
            "\x07|\\e|\x00|A|A|ǿ|\x04g|é|Ã(|é|\\U0001f600".as_bytes()
        );

        let stopped = process_b_escapes(b"abc\\cdef", 4096, &b, &mut noop).unwrap();
        assert!(stopped.stopped);
        assert_eq!(stopped.bytes.as_slice(), b"abc");
    }

    #[test]
    fn shell_quote_q_rules_and_limits() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        assert_eq!(shell_quote_printf(b"", 100, &b, &mut noop).unwrap().as_slice(), b"''");
        assert_eq!(shell_quote_printf(b"abc_1-2./", 100, &b, &mut noop).unwrap().as_slice(), b"abc_1-2./");
        assert_eq!(shell_quote_printf(b"hello world", 100, &b, &mut noop).unwrap().as_slice(), b"hello\\ world");
        assert_eq!(shell_quote_printf(b"a'b", 100, &b, &mut noop).unwrap().as_slice(), b"a\\'b");
        assert_eq!(shell_quote_printf("é".as_bytes(), 100, &b, &mut noop).unwrap().as_slice(), b"$'\\351'");
        assert_eq!(shell_quote_printf(b"a\tb", 100, &b, &mut noop).unwrap().as_slice(), b"$'a\\tb'");
        assert_eq!(shell_quote_printf(b"a", 0, &b, &mut noop).unwrap_err(), FormatLimitKind::OutputSize { limit_bytes: 0 });
        assert_eq!(shell_quote_printf(b"", 1, &b, &mut noop).unwrap_err(), FormatLimitKind::OutputSize { limit_bytes: 1 });
    }

    #[test]
    fn printf_reuse_bad_placeholders_and_precision_errors() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let reuse = format_printf(b"a:%d b:%s\\n", &[b"1", b"one", b"2", b"two", b"3"], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(reuse.bytes.as_slice(), b"a:1 b:one\na:2 b:two\na:3 b:\n");

        let plain = format_printf(b"plain%%", &[b"x", b"y"], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(plain.bytes.as_slice(), b"plain%");

        let bad_pct = format_printf(b"%10%", &[b"1"], &opts(4096), &b, &mut noop).unwrap();
        assert!(bad_pct.had_error);
        assert_eq!(bad_pct.stderr.as_str(), "printf: [sprintf] unexpected placeholder\n");

        let bad_q = format_printf(b"%.2q", &[b"hi"], &opts(4096), &b, &mut noop).unwrap();
        assert!(bad_q.had_error);

        let range = format_printf(b"%.101f", &[b"1.0"], &opts(4096), &b, &mut noop).unwrap();
        assert!(range.had_error);
        assert_eq!(range.stderr.as_str(), "printf: toFixed() digits argument must be between 0 and 100\n");
    }

    #[test]
    fn printf_strftime_directive_and_width() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%12(%Y-%m-%d)T|%()T|%(%F)T", &[b"1700000000", b"0", b"-1"], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), b"  2023-11-14||2023-11-14");
    }

    #[test]
    fn printf_b_stops_reuse_loop() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%b-%s ", &[b"abc\\cdef", b"ignored", b"next"], &opts(4096), &b, &mut noop).unwrap();
        assert!(out.stopped);
        assert_eq!(out.bytes.as_slice(), b"abc");
    }

    #[test]
    fn awk_printf_all_branches() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_awk_printf(
            b"\\n\\t\\r\\\\\\q|%*.*f|%5.2s|%c|%c|%%",
            &[
                AwkArg::Num(-8.0),
                AwkArg::Num(2.0),
                AwkArg::Num(3.5),
                AwkArg::Str(b"hello"),
                AwkArg::Num(65.0),
                AwkArg::Str(b"xyz"),
            ],
            4096,
            &b,
            &mut noop,
        )
        .unwrap();
        assert_eq!(out.as_slice(), b"\n\t\r\\q|3.50    |   he|A|x|%" as &[u8]);
    }

    #[test]
    fn tz_spec_variants() {
        assert_eq!(parse_tz_spec(None), TzSpec::utc());
        assert_eq!(parse_tz_spec(Some(b"GMT")), TzSpec::utc());
        assert_eq!(parse_tz_spec(Some(b"Etc/GMT+0")), TzSpec::utc());
        assert_eq!(parse_tz_spec(Some(b"Etc/GMT-2")).offset_east_sec, 7200);
        assert_eq!(parse_tz_spec(Some(b"+05:30")).offset_east_sec, 19800);
        assert_eq!(parse_tz_spec(Some(b"-0400")).offset_east_sec, -14400);
        let br = parse_tz_spec(Some(b"<+03>-3"));
        assert_eq!(br.offset_east_sec, 10800);
        assert_eq!(br.abbr_str(), "+03");
        assert_eq!(parse_tz_spec(Some(b"America/New_York")), TzSpec::utc());
    }

    #[test]
    fn strftime_all_39_directives_epoch0() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_strftime(
            b"%Y-%m-%d %H:%M:%S %Z %z %a %A %b %h %B %C %d %D %e %F %g %G %I %j %k %l %m %M %p %P %r %R %s %S %u %U %V %w %W %x %X %y %N %Q%%",
            0,
            Some(b"UTC"),
            4096,
            4096,
            &b,
            &mut noop,
        )
        .unwrap();
        assert_eq!(
            core::str::from_utf8(out.as_slice()).unwrap(),
            "1970-01-01 00:00:00 UTC +0000 Thu Thursday Jan Jan January 19 01 01/01/70  1 1970-01-01 70 1970 12 001  0 12 01 00 AM am 12:00:00 AM 00:00 0 00 4 00 01 4 00 01/01/70 00:00:00 70 000000000 %Q%"
        );
    }

    #[test]
    fn strftime_limits_iterations_and_size() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        assert_eq!(
            format_strftime(b"%Y%Y%Y", 0, None, 2, 100, &b, &mut noop).unwrap_err(),
            FormatLimitKind::StrftimeIterations { max_ops: 2 }
        );
        assert_eq!(
            format_strftime(b"%Y-%m-%d", 0, None, 100, 5, &b, &mut noop).unwrap_err(),
            FormatLimitKind::StrftimeOutputSize { limit_bytes: 5 }
        );
    }

    #[test]
    fn seeded_format_fuzz_native() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let mut s = 0x5345_4e54_4c38_4231_u64;
        let specs: [&[u8]; 8] = [b"%s", b"%d", b"%o", b"%x", b"%f", b"%g", b"%q", b"%b"];
        let args_pool: [&[u8]; 6] = [b"", b"-42", b"3.14", b"a\\cb", b"hello\tworld", b"\xff\x00"];
        for _ in 0..500 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let sp = specs[(s as usize) % specs.len()];
            let ag = args_pool[((s >> 8) as usize) % args_pool.len()];
            let _ = format_printf(sp, &[ag], &opts(4096), &b, &mut noop);
            let _ = format_awk_printf(sp, &[AwkArg::Str(ag)], 4096, &b, &mut noop);
            let _ = format_strftime(b"%F %T %V %G", (s as i64) >> 24, None, 1024, 1024, &b, &mut noop);
        }
    }

    #[test]
    fn process_format_escapes_limit_check() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        assert_eq!(
            process_format_escapes(b"abcdef", 5, &b, &mut noop).unwrap_err(),
            FormatLimitKind::EscapedStringSize { limit_bytes: 5 }
        );
    }

    #[test]
    fn process_b_escapes_limit_check() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        assert_eq!(
            process_b_escapes(b"abcdef", 5, &b, &mut noop).err().unwrap(),
            FormatLimitKind::OutputSize { limit_bytes: 5 }
        );
    }

    #[test]
    fn process_b_escapes_octal_zero_vs_nonzero() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = process_b_escapes(b"\\0101|\\101|\\0", 100, &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), b"A|A|\x00");
    }

    #[test]
    fn shell_quote_printf_all_ctrl_escapes() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let q = shell_quote_printf(b"\x07\x08\x0c\x0b\x1b\n\r\t'\\\"", 200, &b, &mut noop).unwrap();
        assert_eq!(q.as_slice(), b"$'\\a\\b\\f\\v\\E\\n\\r\\t\\'\\\\\\\"'");
    }

    #[test]
    fn printf_flags_zero_plus_space_hash_combos() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%+05d|% 05d|%08.4d", &[b"42", b"42", b"42"], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), b"+0042| 0042|    0042");
    }

    #[test]
    fn printf_u_wrapping_negative_and_large() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%u|%u|%+u", &[b"-1", b"-4294967296", b"5"], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), b"4294967295|0|+5");
    }

    #[test]
    fn printf_o_x_negative_and_hash() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%o|%#o|%x|%#X|%#x", &[b"-8", b"-8", b"-16", b"16", b"0"], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), b"10|010|10|0X10|0");
    }

    #[test]
    fn printf_f_hash_zero_prec_and_inf() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%#.0f|%#.0F|%F|%08F", &[b"1", b"Infinity", b"-Infinity", b"Infinity"], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), b"1.|Infinity|-Infinity|Infinity");
    }

    #[test]
    fn printf_c_empty_ascii_and_multibyte() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%c|%c|%c", &[b"", b"abc", "é".as_bytes()], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), "|a|Ã".as_bytes());
    }

    #[test]
    fn printf_s_empty_and_missing_args() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_printf(b"%s:%d", &[b"hi"], &opts(4096), &b, &mut noop).unwrap();
        assert_eq!(out.bytes.as_slice(), b"hi:0");
    }

    #[test]
    fn printf_trailing_percent_and_plain_literal() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        assert_eq!(format_printf(b"%", &[], &opts(4096), &b, &mut noop).unwrap().bytes.as_slice(), b"%");
        assert_eq!(format_printf(b"hello", &[b"1", b"2"], &opts(4096), &b, &mut noop).unwrap().bytes.as_slice(), b"hello");
    }

    #[test]
    fn awk_printf_negative_g_and_zero_g() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_awk_printf(
            b"%g|%G|%.3g|%g",
            &[AwkArg::Num(-0.0), AwkArg::Num(f64::NAN), AwkArg::Num(-1.5e-5), AwkArg::Num(100.0)],
            4096,
            &b,
            &mut noop,
        )
        .unwrap();
        assert_eq!(out.as_slice(), b"0|0|-1.5e-5|100");
    }

    #[test]
    fn awk_printf_length_modifiers_hh_ll_z_j() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_awk_printf(
            b"%hhd|%lld|%zu|%jd",
            &[AwkArg::Num(5.0), AwkArg::Num(6.0), AwkArg::Num(7.0), AwkArg::Num(8.0)],
            4096,
            &b,
            &mut noop,
        )
        .unwrap();
        assert_eq!(out.as_slice(), b"5|6|%zu|7");
    }

    #[test]
    fn awk_printf_hash_ignored_and_f_upper_literal() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_awk_printf(
            b"%#x|%#o|%F|%s",
            &[AwkArg::Num(16.0), AwkArg::Num(8.0), AwkArg::Str(b"ok")],
            4096,
            &b,
            &mut noop,
        )
        .unwrap();
        assert_eq!(out.as_slice(), b"10|10|%F|ok");
    }

    #[test]
    fn awk_printf_positional_reuse_and_missing() {
        let b = budget();
        let mut noop = |_: u64| Ok(());
        let out = format_awk_printf(b"%2$s-%1$03d-%2$s-%3$d", &[AwkArg::Num(7.0), AwkArg::Str(b"x")], 4096, &b, &mut noop).unwrap();
        assert_eq!(out.as_slice(), b"x-007-x-0");
    }

    #[test]
    fn civil_from_timestamp_leap_centuries() {
        // 2000-02-29 is leap day (951782400)
        let dt2000 = civil_from_timestamp(951_782_400, 0);
        assert_eq!((dt2000.year, dt2000.month, dt2000.day, dt2000.yday), (2000, 2, 29, 60));
        // 2100-03-01 is non-leap century (4107542400)
        let dt2100 = civil_from_timestamp(4_107_542_400, 0);
        assert_eq!((dt2100.year, dt2100.month, dt2100.day, dt2100.yday), (2100, 3, 1, 60));
    }

    #[test]
    fn tz_spec_posix_hms_and_invalid() {
        let p = parse_tz_spec(Some(b"IST-5:30:00"));
        assert_eq!(p.offset_east_sec, 5 * 3600 + 30 * 60);
        assert_eq!(p.abbr_str(), "IST");
        assert_eq!(parse_tz_spec(Some(b"UTC+25")), TzSpec::utc());
    }
}
