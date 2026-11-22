//! Linear-time glob / extglob matcher engine:
//! - Thompson NFA (`O(|subject| * |insts|)`) for all `!(...)`-free globs (forward + reversed).
//! - Exact polynomial/linear Reachability DP over Unicode scalar boundaries for `!(...)`
//!   extglobs (including nested `!(!(a)|b)`, `!(foo)bar`, `!(a*)b`, strip, and replace),
//!   with zero regex lookahead and fuel charging on every step.

use super::parser::{GlobAst, GlobClass};
use super::{CompiledGlob, GlobError, GlobOptions, GlobReplaceMode};
use crate::mem::{Budget, KVec};
use crate::regex::{char_matches_icase, decode_scalar};

#[derive(Clone, Copy, Debug)]
pub(crate) enum GlobInst {
    Lit(u32),
    Any,
    Class(u32),
    Jump(u32),
    Split(u32, u32),
    Match,
}

pub(crate) fn compile_nfa(
    root: &GlobAst,
    reverse: bool,
    b: &Budget,
) -> Result<KVec<GlobInst>, GlobError> {
    let mut insts = KVec::new_in(b);
    emit_ast(root, reverse, &mut insts, b)?;
    insts.try_push(GlobInst::Match)?;
    Ok(insts)
}

fn emit_ast(
    ast: &GlobAst,
    reverse: bool,
    insts: &mut KVec<GlobInst>,
    b: &Budget,
) -> Result<(), GlobError> {
    match ast {
        GlobAst::Empty => Ok(()),
        GlobAst::Lit(ch) => {
            insts.try_push(GlobInst::Lit(*ch))?;
            Ok(())
        }
        GlobAst::Question => {
            insts.try_push(GlobInst::Any)?;
            Ok(())
        }
        GlobAst::Star => {
            let split_pc = insts.len() as u32;
            insts.try_push(GlobInst::Jump(0))?;
            let body_pc = insts.len() as u32;
            insts.try_push(GlobInst::Any)?;
            insts.try_push(GlobInst::Jump(split_pc))?;
            let after_pc = insts.len() as u32;
            insts.as_mut_slice()[split_pc as usize] = GlobInst::Split(body_pc, after_pc);
            Ok(())
        }
        GlobAst::Class(id) => {
            insts.try_push(GlobInst::Class(*id))?;
            Ok(())
        }
        GlobAst::Seq(items) => {
            let slice = items.as_slice();
            if reverse {
                for item in slice.iter().rev() {
                    emit_ast(item, true, insts, b)?;
                }
            } else {
                for item in slice {
                    emit_ast(item, false, insts, b)?;
                }
            }
            Ok(())
        }
        GlobAst::Alt(branches) => {
            let slice = branches.as_slice();
            if slice.is_empty() {
                return Ok(());
            }
            if slice.len() == 1 {
                return emit_ast(&slice[0], reverse, insts, b);
            }
            let mut end_jumps: KVec<u32> = KVec::new_in(b);
            for (i, branch) in slice.iter().enumerate() {
                if i + 1 < slice.len() {
                    let split_pc = insts.len() as u32;
                    insts.try_push(GlobInst::Jump(0))?;
                    let b_start = insts.len() as u32;
                    emit_ast(branch, reverse, insts, b)?;
                    let jmp_pc = insts.len() as u32;
                    insts.try_push(GlobInst::Jump(0))?;
                    end_jumps.try_push(jmp_pc)?;
                    let next_pc = insts.len() as u32;
                    insts.as_mut_slice()[split_pc as usize] = GlobInst::Split(b_start, next_pc);
                } else {
                    emit_ast(branch, reverse, insts, b)?;
                }
            }
            let end_pc = insts.len() as u32;
            for &jpc in end_jumps.as_slice() {
                insts.as_mut_slice()[jpc as usize] = GlobInst::Jump(end_pc);
            }
            Ok(())
        }
        GlobAst::Optional(inner) => {
            let split_pc = insts.len() as u32;
            insts.try_push(GlobInst::Jump(0))?;
            let body_pc = insts.len() as u32;
            emit_ast(inner, reverse, insts, b)?;
            let after_pc = insts.len() as u32;
            insts.as_mut_slice()[split_pc as usize] = GlobInst::Split(body_pc, after_pc);
            Ok(())
        }
        GlobAst::StarOf(inner) => {
            let split_pc = insts.len() as u32;
            insts.try_push(GlobInst::Jump(0))?;
            let body_pc = insts.len() as u32;
            emit_ast(inner, reverse, insts, b)?;
            insts.try_push(GlobInst::Jump(split_pc))?;
            let after_pc = insts.len() as u32;
            insts.as_mut_slice()[split_pc as usize] = GlobInst::Split(body_pc, after_pc);
            Ok(())
        }
        GlobAst::PlusOf(inner) => {
            let body_pc = insts.len() as u32;
            emit_ast(inner, reverse, insts, b)?;
            let split_pc = insts.len() as u32;
            insts.try_push(GlobInst::Split(body_pc, split_pc + 1))?;
            Ok(())
        }
        GlobAst::Not(_) => Ok(()),
    }
}

#[inline]
fn is_leading_dot(subject: &[u8], pos: usize) -> bool {
    pos < subject.len() && subject[pos] == b'.' && (pos == 0 || subject[pos - 1] == b'/')
}

#[inline]
fn lit_matches(pat_ch: u32, subj_ch: u32, nocase: bool) -> bool {
    if nocase {
        char_matches_icase(pat_ch, subj_ch)
    } else {
        pat_ch == subj_ch
    }
}

fn class_matches(class: &GlobClass, ch: u32, opts: GlobOptions, leading_dot: bool) -> bool {
    if opts.pathname {
        if ch == (b'/' as u32) {
            return false;
        }
        if !opts.dotglob && leading_dot {
            return false;
        }
    }
    let mut hit = false;
    for &(lo, hi) in class.ranges.as_slice() {
        if ch >= lo && ch <= hi {
            hit = true;
            break;
        }
        if opts.nocase {
            if lo < 0x80 && hi < 0x80 && ch < 0x80 {
                let b = ch as u8;
                let alt = if b.is_ascii_lowercase() {
                    b.to_ascii_uppercase() as u32
                } else if b.is_ascii_uppercase() {
                    b.to_ascii_lowercase() as u32
                } else {
                    ch
                };
                if alt >= lo && alt <= hi {
                    hit = true;
                    break;
                }
            } else if let Some(c) = char::from_u32(ch) {
                let lc = c.to_lowercase().next().unwrap_or(c) as u32;
                let uc = c.to_uppercase().next().unwrap_or(c) as u32;
                if (lc >= lo && lc <= hi) || (uc >= lo && uc <= hi) {
                    hit = true;
                    break;
                }
            }
        }
    }
    if class.neg { !hit } else { hit }
}

#[inline]
fn any_matches(ch: u32, opts: GlobOptions, leading_dot: bool) -> bool {
    if opts.pathname {
        if ch == (b'/' as u32) {
            return false;
        }
        if !opts.dotglob && leading_dot {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Public entrypoints dispatched to NFA or Reachability DP

pub(crate) fn is_match(
    g: &CompiledGlob,
    subject: &[u8],
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<bool, GlobError> {
    work(1).map_err(|()| GlobError::FuelExhausted)?;
    if !g.parsed.has_not {
        nfa_is_match(g, subject, 0, b, work)
    } else {
        let mut dp = DpCtx::new(g, subject, b, work)?;
        let mut start = dp.empty_set()?;
        start.as_mut_slice()[0] = true;
        let end = dp.eval(&g.parsed.root, &start)?;
        Ok(end.as_slice()[dp.n_scalars])
    }
}

pub(crate) fn strip_prefix(
    g: &CompiledGlob,
    subject: &[u8],
    greedy: bool,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError> {
    work(1).map_err(|()| GlobError::FuelExhausted)?;
    if !g.parsed.has_not {
        nfa_strip_prefix(g, subject, 0, greedy, b, work)
    } else {
        let mut dp = DpCtx::new(g, subject, b, work)?;
        let mut start = dp.empty_set()?;
        start.as_mut_slice()[0] = true;
        let end = dp.eval(&g.parsed.root, &start)?;
        if greedy {
            for i in (0..=dp.n_scalars).rev() {
                if end.as_slice()[i] {
                    return Ok(Some(dp.offsets.as_slice()[i]));
                }
            }
            Ok(None)
        } else {
            for i in 0..=dp.n_scalars {
                if end.as_slice()[i] {
                    return Ok(Some(dp.offsets.as_slice()[i]));
                }
            }
            Ok(None)
        }
    }
}

pub(crate) fn strip_suffix(
    g: &CompiledGlob,
    subject: &[u8],
    greedy: bool,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError> {
    work(1).map_err(|()| GlobError::FuelExhausted)?;
    if !g.parsed.has_not {
        nfa_strip_suffix(g, subject, greedy, b, work)
    } else {
        let mut dp = DpCtx::new(g, subject, b, work)?;
        if greedy {
            for i in 0..=dp.n_scalars {
                let mut start = dp.empty_set()?;
                start.as_mut_slice()[i] = true;
                let end = dp.eval(&g.parsed.root, &start)?;
                if end.as_slice()[dp.n_scalars] {
                    return Ok(Some(dp.offsets.as_slice()[i]));
                }
            }
            Ok(None)
        } else {
            for i in (0..=dp.n_scalars).rev() {
                let mut start = dp.empty_set()?;
                start.as_mut_slice()[i] = true;
                let end = dp.eval(&g.parsed.root, &start)?;
                if end.as_slice()[dp.n_scalars] {
                    return Ok(Some(dp.offsets.as_slice()[i]));
                }
            }
            Ok(None)
        }
    }
}

pub(crate) fn replace_glob(
    g: &CompiledGlob,
    subject: &[u8],
    replacement: &[u8],
    mode: GlobReplaceMode,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, GlobError> {
    match mode {
        GlobReplaceMode::AnchorHead => {
            let mut out = KVec::new_in(b);
            if let Some(end) = strip_prefix(g, subject, true, b, work)? {
                out.try_extend_from_slice(replacement)?;
                out.try_extend_from_slice(&subject[end..])?;
            } else {
                out.try_extend_from_slice(subject)?;
            }
            Ok(out)
        }
        GlobReplaceMode::AnchorTail => {
            let mut out = KVec::new_in(b);
            if let Some(start) = strip_suffix(g, subject, true, b, work)? {
                out.try_extend_from_slice(&subject[..start])?;
                out.try_extend_from_slice(replacement)?;
            } else {
                out.try_extend_from_slice(subject)?;
            }
            Ok(out)
        }
        GlobReplaceMode::First | GlobReplaceMode::All => {
            let global = matches!(mode, GlobReplaceMode::All);
            let mut out = KVec::new_in(b);
            let mut pos = 0usize;
            let mut prev_nonempty_reached_end = false;

            while pos <= subject.len() {
                if pos == subject.len() && prev_nonempty_reached_end {
                    break;
                }
                let matched_end = find_longest_at(g, subject, pos, b, work)?;
                if let Some(end) = matched_end {
                    out.try_extend_from_slice(replacement)?;
                    if !global {
                        out.try_extend_from_slice(&subject[end..])?;
                        return Ok(out);
                    }
                    if end > pos {
                        prev_nonempty_reached_end = end == subject.len();
                        pos = end;
                    } else {
                        prev_nonempty_reached_end = false;
                        if pos >= subject.len() {
                            break;
                        }
                        let (_, step) = decode_scalar(subject, pos);
                        out.try_extend_from_slice(&subject[pos..pos + step])?;
                        pos += step;
                    }
                } else {
                    prev_nonempty_reached_end = false;
                    if pos >= subject.len() {
                        break;
                    }
                    let (_, step) = decode_scalar(subject, pos);
                    out.try_extend_from_slice(&subject[pos..pos + step])?;
                    pos += step;
                }
            }
            Ok(out)
        }
    }
}

fn find_longest_at(
    g: &CompiledGlob,
    subject: &[u8],
    start: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError> {
    if !g.parsed.has_not {
        nfa_strip_prefix(g, subject, start, true, b, work)
    } else {
        let mut dp = DpCtx::new(g, subject, b, work)?;
        let Some(idx) = dp.offsets.as_slice().binary_search(&start).ok() else {
            return Ok(None);
        };
        let mut s = dp.empty_set()?;
        s.as_mut_slice()[idx] = true;
        let end = dp.eval(&g.parsed.root, &s)?;
        for i in (idx..=dp.n_scalars).rev() {
            if end.as_slice()[i] {
                return Ok(Some(dp.offsets.as_slice()[i]));
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Fast Thompson NFA engine (strictly `O(|subject| * |insts|)`)

fn nfa_closure(
    insts: &[GlobInst],
    input: &[u32],
    seen: &mut [u32],
    gen_tag: u32,
    stack: &mut KVec<u32>,
    active: &mut KVec<u32>,
    work_steps: &mut u64,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<bool, GlobError> {
    stack.clear();
    active.clear();
    for &pc in input {
        stack.try_push(pc)?;
    }
    let mut matched = false;
    while let Some(pc) = stack.pop() {
        *work_steps += 1;
        if (*work_steps & 63) == 0 {
            work(1).map_err(|()| GlobError::FuelExhausted)?;
        }
        let idx = pc as usize;
        if seen[idx] == gen_tag {
            continue;
        }
        seen[idx] = gen_tag;
        match insts[idx] {
            GlobInst::Match => matched = true,
            GlobInst::Jump(t) => stack.try_push(t)?,
            GlobInst::Split(a, b_pc) => {
                stack.try_push(b_pc)?;
                stack.try_push(a)?;
            }
            GlobInst::Lit(_) | GlobInst::Any | GlobInst::Class(_) => {
                active.try_push(pc)?;
            }
        }
    }
    Ok(matched)
}

fn nfa_is_match(
    g: &CompiledGlob,
    subject: &[u8],
    start: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<bool, GlobError> {
    let insts = g.fwd.as_slice();
    let mut seen = KVec::with_capacity_in(insts.len(), b)?;
    for _ in 0..insts.len() {
        seen.try_push(0u32)?;
    }
    let mut gen_tag = 1u32;
    let mut stack = KVec::new_in(b);
    let mut curr = KVec::new_in(b);
    let mut active = KVec::new_in(b);
    let mut next = KVec::new_in(b);
    let mut work_steps = 0u64;

    curr.try_push(0u32)?;
    let mut pos = start;
    loop {
        let matched = nfa_closure(
            insts,
            curr.as_slice(),
            seen.as_mut_slice(),
            gen_tag,
            &mut stack,
            &mut active,
            &mut work_steps,
            work,
        )?;
        gen_tag = gen_tag.wrapping_add(1);
        if gen_tag == 0 {
            for x in seen.as_mut_slice() {
                *x = 0;
            }
            gen_tag = 1;
        }
        if pos >= subject.len() {
            return Ok(matched);
        }
        if active.is_empty() {
            return Ok(false);
        }
        let leading_dot = is_leading_dot(subject, pos);
        let (ch, step_len) = decode_scalar(subject, pos);
        next.clear();
        for &pc in active.as_slice() {
            let ok = match insts[pc as usize] {
                GlobInst::Lit(pat_ch) => lit_matches(pat_ch, ch, g.opts.nocase),
                GlobInst::Any => any_matches(ch, g.opts, leading_dot),
                GlobInst::Class(cid) => {
                    class_matches(&g.parsed.classes.as_slice()[cid as usize], ch, g.opts, leading_dot)
                }
                _ => false,
            };
            if ok {
                next.try_push(pc + 1)?;
            }
        }
        core::mem::swap(&mut curr, &mut next);
        pos += step_len;
    }
}

fn nfa_strip_prefix(
    g: &CompiledGlob,
    subject: &[u8],
    start: usize,
    greedy: bool,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError> {
    let insts = g.fwd.as_slice();
    let mut seen = KVec::with_capacity_in(insts.len(), b)?;
    for _ in 0..insts.len() {
        seen.try_push(0u32)?;
    }
    let mut gen_tag = 1u32;
    let mut stack = KVec::new_in(b);
    let mut curr = KVec::new_in(b);
    let mut active = KVec::new_in(b);
    let mut next = KVec::new_in(b);
    let mut work_steps = 0u64;
    let mut best: Option<usize> = None;

    curr.try_push(0u32)?;
    let mut pos = start;
    loop {
        let matched = nfa_closure(
            insts,
            curr.as_slice(),
            seen.as_mut_slice(),
            gen_tag,
            &mut stack,
            &mut active,
            &mut work_steps,
            work,
        )?;
        gen_tag = gen_tag.wrapping_add(1);
        if gen_tag == 0 {
            for x in seen.as_mut_slice() {
                *x = 0;
            }
            gen_tag = 1;
        }
        if matched {
            best = Some(pos);
            if !greedy {
                return Ok(best);
            }
        }
        if pos >= subject.len() || active.is_empty() {
            return Ok(best);
        }
        let leading_dot = is_leading_dot(subject, pos);
        let (ch, step_len) = decode_scalar(subject, pos);
        next.clear();
        for &pc in active.as_slice() {
            let ok = match insts[pc as usize] {
                GlobInst::Lit(pat_ch) => lit_matches(pat_ch, ch, g.opts.nocase),
                GlobInst::Any => any_matches(ch, g.opts, leading_dot),
                GlobInst::Class(cid) => {
                    class_matches(&g.parsed.classes.as_slice()[cid as usize], ch, g.opts, leading_dot)
                }
                _ => false,
            };
            if ok {
                next.try_push(pc + 1)?;
            }
        }
        core::mem::swap(&mut curr, &mut next);
        pos += step_len;
    }
}

fn nfa_strip_suffix(
    g: &CompiledGlob,
    subject: &[u8],
    greedy: bool,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError> {
    // Collect scalar boundaries forward in O(N) so we can walk backward cleanly.
    let mut offsets = KVec::new_in(b);
    let mut scalars = KVec::new_in(b);
    let mut pos = 0;
    while pos < subject.len() {
        offsets.try_push(pos)?;
        let (ch, len) = decode_scalar(subject, pos);
        scalars.try_push(ch)?;
        pos += len;
    }
    offsets.try_push(subject.len())?;

    let insts = g.rev.as_slice();
    let mut seen = KVec::with_capacity_in(insts.len(), b)?;
    for _ in 0..insts.len() {
        seen.try_push(0u32)?;
    }
    let mut gen_tag = 1u32;
    let mut stack = KVec::new_in(b);
    let mut curr = KVec::new_in(b);
    let mut active = KVec::new_in(b);
    let mut next = KVec::new_in(b);
    let mut work_steps = 0u64;
    let mut best: Option<usize> = None;

    curr.try_push(0u32)?;
    let mut idx = scalars.len();
    loop {
        let matched = nfa_closure(
            insts,
            curr.as_slice(),
            seen.as_mut_slice(),
            gen_tag,
            &mut stack,
            &mut active,
            &mut work_steps,
            work,
        )?;
        gen_tag = gen_tag.wrapping_add(1);
        if gen_tag == 0 {
            for x in seen.as_mut_slice() {
                *x = 0;
            }
            gen_tag = 1;
        }
        if matched {
            best = Some(offsets.as_slice()[idx]);
            if !greedy {
                return Ok(best);
            }
        }
        if idx == 0 || active.is_empty() {
            return Ok(best);
        }
        idx -= 1;
        let byte_pos = offsets.as_slice()[idx];
        let ch = scalars.as_slice()[idx];
        let leading_dot = is_leading_dot(subject, byte_pos);
        next.clear();
        for &pc in active.as_slice() {
            let ok = match insts[pc as usize] {
                GlobInst::Lit(pat_ch) => lit_matches(pat_ch, ch, g.opts.nocase),
                GlobInst::Any => any_matches(ch, g.opts, leading_dot),
                GlobInst::Class(cid) => {
                    class_matches(&g.parsed.classes.as_slice()[cid as usize], ch, g.opts, leading_dot)
                }
                _ => false,
            };
            if ok {
                next.try_push(pc + 1)?;
            }
        }
        core::mem::swap(&mut curr, &mut next);
    }
}

// ---------------------------------------------------------------------------
// Exact Reachability DP for `!(...)` extglobs (zero regex lookahead)

struct DpCtx<'a> {
    g: &'a CompiledGlob,
    subject: &'a [u8],
    offsets: KVec<usize>,
    scalars: KVec<u32>,
    n_scalars: usize,
    b: &'a Budget,
    work: &'a mut dyn FnMut(u64) -> Result<(), ()>,
    work_steps: u64,
}

impl<'a> DpCtx<'a> {
    fn new(
        g: &'a CompiledGlob,
        subject: &'a [u8],
        b: &'a Budget,
        work: &'a mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<Self, GlobError> {
        let mut offsets = KVec::new_in(b);
        let mut scalars = KVec::new_in(b);
        let mut pos = 0;
        while pos < subject.len() {
            offsets.try_push(pos)?;
            let (ch, len) = decode_scalar(subject, pos);
            scalars.try_push(ch)?;
            pos += len;
        }
        offsets.try_push(subject.len())?;
        let n_scalars = scalars.len();
        Ok(Self {
            g,
            subject,
            offsets,
            scalars,
            n_scalars,
            b,
            work,
            work_steps: 0,
        })
    }

    fn charge_step(&mut self, n: u64) -> Result<(), GlobError> {
        self.work_steps += n;
        if self.work_steps >= 64 {
            let units = self.work_steps / 64;
            self.work_steps %= 64;
            (self.work)(units).map_err(|()| GlobError::FuelExhausted)?;
        }
        Ok(())
    }

    fn empty_set(&self) -> Result<KVec<bool>, GlobError> {
        let mut v = KVec::with_capacity_in(self.n_scalars + 1, self.b)?;
        for _ in 0..=self.n_scalars {
            v.try_push(false)?;
        }
        Ok(v)
    }

    fn eval(&mut self, ast: &GlobAst, start: &KVec<bool>) -> Result<KVec<bool>, GlobError> {
        let n = self.n_scalars;
        self.charge_step((n as u64).max(1))?;
        match ast {
            GlobAst::Empty => {
                let mut out = self.empty_set()?;
                out.as_mut_slice().copy_from_slice(start.as_slice());
                Ok(out)
            }
            GlobAst::Lit(pat_ch) => {
                let mut out = self.empty_set()?;
                for i in 0..n {
                    if start.as_slice()[i]
                        && lit_matches(*pat_ch, self.scalars.as_slice()[i], self.g.opts.nocase)
                    {
                        out.as_mut_slice()[i + 1] = true;
                    }
                }
                Ok(out)
            }
            GlobAst::Question => {
                let mut out = self.empty_set()?;
                for i in 0..n {
                    if start.as_slice()[i] {
                        let byte_pos = self.offsets.as_slice()[i];
                        let ld = is_leading_dot(self.subject, byte_pos);
                        if any_matches(self.scalars.as_slice()[i], self.g.opts, ld) {
                            out.as_mut_slice()[i + 1] = true;
                        }
                    }
                }
                Ok(out)
            }
            GlobAst::Star => {
                let mut out = self.empty_set()?;
                for i in 0..=n {
                    if !start.as_slice()[i] {
                        continue;
                    }
                    out.as_mut_slice()[i] = true;
                    let mut j = i;
                    while j < n {
                        let byte_pos = self.offsets.as_slice()[j];
                        let ld = is_leading_dot(self.subject, byte_pos);
                        if !any_matches(self.scalars.as_slice()[j], self.g.opts, ld) {
                            break;
                        }
                        j += 1;
                        out.as_mut_slice()[j] = true;
                    }
                }
                Ok(out)
            }
            GlobAst::Class(cid) => {
                let cls = &self.g.parsed.classes.as_slice()[*cid as usize];
                let mut out = self.empty_set()?;
                for i in 0..n {
                    if start.as_slice()[i] {
                        let byte_pos = self.offsets.as_slice()[i];
                        let ld = is_leading_dot(self.subject, byte_pos);
                        if class_matches(cls, self.scalars.as_slice()[i], self.g.opts, ld) {
                            out.as_mut_slice()[i + 1] = true;
                        }
                    }
                }
                Ok(out)
            }
            GlobAst::Seq(items) => {
                let mut cur = self.empty_set()?;
                cur.as_mut_slice().copy_from_slice(start.as_slice());
                for item in items.as_slice() {
                    cur = self.eval(item, &cur)?;
                    if !cur.as_slice().iter().any(|&x| x) {
                        break;
                    }
                }
                Ok(cur)
            }
            GlobAst::Alt(branches) => {
                let mut out = self.empty_set()?;
                for branch in branches.as_slice() {
                    let sub = self.eval(branch, start)?;
                    for i in 0..=n {
                        out.as_mut_slice()[i] |= sub.as_slice()[i];
                    }
                }
                Ok(out)
            }
            GlobAst::Optional(inner) => {
                let mut out = self.eval(inner, start)?;
                for i in 0..=n {
                    out.as_mut_slice()[i] |= start.as_slice()[i];
                }
                Ok(out)
            }
            GlobAst::StarOf(inner) => {
                let mut acc = self.empty_set()?;
                acc.as_mut_slice().copy_from_slice(start.as_slice());
                let mut frontier = self.empty_set()?;
                frontier.as_mut_slice().copy_from_slice(start.as_slice());
                loop {
                    let next = self.eval(inner, &frontier)?;
                    let mut grew = false;
                    for i in 0..=n {
                        if next.as_slice()[i] && !acc.as_slice()[i] {
                            acc.as_mut_slice()[i] = true;
                            frontier.as_mut_slice()[i] = true;
                            grew = true;
                        } else {
                            frontier.as_mut_slice()[i] = false;
                        }
                    }
                    if !grew {
                        break;
                    }
                }
                Ok(acc)
            }
            GlobAst::PlusOf(inner) => {
                let first = self.eval(inner, start)?;
                let mut acc = self.empty_set()?;
                acc.as_mut_slice().copy_from_slice(first.as_slice());
                let mut frontier = first;
                loop {
                    let next = self.eval(inner, &frontier)?;
                    let mut grew = false;
                    for i in 0..=n {
                        if next.as_slice()[i] && !acc.as_slice()[i] {
                            acc.as_mut_slice()[i] = true;
                            frontier.as_mut_slice()[i] = true;
                            grew = true;
                        } else {
                            frontier.as_mut_slice()[i] = false;
                        }
                    }
                    if !grew {
                        break;
                    }
                }
                Ok(acc)
            }
            GlobAst::Not(inner) => {
                let mut out = self.empty_set()?;
                for i in 0..=n {
                    if !start.as_slice()[i] {
                        continue;
                    }
                    let mut single = self.empty_set()?;
                    single.as_mut_slice()[i] = true;
                    let matched = self.eval(inner, &single)?;
                    // A span `i..j` is matched by `!(P)` iff:
                    // 1. In pathname mode, `i..j` contains no `/` and (when `!dotglob` and
                    //    `j > i`) does not begin on a leading `.`.
                    // 2. `inner` does NOT match `i..j`.
                    for j in i..=n {
                        if j > i && self.g.opts.pathname {
                            let ch = self.scalars.as_slice()[j - 1];
                            if ch == (b'/' as u32) {
                                break;
                            }
                            if !self.g.opts.dotglob
                                && j == i + 1
                                && is_leading_dot(self.subject, self.offsets.as_slice()[i])
                            {
                                // Non-empty span starting on a leading `.` is disallowed.
                                break;
                            }
                        }
                        if !matched.as_slice()[j] {
                            out.as_mut_slice()[j] = true;
                        }
                    }
                }
                Ok(out)
            }
        }
    }
}
