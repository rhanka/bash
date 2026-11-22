use super::*;
use crate::mem::{Budget, LimitId};

fn test_budget() -> Budget {
    Budget::root(LimitId::HEADROOM, 64 * 1024 * 1024).unwrap()
}

fn no_limit(_: u64) -> Result<(), ()> {
    Ok(())
}

fn compile_ere(pat: &[u8], flags: RegexFlags, b: &Budget) -> CompiledRegex {
    CompiledRegex::compile(pat, RegexMode::Extended, flags, CompileLimits::default(), b).unwrap()
}

fn compile_bre(pat: &[u8], dialect: BreDialect, flags: RegexFlags, b: &Budget) -> CompiledRegex {
    CompiledRegex::compile(pat, RegexMode::Basic(dialect), flags, CompileLimits::default(), b).unwrap()
}

fn compile_perl(pat: &[u8], flags: RegexFlags, b: &Budget) -> CompiledRegex {
    CompiledRegex::compile(pat, RegexMode::Perl, flags, CompileLimits::default(), b).unwrap()
}

#[test]
fn bre_grep_literals_and_groups() {
    let b = test_budget();
    let mut r = compile_bre(br"a+b?c(1)\|2", BreDialect::Grep, RegexFlags::default(), &b);
    assert!(r.is_match(b"a+b?c(1)|2", &b, &mut no_limit).unwrap());
    assert!(r.is_match(b"2", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"aabbc11", &b, &mut no_limit).unwrap());

    let mut r2 = compile_bre(br"a\(bc\)*d\|e", BreDialect::Grep, RegexFlags::default(), &b);
    assert!(r2.is_match(b"ad", &b, &mut no_limit).unwrap());
    assert!(r2.is_match(b"abcbcd", &b, &mut no_limit).unwrap());
    assert!(r2.is_match(b"axe", &b, &mut no_limit).unwrap());
    assert!(!r2.is_match(b"abd", &b, &mut no_limit).unwrap());

    let mut r3 = compile_bre(br"\(*abc\)", BreDialect::Grep, RegexFlags::default(), &b);
    assert!(r3.is_match(b"*abc", &b, &mut no_limit).unwrap());
    assert!(!r3.is_match(b"abc", &b, &mut no_limit).unwrap());

    let mut r4 = compile_bre(b"*abc", BreDialect::Grep, RegexFlags::default(), &b);
    assert!(r4.is_match(b"*abc", &b, &mut no_limit).unwrap());
    assert!(!r4.is_match(b"abc", &b, &mut no_limit).unwrap());
}

#[test]
fn bre_grep_plus_qmark_and_comma_interval_literal() {
    let b = test_budget();
    let mut r1 = compile_bre(br"a\+b", BreDialect::Grep, RegexFlags::default(), &b);
    assert!(r1.is_match(b"a+b", &b, &mut no_limit).unwrap());
    assert!(!r1.is_match(b"aaab", &b, &mut no_limit).unwrap());

    let mut r2 = compile_bre(br"a\?b", BreDialect::Grep, RegexFlags::default(), &b);
    assert!(r2.is_match(b"a?b", &b, &mut no_limit).unwrap());
    assert!(!r2.is_match(b"ab", &b, &mut no_limit).unwrap());

    let mut r3 = compile_bre(br"a\{,2\}b", BreDialect::Grep, RegexFlags::default(), &b);
    assert!(r3.is_match(b"a{,2}b", &b, &mut no_limit).unwrap());
    assert!(!r3.is_match(b"ab", &b, &mut no_limit).unwrap());
}

#[test]
fn bre_sed_operators_and_anchors() {
    let b = test_budget();
    let mut r_plus = compile_bre(br"a\+b", BreDialect::Sed, RegexFlags::default(), &b);
    assert!(r_plus.is_match(b"aaab", &b, &mut no_limit).unwrap());
    assert!(!r_plus.is_match(b"b", &b, &mut no_limit).unwrap());

    let mut r_qmark = compile_bre(br"a\?b", BreDialect::Sed, RegexFlags::default(), &b);
    assert!(r_qmark.is_match(b"b", &b, &mut no_limit).unwrap());
    assert!(r_qmark.is_match(b"ab", &b, &mut no_limit).unwrap());

    let mut r_comma = compile_bre(br"^a\{,2\}b$", BreDialect::Sed, RegexFlags::default(), &b);
    assert!(r_comma.is_match(b"b", &b, &mut no_limit).unwrap());
    assert!(r_comma.is_match(b"ab", &b, &mut no_limit).unwrap());
    assert!(r_comma.is_match(b"aab", &b, &mut no_limit).unwrap());
    assert!(!r_comma.is_match(b"aaab", &b, &mut no_limit).unwrap());

    // Mid-pattern ^ and $ are literal in sed BRE.
    let mut r_mid = compile_bre(b"a^b$c", BreDialect::Sed, RegexFlags::default(), &b);
    assert!(r_mid.is_match(b"a^b$c", &b, &mut no_limit).unwrap());
    assert!(!r_mid.is_match(b"abc", &b, &mut no_limit).unwrap());
}

#[test]
fn ere_comma_first_interval_flag() {
    let b = test_budget();
    let mut lit = compile_ere(b"a{,2}b", RegexFlags::default(), &b);
    assert!(lit.is_match(b"a{,2}b", &b, &mut no_limit).unwrap());
    assert!(!lit.is_match(b"ab", &b, &mut no_limit).unwrap());

    let flags = RegexFlags {
        comma_first_interval: true,
        ..Default::default()
    };
    let mut sed_ere = compile_ere(b"^a{,2}b$", flags, &b);
    assert!(sed_ere.is_match(b"b", &b, &mut no_limit).unwrap());
    assert!(sed_ere.is_match(b"ab", &b, &mut no_limit).unwrap());
    assert!(sed_ere.is_match(b"aab", &b, &mut no_limit).unwrap());
    assert!(!sed_ere.is_match(b"aaab", &b, &mut no_limit).unwrap());
}

#[test]
fn bre_expr_dialect_anchored_and_captures() {
    let b = test_budget();
    let mut r = compile_bre(b"a(b)c", BreDialect::Expr, RegexFlags::default(), &b);
    let caps = r.captures_at(b"abc", 0, &b, &mut no_limit).unwrap().unwrap();
    assert_eq!(caps.full(), Span { start: 0, end: 3 });
    assert_eq!(caps.group(1), Some(Span { start: 1, end: 2 }));
    assert!(!r.is_match(b"xabc", &b, &mut no_limit).unwrap());

    let mut r_esc = compile_bre(br"a\(b\)c", BreDialect::Expr, RegexFlags::default(), &b);
    assert!(!r_esc.is_match(b"abc", &b, &mut no_limit).unwrap());
    assert!(r_esc.is_match(b"a(b)c", &b, &mut no_limit).unwrap());

    let mut r_int = compile_bre(b"a{2}b", BreDialect::Expr, RegexFlags::default(), &b);
    assert_eq!(
        r_int.find_at(b"aab", 0, &b, &mut no_limit).unwrap(),
        Some(Span { start: 0, end: 3 })
    );
}

#[test]
fn sed_replace_style_and_posix_empty_skip() {
    let b = test_budget();
    let mut r = compile_bre(br"\([a-z]\+\)-\([0-9]\+\)", BreDialect::Sed, RegexFlags::default(), &b);
    let (out, changed) = r
        .replace_all(
            b"foo-12 bar-34",
            br"[\2:\1=&]",
            ReplaceStyle::Sed,
            None,
            EmptyAdvance::PosixSed,
            &b,
            &mut no_limit,
        )
        .unwrap();
    assert!(changed);
    assert_eq!(out.as_slice(), b"[12:foo=foo-12] [34:bar=bar-34]");

    // s/a*/X/g on "aba" -> "XbX"
    let mut r_empty = compile_bre(b"a*", BreDialect::Sed, RegexFlags::default(), &b);
    let (out2, changed2) = r_empty
        .replace_all(
            b"aba",
            b"X",
            ReplaceStyle::Sed,
            None,
            EmptyAdvance::PosixSed,
            &b,
            &mut no_limit,
        )
        .unwrap();
    assert!(changed2);
    assert_eq!(out2.as_slice(), b"XbX");
}

#[test]
fn dollar_replace_and_nth() {
    let b = test_budget();
    let mut r = compile_perl(b"(?P<k>[a-z]+)=([0-9]+)", RegexFlags::default(), &b);
    assert_eq!(r.group_index(b"k"), Some(1));
    let (out, changed) = r
        .replace_all(
            b"a=1 b=2 c=3",
            b"${k}:$2($$/$&)",
            ReplaceStyle::Dollar,
            Some(2),
            EmptyAdvance::Standard,
            &b,
            &mut no_limit,
        )
        .unwrap();
    assert!(changed);
    assert_eq!(out.as_slice(), b"a=1 b:2($/b=2) c=3");
}

#[test]
fn fixed_mode_multiline_flags() {
    let b = test_budget();
    let mut r = CompiledRegex::compile(
        b"foo\nbar",
        RegexMode::Fixed,
        RegexFlags::default(),
        CompileLimits::default(),
        &b,
    )
    .unwrap();
    assert!(r.is_match(b"xxfooyy", &b, &mut no_limit).unwrap());
    assert!(r.is_match(b"bar", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"baz", &b, &mut no_limit).unwrap());

    let mut r_empty = CompiledRegex::compile(
        b"",
        RegexMode::Fixed,
        RegexFlags::default(),
        CompileLimits::default(),
        &b,
    )
    .unwrap();
    assert_eq!(
        r_empty.find_at(b"anything", 0, &b, &mut no_limit).unwrap(),
        Some(Span { start: 0, end: 0 })
    );

    let mut r_w = CompiledRegex::compile(
        b"cat",
        RegexMode::Fixed,
        RegexFlags {
            whole_word: true,
            ..Default::default()
        },
        CompileLimits::default(),
        &b,
    )
    .unwrap();
    assert!(r_w.is_match(b"a cat!", &b, &mut no_limit).unwrap());
    assert!(!r_w.is_match(b"cats", &b, &mut no_limit).unwrap());
}

#[test]
fn posix_classes_and_word_boundaries() {
    let b = test_budget();
    let mut r = compile_ere(
        b"[[:alpha:]]+-[[:digit:]]+-[[:alnum:]]+-[[:xdigit:]]+",
        RegexFlags {
            line_regexp: true,
            ..Default::default()
        },
        &b,
    );
    assert!(r.is_match(b"abc-123-a1Z-1aF", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"abc-123-a1Z-1aG", &b, &mut no_limit).unwrap());

    let mut wb = compile_ere(b"[[:<:]]cat[[:>:]]", RegexFlags::default(), &b);
    assert!(wb.is_match(b"cat", &b, &mut no_limit).unwrap());
    assert!(wb.is_match(b"a cat!", &b, &mut no_limit).unwrap());
    assert!(!wb.is_match(b"cats", &b, &mut no_limit).unwrap());
    assert!(!wb.is_match(b"_cat", &b, &mut no_limit).unwrap());
}

#[test]
fn perl_features_keep_lazy_hex_octal_inline_flags() {
    let b = test_budget();
    let mut r = compile_perl(br"(?i:ab)\d+\K[a-z]+?", RegexFlags::default(), &b);
    assert_eq!(
        r.find_at(b"AB12xyz", 0, &b, &mut no_limit).unwrap(),
        Some(Span { start: 4, end: 5 })
    );

    let mut r2 = compile_perl(br"\Q(a+b)\E\x41\x{3b1}\101", RegexFlags::default(), &b);
    assert!(r2.is_match("(a+b)AαA".as_bytes(), &b, &mut no_limit).unwrap());

    let mut r3 = compile_perl(b"(?i:abc)-(?-i:DEF)-(?P<id>[0-9]+)", RegexFlags::default(), &b);
    assert!(r3.is_match(b"abc-DEF-7", &b, &mut no_limit).unwrap());
    assert!(!r3.is_match(b"ABC-def-42", &b, &mut no_limit).unwrap());
}

#[test]
fn unicode_ignore_case_and_astral() {
    let b = test_budget();
    let mut r = compile_ere(
        "café".as_bytes(),
        RegexFlags {
            ignore_case: true,
            ..Default::default()
        },
        &b,
    );
    assert!(r.is_match("CAFÉ".as_bytes(), &b, &mut no_limit).unwrap());
    assert!(r.is_match("café".as_bytes(), &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"cafe", &b, &mut no_limit).unwrap());

    let mut dot = compile_ere(
        b".",
        RegexFlags {
            line_regexp: true,
            ..Default::default()
        },
        &b,
    );
    assert!(dot.is_match("😀".as_bytes(), &b, &mut no_limit).unwrap());
    assert!(!dot.is_match(b"ab", &b, &mut no_limit).unwrap());
}

#[test]
fn syntax_errors_and_lookaround_rejected() {
    let b = test_budget();
    for pat in [b"a(?=b)".as_slice(), b"a(?!b)", b"(?<=a)b", b"(?<!a)b"] {
        let err = CompiledRegex::compile(pat, RegexMode::Perl, RegexFlags::default(), CompileLimits::default(), &b).err();
        assert_eq!(err, Some(RegexError::Syntax(SyntaxKind::LookaroundUnsupported)));
    }
    assert_eq!(
        CompiledRegex::compile(b"[", RegexMode::Basic(BreDialect::Grep), RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::UnclosedBracket))
    );
    assert_eq!(
        CompiledRegex::compile(b"(abc", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::UnclosedGroup))
    );
    assert_eq!(
        CompiledRegex::compile(b"[z-a]", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::InvalidCharacterRange))
    );
    assert_eq!(
        CompiledRegex::compile(b"[[:bogus:]a]", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::UnknownPosixClass))
    );
    assert_eq!(
        CompiledRegex::compile(br"\e", RegexMode::Perl, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::InvalidEscape))
    );
    assert_eq!(
        CompiledRegex::compile(br"\x4", RegexMode::Perl, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::InvalidEscape))
    );
}

#[test]
fn compile_caps_enforced() {
    let b = test_budget();
    assert_eq!(
        CompiledRegex::compile(b"a{1000}{1000}", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::CompileCap(CapKind::ProgramTooLarge { limit: MAX_PROGRAM_INSTS }))
    );
    assert_eq!(
        CompiledRegex::compile(b"a{1001}", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::CompileCap(CapKind::RepeatTooLarge { limit: MAX_REPEAT_EXPANSION }))
    );
    let mut deep = KBytes::new_in(&b);
    for _ in 0..201 {
        deep.try_push(b'(').unwrap();
    }
    deep.try_push(b'a').unwrap();
    for _ in 0..201 {
        deep.try_push(b')').unwrap();
    }
    assert_eq!(
        CompiledRegex::compile(deep.as_slice(), RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::CompileCap(CapKind::NestingTooDeep { limit: MAX_NESTING_DEPTH }))
    );
    let mut many = KBytes::new_in(&b);
    for _ in 0..101 {
        many.try_extend_from_slice(b"(a)").unwrap();
    }
    assert_eq!(
        CompiledRegex::compile(many.as_slice(), RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::CompileCap(CapKind::TooManyCaptures { limit: MAX_CAPTURE_GROUPS }))
    );
}

#[test]
fn prefilter_extraction() {
    let b = test_budget();
    let r1 = compile_ere(b"hello.*", RegexFlags::default(), &b);
    assert_eq!(r1.prefilter(), Prefilter::Exact(b"hello"));

    let r2 = compile_ere(b".*needle.*", RegexFlags::default(), &b);
    assert_eq!(r2.prefilter(), Prefilter::Substring(b"needle"));

    let r3 = compile_ere(b"a|b", RegexFlags::default(), &b);
    assert_eq!(r3.prefilter(), Prefilter::None);

    let r4 = compile_ere(
        b"hello.*",
        RegexFlags {
            ignore_case: true,
            ..Default::default()
        },
        &b,
    );
    assert_eq!(r4.prefilter(), Prefilter::None);
}

#[test]
fn s9_redos_traps_linear_time() {
    let b = test_budget();
    let mut hay50 = KBytes::with_capacity_in(50_001, &b).unwrap();
    for _ in 0..50_000 {
        hay50.try_push(b'a').unwrap();
    }
    hay50.try_push(b'!').unwrap();

    for pat in [b"(a+)+b".as_slice(), b"(a|a)*b", b"^([a-zA-Z]+)*$"] {
        let mut r = compile_ere(pat, RegexFlags::default(), &b);
        let mut units = 0u64;
        let mut work = |u| {
            units += u;
            Ok(())
        };
        assert!(!r.is_match(hay50.as_slice(), &b, &mut work).unwrap());
        assert!(units < 2000, "unexpected work units: {units}");
    }
    // Unanchored `([a-zA-Z]+)*$` matches empty at EOF in linear time as well.
    let mut r_unanch = compile_ere(b"([a-zA-Z]+)*$", RegexFlags::default(), &b);
    assert!(r_unanch.is_match(hay50.as_slice(), &b, &mut no_limit).unwrap());

    let mut hay20 = KBytes::with_capacity_in(20_001, &b).unwrap();
    for _ in 0..20_000 {
        hay20.try_push(b'a').unwrap();
    }
    hay20.try_push(b'!').unwrap();
    let mut rp = compile_perl(b"(.*a){25}b", RegexFlags::default(), &b);
    assert!(!rp.is_match(hay20.as_slice(), &b, &mut no_limit).unwrap());
}

#[test]
fn dfa_cache_cap_and_equivalence_on_state_explosion() {
    let b = test_budget();
    let mut hay = KBytes::with_capacity_in(65_536, &b).unwrap();
    let mut rng = 0x5345_4E54_4C38_415Au64;
    for _ in 0..65_536 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        hay.try_push(if (rng >> 33) & 1 == 0 { b'a' } else { b'b' }).unwrap();
    }
    // Non-matching suffix forces the DFA to scan all 64 KiB and repeatedly flush at 256 KiB.
    let mut r_miss = compile_ere(b"a[ab]{20}c", RegexFlags::default(), &b);
    let dfa_res = r_miss.is_match_dfa_only(hay.as_slice(), &b, &mut no_limit).unwrap();
    assert_eq!(dfa_res, Some(false));
    assert!(r_miss.dfa_cached_bytes() <= MAX_DFA_CACHE_BYTES);
    assert_eq!(
        r_miss.is_match_pikevm_only(hay.as_slice(), &b, &mut no_limit).unwrap(),
        false
    );

    // Matching scan on `a[ab]{20}`.
    let mut r_hit = compile_ere(b"a[ab]{20}", RegexFlags::default(), &b);
    assert_eq!(
        r_hit.is_match_dfa_only(hay.as_slice(), &b, &mut no_limit).unwrap(),
        Some(true)
    );
    assert!(r_hit.dfa_cached_bytes() <= MAX_DFA_CACHE_BYTES);
    assert_eq!(
        r_hit.is_match_pikevm_only(hay.as_slice(), &b, &mut no_limit).unwrap(),
        true
    );
}

#[test]
fn bounded_backreferences_and_fuel_limit() {
    let b = test_budget();
    // Default allow_backrefs: false rejects \1.
    assert_eq!(
        CompiledRegex::compile(b"(a)\\1", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::BackrefUnsupported(1)))
    );

    let flags = RegexFlags {
        allow_backrefs: true,
        ..Default::default()
    };
    let mut r = compile_ere(br"([a-z]+)-\1", flags, &b);
    let caps = r.captures_at(b"abc-abc", 0, &b, &mut no_limit).unwrap().unwrap();
    assert_eq!(caps.full(), Span { start: 0, end: 7 });
    assert_eq!(caps.group(1), Some(Span { start: 0, end: 3 }));
    assert!(!r.is_match(b"abc-def", &b, &mut no_limit).unwrap());

    for dialect in [BreDialect::Grep, BreDialect::Sed] {
        let mut rb = compile_bre(br"\(a*\)\1", dialect, flags, &b);
        assert!(rb.is_match(b"aaaa", &b, &mut no_limit).unwrap());
    }

    // Adversarial backref hits fuel cap cleanly.
    let mut adv = compile_ere(br"([a-z]+)\1+b", flags, &b);
    let mut hay = KBytes::with_capacity_in(501, &b).unwrap();
    for _ in 0..500 {
        hay.try_push(b'a').unwrap();
    }
    hay.try_push(b'!').unwrap();
    let mut used = 0u64;
    let mut capped = |u: u64| {
        used += u;
        if used > 5_000 {
            Err(())
        } else {
            Ok(())
        }
    };
    assert_eq!(
        adv.is_match(hay.as_slice(), &b, &mut capped),
        Err(MatchError::FuelExhausted)
    );
}

#[test]
fn native_seeded_fuzz_regex_dfa_pikevm_equivalence() {
    let b = test_budget();
    let mut rng = 0x5345_4E54_4C38_415Au64;
    let mut next_u32 = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (rng >> 32) as u32
    };
    let atoms: &[&[u8]] = &[
        b"a", b"b", b"c", b".", b"[ab]", b"[^a]", b"[[:digit:]]", b"\\w", b"\\d", b"\\s",
        b"(a|b)", b"(ab)", b"a*", b"b+", b"a?", b"^", b"$", b"\\b", b"a{1,2}", b"[a-c]",
        b"(", b")", b"[", b"\\", b"|", b"a{2,1}", b"[[:bogus:]]",
    ];
    let modes = [
        RegexMode::Extended,
        RegexMode::Basic(BreDialect::Grep),
        RegexMode::Basic(BreDialect::Sed),
        RegexMode::Basic(BreDialect::Expr),
        RegexMode::Perl,
        RegexMode::Fixed,
    ];

    for i in 0..1000u32 {
        let mut pat = KBytes::new_in(&b);
        let n_atoms = 1 + (next_u32() % 5);
        for _ in 0..n_atoms {
            let tok = atoms[(next_u32() as usize) % atoms.len()];
            pat.try_extend_from_slice(tok).unwrap();
        }
        let mode = modes[(i as usize) % modes.len()];
        let flags = RegexFlags {
            ignore_case: (next_u32() & 1) != 0,
            multiline: (next_u32() & 1) != 0,
            dot_all: (next_u32() & 1) != 0,
            whole_word: (next_u32() & 3) == 0,
            line_regexp: (next_u32() & 3) == 0,
            allow_backrefs: false,
            comma_first_interval: (next_u32() & 1) != 0,
        };
        let Ok(mut compiled) = CompiledRegex::compile(
            pat.as_slice(),
            mode,
            flags,
            CompileLimits::default(),
            &b,
        ) else {
            continue;
        };

        let mut hay = KBytes::new_in(&b);
        let hlen = (next_u32() % 24) as usize;
        let raw_mode = (i % 5) == 0;
        for _ in 0..hlen {
            if raw_mode {
                hay.try_push((next_u32() & 0xFF) as u8).unwrap();
            } else {
                let c = b"abc012 _\n-"[(next_u32() as usize) % 10];
                hay.try_push(c).unwrap();
            }
        }

        let pike = compiled
            .is_match_pikevm_only(hay.as_slice(), &b, &mut no_limit)
            .unwrap();
        if let Some(dfa) = compiled
            .is_match_dfa_only(hay.as_slice(), &b, &mut no_limit)
            .unwrap()
        {
            assert_eq!(
                dfa, pike,
                "DFA/PikeVM mismatch on iter={i} pat={:?} hay={:?}",
                pat.as_slice(),
                hay.as_slice()
            );
        }
        let _ = compiled.find_all(hay.as_slice(), EmptyAdvance::Standard, &b, &mut no_limit).unwrap();
    }
}

#[test]
fn additional_regex_unit_checks() {
    let b = test_budget();
    // Multiline ^ and $
    let mut ml = compile_ere(
        b"^bar$",
        RegexFlags {
            multiline: true,
            ..Default::default()
        },
        &b,
    );
    assert_eq!(
        ml.find_at(b"foo\nbar\nbaz", 0, &b, &mut no_limit).unwrap(),
        Some(Span { start: 4, end: 7 })
    );
    // Dot-all
    let mut ds = compile_ere(
        b"a.b",
        RegexFlags {
            dot_all: true,
            ..Default::default()
        },
        &b,
    );
    assert!(ds.is_match(b"a\nb", &b, &mut no_limit).unwrap());

    // Bracket edge cases (matches oracle l8a/bracket-edge-cases)
    let mut b1 = compile_ere(b"[]a-]", RegexFlags::default(), &b);
    assert!(b1.is_match(b"]", &b, &mut no_limit).unwrap());
    assert!(b1.is_match(b"a", &b, &mut no_limit).unwrap());
    assert!(b1.is_match(b"-", &b, &mut no_limit).unwrap());
    assert!(!b1.is_match(b"b", &b, &mut no_limit).unwrap());

    let mut b2 = compile_ere(b"[^]a]", RegexFlags::default(), &b);
    assert!(!b2.is_match(b"]", &b, &mut no_limit).unwrap());
    assert!(!b2.is_match(b"a", &b, &mut no_limit).unwrap());
    assert!(b2.is_match(b"b", &b, &mut no_limit).unwrap());

    let mut b3 = compile_ere(b"[!a]", RegexFlags::default(), &b);
    assert!(b3.is_match(b"!", &b, &mut no_limit).unwrap());
    assert!(!b3.is_match(b"a", &b, &mut no_limit).unwrap());
    assert!(b3.is_match(b"b", &b, &mut no_limit).unwrap());
}

#[test]
fn posix_all_14_classes_regex() {
    let b = test_budget();
    let cases: &[(&[u8], &[u8], &[u8])] = &[
        (b"^[[:alnum:]]$", b"9", b"-"),
        (b"^[[:alpha:]]$", b"Z", b"5"),
        (b"^[[:ascii:]]$", b"\x7f", "é".as_bytes()),
        (b"^[[:blank:]]$", b"\t", b"\n"),
        (b"^[[:cntrl:]]$", b"\x01", b"a"),
        (b"^[[:digit:]]$", b"7", b"a"),
        (b"^[[:graph:]]$", b"~", b" "),
        (b"^[[:lower:]]$", b"z", b"Z"),
        (b"^[[:print:]]$", b" ", b"\t"),
        (b"^[[:punct:]]$", b"!", b"a"),
        (b"^[[:space:]]$", b"\x0b", b"a"),
        (b"^[[:upper:]]$", b"A", b"a"),
        (b"^[[:word:]]$", b"_", b"-"),
        (b"^[[:xdigit:]]$", b"f", b"g"),
    ];
    for &(pat, hit, miss) in cases {
        let mut r = compile_ere(pat, RegexFlags::default(), &b);
        assert!(r.is_match(hit, &b, &mut no_limit).unwrap(), "expected hit on {:?}", pat);
        assert!(!r.is_match(miss, &b, &mut no_limit).unwrap(), "expected miss on {:?}", pat);
    }
}

#[test]
fn perl_named_groups_angle_and_errors() {
    let b = test_budget();
    let mut r = compile_perl(b"(?<first>[a-z]+)-(?<second>[0-9]+)", RegexFlags::default(), &b);
    assert_eq!(r.capture_count(), 2);
    assert_eq!(r.group_index(b"first"), Some(1));
    assert_eq!(r.group_index(b"second"), Some(2));
    assert_eq!(r.group_index(b"missing"), None);
    let caps = r.captures_at(b"foo-42", 0, &b, &mut no_limit).unwrap().unwrap();
    assert_eq!(caps.len(), 3);
    assert!(!caps.is_empty());
    assert_eq!(caps.group(1), Some(Span { start: 0, end: 3 }));
    assert_eq!(caps.group(2), Some(Span { start: 4, end: 6 }));

    assert_eq!(
        CompiledRegex::compile(b"(?<x>a)(?<x>b)", RegexMode::Perl, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::DuplicateCaptureName))
    );
    assert_eq!(
        CompiledRegex::compile(b"(?<1bad>a)", RegexMode::Perl, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::InvalidCaptureName))
    );
}

#[test]
fn perl_octal_formfeed_vtab_and_noncap() {
    let b = test_budget();
    let mut r = compile_perl(br"(?:a|b)\0\f\v\077", RegexFlags::default(), &b);
    assert_eq!(r.capture_count(), 0);
    assert!(r.is_match(b"b\x00\x0c\x0b?", &b, &mut no_limit).unwrap());
}

#[test]
fn perl_inline_flag_statement_scoping() {
    let b = test_budget();
    let mut r = compile_perl(b"((?i)abc)-def", RegexFlags::default(), &b);
    assert!(r.is_match(b"ABC-def", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"ABC-DEF", &b, &mut no_limit).unwrap());
}

#[test]
fn perl_lazy_quantifiers_all_forms() {
    let b = test_budget();
    let mut r1 = compile_perl(b"a*?", RegexFlags::default(), &b);
    assert_eq!(r1.find_at(b"aaa", 0, &b, &mut no_limit).unwrap(), Some(Span { start: 0, end: 0 }));

    let mut r2 = compile_perl(b"a+?", RegexFlags::default(), &b);
    assert_eq!(r2.find_at(b"aaa", 0, &b, &mut no_limit).unwrap(), Some(Span { start: 0, end: 1 }));

    let mut r3 = compile_perl(b"a??", RegexFlags::default(), &b);
    assert_eq!(r3.find_at(b"a", 0, &b, &mut no_limit).unwrap(), Some(Span { start: 0, end: 0 }));

    let mut r4 = compile_perl(b"a{2,4}?", RegexFlags::default(), &b);
    assert_eq!(r4.find_at(b"aaaa", 0, &b, &mut no_limit).unwrap(), Some(Span { start: 0, end: 2 }));
}

#[test]
fn syntax_errors_complete_matrix() {
    let b = test_budget();
    assert_eq!(
        CompiledRegex::compile(b"a)", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::UnmatchedCloseParen))
    );
    assert_eq!(
        CompiledRegex::compile(b"*a", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::DanglingQuantifier))
    );
    assert_eq!(
        CompiledRegex::compile(b"a{3,1}", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::InvalidRepeatRange))
    );
    assert_eq!(
        CompiledRegex::compile(b"abc\\", RegexMode::Extended, RegexFlags::default(), CompileLimits::default(), &b).err(),
        Some(RegexError::Syntax(SyntaxKind::TrailingBackslash))
    );
    assert_eq!(
        CompiledRegex::compile(
            br"(a)\2",
            RegexMode::Extended,
            RegexFlags { allow_backrefs: true, ..Default::default() },
            CompileLimits::default(),
            &b
        ).err(),
        Some(RegexError::Syntax(SyntaxKind::InvalidBackref(2)))
    );
}

#[test]
fn empty_advance_policies_find_all() {
    let b = test_budget();
    let mut r = compile_ere(b"a*", RegexFlags::default(), &b);
    let std_spans = r.find_all(b"aba", EmptyAdvance::Standard, &b, &mut no_limit).unwrap();
    assert_eq!(std_spans.as_slice(), &[
        Span { start: 0, end: 1 },
        Span { start: 1, end: 1 },
        Span { start: 2, end: 3 },
        Span { start: 3, end: 3 },
    ]);

    let sed_spans = r.find_all(b"aba", EmptyAdvance::PosixSed, &b, &mut no_limit).unwrap();
    assert_eq!(sed_spans.as_slice(), &[
        Span { start: 0, end: 1 },
        Span { start: 2, end: 3 },
    ]);

    let sh_spans = r.find_all(b"aba", EmptyAdvance::ShellExpansion, &b, &mut no_limit).unwrap();
    assert_eq!(sh_spans.as_slice(), &[
        Span { start: 0, end: 1 },
        Span { start: 1, end: 1 },
        Span { start: 2, end: 3 },
    ]);
}

#[test]
fn replace_style_literal_no_interpretation() {
    let b = test_budget();
    let mut r = compile_ere(b"(a+)", RegexFlags::default(), &b);
    let (out, changed) = r
        .replace_all(b"aa b a", br"$1&\1", ReplaceStyle::Literal, None, EmptyAdvance::Standard, &b, &mut no_limit)
        .unwrap();
    assert!(changed);
    assert_eq!(out.as_slice(), br"$1&\1 b $1&\1");
}

#[test]
fn replace_no_match_returns_unchanged() {
    let b = test_budget();
    let mut r = compile_ere(b"xyz", RegexFlags::default(), &b);
    let (out, changed) = r
        .replace_all(b"abc", b"Q", ReplaceStyle::Sed, None, EmptyAdvance::Standard, &b, &mut no_limit)
        .unwrap();
    assert!(!changed);
    assert_eq!(out.as_slice(), b"abc");
}

#[test]
fn bre_sed_escapes_tab_newline_cr_and_alt() {
    let b = test_budget();
    let mut r = compile_bre(br"a\tb\|c\rd", BreDialect::Sed, RegexFlags::default(), &b);
    assert!(r.is_match(b"a\tb", &b, &mut no_limit).unwrap());
    assert!(r.is_match(b"c\rd", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"ab", &b, &mut no_limit).unwrap());
}

#[test]
fn word_boundary_and_non_word_boundary() {
    let b = test_budget();
    let mut wb = compile_ere(br"\bcat\b", RegexFlags::default(), &b);
    assert!(wb.is_match(b"cat", &b, &mut no_limit).unwrap());
    assert!(wb.is_match(b"the cat.", &b, &mut no_limit).unwrap());
    assert!(!wb.is_match(b"cats", &b, &mut no_limit).unwrap());

    let mut nwb = compile_ere(br"\Bcat\B", RegexFlags::default(), &b);
    assert!(nwb.is_match(b"scats", &b, &mut no_limit).unwrap());
    assert!(!nwb.is_match(b"cat", &b, &mut no_limit).unwrap());
}

#[test]
fn whole_word_and_line_regexp_flags() {
    let b = test_budget();
    let mut r = compile_ere(
        b"foo|bar",
        RegexFlags { whole_word: true, ..Default::default() },
        &b,
    );
    assert_eq!(r.find_at(b"foobar bar", 0, &b, &mut no_limit).unwrap(), Some(Span { start: 7, end: 10 }));

    let mut r2 = compile_ere(
        b"bar",
        RegexFlags { line_regexp: true, multiline: true, ..Default::default() },
        &b,
    );
    assert_eq!(r2.find_at(b"foo\nbar\nbaz", 0, &b, &mut no_limit).unwrap(), Some(Span { start: 4, end: 7 }));
}

#[test]
fn backref_case_insensitive_and_empty_capture() {
    let b = test_budget();
    let flags = RegexFlags {
        ignore_case: true,
        allow_backrefs: true,
        ..Default::default()
    };
    let mut r = compile_ere(br"([a-z]+)-\1", flags, &b);
    assert!(r.is_match(b"AbC-aBc", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"AbC-xYz", &b, &mut no_limit).unwrap());

    let mut r2 = compile_ere(br"(a*)\1", flags, &b);
    assert!(r2.is_match(b"", &b, &mut no_limit).unwrap());
}

#[test]
fn empty_alternation_branches_ere() {
    let b = test_budget();
    let mut r = compile_ere(b"(|a)", RegexFlags::default(), &b);
    assert_eq!(r.find_at(b"a", 0, &b, &mut no_limit).unwrap(), Some(Span { start: 0, end: 0 }));

    let mut r2 = compile_ere(b"(a|)", RegexFlags::default(), &b);
    assert_eq!(r2.find_at(b"a", 0, &b, &mut no_limit).unwrap(), Some(Span { start: 0, end: 1 }));
}

#[test]
fn malformed_brace_literal_in_ere() {
    let b = test_budget();
    let mut r = compile_ere(b"a{b}", RegexFlags::default(), &b);
    assert!(r.is_match(b"a{b}", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"ab", &b, &mut no_limit).unwrap());
}

#[test]
fn fixed_mode_ignore_case_and_line_regexp() {
    let b = test_budget();
    let mut r = CompiledRegex::compile(
        b"Hello\nWorld",
        RegexMode::Fixed,
        RegexFlags { ignore_case: true, line_regexp: true, ..Default::default() },
        CompileLimits::default(),
        &b,
    )
    .unwrap();
    assert!(r.is_match(b"HELLO", &b, &mut no_limit).unwrap());
    assert!(r.is_match(b"world", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"hello!", &b, &mut no_limit).unwrap());
}

#[test]
fn raw_invalid_utf8_bytes_handled_without_panic() {
    let b = test_budget();
    let mut r = compile_ere(b"a.b", RegexFlags::default(), &b);
    assert!(r.is_match(b"a\xffb", &b, &mut no_limit).unwrap());
    let mut r2 = compile_ere(b"[^a]", RegexFlags::default(), &b);
    assert!(r2.is_match(b"\xfe", &b, &mut no_limit).unwrap());
}

#[test]
fn perl_unclosed_quote_runs_to_end() {
    let b = test_budget();
    let mut r = compile_perl(br"\Q[a+b](c*)", RegexFlags::default(), &b);
    assert!(r.is_match(b"[a+b](c*)", &b, &mut no_limit).unwrap());
    assert!(!r.is_match(b"ac", &b, &mut no_limit).unwrap());
}
