//! The `fiber` resource: one exec, run by the bootstrap interpreter (spec section 5) until L2 replaces it.
//! A fiber is a resumable state machine: `run` executes commands until the fuel is out, a host call is
//! needed or the script ends; `resume` answers a host call.
use crate::limits::{self, I_INPUT_BYTES, I_LIVE_BYTES, I_OUTPUT_SIZE, I_SOURCE_BYTES};
use crate::mem::{Budget, KBytes, KRc, KString, KVec, LimitId, MemError, MemResult, TryClone};
use crate::shell::{Shell, G_CUSTOM};
use crate::util::{bytes_in, fmt_in, str_in, Env};
use core::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Completed,
    LimitExceeded,
    Aborted,
    Deadline,
}

pub struct ExecResult {
    pub exit: i32,
    pub status: Status,
    pub limit_name: Option<KString>,
    pub env: Env,
}

impl TryClone for ExecResult {
    fn try_clone(&self) -> MemResult<Self> {
        Ok(ExecResult { exit: self.exit, status: self.status, limit_name: self.limit_name.try_clone()?, env: self.env.try_clone()? })
    }
}

/// A host request, owned (it leaves the kernel through the adapter).
pub enum HostReq {
    FsStat { mount_id: u32, path: KBytes },
    LazyFile(KBytes),
    Custom { name: KBytes, args: KVec<KBytes>, cwd: KBytes, env: Env, input_len: u64 },
    Fetch { url: KBytes },
    Sleep(u64),
    Clock,
    Entropy(u32),
}

pub enum Step {
    Done(ExecResult),
    OutOfFuel,
    Host { epoch: u64, req: HostReq },
}

/// The driver's answer to a host call, borrowed from the adapter's decoded value.
pub enum Outcome<'a> {
    /// First result of an `fs-batch`: `None` for success, else the error code name.
    Fs(Option<&'static str>),
    Data(&'a [u8]),
    CommandExit(i32),
    /// Status of a fetch response.
    Fetched(u16),
    Clock(i64),
    Completed,
    Denied,
    LimitExceeded(&'a str),
    NetFailed(&'static str),
    Failed(&'a [u8]),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CancelReason {
    Aborted,
    Deadline,
    Limit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chan {
    Stdout,
    Stderr,
    Body,
}

/// Errors of the stateful calls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallError {
    StaleEpoch,
    BadState,
    Denied,
}

/// Why an exec ends with a limit result.
pub(crate) enum Fail {
    Output,
    Input,
    Mem(MemError),
    Named(KString),
}

#[derive(Clone, Copy)]
pub(crate) enum Pending {
    Custom,
    /// A test-only command waiting for its answer: command code and one value.
    #[allow(dead_code)]
    Test(u8, u64),
}

#[allow(dead_code)]
pub(crate) enum Flow {
    Next,
    Spin,
    Exit,
    Host(Pending, HostReq),
    Fail(Fail),
}

enum St {
    Ready,
    Spin,
    Pending(Pending),
    Done,
}

/// A drainable byte queue charged to the output budget.
struct Out {
    buf: KBytes,
    head: usize,
}

impl Out {
    fn new(b: &Budget) -> Out {
        Out { buf: KBytes::new_in(b), head: 0 }
    }
    fn chunk(&self, max: usize) -> &[u8] {
        let end = self.buf.len().min(self.head.saturating_add(max));
        &self.buf[self.head..end]
    }
    fn consume(&mut self, n: usize) {
        self.head = (self.head + n).min(self.buf.len());
        if self.head == self.buf.len() {
            self.buf.clear();
            self.buf.shrink_to_fit();
            self.head = 0;
        }
    }
}

pub struct Fiber {
    pub(crate) shell: KRc<Shell>,
    pub(crate) live: Budget,
    pub(crate) scratch: Budget,
    out_budget: Budget,
    out_cap: u64,
    out_total: u64,
    in_cap: u64,
    stdout: Out,
    stderr: Out,
    script: KBytes,
    pos: usize,
    checked: bool,
    overflow: Option<(u64, u64)>,
    pub(crate) stdin: KBytes,
    pub(crate) stdin_pos: usize,
    stdin_total: u64,
    stdin_fail: Option<Fail>,
    pub(crate) env: Env,
    pub(crate) cwd: KBytes,
    pub(crate) last_exit: i32,
    st: St,
    epoch: u64,
    pub(crate) body: KBytes,
    call_env: Option<Env>,
    fuel: u64,
    result: Option<ExecResult>,
    pub(crate) kept: KVec<KBytes>,
    bypass: Option<KString>,
}

/// What `spawn` needs from the caller, borrowed.
pub struct Opts<'a> {
    pub script: &'a [u8],
    pub overflow: Option<(u64, u64)>,
    pub env: &'a [(&'a [u8], &'a [u8], bool)],
    pub replace_env: bool,
    pub cwd: Option<&'a [u8]>,
    pub bypass_custom: Option<&'a str>,
}

impl Fiber {
    /// Creates a fiber on `shell`. `parent_live` is the live budget of the parent fiber, if any.
    pub fn spawn(shell: KRc<Shell>, parent_live: Option<&Budget>, o: &Opts<'_>) -> MemResult<Fiber> {
        let live_cap = shell.lim.vals[I_LIVE_BYTES];
        let live_id = limits::id_of_index(I_LIVE_BYTES);
        let live = match parent_live {
            Some(p) => p.child(live_id, live_cap)?,
            None => shell.live.child(live_id, live_cap)?,
        };
        let out_cap = shell.lim.vals[I_OUTPUT_SIZE];
        // A little slack above the cap holds the diagnostic of the limit itself.
        let out_budget = Budget::root(limits::id_of_index(I_OUTPUT_SIZE), out_cap.saturating_add(4096))?;
        let scratch = Budget::root(LimitId::HEADROOM, u64::MAX)?;
        let mut env = Env::new_in(&live);
        let mut early: Option<Fail> = None;
        if !o.replace_env {
            for e in shell.env.v.iter() {
                if let Err(x) = env.set(&e.name, &e.value, e.exported) {
                    early.get_or_insert(Fail::Mem(x));
                }
            }
        }
        for (n, v, x) in o.env {
            if let Err(e) = env.set(n, v, *x) {
                early.get_or_insert(Fail::Mem(e));
            }
        }
        let cwd = match bytes_in(o.cwd.unwrap_or(shell.cwd.as_slice()), &live) {
            Ok(c) => c,
            Err(e) => {
                early.get_or_insert(Fail::Mem(e));
                KBytes::new_in(&live)
            }
        };
        let script = match bytes_in(o.script, &live) {
            Ok(s) => s,
            Err(e) => {
                early.get_or_insert(Fail::Mem(e));
                KBytes::new_in(&live)
            }
        };
        let bypass = match o.bypass_custom {
            Some(b) => str_in(b, &scratch).ok(),
            None => None,
        };
        Ok(Fiber {
            in_cap: shell.lim.vals[I_INPUT_BYTES],
            stdout: Out::new(&out_budget),
            stderr: Out::new(&out_budget),
            stdin: KBytes::new_in(&live),
            body: KBytes::new_in(&live),
            kept: KVec::new_in(&live),
            shell,
            live,
            scratch,
            out_budget,
            out_cap,
            out_total: 0,
            script,
            pos: 0,
            checked: false,
            overflow: o.overflow,
            stdin_pos: 0,
            stdin_total: 0,
            stdin_fail: early,
            env,
            cwd,
            last_exit: 0,
            st: St::Ready,
            epoch: 0,
            call_env: None,
            fuel: 100_000,
            result: None,
            bypass,
        })
    }

    // ---------------------------------------------------------------- output

    pub(crate) fn out_len_ok(&self, n: u64) -> bool {
        self.out_total.checked_add(n).is_some_and(|t| t <= self.out_cap)
    }

    fn map_out(&self, e: MemError) -> Fail {
        match e {
            MemError::Limit(id) if id == self.out_budget.limit() => Fail::Output,
            e => Fail::Mem(e),
        }
    }

    fn chan(&mut self, c: Chan) -> &mut Out {
        match c {
            Chan::Stderr => &mut self.stderr,
            _ => &mut self.stdout,
        }
    }

    /// Appends to stdout or stderr through the output cap.
    pub(crate) fn write_out(&mut self, c: Chan, s: &[u8]) -> Result<(), Fail> {
        if !self.out_len_ok(s.len() as u64) {
            return Err(Fail::Output);
        }
        match self.chan(c).buf.try_extend_from_slice(s) {
            Ok(()) => {}
            Err(e) => return Err(self.map_out(e)),
        }
        self.out_total += s.len() as u64;
        Ok(())
    }

    /// Appends `n` copies of `byte`.
    #[allow(dead_code)]
    pub(crate) fn fill_out(&mut self, c: Chan, byte: u8, n: u64) -> Result<(), Fail> {
        if !self.out_len_ok(n) {
            return Err(Fail::Output);
        }
        let n_us = usize::try_from(n).map_err(|_| Fail::Output)?;
        let out = self.chan(c);
        let target = out.buf.len() + n_us;
        match out.buf.try_resize(target, byte) {
            Ok(()) => {}
            Err(e) => return Err(self.map_out(e)),
        }
        self.out_total += n;
        Ok(())
    }

    /// Appends formatted text.
    pub(crate) fn say(&mut self, c: Chan, f: impl FnOnce(&mut KString) -> core::fmt::Result) -> Result<(), Fail> {
        let s = fmt_in(&self.scratch, f).map_err(Fail::Mem)?;
        self.write_out(c, s.as_str().as_bytes())
    }

    pub fn stdout_chunk(&self, max: usize) -> &[u8] {
        self.stdout.chunk(max)
    }
    pub fn stdout_consume(&mut self, n: usize) {
        self.stdout.consume(n)
    }
    pub fn stderr_chunk(&self, max: usize) -> &[u8] {
        self.stderr.chunk(max)
    }
    pub fn stderr_consume(&mut self, n: usize) {
        self.stderr.consume(n)
    }

    // ---------------------------------------------------------------- stateful calls

    pub fn write_stdin(&mut self, chunk: &[u8], _last: bool) -> Result<(), CallError> {
        if matches!(self.st, St::Done) {
            return Err(CallError::BadState);
        }
        self.stdin_total = self.stdin_total.saturating_add(chunk.len() as u64);
        if self.stdin_fail.is_some() {
            return Ok(());
        }
        if self.stdin_total > self.in_cap {
            self.stdin_fail = Some(Fail::Input);
            self.stdin.clear();
            self.stdin.shrink_to_fit();
        } else if let Err(e) = self.stdin.try_extend_from_slice(chunk) {
            self.stdin_fail = Some(Fail::Mem(e));
        }
        Ok(())
    }

    fn check_call(&self, epoch: u64) -> Result<Pending, CallError> {
        match self.st {
            St::Done => Err(CallError::BadState),
            St::Pending(p) if epoch == self.epoch => Ok(p),
            _ => Err(CallError::StaleEpoch),
        }
    }

    pub fn read_call_input(&mut self, epoch: u64, fd: u32, max: usize) -> Result<&[u8], CallError> {
        self.check_call(epoch)?;
        if fd != 0 {
            return Ok(&[]);
        }
        let start = self.stdin_pos.min(self.stdin.len());
        let end = self.stdin.len().min(start.saturating_add(max));
        self.stdin_pos = end;
        Ok(&self.stdin[start..end])
    }

    pub fn push_call_data(&mut self, epoch: u64, c: Chan, chunk: &[u8]) -> Result<(), CallError> {
        self.check_call(epoch)?;
        let r = match c {
            Chan::Body => self.body.try_extend_from_slice(chunk).map_err(Fail::Mem),
            c => self.write_out(c, chunk),
        };
        if let Err(f) = r {
            // The limit is reported when the call is resumed.
            self.stdin_fail.get_or_insert(f);
        }
        Ok(())
    }

    pub fn assign_variable(&mut self, epoch: u64, name: &[u8], value: &[u8], subscript: Option<&[u8]>) -> Result<(), CallError> {
        self.check_call(epoch)?;
        if subscript.is_some() || name.is_empty() || name.contains(&b'=') {
            return Err(CallError::Denied);
        }
        if let Err(e) = self.env.set(name, value, false) {
            self.stdin_fail.get_or_insert(Fail::Mem(e));
        }
        Ok(())
    }

    pub fn set_call_env(&mut self, epoch: u64, env: &[(&[u8], &[u8], bool)]) -> Result<(), CallError> {
        self.check_call(epoch)?;
        let mut e = Env::new_in(&self.live);
        for (n, v, x) in env {
            if let Err(m) = e.set(n, v, *x) {
                self.stdin_fail.get_or_insert(Fail::Mem(m));
                return Ok(());
            }
        }
        self.call_env = Some(e);
        Ok(())
    }

    pub fn live_budget(&self) -> &Budget {
        &self.live
    }

    // ---------------------------------------------------------------- running

    pub fn run(&mut self, fuel: u64) -> Step {
        self.fuel = fuel;
        self.advance(fuel)
    }

    pub fn resume(&mut self, epoch: u64, o: Outcome<'_>) -> Result<Step, CallError> {
        let p = self.check_call(epoch)?;
        self.st = St::Ready;
        match self.complete(p, o) {
            Ok(()) => {}
            Err(CompleteError::BadOutcome) => {
                self.st = St::Pending(p);
                return Err(CallError::BadState);
            }
            Err(CompleteError::Fail(f)) => return Ok(self.end_fail(f)),
        }
        self.body.clear();
        self.body.shrink_to_fit();
        if let Some(e) = self.call_env.take() {
            self.env = e;
        }
        let fuel = self.fuel;
        Ok(self.advance(fuel))
    }

    fn done_step(&self) -> Step {
        match &self.result {
            Some(r) => match r.try_clone() {
                Ok(c) => Step::Done(c),
                Err(_) => Step::Done(ExecResult { exit: r.exit, status: r.status, limit_name: None, env: Env::new_in(&self.scratch) }),
            },
            None => Step::OutOfFuel,
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self.st, St::Done)
    }

    fn advance(&mut self, mut fuel: u64) -> Step {
        loop {
            match self.st {
                St::Done => return self.done_step(),
                St::Pending(_) | St::Spin => return Step::OutOfFuel,
                St::Ready => {}
            }
            if !self.checked {
                self.checked = true;
                if let Some(step) = self.precheck() {
                    return step;
                }
            }
            if let Some(f) = self.stdin_fail.take() {
                return self.end_fail(f);
            }
            let Some((a, b)) = self.next_line() else {
                let exit = self.last_exit;
                self.finish(exit, Status::Completed, None);
                return self.done_step();
            };
            if fuel == 0 {
                // Not started: put the line back.
                self.pos = a;
                return Step::OutOfFuel;
            }
            fuel -= 1;
            let script = core::mem::replace(&mut self.script, KBytes::new_in(&self.scratch));
            let flow = self.exec_line(&script[a..b]);
            self.script = script;
            match flow {
                Flow::Next => {}
                Flow::Spin => {
                    self.st = St::Spin;
                    return Step::OutOfFuel;
                }
                Flow::Exit => {
                    let exit = self.last_exit;
                    self.finish(exit, Status::Completed, None);
                    return self.done_step();
                }
                Flow::Fail(f) => return self.end_fail(f),
                Flow::Host(p, req) => {
                    self.epoch += 1;
                    self.st = St::Pending(p);
                    self.body.clear();
                    self.call_env = None;
                    return Step::Host { epoch: self.epoch, req };
                }
            }
        }
    }

    /// Next non-empty trimmed line as a range of the script.
    fn next_line(&mut self) -> Option<(usize, usize)> {
        while self.pos < self.script.len() {
            let start = self.pos;
            let rest = &self.script[start..];
            let len = rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len());
            self.pos = start + len + 1;
            let line = &rest[..len];
            let a = line.iter().position(|&c| c != b' ' && c != b'\t');
            if let Some(a) = a {
                let b = line.iter().rposition(|&c| c != b' ' && c != b'\t').map_or(a, |x| x + 1);
                return Some((start + a, start + b));
            }
        }
        None
    }

    fn exec_line(&mut self, line: &[u8]) -> Flow {
        let mut words: KVec<&[u8]> = KVec::new_in(&self.scratch);
        for w in line.split(|&c| c == b' ' || c == b'\t').filter(|w| !w.is_empty()) {
            if let Err(e) = words.try_push(w) {
                return Flow::Fail(Fail::Mem(e));
            }
        }
        let Some((name, args)) = words.split_first() else { return Flow::Next };
        if self.shell.is_custom(name) && self.bypass.as_ref().map(|b| b.as_str().as_bytes()) != Some(*name) {
            return self.custom_call(name, args);
        }
        if let Some(f) = self.exec_test(name, args) {
            return f;
        }
        self.last_exit = 127;
        let n = *name;
        match self.say(Chan::Stderr, |s| {
            s.try_push_str("bash: ").map_err(|_| core::fmt::Error)?;
            for ch in n.utf8_chunks() {
                s.try_push_str(ch.valid()).map_err(|_| core::fmt::Error)?;
                if !ch.invalid().is_empty() {
                    s.try_push('\u{FFFD}').map_err(|_| core::fmt::Error)?;
                }
            }
            s.try_push_str(": command not found\n").map_err(|_| core::fmt::Error)
        }) {
            Ok(()) => Flow::Next,
            Err(f) => Flow::Fail(f),
        }
    }

    fn custom_call(&mut self, name: &[u8], args: &[&[u8]]) -> Flow {
        if !self.shell.granted(G_CUSTOM) {
            self.last_exit = 126;
            let _ = self.write_out(Chan::Stderr, b"bash: permission denied\n");
            return Flow::Next;
        }
        let r = (|| -> MemResult<HostReq> {
            let mut a = KVec::new_in(&self.scratch);
            for w in args {
                a.try_push(bytes_in(w, &self.scratch)?)?;
            }
            Ok(HostReq::Custom {
                name: bytes_in(name, &self.scratch)?,
                args: a,
                cwd: bytes_in(&self.cwd, &self.scratch)?,
                env: self.env.try_clone()?,
                input_len: (self.stdin.len() - self.stdin_pos.min(self.stdin.len())) as u64,
            })
        })();
        match r {
            Ok(req) => Flow::Host(Pending::Custom, req),
            Err(e) => Flow::Fail(Fail::Mem(e)),
        }
    }

    // ---------------------------------------------------------------- answers

    fn complete(&mut self, p: Pending, o: Outcome<'_>) -> Result<(), CompleteError> {
        if let Some(f) = self.stdin_fail.take() {
            return Err(CompleteError::Fail(f));
        }
        match p {
            Pending::Custom => match o {
                Outcome::CommandExit(n) => self.last_exit = n,
                Outcome::Denied => {
                    self.last_exit = 126;
                    self.write_out(Chan::Stderr, b"bash: permission denied\n").map_err(CompleteError::Fail)?;
                }
                Outcome::Failed(msg) => {
                    self.last_exit = 1;
                    self.write_out(Chan::Stderr, msg).map_err(CompleteError::Fail)?;
                }
                Outcome::LimitExceeded(name) => return Err(CompleteError::Fail(Fail::Named(str_in(name, &self.scratch).map_err(|e| CompleteError::Fail(Fail::Mem(e)))?))),
                _ => return Err(CompleteError::BadOutcome),
            },
            Pending::Test(code, aux) => self.complete_test(code, aux, o)?,
        }
        Ok(())
    }

    // ---------------------------------------------------------------- ending

    fn precheck(&mut self) -> Option<Step> {
        let cap = self.shell.lim.vals[I_SOURCE_BYTES];
        let (units, bytes) = match self.overflow {
            Some(x) => x,
            None => {
                let b = self.script.len() as u64;
                let s = self.script.as_slice();
                let u = s.iter().filter(|&&c| c & 0xC0 != 0x80).count() as u64 + s.iter().filter(|&&c| c >= 0xF0).count() as u64;
                (u, b)
            }
        };
        if self.overflow.is_some() || bytes > cap || units > cap {
            let _ = self.say(Chan::Stderr, |s| write!(s, "bash: script input size limit exceeded ({cap} bytes)\n"));
            self.finish(126, Status::LimitExceeded, Some("maxSourceBytes"));
            return Some(self.done_step());
        }
        if units > 1_000_000 {
            let _ = self.say(Chan::Stderr, |s| write!(s, "bash: syntax error: Parse error at 1:1: Input too large: {units} bytes exceeds limit of 1000000\n"));
            self.finish(2, Status::Completed, None);
            return Some(self.done_step());
        }
        None
    }

    fn end_fail(&mut self, f: Fail) -> Step {
        let name: &str;
        let mut dynamic: Option<KString> = None;
        match f {
            Fail::Output => {
                name = "maxOutputSize";
                let cap = self.out_cap;
                let _ = self.say_unchecked(|s| write!(s, "bash: pipeline: total output size exceeded (>{cap} bytes), increase executionLimits.maxOutputSize\n"));
            }
            Fail::Input => {
                name = "maxInputBytes";
                let _ = self.say_unchecked(|s| s.try_push_str("bash: maxInputBytes exceeded\n").map_err(|_| core::fmt::Error));
            }
            Fail::Mem(e) => {
                name = match e {
                    MemError::Limit(id) => limits::name_of(id),
                    _ => "memory-ceiling",
                };
                let _ = self.say_unchecked(|s| write!(s, "bash: {name} exceeded\n"));
            }
            Fail::Named(n) => {
                let _ = self.say_unchecked(|s| write!(s, "bash: {} exceeded\n", n.as_str()));
                name = "";
                dynamic = Some(n);
            }
        }
        match dynamic {
            Some(n) => self.finish_named(126, Status::LimitExceeded, Some(n)),
            None => self.finish(126, Status::LimitExceeded, Some(name)),
        }
        self.done_step()
    }

    /// Formats and appends without the output cap (diagnostics of a limit that was just hit).
    fn say_unchecked(&mut self, f: impl FnOnce(&mut KString) -> core::fmt::Result) -> MemResult<()> {
        let s = fmt_in(&self.scratch, f)?;
        self.stderr.buf.try_extend_from_slice(s.as_str().as_bytes())
    }

    fn finish(&mut self, exit: i32, status: Status, limit: Option<&str>) {
        let name = limit.and_then(|n| str_in(n, &self.scratch).ok());
        self.finish_named(exit, status, name);
    }

    fn finish_named(&mut self, exit: i32, status: Status, limit_name: Option<KString>) {
        self.last_exit = exit;
        let mut env = match self.env.try_clone() {
            Ok(e) => e,
            Err(_) => Env::new_in(&self.scratch),
        };
        let mut code = KString::new_in(&self.scratch);
        let _ = write!(code, "{exit}");
        let _ = env.set(b"?", code.as_str().as_bytes(), false);
        self.result = Some(ExecResult { exit, status, limit_name, env });
        self.st = St::Done;
        // Release what the exec held, so that the instance stays usable.
        self.script = KBytes::new_in(&self.scratch);
        self.stdin = KBytes::new_in(&self.scratch);
        self.body = KBytes::new_in(&self.scratch);
        self.kept = KVec::new_in(&self.scratch);
        self.call_env = None;
        self.stdin_fail = None;
    }

    pub fn cancel(&mut self, reason: CancelReason) -> ExecResult {
        if !matches!(self.st, St::Done) {
            self.cancel_inner(reason);
        }
        match self.done_step() {
            Step::Done(r) => r,
            _ => ExecResult { exit: 1, status: Status::Completed, limit_name: None, env: Env::new_in(&self.scratch) },
        }
    }

    fn cancel_inner(&mut self, reason: CancelReason) {
        let (status, exit, name) = match reason {
            CancelReason::Aborted => (Status::Aborted, 124, None),
            CancelReason::Deadline => (Status::Deadline, 124, None),
            CancelReason::Limit => (Status::LimitExceeded, 126, Some("limit")),
        };
        let _ = self.say_unchecked(|s| s.try_push_str("bash: execution aborted\n").map_err(|_| core::fmt::Error));
        self.finish(exit, status, name);
    }
}

pub(crate) enum CompleteError {
    BadOutcome,
    Fail(Fail),
}

#[cfg(feature = "test-commands")]
mod testcmd;

#[cfg(not(feature = "test-commands"))]
impl Fiber {
    fn exec_test(&mut self, _name: &[u8], _args: &[&[u8]]) -> Option<Flow> {
        None
    }
    fn complete_test(&mut self, _code: u8, _aux: u64, _o: Outcome<'_>) -> Result<(), CompleteError> {
        Err(CompleteError::BadOutcome)
    }
}

#[cfg(test)]
mod tests;
