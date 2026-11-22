//! Execution engines for `kernel::regex`:
//! - Ordered Thompson NFA / PikeVM (`O(|haystack| * |insts|)`, zero backtracking).
//! - On-demand byte-capped lazy DFA (`<= max_dfa_cache_bytes`, flushing cleanly on cap)
//!   for capture-free ASCII scans.
//! - Fuel-charged bounded backtracker (`KVec<BtFrame>`, zero Rust recursion) for `\1`..`\9`.
//! - Match iteration (`EmptyAdvance`) and replacement formatting (`ReplaceStyle`).

use super::compiler::Inst;
use super::parser::{AssertKind, CharClass};
use super::{
    EmptyAdvance, MatchError, Prefilter, ReplaceStyle, Span, MAX_BACKREF_VISITED_STATES,
};
use crate::mem::{Budget, KBytes, KMap, KVec, MemError, MemResult, TryClone};

/// Sentinel base for raw invalid UTF-8 bytes so they never collide with valid Unicode scalars.
pub(crate) const RAW_BYTE_BASE: u32 = 0x11_0000;

/// Hard per-match step ceiling for the bounded backtracker (EVOL D12 / S9).
const MAX_BACKREF_STEPS: u64 = 4_000_000;

/// Decode one Unicode scalar at `pos`, or a single raw byte `(RAW_BYTE_BASE | b, 1)` on invalid UTF-8.
pub(crate) fn decode_scalar(input: &[u8], pos: usize) -> (u32, usize) {
    let b0 = input[pos];
    if b0 < 0x80 {
        return (b0 as u32, 1);
    }
    let rest = &input[pos..];
    let len = if b0 & 0xE0 == 0xC0 {
        2
    } else if b0 & 0xF0 == 0xE0 {
        3
    } else if b0 & 0xF8 == 0xF0 {
        4
    } else {
        return (RAW_BYTE_BASE | (b0 as u32), 1);
    };
    if rest.len() >= len {
        if let Ok(s) = core::str::from_utf8(&rest[..len]) {
            if let Some(c) = s.chars().next() {
                return (c as u32, len);
            }
        }
    }
    (RAW_BYTE_BASE | (b0 as u32), 1)
}

pub(crate) fn is_word_scalar(ch: u32) -> bool {
    matches!(ch, 0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A)
}

#[inline]
fn fold_char_lower(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

#[inline]
fn fold_char_upper(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_uppercase()
    } else {
        c.to_uppercase().next().unwrap_or(c)
    }
}

pub(crate) fn char_matches_icase(pat_ch: u32, hay_ch: u32) -> bool {
    if pat_ch == hay_ch {
        return true;
    }
    if pat_ch < 0x80 && hay_ch < 0x80 {
        return (pat_ch as u8).to_ascii_lowercase() == (hay_ch as u8).to_ascii_lowercase();
    }
    let (Some(p), Some(h)) = (char::from_u32(pat_ch), char::from_u32(hay_ch)) else {
        return false;
    };
    fold_char_lower(p) == fold_char_lower(h) || fold_char_upper(p) == fold_char_upper(h)
}

fn class_contains_scalar(class: &CharClass, ch: u32) -> bool {
    let mut hit = ranges_contain(class.ranges.as_slice(), ch);
    if !hit && class.icase {
        if ch < 0x80 {
            let b = ch as u8;
            let alt = if b.is_ascii_lowercase() {
                b.to_ascii_uppercase() as u32
            } else if b.is_ascii_uppercase() {
                b.to_ascii_lowercase() as u32
            } else {
                ch
            };
            if alt != ch {
                hit = ranges_contain(class.ranges.as_slice(), alt);
            }
        } else if let Some(c) = char::from_u32(ch) {
            let lo = fold_char_lower(c) as u32;
            let hi = fold_char_upper(c) as u32;
            hit = (lo != ch && ranges_contain(class.ranges.as_slice(), lo))
                || (hi != ch && ranges_contain(class.ranges.as_slice(), hi));
        }
    }
    if class.neg { !hit } else { hit }
}

fn ranges_contain(ranges: &[(u32, u32)], ch: u32) -> bool {
    for &(lo, hi) in ranges {
        if ch >= lo && ch <= hi {
            return true;
        }
    }
    false
}

/// 2-bit boundary class for assertions.
const CLS_EDGE: u8 = 0; // StartOfText (for prev) or EndOfText (for next)
const CLS_NL: u8 = 1; // '\n'
const CLS_WORD: u8 = 2; // [A-Za-z0-9_]
const CLS_OTHER: u8 = 3; // any other scalar/byte

#[inline]
fn byte_class(b: u8) -> u8 {
    if b == b'\n' {
        CLS_NL
    } else if b.is_ascii_alphanumeric() || b == b'_' {
        CLS_WORD
    } else {
        CLS_OTHER
    }
}

#[inline]
fn scalar_class(ch: u32) -> u8 {
    if ch == b'\n' as u32 {
        CLS_NL
    } else if is_word_scalar(ch) {
        CLS_WORD
    } else {
        CLS_OTHER
    }
}

#[inline]
fn check_assert(kind: AssertKind, prev_cls: u8, next_cls: u8) -> bool {
    match kind {
        AssertKind::StartText => prev_cls == CLS_EDGE,
        AssertKind::EndText => next_cls == CLS_EDGE,
        AssertKind::StartLine => prev_cls == CLS_EDGE || prev_cls == CLS_NL,
        AssertKind::EndLine => next_cls == CLS_EDGE || next_cls == CLS_NL,
        AssertKind::WordBound => (prev_cls == CLS_WORD) != (next_cls == CLS_WORD),
        AssertKind::NotWordBound => (prev_cls == CLS_WORD) == (next_cls == CLS_WORD),
        AssertKind::WordStart => prev_cls != CLS_WORD && next_cls == CLS_WORD,
        AssertKind::WordEnd => prev_cls == CLS_WORD && next_cls != CLS_WORD,
        AssertKind::PrevNotWord => prev_cls != CLS_WORD,
        AssertKind::NextNotWord => next_cls != CLS_WORD,
    }
}

fn prev_class_at(haystack: &[u8], pos: usize) -> u8 {
    if pos == 0 {
        CLS_EDGE
    } else {
        // UTF-8 continuation bytes are >= 0x80 so checking the previous byte is exact
        // for `\n` (0x0A) and ASCII word bytes (`[A-Za-z0-9_]`).
        byte_class(haystack[pos - 1])
    }
}

fn next_class_at(haystack: &[u8], pos: usize) -> u8 {
    if pos >= haystack.len() {
        CLS_EDGE
    } else {
        byte_class(haystack[pos])
    }
}

fn find_bytes(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(start);
    }
    if start + needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    let last = haystack.len() - needle.len();
    let mut i = start;
    while i <= last {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Byte-capped Lazy DFA (ASCII fast path for capture-free `is_match`)

const DFA_UNCOMPUTED: u32 = u32::MAX;
const DFA_DEAD: u32 = u32::MAX - 1;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct DfaKey {
    prev_cls: u8,
    pcs: KVec<u32>,
}

struct DfaState {
    prev_cls: u8,
    pcs: KVec<u32>,
    /// Bit `nc` (0..4) is set iff epsilon-closure with `(prev_cls, nc)` reaches `Inst::Match`.
    match_mask: u8,
    trans: [u32; 128],
}

pub(crate) struct DfaCache {
    states: KVec<DfaState>,
    index: KMap<DfaKey, u32>,
    cached_bytes: usize,
    max_bytes: usize,
    generation: u64,
    budget: Budget,
}

impl DfaCache {
    pub(crate) fn new_in(max_bytes: usize, b: &Budget) -> Self {
        Self {
            states: KVec::new_in(b),
            index: KMap::new_in(b),
            cached_bytes: 0,
            max_bytes,
            generation: 0,
            budget: b.clone(),
        }
    }

    pub(crate) fn cached_bytes(&self) -> usize {
        self.cached_bytes
    }

    fn clear(&mut self) {
        self.states.clear();
        self.index.clear();
        self.cached_bytes = 0;
        self.generation = self.generation.wrapping_add(1);
    }

    fn state_footprint(pcs_len: usize) -> usize {
        core::mem::size_of::<DfaState>() + pcs_len * 2 * core::mem::size_of::<u32>() + 64
    }

    /// Inserts or looks up a DFA state. Returns `Ok(None)` if a single state cannot fit in `max_bytes`.
    fn intern(
        &mut self,
        prev_cls: u8,
        pcs: KVec<u32>,
        insts: &[Inst],
        scratch_b: &Budget,
    ) -> MemResult<Option<u32>> {
        let key = DfaKey { prev_cls, pcs };
        if let Some(&id) = self.index.get(&key) {
            return Ok(Some(id));
        }
        let cost = Self::state_footprint(key.pcs.len());
        if cost > self.max_bytes {
            return Ok(None);
        }
        if self.cached_bytes.saturating_add(cost) > self.max_bytes {
            self.clear();
        }
        // Compute match_mask across all 4 possible next_cls values.
        let mut match_mask = 0u8;
        let mut seen = KVec::with_capacity_in(insts.len(), scratch_b)?;
        for _ in 0..insts.len() {
            seen.try_push(0u8)?;
        }
        let mut stack = KVec::new_in(scratch_b);
        for nc in 0..4u8 {
            let tag = nc + 1;
            stack.clear();
            for &pc in key.pcs.as_slice() {
                stack.try_push(pc)?;
            }
            while let Some(pc) = stack.pop() {
                let idx = pc as usize;
                if seen.as_slice()[idx] == tag {
                    continue;
                }
                seen.as_mut_slice()[idx] = tag;
                match insts[idx] {
                    Inst::Match => {
                        match_mask |= 1 << nc;
                        break;
                    }
                    Inst::Jump(t) => stack.try_push(t)?,
                    Inst::Save(_) | Inst::Keep => stack.try_push(pc + 1)?,
                    Inst::Split(a, b_pc) => {
                        stack.try_push(b_pc)?;
                        stack.try_push(a)?;
                    }
                    Inst::Assert(kind) => {
                        if check_assert(kind, prev_cls, nc) {
                            stack.try_push(pc + 1)?;
                        }
                    }
                    _ => {}
                }
            }
        }

        let id = self.states.len() as u32;
        let mut pcs_copy = KVec::with_capacity_in(key.pcs.len(), &self.budget)?;
        pcs_copy.try_extend_from_slice(key.pcs.as_slice())?;
        let state = DfaState {
            prev_cls,
            pcs: pcs_copy,
            match_mask,
            trans: [DFA_UNCOMPUTED; 128],
        };
        let mut key_copy_pcs = KVec::with_capacity_in(key.pcs.len(), &self.budget)?;
        key_copy_pcs.try_extend_from_slice(key.pcs.as_slice())?;
        let key_copy = DfaKey {
            prev_cls,
            pcs: key_copy_pcs,
        };
        self.states.try_push(state)?;
        self.index.try_insert(key_copy, id)?;
        self.cached_bytes = self.cached_bytes.saturating_add(cost);
        Ok(Some(id))
    }
}

// ---------------------------------------------------------------------------
// Execution context

pub(crate) struct ExecCtx<'a> {
    pub insts: &'a [Inst],
    pub classes: &'a [CharClass],
    pub names: &'a [(KBytes, u32)],
    pub prefilter: Prefilter<'a>,
    pub groups: u32,
    pub has_backrefs: bool,
    pub haystack: &'a [u8],
    pub dfa: &'a mut DfaCache,
    pub b: &'a Budget,
    pub work: &'a mut dyn FnMut(u64) -> Result<(), ()>,
}

struct PikeThread {
    pc: u32,
    start: usize,
    keep: Option<usize>,
    slots: KVec<Option<usize>>,
}

struct MatchSlots {
    raw_start: usize,
    reported_start: usize,
    end: usize,
    slots: KVec<Option<usize>>,
}

impl<'a> ExecCtx<'a> {
    fn charge(&mut self, units: u64) -> Result<(), MatchError> {
        if units == 0 {
            return Ok(());
        }
        (self.work)(units).map_err(|()| MatchError::FuelExhausted)
    }

    pub(crate) fn is_match(&mut self) -> Result<bool, MatchError> {
        self.charge(1)?;
        if self.has_backrefs {
            return Ok(self.backref_search(0)?.is_some());
        }
        // Prefilter quick rejection.
        match self.prefilter {
            Prefilter::Exact(lit) | Prefilter::Substring(lit) => {
                if find_bytes(self.haystack, 0, lit).is_none() {
                    return Ok(false);
                }
            }
            Prefilter::None => {}
        }
        // Try lazy DFA for ASCII haystacks.
        if let Some(res) = self.dfa_is_match(0)? {
            return Ok(res);
        }
        Ok(self.pikevm_search(0, false)?.is_some())
    }

    #[cfg(test)]
    pub(crate) fn is_match_pikevm_only(&mut self) -> Result<bool, MatchError> {
        self.charge(1)?;
        Ok(self.pikevm_search(0, false)?.is_some())
    }

    #[cfg(test)]
    pub(crate) fn is_match_dfa_only(&mut self) -> Result<Option<bool>, MatchError> {
        self.charge(1)?;
        self.dfa_is_match(0)
    }

    pub(crate) fn find_at(&mut self, at: usize) -> Result<Option<Span>, MatchError> {
        self.charge(1)?;
        if at > self.haystack.len() {
            return Ok(None);
        }
        if self.has_backrefs {
            return Ok(self.backref_search(at)?.map(|m| Span {
                start: m.reported_start,
                end: m.end,
            }));
        }
        let search_start = match self.prefilter {
            Prefilter::Exact(lit) => match find_bytes(self.haystack, at, lit) {
                Some(pos) => pos,
                None => return Ok(None),
            },
            Prefilter::Substring(lit) => {
                if find_bytes(self.haystack, at, lit).is_none() {
                    return Ok(None);
                }
                at
            }
            Prefilter::None => at,
        };
        // Fast reject via lazy DFA when haystack tail is ASCII.
        if let Some(false) = self.dfa_is_match(search_start)? {
            return Ok(None);
        }
        Ok(self.pikevm_search(search_start, false)?.map(|m| Span {
            start: m.reported_start,
            end: m.end,
        }))
    }

    pub(crate) fn captures_at(&mut self, at: usize) -> Result<Option<KVec<Option<Span>>>, MatchError> {
        self.charge(1)?;
        if at > self.haystack.len() {
            return Ok(None);
        }
        let m = if self.has_backrefs {
            self.backref_search(at)?
        } else {
            let search_start = match self.prefilter {
                Prefilter::Exact(lit) => match find_bytes(self.haystack, at, lit) {
                    Some(pos) => pos,
                    None => return Ok(None),
                },
                Prefilter::Substring(lit) => {
                    if find_bytes(self.haystack, at, lit).is_none() {
                        return Ok(None);
                    }
                    at
                }
                Prefilter::None => at,
            };
            if let Some(false) = self.dfa_is_match(search_start)? {
                return Ok(None);
            }
            self.pikevm_search(search_start, true)?
        };
        let Some(m) = m else { return Ok(None) };
        let count = (self.groups as usize) + 1;
        let mut out = KVec::with_capacity_in(count, self.b)?;
        out.try_push(Some(Span {
            start: m.reported_start,
            end: m.end,
        }))?;
        for g in 1..count {
            let s = m.slots.as_slice().get(g * 2).copied().flatten();
            let e = m.slots.as_slice().get(g * 2 + 1).copied().flatten();
            out.try_push(match (s, e) {
                (Some(a), Some(b_end)) if a <= b_end && b_end <= self.haystack.len() => {
                    Some(Span {
                        start: a,
                        end: b_end,
                    })
                }
                _ => None,
            })?;
        }
        Ok(Some(out))
    }

    pub(crate) fn find_all(&mut self, policy: EmptyAdvance) -> Result<KVec<Span>, MatchError> {
        let mut out = KVec::new_in(self.b);
        let mut pos = 0usize;
        let mut last_nonempty = false;
        while pos <= self.haystack.len() {
            let m = self.match_full_at(pos, false)?;
            let Some(m) = m else { break };

            if last_nonempty && m.raw_start == pos && m.end == pos {
                match policy {
                    EmptyAdvance::PosixSed => {
                        if pos >= self.haystack.len() {
                            break;
                        }
                        let (_, step) = decode_scalar(self.haystack, pos);
                        pos += step;
                        last_nonempty = false;
                        continue;
                    }
                    EmptyAdvance::ShellExpansion if pos == self.haystack.len() => break,
                    _ => {}
                }
            }

            out.try_push(Span {
                start: m.reported_start,
                end: m.end,
            })?;

            if m.end > m.raw_start {
                last_nonempty = true;
                pos = m.end;
            } else {
                last_nonempty = false;
                if m.end >= self.haystack.len() {
                    break;
                }
                let (_, step) = decode_scalar(self.haystack, m.end);
                pos = m.end + step;
            }
        }
        Ok(out)
    }

    pub(crate) fn replace_all(
        &mut self,
        replacement: &[u8],
        style: ReplaceStyle,
        nth: Option<usize>,
        policy: EmptyAdvance,
    ) -> Result<(KVec<u8>, bool), MatchError> {
        let need_caps = !matches!(style, ReplaceStyle::Literal);
        let mut out = KVec::new_in(self.b);
        let mut copy_from = 0usize;
        let mut pos = 0usize;
        let mut match_num = 0usize;
        let mut changed = false;
        let mut last_nonempty = false;

        if nth == Some(0) {
            out.try_extend_from_slice(self.haystack)?;
            return Ok((out, false));
        }

        while pos <= self.haystack.len() {
            let m = self.match_full_at(pos, need_caps)?;
            let Some(m) = m else { break };

            if last_nonempty && m.raw_start == pos && m.end == pos {
                match policy {
                    EmptyAdvance::PosixSed => {
                        if pos >= self.haystack.len() {
                            break;
                        }
                        let (_, step) = decode_scalar(self.haystack, pos);
                        pos += step;
                        last_nonempty = false;
                        continue;
                    }
                    EmptyAdvance::ShellExpansion if pos == self.haystack.len() => break,
                    _ => {}
                }
            }

            match_num += 1;
            let do_replace = match nth {
                None => true,
                Some(k) => match_num == k,
            };

            if do_replace {
                out.try_extend_from_slice(&self.haystack[copy_from..m.reported_start])?;
                self.expand_replacement(&mut out, replacement, style, &m)?;
                copy_from = m.end;
                changed = true;
                if nth.is_some() {
                    break;
                }
            }

            if m.end > m.raw_start {
                last_nonempty = true;
                pos = m.end;
            } else {
                last_nonempty = false;
                if m.end >= self.haystack.len() {
                    break;
                }
                let (_, step) = decode_scalar(self.haystack, m.end);
                pos = m.end + step;
            }
        }

        out.try_extend_from_slice(&self.haystack[copy_from..])?;
        Ok((out, changed))
    }

    fn match_full_at(&mut self, at: usize, need_caps: bool) -> Result<Option<MatchSlots>, MatchError> {
        self.charge(1)?;
        if at > self.haystack.len() {
            return Ok(None);
        }
        if self.has_backrefs {
            return self.backref_search(at);
        }
        let search_start = match self.prefilter {
            Prefilter::Exact(lit) => match find_bytes(self.haystack, at, lit) {
                Some(p) => p,
                None => return Ok(None),
            },
            Prefilter::Substring(lit) => {
                if find_bytes(self.haystack, at, lit).is_none() {
                    return Ok(None);
                }
                at
            }
            Prefilter::None => at,
        };
        if let Some(false) = self.dfa_is_match(search_start)? {
            return Ok(None);
        }
        self.pikevm_search(search_start, need_caps)
    }

    fn expand_replacement(
        &self,
        out: &mut KVec<u8>,
        replacement: &[u8],
        style: ReplaceStyle,
        m: &MatchSlots,
    ) -> MemResult<()> {
        match style {
            ReplaceStyle::Literal => out.try_extend_from_slice(replacement),
            ReplaceStyle::Sed => {
                let mut i = 0;
                while i < replacement.len() {
                    let c = replacement[i];
                    if c == b'&' {
                        out.try_extend_from_slice(&self.haystack[m.reported_start..m.end])?;
                        i += 1;
                    } else if c == b'\\' && i + 1 < replacement.len() {
                        let nc = replacement[i + 1];
                        i += 2;
                        match nc {
                            b'n' => out.try_push(b'\n')?,
                            b't' => out.try_push(b'\t')?,
                            b'r' => out.try_push(b'\r')?,
                            b'\\' => out.try_push(b'\\')?,
                            b'&' => out.try_push(b'&')?,
                            b'1'..=b'9' => {
                                let g = (nc - b'0') as usize;
                                self.push_group_slice(out, m, g)?;
                            }
                            other => out.try_push(other)?,
                        }
                    } else {
                        out.try_push(c)?;
                        i += 1;
                    }
                }
                Ok(())
            }
            ReplaceStyle::Dollar => {
                let mut i = 0;
                while i < replacement.len() {
                    let c = replacement[i];
                    if c == b'$' && i + 1 < replacement.len() {
                        let nc = replacement[i + 1];
                        match nc {
                            b'$' => {
                                out.try_push(b'$')?;
                                i += 2;
                            }
                            b'&' => {
                                out.try_extend_from_slice(&self.haystack[m.reported_start..m.end])?;
                                i += 2;
                            }
                            b'1'..=b'9' => {
                                let g = (nc - b'0') as usize;
                                self.push_group_slice(out, m, g)?;
                                i += 2;
                            }
                            b'{' => {
                                if let Some(rel_end) = replacement[i + 2..].iter().position(|&x| x == b'}') {
                                    let name = &replacement[i + 2..i + 2 + rel_end];
                                    if let Some((_, idx)) = self.names.iter().find(|(n, _)| n.as_slice() == name) {
                                        self.push_group_slice(out, m, *idx as usize)?;
                                    }
                                    i += 2 + rel_end + 1;
                                } else {
                                    out.try_push(b'$')?;
                                    i += 1;
                                }
                            }
                            _ => {
                                out.try_push(b'$')?;
                                i += 1;
                            }
                        }
                    } else {
                        out.try_push(c)?;
                        i += 1;
                    }
                }
                Ok(())
            }
        }
    }

    fn push_group_slice(&self, out: &mut KVec<u8>, m: &MatchSlots, g: usize) -> MemResult<()> {
        let s = m.slots.as_slice().get(g * 2).copied().flatten();
        let e = m.slots.as_slice().get(g * 2 + 1).copied().flatten();
        if let (Some(a), Some(b_end)) = (s, e) {
            if a <= b_end {
                if let Some(slice) = self.haystack.get(a..b_end) {
                    out.try_extend_from_slice(slice)?;
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Byte-capped Lazy DFA (`is_match` on ASCII slices)

    fn dfa_is_match(&mut self, start_at: usize) -> Result<Option<bool>, MatchError> {
        if self.has_backrefs || !self.haystack[start_at..].is_ascii() {
            return Ok(None);
        }
        let init_prev = prev_class_at(self.haystack, start_at);
        let mut init_pcs = KVec::with_capacity_in(1, self.b)?;
        init_pcs.try_push(0u32)?;
        let Some(mut state_id) = self.dfa.intern(init_prev, init_pcs, self.insts, self.b)? else {
            return Ok(None);
        };

        let mut pos = start_at;
        let mut byte_steps = 0u64;
        let mut seen = KVec::with_capacity_in(self.insts.len(), self.b)?;
        for _ in 0..self.insts.len() {
            seen.try_push(0u32)?;
        }
        let mut gen_counter = 0u32;
        let mut eps_stack = KVec::new_in(self.b);

        loop {
            let next_cls = next_class_at(self.haystack, pos);
            if (self.dfa.states.as_slice()[state_id as usize].match_mask & (1 << next_cls)) != 0 {
                return Ok(Some(true));
            }
            if pos >= self.haystack.len() {
                return Ok(Some(false));
            }
            let b = self.haystack[pos];
            byte_steps += 1;
            if byte_steps & 255 == 0 {
                self.charge(1)?;
            }

            let cached = self.dfa.states.as_slice()[state_id as usize].trans[b as usize];
            if cached != DFA_UNCOMPUTED {
                if cached == DFA_DEAD {
                    return Ok(Some(false));
                }
                state_id = cached;
                pos += 1;
                continue;
            }

            // Compute transition for `(state_id, b)`.
            let cur_prev = self.dfa.states.as_slice()[state_id as usize].prev_cls;
            let nc = byte_class(b);
            gen_counter = gen_counter.wrapping_add(1);
            if gen_counter == 0 {
                for x in seen.as_mut_slice() {
                    *x = 0;
                }
                gen_counter = 1;
            }
            eps_stack.clear();
            for &pc in self.dfa.states.as_slice()[state_id as usize].pcs.as_slice() {
                eps_stack.try_push(pc)?;
            }
            let mut next_pcs = KVec::new_in(self.b);
            // Always keep unanchored start PC 0 in the next state so matches can begin at `pos + 1`.
            next_pcs.try_push(0u32)?;

            let ch = b as u32;
            while let Some(pc) = eps_stack.pop() {
                let idx = pc as usize;
                if seen.as_slice()[idx] == gen_counter {
                    continue;
                }
                seen.as_mut_slice()[idx] = gen_counter;
                match self.insts[idx] {
                    Inst::Char { ch: pat_ch, icase } => {
                        let matched = if icase {
                            char_matches_icase(pat_ch, ch)
                        } else {
                            pat_ch == ch
                        };
                        if matched {
                            next_pcs.try_push(pc + 1)?;
                        }
                    }
                    Inst::Dot { dot_all } => {
                        if dot_all || b != b'\n' {
                            next_pcs.try_push(pc + 1)?;
                        }
                    }
                    Inst::Class(cid) => {
                        if class_contains_scalar(&self.classes[cid as usize], ch) {
                            next_pcs.try_push(pc + 1)?;
                        }
                    }
                    Inst::Assert(kind) => {
                        if check_assert(kind, cur_prev, nc) {
                            eps_stack.try_push(pc + 1)?;
                        }
                    }
                    Inst::Jump(t) => eps_stack.try_push(t)?,
                    Inst::Split(a, b_pc) => {
                        eps_stack.try_push(b_pc)?;
                        eps_stack.try_push(a)?;
                    }
                    Inst::Save(_) | Inst::Keep => eps_stack.try_push(pc + 1)?,
                    Inst::Match | Inst::Backref { .. } => {}
                }
            }

            // Sort and deduplicate `next_pcs`.
            next_pcs.as_mut_slice().sort_unstable();
            dedup_u32(&mut next_pcs);

            let gen_before = self.dfa.generation;
            let Some(target_id) = self.dfa.intern(nc, next_pcs, self.insts, self.b)? else {
                return Ok(None);
            };
            // Only write back the transition if `intern` did not flush the cache.
            if self.dfa.generation == gen_before && (state_id as usize) < self.dfa.states.len() {
                self.dfa.states.as_mut_slice()[state_id as usize].trans[b as usize] = target_id;
            }
            state_id = target_id;
            pos += 1;
        }
    }

    // -----------------------------------------------------------------------
    // Ordered Thompson NFA / PikeVM (`O(|haystack| * |insts|)`)

    fn pikevm_search(
        &mut self,
        start_at: usize,
        track_caps: bool,
    ) -> Result<Option<MatchSlots>, MatchError> {
        let num_slots = if track_caps {
            ((self.groups as usize) + 1) * 2
        } else {
            0
        };
        let n_insts = self.insts.len();
        let mut seen = KVec::with_capacity_in(n_insts, self.b)?;
        for _ in 0..n_insts {
            seen.try_push(0u32)?;
        }
        let mut gen_counter = 0u32;

        let mut curr: KVec<PikeThread> = KVec::new_in(self.b);
        let mut next: KVec<PikeThread> = KVec::new_in(self.b);
        let mut best: Option<MatchSlots> = None;
        let mut work_steps = 0u64;

        let mut pos = start_at;
        loop {
            let prev_cls = prev_class_at(self.haystack, pos);
            let next_cls = next_class_at(self.haystack, pos);

            // Spawn unanchored start thread at lowest priority if no match has been found yet.
            if best.is_none() {
                let mut slots = KVec::with_capacity_in(num_slots, self.b)?;
                for _ in 0..num_slots {
                    slots.try_push(None)?;
                }
                curr.try_push(PikeThread {
                    pc: 0,
                    start: pos,
                    keep: None,
                    slots,
                })?;
            }

            if curr.is_empty() {
                break;
            }

            gen_counter = gen_counter.wrapping_add(1);
            if gen_counter == 0 {
                for x in seen.as_mut_slice() {
                    *x = 0;
                }
                gen_counter = 1;
            }

            // Expand epsilon closures in strict priority order.
            let mut active: KVec<PikeThread> = KVec::new_in(self.b);
            let mut matched_here = false;
            let mut i = 0;
            while i < curr.len() {
                let th = core::mem::replace(
                    &mut curr.as_mut_slice()[i],
                    PikeThread {
                        pc: 0,
                        start: 0,
                        keep: None,
                        slots: KVec::new_in(self.b),
                    },
                );
                i += 1;
                if self.pike_add(
                    th,
                    pos,
                    prev_cls,
                    next_cls,
                    track_caps,
                    &mut seen,
                    gen_counter,
                    &mut active,
                    &mut best,
                    &mut work_steps,
                )? {
                    matched_here = true;
                    break;
                }
            }
            curr.clear();

            if pos >= self.haystack.len() || (matched_here && active.is_empty()) {
                break;
            }

            let (ch, step_len) = decode_scalar(self.haystack, pos);
            next.clear();
            for j in 0..active.len() {
                work_steps += 1;
                if work_steps & 63 == 0 {
                    self.charge(1)?;
                }
                let th = core::mem::replace(
                    &mut active.as_mut_slice()[j],
                    PikeThread {
                        pc: 0,
                        start: 0,
                        keep: None,
                        slots: KVec::new_in(self.b),
                    },
                );
                let matched = match self.insts[th.pc as usize] {
                    Inst::Char { ch: pat_ch, icase } => {
                        if icase {
                            char_matches_icase(pat_ch, ch)
                        } else {
                            pat_ch == ch
                        }
                    }
                    Inst::Dot { dot_all } => dot_all || ch != (b'\n' as u32),
                    Inst::Class(cid) => class_contains_scalar(&self.classes[cid as usize], ch),
                    _ => false,
                };
                if matched {
                    next.try_push(PikeThread {
                        pc: th.pc + 1,
                        start: th.start,
                        keep: th.keep,
                        slots: th.slots,
                    })?;
                }
            }

            core::mem::swap(&mut curr, &mut next);
            pos += step_len;
        }

        Ok(best)
    }

    #[allow(clippy::too_many_arguments)]
    fn pike_add(
        &mut self,
        initial: PikeThread,
        pos: usize,
        prev_cls: u8,
        next_cls: u8,
        track_caps: bool,
        seen: &mut KVec<u32>,
        gen_counter: u32,
        active: &mut KVec<PikeThread>,
        best: &mut Option<MatchSlots>,
        work_steps: &mut u64,
    ) -> Result<bool, MatchError> {
        // Explicit stack to avoid Rust call recursion on deeply nested groups/alternations.
        let mut stack: KVec<PikeThread> = KVec::new_in(self.b);
        stack.try_push(initial)?;

        while let Some(mut th) = stack.pop() {
            *work_steps += 1;
            if (*work_steps & 63) == 0 {
                self.charge(1)?;
            }
            let idx = th.pc as usize;
            if seen.as_slice()[idx] == gen_counter {
                continue;
            }
            seen.as_mut_slice()[idx] = gen_counter;

            match self.insts[idx] {
                Inst::Match => {
                    let reported_start = th.keep.unwrap_or(th.start);
                    *best = Some(MatchSlots {
                        raw_start: th.start,
                        reported_start,
                        end: pos,
                        slots: th.slots,
                    });
                    return Ok(true);
                }
                Inst::Jump(target) => {
                    th.pc = target;
                    stack.try_push(th)?;
                }
                Inst::Split(a, b_pc) => {
                    // Push lower-priority branch `b_pc` first so `a` pops first.
                    let slots_b = th.slots.try_clone()?;
                    stack.try_push(PikeThread {
                        pc: b_pc,
                        start: th.start,
                        keep: th.keep,
                        slots: slots_b,
                    })?;
                    th.pc = a;
                    stack.try_push(th)?;
                }
                Inst::Save(slot) => {
                    if track_caps && (slot as usize) < th.slots.len() {
                        th.slots.as_mut_slice()[slot as usize] = Some(pos);
                    }
                    th.pc += 1;
                    stack.try_push(th)?;
                }
                Inst::Keep => {
                    th.keep = Some(pos);
                    th.pc += 1;
                    stack.try_push(th)?;
                }
                Inst::Assert(kind) => {
                    if check_assert(kind, prev_cls, next_cls) {
                        th.pc += 1;
                        stack.try_push(th)?;
                    }
                }
                Inst::Char { .. } | Inst::Dot { .. } | Inst::Class(_) => {
                    active.try_push(th)?;
                }
                Inst::Backref { .. } => {}
            }
        }
        Ok(false)
    }

    // -----------------------------------------------------------------------
    // Fuel-charged bounded backtracker (only for `has_backrefs == true`)

    fn backref_search(&mut self, start_at: usize) -> Result<Option<MatchSlots>, MatchError> {
        let mut steps = 0u64;
        let mut pos = start_at;
        while pos <= self.haystack.len() {
            if let Some(m) = self.backref_at(pos, &mut steps)? {
                return Ok(Some(m));
            }
            if pos >= self.haystack.len() {
                break;
            }
            let (_, step) = decode_scalar(self.haystack, pos);
            pos += step;
        }
        Ok(None)
    }

    fn backref_at(
        &mut self,
        start_pos: usize,
        steps: &mut u64,
    ) -> Result<Option<MatchSlots>, MatchError> {
        enum BtOp {
            Eval {
                pc: u32,
                pos: usize,
            },
            RestoreSlot {
                slot: usize,
                prev: Option<usize>,
                prev_cap_id: u32,
            },
            RestoreKeep {
                prev: Option<usize>,
            },
        }

        let num_slots = ((self.groups as usize) + 1) * 2;
        let mut slots: KVec<Option<usize>> = KVec::with_capacity_in(num_slots, self.b)?;
        for _ in 0..num_slots {
            slots.try_push(None)?;
        }
        let mut keep: Option<usize> = None;

        // Intern active capture slot slices (`slots[2..]`) so `visited` keys on `(cap_id, pc, pos)`
        // and stays invariant when `Save` writes an unchanged value or `RestoreSlot` restores a prior state.
        let mut cap_intern: KMap<KVec<Option<usize>>, u32> = KMap::new_in(self.b);
        let mut init_key: KVec<Option<usize>> =
            KVec::with_capacity_in(num_slots.saturating_sub(2), self.b)?;
        init_key.try_extend_from_slice(&slots.as_slice()[2..])?;
        cap_intern.try_insert(init_key, 0u32)?;
        let mut next_cap_id = 1u32;
        let mut cap_id = 0u32;

        // Bounded visited table keyed by `(cap_id, pc, pos)`, capped at MAX_BACKREF_VISITED_STATES.
        let mut visited: KMap<(u32, u32, usize), ()> = KMap::new_in(self.b);
        let mut visited_count = 0usize;

        let mut stack: KVec<BtOp> = KVec::new_in(self.b);
        stack.try_push(BtOp::Eval {
            pc: 0,
            pos: start_pos,
        })?;

        while let Some(op) = stack.pop() {
            match op {
                BtOp::RestoreSlot {
                    slot,
                    prev,
                    prev_cap_id,
                } => {
                    slots.as_mut_slice()[slot] = prev;
                    cap_id = prev_cap_id;
                    continue;
                }
                BtOp::RestoreKeep { prev } => {
                    keep = prev;
                    continue;
                }
                BtOp::Eval { pc, pos } => {
                    *steps += 1;
                    if *steps > MAX_BACKREF_STEPS {
                        return Err(MatchError::BacktrackLimitExceeded);
                    }
                    if (*steps & 15) == 0 {
                        self.charge(1)?;
                    }
                    let key = (cap_id, pc, pos);
                    if visited.get(&key).is_some() {
                        continue;
                    }
                    if visited_count < MAX_BACKREF_VISITED_STATES {
                        visited.try_insert(key, ())?;
                        visited_count += 1;
                    }

                    match self.insts[pc as usize] {
                        Inst::Match => {
                            return Ok(Some(MatchSlots {
                                raw_start: start_pos,
                                reported_start: keep.unwrap_or(start_pos),
                                end: pos,
                                slots,
                            }));
                        }
                        Inst::Jump(t) => {
                            stack.try_push(BtOp::Eval { pc: t, pos })?;
                        }
                        Inst::Split(a, b_pc) => {
                            stack.try_push(BtOp::Eval { pc: b_pc, pos })?;
                            stack.try_push(BtOp::Eval { pc: a, pos })?;
                        }
                        Inst::Save(slot_u32) => {
                            let slot = slot_u32 as usize;
                            if slot >= 2 && slot < slots.len() {
                                let prev = slots.as_slice()[slot];
                                let next_val = Some(pos);
                                if prev != next_val {
                                    let prev_cap_id = cap_id;
                                    slots.as_mut_slice()[slot] = next_val;
                                    let active_slice = &slots.as_slice()[2..];
                                    cap_id = if let Some(&existing) = cap_intern.get(active_slice) {
                                        existing
                                    } else {
                                        let id = next_cap_id;
                                        next_cap_id = next_cap_id.wrapping_add(1);
                                        if cap_intern.len() < MAX_BACKREF_VISITED_STATES {
                                            let mut k =
                                                KVec::with_capacity_in(active_slice.len(), self.b)?;
                                            k.try_extend_from_slice(active_slice)?;
                                            cap_intern.try_insert(k, id)?;
                                        }
                                        id
                                    };
                                    stack.try_push(BtOp::RestoreSlot {
                                        slot,
                                        prev,
                                        prev_cap_id,
                                    })?;
                                }
                            }
                            stack.try_push(BtOp::Eval { pc: pc + 1, pos })?;
                        }
                        Inst::Keep => {
                            let prev = keep;
                            keep = Some(pos);
                            stack.try_push(BtOp::RestoreKeep { prev })?;
                            stack.try_push(BtOp::Eval { pc: pc + 1, pos })?;
                        }
                        Inst::Assert(kind) => {
                            let prev_cls = prev_class_at(self.haystack, pos);
                            let next_cls = next_class_at(self.haystack, pos);
                            if check_assert(kind, prev_cls, next_cls) {
                                stack.try_push(BtOp::Eval { pc: pc + 1, pos })?;
                            }
                        }
                        Inst::Char { ch: pat_ch, icase } => {
                            if pos < self.haystack.len() {
                                let (ch, step_len) = decode_scalar(self.haystack, pos);
                                let ok = if icase {
                                    char_matches_icase(pat_ch, ch)
                                } else {
                                    pat_ch == ch
                                };
                                if ok {
                                    stack.try_push(BtOp::Eval {
                                        pc: pc + 1,
                                        pos: pos + step_len,
                                    })?;
                                }
                            }
                        }
                        Inst::Dot { dot_all } => {
                            if pos < self.haystack.len() {
                                let (ch, step_len) = decode_scalar(self.haystack, pos);
                                if dot_all || ch != (b'\n' as u32) {
                                    stack.try_push(BtOp::Eval {
                                        pc: pc + 1,
                                        pos: pos + step_len,
                                    })?;
                                }
                            }
                        }
                        Inst::Class(cid) => {
                            if pos < self.haystack.len() {
                                let (ch, step_len) = decode_scalar(self.haystack, pos);
                                if class_contains_scalar(&self.classes[cid as usize], ch) {
                                    stack.try_push(BtOp::Eval {
                                        pc: pc + 1,
                                        pos: pos + step_len,
                                    })?;
                                }
                            }
                        }
                        Inst::Backref { group, icase } => {
                            let g = group as usize;
                            let s = slots.as_slice().get(g * 2).copied().flatten();
                            let e = slots.as_slice().get(g * 2 + 1).copied().flatten();
                            // Skip unset or in-progress captures (`a > b_end` when an outer loop
                            // re-enters group `g` and overwrites `slots[2*g]` before `Save(2*g+1)`).
                            if let (Some(a), Some(b_end)) = (s, e) {
                                if a <= b_end {
                                    if let Some(cap_slice) = self.haystack.get(a..b_end) {
                                        if let Some(new_pos) =
                                            match_slice_at(self.haystack, pos, cap_slice, icase)
                                        {
                                            stack.try_push(BtOp::Eval {
                                                pc: pc + 1,
                                                pos: new_pos,
                                            })?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(None)
    }
}

fn match_slice_at(haystack: &[u8], pos: usize, needle: &[u8], icase: bool) -> Option<usize> {
    if !icase {
        if pos + needle.len() <= haystack.len() && &haystack[pos..pos + needle.len()] == needle {
            Some(pos + needle.len())
        } else {
            None
        }
    } else {
        let mut ni = 0;
        let mut hi = pos;
        while ni < needle.len() {
            if hi >= haystack.len() {
                return None;
            }
            let (nc, nl) = decode_scalar(needle, ni);
            let (hc, hl) = decode_scalar(haystack, hi);
            if !char_matches_icase(nc, hc) {
                return None;
            }
            ni += nl;
            hi += hl;
        }
        Some(hi)
    }
}

fn dedup_u32(v: &mut KVec<u32>) {
    if v.len() <= 1 {
        return;
    }
    let slice = v.as_mut_slice();
    let mut w = 1usize;
    for r in 1..slice.len() {
        if slice[r] != slice[w - 1] {
            slice[w] = slice[r];
            w += 1;
        }
    }
    v.truncate(w);
}

#[allow(dead_code)]
fn _keep_mem_err(_: MemError) {}
#[allow(dead_code)]
fn _keep_scalar_class(c: u32) -> u8 {
    scalar_class(c)
}
