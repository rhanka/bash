//! Glob and bash `extglob` parser, globstar counter, and `GLOBIGNORE` splitter.
//!
//! Produces a flat post-order node arena (`ParsedGlob::nodes`) with an explicit parse
//! stack and an `O(N)` extglob parenthesis pre-pass so neither deeply nested extglobs
//! nor unclosed `<op>(` sequences can trigger recursive stack overflow or exponential
//! parser backtracking.

use super::{GlobError, GlobOptions, MAX_GLOBSTAR_SEGMENTS};
use crate::mem::{Budget, KVec, MemError};
use crate::regex::{decode_scalar, posix_ranges};

pub(crate) struct GlobClass {
    pub neg: bool,
    pub ranges: KVec<(u32, u32)>,
}

pub(crate) enum GlobNode {
    Empty,
    Lit(u32),
    Question,
    Star,
    Class(u32),
    Seq(KVec<u32>),
    Alt(KVec<u32>),
    /// `@(p1|p2)` -> `Alt`; `?(...)` -> `Optional`; `*(...)` -> `StarOf`; `+(...)` -> `PlusOf`; `!(...)` -> `Not`.
    Optional(u32),
    StarOf(u32),
    PlusOf(u32),
    Not(u32),
}

pub(crate) struct ParsedGlob {
    pub nodes: KVec<GlobNode>,
    pub node_has_not: KVec<bool>,
    pub root: u32,
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
    let last_rbracket = raw.iter().rposition(|&c| c == b']');
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
                if find_closing_bracket(raw, i, last_rbracket).is_some() {
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

struct ParseFrame {
    op: Option<u8>,
    branches: KVec<u32>,
    items: KVec<u32>,
}

struct GlobParser<'a> {
    pat: &'a [u8],
    pos: usize,
    last_rbracket: Option<usize>,
    nodes: KVec<GlobNode>,
    node_has_not: KVec<bool>,
    classes: KVec<GlobClass>,
    b: &'a Budget,
}

pub(crate) fn parse_glob(
    pattern: &[u8],
    opts: GlobOptions,
    b: &Budget,
) -> Result<ParsedGlob, GlobError> {
    let last_rbracket = pattern.iter().rposition(|&c| c == b']');
    let valid_extglob = if opts.extglob {
        compute_valid_extglobs(pattern, last_rbracket, b)?
    } else {
        KVec::new_in(b)
    };

    let mut p = GlobParser {
        pat: pattern,
        pos: 0,
        last_rbracket,
        nodes: KVec::new_in(b),
        node_has_not: KVec::new_in(b),
        classes: KVec::new_in(b),
        b,
    };

    let mut frames: KVec<ParseFrame> = KVec::new_in(b);
    frames.try_push(ParseFrame {
        op: None,
        branches: KVec::new_in(b),
        items: KVec::new_in(b),
    })?;

    while p.pos < p.pat.len() {
        let c = p.pat[p.pos];
        let in_extglob = frames.len() > 1;

        if in_extglob && c == b'|' {
            p.pos += 1;
            let top = frames.as_mut_slice().last_mut().unwrap();
            let seq_items = core::mem::replace(&mut top.items, KVec::new_in(b));
            let branch_id = p.finish_seq(seq_items)?;
            frames.as_mut_slice().last_mut().unwrap().branches.try_push(branch_id)?;
            continue;
        }

        if in_extglob && c == b')' {
            p.pos += 1;
            let mut frame = frames.pop().unwrap();
            let last_branch = p.finish_seq(frame.items)?;
            frame.branches.try_push(last_branch)?;
            let inner = if frame.branches.len() == 1 {
                frame.branches.pop().unwrap()
            } else {
                p.push_node(GlobNode::Alt(frame.branches))?
            };
            let node_id = match frame.op.unwrap_or(b'@') {
                b'@' => inner,
                b'?' => p.push_node(GlobNode::Optional(inner))?,
                b'*' => p.push_node(GlobNode::StarOf(inner))?,
                b'+' => p.push_node(GlobNode::PlusOf(inner))?,
                b'!' => p.push_node(GlobNode::Not(inner))?,
                _ => unreachable!(),
            };
            frames.as_mut_slice().last_mut().unwrap().items.try_push(node_id)?;
            continue;
        }

        if opts.extglob && valid_extglob.as_slice()[p.pos] {
            let cur_depth = (frames.len() - 1) as u32;
            if cur_depth >= opts.max_extglob_depth {
                return Err(GlobError::ExtglobDepthExceeded {
                    max: opts.max_extglob_depth,
                });
            }
            p.pos += 2; // consume `<op>(`
            frames.try_push(ParseFrame {
                op: Some(c),
                branches: KVec::new_in(b),
                items: KVec::new_in(b),
            })?;
            continue;
        }

        match c {
            b'\\' => {
                p.pos += 1;
                let ch = if p.pos < p.pat.len() {
                    let (ch, len) = decode_scalar(p.pat, p.pos);
                    p.pos += len;
                    ch
                } else {
                    b'\\' as u32
                };
                let id = p.push_node(GlobNode::Lit(ch))?;
                frames.as_mut_slice().last_mut().unwrap().items.try_push(id)?;
            }
            b'*' => {
                p.pos += 1;
                // Collapse consecutive `*` into a single `Star`.
                let prev_is_star = frames
                    .as_slice()
                    .last()
                    .and_then(|f| f.items.as_slice().last().copied())
                    .is_some_and(|id| matches!(p.nodes.as_slice()[id as usize], GlobNode::Star));
                if !prev_is_star {
                    let id = p.push_node(GlobNode::Star)?;
                    frames.as_mut_slice().last_mut().unwrap().items.try_push(id)?;
                }
            }
            b'?' => {
                p.pos += 1;
                let id = p.push_node(GlobNode::Question)?;
                frames.as_mut_slice().last_mut().unwrap().items.try_push(id)?;
            }
            b'[' => {
                let id = if let Some(cid) = p.try_bracket()? {
                    p.push_node(GlobNode::Class(cid))?
                } else {
                    p.pos += 1;
                    p.push_node(GlobNode::Lit(b'[' as u32))?
                };
                frames.as_mut_slice().last_mut().unwrap().items.try_push(id)?;
            }
            _ => {
                let (ch, len) = decode_scalar(p.pat, p.pos);
                p.pos += len;
                let id = p.push_node(GlobNode::Lit(ch))?;
                frames.as_mut_slice().last_mut().unwrap().items.try_push(id)?;
            }
        }
    }

    let root_frame = frames.pop().unwrap();
    let root = p.finish_seq(root_frame.items)?;
    let has_not = p.node_has_not.as_slice()[root as usize];
    Ok(ParsedGlob {
        nodes: p.nodes,
        node_has_not: p.node_has_not,
        root,
        classes: p.classes,
        has_not,
    })
}

/// Linear-time pre-pass marking every `<op>(` position that has a matching `)` later
/// in the pattern (respecting `\` escapes and `[...]` brackets). Eliminates exponential
/// backtracking on unclosed extglob sequences like `!(!(!(...`.
fn compute_valid_extglobs(
    pat: &[u8],
    last_rbracket: Option<usize>,
    b: &Budget,
) -> Result<KVec<bool>, GlobError> {
    let mut valid = KVec::with_capacity_in(pat.len(), b)?;
    for _ in 0..pat.len() {
        valid.try_push(false)?;
    }
    let mut open_stack: KVec<usize> = KVec::new_in(b);
    let mut i = 0usize;
    while i < pat.len() {
        let c = pat[i];
        if c == b'\\' {
            i += 1;
            if i < pat.len() {
                let (_, len) = decode_scalar(pat, i);
                i += len;
            }
            continue;
        }
        if c == b'[' {
            if let Some(close) = find_closing_bracket(pat, i, last_rbracket) {
                i = close + 1;
                continue;
            }
            i += 1;
            continue;
        }
        if matches!(c, b'@' | b'*' | b'+' | b'?' | b'!') && pat.get(i + 1) == Some(&b'(') {
            open_stack.try_push(i)?;
            i += 2;
            continue;
        }
        if c == b')' {
            if let Some(open_pos) = open_stack.pop() {
                valid.as_mut_slice()[open_pos] = true;
            }
            i += 1;
            continue;
        }
        let (_, len) = decode_scalar(pat, i);
        i += len;
    }
    Ok(valid)
}

impl<'a> GlobParser<'a> {
    fn push_node(&mut self, node: GlobNode) -> Result<u32, GlobError> {
        let has_not = match &node {
            GlobNode::Empty
            | GlobNode::Lit(_)
            | GlobNode::Question
            | GlobNode::Star
            | GlobNode::Class(_) => false,
            GlobNode::Seq(ids) | GlobNode::Alt(ids) => ids
                .as_slice()
                .iter()
                .any(|&id| self.node_has_not.as_slice()[id as usize]),
            GlobNode::Optional(inner) | GlobNode::StarOf(inner) | GlobNode::PlusOf(inner) => {
                self.node_has_not.as_slice()[*inner as usize]
            }
            GlobNode::Not(_) => true,
        };
        let id = self.nodes.len() as u32;
        self.nodes.try_push(node)?;
        self.node_has_not.try_push(has_not)?;
        Ok(id)
    }

    fn finish_seq(&mut self, mut items: KVec<u32>) -> Result<u32, GlobError> {
        if items.is_empty() {
            self.push_node(GlobNode::Empty)
        } else if items.len() == 1 {
            Ok(items.pop().unwrap())
        } else {
            self.push_node(GlobNode::Seq(items))
        }
    }

    fn try_bracket(&mut self) -> Result<Option<u32>, GlobError> {
        if find_closing_bracket(self.pat, self.pos, self.last_rbracket).is_none() {
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
                return Ok(Some(id));
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

        // Unclosed bracket fallback (should not be hit due to `find_closing_bracket`).
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

fn find_closing_bracket(pat: &[u8], open: usize, last_rbracket: Option<usize>) -> Option<usize> {
    let last = last_rbracket?;
    if open + 1 >= last {
        return None;
    }
    let mut i = open + 1;
    if pat.get(i) == Some(&b'^') {
        i += 1;
    }
    let mut first = true;
    while i <= last {
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
            return Some(i);
        }
        first = false;
        i += 1;
    }
    None
}
