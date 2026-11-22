//! Pattern parsers for ERE, BRE (`Grep`/`Sed`/`Expr`), `Fixed` and Perl-subset modes.
//!
//! Produces an [`Ast`] plus the bracket-class table and named-group map, enforcing
//! compile-time structural caps (`max_nesting_depth`, `max_capture_groups`,
//! `max_repeat_expansion`) and validating backreferences against `allow_backrefs`.

use super::{
    decode_scalar, BreDialect, CapKind, CompileLimits, RegexError, RegexFlags, RegexMode,
    SyntaxKind, RAW_BYTE_BASE,
};
use crate::mem::{Budget, KBox, KBytes, KVec, MemResult};
use crate::util::bytes_in;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AssertKind {
    StartText,
    EndText,
    StartLine,
    EndLine,
    WordBound,
    NotWordBound,
    WordStart,
    WordEnd,
    PrevNotWord,
    NextNotWord,
}

pub(crate) struct CharClass {
    pub neg: bool,
    pub icase: bool,
    pub ranges: KVec<(u32, u32)>,
}

pub(crate) enum Ast {
    Empty,
    Char {
        ch: u32,
        icase: bool,
    },
    Dot {
        dot_all: bool,
    },
    Class(u32),
    Assert(AssertKind),
    Keep,
    Backref {
        group: u8,
        icase: bool,
    },
    Seq(KVec<Ast>),
    Alt(KVec<Ast>),
    Repeat {
        inner: KBox<Ast>,
        min: u32,
        max: Option<u32>,
        lazy: bool,
    },
    Group {
        inner: KBox<Ast>,
        index: u32,
    },
}

pub(crate) struct Parsed {
    pub root: Ast,
    pub classes: KVec<CharClass>,
    pub names: KVec<(KBytes, u32)>,
    pub groups: u32,
    pub has_backrefs: bool,
}

#[derive(Clone, Copy)]
struct Flags3 {
    i: bool,
    m: bool,
    s: bool,
}

struct Parser<'a> {
    pat: &'a [u8],
    pos: usize,
    mode: RegexMode,
    cur: Flags3,
    stack: KVec<Flags3>,
    limits: CompileLimits,
    allow_backrefs: bool,
    comma_first: bool,
    perl: bool,
    /// True when `*` is a literal character in `Grep`/`Sed` BRE (at start, after `^`, `\(`, `\|`).
    star_lit: bool,
    /// Set by `parse_atom`: the atom just parsed was a `^`.
    caret: bool,
    depth: u32,
    groups: u32,
    has_backrefs: bool,
    refs: KVec<u8>,
    names: KVec<(KBytes, u32)>,
    classes: KVec<CharClass>,
    b: &'a Budget,
}

enum IntervalErr {
    NotAnInterval,
    Fail(RegexError),
}

pub(crate) fn parse(
    pattern: &[u8],
    mode: RegexMode,
    flags: RegexFlags,
    limits: CompileLimits,
    b: &Budget,
) -> Result<Parsed, RegexError> {
    if let RegexMode::Fixed = mode {
        return parse_fixed(pattern, flags, b);
    }
    let comma_first =
        flags.comma_first_interval || matches!(mode, RegexMode::Basic(BreDialect::Sed));
    let mut p = Parser {
        pat: pattern,
        pos: 0,
        mode,
        cur: Flags3 {
            i: flags.ignore_case,
            m: flags.multiline,
            s: flags.dot_all,
        },
        stack: KVec::new_in(b),
        limits,
        allow_backrefs: flags.allow_backrefs,
        comma_first,
        perl: matches!(mode, RegexMode::Perl),
        star_lit: true,
        caret: false,
        depth: 0,
        groups: 0,
        has_backrefs: false,
        refs: KVec::new_in(b),
        names: KVec::new_in(b),
        classes: KVec::new_in(b),
        b,
    };
    let mut root = p.parse_alt(false)?;
    if p.pos != p.pat.len() {
        return Err(RegexError::Syntax(SyntaxKind::UnmatchedCloseParen));
    }
    if p.groups > p.limits.max_capture_groups {
        return Err(RegexError::CompileCap(CapKind::TooManyCaptures {
            limit: p.limits.max_capture_groups,
        }));
    }
    for &n in p.refs.as_slice() {
        if !p.allow_backrefs {
            return Err(RegexError::Syntax(SyntaxKind::BackrefUnsupported(n)));
        }
        if (n as u32) > p.groups {
            return Err(RegexError::Syntax(SyntaxKind::InvalidBackref(n)));
        }
    }
    if matches!(mode, RegexMode::Basic(BreDialect::Expr)) {
        let mut seq = KVec::new_in(b);
        seq.try_push(Ast::Assert(AssertKind::StartText))?;
        seq.try_push(root)?;
        root = Ast::Seq(seq);
    }
    Ok(Parsed {
        root,
        classes: p.classes,
        names: p.names,
        groups: p.groups,
        has_backrefs: p.has_backrefs,
    })
}

fn parse_fixed(pattern: &[u8], flags: RegexFlags, b: &Budget) -> Result<Parsed, RegexError> {
    let icase = flags.ignore_case;
    let build_needle = |slice: &[u8]| -> Result<Ast, RegexError> {
        if slice.is_empty() {
            return Ok(Ast::Empty);
        }
        let mut items = KVec::new_in(b);
        let mut i = 0;
        while i < slice.len() {
            let (ch, len) = decode_scalar(slice, i);
            items.try_push(Ast::Char { ch, icase })?;
            i += len;
        }
        if items.len() == 1 {
            Ok(items.pop().unwrap_or(Ast::Empty))
        } else {
            Ok(Ast::Seq(items))
        }
    };

    let root = if pattern.is_empty() {
        Ast::Empty
    } else {
        let mut branches = KVec::new_in(b);
        let mut start = 0;
        for (i, &c) in pattern.iter().enumerate() {
            if c == b'\n' {
                branches.try_push(build_needle(&pattern[start..i])?)?;
                start = i + 1;
            }
        }
        branches.try_push(build_needle(&pattern[start..])?)?;
        if branches.len() == 1 {
            branches.pop().unwrap_or(Ast::Empty)
        } else {
            Ast::Alt(branches)
        }
    };
    Ok(Parsed {
        root,
        classes: KVec::new_in(b),
        names: KVec::new_in(b),
        groups: 0,
        has_backrefs: false,
    })
}

/// Exact ASCII byte sets for the 14 POSIX character classes (spec section 5.2).
pub(crate) fn posix_ranges(name: &[u8]) -> Option<&'static [(u32, u32)]> {
    match name {
        b"alnum" => Some(&[(0x30, 0x39), (0x41, 0x5A), (0x61, 0x7A)]),
        b"alpha" => Some(&[(0x41, 0x5A), (0x61, 0x7A)]),
        b"ascii" => Some(&[(0x00, 0x7F)]),
        b"blank" => Some(&[(0x09, 0x09), (0x20, 0x20)]),
        b"cntrl" => Some(&[(0x00, 0x1F), (0x7F, 0x7F)]),
        b"digit" => Some(&[(0x30, 0x39)]),
        b"graph" => Some(&[(0x21, 0x7E)]),
        b"lower" => Some(&[(0x61, 0x7A)]),
        b"print" => Some(&[(0x20, 0x7E)]),
        b"punct" => Some(&[
            (0x21, 0x2F),
            (0x3A, 0x40),
            (0x5B, 0x60),
            (0x7B, 0x7E),
        ]),
        b"space" => Some(&[(0x09, 0x0D), (0x20, 0x20)]),
        b"upper" => Some(&[(0x41, 0x5A)]),
        b"word" => Some(&[
            (0x30, 0x39),
            (0x41, 0x5A),
            (0x5F, 0x5F),
            (0x61, 0x7A),
        ]),
        b"xdigit" => Some(&[(0x30, 0x39), (0x41, 0x46), (0x61, 0x66)]),
        _ => None,
    }
}

fn shorthand_ranges(c: u8) -> Option<(bool, &'static [(u32, u32)])> {
    match c {
        b'w' => Some((false, &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)])),
        b'W' => Some((true, &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)])),
        b's' => Some((false, &[(0x09, 0x0D), (0x20, 0x20)])),
        b'S' => Some((true, &[(0x09, 0x0D), (0x20, 0x20)])),
        b'd' => Some((false, &[(0x30, 0x39)])),
        b'D' => Some((true, &[(0x30, 0x39)])),
        _ => None,
    }
}

fn hex_val(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some((b - b'0') as u32),
        b'a'..=b'f' => Some((b - b'a' + 10) as u32),
        b'A'..=b'F' => Some((b - b'A' + 10) as u32),
        _ => None,
    }
}

impl<'a> Parser<'a> {
    fn is_bre_escaped_ops(&self) -> bool {
        matches!(
            self.mode,
            RegexMode::Basic(BreDialect::Grep | BreDialect::Sed)
        )
    }

    fn err<T>(&self, k: SyntaxKind) -> Result<T, RegexError> {
        Err(RegexError::Syntax(k))
    }

    fn peek(&self) -> Option<u8> {
        self.pat.get(self.pos).copied()
    }

    fn boxed(&self, ast: Ast) -> Result<KBox<Ast>, RegexError> {
        KBox::try_new_in(ast, self.b).map_err(|(_, e)| RegexError::Mem(e))
    }

    fn char_ast(&self, ch: u32) -> Ast {
        Ast::Char {
            ch,
            icase: self.cur.i,
        }
    }

    fn push_class(
        &mut self,
        neg: bool,
        icase: bool,
        ranges: &[(u32, u32)],
    ) -> Result<u32, RegexError> {
        let mut v = KVec::new_in(self.b);
        v.try_extend_from_slice(ranges)?;
        let id = self.classes.len() as u32;
        self.classes.try_push(CharClass {
            neg,
            icase,
            ranges: v,
        })?;
        Ok(id)
    }

    fn class_escape(&mut self, c: u8) -> Result<Ast, RegexError> {
        let Some((neg, ranges)) = shorthand_ranges(c) else {
            return self.err(SyntaxKind::InvalidEscape);
        };
        Ok(Ast::Class(self.push_class(neg, false, ranges)?))
    }

    fn enter_nesting(&mut self) -> Result<(), RegexError> {
        if self.depth >= self.limits.max_nesting_depth {
            return Err(RegexError::CompileCap(CapKind::NestingTooDeep {
                limit: self.limits.max_nesting_depth,
            }));
        }
        self.depth += 1;
        Ok(())
    }

    fn open_capture(&mut self) -> Result<u32, RegexError> {
        self.enter_nesting()?;
        // Track count; checked against `max_capture_groups` after parse so deep
        // nesting `((((...201...))))` reports `NestingTooDeep` before `TooManyCaptures`.
        self.groups = self.groups.saturating_add(1);
        Ok(self.groups)
    }

    fn push_scope(&mut self) -> MemResult<()> {
        self.stack.try_push(self.cur)
    }

    fn pop_scope(&mut self) {
        if let Some(f) = self.stack.pop() {
            self.cur = f;
        }
    }

    fn check_name(&self, name: &[u8]) -> Result<(), RegexError> {
        if name.is_empty() {
            return Err(RegexError::Syntax(SyntaxKind::InvalidCaptureName));
        }
        let ok_first = |c: u8| c == b'_' || c.is_ascii_alphabetic();
        if !ok_first(name[0]) || !name.iter().all(|c| ok_first(*c) || c.is_ascii_digit()) {
            return Err(RegexError::Syntax(SyntaxKind::InvalidCaptureName));
        }
        Ok(())
    }

    fn add_name(&mut self, name: &[u8], index: u32) -> Result<(), RegexError> {
        for (n, _) in self.names.as_slice() {
            if n.as_slice() == name {
                return Err(RegexError::Syntax(SyntaxKind::DuplicateCaptureName));
            }
        }
        let owned = bytes_in(name, self.b)?;
        self.names.try_push((owned, index))?;
        Ok(())
    }

    fn alt_len(&self) -> Option<usize> {
        if self.is_bre_escaped_ops() {
            if self.pat.get(self.pos) == Some(&b'\\')
                && self.pat.get(self.pos + 1) == Some(&b'|')
            {
                Some(2)
            } else {
                None
            }
        } else if self.peek() == Some(b'|') {
            Some(1)
        } else {
            None
        }
    }

    fn close_len(&self) -> Option<usize> {
        if self.is_bre_escaped_ops() {
            if self.pat.get(self.pos) == Some(&b'\\')
                && self.pat.get(self.pos + 1) == Some(&b')')
            {
                Some(2)
            } else {
                None
            }
        } else if self.peek() == Some(b')') {
            Some(1)
        } else {
            None
        }
    }

    fn parse_alt(&mut self, in_group: bool) -> Result<Ast, RegexError> {
        let mut branches = KVec::new_in(self.b);
        let mut branch_idx = 0usize;
        branches.try_push(self.parse_seq(in_group, branch_idx)?)?;
        while let Some(n) = self.alt_len() {
            self.pos += n;
            branch_idx += 1;
            branches.try_push(self.parse_seq(in_group, branch_idx)?)?;
        }
        if branches.len() == 1 {
            Ok(branches.pop().unwrap_or(Ast::Empty))
        } else {
            Ok(Ast::Alt(branches))
        }
    }

    fn parse_seq(&mut self, in_group: bool, branch_idx: usize) -> Result<Ast, RegexError> {
        let mut items = KVec::new_in(self.b);
        self.star_lit = self.is_bre_escaped_ops();
        loop {
            if self.peek().is_none() || self.alt_len().is_some() {
                break;
            }
            if self.close_len().is_some() {
                if in_group {
                    break;
                }
                return self.err(SyntaxKind::UnmatchedCloseParen);
            }
            let first = items.is_empty();
            // Handle Perl `\Q...\E` literal quoting by pushing each scalar individually
            // so a trailing quantifier applies only to the final scalar if any.
            if self.perl
                && self.pat.get(self.pos) == Some(&b'\\')
                && self.pat.get(self.pos + 1) == Some(&b'Q')
            {
                self.pos += 2;
                let mut pushed_any = false;
                while self.pos < self.pat.len() {
                    if self.pat.get(self.pos) == Some(&b'\\')
                        && self.pat.get(self.pos + 1) == Some(&b'E')
                    {
                        self.pos += 2;
                        break;
                    }
                    let (ch, len) = decode_scalar(self.pat, self.pos);
                    self.pos += len;
                    items.try_push(self.char_ast(ch))?;
                    pushed_any = true;
                }
                if pushed_any {
                    let last = items.pop().unwrap_or(Ast::Empty);
                    let wrapped = self.parse_suffix(last)?;
                    items.try_push(wrapped)?;
                }
                self.star_lit = false;
                continue;
            }
            let atom = self.parse_atom(first, branch_idx)?;
            let quantifiable = !matches!(atom, Ast::Assert(_) | Ast::Keep | Ast::Empty);
            let atom = if quantifiable {
                self.parse_suffix(atom)?
            } else {
                // In ERE/Perl, a quantifier immediately after an assertion/empty group
                // is a dangling quantifier (except `^*` in BRE where `star_lit` handles it).
                if !self.is_bre_escaped_ops() && self.starts_ere_quantifier() {
                    return self.err(SyntaxKind::DanglingQuantifier);
                }
                atom
            };
            if !matches!(atom, Ast::Empty) {
                items.try_push(atom)?;
            }
            self.star_lit = self.is_bre_escaped_ops() && self.caret;
        }
        if items.is_empty() {
            Ok(Ast::Empty)
        } else if items.len() == 1 {
            Ok(items.pop().unwrap_or(Ast::Empty))
        } else {
            Ok(Ast::Seq(items))
        }
    }

    fn starts_ere_quantifier(&self) -> bool {
        matches!(self.peek(), Some(b'*' | b'+' | b'?'))
    }

    fn parse_atom(&mut self, first: bool, branch_idx: usize) -> Result<Ast, RegexError> {
        self.caret = false;
        let c = self.peek().unwrap_or(0);
        if self.is_bre_escaped_ops() {
            self.parse_atom_bre(c, first, branch_idx)
        } else {
            self.parse_atom_ere(c)
        }
    }

    fn parse_atom_bre(&mut self, c: u8, first: bool, branch_idx: usize) -> Result<Ast, RegexError> {
        match c {
            b'\\' => {
                let Some(&nc) = self.pat.get(self.pos + 1) else {
                    return self.err(SyntaxKind::TrailingBackslash);
                };
                match (self.mode, nc) {
                    (_, b'(') => {
                        self.pos += 2;
                        let save_star = self.star_lit;
                        let index = self.open_capture()?;
                        self.push_scope()?;
                        let inner = self.parse_alt(true)?;
                        match self.close_len() {
                            Some(n) => self.pos += n,
                            None => return self.err(SyntaxKind::UnclosedGroup),
                        }
                        self.pop_scope();
                        self.depth -= 1;
                        self.star_lit = save_star;
                        Ok(Ast::Group {
                            inner: self.boxed(inner)?,
                            index,
                        })
                    }
                    (RegexMode::Basic(BreDialect::Sed), b'+' | b'?') => {
                        self.err(SyntaxKind::DanglingQuantifier)
                    }
                    (_, b'{') => {
                        let save = self.pos;
                        self.pos += 2;
                        match self.try_interval(true) {
                            Ok(_) => {
                                self.pos = save;
                                self.err(SyntaxKind::DanglingQuantifier)
                            }
                            Err(IntervalErr::NotAnInterval) => {
                                self.pos = save + 2;
                                Ok(self.char_ast(b'{' as u32))
                            }
                            Err(IntervalErr::Fail(e)) => Err(e),
                        }
                    }
                    _ => {
                        self.pos += 2;
                        self.common_escape(nc)
                    }
                }
            }
            b'*' => {
                if self.star_lit {
                    self.pos += 1;
                    Ok(self.char_ast(b'*' as u32))
                } else {
                    self.err(SyntaxKind::DanglingQuantifier)
                }
            }
            b'.' => {
                self.pos += 1;
                Ok(Ast::Dot { dot_all: self.cur.s })
            }
            b'^' => {
                self.pos += 1;
                self.caret = true;
                let anchor_ok = match self.mode {
                    RegexMode::Basic(BreDialect::Sed) => first && branch_idx == 0,
                    _ => first,
                };
                if anchor_ok {
                    Ok(Ast::Assert(if self.cur.m {
                        AssertKind::StartLine
                    } else {
                        AssertKind::StartText
                    }))
                } else {
                    Ok(self.char_ast(b'^' as u32))
                }
            }
            b'$' => {
                self.pos += 1;
                let at_end = match self.mode {
                    RegexMode::Basic(BreDialect::Sed) => {
                        self.pos >= self.pat.len() || self.close_len().is_some()
                    }
                    _ => {
                        self.pos >= self.pat.len()
                            || self.close_len().is_some()
                            || self.alt_len().is_some()
                    }
                };
                if at_end {
                    Ok(Ast::Assert(if self.cur.m {
                        AssertKind::EndLine
                    } else {
                        AssertKind::EndText
                    }))
                } else {
                    Ok(self.char_ast(b'$' as u32))
                }
            }
            b'[' => self.parse_bracket_ast(),
            b'(' | b')' | b'|' | b'+' | b'?' | b'{' | b'}' => {
                self.pos += 1;
                Ok(self.char_ast(c as u32))
            }
            _ => {
                let (ch, len) = decode_scalar(self.pat, self.pos);
                self.pos += len;
                Ok(self.char_ast(ch))
            }
        }
    }

    fn parse_atom_ere(&mut self, c: u8) -> Result<Ast, RegexError> {
        match c {
            b'\\' => {
                let Some(&nc) = self.pat.get(self.pos + 1) else {
                    return self.err(SyntaxKind::TrailingBackslash);
                };
                self.pos += 2;
                if self.perl {
                    self.perl_escape(nc)
                } else {
                    self.common_escape(nc)
                }
            }
            b'*' | b'+' | b'?' => self.err(SyntaxKind::DanglingQuantifier),
            b'{' => {
                let save = self.pos;
                self.pos += 1;
                match self.try_interval(false) {
                    Ok(_) => {
                        self.pos = save;
                        self.err(SyntaxKind::DanglingQuantifier)
                    }
                    Err(IntervalErr::NotAnInterval) => {
                        self.pos = save + 1;
                        Ok(self.char_ast(b'{' as u32))
                    }
                    Err(IntervalErr::Fail(e)) => Err(e),
                }
            }
            b'.' => {
                self.pos += 1;
                Ok(Ast::Dot { dot_all: self.cur.s })
            }
            b'^' => {
                self.pos += 1;
                Ok(Ast::Assert(if self.cur.m {
                    AssertKind::StartLine
                } else {
                    AssertKind::StartText
                }))
            }
            b'$' => {
                self.pos += 1;
                Ok(Ast::Assert(if self.cur.m {
                    AssertKind::EndLine
                } else {
                    AssertKind::EndText
                }))
            }
            b'[' => self.parse_bracket_ast(),
            b'(' => self.parse_ere_group(),
            _ => {
                let (ch, len) = decode_scalar(self.pat, self.pos);
                self.pos += len;
                Ok(self.char_ast(ch))
            }
        }
    }

    fn parse_ere_group(&mut self) -> Result<Ast, RegexError> {
        self.pos += 1; // consume '('
        if self.perl && self.peek() == Some(b'?') {
            self.pos += 1; // consume '?'
            return self.parse_perl_special_group();
        }
        let index = self.open_capture()?;
        self.push_scope()?;
        let inner = self.parse_alt(true)?;
        if self.peek() != Some(b')') {
            return self.err(SyntaxKind::UnclosedGroup);
        }
        self.pos += 1;
        self.pop_scope();
        self.depth -= 1;
        Ok(Ast::Group {
            inner: self.boxed(inner)?,
            index,
        })
    }

    fn parse_perl_special_group(&mut self) -> Result<Ast, RegexError> {
        match self.peek() {
            Some(b'=') | Some(b'!') => self.err(SyntaxKind::LookaroundUnsupported),
            Some(b'<') => {
                match self.pat.get(self.pos + 1) {
                    Some(b'=') | Some(b'!') => return self.err(SyntaxKind::LookaroundUnsupported),
                    _ => {}
                }
                self.pos += 1; // consume '<'
                self.parse_named_group()
            }
            Some(b'P') if self.pat.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2; // consume 'P<'
                self.parse_named_group()
            }
            Some(b':') => {
                self.pos += 1;
                self.enter_nesting()?;
                self.push_scope()?;
                let inner = self.parse_alt(true)?;
                if self.peek() != Some(b')') {
                    return self.err(SyntaxKind::UnclosedGroup);
                }
                self.pos += 1;
                self.pop_scope();
                self.depth -= 1;
                Ok(inner)
            }
            Some(b'i' | b'm' | b's' | b'-') => {
                let mut next_flags = self.cur;
                let mut neg = false;
                while let Some(c) = self.peek() {
                    match c {
                        b'i' => {
                            next_flags.i = !neg;
                            self.pos += 1;
                        }
                        b'm' => {
                            next_flags.m = !neg;
                            self.pos += 1;
                        }
                        b's' => {
                            next_flags.s = !neg;
                            self.pos += 1;
                        }
                        b'-' if !neg => {
                            neg = true;
                            self.pos += 1;
                        }
                        b':' => {
                            self.pos += 1;
                            self.enter_nesting()?;
                            self.push_scope()?;
                            self.cur = next_flags;
                            let inner = self.parse_alt(true)?;
                            if self.peek() != Some(b')') {
                                return self.err(SyntaxKind::UnclosedGroup);
                            }
                            self.pos += 1;
                            self.pop_scope();
                            self.depth -= 1;
                            return Ok(inner);
                        }
                        b')' => {
                            self.pos += 1;
                            self.cur = next_flags;
                            return Ok(Ast::Empty);
                        }
                        _ => return self.err(SyntaxKind::InvalidEscape),
                    }
                }
                self.err(SyntaxKind::UnclosedGroup)
            }
            None => self.err(SyntaxKind::UnclosedGroup),
            _ => self.err(SyntaxKind::InvalidEscape),
        }
    }

    fn parse_named_group(&mut self) -> Result<Ast, RegexError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == b'>' {
                break;
            }
            self.pos += 1;
        }
        if self.peek() != Some(b'>') {
            return self.err(SyntaxKind::InvalidCaptureName);
        }
        let name = &self.pat[start..self.pos];
        self.check_name(name)?;
        self.pos += 1; // consume '>'
        let index = self.open_capture()?;
        self.add_name(name, index)?;
        self.push_scope()?;
        let inner = self.parse_alt(true)?;
        if self.peek() != Some(b')') {
            return self.err(SyntaxKind::UnclosedGroup);
        }
        self.pos += 1;
        self.pop_scope();
        self.depth -= 1;
        Ok(Ast::Group {
            inner: self.boxed(inner)?,
            index,
        })
    }

    fn common_escape(&mut self, nc: u8) -> Result<Ast, RegexError> {
        match nc {
            b't' => Ok(self.char_ast(0x09)),
            b'n' => Ok(self.char_ast(0x0A)),
            b'r' => Ok(self.char_ast(0x0D)),
            b'b' => Ok(Ast::Assert(AssertKind::WordBound)),
            b'B' => Ok(Ast::Assert(AssertKind::NotWordBound)),
            b'w' | b'W' | b's' | b'S' | b'd' | b'D' => self.class_escape(nc),
            b'1'..=b'9' => {
                let n = nc - b'0';
                self.refs.try_push(n)?;
                self.has_backrefs = true;
                Ok(Ast::Backref {
                    group: n,
                    icase: self.cur.i,
                })
            }
            _ if nc.is_ascii_alphanumeric() => self.err(SyntaxKind::InvalidEscape),
            _ => {
                // Non-ASCII lead byte after `\` decodes the full scalar.
                if nc >= 0x80 {
                    self.pos -= 1;
                    let (ch, len) = decode_scalar(self.pat, self.pos);
                    self.pos += len;
                    Ok(self.char_ast(ch))
                } else {
                    Ok(self.char_ast(nc as u32))
                }
            }
        }
    }

    fn perl_escape(&mut self, nc: u8) -> Result<Ast, RegexError> {
        match nc {
            b'f' => Ok(self.char_ast(0x0C)),
            b'v' => Ok(self.char_ast(0x0B)),
            b'K' => Ok(Ast::Keep),
            b'e' | b'a' => self.err(SyntaxKind::InvalidEscape),
            b'x' => {
                let ch = self.parse_hex_escape()?;
                Ok(self.char_ast(ch))
            }
            b'0' => {
                let mut val = 0u32;
                let mut count = 0;
                while count < 2 {
                    if let Some(d @ b'0'..=b'7') = self.peek() {
                        val = (val << 3) | ((d - b'0') as u32);
                        self.pos += 1;
                        count += 1;
                    } else {
                        break;
                    }
                }
                Ok(self.char_ast(val))
            }
            b'1'..=b'7' => {
                // 3-digit octal (`\101` = 'A', up to `\377`) vs single-digit backref `\1`..`\7`.
                if let (Some(d1 @ b'0'..=b'7'), Some(d2 @ b'0'..=b'7')) =
                    (self.pat.get(self.pos).copied(), self.pat.get(self.pos + 1).copied())
                {
                    let val = (((nc - b'0') as u32) << 6)
                        | (((d1 - b'0') as u32) << 3)
                        | ((d2 - b'0') as u32);
                    if val <= 0xFF {
                        self.pos += 2;
                        return Ok(self.char_ast(val));
                    }
                }
                let n = nc - b'0';
                self.refs.try_push(n)?;
                self.has_backrefs = true;
                Ok(Ast::Backref {
                    group: n,
                    icase: self.cur.i,
                })
            }
            _ => self.common_escape(nc),
        }
    }

    fn parse_hex_escape(&mut self) -> Result<u32, RegexError> {
        if self.peek() == Some(b'{') {
            self.pos += 1;
            let mut val = 0u32;
            let mut digits = 0u32;
            while let Some(c) = self.peek() {
                if c == b'}' {
                    self.pos += 1;
                    if digits == 0 || digits > 6 {
                        return self.err(SyntaxKind::InvalidEscape);
                    }
                    if char::from_u32(val).is_none() {
                        return self.err(SyntaxKind::InvalidEscape);
                    }
                    return Ok(val);
                }
                let Some(d) = hex_val(c) else {
                    return self.err(SyntaxKind::InvalidEscape);
                };
                val = val.saturating_mul(16).saturating_add(d);
                digits += 1;
                if digits > 6 {
                    return self.err(SyntaxKind::InvalidEscape);
                }
                self.pos += 1;
            }
            return self.err(SyntaxKind::InvalidEscape);
        }
        let Some(h1) = self.pat.get(self.pos).copied().and_then(hex_val) else {
            return self.err(SyntaxKind::InvalidEscape);
        };
        let Some(h2) = self.pat.get(self.pos + 1).copied().and_then(hex_val) else {
            return self.err(SyntaxKind::InvalidEscape);
        };
        self.pos += 2;
        Ok((h1 << 4) | h2)
    }

    fn parse_suffix(&mut self, mut atom: Ast) -> Result<Ast, RegexError> {
        loop {
            let (min, max) = if self.is_bre_escaped_ops() {
                match self.peek() {
                    Some(b'*') => {
                        self.pos += 1;
                        (0, None)
                    }
                    Some(b'\\') => match self.pat.get(self.pos + 1) {
                        Some(b'+') if matches!(self.mode, RegexMode::Basic(BreDialect::Sed)) => {
                            self.pos += 2;
                            (1, None)
                        }
                        Some(b'?') if matches!(self.mode, RegexMode::Basic(BreDialect::Sed)) => {
                            self.pos += 2;
                            (0, Some(1))
                        }
                        Some(b'{') => {
                            let save = self.pos;
                            self.pos += 2;
                            match self.try_interval(true) {
                                Ok(pair) => pair,
                                Err(IntervalErr::NotAnInterval) => {
                                    self.pos = save;
                                    break;
                                }
                                Err(IntervalErr::Fail(e)) => return Err(e),
                            }
                        }
                        _ => break,
                    },
                    _ => break,
                }
            } else {
                match self.peek() {
                    Some(b'*') => {
                        self.pos += 1;
                        (0, None)
                    }
                    Some(b'+') => {
                        self.pos += 1;
                        (1, None)
                    }
                    Some(b'?') => {
                        self.pos += 1;
                        (0, Some(1))
                    }
                    Some(b'{') => {
                        let save = self.pos;
                        self.pos += 1;
                        match self.try_interval(false) {
                            Ok(pair) => pair,
                            Err(IntervalErr::NotAnInterval) => {
                                self.pos = save;
                                break;
                            }
                            Err(IntervalErr::Fail(e)) => return Err(e),
                        }
                    }
                    _ => break,
                }
            };
            let lazy = if self.perl && self.peek() == Some(b'?') {
                self.pos += 1;
                true
            } else {
                false
            };
            atom = Ast::Repeat {
                inner: self.boxed(atom)?,
                min,
                max,
                lazy,
            };
        }
        Ok(atom)
    }

    /// Parses an interval body after `{` (if `!bre`) or `\{` (if `bre`).
    fn try_interval(&mut self, bre: bool) -> Result<(u32, Option<u32>), IntervalErr> {
        let mut min: Option<u32> = None;
        let mut min_overflow = false;
        let mut has_min_digits = false;
        while let Some(d @ b'0'..=b'9') = self.peek() {
            has_min_digits = true;
            let cur = min.unwrap_or(0);
            match cur
                .checked_mul(10)
                .and_then(|v| v.checked_add((d - b'0') as u32))
            {
                Some(v) => min = Some(v),
                None => min_overflow = true,
            }
            self.pos += 1;
        }
        let mut has_comma = false;
        let mut max: Option<u32> = None;
        let mut max_overflow = false;
        if self.peek() == Some(b',') {
            has_comma = true;
            self.pos += 1;
            while let Some(d @ b'0'..=b'9') = self.peek() {
                let cur = max.unwrap_or(0);
                match cur
                    .checked_mul(10)
                    .and_then(|v| v.checked_add((d - b'0') as u32))
                {
                    Some(v) => max = Some(v),
                    None => max_overflow = true,
                }
                self.pos += 1;
            }
        }
        // Check closing `}` or `\}`.
        if bre {
            if self.pat.get(self.pos) != Some(&b'\\')
                || self.pat.get(self.pos + 1) != Some(&b'}')
            {
                return Err(IntervalErr::NotAnInterval);
            }
            self.pos += 2;
        } else {
            if self.peek() != Some(b'}') {
                return Err(IntervalErr::NotAnInterval);
            }
            self.pos += 1;
        }

        if !has_min_digits {
            // `{}` or `{,}` or `{,m}`: only valid when `comma_first` and `m` is present.
            if has_comma && max.is_some() && self.comma_first {
                min = Some(0);
            } else {
                return Err(IntervalErr::NotAnInterval);
            }
        }

        let limit = self.limits.max_repeat_expansion;
        let min_val = min.unwrap_or(0);
        if min_overflow || min_val > limit {
            return Err(IntervalErr::Fail(RegexError::CompileCap(
                CapKind::RepeatTooLarge { limit },
            )));
        }
        let max_val = if !has_comma {
            Some(min_val)
        } else if let Some(m) = max {
            if max_overflow || m > limit {
                return Err(IntervalErr::Fail(RegexError::CompileCap(
                    CapKind::RepeatTooLarge { limit },
                )));
            }
            if m < min_val {
                return Err(IntervalErr::Fail(RegexError::Syntax(
                    SyntaxKind::InvalidRepeatRange,
                )));
            }
            Some(m)
        } else {
            None
        };
        Ok((min_val, max_val))
    }

    fn parse_bracket_ast(&mut self) -> Result<Ast, RegexError> {
        // Standalone word-boundary bracket tokens `[[:<:]]` and `[[:>:]]`.
        if self.pat[self.pos..].starts_with(b"[[:<:]]") {
            self.pos += 7;
            return Ok(Ast::Assert(AssertKind::WordStart));
        }
        if self.pat[self.pos..].starts_with(b"[[:>:]]") {
            self.pos += 7;
            return Ok(Ast::Assert(AssertKind::WordEnd));
        }
        self.pos += 1; // consume '['
        // Leading `^` or `!` negates the regex bracket class (matching `just-bash@3.6.0`
        // oracle snapshot `l8a/bracket-edge-cases` step 2).
        let neg = if matches!(self.peek(), Some(b'^' | b'!')) {
            self.pos += 1;
            true
        } else {
            false
        };
        let mut ranges: KVec<(u32, u32)> = KVec::new_in(self.b);
        let mut first = true;
        let mut prev_single: Option<u32> = None;

        while self.pos < self.pat.len() {
            let c = self.pat[self.pos];
            if c == b']' && !first {
                self.pos += 1;
                if let Some(ch) = prev_single.take() {
                    ranges.try_push((ch, ch))?;
                }
                let id = self.classes.len() as u32;
                self.classes.try_push(CharClass {
                    neg,
                    icase: self.cur.i,
                    ranges,
                })?;
                return Ok(Ast::Class(id));
            }
            first = false;

            // POSIX named class `[:name:]`.
            if c == b'[' && self.pat.get(self.pos + 1) == Some(&b':') {
                if let Some(end_rel) = find_colon_close(&self.pat[self.pos + 2..]) {
                    if let Some(ch) = prev_single.take() {
                        ranges.try_push((ch, ch))?;
                    }
                    let name = &self.pat[self.pos + 2..self.pos + 2 + end_rel];
                    let Some(set) = posix_ranges(name) else {
                        return self.err(SyntaxKind::UnknownPosixClass);
                    };
                    ranges.try_extend_from_slice(set)?;
                    self.pos += 2 + end_rel + 2;
                    continue;
                }
            }

            // Range `prev_single - next_char` if `-` is NOT immediately followed by `]`.
            if c == b'-' && prev_single.is_some() && self.pat.get(self.pos + 1) != Some(&b']') {
                self.pos += 1; // consume '-'
                let Some(hi) = self.parse_bracket_single()? else {
                    return self.err(SyntaxKind::InvalidCharacterRange);
                };
                let lo = prev_single.take().unwrap_or(0);
                if lo > hi {
                    return self.err(SyntaxKind::InvalidCharacterRange);
                }
                ranges.try_push((lo, hi))?;
                continue;
            }

            if let Some(ch) = prev_single.take() {
                ranges.try_push((ch, ch))?;
            }

            // Escape sequences inside `[...]`.
            if c == b'\\' {
                if let Some(item) = self.parse_bracket_escape(&mut ranges)? {
                    prev_single = Some(item);
                }
                continue;
            }

            let (ch, len) = decode_scalar(self.pat, self.pos);
            self.pos += len;
            prev_single = Some(ch);
        }
        self.err(SyntaxKind::UnclosedBracket)
    }

    /// Reads one single-character endpoint inside `[...]` (used for the right side of `a-z`).
    fn parse_bracket_single(&mut self) -> Result<Option<u32>, RegexError> {
        if self.pos >= self.pat.len() {
            return self.err(SyntaxKind::UnclosedBracket);
        }
        if self.pat[self.pos] == b'[' && self.pat.get(self.pos + 1) == Some(&b':') {
            return Ok(None);
        }
        if self.pat[self.pos] == b'\\' {
            let mut dummy = KVec::new_in(self.b);
            return self.parse_bracket_escape(&mut dummy);
        }
        let (ch, len) = decode_scalar(self.pat, self.pos);
        self.pos += len;
        Ok(Some(ch))
    }

    /// Parses a `\` escape inside `[...]`. Returns `Ok(Some(ch))` for a single scalar,
    /// or `Ok(None)` if it expanded a class shorthand (`\d`, `\w`, `\s`, ...) directly into `ranges`.
    fn parse_bracket_escape(
        &mut self,
        ranges: &mut KVec<(u32, u32)>,
    ) -> Result<Option<u32>, RegexError> {
        let Some(&nc) = self.pat.get(self.pos + 1) else {
            return self.err(SyntaxKind::UnclosedBracket);
        };
        self.pos += 2;
        match nc {
            b't' => Ok(Some(0x09)),
            b'n' => Ok(Some(0x0A)),
            b'r' => Ok(Some(0x0D)),
            b'f' if self.perl => Ok(Some(0x0C)),
            b'v' if self.perl => Ok(Some(0x0B)),
            b'd' | b'w' | b's' => {
                if let Some((_, set)) = shorthand_ranges(nc) {
                    ranges.try_extend_from_slice(set)?;
                }
                Ok(None)
            }
            b'D' | b'W' | b'S' if self.perl => {
                // Complement of ASCII shorthand inside `[...]`.
                let Some((_, set)) = shorthand_ranges(nc.to_ascii_lowercase()) else {
                    return Ok(None);
                };
                push_complement_ranges(set, ranges)?;
                Ok(None)
            }
            b'x' if self.perl => Ok(Some(self.parse_hex_escape()?)),
            b'0' if self.perl => {
                let mut val = 0u32;
                let mut count = 0;
                while count < 2 {
                    if let Some(d @ b'0'..=b'7') = self.peek() {
                        val = (val << 3) | ((d - b'0') as u32);
                        self.pos += 1;
                        count += 1;
                    } else {
                        break;
                    }
                }
                Ok(Some(val))
            }
            _ => {
                if nc >= 0x80 {
                    self.pos -= 1;
                    let (ch, len) = decode_scalar(self.pat, self.pos);
                    self.pos += len;
                    Ok(Some(ch))
                } else {
                    Ok(Some(nc as u32))
                }
            }
        }
    }
}

fn find_colon_close(slice: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < slice.len() {
        if slice[i] == b':' && slice[i + 1] == b']' {
            return Some(i);
        }
        if slice[i] == b']' || slice[i] == b'[' {
            return None;
        }
        i += 1;
    }
    None
}

fn push_complement_ranges(
    sorted_set: &[(u32, u32)],
    out: &mut KVec<(u32, u32)>,
) -> MemResult<()> {
    let mut cur = 0u32;
    for &(lo, hi) in sorted_set {
        if cur < lo {
            out.try_push((cur, lo - 1))?;
        }
        cur = hi.saturating_add(1);
    }
    if cur <= 0x10_FFFF {
        out.try_push((cur, 0x10_FFFF))?;
    }
    // Raw invalid bytes are also outside ASCII shorthand classes.
    out.try_push((RAW_BYTE_BASE, RAW_BYTE_BASE + 0xFF))?;
    Ok(())
}
