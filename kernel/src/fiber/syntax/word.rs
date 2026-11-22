//! Word, parameter expansion (all 15 forms), brace, tilde, glob, and array literal sub-parsers.
use super::arith::parse_arith;
use super::ast::{
    BraceItem, CaseDir, ParamExpand, ParamOp, Pos, RemovalSide, ReplaceAnchor, Span, WordId,
    WordPart,
};
use super::lexer::decode_ansic;
use super::parser::{parse_sub_script, ParseCtx};
use super::{ident_byte, ident_start, identifier, parser_err, Error};
use crate::mem::{KBytes, KVec};

/// Parses a general shell word (from raw lexed bytes) into a `WordId`.
pub fn parse_word(raw: &[u8], err_pos: Pos, ctx: &mut ParseCtx<'_, '_>) -> Result<WordId, Error> {
    parse_word_impl(raw, err_pos, ctx, true, true, true)
}

/// Parses a word where brace expansion is disabled (e.g., redirection target, assignment RHS,
/// `case` word/pattern, `[[ ... ]]` operand).
pub fn parse_word_no_brace(
    raw: &[u8],
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<WordId, Error> {
    parse_word_impl(raw, err_pos, ctx, false, true, true)
}

/// Parses a here-document body into a `WordId`.
pub fn parse_heredoc_word(
    raw: &[u8],
    quoted: bool,
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<WordId, Error> {
    if quoted {
        return Ok(ctx.ast.push_literal_word(raw)?);
    }
    let mut parts = KVec::new_in(ctx.budget);
    let mut i = 0;
    let mut lit_start = 0;
    while i < raw.len() {
        match raw[i] {
            b'\\' if raw.get(i + 1).is_some_and(|b| matches!(b, b'$' | b'`' | b'\\' | b'\n')) => {
                if i > lit_start {
                    let s = ctx.ast.push_bytes(&raw[lit_start..i])?;
                    parts.try_push(WordPart::Literal(s))?;
                }
                let next = raw[i + 1];
                i += 2;
                if next != b'\n' {
                    let s = ctx.ast.push_bytes(&[next])?;
                    parts.try_push(WordPart::Escaped(s))?;
                }
                lit_start = i;
            }
            b'$' => {
                if i > lit_start {
                    let s = ctx.ast.push_bytes(&raw[lit_start..i])?;
                    parts.try_push(WordPart::Literal(s))?;
                }
                let (part, next_i) = parse_dollar(raw, i, err_pos, ctx, true)?;
                parts.try_push(part)?;
                i = next_i;
                lit_start = i;
            }
            b'`' => {
                if i > lit_start {
                    let s = ctx.ast.push_bytes(&raw[lit_start..i])?;
                    parts.try_push(WordPart::Literal(s))?;
                }
                let (part, next_i) = parse_backtick(raw, i, err_pos, ctx, true)?;
                parts.try_push(part)?;
                i = next_i;
                lit_start = i;
            }
            _ => i += 1,
        }
    }
    if i > lit_start {
        let s = ctx.ast.push_bytes(&raw[lit_start..i])?;
        parts.try_push(WordPart::Literal(s))?;
    }
    let p_start = ctx.ast.parts.len();
    ctx.ast.parts.try_extend_from_slice(&parts)?;
    Ok(ctx.ast.push_word(Span::new(p_start, ctx.ast.parts.len()))?)
}

/// Parses elements inside `(...)` of an array assignment `name=(a "b c" ...)`.
pub fn parse_array_elements(
    inner: &[u8],
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<Span, Error> {
    let mut words = KVec::new_in(ctx.budget);
    let mut i = 0;
    while i < inner.len() {
        while i < inner.len() && matches!(inner[i], b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
        }
        if i >= inner.len() {
            break;
        }
        if inner[i] == b'#' {
            while i < inner.len() && inner[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        while i < inner.len() && !matches!(inner[i], b' ' | b'\t' | b'\r' | b'\n') {
            match inner[i] {
                b'\\' if i + 1 < inner.len() => i += 2,
                b'\'' => {
                    i += 1;
                    while i < inner.len() && inner[i] != b'\'' {
                        i += 1;
                    }
                    if i < inner.len() {
                        i += 1;
                    }
                }
                b'"' => {
                    i += 1;
                    while i < inner.len() && inner[i] != b'"' {
                        if inner[i] == b'\\' && i + 1 < inner.len() {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    if i < inner.len() {
                        i += 1;
                    }
                }
                b'$' if inner.get(i + 1) == Some(&b'\'') => {
                    i += 2;
                    while i < inner.len() && inner[i] != b'\'' {
                        if inner[i] == b'\\' && i + 1 < inner.len() {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    if i < inner.len() {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        let w = parse_word(&inner[start..i], err_pos, ctx)?;
        words.try_push(w)?;
    }
    let start = ctx.ast.word_lists.len();
    ctx.ast.word_lists.try_extend_from_slice(&words)?;
    Ok(Span::new(start, ctx.ast.word_lists.len()))
}

fn parse_word_impl(
    raw: &[u8],
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
    allow_brace: bool,
    allow_tilde: bool,
    allow_glob: bool,
) -> Result<WordId, Error> {
    // Fast path for plain literal words (no quoting, expansions, braces, globs, or tilde).
    let plain = !(allow_tilde && raw.first() == Some(&b'~'))
        && !raw.iter().any(|&b| {
            matches!(
                b,
                b'\\' | b'\'' | b'"' | b'$' | b'`' | b'<' | b'>' | b'{' | b'*' | b'?' | b'[' | b'@' | b'+' | b'!'
            )
        });
    if plain {
        return Ok(ctx.ast.push_literal_word(raw)?);
    }

    let mut parts = KVec::new_in(ctx.budget);
    let mut i = 0;

    // Leading tilde expansion `~` or `~user` (when unquoted at word start)
    if allow_tilde && raw.first() == Some(&b'~') {
        let mut end = 1;
        let mut valid = true;
        while end < raw.len() && raw[end] != b'/' {
            let c = raw[end];
            if matches!(c, b'\'' | b'"' | b'\\' | b'$' | b'`' | b'{' | b'*' | b'?' | b'[') {
                valid = false;
                break;
            }
            end += 1;
        }
        if valid {
            let user = if end > 1 {
                Some(ctx.ast.push_bytes(&raw[1..end])?)
            } else {
                None
            };
            parts.try_push(WordPart::TildeExpand { user })?;
            i = end;
        }
    }

    let mut lit_start = i;

    while i < raw.len() {
        let b = raw[i];
        match b {
            b'\\' => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                if let Some(&next) = raw.get(i + 1) {
                    if next == b'\n' {
                        i += 2;
                    } else {
                        let s = ctx.ast.push_bytes(&[next])?;
                        parts.try_push(WordPart::Escaped(s))?;
                        i += 2;
                    }
                } else {
                    let s = ctx.ast.push_bytes(b"\\")?;
                    parts.try_push(WordPart::Escaped(s))?;
                    i += 1;
                }
                lit_start = i;
            }
            b'\'' => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                let start = i + 1;
                let end = raw[start..]
                    .iter()
                    .position(|&c| c == b'\'')
                    .map_or(raw.len(), |p| start + p);
                let s = ctx.ast.push_bytes(&raw[start..end])?;
                parts.try_push(WordPart::SingleQuoted(s))?;
                i = (end + 1).min(raw.len());
                lit_start = i;
            }
            b'"' => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                let (part, next_i) = parse_double_quoted(raw, i, err_pos, ctx)?;
                parts.try_push(part)?;
                i = next_i;
                lit_start = i;
            }
            b'$' if raw.get(i + 1) == Some(&b'\'') => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                let start = i + 2;
                let mut end = start;
                while end < raw.len() {
                    if raw[end] == b'\\' && end + 1 < raw.len() {
                        end += 2;
                        continue;
                    }
                    if raw[end] == b'\'' {
                        break;
                    }
                    end += 1;
                }
                let mut decoded = KBytes::new_in(ctx.budget);
                decode_ansic(&raw[start..end], &mut decoded, ctx.budget)?;
                let s = ctx.ast.push_bytes(&decoded)?;
                parts.try_push(WordPart::AnsiCQuoted(s))?;
                i = (end + 1).min(raw.len());
                lit_start = i;
            }
            b'$' if raw.get(i + 1) == Some(&b'"') => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                let (part, next_i) = parse_double_quoted(raw, i + 1, err_pos, ctx)?;
                parts.try_push(part)?;
                i = next_i;
                lit_start = i;
            }
            b'$' => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                let (part, next_i) = parse_dollar(raw, i, err_pos, ctx, false)?;
                parts.try_push(part)?;
                i = next_i;
                lit_start = i;
            }
            b'`' => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                let (part, next_i) = parse_backtick(raw, i, err_pos, ctx, false)?;
                parts.try_push(part)?;
                i = next_i;
                lit_start = i;
            }
            b'<' | b'>' if raw.get(i + 1) == Some(&b'(') => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                let output = b == b'>';
                let (inner, next_i) = scan_cmd_subst_inner(raw, i + 1);
                let body = parse_sub_script(inner, err_pos, ctx)?;
                parts.try_push(WordPart::ProcessSubst { body, output })?;
                i = next_i;
                lit_start = i;
            }
            b'{' if allow_brace => {
                if let Some((part, next_i)) = try_parse_brace(raw, i, err_pos, ctx)? {
                    flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                    parts.try_push(part)?;
                    i = next_i;
                    lit_start = i;
                } else {
                    i += 1;
                }
            }
            b'@' | b'*' | b'+' | b'?' | b'!' if allow_glob && raw.get(i + 1) == Some(&b'(') => {
                if let Some(end) = find_matching_paren(raw, i + 1) {
                    flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                    let pattern = ctx.ast.push_bytes(&raw[i..=end])?;
                    parts.try_push(WordPart::Glob { pattern })?;
                    i = end + 1;
                    lit_start = i;
                } else if matches!(b, b'*' | b'?') {
                    flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                    let pattern = ctx.ast.push_bytes(&raw[i..i + 1])?;
                    parts.try_push(WordPart::Glob { pattern })?;
                    i += 1;
                    lit_start = i;
                } else {
                    i += 1;
                }
            }
            b'*' | b'?' if allow_glob => {
                flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                let pattern = ctx.ast.push_bytes(&raw[i..i + 1])?;
                parts.try_push(WordPart::Glob { pattern })?;
                i += 1;
                lit_start = i;
            }
            b'[' if allow_glob => {
                if let Some(end) = find_glob_bracket(raw, i) {
                    flush_lit(raw, lit_start, i, &mut parts, ctx)?;
                    let pattern = ctx.ast.push_bytes(&raw[i..=end])?;
                    parts.try_push(WordPart::Glob { pattern })?;
                    i = end + 1;
                    lit_start = i;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    flush_lit(raw, lit_start, i, &mut parts, ctx)?;

    let p_start = ctx.ast.parts.len();
    ctx.ast.parts.try_extend_from_slice(&parts)?;
    Ok(ctx.ast.push_word(Span::new(p_start, ctx.ast.parts.len()))?)
}

fn flush_lit(
    raw: &[u8],
    start: usize,
    end: usize,
    parts: &mut KVec<WordPart>,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<(), Error> {
    if end > start {
        let s = ctx.ast.push_bytes(&raw[start..end])?;
        parts.try_push(WordPart::Literal(s))?;
    }
    Ok(())
}

fn parse_double_quoted(
    raw: &[u8],
    open_idx: usize,
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<(WordPart, usize), Error> {
    let mut inner_parts = KVec::new_in(ctx.budget);
    let mut i = open_idx + 1;
    let mut lit_start = i;
    while i < raw.len() {
        match raw[i] {
            b'"' => {
                flush_lit(raw, lit_start, i, &mut inner_parts, ctx)?;
                i += 1;
                break;
            }
            b'\\' if raw
                .get(i + 1)
                .is_some_and(|b| matches!(b, b'"' | b'\\' | b'$' | b'`' | b'\n')) =>
            {
                flush_lit(raw, lit_start, i, &mut inner_parts, ctx)?;
                let next = raw[i + 1];
                i += 2;
                if next != b'\n' {
                    let s = ctx.ast.push_bytes(&[next])?;
                    inner_parts.try_push(WordPart::Escaped(s))?;
                }
                lit_start = i;
            }
            b'\\' => {
                // Non-special backslash inside double quotes stays literal (skip next byte so \" check isn't fooled)
                i += if i + 1 < raw.len() { 2 } else { 1 };
            }
            b'$' => {
                flush_lit(raw, lit_start, i, &mut inner_parts, ctx)?;
                let (part, next_i) = parse_dollar(raw, i, err_pos, ctx, true)?;
                inner_parts.try_push(part)?;
                i = next_i;
                lit_start = i;
            }
            b'`' => {
                flush_lit(raw, lit_start, i, &mut inner_parts, ctx)?;
                let (part, next_i) = parse_backtick(raw, i, err_pos, ctx, true)?;
                inner_parts.try_push(part)?;
                i = next_i;
                lit_start = i;
            }
            _ => i += 1,
        }
    }
    let p_start = ctx.ast.parts.len();
    ctx.ast.parts.try_extend_from_slice(&inner_parts)?;
    Ok((
        WordPart::DoubleQuoted(Span::new(p_start, ctx.ast.parts.len())),
        i,
    ))
}

fn parse_dollar(
    raw: &[u8],
    dollar_idx: usize,
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
    in_dq: bool,
) -> Result<(WordPart, usize), Error> {
    let next = dollar_idx + 1;
    match raw.get(next).copied() {
        Some(b'{') => {
            let (inner, end_after) = scan_braced_inner(raw, next);
            let idx = parse_param_expand(inner, err_pos, ctx)?;
            Ok((WordPart::ParamExpand(idx), end_after))
        }
        Some(b'(') => {
            if raw.get(next + 1) == Some(&b'(') && is_arith_in_word(raw, next) {
                let (inner, end_after) = scan_dparen_inner(raw, next);
                let arith = parse_arith(inner, err_pos, ctx)?;
                Ok((WordPart::ArithExpand(arith), end_after))
            } else {
                let (inner, end_after) = scan_cmd_subst_inner(raw, next);
                let body = parse_sub_script(inner, err_pos, ctx)?;
                Ok((
                    WordPart::CommandSubst {
                        body,
                        backtick: false,
                    },
                    end_after,
                ))
            }
        }
        Some(b'[') => {
            let (inner, end_after) = scan_bracket_arith_inner(raw, next);
            let arith = parse_arith(inner, err_pos, ctx)?;
            Ok((WordPart::ArithExpand(arith), end_after))
        }
        Some(b) if ident_start(b) => {
            let mut end = next + 1;
            while raw.get(end).is_some_and(|&c| ident_byte(c)) {
                end += 1;
            }
            let param = ctx.ast.push_bytes(&raw[next..end])?;
            let idx = ctx.ast.push_param(ParamExpand {
                parameter: param,
                op: None,
            })?;
            Ok((WordPart::ParamExpand(idx), end))
        }
        Some(b'?' | b'$' | b'!' | b'#' | b'@' | b'*' | b'-' | b'0'..=b'9') => {
            let param = ctx.ast.push_bytes(&raw[next..next + 1])?;
            let idx = ctx.ast.push_param(ParamExpand {
                parameter: param,
                op: None,
            })?;
            Ok((WordPart::ParamExpand(idx), next + 1))
        }
        _ => {
            let _ = in_dq;
            let s = ctx.ast.push_bytes(b"$")?;
            Ok((WordPart::Literal(s), next))
        }
    }
}

fn parse_backtick(
    raw: &[u8],
    open_idx: usize,
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
    in_dq: bool,
) -> Result<(WordPart, usize), Error> {
    let mut i = open_idx + 1;
    let mut unescaped = KBytes::new_in(ctx.budget);
    while i < raw.len() {
        if raw[i] == b'`' {
            i += 1;
            break;
        }
        if raw[i] == b'\\' {
            if let Some(&next) = raw.get(i + 1) {
                let strip = matches!(next, b'$' | b'`' | b'\\') || (in_dq && next == b'"');
                if strip {
                    unescaped.try_push(next)?;
                    i += 2;
                    continue;
                }
            }
        }
        unescaped.try_push(raw[i])?;
        i += 1;
    }
    let body = parse_sub_script(&unescaped, err_pos, ctx)?;
    Ok((
        WordPart::CommandSubst {
            body,
            backtick: true,
        },
        i,
    ))
}

/// Parses `${...}` inner content (between `{` and `}`) into `Ast::param_expands`,
/// handling all 15 `ParamOp` forms and the exact parse-time vs runtime bad-substitution split.
fn parse_param_expand(
    inner: &[u8],
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<u32, Error> {
    // `${}` empty -> valid, expands to `""`.
    if inner.is_empty() {
        let p = ctx.ast.push_bytes(b"")?;
        return Ok(ctx.ast.push_param(ParamExpand {
            parameter: p,
            op: None,
        })?);
    }

    // Leading `#` -> length forms
    if inner[0] == b'#' {
        if inner.len() == 1 {
            // `${#}` -> positional parameter count
            let p = ctx.ast.push_bytes(b"#")?;
            return Ok(ctx.ast.push_param(ParamExpand {
                parameter: p,
                op: None,
            })?);
        }
        let rest = &inner[1..];
        let (name_end, multi_sub) = scan_param_name(rest);
        if name_end == 0 || multi_sub {
            let text = ctx.ast.push_bytes(inner)?;
            return Ok(ctx.ast.push_param(ParamExpand {
                parameter: text,
                op: Some(ParamOp::BadSubstitution { text }),
            })?);
        }
        let param_slice = &rest[..name_end];
        let tail = &rest[name_end..];
        if tail.is_empty() {
            let parameter = ctx.ast.push_bytes(param_slice)?;
            return Ok(ctx.ast.push_param(ParamExpand {
                parameter,
                op: Some(ParamOp::Length),
            })?);
        }
        if tail[0] == b':' {
            // `${#var:...}` -> deferred runtime bad substitution on `var`!
            let parameter = ctx.ast.push_bytes(param_slice)?;
            return Ok(ctx.ast.push_param(ParamExpand {
                parameter,
                op: Some(ParamOp::LengthSliceError),
            })?);
        }
        // Any other operator after `#var` (`${#x-y}`, `${#x/a/b}`, `${#x#p}`, etc.) -> parse-time error!
        let inner_str = core::str::from_utf8(inner).unwrap_or("?");
        return Err(parser_err(
            err_pos,
            ctx.budget,
            format_args!("${{{inner_str}}}: bad substitution"),
        ));
    }

    // Leading `!` -> indirection / array keys / var name prefix
    if inner[0] == b'!' {
        if inner.len() == 1 {
            let p = ctx.ast.push_bytes(b"!")?;
            return Ok(ctx.ast.push_param(ParamExpand {
                parameter: p,
                op: None,
            })?);
        }
        let rest = &inner[1..];
        // `${!arr[@]}` / `${!arr[*]}`
        if let Some(lb) = rest.iter().position(|&b| b == b'[') {
            let arr = &rest[..lb];
            let after = &rest[lb..];
            if identifier(arr) && (after == b"[@]" || after == b"[*]") {
                let array = ctx.ast.push_bytes(arr)?;
                let star = after == b"[*]";
                return Ok(ctx.ast.push_param(ParamExpand {
                    parameter: array,
                    op: Some(ParamOp::ArrayKeys { array, star }),
                })?);
            }
        }
        // `${!prefix*}` / `${!prefix@}`
        if rest.len() >= 2 && matches!(rest.last(), Some(b'*' | b'@')) {
            let pfx = &rest[..rest.len() - 1];
            if identifier(pfx) {
                let prefix = ctx.ast.push_bytes(pfx)?;
                let star = *rest.last().unwrap() == b'*';
                return Ok(ctx.ast.push_param(ParamExpand {
                    parameter: prefix,
                    op: Some(ParamOp::VarNamePrefix { prefix, star }),
                })?);
            }
        }
        // `${!var}` or `${!var<op>}`
        let (name_end, multi_sub) = scan_param_name(rest);
        if name_end == 0 || multi_sub {
            let text = ctx.ast.push_bytes(inner)?;
            return Ok(ctx.ast.push_param(ParamExpand {
                parameter: text,
                op: Some(ParamOp::BadSubstitution { text }),
            })?);
        }
        let parameter = ctx.ast.push_bytes(&rest[..name_end])?;
        let tail = &rest[name_end..];
        let inner_op = if tail.is_empty() {
            None
        } else {
            Some(parse_param_expand(rest, err_pos, ctx)?)
        };
        return Ok(ctx.ast.push_param(ParamExpand {
            parameter,
            op: Some(ParamOp::Indirection { inner_op }),
        })?);
    }

    // Standard parameter name + optional operator
    let (name_end, multi_sub) = scan_param_name(inner);
    if name_end == 0 || multi_sub {
        let text = ctx.ast.push_bytes(inner)?;
        return Ok(ctx.ast.push_param(ParamExpand {
            parameter: text,
            op: Some(ParamOp::BadSubstitution { text }),
        })?);
    }
    let param_slice = &inner[..name_end];
    let tail = &inner[name_end..];
    let parameter = ctx.ast.push_bytes(param_slice)?;

    if tail.is_empty() {
        return Ok(ctx.ast.push_param(ParamExpand {
            parameter,
            op: None,
        })?);
    }

    // `${var!ref}` -> parse-time error!
    if tail[0] == b'!' {
        let inner_str = core::str::from_utf8(inner).unwrap_or("?");
        return Err(parser_err(
            err_pos,
            ctx.budget,
            format_args!("${{{inner_str}}}: bad substitution"),
        ));
    }

    let op = match tail {
        // Default / Assign / Error / Alternative (with or without `:`)
        [b':', b'-', rest @ ..] => {
            let word = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::DefaultValue {
                word,
                check_empty: true,
            }
        }
        [b'-', rest @ ..] => {
            let word = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::DefaultValue {
                word,
                check_empty: false,
            }
        }
        [b':', b'=', rest @ ..] => {
            let word = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::AssignDefault {
                word,
                check_empty: true,
            }
        }
        [b'=', rest @ ..] => {
            let word = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::AssignDefault {
                word,
                check_empty: false,
            }
        }
        [b':', b'?', rest @ ..] => {
            let word = if rest.is_empty() {
                None
            } else {
                Some(parse_word_no_brace(rest, err_pos, ctx)?)
            };
            ParamOp::ErrorIfUnset {
                word,
                check_empty: true,
            }
        }
        [b'?', rest @ ..] => {
            let word = if rest.is_empty() {
                None
            } else {
                Some(parse_word_no_brace(rest, err_pos, ctx)?)
            };
            ParamOp::ErrorIfUnset {
                word,
                check_empty: false,
            }
        }
        [b':', b'+', rest @ ..] => {
            let word = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::UseAlternative {
                word,
                check_empty: true,
            }
        }
        [b'+', rest @ ..] => {
            let word = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::UseAlternative {
                word,
                check_empty: false,
            }
        }
        // Substring `${var:offset[:length]}`
        [b':', rest @ ..] => {
            let (off_bytes, len_bytes) = split_substring_colon(rest);
            let offset = parse_arith(off_bytes, err_pos, ctx)?;
            let length = match len_bytes {
                Some(lb) => Some(parse_arith(lb, err_pos, ctx)?),
                None => None,
            };
            ParamOp::Substring { offset, length }
        }
        // Pattern removal `#`, `##`, `%`, `%%`
        [b'#', b'#', rest @ ..] => {
            let pattern = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::PatternRemoval {
                pattern,
                side: RemovalSide::Prefix,
                greedy: true,
            }
        }
        [b'#', rest @ ..] => {
            let pattern = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::PatternRemoval {
                pattern,
                side: RemovalSide::Prefix,
                greedy: false,
            }
        }
        [b'%', b'%', rest @ ..] => {
            let pattern = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::PatternRemoval {
                pattern,
                side: RemovalSide::Suffix,
                greedy: true,
            }
        }
        [b'%', rest @ ..] => {
            let pattern = parse_word_no_brace(rest, err_pos, ctx)?;
            ParamOp::PatternRemoval {
                pattern,
                side: RemovalSide::Suffix,
                greedy: false,
            }
        }
        // Pattern replacement `/`, `//`, `/#`, `/%`
        [b'/', rest @ ..] => {
            let (all, anchor, body) = match rest {
                [b'/', r @ ..] => (true, None, r),
                [b'#', r @ ..] => (false, Some(ReplaceAnchor::Start), r),
                [b'%', r @ ..] => (false, Some(ReplaceAnchor::End), r),
                _ => (false, None, rest),
            };
            let (pat_bytes, rep_bytes) = split_replacement_slash(body);
            let pattern = parse_word_no_brace(pat_bytes, err_pos, ctx)?;
            let replacement = match rep_bytes {
                Some(rb) => Some(parse_word_no_brace(rb, err_pos, ctx)?),
                None => None,
            };
            ParamOp::PatternReplacement {
                pattern,
                replacement,
                all,
                anchor,
            }
        }
        // Case modification `^`, `^^`, `,`, `,,`, `~`, `~~`
        [b'^', b'^', rest @ ..] => ParamOp::CaseModification {
            direction: CaseDir::Upper,
            all: true,
            pattern: opt_word(rest, err_pos, ctx)?,
        },
        [b'^', rest @ ..] => ParamOp::CaseModification {
            direction: CaseDir::Upper,
            all: false,
            pattern: opt_word(rest, err_pos, ctx)?,
        },
        [b',', b',', rest @ ..] => ParamOp::CaseModification {
            direction: CaseDir::Lower,
            all: true,
            pattern: opt_word(rest, err_pos, ctx)?,
        },
        [b',', rest @ ..] => ParamOp::CaseModification {
            direction: CaseDir::Lower,
            all: false,
            pattern: opt_word(rest, err_pos, ctx)?,
        },
        [b'~', b'~', rest @ ..] => ParamOp::CaseModification {
            direction: CaseDir::Toggle,
            all: true,
            pattern: opt_word(rest, err_pos, ctx)?,
        },
        [b'~', rest @ ..] => ParamOp::CaseModification {
            direction: CaseDir::Toggle,
            all: false,
            pattern: opt_word(rest, err_pos, ctx)?,
        },
        // Parameter transformation `@Q`, `@E`, `@P`, `@A`, `@K`, `@a`, `@k`, `@u`, `@U`, `@L`
        [b'@', op_byte]
            if matches!(
                op_byte,
                b'Q' | b'E' | b'P' | b'A' | b'K' | b'a' | b'k' | b'u' | b'U' | b'L'
            ) =>
        {
            ParamOp::Transform { operator: *op_byte }
        }
        _ => {
            let text = ctx.ast.push_bytes(inner)?;
            ParamOp::BadSubstitution { text }
        }
    };

    Ok(ctx.ast.push_param(ParamExpand {
        parameter,
        op: Some(op),
    })?)
}

fn opt_word(
    rest: &[u8],
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<Option<WordId>, Error> {
    if rest.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_word_no_brace(rest, err_pos, ctx)?))
    }
}

/// Scans a parameter name (identifier with optional `[subscript]`, decimal positional, or special).
/// Returns `(end_index, has_multi_subscript)`.
fn scan_param_name(s: &[u8]) -> (usize, bool) {
    let Some(&first) = s.first() else {
        return (0, false);
    };
    if ident_start(first) {
        let mut i = 1;
        while s.get(i).is_some_and(|&b| ident_byte(b)) {
            i += 1;
        }
        if s.get(i) == Some(&b'[') {
            if let Some(end) = find_matching_bracket(s, i) {
                i = end + 1;
                if s.get(i) == Some(&b'[') {
                    return (i, true);
                }
            }
        }
        return (i, false);
    }
    if first.is_ascii_digit() {
        let mut i = 1;
        while s.get(i).is_some_and(|b| b.is_ascii_digit()) {
            i += 1;
        }
        return (i, false);
    }
    if matches!(first, b'?' | b'$' | b'!' | b'#' | b'@' | b'*' | b'-') {
        return (1, false);
    }
    (0, false)
}

fn split_substring_colon(s: &[u8]) -> (&[u8], Option<&[u8]>) {
    let mut paren = 0u32;
    let mut brace = 0u32;
    let mut bracket = 0u32;
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'\\' if i + 1 < s.len() => i += 2,
            b'(' => {
                paren += 1;
                i += 1;
            }
            b')' => {
                paren = paren.saturating_sub(1);
                i += 1;
            }
            b'{' => {
                brace += 1;
                i += 1;
            }
            b'}' => {
                brace = brace.saturating_sub(1);
                i += 1;
            }
            b'[' => {
                bracket += 1;
                i += 1;
            }
            b']' => {
                bracket = bracket.saturating_sub(1);
                i += 1;
            }
            b':' if paren == 0 && brace == 0 && bracket == 0 => {
                return (&s[..i], Some(&s[i + 1..]));
            }
            _ => i += 1,
        }
    }
    (s, None)
}

fn split_replacement_slash(s: &[u8]) -> (&[u8], Option<&[u8]>) {
    let mut i = 0;
    let mut in_sq = false;
    let mut in_dq = false;
    let mut brace = 0u32;
    let mut bracket = 0u32;
    while i < s.len() {
        let b = s[i];
        if in_sq {
            if b == b'\'' {
                in_sq = false;
            }
            i += 1;
            continue;
        }
        if in_dq {
            if b == b'\\' && i + 1 < s.len() {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_dq = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\\' if i + 1 < s.len() => i += 2,
            b'\'' => {
                in_sq = true;
                i += 1;
            }
            b'"' => {
                in_dq = true;
                i += 1;
            }
            b'{' => {
                brace += 1;
                i += 1;
            }
            b'}' => {
                brace = brace.saturating_sub(1);
                i += 1;
            }
            b'[' => {
                bracket += 1;
                i += 1;
            }
            b']' => {
                bracket = bracket.saturating_sub(1);
                i += 1;
            }
            b'/' if brace == 0 && bracket == 0 => {
                return (&s[..i], Some(&s[i + 1..]));
            }
            _ => i += 1,
        }
    }
    (s, None)
}

/// Attempts to parse a brace expansion `{a,b}` or `{start..end[..step]}` at `raw[open_idx]`.
fn try_parse_brace(
    raw: &[u8],
    open_idx: usize,
    err_pos: Pos,
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<Option<(WordPart, usize)>, Error> {
    // Find matching `}` respecting quotes, escapes, `${...}`, and nested `{...}`.
    let mut i = open_idx + 1;
    let mut depth = 1u32;
    let mut in_sq = false;
    let mut in_dq = false;
    let mut commas = KVec::new_in(ctx.budget);

    while i < raw.len() && depth > 0 {
        let b = raw[i];
        if in_sq {
            if b == b'\'' {
                in_sq = false;
            }
            i += 1;
            continue;
        }
        if in_dq {
            if b == b'\\' && i + 1 < raw.len() {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_dq = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\\' if i + 1 < raw.len() => i += 2,
            b'\'' => {
                in_sq = true;
                i += 1;
            }
            b'"' => {
                in_dq = true;
                i += 1;
            }
            b'$' if raw.get(i + 1) == Some(&b'{') => {
                let (_, end) = scan_braced_inner(raw, i + 1);
                i = end;
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                i += 1;
            }
            b',' if depth == 1 => {
                commas.try_push(i)?;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return Ok(None);
    }
    let close_idx = i;
    let inner = &raw[open_idx + 1..close_idx];

    // Comma list `{a,b,...}`
    if !commas.is_empty() {
        let mut items = KVec::new_in(ctx.budget);
        let mut seg_start = open_idx + 1;
        for &c_idx in commas.iter() {
            let w = parse_word(&raw[seg_start..c_idx], err_pos, ctx)?;
            items.try_push(BraceItem::Word(w))?;
            seg_start = c_idx + 1;
        }
        let w = parse_word(&raw[seg_start..close_idx], err_pos, ctx)?;
        items.try_push(BraceItem::Word(w))?;
        let start = ctx.ast.brace_items.len();
        ctx.ast.brace_items.try_extend_from_slice(&items)?;
        return Ok(Some((
            WordPart::BraceExpand(Span::new(start, ctx.ast.brace_items.len())),
            close_idx + 1,
        )));
    }

    // Sequence `{start..end}` or `{start..end..step}`
    if let Some(range_item) = try_parse_brace_range(inner, &raw[open_idx..=close_idx], ctx)? {
        let start = ctx.ast.brace_items.len();
        ctx.ast.brace_items.try_push(range_item)?;
        return Ok(Some((
            WordPart::BraceExpand(Span::new(start, ctx.ast.brace_items.len())),
            close_idx + 1,
        )));
    }

    Ok(None)
}

fn try_parse_brace_range(
    inner: &[u8],
    full_raw: &[u8],
    ctx: &mut ParseCtx<'_, '_>,
) -> Result<Option<BraceItem>, Error> {
    // Split on `..`
    let Some(first_dd) = find_double_dot(inner, 0) else {
        return Ok(None);
    };
    let part1 = &inner[..first_dd];
    let rest = &inner[first_dd + 2..];
    let (part2, part3) = match find_double_dot(rest, 0) {
        Some(second_dd) => (&rest[..second_dd], Some(&rest[second_dd + 2..])),
        None => (rest, None),
    };
    if part1.is_empty() || part2.is_empty() {
        return Ok(None);
    }
    let step = match part3 {
        Some(s) => {
            let Some(n) = parse_signed_i64(s) else {
                return Ok(None);
            };
            Some(n)
        }
        None => None,
    };
    // Integer range
    if let (Some(a), Some(b)) = (parse_signed_i64(part1), parse_signed_i64(part2)) {
        let raw = ctx.ast.push_bytes(full_raw)?;
        return Ok(Some(BraceItem::Range {
            start_num: Some(a),
            end_num: Some(b),
            start_char: None,
            end_char: None,
            step,
            raw,
        }));
    }
    // Single ASCII letter range
    if part1.len() == 1
        && part2.len() == 1
        && part1[0].is_ascii_alphabetic()
        && part2[0].is_ascii_alphabetic()
    {
        let raw = ctx.ast.push_bytes(full_raw)?;
        return Ok(Some(BraceItem::Range {
            start_num: None,
            end_num: None,
            start_char: Some(part1[0]),
            end_char: Some(part2[0]),
            step,
            raw,
        }));
    }
    Ok(None)
}

fn find_double_dot(s: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < s.len() {
        if s[i] == b'.' && s[i + 1] == b'.' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_signed_i64(s: &[u8]) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let (neg, digits) = if let Some(rest) = s.strip_prefix(b"-") {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix(b"+") {
        (false, rest)
    } else {
        (false, s)
    };
    if digits.is_empty() || !digits.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut acc: i64 = 0;
    for &b in digits {
        let d = (b - b'0') as i64;
        acc = if neg {
            acc.checked_mul(10)?.checked_sub(d)?
        } else {
            acc.checked_mul(10)?.checked_add(d)?
        };
    }
    Some(acc)
}

fn scan_braced_inner(raw: &[u8], open_brace: usize) -> (&[u8], usize) {
    let mut i = open_brace + 1;
    let mut depth = 1u32;
    let mut in_sq = false;
    let mut in_dq = false;
    while i < raw.len() && depth > 0 {
        let b = raw[i];
        if in_sq {
            if b == b'\'' {
                in_sq = false;
            }
            i += 1;
            continue;
        }
        if in_dq {
            if b == b'\\' && i + 1 < raw.len() {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_dq = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\\' if i + 1 < raw.len() => i += 2,
            b'\'' => {
                in_sq = true;
                i += 1;
            }
            b'"' => {
                in_dq = true;
                i += 1;
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (&raw[open_brace + 1..i], i + 1);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    (&raw[open_brace + 1..raw.len()], raw.len())
}

fn is_arith_in_word(raw: &[u8], first_lparen: usize) -> bool {
    let mut i = first_lparen + 2;
    let mut depth = 2i32;
    let mut in_sq = false;
    let mut in_dq = false;
    while i < raw.len() {
        let b = raw[i];
        if in_sq {
            if b == b'\'' {
                in_sq = false;
            }
            i += 1;
            continue;
        }
        if in_dq {
            if b == b'\\' && i + 1 < raw.len() {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_dq = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\\' if i + 1 < raw.len() => i += 2,
            b'\'' => {
                in_sq = true;
                i += 1;
            }
            b'"' => {
                in_dq = true;
                i += 1;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                if depth == 2 {
                    return raw.get(i + 1) == Some(&b')');
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    false
}

fn scan_dparen_inner(raw: &[u8], first_lparen: usize) -> (&[u8], usize) {
    let start = first_lparen + 2;
    let mut i = start;
    let mut depth = 2u32;
    let mut in_sq = false;
    let mut in_dq = false;
    while i < raw.len() {
        let b = raw[i];
        if in_sq {
            if b == b'\'' {
                in_sq = false;
            }
            i += 1;
            continue;
        }
        if in_dq {
            if b == b'\\' && i + 1 < raw.len() {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_dq = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\\' if i + 1 < raw.len() => i += 2,
            b'\'' => {
                in_sq = true;
                i += 1;
            }
            b'"' => {
                in_dq = true;
                i += 1;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                if depth == 2 && raw.get(i + 1) == Some(&b')') {
                    return (&raw[start..i], i + 2);
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }
    (&raw[start..raw.len()], raw.len())
}

fn scan_bracket_arith_inner(raw: &[u8], open_bracket: usize) -> (&[u8], usize) {
    let start = open_bracket + 1;
    let mut i = start;
    let mut depth = 1u32;
    while i < raw.len() && depth > 0 {
        match raw[i] {
            b'\\' if i + 1 < raw.len() => i += 2,
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return (&raw[start..i], i + 1);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    (&raw[start..raw.len()], raw.len())
}

fn scan_cmd_subst_inner(raw: &[u8], open_paren: usize) -> (&[u8], usize) {
    let start = open_paren + 1;
    let mut i = start;
    let mut paren_depth = 1u32;
    let mut case_depth = 0u32;
    let mut in_case_pat = false;
    let mut in_sq = false;
    let mut in_dq = false;
    let mut word_start = true;
    let mut wbuf = [0u8; 8];
    let mut wlen = 0usize;

    while i < raw.len() {
        let b = raw[i];
        if in_sq {
            if b == b'\'' {
                in_sq = false;
            }
            i += 1;
            continue;
        }
        if in_dq {
            if b == b'\\' && i + 1 < raw.len() {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_dq = false;
            }
            i += 1;
            continue;
        }
        if super::lexer::is_delimiter(b) && wlen > 0 {
            let w = &wbuf[..wlen];
            if w == b"case" {
                case_depth += 1;
            } else if w == b"in" && case_depth > 0 {
                in_case_pat = true;
            } else if w == b"esac" && case_depth > 0 {
                case_depth -= 1;
                in_case_pat = false;
            }
            wlen = 0;
        }
        match b {
            b'\\' if i + 1 < raw.len() => {
                i += 2;
                word_start = false;
                wlen = 0;
            }
            b'\'' => {
                in_sq = true;
                i += 1;
                word_start = false;
                wlen = 0;
            }
            b'"' => {
                in_dq = true;
                i += 1;
                word_start = false;
                wlen = 0;
            }
            b'#' if word_start => {
                while i < raw.len() && raw[i] != b'\n' {
                    i += 1;
                }
            }
            b';' if case_depth > 0 => {
                if raw.get(i + 1) == Some(&b';') {
                    i += if raw.get(i + 2) == Some(&b'&') { 3 } else { 2 };
                    in_case_pat = true;
                } else if raw.get(i + 1) == Some(&b'&') {
                    i += 2;
                    in_case_pat = true;
                } else {
                    i += 1;
                }
                word_start = true;
            }
            b'(' => {
                if !in_case_pat {
                    paren_depth += 1;
                }
                i += 1;
                word_start = true;
            }
            b')' => {
                if in_case_pat {
                    in_case_pat = false;
                    i += 1;
                    word_start = true;
                } else {
                    paren_depth -= 1;
                    if paren_depth == 0 {
                        return (&raw[start..i], i + 1);
                    }
                    i += 1;
                    word_start = true;
                }
            }
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
                word_start = true;
            }
            _ => {
                if (word_start || wlen > 0) && b.is_ascii_alphabetic() {
                    if wlen < wbuf.len() {
                        wbuf[wlen] = b;
                        wlen += 1;
                    } else {
                        wlen = 0;
                    }
                    word_start = false;
                } else {
                    wlen = 0;
                    word_start = false;
                }
                i += 1;
            }
        }
    }
    (&raw[start..raw.len()], raw.len())
}

fn find_matching_paren(raw: &[u8], open: usize) -> Option<usize> {
    let mut i = open + 1;
    let mut depth = 1u32;
    while i < raw.len() {
        match raw[i] {
            b'\\' if i + 1 < raw.len() => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn find_matching_bracket(raw: &[u8], open: usize) -> Option<usize> {
    let mut i = open + 1;
    let mut depth = 1u32;
    while i < raw.len() {
        match raw[i] {
            b'\\' if i + 1 < raw.len() => i += 2,
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn find_glob_bracket(raw: &[u8], open: usize) -> Option<usize> {
    let mut i = open + 1;
    if i >= raw.len() {
        return None;
    }
    // Optional negation `!` or `^`
    if matches!(raw[i], b'!' | b'^') {
        i += 1;
    }
    // Leading `]` is literal inside `[...]`
    if raw.get(i) == Some(&b']') {
        i += 1;
    }
    while i < raw.len() {
        match raw[i] {
            b'\\' if i + 1 < raw.len() => i += 2,
            b'[' if raw.get(i + 1).is_some_and(|c| matches!(c, b':' | b'.' | b'=')) => {
                let delim = raw[i + 1];
                i += 2;
                while i + 1 < raw.len() && !(raw[i] == delim && raw[i + 1] == b']') {
                    i += 1;
                }
                i = (i + 2).min(raw.len());
            }
            b']' => return Some(i),
            _ => i += 1,
        }
    }
    None
}
