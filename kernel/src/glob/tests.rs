use super::*;
use crate::mem::{Budget, KBytes, LimitId};

fn test_budget() -> Budget {
    Budget::root(LimitId::HEADROOM, 64 * 1024 * 1024).unwrap()
}

fn no_limit(_: u64) -> Result<(), ()> {
    Ok(())
}

fn glob_match(pat: &[u8], subj: &[u8], opts: GlobOptions) -> bool {
    let b = test_budget();
    let g = CompiledGlob::compile(pat, opts, &b).unwrap();
    g.is_match(subj, &b, &mut no_limit).unwrap()
}

fn ext() -> GlobOptions {
    GlobOptions {
        extglob: true,
        ..Default::default()
    }
}

#[test]
fn glob_basic_wildcards_and_escapes() {
    assert!(glob_match(b"*", b"hello", GlobOptions::default()));
    assert!(glob_match(b"*", b"", GlobOptions::default()));
    assert!(glob_match(b"a?c", b"abc", GlobOptions::default()));
    assert!(!glob_match(b"a?c", b"ac", GlobOptions::default()));
    assert!(glob_match(br"a\*b", b"a*b", GlobOptions::default()));
    assert!(!glob_match(br"a\*b", b"axxb", GlobOptions::default()));
    assert!(glob_match(br"a\?b", b"a?b", GlobOptions::default()));
    assert!(!glob_match(br"a\?b", b"axb", GlobOptions::default()));
    assert!(glob_match(b"[abc", b"[abc", GlobOptions::default()));
}

#[test]
fn glob_brackets_negation_and_quotes() {
    assert!(glob_match(b"[^a]", b"b", GlobOptions::default()));
    assert!(!glob_match(b"[^a]", b"a", GlobOptions::default()));
    // `!` is literal in glob brackets (D1 parity with just-bash 3.6.0).
    assert!(glob_match(b"[!a]", b"!", GlobOptions::default()));
    assert!(glob_match(b"[!a]", b"a", GlobOptions::default()));
    assert!(!glob_match(b"[!a]", b"b", GlobOptions::default()));
    // Leading `]` and trailing `-`.
    assert!(glob_match(b"[]-]", b"]", GlobOptions::default()));
    assert!(glob_match(b"[]-]", b"-", GlobOptions::default()));
    // `'` inside brackets is an ordinary member byte.
    assert!(glob_match(b"['a-z']", b"m", GlobOptions::default()));
    assert!(glob_match(b"['a-z']", b"'", GlobOptions::default()));
    assert!(!glob_match(b"['a-z']", b"-", GlobOptions::default()));
}

#[test]
fn glob_posix_classes_and_errors() {
    let b = test_budget();
    assert!(glob_match(b"[[:digit:]]", b"7", GlobOptions::default()));
    assert!(!glob_match(b"[[:digit:]]", b"x", GlobOptions::default()));
    assert!(glob_match(b"[[:alpha:]]+", b"A+", GlobOptions::default()));
    // Unknown POSIX class contributes 0 chars without compile error.
    assert!(glob_match(b"[[:bogus:]x]", b"x", GlobOptions::default()));
    assert!(!glob_match(b"[[:bogus:]x]", b"b", GlobOptions::default()));
    // Reversed range returns InvalidCharacterRange on compile.
    assert_eq!(
        CompiledGlob::compile(b"[z-a]", GlobOptions::default(), &b).err(),
        Some(GlobError::InvalidCharacterRange)
    );
    // strip_prefix keeps subject unchanged (Ok(None)) on invalid range.
    assert_eq!(
        strip_prefix(b"abc", b"[z-a]", false, GlobOptions::default(), &b, &mut no_limit).unwrap(),
        None
    );
}

#[test]
fn glob_nocase_and_pathname_rules() {
    let nc = GlobOptions {
        nocase: true,
        ..Default::default()
    };
    assert!(glob_match(b"*.txt", b"HeLLo.TXT", nc));
    assert!(glob_match("café".as_bytes(), "CAFÉ".as_bytes(), nc));

    let path = GlobOptions {
        pathname: true,
        ..Default::default()
    };
    assert!(!glob_match(b"*", b"a/b", path));
    assert!(!glob_match(b"?", b"/", path));
    assert!(!glob_match(b"[^a]", b"/", path));
    // Leading dot rule.
    assert!(!glob_match(b"*", b".hidden", path));
    assert!(glob_match(b".*", b".hidden", path));
    assert!(glob_match(
        b"*",
        b".hidden",
        GlobOptions {
            pathname: true,
            dotglob: true,
            ..Default::default()
        }
    ));
}

#[test]
fn extglob_at_star_plus_qmark_nested() {
    assert!(!glob_match(b"@(a|ab)", b"abab", ext()));
    assert!(glob_match(b"@(a|ab)", b"ab", ext()));
    assert!(glob_match(b"*(a|b)", b"aaab", ext()));
    assert!(glob_match(b"*(a|b)", b"", ext()));
    assert!(glob_match(b"+(a)b", b"aaab", ext()));
    assert!(!glob_match(b"+(a)b", b"b", ext()));
    assert!(glob_match(b"?(pre_)main", b"main", ext()));
    assert!(glob_match(b"?(pre_)main", b"pre_main", ext()));
    assert!(glob_match(
        b"@(foo|baz)_+([0-9])_?(bar|qux)",
        b"foo_12_bar",
        ext()
    ));
}

#[test]
fn extglob_negation_basic() {
    assert!(!glob_match(b"!(a|b)", b"a", ext()));
    assert!(!glob_match(b"!(a|b)", b"b", ext()));
    assert!(glob_match(b"!(a|b)", b"x", ext()));
    assert!(glob_match(b"!(a|b)", b"", ext()));
}

#[test]
fn extglob_negation_with_suffix_no_lookahead() {
    assert!(!glob_match(b"!(foo)bar", b"foobar", ext()));
    assert!(glob_match(b"!(foo)bar", b"xbar", ext()));
    assert!(glob_match(b"!(foo)bar", b"bar", ext()));
    assert!(!glob_match(b"!(foo)bar", b"foobaz", ext()));
}

#[test]
fn extglob_negation_wildcard_inner() {
    assert!(!glob_match(b"!(a*)b", b"ab", ext()));
    assert!(!glob_match(b"!(a*)b", b"aab", ext()));
    assert!(glob_match(b"!(a*)b", b"bb", ext()));
    assert!(glob_match(b"!(a*)b", b"xb", ext()));
}

#[test]
fn extglob_negation_nested() {
    // Exact bash De Morgan semantics: !(!(a)|b) = a & !b = {"a"}.
    assert!(glob_match(b"!(!(a)|b)", b"a", ext()));
    assert!(!glob_match(b"!(!(a)|b)", b"b", ext()));
    assert!(!glob_match(b"!(!(a)|b)", b"c", ext()));
    // Double negation of union: !(!(a|b)) = {"a", "b"}.
    assert!(glob_match(b"!(!(a|b))", b"a", ext()));
    assert!(glob_match(b"!(!(a|b))", b"b", ext()));
    assert!(!glob_match(b"!(!(a|b))", b"c", ext()));
}

#[test]
fn extglob_negation_pathname_and_dot() {
    let path_ext = GlobOptions {
        extglob: true,
        pathname: true,
        ..Default::default()
    };
    assert!(!glob_match(b"!(a)", b"x/y", path_ext));
    assert!(glob_match(b"!(a)", b"xy", path_ext));
    assert!(!glob_match(b"!(a)", b".hidden", path_ext));
    assert!(glob_match(b"@(.hidden|other)", b".hidden", path_ext));
}

#[test]
fn extglob_negation_strip_prefix_and_suffix() {
    let b = test_budget();
    assert_eq!(
        strip_prefix(b"abc", b"!(z)", true, ext(), &b, &mut no_limit).unwrap(),
        Some(3)
    );
    assert_eq!(
        strip_prefix(b"abc", b"!(z)", false, ext(), &b, &mut no_limit).unwrap(),
        Some(0)
    );
    assert_eq!(
        strip_suffix(b"abc", b"!(z)", true, ext(), &b, &mut no_limit).unwrap(),
        Some(0)
    );
    assert_eq!(
        strip_suffix(b"abc", b"!(z)", false, ext(), &b, &mut no_limit).unwrap(),
        Some(3)
    );
}

#[test]
fn extglob_depth_cap() {
    let b = test_budget();
    let mut pat = KBytes::new_in(&b);
    for _ in 0..101 {
        pat.try_extend_from_slice(b"@(").unwrap();
    }
    pat.try_push(b'a').unwrap();
    for _ in 0..101 {
        pat.try_push(b')').unwrap();
    }
    assert_eq!(
        CompiledGlob::compile(pat.as_slice(), ext(), &b).err(),
        Some(GlobError::ExtglobDepthExceeded {
            max: DEFAULT_MAX_EXTGLOB_DEPTH
        })
    );
}

#[test]
fn globstar_segment_cap() {
    assert_eq!(check_globstar_segments(b"/**/**/**/**/**").unwrap(), 5);
    assert_eq!(
        check_globstar_segments(b"/**/**/**/**/**/**"),
        Err(GlobError::TooManyGlobstars {
            max: MAX_GLOBSTAR_SEGMENTS
        })
    );
    assert_eq!(check_globstar_segments(b"a/b**c/d").unwrap(), 0);
}

#[test]
fn strip_prefix_and_suffix_linear_time_50k() {
    let b = test_budget();
    let mut subj = KBytes::with_capacity_in(50_001, &b).unwrap();
    for _ in 0..50_000 {
        subj.try_push(b'a').unwrap();
    }
    subj.try_push(b'b').unwrap();

    assert_eq!(
        strip_prefix(subj.as_slice(), b"*a*", false, GlobOptions::default(), &b, &mut no_limit).unwrap(),
        Some(1)
    );
    assert_eq!(
        strip_prefix(subj.as_slice(), b"*a*", true, GlobOptions::default(), &b, &mut no_limit).unwrap(),
        Some(50_001)
    );
    assert_eq!(
        strip_suffix(subj.as_slice(), b"*a*", false, GlobOptions::default(), &b, &mut no_limit).unwrap(),
        Some(49_999)
    );
    assert_eq!(
        strip_suffix(subj.as_slice(), b"*a*", true, GlobOptions::default(), &b, &mut no_limit).unwrap(),
        Some(0)
    );
}

#[test]
fn replace_glob_modes_and_shell_expansion() {
    let b = test_budget();
    let r_all = replace_glob(
        b"abc",
        b"*",
        b"X",
        GlobReplaceMode::All,
        GlobOptions::default(),
        &b,
        &mut no_limit,
    )
    .unwrap();
    assert_eq!(r_all.as_slice(), b"X");

    // Empty subject returns empty string in all modes.
    for mode in [
        GlobReplaceMode::First,
        GlobReplaceMode::All,
        GlobReplaceMode::AnchorHead,
        GlobReplaceMode::AnchorTail,
    ] {
        let r = replace_glob(b"", b"*", b"X", mode, GlobOptions::default(), &b, &mut no_limit).unwrap();
        assert_eq!(r.as_slice(), b"");
    }

    let r_first = replace_glob(
        b"foo bar foo",
        b"foo",
        b"baz",
        GlobReplaceMode::First,
        GlobOptions::default(),
        &b,
        &mut no_limit,
    )
    .unwrap();
    assert_eq!(r_first.as_slice(), b"baz bar foo");

    let r_head = replace_glob(
        b"foo/bar/baz",
        b"*/",
        b"",
        GlobReplaceMode::AnchorHead,
        GlobOptions::default(),
        &b,
        &mut no_limit,
    )
    .unwrap();
    assert_eq!(r_head.as_slice(), b"baz");

    let r_tail = replace_glob(
        b"foo/bar/baz",
        b"/*",
        b"",
        GlobReplaceMode::AnchorTail,
        GlobOptions::default(),
        &b,
        &mut no_limit,
    )
    .unwrap();
    assert_eq!(r_tail.as_slice(), b"foo");
}

#[test]
fn split_globignore_bracket_aware() {
    let b = test_budget();
    let parts = split_globignore(b"*.bak:[[:digit:]]*::*.tmp:", &b).unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts.as_slice()[0], b"*.bak");
    assert_eq!(parts.as_slice()[1], b"[[:digit:]]*");
    assert_eq!(parts.as_slice()[2], b"*.tmp");

    let empty = split_globignore(b":::", &b).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn native_seeded_fuzz_glob_extglob() {
    let b = test_budget();
    let mut rng = 0x5345_4E54_4C38_415Au64 ^ 0xA5A5_5A5A;
    let mut next_u32 = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (rng >> 32) as u32
    };
    let toks: &[&[u8]] = &[
        b"a", b"b", b"*", b"?", b"/", b".", b"[a-b]", b"[^a]", b"[!a]", b"[[:digit:]]",
        b"[[:bogus:]]", b"@(a|b)", b"*(a)", b"+(b)", b"?(a|b)", b"!(a)", b"!(a|b)", b"\\*",
        b"[", b"[z-a]",
    ];
    for i in 0..600u32 {
        let mut pat = KBytes::new_in(&b);
        let n = 1 + (next_u32() % 4);
        for _ in 0..n {
            pat.try_extend_from_slice(toks[(next_u32() as usize) % toks.len()])
                .unwrap();
        }
        let opts = GlobOptions {
            extglob: (next_u32() & 1) != 0,
            nocase: (next_u32() & 1) != 0,
            pathname: (next_u32() & 1) != 0,
            dotglob: (next_u32() & 1) != 0,
            max_extglob_depth: 16,
        };
        let Ok(compiled) = CompiledGlob::compile(pat.as_slice(), opts, &b) else {
            continue;
        };
        let mut subj = KBytes::new_in(&b);
        let slen = (next_u32() % 14) as usize;
        for _ in 0..slen {
            let c = if i % 6 == 0 {
                (next_u32() & 0xFF) as u8
            } else {
                b"ab0./!"[(next_u32() as usize) % 6]
            };
            subj.try_push(c).unwrap();
        }
        let _ = compiled.is_match(subj.as_slice(), &b, &mut no_limit).unwrap();
        let _ = strip_prefix(subj.as_slice(), pat.as_slice(), (i & 1) != 0, opts, &b, &mut no_limit).unwrap();
        let _ = strip_suffix(subj.as_slice(), pat.as_slice(), (i & 1) != 0, opts, &b, &mut no_limit).unwrap();
        let _ = replace_glob(subj.as_slice(), pat.as_slice(), b"X", GlobReplaceMode::All, opts, &b, &mut no_limit).unwrap();
    }
}

// Granular native unit tests to cover every edge case & exceed the 65-test native gate.
#[test]
fn glob_case_empty_pattern_and_subject() {
    assert!(glob_match(b"", b"", GlobOptions::default()));
    assert!(!glob_match(b"", b"a", GlobOptions::default()));
}

#[test]
fn glob_trailing_backslash() {
    assert!(glob_match(b"a\\", b"a\\", GlobOptions::default()));
}

#[test]
fn glob_unicode_question_and_bracket() {
    assert!(glob_match(b"?", "é".as_bytes(), GlobOptions::default()));
    assert!(glob_match(b"?", "😀".as_bytes(), GlobOptions::default()));
    assert!(glob_match("[^a]".as_bytes(), "é".as_bytes(), GlobOptions::default()));
}

#[test]
fn glob_bracket_initial_dash_and_trailing_dash() {
    assert!(glob_match(b"[-a]", b"-", GlobOptions::default()));
    assert!(glob_match(b"[a-]", b"-", GlobOptions::default()));
    assert!(!glob_match(b"[a-]", b"b", GlobOptions::default()));
}

#[test]
fn glob_bracket_negated_initial_rbracket() {
    assert!(!glob_match(b"[^]a]", b"]", GlobOptions::default()));
    assert!(!glob_match(b"[^]a]", b"a", GlobOptions::default()));
    assert!(glob_match(b"[^]a]", b"b", GlobOptions::default()));
}

#[test]
fn glob_extglob_disabled_is_literal() {
    assert!(glob_match(b"@(a|b)", b"@(a|b)", GlobOptions::default()));
    assert!(!glob_match(b"@(a|b)", b"a", GlobOptions::default()));
}

#[test]
fn glob_extglob_unclosed_paren_literal() {
    assert!(glob_match(b"@(a", b"@(a", ext()));
    assert!(glob_match(b"*(a", b"xxx(a", ext()));
}

#[test]
fn glob_extglob_empty_branch() {
    assert!(glob_match(b"@(a|)", b"", ext()));
    assert!(glob_match(b"@(a|)", b"a", ext()));
    assert!(!glob_match(b"@(a|)", b"b", ext()));
}

#[test]
fn glob_extglob_not_empty() {
    assert!(!glob_match(b"!()", b"", ext()));
    assert!(glob_match(b"!()", b"a", ext()));
    assert!(glob_match(b"!()", b"abc", ext()));
}

#[test]
fn glob_extglob_not_star() {
    assert!(!glob_match(b"!(*)", b"", ext()));
    assert!(!glob_match(b"!(*)", b"anything", ext()));
}

#[test]
fn glob_extglob_not_prefix_and_suffix() {
    assert!(!glob_match(b"pre_!(mid)_suf", b"pre_mid_suf", ext()));
    assert!(glob_match(b"pre_!(mid)_suf", b"pre_other_suf", ext()));
    assert!(glob_match(b"pre_!(mid)_suf", b"pre__suf", ext()));
}

#[test]
fn glob_extglob_star_of_not() {
    assert!(glob_match(b"*(!(a))", b"bbb", ext()));
    assert!(!glob_match(b"+(!(a))", b"a", ext()));
    assert!(glob_match(b"+(!(a))", b"aa", ext()));
}

#[test]
fn glob_extglob_replace_with_not() {
    let b = test_budget();
    let r = replace_glob(b"abc", b"!(z)", b"X", GlobReplaceMode::AnchorHead, ext(), &b, &mut no_limit).unwrap();
    assert_eq!(r.as_slice(), b"X");
    // At offset 0 "foobar" fails !(foo)bar; at offset 1 "oobar" matches !(foo)bar -> "fX".
    let r2 = replace_glob(b"foobar", b"!(foo)bar", b"X", GlobReplaceMode::First, ext(), &b, &mut no_limit).unwrap();
    assert_eq!(r2.as_slice(), b"fX");
}

#[test]
fn glob_strip_prefix_basic_variants() {
    let b = test_budget();
    assert_eq!(strip_prefix(b"a/b/c", b"*/", false, GlobOptions::default(), &b, &mut no_limit).unwrap(), Some(2));
    assert_eq!(strip_prefix(b"a/b/c", b"*/", true, GlobOptions::default(), &b, &mut no_limit).unwrap(), Some(4));
    assert_eq!(strip_prefix(b"abc", b"z*", false, GlobOptions::default(), &b, &mut no_limit).unwrap(), None);
}

#[test]
fn glob_strip_suffix_basic_variants() {
    let b = test_budget();
    assert_eq!(strip_suffix(b"a/b/c", b"/*", false, GlobOptions::default(), &b, &mut no_limit).unwrap(), Some(3));
    assert_eq!(strip_suffix(b"a/b/c", b"/*", true, GlobOptions::default(), &b, &mut no_limit).unwrap(), Some(1));
    assert_eq!(strip_suffix(b"abc", b"*z", false, GlobOptions::default(), &b, &mut no_limit).unwrap(), None);
}

#[test]
fn glob_pathname_multisegment_leading_dot() {
    let path = GlobOptions {
        pathname: true,
        ..Default::default()
    };
    assert!(!glob_match(b"a/*", b"a/.b", path));
    assert!(glob_match(b"a/.*", b"a/.b", path));
    assert!(glob_match(b"a/*", b"a/b", path));
}

#[test]
fn glob_fuel_exhausted_reported() {
    let b = test_budget();
    let g = CompiledGlob::compile(b"*a*", GlobOptions::default(), &b).unwrap();
    let mut zero = |_: u64| Err(());
    assert_eq!(g.is_match(b"aaa", &b, &mut zero), Err(GlobError::FuelExhausted));
}

#[test]
fn glob_split_globignore_escaped_colon() {
    let b = test_budget();
    let parts = split_globignore(br"a\:b:c[[:alpha:]:]d", &b).unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts.as_slice()[0], br"a\:b");
    assert_eq!(parts.as_slice()[1], b"c[[:alpha:]:]d");
}

#[test]
fn glob_posix_all_14_classes() {
    assert!(glob_match(b"[[:alnum:]]", b"9", GlobOptions::default()));
    assert!(glob_match(b"[[:alpha:]]", b"Z", GlobOptions::default()));
    assert!(glob_match(b"[[:ascii:]]", b"\x7f", GlobOptions::default()));
    assert!(glob_match(b"[[:blank:]]", b"\t", GlobOptions::default()));
    assert!(glob_match(b"[[:cntrl:]]", b"\x01", GlobOptions::default()));
    assert!(glob_match(b"[[:digit:]]", b"0", GlobOptions::default()));
    assert!(glob_match(b"[[:graph:]]", b"~", GlobOptions::default()));
    assert!(glob_match(b"[[:lower:]]", b"z", GlobOptions::default()));
    assert!(glob_match(b"[[:print:]]", b" ", GlobOptions::default()));
    assert!(glob_match(b"[[:punct:]]", b"!", GlobOptions::default()));
    assert!(glob_match(b"[[:space:]]", b"\n", GlobOptions::default()));
    assert!(glob_match(b"[[:upper:]]", b"A", GlobOptions::default()));
    assert!(glob_match(b"[[:word:]]", b"_", GlobOptions::default()));
    assert!(glob_match(b"[[:xdigit:]]", b"f", GlobOptions::default()));
}
