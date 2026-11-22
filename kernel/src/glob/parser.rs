//! Glob and bash `extglob` parser, globstar counter, and `GLOBIGNORE` splitter.

use super::{GlobError, GlobOptions, MAX_GLOBSTAR_SEGMENTS};
use crate::mem::{Budget, KBox, KVec, MemError};
use crate::regex::{decode_scalar, posix_ranges};

pub(crate) struct GlobClass {
    pub neg: bool,
    pub ranges: KVec<(u32, u32)>,
}

pub(crate) enum GlobAst {
    Empty,
    Lit(u32),
    Question,
    Star,
    Class(u32),
    Seq(KVec<GlobAst>),
    Alt(KVec<GlobAst>),
    /// `@(p1|p2)` -> `Alt`; `?(...)` -> `Optional`; `*(...)` -> `StarOf`; `+(...)` -> `PlusOf`; `!(...)` -> `Not`.
    Optional(KBox<GlobAst>),
    StarOf(KBox<GlobAst>),
    PlusOf(KBox<GlobAst>),
    Not(KBox<GlobAst>),
}

pub(crate) struct ParsedGlob {
    pub root: GlobAst,
    pub classes: KVec<GlobClass>,
    pub has_not: bool,
}

pub(crate) fn check_globstar_segments(path_pattern: &[u8]) -> Result<usize, GlobError> {
    let mut count = 0usize;
    let mut start = 0usize;
    for (i, &c) in path_pattern.iter().enumerate() {
        if c == b'/' {
            if &path_pattern[start..i] == b"**" {
                count += 1;
            }
            start = i + 1;
        }
    }
    if &path_pattern[start..] == b"**" {
        count += 1;
    }
    if count > MAX_GLOBSTAR_SEGMENTS {
        return Err(GlobError::TooManyGlobstars {
            max: MAX_GLOBSTAR_SEGMENTS,
        });
    }
    Ok(count)
}

pub(crate) fn split_globignore<'a>(raw: &'a [u8], b: &Budget) -> Result<KVec<&'a [u8]>, MemError> {
    let mut out = KVec::new_in(b);
    let mut start = 0usize;
    let mut i = 0usize;
    let mut in_bracket = false;
    let mut bracket_first = false;

    while i < raw.len() {
        let c = raw[i];
        if !in_bracket {
            if c == b'\\' && i + 1 < raw.len() {
                i += 2;
                continue;
            }
            if c == b'[' {
                // Only enter bracket mode if a closing `]` exists later.
                if has_closing_bracket(raw, i) {
                    in_bracket = true;
                    bracket_first = true;
                    i += 1;
                    if raw.get(i) == Some(&b'^') {
                        i += 1;
                    }
                    continue;
                }
            }
            if c == b':' {
                if i > start {
                    out.try_push(&raw[start..i])?;
                }
                start = i + 1;
            }
            i += 1;
        } else {
            if c == b'\\' && i + 1 < raw.len() {
                bracket_first = false;
                i += 2;
                continue;
            }
            if c == b'[' && raw.get(i + 1) == Some(&b':') {
                if let Some(end_rel) = find_colon_close(&raw[i + 2..]) {
                    bracket_first = false;
                    i += 2 + end_rel + 2;
                    continue;
                }
            }
            if c == b']' && !bracket_first {
                in_bracket = false;
                i += 1;
                continue;
            }
            bracket_first = false;
            i += 1;
        }
    }
    if raw.len() > start {
        out.try_push(&raw[start..])?;
    }
    Ok(out)
}

struct GlobParser<'a> {
    pat: &'a [u8],
    pos: usize,
    opts: GlobOptions,
    depth: u32,
    classes: KVec<GlobClass>,
    has_not: bool,
    b: &'a Budget,
}

pub(crate) fn parse_glob(
    pattern: &[u8],
    opts: GlobOptions,
    b: &Budget,
) -> Result<ParsedGlob, GlobError> {
    let mut p = GlobParser {
        pat: pattern,
        pos: 0,
        opts,
        depth: 0,
        classes: KVec::new_in(b),
        has_not: false,
        b,
    };
    let root = p.parse_seq(false)?;
    Ok(ParsedGlob {
        root,
        classes: p.classes,
        has_not: p.has_not,
    })
}

impl<'a> GlobParser<'a> {
    fn boxed(&self, ast: GlobAst) -> Result<KBox<GlobAst>, GlobError> {
        KBox::try_new_in(ast, self.b).map_err(|(_, e)| GlobError::Mem(e))
    }

    fn parse_seq(&mut self, in_extglob: bool) -> Result<GlobAst, GlobError> {
        let mut items = KVec::new_in(self.b);
        while self.pos < self.pat.len() {
            let c = self.pat[self.pos];
            if in_extglob && (c == b'|' || c == b')') {
                break;
            }
            if self.opts.extglob
                && matches!(c, b'@' | b'*' | b'+' | b'?' | b'!')
                && self.pat.get(self.pos + 1) == Some(&b'(')
            {
                if let Some(node) = self.try_extglob(c)? {
                    items.try_push(node)?;
                    continue;
                }
            }
            match c {
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.pat.len() {
                        let (ch, len) = decode_scalar(self.pat, self.pos);
                        self.pos += len;
                        items.try_push(GlobAst::Lit(ch))?;
                    } else {
                        items.try_push(GlobAst::Lit(b'\\' as u32))?;
                    }
                }
                b'*' => {
                    self.pos += 1;
                    // Collapse consecutive `*` into a single `Star`.
                    if !matches!(items.as_slice().last(), Some(GlobAst::Star)) {
                        items.try_push(GlobAst::Star)?;
                    }
                }
                b'?' => {
                    self.pos += 1;
                    items.try_push(GlobAst::Question)?;
                }
                b'[' => {
                    if let Some(cls) = self.try_bracket()? {
                        items.try_push(cls)?;
                    } else {
                        self.pos += 1;
                        items.try_push(GlobAst::Lit(b'[' as u32))?;
                    }
                }
                _ => {
                    let (ch, len) = decode_scalar(self.pat, self.pos);
                    self.pos += len;
                    items.try_push(GlobAst::Lit(ch))?;
                }
            }
        }
        if items.is_empty() {
            Ok(GlobAst::Empty)
        } else if items.len() == 1 {
            Ok(items.pop().unwrap_or(GlobAst::Empty))
        } else {
            Ok(GlobAst::Seq(items))
        }
    }

    fn try_extglob(&mut self, op: u8) -> Result<Option<GlobAst>, GlobError> {
        let save = self.pos;
        if self.depth >= self.opts.max_extglob_depth {
            return Err(GlobError::ExtglobDepthExceeded {
                max: self.opts.max_extglob_depth,
            });
        }
        self.pos += 2; // consume `<op>(`
        self.depth += 1;
        let mut branches = KVec::new_in(self.b);
        loop {
            let branch = self.parse_seq(true)?;
            branches.try_push(branch)?;
            match self.pat.get(self.pos).copied() {
                Some(b'|') => {
                    self.pos += 1;
                }
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    // No matching `)`: backtrack and treat `<op>` normally.
                    self.depth -= 1;
                    self.pos = save;
                    return Ok(None);
                }
            }
        }
        self.depth -= 1;
        let inner = if branches.len() == 1 {
            branches.pop().unwrap_or(GlobAst::Empty)
        } else {
            GlobAst::Alt(branches)
        };
        let node = match op {
            b'@' => inner,
            b'?' => GlobAst::Optional(self.boxed(inner)?),
            b'*' => GlobAst::StarOf(self.boxed(inner)?),
            b'+' => GlobAst::PlusOf(self.boxed(inner)?),
            b'!' => {
                self.has_not = true;
                GlobAst::Not(self.boxed(inner)?)
            }
            _ => unreachable!(),
        };
        Ok(Some(node))
    }

    fn try_bracket(&mut self) -> Result<Option<GlobAst>, GlobError> {
        if !has_closing_bracket(self.pat, self.pos) {
            return Ok(None);
        }
        let save = self.pos;
        self.pos += 1; // consume '['
        // Only leading `^` negates in glob brackets (`!` is an ordinary member, per spec 6.1).
        let neg = if self.pat.get(self.pos) == Some(&b'^') {
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
                self.classes.try_push(GlobClass { neg, ranges })?;
                return Ok(Some(GlobAst::Class(id)));
            }
            first = false;

            // POSIX class `[:name:]`: unknown class contributes 0 characters without error.
            if c == b'[' && self.pat.get(self.pos + 1) == Some(&b':') {
                if let Some(end_rel) = find_colon_close(&self.pat[self.pos + 2..]) {
                    if let Some(ch) = prev_single.take() {
                        ranges.try_push((ch, ch))?;
                    }
                    let name = &self.pat[self.pos + 2..self.pos + 2 + end_rel];
                    if let Some(set) = posix_ranges(name) {
                        ranges.try_extend_from_slice(set)?;
                    }
                    self.pos += 2 + end_rel + 2;
                    continue;
                }
            }

            // Character range `lo-hi` when `-` is not immediately before `]`.
            if c == b'-' && prev_single.is_some() && self.pat.get(self.pos + 1) != Some(&b']') {
                self.pos += 1; // consume '-'
                let hi = self.parse_bracket_char()?;
                let lo = prev_single.take().unwrap_or(0);
                if lo > hi {
                    return Err(GlobError::InvalidCharacterRange);
                }
                ranges.try_push((lo, hi))?;
                continue;
            }

            if let Some(ch) = prev_single.take() {
                ranges.try_push((ch, ch))?;
            }

            let ch = self.parse_bracket_char()?;
            prev_single = Some(ch);
        }

        // Unclosed bracket fallback (should not be hit due to `has_closing_bracket`).
        self.pos = save;
        Ok(None)
    }

    fn parse_bracket_char(&mut self) -> Result<u32, GlobError> {
        if self.pat[self.pos] == b'\\' && self.pos + 1 < self.pat.len() {
            self.pos += 1;
        }
        let (ch, len) = decode_scalar(self.pat, self.pos);
        self.pos += len;
        Ok(ch)
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

fn has_closing_bracket(pat: &[u8], open: usize) -> bool {
    let mut i = open + 1;
    if pat.get(i) == Some(&b'^') {
        i += 1;
    }
    let mut first = true;
    while i < pat.len() {
        let c = pat[i];
        if c == b'\\' && i + 1 < pat.len() {
            first = false;
            i += 2;
            continue;
        }
        if c == b'[' && pat.get(i + 1) == Some(&b':') {
            if let Some(end_rel) = find_colon_close(&pat[i + 2..]) {
                first = false;
                i += 2 + end_rel + 2;
                continue;
            }
        }
        if c == b']' && !first {
            return true;
        }
        first = false;
        i += 1;
    }
    false
}
