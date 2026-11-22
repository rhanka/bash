//! Linear-time glob / extglob matcher engine:
//! - Thompson NFA (`O(|subject| * |insts|)`) for all `!(...)`-free globs and maximal
//!   `!(...)`-free subtrees (forward + reversed), compiled iteratively with an explicit stack.
//! - Exact memoized Reachability DP over packed `u64` bitsets for `!(...)` extglobs
//!   (including arbitrarily nested `!(!(a) ...)`, `+(+(...!(a)...))`, `!(foo)bar`, `!(a*)b`,
//!   strip, and replace), using an explicit frame stack (zero Rust recursion) and caching
//!   every `(node_id, start_idx) -> BitSet` once per `DpCtx`.

use super::parser::{GlobClass, GlobNode, ParsedGlob};
use super::{CompiledGlob, GlobError, GlobOptions, GlobReplaceMode};
use crate::mem::{Budget, KMap, KVec};
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

enum NfaOp {
    Node(u32),
    PatchSplitToHere { split_pc: u32, body_pc: u32 },
    PatchStarLoop { split_pc: u32, body_pc: u32 },
    EmitPlusSplit { body_pc: u32 },
    AltBranch { node: u32, idx: usize },
    AltAfterBranch { node: u32, next_idx: usize, split_pc: u32, branch_start: u32 },
    AltFinish { base: usize },
}

pub(crate) fn compile_nfa(
    nodes: &[GlobNode],
    root: u32,
    reverse: bool,
    b: &Budget,
) -> Result<KVec<GlobInst>, GlobError> {
    let mut insts = KVec::new_in(b);
    let mut stack: KVec<NfaOp> = KVec::new_in(b);
    let mut end_jumps: KVec<u32> = KVec::new_in(b);
    stack.try_push(NfaOp::Node(root))?;

    while let Some(op) = stack.pop() {
        match op {
            NfaOp::Node(id) => match &nodes[id as usize] {
                GlobNode::Empty | GlobNode::Not(_) => {}
                GlobNode::Lit(ch) => insts.try_push(GlobInst::Lit(*ch))?,
                GlobNode::Question => insts.try_push(GlobInst::Any)?,
                GlobNode::Star => {
                    let split_pc = insts.len() as u32;
                    insts.try_push(GlobInst::Jump(0))?;
                    let body_pc = insts.len() as u32;
                    insts.try_push(GlobInst::Any)?;
                    insts.try_push(GlobInst::Jump(split_pc))?;
                    let after_pc = insts.len() as u32;
                    insts.as_mut_slice()[split_pc as usize] = GlobInst::Split(body_pc, after_pc);
                }
                GlobNode::Class(cid) => insts.try_push(GlobInst::Class(*cid))?,
                GlobNode::Seq(items) => {
                    let slice = items.as_slice();
                    if reverse {
                        for &item in slice {
                            stack.try_push(NfaOp::Node(item))?;
                        }
                    } else {
                        for &item in slice.iter().rev() {
                            stack.try_push(NfaOp::Node(item))?;
                        }
                    }
                }
                GlobNode::Alt(branches) => {
                    let slice = branches.as_slice();
                    if slice.is_empty() {
                        continue;
                    }
                    if slice.len() == 1 {
                        stack.try_push(NfaOp::Node(slice[0]))?;
                        continue;
                    }
                    let base = end_jumps.len();
                    stack.try_push(NfaOp::AltFinish { base })?;
                    stack.try_push(NfaOp::AltBranch { node: id, idx: 0 })?;
                }
                GlobNode::Optional(inner) => {
                    let split_pc = insts.len() as u32;
                    insts.try_push(GlobInst::Jump(0))?;
                    let body_pc = insts.len() as u32;
                    stack.try_push(NfaOp::PatchSplitToHere { split_pc, body_pc })?;
                    stack.try_push(NfaOp::Node(*inner))?;
                }
                GlobNode::StarOf(inner) => {
                    let split_pc = insts.len() as u32;
                    insts.try_push(GlobInst::Jump(0))?;
                    let body_pc = insts.len() as u32;
                    stack.try_push(NfaOp::PatchStarLoop { split_pc, body_pc })?;
                    stack.try_push(NfaOp::Node(*inner))?;
                }
                GlobNode::PlusOf(inner) => {
                    let body_pc = insts.len() as u32;
                    stack.try_push(NfaOp::EmitPlusSplit { body_pc })?;
                    stack.try_push(NfaOp::Node(*inner))?;
                }
            },
            NfaOp::PatchSplitToHere { split_pc, body_pc } => {
                let after_pc = insts.len() as u32;
                insts.as_mut_slice()[split_pc as usize] = GlobInst::Split(body_pc, after_pc);
            }
            NfaOp::PatchStarLoop { split_pc, body_pc } => {
                insts.try_push(GlobInst::Jump(split_pc))?;
                let after_pc = insts.len() as u32;
                insts.as_mut_slice()[split_pc as usize] = GlobInst::Split(body_pc, after_pc);
            }
            NfaOp::EmitPlusSplit { body_pc } => {
                let split_pc = insts.len() as u32;
                insts.try_push(GlobInst::Split(body_pc, split_pc + 1))?;
            }
            NfaOp::AltBranch { node, idx } => {
                let GlobNode::Alt(branches) = &nodes[node as usize] else {
                    unreachable!();
                };
                let slice = branches.as_slice();
                if idx + 1 < slice.len() {
                    let split_pc = insts.len() as u32;
                    insts.try_push(GlobInst::Jump(0))?;
                    let branch_start = insts.len() as u32;
                    stack.try_push(NfaOp::AltAfterBranch {
                        node,
                        next_idx: idx + 1,
                        split_pc,
                        branch_start,
                    })?;
                    stack.try_push(NfaOp::Node(slice[idx]))?;
                } else {
                    stack.try_push(NfaOp::Node(slice[idx]))?;
                }
            }
            NfaOp::AltAfterBranch {
                node,
                next_idx,
                split_pc,
                branch_start,
            } => {
                let jmp_pc = insts.len() as u32;
                insts.try_push(GlobInst::Jump(0))?;
                end_jumps.try_push(jmp_pc)?;
                let next_pc = insts.len() as u32;
                insts.as_mut_slice()[split_pc as usize] = GlobInst::Split(branch_start, next_pc);
                stack.try_push(NfaOp::AltBranch {
                    node,
                    idx: next_idx,
                })?;
            }
            NfaOp::AltFinish { base } => {
                let end_pc = insts.len() as u32;
                for &jpc in &end_jumps.as_slice()[base..] {
                    insts.as_mut_slice()[jpc as usize] = GlobInst::Jump(end_pc);
                }
                end_jumps.truncate(base);
            }
        }
    }

    insts.try_push(GlobInst::Match)?;
    Ok(insts)
}

/// Compiles every maximal `!(...)`-free subtree under a `has_not` parent into a flat
/// NFA buffer so the Reachability DP evaluates `Not`-free sub-expressions in a single
/// linear-time Thompson NFA pass (`O(N * |sub_insts|)`).
pub(crate) fn compile_sub_nfas(
    parsed: &ParsedGlob,
    b: &Budget,
) -> Result<(KVec<GlobInst>, KVec<(u32, u32)>), GlobError> {
    let nodes = parsed.nodes.as_slice();
    let has_not = parsed.node_has_not.as_slice();
    let mut sub_insts = KVec::new_in(b);
    let mut spans = KVec::with_capacity_in(nodes.len(), b)?;
    for _ in 0..nodes.len() {
        spans.try_push((0u32, 0u32))?;
    }

    let mut compile_child = |cid: u32| -> Result<(), GlobError> {
        let idx = cid as usize;
        if !has_not[idx] && spans.as_slice()[idx].1 == 0 {
            let nfa = compile_nfa(nodes, cid, false, b)?;
            let start = sub_insts.len() as u32;
            let len = nfa.len() as u32;
            sub_insts.try_extend_from_slice(nfa.as_slice())?;
            spans.as_mut_slice()[idx] = (start, len);
        }
        Ok(())
    };

    for (id, node) in nodes.iter().enumerate() {
        if !has_not[id] {
            continue;
        }
        match node {
            GlobNode::Seq(items) | GlobNode::Alt(items) => {
                for &cid in items.as_slice() {
                    compile_child(cid)?;
                }
            }
            GlobNode::Optional(inner)
            | GlobNode::StarOf(inner)
            | GlobNode::PlusOf(inner)
            | GlobNode::Not(inner) => {
                compile_child(*inner)?;
            }
            GlobNode::Empty
            | GlobNode::Lit(_)
            | GlobNode::Question
            | GlobNode::Star
            | GlobNode::Class(_) => {}
        }
    }

    Ok((sub_insts, spans))
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
// Public entrypoints dispatched to NFA or Memoized Reachability DP

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
        dp.eval_root_single(0)?;
        let n = dp.n_scalars;
        Ok(dp.root_memo(0).get(n))
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
        dp.eval_root_single(0)?;
        let end = dp.root_memo(0);
        let idx = if greedy {
            end.last_set()
        } else {
            end.first_set()
        };
        Ok(idx.map(|i| dp.offsets.as_slice()[i]))
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
        let n = dp.n_scalars;
        if greedy {
            for i in 0..=n {
                dp.eval_root_single(i)?;
                if dp.root_memo(i).get(n) {
                    return Ok(Some(dp.offsets.as_slice()[i]));
                }
            }
            Ok(None)
        } else {
            for i in (0..=n).rev() {
                dp.eval_root_single(i)?;
                if dp.root_memo(i).get(n) {
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

            if !g.parsed.has_not {
                while pos <= subject.len() {
                    if pos == subject.len() && prev_nonempty_reached_end {
                        break;
                    }
                    let matched_end = nfa_strip_prefix(g, subject, pos, true, b, work)?;
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
            } else {
                // Reuse one `DpCtx` and its memo table across all start positions.
                let mut dp = DpCtx::new(g, subject, b, work)?;
                let mut idx = 0usize;
                let n = dp.n_scalars;
                while idx <= n {
                    if idx == n && prev_nonempty_reached_end {
                        break;
                    }
                    dp.eval_root_single(idx)?;
                    let matched_idx = dp.root_memo(idx).last_set().filter(|&e| e >= idx);
                    if let Some(end_idx) = matched_idx {
                        let end_byte = dp.offsets.as_slice()[end_idx];
                        out.try_extend_from_slice(replacement)?;
                        if !global {
                            out.try_extend_from_slice(&subject[end_byte..])?;
                            return Ok(out);
                        }
                        if end_idx > idx {
                            prev_nonempty_reached_end = end_idx == n;
                            idx = end_idx;
                        } else {
                            prev_nonempty_reached_end = false;
                            if idx >= n {
                                break;
                            }
                            let b0 = dp.offsets.as_slice()[idx];
                            let b1 = dp.offsets.as_slice()[idx + 1];
                            out.try_extend_from_slice(&subject[b0..b1])?;
                            idx += 1;
                        }
                    } else {
                        prev_nonempty_reached_end = false;
                        if idx >= n {
                            break;
                        }
                        let b0 = dp.offsets.as_slice()[idx];
                        let b1 = dp.offsets.as_slice()[idx + 1];
                        out.try_extend_from_slice(&subject[b0..b1])?;
                        idx += 1;
                    }
                }
            }
            Ok(out)
        }
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
// Packed bitset + Exact Memoized Reachability DP for `!(...)` extglobs

struct BitSet {
    words: KVec<u64>,
}

impl BitSet {
    fn empty(n_words: usize, b: &Budget) -> Result<Self, GlobError> {
        let mut words = KVec::with_capacity_in(n_words, b)?;
        for _ in 0..n_words {
            words.try_push(0u64)?;
        }
        Ok(Self { words })
    }

    fn try_clone(&self) -> Result<Self, GlobError> {
        let mut words = KVec::with_capacity_in(self.words.len(), self.words.budget())?;
        words.try_extend_from_slice(self.words.as_slice())?;
        Ok(Self { words })
    }

    #[inline]
    fn clear(&mut self) {
        for w in self.words.as_mut_slice() {
            *w = 0;
        }
    }

    #[inline]
    fn set(&mut self, idx: usize) {
        self.words.as_mut_slice()[idx >> 6] |= 1u64 << (idx & 63);
    }

    #[inline]
    fn clear_bit(&mut self, idx: usize) {
        self.words.as_mut_slice()[idx >> 6] &= !(1u64 << (idx & 63));
    }

    #[inline]
    fn get(&self, idx: usize) -> bool {
        (self.words.as_slice()[idx >> 6] & (1u64 << (idx & 63))) != 0
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.words.as_slice().iter().all(|&w| w == 0)
    }

    #[inline]
    fn or_assign(&mut self, other: &BitSet) {
        let dst = self.words.as_mut_slice();
        let src = other.words.as_slice();
        for i in 0..dst.len() {
            dst[i] |= src[i];
        }
    }

    #[inline]
    fn or_and_not_assign(&mut self, src: &BitSet, mask_out: &BitSet) {
        let dst = self.words.as_mut_slice();
        let s = src.words.as_slice();
        let m = mask_out.words.as_slice();
        for i in 0..dst.len() {
            dst[i] |= s[i] & !m[i];
        }
    }

    #[inline]
    fn first_set(&self) -> Option<usize> {
        self.next_set(0)
    }

    #[inline]
    fn next_set(&self, from: usize) -> Option<usize> {
        let slice = self.words.as_slice();
        let mut wi = from >> 6;
        if wi >= slice.len() {
            return None;
        }
        let first_word = slice[wi] & (!0u64 << (from & 63));
        if first_word != 0 {
            return Some((wi << 6) + (first_word.trailing_zeros() as usize));
        }
        wi += 1;
        while wi < slice.len() {
            let w = slice[wi];
            if w != 0 {
                return Some((wi << 6) + (w.trailing_zeros() as usize));
            }
            wi += 1;
        }
        None
    }

    #[inline]
    fn last_set(&self) -> Option<usize> {
        let slice = self.words.as_slice();
        for wi in (0..slice.len()).rev() {
            let w = slice[wi];
            if w != 0 {
                return Some((wi << 6) + (63 - w.leading_zeros() as usize));
            }
        }
        None
    }
}

enum DpFrame {
    Not {
        node: u32,
        start: usize,
        inner: u32,
    },
    Optional {
        node: u32,
        start: usize,
        inner: u32,
    },
    Alt {
        node: u32,
        start: usize,
        branch_idx: usize,
        acc: BitSet,
    },
    Seq {
        node: u32,
        start: usize,
        item_idx: usize,
        cur: BitSet,
        iter_pos: usize,
        next_cur: BitSet,
        iterating: bool,
    },
    Repeat {
        node: u32,
        start: usize,
        inner: u32,
        acc: BitSet,
        frontier: BitSet,
        expanded: BitSet,
    },
}

struct DpCtx<'a> {
    g: &'a CompiledGlob,
    subject: &'a [u8],
    offsets: KVec<usize>,
    scalars: KVec<u32>,
    next_slash: KVec<usize>,
    n_scalars: usize,
    n_words: usize,
    memo: KMap<(u32, usize), BitSet>,
    nfa_seen: KVec<u32>,
    nfa_stack: KVec<u32>,
    nfa_curr: KVec<u32>,
    nfa_active: KVec<u32>,
    nfa_next: KVec<u32>,
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
        let n_words = (n_scalars + 1).div_ceil(64);

        let mut next_slash = KVec::with_capacity_in(n_scalars + 1, b)?;
        for _ in 0..=n_scalars {
            next_slash.try_push(n_scalars)?;
        }
        let mut cur_slash = n_scalars;
        for i in (0..n_scalars).rev() {
            if scalars.as_slice()[i] == (b'/' as u32) {
                cur_slash = i;
            }
            next_slash.as_mut_slice()[i] = cur_slash;
        }

        Ok(Self {
            g,
            subject,
            offsets,
            scalars,
            next_slash,
            n_scalars,
            n_words,
            memo: KMap::new_in(b),
            nfa_seen: KVec::new_in(b),
            nfa_stack: KVec::new_in(b),
            nfa_curr: KVec::new_in(b),
            nfa_active: KVec::new_in(b),
            nfa_next: KVec::new_in(b),
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

    fn root_memo(&self, start: usize) -> &BitSet {
        self.memo
            .get(&(self.g.parsed.root, start))
            .expect("root start index was just evaluated")
    }

    /// Evaluates a `!(...)`-free subtree on an arbitrary start bitset in a single
    /// linear-time Thompson NFA pass (`O(|subject| * |sub_insts|)`).
    fn eval_sub_nfa(&mut self, node_id: u32, start: &BitSet) -> Result<BitSet, GlobError> {
        let Some(first_pos) = start.first_set() else {
            return BitSet::empty(self.n_words, self.b);
        };
        let last_start = start.last_set().unwrap_or(first_pos);
        let (inst_start, inst_len) = self.g.sub_nfa_span.as_slice()[node_id as usize];
        let insts = &self.g.sub_nfa_insts.as_slice()
            [inst_start as usize..(inst_start + inst_len) as usize];

        // Fast path for `GlobNode::Empty` (`[Match]`).
        if insts.len() == 1 && matches!(insts[0], GlobInst::Match) {
            self.charge_step(self.n_words as u64)?;
            return start.try_clone();
        }

        if self.nfa_seen.len() < insts.len() {
            let add = insts.len() - self.nfa_seen.len();
            self.nfa_seen.try_reserve(add)?;
            for _ in 0..add {
                self.nfa_seen.try_push(0u32)?;
            }
        }
        let seen_slice = &mut self.nfa_seen.as_mut_slice()[..insts.len()];
        for x in seen_slice.iter_mut() {
            *x = 0;
        }
        let mut gen_tag = 1u32;
        self.nfa_curr.clear();
        let mut out = BitSet::empty(self.n_words, self.b)?;

        let n = self.n_scalars;
        let opts = self.g.opts;
        let classes = self.g.parsed.classes.as_slice();

        for pos in first_pos..=n {
            if start.get(pos) {
                self.nfa_curr.try_push(0u32)?;
            }
            if self.nfa_curr.is_empty() {
                if pos >= last_start {
                    break;
                }
                continue;
            }
            let matched = nfa_closure(
                insts,
                self.nfa_curr.as_slice(),
                &mut self.nfa_seen.as_mut_slice()[..insts.len()],
                gen_tag,
                &mut self.nfa_stack,
                &mut self.nfa_active,
                &mut self.work_steps,
                self.work,
            )?;
            gen_tag = gen_tag.wrapping_add(1);
            if gen_tag == 0 {
                for x in &mut self.nfa_seen.as_mut_slice()[..insts.len()] {
                    *x = 0;
                }
                gen_tag = 1;
            }
            if matched {
                out.set(pos);
            }
            if pos >= n {
                break;
            }
            self.nfa_next.clear();
            if !self.nfa_active.is_empty() {
                let byte_pos = self.offsets.as_slice()[pos];
                let ch = self.scalars.as_slice()[pos];
                let leading_dot = is_leading_dot(self.subject, byte_pos);
                for &pc in self.nfa_active.as_slice() {
                    let ok = match insts[pc as usize] {
                        GlobInst::Lit(pat_ch) => lit_matches(pat_ch, ch, opts.nocase),
                        GlobInst::Any => any_matches(ch, opts, leading_dot),
                        GlobInst::Class(cid) => {
                            class_matches(&classes[cid as usize], ch, opts, leading_dot)
                        }
                        _ => false,
                    };
                    if ok {
                        self.nfa_next.try_push(pc + 1)?;
                    }
                }
            }
            core::mem::swap(&mut self.nfa_curr, &mut self.nfa_next);
        }

        Ok(out)
    }

    fn eval_sub_nfa_single(&mut self, node_id: u32, start_idx: usize) -> Result<BitSet, GlobError> {
        let mut s = BitSet::empty(self.n_words, self.b)?;
        s.set(start_idx);
        self.eval_sub_nfa(node_id, &s)
    }

    fn build_complement(&mut self, i: usize, matched: &BitSet) -> Result<BitSet, GlobError> {
        let max_j = if !self.g.opts.pathname {
            self.n_scalars
        } else if !self.g.opts.dotglob
            && i < self.n_scalars
            && is_leading_dot(self.subject, self.offsets.as_slice()[i])
        {
            i
        } else {
            self.next_slash.as_slice()[i]
        };

        let mut comp = BitSet::empty(self.n_words, self.b)?;
        let first_w = i >> 6;
        let last_w = max_j >> 6;
        self.charge_step(((last_w - first_w) as u64) + 1)?;
        let dst = comp.words.as_mut_slice();
        let m = matched.words.as_slice();
        for w in first_w..=last_w {
            let lo_bit = if w == first_w { i & 63 } else { 0 };
            let hi_bit = if w == last_w { max_j & 63 } else { 63 };
            let lo_mask = !0u64 << lo_bit;
            let hi_mask = if hi_bit == 63 {
                !0u64
            } else {
                (1u64 << (hi_bit + 1)) - 1
            };
            dst[w] = (!m[w]) & lo_mask & hi_mask;
        }
        Ok(comp)
    }

    fn make_frame(&self, node: u32, start: usize) -> Result<DpFrame, GlobError> {
        let w = self.n_words;
        match &self.g.parsed.nodes.as_slice()[node as usize] {
            GlobNode::Not(inner) => Ok(DpFrame::Not {
                node,
                start,
                inner: *inner,
            }),
            GlobNode::Optional(inner) => Ok(DpFrame::Optional {
                node,
                start,
                inner: *inner,
            }),
            GlobNode::Alt(_) => Ok(DpFrame::Alt {
                node,
                start,
                branch_idx: 0,
                acc: BitSet::empty(w, self.b)?,
            }),
            GlobNode::Seq(_) => {
                let mut cur = BitSet::empty(w, self.b)?;
                cur.set(start);
                Ok(DpFrame::Seq {
                    node,
                    start,
                    item_idx: 0,
                    cur,
                    iter_pos: 0,
                    next_cur: BitSet::empty(w, self.b)?,
                    iterating: false,
                })
            }
            GlobNode::StarOf(inner) => {
                let mut acc = BitSet::empty(w, self.b)?;
                acc.set(start);
                let mut frontier = BitSet::empty(w, self.b)?;
                frontier.set(start);
                Ok(DpFrame::Repeat {
                    node,
                    start,
                    inner: *inner,
                    acc,
                    frontier,
                    expanded: BitSet::empty(w, self.b)?,
                })
            }
            GlobNode::PlusOf(inner) => {
                let mut frontier = BitSet::empty(w, self.b)?;
                frontier.set(start);
                Ok(DpFrame::Repeat {
                    node,
                    start,
                    inner: *inner,
                    acc: BitSet::empty(w, self.b)?,
                    frontier,
                    expanded: BitSet::empty(w, self.b)?,
                })
            }
            _ => unreachable!("only has_not nodes are pushed onto the DP stack"),
        }
    }

    /// Evaluates `(root, root_start)` iteratively via an explicit frame stack, caching
    /// every `(node_id, start_idx)` result in `self.memo` so each pair is computed at most once.
    fn eval_root_single(&mut self, root_start: usize) -> Result<(), GlobError> {
        let root = self.g.parsed.root;
        if self.memo.contains_key(&(root, root_start)) {
            return Ok(());
        }
        let mut stack: KVec<DpFrame> = KVec::new_in(self.b);
        stack.try_push(self.make_frame(root, root_start)?)?;

        let has_not = self.g.parsed.node_has_not.as_slice();
        let nodes = self.g.parsed.nodes.as_slice();
        let words_cost = (self.n_words as u64).max(1);

        while !stack.is_empty() {
            let top_idx = stack.len() - 1;
            let mut push_child: Option<(u32, usize)> = None;
            let mut done = false;

            match &mut stack.as_mut_slice()[top_idx] {
                DpFrame::Not { node, start, inner } => {
                    let node_id = *node;
                    let s_idx = *start;
                    let inner_id = *inner;
                    if !has_not[inner_id as usize] {
                        let matched = self.eval_sub_nfa_single(inner_id, s_idx)?;
                        let comp = self.build_complement(s_idx, &matched)?;
                        self.memo.try_insert((node_id, s_idx), comp)?;
                        done = true;
                    } else if self.memo.contains_key(&(inner_id, s_idx)) {
                        // Clone only the bitset ref via temporary split borrow:
                        let matched = self.memo.get(&(inner_id, s_idx)).unwrap().try_clone()?;
                        let comp = self.build_complement(s_idx, &matched)?;
                        self.memo.try_insert((node_id, s_idx), comp)?;
                        done = true;
                    } else {
                        push_child = Some((inner_id, s_idx));
                    }
                }
                DpFrame::Optional { node, start, inner } => {
                    let node_id = *node;
                    let s_idx = *start;
                    let inner_id = *inner;
                    if !has_not[inner_id as usize] {
                        let mut out = self.eval_sub_nfa_single(inner_id, s_idx)?;
                        out.set(s_idx);
                        self.memo.try_insert((node_id, s_idx), out)?;
                        done = true;
                    } else if let Some(sub) = self.memo.get(&(inner_id, s_idx)) {
                        let mut out = sub.try_clone()?;
                        out.set(s_idx);
                        self.charge_step(words_cost)?;
                        self.memo.try_insert((node_id, s_idx), out)?;
                        done = true;
                    } else {
                        push_child = Some((inner_id, s_idx));
                    }
                }
                DpFrame::Alt {
                    node,
                    start,
                    branch_idx,
                    acc,
                } => {
                    let GlobNode::Alt(branches) = &nodes[*node as usize] else {
                        unreachable!();
                    };
                    let bslice = branches.as_slice();
                    let s_idx = *start;
                    while *branch_idx < bslice.len() {
                        let br = bslice[*branch_idx];
                        if !has_not[br as usize] {
                            let sub = self.eval_sub_nfa_single(br, s_idx)?;
                            acc.or_assign(&sub);
                            *branch_idx += 1;
                        } else if let Some(sub) = self.memo.get(&(br, s_idx)) {
                            acc.or_assign(sub);
                            self.charge_step(words_cost)?;
                            *branch_idx += 1;
                        } else {
                            push_child = Some((br, s_idx));
                            break;
                        }
                    }
                    if push_child.is_none() {
                        done = true;
                    }
                }
                DpFrame::Seq {
                    node,
                    start: _,
                    item_idx,
                    cur,
                    iter_pos,
                    next_cur,
                    iterating,
                } => {
                    let GlobNode::Seq(items) = &nodes[*node as usize] else {
                        unreachable!();
                    };
                    let islice = items.as_slice();
                    while *item_idx < islice.len() && !cur.is_empty() {
                        let item = islice[*item_idx];
                        if !has_not[item as usize] {
                            *cur = self.eval_sub_nfa(item, cur)?;
                            *item_idx += 1;
                        } else {
                            if !*iterating {
                                *iterating = true;
                                *iter_pos = 0;
                                next_cur.clear();
                            }
                            while let Some(j) = cur.next_set(*iter_pos) {
                                if let Some(sub) = self.memo.get(&(item, j)) {
                                    next_cur.or_assign(sub);
                                    self.charge_step(words_cost)?;
                                    *iter_pos = j + 1;
                                } else {
                                    push_child = Some((item, j));
                                    break;
                                }
                            }
                            if push_child.is_some() {
                                break;
                            }
                            core::mem::swap(cur, next_cur);
                            *iterating = false;
                            *item_idx += 1;
                        }
                    }
                    if push_child.is_none() {
                        done = true;
                    }
                }
                DpFrame::Repeat {
                    node: _,
                    start: _,
                    inner,
                    acc,
                    frontier,
                    expanded,
                } => {
                    let inner_id = *inner;
                    while let Some(j) = frontier.first_set() {
                        if let Some(sub) = self.memo.get(&(inner_id, j)) {
                            frontier.clear_bit(j);
                            expanded.set(j);
                            acc.or_assign(sub);
                            frontier.or_and_not_assign(sub, expanded);
                            self.charge_step(words_cost)?;
                        } else {
                            push_child = Some((inner_id, j));
                            break;
                        }
                    }
                    if push_child.is_none() {
                        done = true;
                    }
                }
            }

            if let Some((child_node, child_start)) = push_child {
                stack.try_push(self.make_frame(child_node, child_start)?)?;
            } else if done {
                match stack.pop().unwrap() {
                    DpFrame::Not { .. } | DpFrame::Optional { .. } => {
                        // Already inserted into `self.memo`.
                    }
                    DpFrame::Alt {
                        node, start, acc, ..
                    } => {
                        self.memo.try_insert((node, start), acc)?;
                    }
                    DpFrame::Seq {
                        node, start, cur, ..
                    } => {
                        self.memo.try_insert((node, start), cur)?;
                    }
                    DpFrame::Repeat {
                        node, start, acc, ..
                    } => {
                        self.memo.try_insert((node, start), acc)?;
                    }
                }
            }
        }

        Ok(())
    }
}
