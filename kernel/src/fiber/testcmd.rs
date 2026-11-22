//! Test-only commands (spec 5.3). Compiled only with the `test-commands` feature, so the release kernel
//! does not contain these names.
use super::*;
use crate::base64::{self, Base64DecodeMode, Base64Error, Base64Limits};
use crate::format::{
    format_printf, format_strftime, FormatLimitKind, PrintfOpts,
};
use crate::cmd::{coverage_id, CmdCtx, CmdFrame, CmdPoll, FindTraceStats, TraceDetail, TraceEvent, TraceValue};
use crate::fs::{BatchOk, Code, Ok_, Op};
use crate::hash::{Md5, Sha1, Sha256};
use crate::limits::{I_ARRAY_ELEMENTS, I_OUTPUT_SIZE, I_STRING_LENGTH};
use crate::num::{
    fmt_f64_exp, fmt_f64_fixed, fmt_f64_js, fmt_f64_prec, parse_arith_literal,
    parse_f64_prefix, parse_f64_strict, parse_printf_int, ArithNumError, FloatFmtError,
};
use crate::glob::{
    check_globstar_segments, replace_glob, split_globignore, strip_prefix, strip_suffix,
    CompiledGlob, GlobError, GlobOptions, GlobReplaceMode,
};
use crate::regex::{
    BreDialect, CompileLimits, CompiledRegex, EmptyAdvance, MatchError, RegexError, RegexFlags,
    RegexMode, ReplaceStyle,
};
use crate::shell::{G_ENTROPY, G_HOST_FS, G_NETWORK, G_REAL_CLOCK, G_REAL_SLEEP};
use crate::util::parse_u64;

/// The names of the test-only commands, matched by index so that they exist as data in this build only.
const CMDS: [&[u8]; 29] = [
    b"__out", b"__err", b"__stdin", b"__alloc", b"__spin", b"__exit", b"__setenv", b"__write",
    b"__cat", b"__hostfs", b"__lazy", b"__fetch", b"__sleep", b"__clock", b"__entropy", b"__trap",
    b"__fsalloc", b"__work", b"__walk", b"__fsbatch",
    b"__num", b"__printf", b"__strftime", b"__hash", b"__base64", b"__cov", b"__trace", b"__regex", b"__glob",
];
const C_OUT: usize = 0;
const C_ERR: usize = 1;
const C_STDIN: usize = 2;
const C_ALLOC: usize = 3;
const C_SPIN: usize = 4;
const C_EXIT: usize = 5;
const C_SETENV: usize = 6;
const C_WRITE: usize = 7;
const C_CAT: usize = 8;
const C_HOSTFS: usize = 9;
const C_LAZY: usize = 10;
const C_FETCH: usize = 11;
const C_SLEEP: usize = 12;
const C_CLOCK: usize = 13;
const C_ENTROPY: usize = 14;
const C_TRAP: usize = 15;
const C_FSALLOC: usize = 16;
const C_WORK: usize = 17;
const C_WALK: usize = 18;
const C_FSBATCH: usize = 19;
const C_NUM: usize = 20;
const C_PRINTF: usize = 21;
const C_STRFTIME: usize = 22;
const C_HASH: usize = 23;
const C_BASE64: usize = 24;
const C_COV: usize = 25;
const C_TRACE: usize = 26;
const C_REGEX: usize = 27;
const C_GLOB: usize = 28;

const T_HOSTFS: u8 = 1;
const T_LAZY: u8 = 2;
const T_FETCH: u8 = 3;
const T_SLEEP: u8 = 4;
const T_CLOCK: u8 = 5;
const T_ENTROPY: u8 = 6;
const T_CAT: u8 = 7;
const T_WRITE: u8 = 8;
const T_WALK: u8 = 9;
const T_FSBATCH: u8 = 10;
const FS_CHUNK: u32 = 1 << 20;

struct CovFrame {
    id: u16,
    emitted: u64,
    target: u64,
}

impl CmdFrame for CovFrame {
    fn resume(&mut self, ctx: &mut CmdCtx<'_>, _outcome: Option<Outcome<'_>>) -> Result<CmdPoll, Fail> {
        while self.emitted < self.target {
            if ctx.should_yield() {
                return Ok(CmdPoll::Yield);
            }
            ctx.fiber.hit_coverage_id(self.id)?;
            self.emitted += 1;
        }
        Ok(CmdPoll::Done(0))
    }
}

struct TraceFloodFrame {
    emitted: u64,
    target: u64,
}

impl CmdFrame for TraceFloodFrame {
    fn resume(&mut self, ctx: &mut CmdCtx<'_>, _outcome: Option<Outcome<'_>>) -> Result<CmdPoll, Fail> {
        while self.emitted < self.target {
            if ctx.should_yield() {
                return Ok(CmdPoll::Yield);
            }
            let budget = ctx.live_budget();
            let mut details = KVec::with_capacity_in(1, budget)?;
            details.try_push(TraceDetail {
                key: str_in("seq", budget)?,
                value: TraceValue::Number(self.emitted as f64),
            })?;
            let ev = TraceEvent {
                category: str_in("test", budget)?,
                name: str_in("item", budget)?,
                duration_ms: 0.0,
                details,
            };
            ctx.emit_trace(ev)?;
            self.emitted += 1;
        }
        Ok(CmdPoll::Done(0))
    }
}

enum TraceOp {
    Span { label: KBytes },
    Find { search_path: KBytes, stats: FindTraceStats },
}

enum TraceClockStage {
    Start,
    WaitStart,
    WaitEnd { start_ns: u64 },
}

struct TraceClockFrame {
    op: TraceOp,
    stage: TraceClockStage,
}

impl CmdFrame for TraceClockFrame {
    fn resume(&mut self, ctx: &mut CmdCtx<'_>, outcome: Option<Outcome<'_>>) -> Result<CmdPoll, Fail> {
        match self.stage {
            TraceClockStage::Start => {
                if let Some(req) = ctx.trace_clock_req() {
                    self.stage = TraceClockStage::WaitStart;
                    return Ok(CmdPoll::Host(req));
                }
                if let TraceOp::Span { label } = &self.op {
                    ctx.stdout(label)?;
                    ctx.stdout(b"\n")?;
                }
                Ok(CmdPoll::Done(0))
            }
            TraceClockStage::WaitStart => {
                let Some(Outcome::Clock { monotonic_ns, .. }) = outcome else {
                    return Ok(CmdPoll::Done(1));
                };
                if let TraceOp::Span { label } = &self.op {
                    ctx.stdout(label)?;
                    ctx.stdout(b"\n")?;
                }
                self.stage = TraceClockStage::WaitEnd { start_ns: monotonic_ns };
                let Some(req) = ctx.trace_clock_req() else {
                    return Ok(CmdPoll::Done(0));
                };
                Ok(CmdPoll::Host(req))
            }
            TraceClockStage::WaitEnd { start_ns } => {
                let Some(Outcome::Clock { monotonic_ns: end_ns, .. }) = outcome else {
                    return Ok(CmdPoll::Done(1));
                };
                match &self.op {
                    TraceOp::Span { label } => {
                        let budget = ctx.live_budget();
                        let duration_ms = (end_ns.saturating_sub(start_ns) as f64) / 1_000_000.0;
                        let mut details = KVec::with_capacity_in(1, budget)?;
                        details.try_push(TraceDetail {
                            key: str_in("label", budget)?,
                            value: TraceValue::Text(bytes_in(label, budget)?),
                        })?;
                        ctx.emit_trace(TraceEvent {
                            category: str_in("test", budget)?,
                            name: str_in("span", budget)?,
                            duration_ms,
                            details,
                        })?;
                    }
                    TraceOp::Find { search_path, stats } => {
                        ctx.emit_find_trace(search_path, start_ns, end_ns, stats)?;
                    }
                }
                Ok(CmdPoll::Done(0))
            }
        }
    }
}

impl Fiber {
    fn write_chunks(&mut self, chunks: &KChunks) -> Result<(), Fail> {
        if !self.out_len_ok(chunks.len()) { return Err(Fail::Output); }
        for chunk in chunks.chunks() { self.write_out(Chan::Stdout, chunk)?; }
        Ok(())
    }

    fn t_bad(&mut self, name: &[u8]) -> Flow {
        self.last_exit = 2;
        let n = core::str::from_utf8(name).unwrap_or("?");
        match self.say(Chan::Stderr, |s| write!(s, "bash: {n}: invalid argument\n")) {
            Ok(()) => Flow::Next,
            Err(f) => Flow::Fail(f),
        }
    }

    fn t_denied(&mut self) -> Flow {
        self.last_exit = 126;
        match self.write_out(Chan::Stderr, b"denied\n") {
            Ok(()) => Flow::Next,
            Err(f) => Flow::Fail(f),
        }
    }

    fn start_frame(&mut self, mut frame: KBox<dyn CmdFrame>) -> Flow {
        let poll = match frame.resume(&mut CmdCtx { fiber: self, xpg_echo: false }, None) {
            Ok(poll) => poll,
            Err(fail) => return Flow::Fail(fail),
        };
        match poll {
            CmdPoll::Done(code) => {
                self.last_exit = code;
                Flow::Next
            }
            CmdPoll::Yield => {
                self.active = Some(frame);
                Flow::Yield
            }
            CmdPoll::Host(req) => {
                self.active = Some(frame);
                Flow::Host(Pending::Kernel, req)
            }
        }
    }

    fn t_ok(&mut self, r: Result<(), Fail>) -> Flow {
        match r {
            Ok(()) => Flow::Next,
            Err(f) => Flow::Fail(f),
        }
    }

    fn test_batch<const N: usize>(
        &mut self, id: u32, command: u8, original: &[u8],
        build: impl FnOnce(&Budget) -> MemResult<[BatchOp; N]>,
    ) -> Flow {
        if !self.shell.granted(G_HOST_FS) {
            return self.fs_exit(Err(Code::Permission), original);
        }
        let built = (|| {
            let mut ops = KVec::with_capacity_in(N, &self.scratch)?;
            for op in build(&self.scratch)? {
                ops.try_push(op)?;
            }
            Ok(ops)
        })();
        self.emit_test_batch(id, command, original, built)
    }

    fn emit_test_batch(&mut self, id: u32, command: u8, original: &[u8], built: MemResult<KVec<BatchOp>>) -> Flow {
        let built = built.and_then(|ops| {
            self.fs_test_path = bytes_in(original, &self.scratch)?;
            Ok(ops)
        });
        match built {
            Ok(ops) => match self.host_fs_batch(id, ops) {
                Ok(req) => Flow::Host(Pending::Test(command, 0), req),
                Err(code) => self.fs_exit(Err(code), original),
            },
            Err(e) => Flow::Fail(Fail::Mem(e)),
        }
    }

    fn normalized_test_path(&self, path: &[u8]) -> Result<KBytes, Code> {
        // Charge the normalized bytes, best-effort: the resume gate yields when the
        // slice is spent, but the fuel itself is always accounted here.
        let mut charged = path.len();
        let _ = self.pay_bytes(&mut charged);
        if path.starts_with(b"/") {
            return self.shell.root.norm(path);
        }
        let mut full = bytes_in(&self.cwd, &self.scratch).map_err(|_| Code::NoSpace)?;
        full.try_push(b'/').map_err(|_| Code::NoSpace)?;
        full.try_extend_from_slice(path).map_err(|_| Code::NoSpace)?;
        self.shell.root.norm(&full)
    }

    pub(super) fn exec_test(&mut self, name: &[u8], args: &[&[u8]]) -> Option<Flow> {
        let num = |i: usize| args.get(i).and_then(|a| parse_u64(a));
        let Some(idx) = CMDS.iter().position(|c| *c == name) else { return None };
        let flow = match idx {
            C_OUT | C_ERR => {
                let Some(n) = num(0) else { return Some(self.t_bad(name)) };
                let c = if idx == C_OUT { Chan::Stdout } else { Chan::Stderr };
                self.last_exit = 0;
                let r = self.fill_out(c, b'a', n);
                self.t_ok(r)
            }
            C_STDIN => {
                let data = core::mem::replace(&mut self.stdin, KChunks::new_in(&self.live));
                let r = self.write_chunks(&data);
                self.last_exit = 0;
                self.t_ok(r)
            }
            C_ALLOC => {
                let Some(n) = num(0) else { return Some(self.t_bad(name)) };
                let Ok(n) = usize::try_from(n) else { return Some(Flow::Fail(Fail::Mem(MemError::Overflow))) };
                match KBytes::with_capacity_in(n, &self.live).and_then(|b| self.kept.try_push(b)) {
                    Ok(()) => {
                        self.last_exit = 0;
                        Flow::Next
                    }
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            C_FSALLOC => {
                let (Some(path), Some(n)) = (args.first(), num(1)) else { return Some(self.t_bad(name)) };
                match self.shell.root.allocate_file(path, n) {
                    Ok(true) => {
                        self.last_exit = 0;
                        Flow::Next
                    }
                    Ok(false) => {
                        self.last_exit = 1;
                        let written = self.write_out(Chan::Stderr, b"__fsalloc: Is a directory\n");
                        self.t_ok(written)
                    }
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            C_WORK => {
                let Some(n) = num(0) else { return Some(self.t_bad(name)) };
                self.last_exit = 0;
                self.t_ok(self.charge_work(n))
            }
            C_SPIN => Flow::Spin,
            C_EXIT => {
                let Some(n) = num(0) else { return Some(self.t_bad(name)) };
                self.last_exit = n as i32;
                Flow::Exit
            }
            C_SETENV => {
                let (Some(k), Some(v)) = (args.first(), args.get(1)) else { return Some(self.t_bad(name)) };
                match self.env.set(k, v, true) {
                    Ok(()) => {
                        self.last_exit = 0;
                        Flow::Next
                    }
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            C_WRITE => {
                let (Some(p), Some(t)) = (args.first(), args.get(1)) else { return Some(self.t_bad(name)) };
                let path = match self.normalized_test_path(p) {
                    Ok(path) => path,
                    Err(code) => return Some(self.fs_exit(Err(code), p)),
                };
                if let Some((id, rest)) = self.shell.root.resolve_host_mount(&path) {
                    if !self.shell.granted(G_HOST_FS) {
                        return Some(self.fs_exit(Err(Code::Permission), p));
                    }
                    // One chunk per host call so no single ABI value exceeds 1 MiB
                    // (D6.4). The full payload is stashed and the T_WRITE
                    // continuation appends the rest chunk by chunk.
                    let text: &[u8] = t;
                    let mut charged = text.len();
                    let _ = self.pay_bytes(&mut charged);
                    let data = match bytes_in(text, &self.live) {
                        Ok(data) => data,
                        Err(e) => return Some(Flow::Fail(Fail::Mem(e))),
                    };
                    let end = text.len().min(FS_CHUNK as usize);
                    let built = (|| {
                        let mut ops = KVec::new_in(&self.scratch);
                        ops.try_push(BatchOp::WriteFile {
                            path: bytes_in(rest, &self.scratch)?,
                            data: bytes_in(&text[..end], &self.scratch)?,
                            offset: Some(0), truncate: true, mode: None, mtime_ms: None,
                        })?;
                        Ok(ops)
                    })();
                    self.fs_write_data = Some(data);
                    return Some(self.emit_test_batch(id, T_WRITE, p, built));
                }
                let r = self.shell.root.op(Op::WriteFile { path: p, data: t, offset: Some(0), truncate: true, mode: None, mtime_ms: None });
                self.fs_exit(r.map(|_| ()), p)
            }
            C_CAT => {
                let Some(p) = args.first() else { return Some(self.t_bad(name)) };
                let path = match self.normalized_test_path(p) {
                    Ok(path) => path,
                    Err(code) => return Some(self.fs_exit(Err(code), p)),
                };
                if let Some((id, rest)) = self.shell.root.resolve_host_mount(&path) {
                    return Some(self.test_batch(id, T_CAT, p, |b| Ok([BatchOp::ReadFile {
                        path: bytes_in(rest, b)?, offset: 0, len: FS_CHUNK,
                    }])));
                }
                let mut offset = 0u64;
                loop {
                    match self.shell.root.op(Op::ReadFile { path: p, offset, len: 1 << 20 }) {
                        Ok(Ok_::Data(d)) => {
                            if d.is_empty() {
                                self.last_exit = 0;
                                break Flow::Next;
                            }
                            offset += d.len() as u64;
                            if let Err(f) = self.write_out(Chan::Stdout, &d) {
                                break Flow::Fail(f);
                            }
                        }
                        Ok(_) => break Flow::Next,
                        Err(c) => break self.fs_exit(Err(c), p),
                    }
                }
            }
            C_HOSTFS => {
                let (Some(id), Some(p)) = (num(0), args.get(1)) else { return Some(self.t_bad(name)) };
                if !self.shell.granted(G_HOST_FS) {
                    return Some(self.t_denied());
                }
                self.test_batch(id as u32, T_HOSTFS, p, |b| Ok([BatchOp::Stat(bytes_in(p, b)?)]))
            }
            C_WALK => {
                let (Some(p), Some(depth), Some(entries)) = (args.first(), num(1), num(2)) else { return Some(self.t_bad(name)) };
                let path = match self.normalized_test_path(p) {
                    Ok(path) => path,
                    Err(code) => return Some(self.fs_exit(Err(code), p)),
                };
                let Some((id, rest)) = self.shell.root.resolve_host_mount(&path) else {
                    return Some(self.fs_exit(Err(Code::Unsupported), p));
                };
                let (Ok(max_depth), Ok(max_entries)) = (u32::try_from(depth), u32::try_from(entries)) else {
                    return Some(self.t_bad(name));
                };
                self.test_batch(id, T_WALK, p, |b| Ok([BatchOp::Walk { root: bytes_in(rest, b)?, max_depth, max_entries }]))
            }
            C_FSBATCH => {
                let Some(id) = num(0).and_then(|id| u32::try_from(id).ok()) else { return Some(self.t_bad(name)) };
                self.test_batch(id, T_FSBATCH, b"/fb", |b| {
                    let dir = || bytes_in(b"/fb", b);
                    let file = || bytes_in(b"/fb/x", b);
                    Ok([
                        BatchOp::Mkdir { path: dir()?, recursive: false, mode: None },
                        BatchOp::CreateExclusive { path: file()?, mode: 0o600, directory: false },
                        BatchOp::WriteFile { path: file()?, data: bytes_in(b"x", b)?, offset: Some(0), truncate: true, mode: None, mtime_ms: None },
                        BatchOp::ReadDir(dir()?),
                        BatchOp::Stat(file()?),
                        BatchOp::Remove { path: dir()?, recursive: true, force: false },
                    ])
                })
            }
            C_LAZY => {
                let Some(p) = args.first() else { return Some(self.t_bad(name)) };
                if !self.shell.granted(G_HOST_FS) {
                    return Some(self.t_denied());
                }
                match bytes_in(p, &self.scratch) {
                    Ok(path) => Flow::Host(Pending::Test(T_LAZY, 0), HostReq::LazyFile(path)),
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            C_FETCH => {
                let Some(u) = args.first() else { return Some(self.t_bad(name)) };
                if !self.shell.granted(G_NETWORK) {
                    return Some(self.t_denied());
                }
                match bytes_in(u, &self.scratch) {
                    Ok(url) => Flow::Host(Pending::Test(T_FETCH, 0), HostReq::Fetch { url }),
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            C_SLEEP => {
                let Some(ms) = num(0) else { return Some(self.t_bad(name)) };
                if self.shell.granted(G_REAL_SLEEP) {
                    Flow::Host(Pending::Test(T_SLEEP, 0), HostReq::Sleep(ms))
                } else {
                    self.shell.virt_ms.set(self.shell.virt_ms.get().saturating_add(ms));
                    self.last_exit = 0;
                    Flow::Next
                }
            }
            C_CLOCK => {
                if self.shell.granted(G_REAL_CLOCK) {
                    Flow::Host(Pending::Test(T_CLOCK, 0), HostReq::Clock)
                } else {
                    let ms = self.shell.virt_ms.get();
                    self.last_exit = 0;
                    let r = self.say(Chan::Stdout, |s| write!(s, "{ms}\n"));
                    self.t_ok(r)
                }
            }
            C_ENTROPY => {
                let Some(n) = num(0) else { return Some(self.t_bad(name)) };
                if self.shell.granted(G_ENTROPY) {
                    Flow::Host(Pending::Test(T_ENTROPY, 0), HostReq::Entropy(n as u32))
                } else {
                    if n > 1 << 16 {
                        return Some(self.t_bad(name));
                    }
                    if !self.out_len_ok(n) {
                        return Some(Flow::Fail(Fail::Output));
                    }
                    let mut left = n as usize;
                    while left != 0 {
                        let bytes = self.shell.next_random().to_le_bytes();
                        let len = left.min(bytes.len());
                        if let Err(f) = self.write_out(Chan::StdoutBytes, &bytes[..len]) {
                            return Some(Flow::Fail(f));
                        }
                        left -= len;
                    }
                    self.last_exit = 0;
                    Flow::Next
                }
            }
            C_TRAP => {
                #[cfg(target_arch = "wasm32")]
                core::arch::wasm32::unreachable();
                #[cfg(not(target_arch = "wasm32"))]
                panic!("__trap");
            }
            C_NUM => self.exec_test_num(name, args),
            C_PRINTF => self.exec_test_printf(name, args),
            C_STRFTIME => self.exec_test_strftime(name, args),
            C_HASH => self.exec_test_hash(name, args),
            C_BASE64 => self.exec_test_base64(name, args),
            C_COV => {
                let Some(count) = num(0) else { return Some(self.t_bad(name)) };
                let feature = match args.get(1) {
                    Some(b) => core::str::from_utf8(b).ok(),
                    None => Some("bash:cmd:SimpleCommand"),
                };
                let Some(id) = feature.and_then(coverage_id) else {
                    return Some(self.t_bad(name));
                };
                match KBox::try_new_in(CovFrame { id, emitted: 0, target: count }, &self.live) {
                    Ok(frame) => self.start_frame(crate::kbox_unsize!(frame, dyn CmdFrame)),
                    Err((_, e)) => Flow::Fail(Fail::Mem(e)),
                }
            }
            C_TRACE => {
                let Some(op) = args.first().copied() else { return Some(self.t_bad(name)) };
                match op {
                    b"flood" => {
                        let Some(count) = num(1) else { return Some(self.t_bad(name)) };
                        match KBox::try_new_in(TraceFloodFrame { emitted: 0, target: count }, &self.live) {
                            Ok(frame) => self.start_frame(crate::kbox_unsize!(frame, dyn CmdFrame)),
                            Err((_, e)) => Flow::Fail(Fail::Mem(e)),
                        }
                    }
                    b"span" => {
                        let Some(label) = args.get(1) else { return Some(self.t_bad(name)) };
                        let label = match bytes_in(label, &self.live) {
                            Ok(b) => b,
                            Err(e) => return Some(Flow::Fail(Fail::Mem(e))),
                        };
                        let frame = TraceClockFrame {
                            op: TraceOp::Span { label },
                            stage: TraceClockStage::Start,
                        };
                        match KBox::try_new_in(frame, &self.live) {
                            Ok(frame) => self.start_frame(crate::kbox_unsize!(frame, dyn CmdFrame)),
                            Err((_, e)) => Flow::Fail(Fail::Mem(e)),
                        }
                    }
                    b"find" => {
                        let (
                            Some(raw_path),
                            Some(results_found),
                            Some(readdir_calls),
                            Some(stat_calls),
                            Some(eval_calls),
                            Some(node_count),
                            Some(batch_count),
                            Some(early_prunes),
                        ) = (
                            args.get(1),
                            num(2),
                            num(3),
                            num(4),
                            num(5),
                            num(6),
                            num(7),
                            num(8),
                        ) else {
                            return Some(self.t_bad(name));
                        };
                        if args.len() != 9 {
                            return Some(self.t_bad(name));
                        }
                        let search_path = match bytes_in(raw_path, &self.live) {
                            Ok(b) => b,
                            Err(e) => return Some(Flow::Fail(Fail::Mem(e))),
                        };
                        let stats = FindTraceStats {
                            readdir_calls,
                            stat_calls,
                            eval_calls,
                            node_count,
                            batch_count,
                            early_prunes,
                            readdir_fuel: 10,
                            stat_fuel: 5,
                            eval_fuel: 5,
                            batch_fuel: 20,
                            total_fuel: 40,
                            results_found,
                        };
                        let frame = TraceClockFrame {
                            op: TraceOp::Find { search_path, stats },
                            stage: TraceClockStage::Start,
                        };
                        match KBox::try_new_in(frame, &self.live) {
                            Ok(frame) => self.start_frame(crate::kbox_unsize!(frame, dyn CmdFrame)),
                            Err((_, e)) => Flow::Fail(Fail::Mem(e)),
                        }
                    }
                    _ => self.t_bad(name),
                }
            }
            C_REGEX => self.exec_regex(name, args),
            C_GLOB => self.exec_glob(name, args),
            _ => return None,
        };
        Some(flow)
    }

    fn exec_regex(&mut self, name: &[u8], args: &[&[u8]]) -> Flow {
        let (Some(&mode_arg), Some(&flags_arg), Some(&pattern)) =
            (args.first(), args.get(1), args.get(2))
        else {
            return self.t_bad(name);
        };
        let replacement = args.get(3).copied();
        let mode = match mode_arg {
            b"bre-grep" => RegexMode::Basic(BreDialect::Grep),
            b"bre-sed" => RegexMode::Basic(BreDialect::Sed),
            b"bre-expr" => RegexMode::Basic(BreDialect::Expr),
            b"ere" => RegexMode::Extended,
            b"fixed" => RegexMode::Fixed,
            b"perl" => RegexMode::Perl,
            _ => return self.t_bad(name),
        };
        let mut flags = RegexFlags::default();
        let mut only_matching = false;
        let mut invert = false;
        let mut count_only = false;
        let mut line_numbers = false;
        let mut global_replace = false;
        let mut detail_err = false;
        if flags_arg != b"-" {
            for &ch in flags_arg {
                match ch {
                    b'i' => flags.ignore_case = true,
                    b'm' => flags.multiline = true,
                    b's' => flags.dot_all = true,
                    b'w' => flags.whole_word = true,
                    b'x' => flags.line_regexp = true,
                    b'b' => flags.allow_backrefs = true,
                    b'o' => only_matching = true,
                    b'v' => invert = true,
                    b'c' => count_only = true,
                    b'n' => line_numbers = true,
                    b'g' => global_replace = true,
                    b'D' => detail_err = true,
                    _ => return self.t_bad(name),
                }
            }
        }
        let mut compiled = match CompiledRegex::compile(
            pattern,
            mode,
            flags,
            CompileLimits::default(),
            &self.live,
        ) {
            Ok(c) => c,
            Err(RegexError::Mem(e)) => return Flow::Fail(Fail::Mem(e)),
            Err(err) => {
                self.last_exit = 2;
                let r = if detail_err {
                    self.say(Chan::Stderr, |s| write!(s, "__regex: {err:?}\n"))
                } else {
                    self.write_out(Chan::Stderr, b"grep: invalid regular expression: ")
                        .and_then(|()| self.write_out(Chan::Stderr, pattern))
                        .and_then(|()| self.write_out(Chan::Stderr, b"\n"))
                };
                return self.t_ok(r);
            }
        };

        // Drain stdin into a contiguous buffer under `live`.
        let stdin_chunks = core::mem::replace(&mut self.stdin, KChunks::new_in(&self.live));
        let mut input = match KBytes::with_capacity_in(stdin_chunks.len() as usize, &self.live) {
            Ok(v) => v,
            Err(e) => return Flow::Fail(Fail::Mem(e)),
        };
        for chunk in stdin_chunks.chunks() {
            if let Err(e) = input.try_extend_from_slice(chunk) {
                return Flow::Fail(Fail::Mem(e));
            }
        }

        let mut out = KBytes::new_in(&self.live);
        let mut matched_lines = 0usize;
        let mut line_no = 0usize;
        let mut pos = 0usize;
        let data = input.as_slice();
        while pos < data.len() {
            let nl = data[pos..].iter().position(|&b| b == b'\n');
            let (line, next_pos) = match nl {
                Some(rel) => (&data[pos..pos + rel], pos + rel + 1),
                None => (&data[pos..], data.len()),
            };
            pos = next_pos;
            line_no += 1;

            if let Some(rep) = replacement {
                let nth = if global_replace { None } else { Some(1) };
                let res = compiled.replace_all(
                    line,
                    rep,
                    ReplaceStyle::Sed,
                    nth,
                    EmptyAdvance::PosixSed,
                    &self.live,
                    &mut |u| self.charge_work(u).map_err(|_| ()),
                );
                let (replaced, _) = match res {
                    Ok(v) => v,
                    Err(MatchError::Mem(e)) => return Flow::Fail(Fail::Mem(e)),
                    Err(MatchError::FuelExhausted | MatchError::BacktrackLimitExceeded) => {
                        return Flow::Fail(Fail::Work);
                    }
                };
                if let Err(e) = out
                    .try_extend_from_slice(replaced.as_slice())
                    .and_then(|()| out.try_push(b'\n'))
                {
                    return Flow::Fail(Fail::Mem(e));
                }
            } else if only_matching && !invert && !count_only {
                let spans = match compiled.find_all(
                    line,
                    EmptyAdvance::Standard,
                    &self.live,
                    &mut |u| self.charge_work(u).map_err(|_| ()),
                ) {
                    Ok(s) => s,
                    Err(MatchError::Mem(e)) => return Flow::Fail(Fail::Mem(e)),
                    Err(MatchError::FuelExhausted | MatchError::BacktrackLimitExceeded) => {
                        return Flow::Fail(Fail::Work);
                    }
                };
                for sp in spans.as_slice() {
                    if sp.end > sp.start {
                        matched_lines += 1;
                        if line_numbers {
                            if let Err(e) = push_usize_prefix(&mut out, line_no) {
                                return Flow::Fail(Fail::Mem(e));
                            }
                        }
                        if let Err(e) = out
                            .try_extend_from_slice(&line[sp.start..sp.end])
                            .and_then(|()| out.try_push(b'\n'))
                        {
                            return Flow::Fail(Fail::Mem(e));
                        }
                    }
                }
            } else {
                let hit = match compiled.is_match(line, &self.live, &mut |u| {
                    self.charge_work(u).map_err(|_| ())
                }) {
                    Ok(h) => h,
                    Err(MatchError::Mem(e)) => return Flow::Fail(Fail::Mem(e)),
                    Err(MatchError::FuelExhausted | MatchError::BacktrackLimitExceeded) => {
                        return Flow::Fail(Fail::Work);
                    }
                };
                if hit ^ invert {
                    matched_lines += 1;
                    if !count_only {
                        if line_numbers {
                            if let Err(e) = push_usize_prefix(&mut out, line_no) {
                                return Flow::Fail(Fail::Mem(e));
                            }
                        }
                        if let Err(e) = out
                            .try_extend_from_slice(line)
                            .and_then(|()| out.try_push(b'\n'))
                        {
                            return Flow::Fail(Fail::Mem(e));
                        }
                    }
                }
            }
        }

        if replacement.is_some() {
            self.last_exit = 0;
            let r = self.write_out(Chan::Stdout, out.as_slice());
            self.t_ok(r)
        } else if count_only {
            self.last_exit = if matched_lines > 0 { 0 } else { 1 };
            let r = self.say(Chan::Stdout, |s| write!(s, "{matched_lines}\n"));
            self.t_ok(r)
        } else {
            self.last_exit = if matched_lines > 0 { 0 } else { 1 };
            let r = self.write_out(Chan::Stdout, out.as_slice());
            self.t_ok(r)
        }
    }

    fn exec_glob(&mut self, name: &[u8], args: &[&[u8]]) -> Flow {
        let (Some(&op), Some(&flags_arg), Some(&pattern)) =
            (args.first(), args.get(1), args.get(2))
        else {
            return self.t_bad(name);
        };
        let arg1 = args.get(3).copied().unwrap_or(b"");
        let arg2 = args.get(4).copied().unwrap_or(b"");
        let mut opts = GlobOptions::default();
        if flags_arg != b"-" {
            for &ch in flags_arg {
                match ch {
                    b'e' => opts.extglob = true,
                    b'i' => opts.nocase = true,
                    b'p' => opts.pathname = true,
                    b'd' => opts.dotglob = true,
                    _ => return self.t_bad(name),
                }
            }
        }

        let handle_err = |f: &mut Fiber, err: GlobError| -> Flow {
            match err {
                GlobError::TooManyGlobstars { max } => {
                    f.finish_early(
                        126,
                        Status::LimitExceeded,
                        Some("maxGlobOperations"),
                        Some(format_args!(
                            "bash: Glob pattern has too many ** segments (max {max})\n"
                        )),
                    );
                    Flow::Yield
                }
                GlobError::ExtglobDepthExceeded { .. } => {
                    f.last_exit = 2;
                    let r = f.write_out(Chan::Stderr, b"__glob: ExtglobDepthExceeded\n");
                    f.t_ok(r)
                }
                GlobError::InvalidCharacterRange => {
                    f.last_exit = 2;
                    let r = f.write_out(Chan::Stderr, b"__glob: InvalidCharacterRange\n");
                    f.t_ok(r)
                }
                GlobError::FuelExhausted => Flow::Fail(Fail::Work),
                GlobError::Mem(e) => Flow::Fail(Fail::Mem(e)),
            }
        };

        match op {
            b"globstar-check" => match check_globstar_segments(pattern) {
                Ok(n) => {
                    self.last_exit = 0;
                    let r = self.say(Chan::Stdout, |s| write!(s, "{n}\n"));
                    self.t_ok(r)
                }
                Err(e) => handle_err(self, e),
            },
            b"globignore" => match split_globignore(pattern, &self.live) {
                Ok(items) => {
                    let mut out = KBytes::new_in(&self.live);
                    for item in items.as_slice() {
                        if let Err(e) = out
                            .try_extend_from_slice(item)
                            .and_then(|()| out.try_push(b'\n'))
                        {
                            return Flow::Fail(Fail::Mem(e));
                        }
                    }
                    self.last_exit = 0;
                    let r = self.write_out(Chan::Stdout, out.as_slice());
                    self.t_ok(r)
                }
                Err(e) => Flow::Fail(Fail::Mem(e)),
            },
            b"match" => {
                let compiled = match CompiledGlob::compile(pattern, opts, &self.live) {
                    Ok(c) => c,
                    Err(e) => return handle_err(self, e),
                };
                match compiled.is_match(arg1, &self.live, &mut |u| {
                    self.charge_work(u).map_err(|_| ())
                }) {
                    Ok(m) => {
                        self.last_exit = 0;
                        let r = self.write_out(Chan::Stdout, if m { b"match\n" } else { b"no\n" });
                        self.t_ok(r)
                    }
                    Err(e) => handle_err(self, e),
                }
            }
            b"strip-prefix" | b"strip-prefix-greedy" => {
                let greedy = op == b"strip-prefix-greedy";
                match strip_prefix(arg1, pattern, greedy, opts, &self.live, &mut |u| {
                    self.charge_work(u).map_err(|_| ())
                }) {
                    Ok(end) => {
                        let slice = match end {
                            Some(e) => &arg1[e..],
                            None => arg1,
                        };
                        self.last_exit = 0;
                        let r = self
                            .write_out(Chan::Stdout, slice)
                            .and_then(|()| self.write_out(Chan::Stdout, b"\n"));
                        self.t_ok(r)
                    }
                    Err(e) => handle_err(self, e),
                }
            }
            b"strip-suffix" | b"strip-suffix-greedy" => {
                let greedy = op == b"strip-suffix-greedy";
                match strip_suffix(arg1, pattern, greedy, opts, &self.live, &mut |u| {
                    self.charge_work(u).map_err(|_| ())
                }) {
                    Ok(start) => {
                        let slice = match start {
                            Some(s) => &arg1[..s],
                            None => arg1,
                        };
                        self.last_exit = 0;
                        let r = self
                            .write_out(Chan::Stdout, slice)
                            .and_then(|()| self.write_out(Chan::Stdout, b"\n"));
                        self.t_ok(r)
                    }
                    Err(e) => handle_err(self, e),
                }
            }
            b"replace" | b"replace-all" => {
                let mode = if op == b"replace-all" {
                    GlobReplaceMode::All
                } else {
                    GlobReplaceMode::First
                };
                match replace_glob(arg1, pattern, arg2, mode, opts, &self.live, &mut |u| {
                    self.charge_work(u).map_err(|_| ())
                }) {
                    Ok(res) => {
                        self.last_exit = 0;
                        let r = self
                            .write_out(Chan::Stdout, res.as_slice())
                            .and_then(|()| self.write_out(Chan::Stdout, b"\n"));
                        self.t_ok(r)
                    }
                    Err(e) => handle_err(self, e),
                }
            }
            _ => self.t_bad(name),
        }
    }

    fn emit_exit(&mut self, exit: i32, chan: Chan, bytes: &[u8]) -> Flow {
        self.last_exit = exit;
        let r = self.write_out(chan, bytes);
        self.t_ok(r)
    }

    fn limit_yield(&mut self, limit_name: &str, msg: core::fmt::Arguments<'_>) -> Flow {
        self.finish_early(126, Status::LimitExceeded, Some(limit_name), Some(msg));
        Flow::Yield
    }

    fn parse_num_arg(raw: &[u8], bits: bool) -> Option<f64> {
        if bits && (raw.starts_with(b"0x") || raw.starts_with(b"0X")) {
            let s = core::str::from_utf8(&raw[2..]).ok()?;
            let u = u64::from_str_radix(s, 16).ok()?;
            Some(f64::from_bits(u))
        } else {
            parse_f64_strict(raw)
        }
    }

    fn parse_i64_signed(raw: &[u8]) -> Option<i64> {
        let s = core::str::from_utf8(raw).ok()?;
        s.parse::<i64>().ok()
    }

    fn exec_test_num(&mut self, name: &[u8], args: &[&[u8]]) -> Flow {
        let Some(op) = args.first().copied() else {
            return self.t_bad(name);
        };
        let Some(arg1) = args.get(1).copied() else {
            return self.t_bad(name);
        };
        match op {
            b"js" => {
                let is_bits = args.get(2).copied() == Some(b"bits".as_slice());
                let Some(val) = Self::parse_num_arg(arg1, is_bits) else {
                    return self.t_bad(name);
                };
                let mut s = KString::new_in(&self.scratch);
                if let Err(e) = fmt_f64_js(val, &mut s).and_then(|()| s.try_push('\n')) {
                    return Flow::Fail(Fail::Mem(e));
                }
                self.emit_exit(0, Chan::Stdout, s.as_str().as_bytes())
            }
            b"fixed" => {
                let Some(prec_raw) = args.get(2).copied() else {
                    return self.t_bad(name);
                };
                let Some(prec_u64) = parse_u64(prec_raw) else {
                    return self.t_bad(name);
                };
                let Ok(prec) = usize::try_from(prec_u64) else {
                    return self.t_bad(name);
                };
                let is_bits = args.get(3).copied() == Some(b"bits".as_slice());
                let Some(val) = Self::parse_num_arg(arg1, is_bits) else {
                    return self.t_bad(name);
                };
                match fmt_f64_fixed(val, prec, &self.scratch) {
                    Ok(mut s) => {
                        if let Err(e) = s.try_push('\n') {
                            return Flow::Fail(Fail::Mem(e));
                        }
                        self.emit_exit(0, Chan::Stdout, s.as_str().as_bytes())
                    }
                    Err(FloatFmtError::PrecisionRange) => {
                        self.emit_exit(2, Chan::Stderr, b"__num: precision range\n")
                    }
                    Err(FloatFmtError::Mem(e)) => Flow::Fail(Fail::Mem(e)),
                }
            }
            b"exp" => {
                let Some(prec_raw) = args.get(2).copied() else {
                    return self.t_bad(name);
                };
                let Some(prec_u64) = parse_u64(prec_raw) else {
                    return self.t_bad(name);
                };
                let Ok(prec) = usize::try_from(prec_u64) else {
                    return self.t_bad(name);
                };
                let flags = args.get(3).copied().unwrap_or(b"-");
                let (is_bits, upper, two_digit) = if flags == b"bits" {
                    (true, false, false)
                } else {
                    (
                        flags.contains(&b'b'),
                        flags.contains(&b'u'),
                        flags.contains(&b'2'),
                    )
                };
                let Some(val) = Self::parse_num_arg(arg1, is_bits) else {
                    return self.t_bad(name);
                };
                match fmt_f64_exp(val, prec, upper, two_digit, &self.scratch) {
                    Ok(mut s) => {
                        if let Err(e) = s.try_push('\n') {
                            return Flow::Fail(Fail::Mem(e));
                        }
                        self.emit_exit(0, Chan::Stdout, s.as_str().as_bytes())
                    }
                    Err(FloatFmtError::PrecisionRange) => {
                        self.emit_exit(2, Chan::Stderr, b"__num: precision range\n")
                    }
                    Err(FloatFmtError::Mem(e)) => Flow::Fail(Fail::Mem(e)),
                }
            }
            b"prec" => {
                let Some(sig_raw) = args.get(2).copied() else {
                    return self.t_bad(name);
                };
                let Some(sig_u64) = parse_u64(sig_raw) else {
                    return self.t_bad(name);
                };
                let Ok(sig) = usize::try_from(sig_u64) else {
                    return self.t_bad(name);
                };
                let flags = args.get(3).copied().unwrap_or(b"-");
                let (is_bits, upper, keep_zeros, two_digit) = if flags == b"bits" {
                    (true, false, true, false)
                } else {
                    (
                        flags.contains(&b'b'),
                        flags.contains(&b'u'),
                        flags.contains(&b'k'),
                        flags.contains(&b'2'),
                    )
                };
                let Some(val) = Self::parse_num_arg(arg1, is_bits) else {
                    return self.t_bad(name);
                };
                match fmt_f64_prec(val, sig, upper, keep_zeros, two_digit, &self.scratch) {
                    Ok(mut s) => {
                        if let Err(e) = s.try_push('\n') {
                            return Flow::Fail(Fail::Mem(e));
                        }
                        self.emit_exit(0, Chan::Stdout, s.as_str().as_bytes())
                    }
                    Err(FloatFmtError::PrecisionRange) => {
                        self.emit_exit(2, Chan::Stderr, b"__num: precision range\n")
                    }
                    Err(FloatFmtError::Mem(e)) => Flow::Fail(Fail::Mem(e)),
                }
            }
            b"arith" => match parse_arith_literal(arg1) {
                Ok(val) => {
                    let mut s = KString::new_in(&self.scratch);
                    if let Err(e) = fmt_f64_js(val, &mut s).and_then(|()| s.try_push('\n')) {
                        return Flow::Fail(Fail::Mem(e));
                    }
                    self.emit_exit(0, Chan::Stdout, s.as_str().as_bytes())
                }
                Err(ArithNumError::ValueTooGreatForBase) => {
                    self.emit_exit(1, Chan::Stderr, b"bash: value too great for base\n")
                }
                Err(ArithNumError::InvalidNumber) => {
                    self.emit_exit(1, Chan::Stderr, b"bash: invalid number\n")
                }
            },
            b"printf-int" => {
                let r = parse_printf_int(arg1);
                let mut s = KString::new_in(&self.scratch);
                if let Err(e) = fmt_f64_js(r.value, &mut s)
                    .and_then(|()| s.try_push('|'))
                    .and_then(|()| s.try_push(if r.invalid { '1' } else { '0' }))
                    .and_then(|()| s.try_push('\n'))
                {
                    return Flow::Fail(Fail::Mem(e));
                }
                self.emit_exit(0, Chan::Stdout, s.as_str().as_bytes())
            }
            b"f64-prefix" | b"f64-strict" => {
                let parsed = if op == b"f64-prefix" {
                    parse_f64_prefix(arg1)
                } else {
                    parse_f64_strict(arg1)
                };
                match parsed {
                    Some(v) => {
                        let mut s = KString::new_in(&self.scratch);
                        if let Err(e) = fmt_f64_js(v, &mut s).and_then(|()| s.try_push('\n')) {
                            return Flow::Fail(Fail::Mem(e));
                        }
                        self.emit_exit(0, Chan::Stdout, s.as_str().as_bytes())
                    }
                    None => self.emit_exit(1, Chan::Stdout, b"none\n"),
                }
            }
            _ => self.t_bad(name),
        }
    }

    fn format_limit_flow(&mut self, err: FormatLimitKind, work_err: Option<Fail>) -> Flow {
        if let Some(fail) = work_err {
            return Flow::Fail(fail);
        }
        match err {
            FormatLimitKind::Mem(e) => Flow::Fail(Fail::Mem(e)),
            FormatLimitKind::EscapedStringSize { limit_bytes } => {
                self.finish_early(
                    126,
                    Status::LimitExceeded,
                    Some("maxStringLength"),
                    Some(format_args!("bash: escaped string size limit exceeded ({limit_bytes} bytes)\n")),
                );
                Flow::Yield
            }
            FormatLimitKind::Width { limit_bytes } => {
                self.finish_early(
                    126,
                    Status::LimitExceeded,
                    Some("maxStringLength"),
                    Some(format_args!("bash: format width limit exceeded ({limit_bytes} bytes)\n")),
                );
                Flow::Yield
            }
            FormatLimitKind::Precision { limit_bytes } => {
                self.finish_early(
                    126,
                    Status::LimitExceeded,
                    Some("maxStringLength"),
                    Some(format_args!("bash: format precision limit exceeded ({limit_bytes} bytes)\n")),
                );
                Flow::Yield
            }
            FormatLimitKind::AwkFloatPrecision => {
                self.finish_early(
                    126,
                    Status::LimitExceeded,
                    Some("maxStringLength"),
                    Some(format_args!("bash: printf floating-point precision limit exceeded (100 digits)\n")),
                );
                Flow::Yield
            }
            FormatLimitKind::FormattedValueSize { limit_bytes } => {
                self.finish_early(
                    126,
                    Status::LimitExceeded,
                    Some("maxStringLength"),
                    Some(format_args!("bash: formatted value size limit exceeded ({limit_bytes} bytes)\n")),
                );
                Flow::Yield
            }
            FormatLimitKind::OutputSize { limit_bytes } => {
                self.finish_early(
                    126,
                    Status::LimitExceeded,
                    Some("maxStringLength"),
                    Some(format_args!("bash: printf: output size limit exceeded ({limit_bytes} bytes)\n")),
                );
                Flow::Yield
            }
            FormatLimitKind::StrftimeOutputSize { limit_bytes } => {
                self.finish_early(
                    126,
                    Status::LimitExceeded,
                    Some("maxStringLength"),
                    Some(format_args!("bash: strftime: output size limit exceeded ({limit_bytes} bytes)\n")),
                );
                Flow::Yield
            }
            FormatLimitKind::StrftimeIterations { max_ops } => {
                self.finish_early(
                    126,
                    Status::LimitExceeded,
                    Some("maxStringLength"),
                    Some(format_args!("bash: strftime: iteration limit exceeded ({max_ops})\n")),
                );
                Flow::Yield
            }
        }
    }

    fn exec_test_printf(&mut self, name: &[u8], args: &[&[u8]]) -> Flow {
        let mut pos = 0usize;
        let mut saw_dash_dash = false;
        let mut tz_override: Option<&[u8]> = None;

        while pos < args.len() {
            if args[pos] == b"--" {
                saw_dash_dash = true;
                pos += 1;
                break;
            }
            if args[pos] == b"--tz" {
                if pos + 1 >= args.len() {
                    return self.t_bad(name);
                }
                tz_override = Some(args[pos + 1]);
                pos += 2;
                continue;
            }
            break;
        }

        if pos >= args.len() {
            return self.emit_exit(
                if saw_dash_dash { 1 } else { 2 },
                Chan::Stderr,
                b"printf: usage: printf format [arguments]\n",
            );
        }

        let fmt = args[pos];
        let val_args = &args[pos + 1..];
        let tz = tz_override.or_else(|| self.env.get(b"TZ").map(|e| e.value.as_slice()));
        let max_output_bytes = (self.shell.lim.vals[I_STRING_LENGTH]
            .min(self.shell.lim.vals[I_OUTPUT_SIZE])) as usize;
        let now_sec = (self.shell.virt_ms.get() / 1000) as i64;
        let opts = PrintfOpts {
            tz,
            now_sec,
            start_sec: 0,
            max_output_bytes,
        };

        let work_cell = &self.work;
        let work_cap = self.shell.lim.vals[crate::limits::I_WORK_UNITS];
        let mut work_err: Option<Fail> = None;
        let mut charge = |units: u64| -> Result<(), ()> {
            let Some(next) = work_cell.get().checked_add(units) else {
                work_err = Some(Fail::Work);
                return Err(());
            };
            if next > work_cap {
                work_err = Some(Fail::Work);
                return Err(());
            }
            work_cell.set(next);
            Ok(())
        };

        let res = format_printf(fmt, val_args, &opts, &self.scratch, &mut charge);
        match res {
            Ok(out) => {
                if let Err(f) = self.write_out(Chan::Stdout, out.bytes.as_slice()) {
                    return Flow::Fail(f);
                }
                if out.had_error {
                    if let Err(f) = self.write_out(Chan::Stderr, out.stderr.as_str().as_bytes()) {
                        return Flow::Fail(f);
                    }
                }
                self.last_exit = i32::from(out.had_error);
                Flow::Next
            }
            Err(err) => self.format_limit_flow(err, work_err),
        }
    }

    fn exec_test_strftime(&mut self, name: &[u8], args: &[&[u8]]) -> Flow {
        let (Some(sec_raw), Some(fmt)) = (args.first(), args.get(1)) else {
            return self.t_bad(name);
        };
        let Some(epoch_sec) = Self::parse_i64_signed(sec_raw) else {
            return self.t_bad(name);
        };
        let tz = args
            .get(2)
            .copied()
            .or_else(|| self.env.get(b"TZ").map(|e| e.value.as_slice()));
        let max_ops = (self.shell.lim.vals[6] as usize).max(1024);
        let max_out = (self.shell.lim.vals[I_STRING_LENGTH]
            .min(self.shell.lim.vals[I_OUTPUT_SIZE])) as usize;

        let work_cell = &self.work;
        let work_cap = self.shell.lim.vals[crate::limits::I_WORK_UNITS];
        let mut work_err: Option<Fail> = None;
        let mut charge = |units: u64| -> Result<(), ()> {
            let Some(next) = work_cell.get().checked_add(units) else {
                work_err = Some(Fail::Work);
                return Err(());
            };
            if next > work_cap {
                work_err = Some(Fail::Work);
                return Err(());
            }
            work_cell.set(next);
            Ok(())
        };

        match format_strftime(fmt, epoch_sec, tz, max_ops, max_out, &self.scratch, &mut charge) {
            Ok(mut out) => {
                if let Err(e) = out.try_push(b'\n') {
                    return Flow::Fail(Fail::Mem(e));
                }
                self.emit_exit(0, Chan::Stdout, out.as_slice())
            }
            Err(err) => self.format_limit_flow(err, work_err),
        }
    }

    fn exec_test_hash(&mut self, name: &[u8], args: &[&[u8]]) -> Flow {
        let Some(alg) = args.first().copied() else {
            return self.t_bad(name);
        };
        let label = args.get(1).copied().unwrap_or(b"-");
        let label_str = core::str::from_utf8(label).unwrap_or("-");

        enum Hasher {
            Md5(Md5),
            Sha1(Sha1),
            Sha256(Sha256),
        }
        let mut hasher = match alg {
            b"md5" => Hasher::Md5(Md5::new()),
            b"sha1" => Hasher::Sha1(Sha1::new()),
            b"sha256" => Hasher::Sha256(Sha256::new()),
            _ => return self.t_bad(name),
        };

        let mut fed = 0usize;
        for chunk in self.stdin.chunks() {
            match &mut hasher {
                Hasher::Md5(h) => h.update(chunk),
                Hasher::Sha1(h) => h.update(chunk),
                Hasher::Sha256(h) => h.update(chunk),
            }
            fed += chunk.len();
            if fed >= 4096 {
                let units = (fed / 4096) as u64;
                fed %= 4096;
                if let Err(f) = self.charge_work(units) {
                    return Flow::Fail(f);
                }
            }
        }
        if fed > 0 {
            if let Err(f) = self.charge_work(1) {
                return Flow::Fail(f);
            }
        }

        let mut line = KString::new_in(&self.scratch);
        let write_res = match hasher {
            Hasher::Md5(h) => {
                let hex = Md5::hex_lower(&h.finalize());
                let hex_s = core::str::from_utf8(&hex).unwrap();
                write!(line, "{hex_s}  {label_str}\n")
            }
            Hasher::Sha1(h) => {
                let hex = Sha1::hex_lower(&h.finalize());
                let hex_s = core::str::from_utf8(&hex).unwrap();
                write!(line, "{hex_s}  {label_str}\n")
            }
            Hasher::Sha256(h) => {
                let hex = Sha256::hex_lower(&h.finalize());
                let hex_s = core::str::from_utf8(&hex).unwrap();
                write!(line, "{hex_s}  {label_str}\n")
            }
        };
        if write_res.is_err() {
            if let Some(e) = line.take_fmt_error() {
                return Flow::Fail(Fail::Mem(e));
            }
        }
        self.emit_exit(0, Chan::Stdout, line.as_str().as_bytes())
    }

    fn exec_test_base64(&mut self, name: &[u8], args: &[&[u8]]) -> Flow {
        let Some(mode) = args.first().copied() else {
            return self.t_bad(name);
        };
        let total = usize::try_from(self.stdin.len()).unwrap_or(usize::MAX);
        let mut input = match KBytes::with_capacity_in(total, &self.scratch) {
            Ok(v) => v,
            Err(e) => return Flow::Fail(Fail::Mem(e)),
        };
        for chunk in self.stdin.chunks() {
            if let Err(e) = input.try_extend_from_slice(chunk) {
                return Flow::Fail(Fail::Mem(e));
            }
        }

        let limits = Base64Limits {
            max_string_length: self.shell.lim.vals[I_STRING_LENGTH] as usize,
            max_output_size: self.shell.lim.vals[I_OUTPUT_SIZE] as usize,
            max_array_elements: self.shell.lim.vals[I_ARRAY_ELEMENTS] as usize,
        };

        let work_cell = &self.work;
        let work_cap = self.shell.lim.vals[crate::limits::I_WORK_UNITS];
        let mut charge = |units: u64| -> Result<(), ()> {
            let Some(next) = work_cell.get().checked_add(units) else {
                return Err(());
            };
            if next > work_cap {
                return Err(());
            }
            work_cell.set(next);
            Ok(())
        };

        let (res, chan) = match mode {
            b"encode" => {
                let wrap_cols = match args.get(1) {
                    Some(raw) => match parse_u64(raw).and_then(|v| usize::try_from(v).ok()) {
                        Some(w) => w,
                        None => return self.t_bad(name),
                    },
                    None => 76,
                };
                (
                    base64::encode(input.as_slice(), wrap_cols, limits, &self.scratch, &mut charge),
                    Chan::Stdout,
                )
            }
            b"decode" => (
                base64::decode(
                    input.as_slice(),
                    Base64DecodeMode::Permissive,
                    limits,
                    &self.scratch,
                    &mut charge,
                ),
                Chan::StdoutBytes,
            ),
            b"decode-strict" => (
                base64::decode(
                    input.as_slice(),
                    Base64DecodeMode::Strict,
                    limits,
                    &self.scratch,
                    &mut charge,
                ),
                Chan::StdoutBytes,
            ),
            _ => return self.t_bad(name),
        };

        match res {
            Ok(out) => self.emit_exit(0, chan, out.as_slice()),
            Err(Base64Error::InvalidInput) => {
                self.emit_exit(1, Chan::Stderr, b"base64: invalid input\n")
            }
            Err(Base64Error::FuelExhausted) => Flow::Fail(Fail::Work),
            Err(Base64Error::Mem(e)) => Flow::Fail(Fail::Mem(e)),
            Err(Base64Error::WrappedLineLimit { limit }) => self.limit_yield(
                "maxArrayElements",
                format_args!("bash: base64: wrapped line limit exceeded ({limit})\n"),
            ),
            Err(Base64Error::EncodedSizeLimit { limit }) => self.limit_yield(
                "maxStringLength",
                format_args!("bash: base64: encoded size limit exceeded ({limit} bytes)\n"),
            ),
            Err(Base64Error::DecodedSizeLimit { limit }) => self.limit_yield(
                "maxStringLength",
                format_args!("bash: base64: decoded size limit exceeded ({limit} bytes)\n"),
            ),
            Err(Base64Error::OutputSizeLimit { limit }) => self.limit_yield(
                "maxOutputSize",
                format_args!("bash: base64: output size limit exceeded ({limit} bytes)\n"),
            ),
        }
    }

    fn fs_exit(&mut self, r: Result<(), Code>, path: &[u8]) -> Flow {
        let result = self.finish_fs(r, path);
        self.t_ok(result)
    }

    fn finish_fs(&mut self, r: Result<(), Code>, path: &[u8]) -> Result<(), Fail> {
        match r {
            Ok(()) => {
                self.last_exit = 0;
                Ok(())
            }
            Err(c) => {
                self.last_exit = 1;
                let p = core::str::from_utf8(path).unwrap_or("?");
                let msg = match c {
                    Code::NotFound => "No such file or directory",
                    Code::NoSpace => "No space left on device",
                    Code::IsDirectory => "Is a directory",
                    Code::Permission => "Permission denied",
                    _ => "error",
                };
                self.say(Chan::Stderr, |s| write!(s, "cat: {p}: {msg}\n"))
            }
        }
    }

    pub(super) fn complete_test(&mut self, code: u8, aux: u64, o: Outcome<'_>) -> Result<Option<(Pending, HostReq)>, CompleteError> {
        let fail = CompleteError::Fail;
        if let Outcome::Denied = o {
            self.last_exit = 126;
            self.fs_test_path = KBytes::new_in(&self.scratch);
            self.write_out(Chan::Stderr, b"denied\n").map_err(fail)?;
            return Ok(None);
        }
        self.last_exit = 0;
        let finished = match (code, o) {
            (T_HOSTFS, Outcome::Fs(r)) => {
                let t = match r.at(0) {
                    Some(Ok(_)) => "ok",
                    Some(Err((code, _))) => code.name(),
                    None => "io",
                };
                self.say(Chan::Stdout, |s| write!(s, "{t}\n")).map_err(fail)
            }
            (T_CAT | T_WALK, Outcome::Fs(r)) if r.len() == 1 => {
                match (code, r.at(0)) {
                    (T_CAT, Some(Ok(BatchOk::Data(d)))) if d.len() <= FS_CHUNK as usize => {
                        self.write_out(Chan::Stdout, d).map_err(fail)?;
                        if d.len() == FS_CHUNK as usize {
                            // Charge the transferred chunk; the resume gate yields
                            // when the fuel slice is spent instead of chaining
                            // unlimited 1 MiB reads at fuel zero.
                            let mut charged = d.len();
                            let _ = self.pay_bytes(&mut charged);
                            let max_failed = |e| fail(Fail::Mem(e));
                            let pos = aux + u64::from(FS_CHUNK);
                            let path = self.normalized_test_path(&self.fs_test_path).map_err(|_| CompleteError::BadOutcome)?;
                            let (id, rest) = self.shell.root.resolve_host_mount(&path).ok_or(CompleteError::BadOutcome)?;
                            let mut ops = KVec::new_in(&self.scratch);
                            ops.try_push(BatchOp::ReadFile { path: bytes_in(rest, &self.scratch).map_err(max_failed)?, offset: pos, len: FS_CHUNK }).map_err(max_failed)?;
                            let req = self.host_fs_batch(id, ops).map_err(|_| CompleteError::BadOutcome)?;
                            return Ok(Some((Pending::Test(T_CAT, pos), req)));
                        }
                        Ok(())
                    }
                    (T_WALK, Some(Ok(BatchOk::EntriesPlus(e)))) => {
                        self.say(Chan::Stdout, |s| writeln!(s, "{}", e.len())).map_err(fail)
                    }
                    (_, Some(Err((code, _)))) => {
                        let path = core::mem::replace(&mut self.fs_test_path, KBytes::new_in(&self.scratch));
                        self.finish_fs(Err(code), &path).map_err(fail)
                    }
                    _ => Err(CompleteError::BadOutcome),
                }
            }
            (T_WRITE, Outcome::Fs(r)) if r.len() == 1 => {
                match r.at(0) {
                    Some(Ok(BatchOk::None)) => {}
                    Some(Err((code, _))) => {
                        let path = core::mem::replace(&mut self.fs_test_path, KBytes::new_in(&self.scratch));
                        self.fs_write_data = None;
                        return self.finish_fs(Err(code), &path).map(|()| None).map_err(fail);
                    }
                    _ => return Err(CompleteError::BadOutcome),
                }
                // One chunk per host call: append the stashed remainder until done.
                // `aux` is the start of the just-completed chunk; the next
                // chunk starts where it ended.
                let total = self.fs_write_data.as_ref().map(|d| d.len()).unwrap_or(0);
                let start = usize::try_from(aux).map_err(|_| CompleteError::BadOutcome)?.min(total);
                let end = total.min(start.saturating_add(FS_CHUNK as usize));
                if end < total {
                    let next = total.min(end.saturating_add(FS_CHUNK as usize));
                    let mut charged = next - end;
                    let _ = self.pay_bytes(&mut charged);
                    let max_failed = |e| fail(Fail::Mem(e));
                    let path = self.normalized_test_path(&self.fs_test_path).map_err(|_| CompleteError::BadOutcome)?;
                    let (id, rest) = self.shell.root.resolve_host_mount(&path).ok_or(CompleteError::BadOutcome)?;
                    let data = self.fs_write_data.as_ref().ok_or(CompleteError::BadOutcome)?;
                    let mut ops = KVec::new_in(&self.scratch);
                    ops.try_push(BatchOp::WriteFile {
                        path: bytes_in(rest, &self.scratch).map_err(max_failed)?,
                        data: bytes_in(&data.as_slice()[end..next], &self.scratch).map_err(max_failed)?,
                        offset: None, truncate: false, mode: None, mtime_ms: None,
                    }).map_err(max_failed)?;
                    let req = self.host_fs_batch(id, ops).map_err(|_| CompleteError::BadOutcome)?;
                    return Ok(Some((Pending::Test(T_WRITE, end as u64), req)));
                }
                self.fs_write_data = None;
                Ok(())
            }
            (T_FSBATCH, Outcome::Fs(r)) if r.len() == 6 => {
                for i in 0..r.len() {
                    match r.at(i).ok_or(CompleteError::BadOutcome)? {
                        Ok(ok) => self.say(Chan::Stdout, |s| writeln!(s, "ok {}", ok.name())),
                        Err((code, _)) => self.say(Chan::Stdout, |s| writeln!(s, "err {}", code.name())),
                    }.map_err(fail)?;
                }
                Ok(())
            }
            (T_LAZY, Outcome::Completed) => {
                let body = core::mem::replace(&mut self.body, KChunks::new_in(&self.live));
                let r = self.write_chunks(&body);
                self.body = body;
                r.map_err(fail)
            }
            (T_FETCH, Outcome::Fetched(status)) => {
                self.say(Chan::Stdout, |s| write!(s, "{status}\n")).map_err(fail)?;
                let body = core::mem::replace(&mut self.body, KChunks::new_in(&self.live));
                let r = self.write_chunks(&body);
                self.body = body;
                r.map_err(fail)
            }
            (T_FETCH, Outcome::NetFailed(c)) => {
                self.last_exit = 1;
                self.say(Chan::Stderr, |s| write!(s, "fetch: {c}\n")).map_err(fail)
            }
            (T_SLEEP, Outcome::Completed) => Ok(()),
            (T_CLOCK, Outcome::Clock { wall_ms, .. }) => self.say(Chan::Stdout, |s| write!(s, "{wall_ms}\n")).map_err(fail),
            (T_ENTROPY, Outcome::Data(d)) => {
                let n = d.len();
                self.say(Chan::Stdout, |s| write!(s, "{n}\n")).map_err(fail)
            }
            (_, Outcome::Failed(m)) => {
                self.last_exit = 1;
                self.write_out(Chan::Stderr, m).map_err(fail)
            }
            _ => Err(CompleteError::BadOutcome),
        };
        finished?;
        self.fs_test_path = KBytes::new_in(&self.scratch);
        Ok(None)
    }
}

fn push_usize_prefix(out: &mut KBytes, n: usize) -> MemResult<()> {
    let mut buf = [0u8; 24];
    let mut i = buf.len();
    let mut v = n;
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.try_extend_from_slice(&buf[i..])?;
    out.try_push(b':')
}
