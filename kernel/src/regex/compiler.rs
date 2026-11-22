//! Thompson NFA compiler and literal prefilter extractor for `kernel::regex`.
//!
//! Compiles a parsed [`Ast`] into flat [`Inst`] bytecode in `KVec<Inst>`, aborting
//! immediately with [`CapKind::ProgramTooLarge`] if the instruction count reaches
//! `limits.max_program_insts`.

use super::parser::{AssertKind, Ast, Parsed};
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
    insts: KVec<Inst>,
    limit: usize,
    b: &'a Budget,
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

    fn compile_ast(&mut self, ast: &Ast) -> Result<(), RegexError> {
        match ast {
            Ast::Empty => Ok(()),
            Ast::Char { ch, icase } => {
                self.emit(Inst::Char {
                    ch: *ch,
                    icase: *icase,
                })?;
                Ok(())
            }
            Ast::Dot { dot_all } => {
                self.emit(Inst::Dot { dot_all: *dot_all })?;
                Ok(())
            }
            Ast::Class(id) => {
                self.emit(Inst::Class(*id))?;
                Ok(())
            }
            Ast::Assert(kind) => {
                self.emit(Inst::Assert(*kind))?;
                Ok(())
            }
            Ast::Keep => {
                self.emit(Inst::Keep)?;
                Ok(())
            }
            Ast::Backref { group, icase } => {
                self.emit(Inst::Backref {
                    group: *group,
                    icase: *icase,
                })?;
                Ok(())
            }
            Ast::Seq(items) => {
                for item in items.as_slice() {
                    self.compile_ast(item)?;
                }
                Ok(())
            }
            Ast::Alt(branches) => {
                let slice = branches.as_slice();
                if slice.is_empty() {
                    return Ok(());
                }
                if slice.len() == 1 {
                    return self.compile_ast(&slice[0]);
                }
                let mut end_jumps: KVec<u32> = KVec::new_in(self.b);
                for (i, branch) in slice.iter().enumerate() {
                    if i + 1 < slice.len() {
                        let split_pc = self.emit(Inst::Jump(0))?;
                        let branch_start = self.here();
                        self.compile_ast(branch)?;
                        let jmp_pc = self.emit(Inst::Jump(0))?;
                        end_jumps.try_push(jmp_pc)?;
                        let next_pc = self.here();
                        self.patch(split_pc, Inst::Split(branch_start, next_pc));
                    } else {
                        self.compile_ast(branch)?;
                    }
                }
                let end_pc = self.here();
                for &jpc in end_jumps.as_slice() {
                    self.patch(jpc, Inst::Jump(end_pc));
                }
                Ok(())
            }
            Ast::Group { inner, index } => {
                self.emit(Inst::Save(index * 2))?;
                self.compile_ast(inner)?;
                self.emit(Inst::Save(index * 2 + 1))?;
                Ok(())
            }
            Ast::Repeat {
                inner,
                min,
                max,
                lazy,
            } => self.compile_repeat(inner, *min, *max, *lazy),
        }
    }

    fn compile_repeat(
        &mut self,
        inner: &Ast,
        min: u32,
        max: Option<u32>,
        lazy: bool,
    ) -> Result<(), RegexError> {
        // Quick lower-bound check before unrolling nested repeats like `a{1000}{1000}`.
        let min_cost = estimate_min_insts(inner, self.limit);
        let total_copies = max.unwrap_or(min.saturating_add(1)) as usize;
        if min_cost > 0 && min_cost.saturating_mul(total_copies) > self.limit {
            return Err(RegexError::CompileCap(CapKind::ProgramTooLarge {
                limit: self.limit,
            }));
        }

        // Emit `min` mandatory copies.
        for _ in 0..min {
            self.compile_ast(inner)?;
        }

        match max {
            Some(m) => {
                // Emit `m - min` optional copies.
                let opt = m.saturating_sub(min);
                for _ in 0..opt {
                    let split_pc = self.emit(Inst::Jump(0))?;
                    let body_pc = self.here();
                    self.compile_ast(inner)?;
                    let after_pc = self.here();
                    if lazy {
                        self.patch(split_pc, Inst::Split(after_pc, body_pc));
                    } else {
                        self.patch(split_pc, Inst::Split(body_pc, after_pc));
                    }
                }
            }
            None => {
                // Emit `inner*` loop.
                let split_pc = self.emit(Inst::Jump(0))?;
                let body_pc = self.here();
                self.compile_ast(inner)?;
                self.emit(Inst::Jump(split_pc))?;
                let after_pc = self.here();
                if lazy {
                    self.patch(split_pc, Inst::Split(after_pc, body_pc));
                } else {
                    self.patch(split_pc, Inst::Split(body_pc, after_pc));
                }
            }
        }
        Ok(())
    }
}

/// Conservative lower bound on instruction count for `ast`, saturating at `limit + 1`.
fn estimate_min_insts(ast: &Ast, limit: usize) -> usize {
    let cap = limit.saturating_add(1);
    match ast {
        Ast::Empty => 0,
        Ast::Char { .. }
        | Ast::Dot { .. }
        | Ast::Class(_)
        | Ast::Assert(_)
        | Ast::Keep
        | Ast::Backref { .. } => 1,
        Ast::Group { inner, .. } => estimate_min_insts(inner, limit).saturating_add(2).min(cap),
        Ast::Seq(items) => {
            let mut sum = 0usize;
            for item in items.as_slice() {
                sum = sum.saturating_add(estimate_min_insts(item, limit)).min(cap);
            }
            sum
        }
        Ast::Alt(branches) => {
            let mut sum = 0usize;
            for branch in branches.as_slice() {
                sum = sum
                    .saturating_add(estimate_min_insts(branch, limit))
                    .saturating_add(1)
                    .min(cap);
            }
            sum
        }
        Ast::Repeat {
            inner, min, max, ..
        } => {
            let unit = estimate_min_insts(inner, limit).max(1);
            let copies = max.unwrap_or(min.saturating_add(1)) as usize;
            unit.saturating_mul(copies).min(cap)
        }
    }
}

pub(crate) fn compile(
    parsed: &Parsed,
    flags: RegexFlags,
    limits: CompileLimits,
    b: &Budget,
) -> Result<Compiled, RegexError> {
    let mut c = Compiler {
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

    c.compile_ast(&parsed.root)?;

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

    let (prefilter_kind, prefilter_bytes) = extract_prefilter(&parsed.root, flags, b)?;
    Ok(Compiled {
        insts: c.insts,
        prefilter_bytes,
        prefilter_kind,
    })
}

/// Extracts a case-sensitive exact prefix or required literal substring factor.
fn extract_prefilter(
    root: &Ast,
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

    collect_factors(root, &mut at_start, &mut exact, &mut cur_run, &mut best_sub)?;
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

fn collect_factors(
    ast: &Ast,
    at_start: &mut bool,
    exact: &mut KBytes,
    cur_run: &mut KBytes,
    best_sub: &mut KBytes,
) -> MemResult<()> {
    match ast {
        Ast::Empty => Ok(()),
        Ast::Char { ch, icase } => {
            if !*icase {
                if *at_start {
                    if !push_scalar_utf8(exact, *ch)? {
                        *at_start = false;
                    }
                }
                if !push_scalar_utf8(cur_run, *ch)? {
                    flush_run(cur_run, best_sub);
                }
            } else {
                *at_start = false;
                flush_run(cur_run, best_sub);
            }
            Ok(())
        }
        Ast::Assert(AssertKind::StartText | AssertKind::StartLine) => {
            // Leading anchor keeps the prefix anchored to match start.
            Ok(())
        }
        Ast::Assert(_) | Ast::Keep | Ast::Dot { .. } | Ast::Class(_) | Ast::Backref { .. } => {
            *at_start = false;
            flush_run(cur_run, best_sub);
            Ok(())
        }
        Ast::Group { inner, .. } => {
            collect_factors(inner, at_start, exact, cur_run, best_sub)
        }
        Ast::Seq(items) => {
            for item in items.as_slice() {
                collect_factors(item, at_start, exact, cur_run, best_sub)?;
            }
            Ok(())
        }
        Ast::Repeat { inner, min, .. } => {
            if *min >= 1 {
                collect_factors(inner, at_start, exact, cur_run, best_sub)?;
            }
            *at_start = false;
            flush_run(cur_run, best_sub);
            Ok(())
        }
        Ast::Alt(_) => {
            *at_start = false;
            flush_run(cur_run, best_sub);
            Ok(())
        }
    }
}
