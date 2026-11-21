//! One exec: a resumable VM, its pipe streams and the epoch-checked resource boundary.
use crate::cmd::{CmdCtx, CmdFrame, CmdPoll};
use crate::limits::{
    self, I_ARRAY_ELEMENTS, I_CALL_DEPTH, I_COMMAND_COUNT, I_EXEC_DEPTH, I_FS_BYTES, I_INPUT_BYTES,
    I_LIVE_BYTES, I_OUTPUT_SIZE, I_SOURCE_BYTES, I_STRING_LENGTH, I_WORK_UNITS,
};
use crate::mem::{
    reserve_scope, Budget, KBox, KBytes, KChunks, KRc, KString, KVec, LimitId, MemError, MemResult,
    TryClone,
};
use crate::shell::{Shell, G_CUSTOM, G_HOST_FS};
use crate::util::{bytes_in, fmt_in, str_in, Env, EnvValue};
use core::{cell::Cell, fmt::Write};

mod expand;
mod syntax;
mod vm;
pub(crate) use expand::BYTE_CHUNK;
use syntax::{utf16_len, Ast};
use vm::Vm;

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
    pub env: KRc<Env>,
    pub stdout_bytes: bool,
}

impl TryClone for ExecResult {
    fn try_clone(&self) -> MemResult<Self> {
        Ok(ExecResult {
            exit: self.exit,
            status: self.status,
            limit_name: self.limit_name.try_clone()?,
            env: self.env.try_clone()?,
            stdout_bytes: self.stdout_bytes,
        })
    }
}

/// A host request, owned (it leaves the kernel through the adapter).
pub enum HostReq {
    FsStat {
        mount_id: u32,
        path: KBytes,
    },
    LazyFile(KBytes),
    Custom {
        name: KBytes,
        args: KVec<KBytes>,
        cwd: KBytes,
        env: KVec<(KRc<KBytes>, EnvValue)>,
        input_len: u64,
    },
    Fetch {
        url: KBytes,
    },
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
    StdoutBytes,
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
/// Terminal limits and recoverable expansion errors.
#[derive(Debug)]
pub enum Fail {
    Output,
    Input,
    StringLength,
    Elements,
    Arguments,
    CommandCount,
    CallDepth(KBytes),
    ExecDepth,
    Work,
    BadSubstitution(KBytes),
    Readonly(KBytes),
    Mem(MemError),
}

impl From<MemError> for Fail {
    fn from(error: MemError) -> Self {
        Self::Mem(error)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Pending {
    Custom,
    Kernel,
    #[allow(dead_code)]
    Test(u8, u64),
}

#[allow(dead_code)]
pub(crate) enum Flow {
    Next,
    Spin,
    Yield,
    Exit,
    Host(Pending, HostReq),
    Fail(Fail),
}

enum State {
    Ready,
    Spin,
    Pending(Pending),
    Done,
}

/// Small formatting buffer for fixed diagnostics and decimal values.
struct Diagnostic {
    bytes: [u8; 256],
    len: usize,
}

impl Diagnostic {
    fn new() -> Self {
        Self {
            bytes: [0; 256],
            len: 0,
        }
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for Diagnostic {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        self.bytes
            .get_mut(self.len..end)
            .ok_or(core::fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

struct Program {
    source: KBytes,
    ast: Ast,
}

pub struct Fiber {
    pub(crate) shell: KRc<Shell>,
    pub(crate) live: Budget,
    pub(crate) scratch: Budget,
    out_budget: Budget,
    out_cap: u64,
    out_total: u64,
    in_cap: u64,
    stdout: KChunks,
    stdout_bytes: bool,
    stderr: KChunks,
    stderr_tail: KBytes,
    script: Option<KBytes>,
    program: Option<Program>,
    vm: Vm,
    pub(crate) active: Option<KBox<dyn CmdFrame>>,
    overflow: Option<(u64, u64)>,
    raw_script: bool,
    pub(crate) stdin: KChunks,
    input_window: KBytes,
    stdin_total: u64,
    stdin_error: Option<MemError>,
    failed: Option<Fail>,
    pub(crate) env: Env,
    overlay: Env,
    pub(crate) cwd: KBytes,
    pub(crate) last_exit: i32,
    last_arg: Option<KRc<KBytes>>,
    params: KVec<KBytes>,
    args: KVec<KBytes>,
    epoch: u64,
    state: State,
    pub(crate) body: KChunks,
    call_env: Option<Env>,
    fuel: Cell<u64>,
    fuel_credit: Cell<u8>,
    exec_depth: u64,
    work: KRc<Cell<u64>>,
    result: ExecResult,
    pub(crate) kept: KVec<KBytes>,
    bypass: Option<KString>,
    coverage: KVec<u16>,
}

/// Inputs borrowed while creating a fiber.
#[derive(Default)]
pub struct Opts<'a> {
    pub script: &'a [u8],
    pub overflow: Option<(u64, u64)>,
    pub args: &'a [&'a [u8]],
    pub env: &'a [(&'a [u8], &'a [u8], bool)],
    pub replace_env: bool,
    pub cwd: Option<&'a [u8]>,
    pub raw_script: bool,
    pub bypass_custom: Option<&'a str>,
}

impl Fiber {
    pub fn spawn(shell: KRc<Shell>, parent: Option<&Fiber>, o: &Opts<'_>) -> MemResult<Fiber> {
        let cap = shell.lim.vals[I_LIVE_BYTES];
        let live = parent
            .map_or(&shell.live, |p| &p.live)
            .child(limits::id_of_index(I_LIVE_BYTES), cap)?;
        let out_cap = shell.lim.vals[I_OUTPUT_SIZE];
        let out_budget = Budget::root(limits::id_of_index(I_OUTPUT_SIZE), u64::MAX)?;
        let scratch = Budget::root(LimitId::HEADROOM, u64::MAX)?;
        let result = ExecResult {
            exit: 0,
            status: Status::Completed,
            limit_name: None,
            stdout_bytes: false,
            env: KRc::try_new_in(Env::new_in(&scratch), &scratch).map_err(|(_, e)| e)?,
        };
        let mut overlay = Env::new_in(&live);
        for &(name, value, _) in o.env {
            overlay.set(name, value, syntax::identifier(name))?;
        }
        let exec_depth = parent.map_or(0, |p| p.exec_depth.saturating_add(1));
        let work = match parent {
            Some(p) => p.work.clone(),
            None => KRc::try_new_in(Cell::new(0), &scratch).map_err(|(_, e)| e)?,
        };
        let mut failed = (exec_depth > shell.lim.vals[I_EXEC_DEPTH]).then_some(Fail::ExecDepth);
        let mut env = Env::new_in(&live);
        let mut cwd = KBytes::new_in(&live);
        let mut args = KVec::new_in(&live);
        let mut script = KBytes::new_in(&live);
        if failed.is_none() {
            let prepare = (|| -> MemResult<()> {
                if !o.replace_env {
                    env.copy_from(&shell.env)?;
                }
                env.merge_shared(&overlay)?;
                let opt_cwd = o.cwd.filter(|p| !p.is_empty());
                let resolved = opt_cwd
                    .filter(|_| !shell.granted(G_HOST_FS) && !o.env.is_empty() && overlay.get(b"PWD").is_none())
                    .and_then(|path| shell.root.resolve(&shell.cwd, path).ok())
                    .filter(|path| shell.root.stat_normalized(path).is_ok());
                let path = resolved.as_deref().or(opt_cwd).unwrap_or(&shell.cwd);
                if path.starts_with(b"/") {
                    cwd = bytes_in(path, &live)?;
                } else {
                    cwd.try_reserve_exact(shell.cwd.len() + 1 + path.len())?;
                    cwd.try_extend_from_slice(shell.cwd.strip_suffix(b"/").unwrap_or(&shell.cwd))?;
                    cwd.try_push(b'/')?;
                    cwd.try_extend_from_slice(path)?;
                }
                if opt_cwd.is_some() {
                    if let Some(pwd) = overlay.get(b"PWD") {
                        let mut pwd = pwd.clone();
                        pwd.exported = true;
                        env.replace_shared(b"PWD", pwd)?;
                    } else {
                        env.set(b"PWD", &cwd, true)?;
                    }
                }
                args.try_reserve_exact(o.args.len())?;
                for arg in o.args {
                    args.try_push(bytes_in(arg, &live)?)?;
                }
                if o.overflow.is_none() && o.script.len() as u64 <= shell.lim.vals[I_SOURCE_BYTES] {
                    script = bytes_in(o.script, &live)?;
                }
                Ok(())
            })();
            if let Err(e) = prepare {
                failed = Some(Fail::Mem(e));
            }
        }
        let overflow = o.overflow.or_else(|| {
            (o.script.len() as u64 > shell.lim.vals[I_SOURCE_BYTES])
                .then(|| (utf16_len(o.script), o.script.len() as u64))
        });
        let bypass = o.bypass_custom.map(|b| str_in(b, &scratch)).transpose()?;
        Ok(Self {
            in_cap: shell.lim.vals[I_INPUT_BYTES].min(shell.lim.vals[I_STRING_LENGTH]),
            stdout: KChunks::new_in(&out_budget),
            stderr: KChunks::new_in(&out_budget),
            stderr_tail: KBytes::new_in(&scratch),
            stdin: KChunks::new_in(&live),
            input_window: KBytes::new_in(&live),
            body: KChunks::new_in(&live),
            kept: KVec::new_in(&live),
            vm: Vm::new(&live),
            params: KVec::new_in(&live),
            last_arg: None,
            coverage: KVec::new_in(&live),
            shell,
            live,
            scratch,
            out_budget,
            out_cap,
            overflow,
            env,
            overlay,
            cwd,
            bypass,
            args,
            raw_script: o.raw_script,
            script: Some(script),
            program: None,
            active: None,
            failed,
            stdout_bytes: false,
            stdin_total: 0,
            stdin_error: None,
            out_total: 0,
            last_exit: 0,
            epoch: 0,
            state: State::Ready,
            call_env: None,
            fuel: Cell::new(0),
            fuel_credit: Cell::new(0),
            exec_depth,
            work,
            result,
        })
    }

    pub(crate) fn charge_work(&self, units: u64) -> Result<(), Fail> {
        let next = self.work.get().checked_add(units).ok_or(Fail::Work)?;
        if next > self.shell.lim.vals[I_WORK_UNITS] {
            return Err(Fail::Work);
        }
        self.work.set(next);
        Ok(())
    }

    pub(crate) fn out_len_ok(&self, n: u64) -> bool {
        self.out_total
            .checked_add(n)
            .is_some_and(|total| total <= self.out_cap)
    }

    fn target(&mut self, channel: Chan) -> (&mut KChunks, bool) {
        let index = usize::from(channel == Chan::Stderr);
        if let Some(pipe) = self.vm.targets[index] {
            (&mut self.vm.pipes.as_mut_slice()[pipe], true)
        } else if index == 1 {
            (&mut self.stderr, false)
        } else {
            (&mut self.stdout, false)
        }
    }

    pub(crate) fn write_out(&mut self, channel: Chan, data: &[u8]) -> Result<(), Fail> {
        if !self.out_len_ok(data.len() as u64) {
            return Err(Fail::Output);
        }
        let (target, piped) = self.target(channel);
        target.try_append(data)?;
        self.out_total += data.len() as u64;
        self.stdout_bytes |= !piped && channel == Chan::StdoutBytes;
        Ok(())
    }

    #[cfg(feature = "test-commands")]
    pub(crate) fn fill_out(&mut self, channel: Chan, byte: u8, len: u64) -> Result<(), Fail> {
        if !self.out_len_ok(len) {
            return Err(Fail::Output);
        }
        let block = [byte; 4096];
        let mut left = len;
        while left > 0 {
            let n = left.min(block.len() as u64);
            self.write_out(channel, &block[..n as usize])?;
            left -= n;
        }
        Ok(())
    }

    #[cfg(feature = "test-commands")]
    pub(crate) fn say(
        &mut self,
        channel: Chan,
        format: impl FnOnce(&mut KString) -> core::fmt::Result,
    ) -> Result<(), Fail> {
        let text = fmt_in(&self.live, format)?;
        self.write_out(channel, text.as_str().as_bytes())
    }

    #[cfg(test)]
    pub fn stdout_chunk(&self, max: usize) -> &[u8] {
        first_chunk(&self.stdout, max)
    }
    #[cfg(test)]
    pub fn stdout_consume(&mut self, n: usize) {
        self.drain_output(false, n, |_| {});
    }
    #[cfg(test)]
    pub fn stderr_chunk(&self, max: usize) -> &[u8] {
        if self.stderr.is_empty() {
            &self.stderr_tail[..self.stderr_tail.len().min(max)]
        } else {
            first_chunk(&self.stderr, max)
        }
    }
    #[cfg(test)]
    pub fn stderr_consume(&mut self, n: usize) {
        self.drain_output(true, n, |_| {});
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn output_segment_len(&self, stderr: bool) -> usize {
        if stderr && !self.stderr.is_empty() {
            self.stderr.len() as usize
        } else {
            self.output_len(stderr)
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn output_len(&self, stderr: bool) -> usize {
        if stderr {
            self.stderr.len() as usize + self.stderr_tail.len()
        } else {
            self.stdout.len() as usize
        }
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn drain_output(&mut self, stderr: bool, n: usize, mut sink: impl FnMut(&[u8])) {
        if !stderr {
            self.stdout.drain_front(n, sink);
            return;
        }
        let drained = self.stderr.drain_front(n, &mut sink);
        let tail = (n - drained).min(self.stderr_tail.len());
        sink(&self.stderr_tail[..tail]);
        if tail == self.stderr_tail.len() {
            self.stderr_tail = KBytes::new_in(&self.scratch);
        } else if tail != 0 {
            let remain = self.stderr_tail.len() - tail;
            self.stderr_tail.as_mut_slice().copy_within(tail.., 0);
            self.stderr_tail.truncate(remain);
        }
    }

    pub fn write_stdin(&mut self, chunk: &[u8], _last: bool) -> Result<(), CallError> {
        if self.is_done() {
            return Err(CallError::BadState);
        }
        self.stdin_total = self.stdin_total.saturating_add(chunk.len() as u64);
        if self.stdin_total > self.in_cap {
            self.failed.get_or_insert(Fail::Input);
            self.stdin.clear();
        } else if self.failed.is_none() && self.stdin_error.is_none() {
            if let Err(e) = self.stdin.try_append(chunk) {
                // A later chunk can still establish that the whole input exceeds
                // the input cap. Keep this storage error separate from spawn failures.
                self.stdin_error = Some(e);
            }
        }
        Ok(())
    }

    fn take_failure(&mut self) -> Option<Fail> {
        self.failed.take().or_else(|| self.stdin_error.take().map(Fail::Mem))
    }

    fn check_call(&self, epoch: u64) -> Result<Pending, CallError> {
        match self.state {
            State::Done => Err(CallError::BadState),
            State::Pending(p) if epoch == self.epoch => Ok(p),
            _ => Err(CallError::StaleEpoch),
        }
    }

    pub fn read_call_input(&mut self, epoch: u64, fd: u32, max: usize) -> Result<&[u8], CallError> {
        let data = self.call_input_chunk(epoch, fd, max)?;
        self.input_window = bytes_in(data, &self.live).map_err(|_| CallError::Denied)?;
        self.stdin.drain_front(self.input_window.len(), |_| {});
        Ok(&self.input_window)
    }

    pub(crate) fn call_input_chunk(
        &self,
        epoch: u64,
        fd: u32,
        max: usize,
    ) -> Result<&[u8], CallError> {
        self.check_call(epoch)?;
        Ok(if fd == 0 {
            first_chunk(&self.stdin, max)
        } else {
            &[]
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn consume_call_input(&mut self, len: usize) {
        self.stdin.drain_front(len, |_| {});
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn fail_abi_transfer(&mut self) {
        self.end_fail(Fail::Mem(MemError::Ceiling));
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn stdout_bytes(&self) -> bool {
        self.stdout_bytes
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn coverage_hits(&self, max: usize) -> &[u16] {
        &self.coverage[..self.coverage.len().min(max)]
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn consume_coverage(&mut self, n: usize) {
        let remain = self.coverage.len() - n;
        self.coverage.as_mut_slice().copy_within(n.., 0);
        self.coverage.truncate(remain);
    }

    pub fn push_call_data(
        &mut self,
        epoch: u64,
        channel: Chan,
        chunk: &[u8],
    ) -> Result<(), CallError> {
        self.check_call(epoch)?;
        if self.failed.is_none() {
            let written = match channel {
                Chan::Body => self.body.try_append(chunk).map_err(Fail::Mem),
                c => self.write_out(c, chunk),
            };
            if let Err(f) = written {
                self.failed = Some(f);
            }
        }
        Ok(())
    }

    pub fn assign_variable(
        &mut self,
        epoch: u64,
        name: &[u8],
        value: &[u8],
        subscript: Option<&[u8]>,
    ) -> Result<(), CallError> {
        self.check_call(epoch)?;
        if subscript.is_some() || name.is_empty() || name.contains(&b'=') || readonly(name) {
            return Err(CallError::Denied);
        }
        let exported = self.env.get(name).is_some_and(|e| e.exported);
        if let Err(error) = self.env.set(name, value, exported) {
            self.failed.get_or_insert(Fail::Mem(error));
        }
        Ok(())
    }

    pub fn set_call_env(
        &mut self,
        epoch: u64,
        env: &[(&[u8], &[u8], bool)],
    ) -> Result<(), CallError> {
        self.check_call(epoch)?;
        let mut next = Env::new_in(&self.live);
        for &(name, value, exported) in env {
            if let Err(error) = next.set(name, value, exported) {
                self.failed.get_or_insert(Fail::Mem(error));
                return Ok(());
            }
        }
        self.call_env = Some(next);
        Ok(())
    }

    pub fn live_budget(&self) -> &Budget {
        &self.live
    }

    pub fn run(&mut self, fuel: u64) -> Step {
        self.fuel.set(fuel);
        self.fuel_credit.set(0);
        self.advance()
    }

    fn take_fuel(&self) -> bool {
        let Some(left) = self.fuel.get().checked_sub(1) else {
            return false;
        };
        self.fuel.set(left);
        self.fuel_credit.set(16);
        true
    }

    /// A stage pays one unit up front. Longer stages spend another unit for
    /// every 16 small operations, including each 256-byte chunk or field.
    pub(crate) fn tick(&self) -> bool {
        if self.fuel_credit.get() == 0 && !self.take_fuel() {
            return false;
        }
        self.fuel_credit.set(self.fuel_credit.get() - 1);
        true
    }

    fn pay_bytes(&self, remaining: &mut usize) -> bool {
        while *remaining > 0 {
            if !self.tick() {
                return false;
            }
            *remaining = remaining.saturating_sub(BYTE_CHUNK);
        }
        true
    }

    /// KMap uses at most five comparisons per node, with at least eight
    /// keys per non-root node. An insertion can traverse the tree twice.
    fn name_work(len: usize, entries: usize, traversals: usize) -> usize {
        let comparisons = if entries == 0 { 0 } else { 5 * (1 + entries.ilog2() as usize / 3) };
        len.max(1).saturating_mul(1 + traversals * comparisons)
    }

    pub fn resume(&mut self, epoch: u64, outcome: Outcome<'_>) -> Result<Step, CallError> {
        let pending = self.check_call(epoch)?;
        self.state = State::Ready;
        if let Some(fail) = self.take_failure() {
            return Ok(self.end_fail(fail));
        }
        if let Outcome::LimitExceeded(name) = outcome {
            let name = &name[..name.floor_char_boundary(128)];
            self.finish_early(
                126,
                Status::LimitExceeded,
                Some(name),
                Some(format_args!("bash: {name} exceeded\n")),
            );
            return Ok(self.done_step());
        }
        if matches!(pending, Pending::Kernel) {
            return Ok(match self.resume_command(Some(outcome)) {
                Ok(Some(step)) => step,
                Ok(None) => self.advance(),
                Err(fail) => self.end_fail(fail),
            });
        }
        match self.complete(pending, outcome) {
            Ok(()) => {}
            Err(CompleteError::BadOutcome) => {
                self.state = State::Pending(pending);
                return Err(CallError::BadState);
            }
            Err(CompleteError::Fail(f)) => return Ok(self.end_fail(f)),
        }
        self.body.clear();
        self.input_window = KBytes::new_in(&self.live);
        if let Some(env) = self.call_env.take() {
            self.env = env;
        }
        Ok(self.advance())
    }

    fn done_step(&self) -> Step {
        let r = &self.result;
        let result = r
            .try_clone()
            .or_else(|_| reserve_scope(|| r.try_clone()))
            .unwrap_or_else(|_| ExecResult {
                exit: r.exit,
                status: r.status,
                limit_name: reserve_scope(|| r.limit_name.try_clone()).ok().flatten(),
                env: r.env.clone(),
                stdout_bytes: r.stdout_bytes,
            });
        Step::Done(result)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, State::Done)
    }

    fn advance(&mut self) -> Step {
        match self.state {
            State::Done => return self.done_step(),
            State::Pending(_) | State::Spin => return Step::OutOfFuel,
            State::Ready => {}
        }
        if let Some(source) = self.script.take() {
            if let Some(step) = self.prepare(source) {
                return step;
            }
        }
        if let Some(fail) = self.take_failure() {
            return self.end_fail(fail);
        }
        if self.active.is_some() {
            match self.resume_command(None) {
                Ok(Some(step)) => return step,
                Ok(None) => {}
                Err(fail) => return self.end_fail(fail),
            }
        }
        let program = self.program.take().expect("ready fiber has a program");
        match self.run_vm(&program) {
            Ok(Some(step)) => {
                self.program = Some(program);
                step
            }
            Ok(None) => {
                drop(program);
                self.finish(self.last_exit, Status::Completed, None, false, None);
                self.done_step()
            }
            Err(fail) => {
                drop(program);
                self.end_fail(fail)
            }
        }
    }

    fn prepare(&mut self, mut source: KBytes) -> Option<Step> {
        let cap = self.shell.lim.vals[I_SOURCE_BYTES];
        let (units, bytes) = self
            .overflow
            .unwrap_or_else(|| (utf16_len(&source), source.len() as u64));
        if units > cap || bytes > cap {
            drop(source);
            self.finish_early(
                126,
                Status::LimitExceeded,
                Some("maxSourceBytes"),
                Some(format_args!(
                    "bash: script input size limit exceeded ({cap} bytes)\n"
                )),
            );
            return Some(self.done_step());
        }
        if let Some(fail) = self.take_failure() {
            drop(source);
            return Some(self.end_fail(fail));
        }
        if self.overflow.is_none()
            && !source
                .utf8_chunks()
                .any(|c| !c.invalid().is_empty() || c.valid().chars().any(|c| !syntax::space(c)))
        {
            drop(source);
            self.finish_early(0, Status::Completed, None, None);
            return Some(self.done_step());
        }
        let units = if self.overflow.is_none() && !self.raw_script {
            match syntax::normalize(&source, &self.live) {
                Ok(normalized) => source = normalized,
                Err(e) => {
                    drop(source);
                    return Some(self.end_fail(Fail::Mem(e)));
                }
            }
            utf16_len(&source)
        } else {
            units
        };
        if self.overflow.is_some() || units > 1_000_000 {
            drop(source);
            self.finish_early(2, Status::Completed, None,
                Some(format_args!("bash: syntax error: Parse error at 1:1: Input too large: {units} bytes exceeds limit of 1000000\n")));
            return Some(self.done_step());
        }
        let ast = match Ast::parse(&source, &self.live) {
            Ok(ast) => ast,
            Err(syntax::Error::Memory(e)) => {
                drop(source);
                return Some(self.end_fail(Fail::Mem(e)));
            }
            Err(error) => {
                drop(source);
                self.finish_early(2, Status::Completed, None, Some(format_args!("{error}")));
                return Some(self.done_step());
            }
        };
        if let Err(e) = self.start_vm(ast.entry) {
            drop(ast);
            drop(source);
            return Some(self.end_fail(e));
        }
        self.program = Some(Program { source, ast });
        None
    }

    fn host(&mut self, pending: Pending, req: HostReq) -> Step {
        self.epoch += 1;
        self.state = State::Pending(pending);
        self.body.clear();
        self.call_env = None;
        Step::Host {
            epoch: self.epoch,
            req,
        }
    }

    fn command_poll(&mut self, poll: CmdPoll) -> Option<Step> {
        match poll {
            CmdPoll::Done(code) => {
                self.last_exit = code;
                None
            }
            CmdPoll::Yield => Some(Step::OutOfFuel),
            CmdPoll::Host(req) => Some(self.host(Pending::Kernel, req)),
        }
    }

    fn resume_command(&mut self, outcome: Option<Outcome<'_>>) -> Result<Option<Step>, Fail> {
        let mut frame = self.active.take().expect("resumable command has a frame");
        let poll = frame.resume(
            &mut CmdCtx {
                fiber: self,
                xpg_echo: false,
            },
            outcome,
        )?;
        if !matches!(poll, CmdPoll::Done(_)) {
            self.active = Some(frame);
        }
        Ok(self.command_poll(poll))
    }

    fn custom_call(&mut self, name: &[u8], args: KVec<KBytes>) -> Result<Flow, Fail> {
        if !self.shell.granted(G_CUSTOM) {
            self.last_exit = 126;
            self.write_out(Chan::Stderr, b"bash: permission denied\n")?;
            return Ok(Flow::Next);
        }
        Ok(Flow::Host(
            Pending::Custom,
            HostReq::Custom {
                name: bytes_in(name, &self.live)?,
                args,
                cwd: bytes_in(&self.cwd, &self.live)?,
                env: self.env.snapshot()?,
                input_len: self.stdin.len(),
            },
        ))
    }

    fn complete(&mut self, p: Pending, o: Outcome<'_>) -> Result<(), CompleteError> {
        match p {
            Pending::Custom => match o {
                Outcome::CommandExit(n) => self.last_exit = n,
                Outcome::Denied => {
                    self.last_exit = 126;
                    self.write_out(Chan::Stderr, b"bash: permission denied\n")
                        .map_err(CompleteError::Fail)?;
                }
                Outcome::Failed(msg) => {
                    self.last_exit = 1;
                    self.write_out(Chan::Stderr, msg)
                        .map_err(CompleteError::Fail)?;
                }
                _ => return Err(CompleteError::BadOutcome),
            },
            Pending::Kernel => return Err(CompleteError::BadOutcome),
            Pending::Test(code, aux) => self.complete_test(code, aux, o)?,
        }
        Ok(())
    }

    fn end_fail(&mut self, fail: Fail) -> Step {
        let lim = self.shell.lim.vals;
        let mut text = Diagnostic::new();
        let label = match fail {
            Fail::Output => {
                let _ = writeln!(text, "bash: pipeline: total output size exceeded (>{} bytes), increase executionLimits.maxOutputSize", self.out_cap);
                "maxOutputSize"
            }
            Fail::Input => {
                let _ = writeln!(
                    text,
                    "bash: stdin size limit exceeded ({} bytes)",
                    self.in_cap
                );
                if lim[I_INPUT_BYTES] <= lim[I_STRING_LENGTH] {
                    "maxInputBytes"
                } else {
                    "maxStringLength"
                }
            }
            Fail::StringLength => {
                let _ = writeln!(
                    text,
                    "bash: word expansion: string length limit exceeded ({} bytes)",
                    lim[I_STRING_LENGTH]
                );
                "maxStringLength"
            }
            Fail::Elements | Fail::Arguments => {
                let kind = if matches!(fail, Fail::Elements) {
                    "array"
                } else {
                    "expanded argument"
                };
                let _ = writeln!(
                    text,
                    "bash: {kind} element limit exceeded ({})",
                    lim[I_ARRAY_ELEMENTS]
                );
                "maxArrayElements"
            }
            Fail::CommandCount => {
                let _ = writeln!(text, "bash: too many commands executed (>{}), increase executionLimits.maxCommandCount", lim[I_COMMAND_COUNT]);
                "maxCommandCount"
            }
            Fail::CallDepth(func) => {
                let cap = lim[I_CALL_DEPTH];
                let func = core::str::from_utf8(&func).unwrap_or("?");
                let _ = writeln!(text, "bash: {func}: maximum recursion depth ({cap}) exceeded, increase executionLimits.maxCallDepth");
                "maxCallDepth"
            }
            Fail::ExecDepth => {
                let _ = writeln!(
                    text,
                    "bash: maximum nested execution depth ({}) exceeded",
                    lim[I_EXEC_DEPTH]
                );
                "maxExecDepth"
            }
            Fail::Work => {
                let _ = writeln!(
                    text,
                    "bash: execution: aggregate work limit exceeded ({})",
                    lim[I_WORK_UNITS]
                );
                "maxWorkUnits"
            }
            Fail::Mem(MemError::Limit(id)) if id == limits::id_of_index(I_LIVE_BYTES) => {
                let _ = writeln!(
                    text,
                    "bash: execution: live byte limit exceeded ({} bytes)",
                    lim[I_LIVE_BYTES]
                );
                "maxLiveBytes"
            }
            Fail::Mem(MemError::Limit(id)) if id == limits::id_of_index(I_FS_BYTES) => {
                let _ = writeln!(
                    text,
                    "bash: filesystem size limit exceeded ({} bytes)",
                    lim[I_FS_BYTES]
                );
                "maxFileSystemBytes"
            }
            Fail::Readonly(var) => {
                let var = core::str::from_utf8(&var).unwrap_or("?");
                self.finish_early(
                    1,
                    Status::Completed,
                    None,
                    Some(format_args!("bash: {var}: readonly variable\n")),
                );
                return self.done_step();
            }
            Fail::Mem(MemError::Limit(id)) => limits::name_of(id),
            _ => "memory-ceiling",
        };
        if text.len == 0 {
            let _ = writeln!(text, "bash: {label} exceeded");
        }
        let message = core::str::from_utf8(text.bytes()).expect("formatted diagnostic is UTF-8");
        self.finish_early(
            126,
            Status::LimitExceeded,
            Some(label),
            Some(format_args!("{message}")),
        );
        self.done_step()
    }

    fn finish_early(
        &mut self,
        exit: i32,
        status: Status,
        limit: Option<&str>,
        diagnostic: Option<core::fmt::Arguments<'_>>,
    ) {
        self.finish(exit, status, limit, true, diagnostic);
    }

    fn finish(
        &mut self,
        exit: i32,
        status: Status,
        limit: Option<&str>,
        early: bool,
        diagnostic: Option<core::fmt::Arguments<'_>>,
    ) {
        self.vm = Vm::new(&self.live);
        self.script = None;
        self.program = None;
        self.active = None;
        self.stdin.clear();
        self.body.clear();
        self.input_window = KBytes::new_in(&self.live);
        self.last_arg = None;
        self.params = KVec::new_in(&self.live);
        self.args = KVec::new_in(&self.live);
        self.kept = KVec::new_in(&self.live);
        self.call_env = None;
        self.stdin_error = None;
        self.failed = None;
        let mut env = core::mem::replace(&mut self.env, Env::new_in(&self.scratch));
        if early {
            drop(env);
            env = reserve_scope(|| {
                let mut env = Env::new_in(&self.scratch);
                let _ = env.merge_shared(&self.shell.env);
                let _ = env.merge_shared(&self.overlay);
                env
            });
        }
        self.overlay = Env::new_in(&self.scratch);
        let limit_name = reserve_scope(|| {
            if let Some(message) = diagnostic {
                let mut text = Diagnostic::new();
                self.stderr_tail = if text.write_fmt(message).is_ok() {
                    bytes_in(text.bytes(), &self.scratch)
                        .unwrap_or_else(|_| KBytes::new_in(&self.scratch))
                } else {
                    fmt_in(&self.scratch, |buffer| buffer.write_fmt(message))
                        .map(|s| s.into_bytes())
                        .unwrap_or_else(|_| KBytes::new_in(&self.scratch))
                };
            }
            limit.and_then(|name| str_in(name, &self.scratch).ok())
        });
        self.last_exit = exit;
        if let Some(slot) = KRc::get_mut(&mut self.result.env) {
            *slot = env;
        } else if let Ok(replacement) = reserve_scope(|| KRc::try_new_in(env, &self.scratch)) {
            self.result.env = replacement;
        }
        self.result.exit = exit;
        self.result.status = status;
        self.result.limit_name = limit_name;
        self.result.stdout_bytes = self.stdout_bytes;
        self.state = State::Done;
    }

    pub fn cancel(&mut self, reason: CancelReason) -> ExecResult {
        if !self.is_done() {
            let (status, exit, name) = match reason {
                CancelReason::Aborted => (Status::Aborted, 124, None),
                CancelReason::Deadline => (Status::Deadline, 124, None),
                CancelReason::Limit => (Status::LimitExceeded, 126, Some("limit")),
            };
            self.finish_early(
                exit,
                status,
                name,
                Some(format_args!("bash: execution aborted\n")),
            );
        }
        let Step::Done(result) = self.done_step() else {
            unreachable!()
        };
        result
    }
}

fn first_chunk(buf: &KChunks, max: usize) -> &[u8] {
    let first = buf.chunks().next().unwrap_or_default();
    &first[..first.len().min(max)]
}

fn readonly(name: &[u8]) -> bool {
    matches!(name, b"SHELLOPTS" | b"BASHOPTS")
}

pub(crate) enum CompleteError {
    BadOutcome,
    Fail(Fail),
}

#[cfg(feature = "test-commands")]
mod testcmd;

#[cfg(not(feature = "test-commands"))]
impl Fiber {
    fn complete_test(
        &mut self,
        _code: u8,
        _aux: u64,
        _o: Outcome<'_>,
    ) -> Result<(), CompleteError> {
        Err(CompleteError::BadOutcome)
    }
}

#[cfg(test)]
mod tests;
