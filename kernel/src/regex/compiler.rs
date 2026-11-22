//! Thompson NFA compiler and literal prefilter extractor for `kernel::regex`.
//!
//! Compiles a parsed post-order [`AstNode`] arena into flat [`Inst`] bytecode in
//! `KVec<Inst>` iteratively (zero Rust call recursion in size estimation, codegen,
//! or prefilter extraction), aborting immediately with [`CapKind::ProgramTooLarge`]
//! if the instruction count reaches `limits.max_program_insts`.

use super::parser::{AssertKind, AstNode, Parsed};
use super::{CapKind, CompileLimits, RegexError, RegexFlags};
use crate::mem::{Budget, KBytes, KVec, MemResult};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Inst {
    /// Exact Unicode scalar (or raw invalid byte `RAW_BYTE_BASE | b`).
    Char { ch: u32, icase: bool },
    /// Any scalar (excluding `\n` unless `dot_all`).
    Dot { dot_all: bool },
    /// Bracket class index in `CompiledRegex::classes`.
    Class(u32),
    /// Zero-width assertion.
    Assert(AssertKind),
    /// Unconditional jump to `pc`.
    Jump(u32),
    /// Ordered choice: branch `a` has higher priority than branch `b`.
    Split(u32, u32),
    /// Save current offset into capture slot (`2*g` = start, `2*g+1` = end).
    Save(u32),
    /// `\K` match-start reset.
    Keep,
    /// Backreference `\1`..`\9` (bounded backtracker only).
    Backref { group: u8, icase: bool },
    /// Accept.
    Match,
}

pub(crate) struct Compiled {
    pub insts: KVec<Inst>,
    pub prefilter_bytes: KBytes,
    pub prefilter_kind: u8, // 0 = none, 1 = exact, 2 = substring
}

struct Compiler<'a> {
    nodes: &'a [AstNode],
    min_insts: &'a [usize],
    insts: KVec<Inst>,
    limit: usize,
    b: &'a Budget,
}

enum CompileOp {
    Node(u32),
    Emit(Inst),
    AltBranch {
        node: u32,
        idx: usize,
    },
    AltAfterBranch {
        node: u32,
        next_idx: usize,
        split_pc: u32,
        branch_start: u32,
    },
    AltFinish {
        base: usize,
    },
    RepeatLoop {
        inner: u32,
        rem_min: u32,
        rem_opt: u32,
        star: bool,
        lazy: bool,
    },
    PatchOptSplit {
        split_pc: u32,
        body_pc: u32,
        lazy: bool,
    },
    PatchStarSplit {
        split_pc: u32,
        body_pc: u32,
        lazy: bool,
    },
}

impl<'a> Compiler<'a> {
    fn emit(&mut self, inst: Inst) -> Result<u32, RegexError> {
        if self.insts.len() >= self.limit {
            return Err(RegexError::CompileCap(CapKind::ProgramTooLarge {
                limit: self.limit,
            }));
        }
        let pc = self.insts.len() as u32;
        self.insts.try_push(inst)?;
        Ok(pc)
    }

    fn patch(&mut self, pc: u32, inst: Inst) {
        self.insts.as_mut_slice()[pc as usize] = inst;
    }

    fn here(&self) -> u32 {
        self.insts.len() as u32
    }

    fn compile_root(&mut self, root: u32) -> Result<(), RegexError> {
        let mut stack: KVec<CompileOp> = KVec::new_in(self.b);
        let mut end_jumps: KVec<u32> = KVec::new_in(self.b);
        stack.try_push(CompileOp::Node(root))?;

        while let Some(op) = stack.pop() {
            match op {
                CompileOp::Emit(inst) => {
                    self.emit(inst)?;
                }
                CompileOp::Node(id) => match &self.nodes[id as usize] {
                    AstNode::Empty => {}
                    AstNode::Char { ch, icase } => {
                        self.emit(Inst::Char {
                            ch: *ch,
                            icase: *icase,
                        })?;
                    }
                    AstNode::Dot { dot_all } => {
                        self.emit(Inst::Dot { dot_all: *dot_all })?;
                    }
                    AstNode::Class(cid) => {
                        self.emit(Inst::Class(*cid))?;
                    }
                    AstNode::Assert(kind) => {
                        self.emit(Inst::Assert(*kind))?;
                    }
                    AstNode::Keep => {
                        self.emit(Inst::Keep)?;
                    }
                    AstNode::Backref { group, icase } => {
                        self.emit(Inst::Backref {
                            group: *group,
                            icase: *icase,
                        })?;
                    }
                    AstNode::Seq(items) => {
                        for &item in items.as_slice().iter().rev() {
                            stack.try_push(CompileOp::Node(item))?;
                        }
                    }
                    AstNode::Alt(branches) => {
                        let slice = branches.as_slice();
                        if slice.is_empty() {
                            continue;
                        }
                        if slice.len() == 1 {
                            stack.try_push(CompileOp::Node(slice[0]))?;
                            continue;
                        }
                        let base = end_jumps.len();
                        stack.try_push(CompileOp::AltFinish { base })?;
                        stack.try_push(CompileOp::AltBranch { node: id, idx: 0 })?;
                    }
                    AstNode::Group { inner, index } => {
                        self.emit(Inst::Save(index * 2))?;
                        stack.try_push(CompileOp::Emit(Inst::Save(index * 2 + 1)))?;
                        stack.try_push(CompileOp::Node(*inner))?;
                    }
                    AstNode::Repeat {
                        inner,
                        min,
                        max,
                        lazy,
                    } => {
                        let min_cost = self.min_insts[*inner as usize];
                        if min_cost == 0 || *max == Some(0) {
                            continue;
                        }
                        let total_copies = max.unwrap_or(min.saturating_add(1)) as usize;
                        if min_cost.saturating_mul(total_copies) > self.limit {
                            return Err(RegexError::CompileCap(CapKind::ProgramTooLarge {
                                limit: self.limit,
                            }));
                        }
                        let rem_opt = max.map_or(0, |m| m.saturating_sub(*min));
                        stack.try_push(CompileOp::RepeatLoop {
                            inner: *inner,
                            rem_min: *min,
                            rem_opt,
                            star: max.is_none(),
                            lazy: *lazy,
                        })?;
                    }
                },
                CompileOp::AltBranch { node, idx } => {
                    let AstNode::Alt(branches) = &self.nodes[node as usize] else {
                        unreachable!();
                    };
                    let slice = branches.as_slice();
                    if idx + 1 < slice.len() {
                        let split_pc = self.emit(Inst::Jump(0))?;
                        let branch_start = self.here();
                        stack.try_push(CompileOp::AltAfterBranch {
                            node,
                            next_idx: idx + 1,
                            split_pc,
                            branch_start,
                        })?;
                        stack.try_push(CompileOp::Node(slice[idx]))?;
                    } else {
                        stack.try_push(CompileOp::Node(slice[idx]))?;
                    }
                }
                CompileOp::AltAfterBranch {
                    node,
                    next_idx,
                    split_pc,
                    branch_start,
                } => {
                    let jmp_pc = self.emit(Inst::Jump(0))?;
                    end_jumps.try_push(jmp_pc)?;
                    let next_pc = self.here();
                    self.patch(split_pc, Inst::Split(branch_start, next_pc));
                    stack.try_push(CompileOp::AltBranch {
                        node,
                        idx: next_idx,
                    })?;
                }
                CompileOp::AltFinish { base } => {
                    let end_pc = self.here();
                    for &jpc in &end_jumps.as_slice()[base..] {
                        self.patch(jpc, Inst::Jump(end_pc));
                    }
                    end_jumps.truncate(base);
                }
                CompileOp::RepeatLoop {
                    inner,
                    rem_min,
                    rem_opt,
                    star,
                    lazy,
                } => {
                    if rem_min > 0 {
                        stack.try_push(CompileOp::RepeatLoop {
                            inner,
                            rem_min: rem_min - 1,
                            rem_opt,
                            star,
                            lazy,
                        })?;
                        stack.try_push(CompileOp::Node(inner))?;
                    } else if rem_opt > 0 {
                        let split_pc = self.emit(Inst::Jump(0))?;
                        let body_pc = self.here();
                        stack.try_push(CompileOp::RepeatLoop {
                            inner,
                            rem_min: 0,
                            rem_opt: rem_opt - 1,
                            star,
                            lazy,
                        })?;
                        stack.try_push(CompileOp::PatchOptSplit {
                            split_pc,
                            body_pc,
                            lazy,
                        })?;
                        stack.try_push(CompileOp::Node(inner))?;
                    } else if star {
                        let split_pc = self.emit(Inst::Jump(0))?;
                        let body_pc = self.here();
                        stack.try_push(CompileOp::PatchStarSplit {
                            split_pc,
                            body_pc,
                            lazy,
                        })?;
                        stack.try_push(CompileOp::Node(inner))?;
                    }
                }
                CompileOp::PatchOptSplit {
                    split_pc,
                    body_pc,
                    lazy,
                } => {
                    let after_pc = self.here();
                    if lazy {
                        self.patch(split_pc, Inst::Split(after_pc, body_pc));
                    } else {
                        self.patch(split_pc, Inst::Split(body_pc, after_pc));
                    }
                }
                CompileOp::PatchStarSplit {
                    split_pc,
                    body_pc,
                    lazy,
                } => {
                    self.emit(Inst::Jump(split_pc))?;
                    let after_pc = self.here();
                    if lazy {
                        self.patch(split_pc, Inst::Split(after_pc, body_pc));
                    } else {
                        self.patch(split_pc, Inst::Split(body_pc, after_pc));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Bottom-up conservative lower bound on instruction count for every post-order node,
/// saturating at `limit + 1`. Single linear `O(|nodes|)` pass with zero recursion.
fn compute_min_insts(nodes: &[AstNode], limit: usize, b: &Budget) -> MemResult<KVec<usize>> {
    let cap = limit.saturating_add(1);
    let mut out: KVec<usize> = KVec::with_capacity_in(nodes.len(), b)?;
    for node in nodes {
        let est = match node {
            AstNode::Empty => 0,
            AstNode::Char { .. }
            | AstNode::Dot { .. }
            | AstNode::Class(_)
            | AstNode::Assert(_)
            | AstNode::Keep
            | AstNode::Backref { .. } => 1,
            AstNode::Group { inner, .. } => out.as_slice()[*inner as usize]
                .saturating_add(2)
                .min(cap),
            AstNode::Seq(items) => {
                let mut sum = 0usize;
                for &item in items.as_slice() {
                    sum = sum.saturating_add(out.as_slice()[item as usize]).min(cap);
                }
                sum
            }
            AstNode::Alt(branches) => {
                let mut sum = 0usize;
                for &branch in branches.as_slice() {
                    sum = sum
                        .saturating_add(out.as_slice()[branch as usize])
                        .saturating_add(1)
                        .min(cap);
                }
                sum
            }
            AstNode::Repeat {
                inner, min, max, ..
            } => {
                let unit = out.as_slice()[*inner as usize];
                if unit == 0 || *max == Some(0) {
                    0
                } else {
                    let copies = max.unwrap_or(min.saturating_add(1)) as usize;
                    unit.saturating_mul(copies).min(cap)
                }
            }
        };
        out.try_push(est)?;
    }
    Ok(out)
}

pub(crate) fn compile(
    parsed: &Parsed,
    flags: RegexFlags,
    limits: CompileLimits,
    b: &Budget,
) -> Result<Compiled, RegexError> {
    let min_insts = compute_min_insts(parsed.nodes.as_slice(), limits.max_program_insts, b)?;
    let mut c = Compiler {
        nodes: parsed.nodes.as_slice(),
        min_insts: min_insts.as_slice(),
        insts: KVec::new_in(b),
        limit: limits.max_program_insts,
        b,
    };

    if flags.line_regexp {
        c.emit(Inst::Assert(if flags.multiline {
            AssertKind::StartLine
        } else {
            AssertKind::StartText
        }))?;
    }
    if flags.whole_word {
        c.emit(Inst::Assert(AssertKind::PrevNotWord))?;
    }

    c.compile_root(parsed.root)?;

    if flags.whole_word {
        c.emit(Inst::Assert(AssertKind::NextNotWord))?;
    }
    if flags.line_regexp {
        c.emit(Inst::Assert(if flags.multiline {
            AssertKind::EndLine
        } else {
            AssertKind::EndText
        }))?;
    }
    c.emit(Inst::Match)?;

    let (prefilter_kind, prefilter_bytes) =
        extract_prefilter(parsed.nodes.as_slice(), parsed.root, flags, b)?;
    Ok(Compiled {
        insts: c.insts,
        prefilter_bytes,
        prefilter_kind,
    })
}

/// Extracts a case-sensitive exact prefix or required literal substring factor.
fn extract_prefilter(
    nodes: &[AstNode],
    root: u32,
    flags: RegexFlags,
    b: &Budget,
) -> MemResult<(u8, KBytes)> {
    if flags.ignore_case {
        return Ok((0, KBytes::new_in(b)));
    }
    let mut exact = KBytes::new_in(b);
    let mut at_start = true;
    let mut cur_run = KBytes::new_in(b);
    let mut best_sub = KBytes::new_in(b);

    collect_factors(
        nodes,
        root,
        &mut at_start,
        &mut exact,
        &mut cur_run,
        &mut best_sub,
        b,
    )?;
    flush_run(&mut cur_run, &mut best_sub);

    if !exact.is_empty() {
        Ok((1, exact))
    } else if !best_sub.is_empty() {
        Ok((2, best_sub))
    } else {
        Ok((0, KBytes::new_in(b)))
    }
}

fn push_scalar_utf8(out: &mut KBytes, ch: u32) -> MemResult<bool> {
    if let Some(c) = char::from_u32(ch) {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        out.try_extend_from_slice(s.as_bytes())?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn flush_run(cur: &mut KBytes, best: &mut KBytes) {
    if cur.len() > best.len() {
        core::mem::swap(cur, best);
    }
    cur.clear();
}

enum FactorOp {
    Node(u32),
    EndRepeat,
}

fn collect_factors(
    nodes: &[AstNode],
    root: u32,
    at_start: &mut bool,
    exact: &mut KBytes,
    cur_run: &mut KBytes,
    best_sub: &mut KBytes,
    b: &Budget,
) -> MemResult<()> {
    let mut stack: KVec<FactorOp> = KVec::new_in(b);
    stack.try_push(FactorOp::Node(root))?;

    while let Some(op) = stack.pop() {
        match op {
            FactorOp::EndRepeat => {
                *at_start = false;
                flush_run(cur_run, best_sub);
            }
            FactorOp::Node(id) => match &nodes[id as usize] {
                AstNode::Empty => {}
                AstNode::Char { ch, icase } => {
                    if !*icase {
                        if *at_start && !push_scalar_utf8(exact, *ch)? {
                            *at_start = false;
                        }
                        if !push_scalar_utf8(cur_run, *ch)? {
                            flush_run(cur_run, best_sub);
                        }
                    } else {
                        *at_start = false;
                        flush_run(cur_run, best_sub);
                    }
                }
                AstNode::Assert(AssertKind::StartText | AssertKind::StartLine) => {
                    // Leading anchor keeps the prefix anchored to match start.
                }
                AstNode::Assert(_)
                | AstNode::Keep
                | AstNode::Dot { .. }
                | AstNode::Class(_)
                | AstNode::Backref { .. }
                | AstNode::Alt(_) => {
                    *at_start = false;
                    flush_run(cur_run, best_sub);
                }
                AstNode::Group { inner, .. } => {
                    stack.try_push(FactorOp::Node(*inner))?;
                }
                AstNode::Seq(items) => {
                    for &item in items.as_slice().iter().rev() {
                        stack.try_push(FactorOp::Node(item))?;
                    }
                }
                AstNode::Repeat { inner, min, .. } => {
                    stack.try_push(FactorOp::EndRepeat)?;
                    if *min >= 1 {
                        stack.try_push(FactorOp::Node(*inner))?;
                    }
                }
            },
        }
    }
    Ok(())
}
