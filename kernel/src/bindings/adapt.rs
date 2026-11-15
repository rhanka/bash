//! Adapter between the generated contract types and the kernel proper. Conversions copy: the contract
//! types own `Vec`s and `String`s from the global allocator, the kernel works on `kernel::mem` containers.
use super::kernel::exports::sentropic::bash::engine as E;
use super::kernel::exports::sentropic::bash::types as T;
use super::rt;
use crate::fiber::{self, CallError, Chan, Fiber, HostReq, Outcome, Status, Step};
use crate::fs::{self, Code, Ok_, Op, Vfs};
use crate::limits::{self, Resolved};
use crate::mem::{self, Budget, KRc, LimitId, MemError};
use crate::shell::{Config, CreateError, Shell};
use crate::util::Env;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

static CONFIGURED: AtomicBool = AtomicBool::new(false);

pub struct Kernel;

fn kerr(code: T::KernelErrorCode, detail: &str) -> T::KernelError {
    T::KernelError { code, detail: String::from(detail) }
}

fn mem_err(e: MemError) -> T::KernelError {
    match e {
        MemError::Limit(id) => kerr(T::KernelErrorCode::Ceiling, limits::name_of(id)),
        _ => kerr(T::KernelErrorCode::Ceiling, "memory-ceiling"),
    }
}

fn configured() -> Result<(), T::KernelError> {
    if CONFIGURED.load(Relaxed) {
        Ok(())
    } else {
        Err(kerr(T::KernelErrorCode::NotConfigured, "configure was not called"))
    }
}

fn call_err(e: CallError) -> T::KernelError {
    match e {
        CallError::StaleEpoch => kerr(T::KernelErrorCode::StaleEpoch, "stale epoch"),
        CallError::BadState => kerr(T::KernelErrorCode::BadState, "bad state"),
        CallError::Denied => kerr(T::KernelErrorCode::Denied, "denied"),
    }
}

fn meta() -> Result<Budget, T::KernelError> {
    Budget::root(LimitId::HEADROOM, u64::MAX).map_err(mem_err)
}

fn rc<V>(v: V) -> Result<KRc<V>, T::KernelError> {
    let m = meta()?;
    KRc::try_new_in(v, &m).map_err(|(_, e)| mem_err(e))
}

impl E::Guest for Kernel {
    type Vfs = VfsRes;
    type Shell = ShellRes;
    type Fiber = FiberRes;

    fn configure(memory_ceiling_bytes: u64) -> Result<(), T::KernelError> {
        match mem::configure(memory_ceiling_bytes) {
            Ok(()) => {
                CONFIGURED.store(true, Relaxed);
                Ok(())
            }
            Err(mem::ConfigError::AlreadyConfigured) => Err(kerr(T::KernelErrorCode::AlreadyConfigured, "already configured")),
            Err(_) => Err(kerr(T::KernelErrorCode::InvalidConfig, "ceiling below the bytes in use")),
        }
    }

    fn raise_ceiling(memory_ceiling_bytes: u64) -> Result<(), T::KernelError> {
        match mem::raise_ceiling(memory_ceiling_bytes) {
            Ok(()) => Ok(()),
            Err(mem::ConfigError::NotConfigured) => Err(kerr(T::KernelErrorCode::NotConfigured, "configure was not called")),
            Err(_) => Err(kerr(T::KernelErrorCode::InvalidConfig, "ceiling below the current one")),
        }
    }

    fn usage() -> T::MemoryUsage {
        let u = mem::usage();
        T::MemoryUsage { used: u.used, ceiling: u.ceiling, high_water: u.high_water }
    }

    fn coverage_features() -> Vec<String> {
        Vec::new()
    }

    fn exec_oneshot(req: T::OneshotReq) -> Result<T::OneshotResult, T::KernelError> {
        oneshot(req)
    }
}

// ------------------------------------------------------------------------------------------ vfs

pub struct VfsRes(KRc<Vfs>);

impl rt::Resource for VfsRes {
    type Rep = VfsRes;
}

fn fs_code(c: Code) -> T::FsErrorCode {
    match c {
        Code::NotFound => T::FsErrorCode::NotFound,
        Code::Exists => T::FsErrorCode::Exists,
        Code::NotDirectory => T::FsErrorCode::NotDirectory,
        Code::IsDirectory => T::FsErrorCode::IsDirectory,
        Code::NotEmpty => T::FsErrorCode::NotEmpty,
        Code::Permission => T::FsErrorCode::Permission,
        Code::Invalid => T::FsErrorCode::Invalid,
        Code::NoSpace => T::FsErrorCode::NoSpace,
        Code::Unsupported => T::FsErrorCode::Unsupported,
    }
}

fn fs_code_name(c: &T::FsErrorCode) -> &'static str {
    match c {
        T::FsErrorCode::NotFound => "not-found",
        T::FsErrorCode::Exists => "exists",
        T::FsErrorCode::NotDirectory => "not-directory",
        T::FsErrorCode::IsDirectory => "is-directory",
        T::FsErrorCode::NotEmpty => "not-empty",
        T::FsErrorCode::Permission => "permission",
        T::FsErrorCode::Invalid => "invalid",
        T::FsErrorCode::LinkLoop => "link-loop",
        T::FsErrorCode::TooLarge => "too-large",
        T::FsErrorCode::NoSpace => "no-space",
        T::FsErrorCode::Busy => "busy",
        T::FsErrorCode::CrossDevice => "cross-device",
        T::FsErrorCode::ReadOnly => "read-only",
        T::FsErrorCode::Unsupported => "unsupported",
        T::FsErrorCode::Io => "io",
    }
}

fn fs_err(c: Code, path: &[u8]) -> T::FsError {
    T::FsError { code: fs_code(c), path: path.to_vec() }
}

fn kind_out(k: fs::Kind) -> T::NodeKind {
    match k {
        fs::Kind::File => T::NodeKind::File,
        fs::Kind::Dir => T::NodeKind::Directory,
    }
}

fn op_path(op: &T::FsOp) -> &[u8] {
    match op {
        T::FsOp::Stat(p) | T::FsOp::Lstat(p) | T::FsOp::Exists(p) | T::FsOp::ReadDir(p) | T::FsOp::ReadLink(p) | T::FsOp::RealPath(p) | T::FsOp::ReadDirPlus(p) => p,
        T::FsOp::ReadFile(r) => &r.path,
        T::FsOp::WriteFile(w) => &w.path,
        T::FsOp::Mkdir(m) => &m.path,
        T::FsOp::Remove(r) => &r.path,
        _ => &[],
    }
}

fn to_op(op: &T::FsOp) -> Op<'_> {
    match op {
        T::FsOp::Stat(p) => Op::Stat(p),
        T::FsOp::Lstat(p) => Op::Lstat(p),
        T::FsOp::Exists(p) => Op::Exists(p),
        T::FsOp::ReadDir(p) => Op::ReadDir(p),
        T::FsOp::AllPaths => Op::AllPaths,
        T::FsOp::ReadFile(r) => Op::ReadFile { path: &r.path, offset: r.offset, len: r.len },
        T::FsOp::WriteFile(w) => Op::WriteFile { path: &w.path, data: &w.data, offset: w.offset, truncate: w.truncate, mode: w.mode, mtime_ms: w.mtime_ms },
        T::FsOp::Mkdir(m) => Op::Mkdir { path: &m.path, recursive: m.recursive, mode: m.mode },
        T::FsOp::Remove(r) => Op::Remove { path: &r.path, recursive: r.recursive, force: r.force },
        _ => Op::Other,
    }
}

fn fs_ok(o: Ok_) -> T::FsOk {
    match o {
        Ok_::None => T::FsOk::None,
        Ok_::Flag(b) => T::FsOk::Flag(b),
        Ok_::Info(s) => T::FsOk::Info(T::Stat { kind: kind_out(s.kind), size: s.size, mode: s.mode, mtime_ms: s.mtime_ms, dev: 0, ino: s.ino }),
        Ok_::Data(d) => T::FsOk::Data(d.to_vec()),
        Ok_::Paths(p) => T::FsOk::Paths(p.iter().map(|x| x.to_vec()).collect()),
        Ok_::Entries(e) => T::FsOk::Entries(e.iter().map(|(n, k)| T::DirEntry { name: n.to_vec(), kind: kind_out(*k) }).collect()),
    }
}

impl E::GuestVfs for VfsRes {
    fn create(max_bytes: u64) -> Result<E::Vfs, T::KernelError> {
        configured()?;
        let v = Vfs::create(max_bytes, limits::id_of_index(limits::I_FS_BYTES)).map_err(mem_err)?;
        Ok(E::Vfs::new(VfsRes(rc(v)?)))
    }

    fn op(&self, req: T::FsOp) -> T::FsResult {
        match self.0.op(to_op(&req)) {
            Ok(o) => Ok(fs_ok(o)),
            Err(c) => Err(fs_err(c, op_path(&req))),
        }
    }

    fn mount_vfs(&self, path: Vec<u8>, _source: E::VfsBorrow<'_>) -> Result<(), T::FsError> {
        Err(fs_err(Code::Unsupported, &path))
    }

    fn mount_host(&self, path: Vec<u8>, _mount_id: u32) -> Result<(), T::FsError> {
        Err(fs_err(Code::Unsupported, &path))
    }

    fn unmount(&self, path: Vec<u8>) -> Result<(), T::FsError> {
        Err(fs_err(Code::Unsupported, &path))
    }

    fn take_dirty(&self) -> Vec<T::ChangedFile> {
        Vec::new()
    }

    fn used_bytes(&self) -> u64 {
        self.0.used_bytes()
    }
}

// ----------------------------------------------------------------------------------------- shell

pub struct ShellRes(KRc<Shell>);

impl rt::Resource for ShellRes {
    type Rep = ShellRes;
}

fn env_out(e: &Env) -> Vec<T::EnvVar> {
    e.v.iter().map(|x| T::EnvVar { name: x.name.to_vec(), value: x.value.to_vec(), exported: x.exported }).collect()
}

fn env_in(e: &[T::EnvVar]) -> Vec<(&[u8], &[u8], bool)> {
    e.iter().map(|x| (x.name.as_slice(), x.value.as_slice(), x.exported)).collect()
}

fn create_err(e: CreateError) -> T::KernelError {
    match e {
        CreateError::Invalid(m) => kerr(T::KernelErrorCode::InvalidConfig, m),
        CreateError::Mem(m) => mem_err(m),
    }
}

fn overrides_of(l: &T::Limits) -> Vec<(&str, u64)> {
    l.overrides.iter().map(|o| (o.name.as_str(), o.value)).collect()
}

impl E::GuestShell for ShellRes {
    fn create(root: E::VfsBorrow<'_>, config: T::ShellConfig) -> Result<E::Shell, T::KernelError> {
        configured()?;
        let vfs = root.get::<VfsRes>().0.clone();
        let overrides = overrides_of(&config.limits);
        let env = env_in(&config.env);
        let custom: Vec<&str> = config.custom_commands.iter().map(|s| s.as_str()).collect();
        let cfg = Config {
            profile: config.limits.profile as usize,
            overrides: &overrides,
            grants: config.grants.bits(),
            custom_commands: &custom,
            env: &env,
            cwd: config.cwd.as_deref(),
        };
        let sh = Shell::create(vfs, &cfg, mem::usage().ceiling).map_err(create_err)?;
        Ok(E::Shell::new(ShellRes(rc(sh)?)))
    }

    fn register_command(&self, name: String) -> Result<(), T::KernelError> {
        self.0.register(&name).map_err(mem_err)
    }

    fn env(&self) -> Vec<T::EnvVar> {
        env_out(&self.0.env)
    }

    fn cwd(&self) -> Vec<u8> {
        self.0.cwd.to_vec()
    }
}

// ----------------------------------------------------------------------------------------- fiber

pub struct FiberRes(RefCell<Fiber>);

impl rt::Resource for FiberRes {
    type Rep = FiberRes;
}

fn status_out(s: Status) -> T::ExecStatus {
    match s {
        Status::Completed => T::ExecStatus::Completed,
        Status::LimitExceeded => T::ExecStatus::LimitExceeded,
        Status::Aborted => T::ExecStatus::Aborted,
        Status::Deadline => T::ExecStatus::Deadline,
    }
}

fn result_out(r: &fiber::ExecResult) -> T::ExecResult {
    T::ExecResult { exit_code: r.exit, status: status_out(r.status), limit_name: r.limit_name.as_ref().map(|n| String::from(n.as_str())), env: env_out(&r.env) }
}

fn req_out(r: &HostReq) -> T::HostReq {
    match r {
        HostReq::FsStat { mount_id, path } => T::HostReq::FsBatch(T::FsBatchReq { mount_id: *mount_id, ops: alloc::vec![T::FsOp::Stat(path.to_vec())] }),
        HostReq::LazyFile(p) => T::HostReq::LazyFile(p.to_vec()),
        HostReq::Custom { name, args, cwd, env, input_len } => T::HostReq::CustomCommand(T::CommandCall {
            name: String::from_utf8_lossy(name).into_owned(),
            args: args.iter().map(|a| a.to_vec()).collect(),
            cwd: cwd.to_vec(),
            env: env_out(env),
            fds: Vec::new(),
            input_len: *input_len,
            xpg_echo: false,
        }),
        HostReq::Fetch { url } => T::HostReq::Fetch(T::FetchReq {
            method: String::from("GET"),
            url: String::from_utf8_lossy(url).into_owned(),
            headers: Vec::new(),
            input_len: 0,
            follow_redirects: true,
            max_redirects: None,
            timeout_ms: 30_000,
            max_response_bytes: 10 << 20,
        }),
        HostReq::Sleep(ms) => T::HostReq::Sleep(*ms),
        HostReq::Clock => T::HostReq::Clock,
        HostReq::Entropy(n) => T::HostReq::Entropy(*n),
    }
}

fn step_out(s: Step) -> T::Step {
    match s {
        Step::Done(r) => T::Step::Done(result_out(&r)),
        Step::OutOfFuel => T::Step::OutOfFuel,
        Step::Host { epoch, req } => T::Step::HostCall(T::PendingCall { epoch, req: req_out(&req) }),
    }
}

fn net_code_name(c: &T::NetErrorCode) -> &'static str {
    match c {
        T::NetErrorCode::AccessDenied => "access-denied",
        T::NetErrorCode::MethodNotAllowed => "method-not-allowed",
        T::NetErrorCode::RedirectDenied => "redirect-denied",
        T::NetErrorCode::TooManyRedirects => "too-many-redirects",
        T::NetErrorCode::ResponseTooLarge => "response-too-large",
        T::NetErrorCode::Timeout => "timeout",
        T::NetErrorCode::Network => "network",
    }
}

fn outcome_in(o: &T::HostOutcome) -> Outcome<'_> {
    match o {
        T::HostOutcome::Fs(list) => match list.first() {
            Some(Ok(_)) => Outcome::Fs(None),
            Some(Err(e)) => Outcome::Fs(Some(fs_code_name(&e.code))),
            None => Outcome::Fs(Some("io")),
        },
        T::HostOutcome::Data(d) => Outcome::Data(d),
        T::HostOutcome::CommandExit(n) => Outcome::CommandExit(*n),
        T::HostOutcome::Fetched(r) => Outcome::Fetched(r.status),
        T::HostOutcome::Clock(c) => Outcome::Clock(c.wall_ms),
        T::HostOutcome::Completed => Outcome::Completed,
        T::HostOutcome::Denied => Outcome::Denied,
        T::HostOutcome::LimitExceeded(n) => Outcome::LimitExceeded(n),
        T::HostOutcome::NetFailed(e) => Outcome::NetFailed(net_code_name(&e.code)),
        T::HostOutcome::Failed(b) => Outcome::Failed(b),
    }
}

fn chan_in(c: &T::OutChannel) -> Chan {
    match c {
        T::OutChannel::Stdout => Chan::Stdout,
        T::OutChannel::Stderr => Chan::Stderr,
        T::OutChannel::Body => Chan::Body,
    }
}

fn spawn_fiber(shell: KRc<Shell>, parent: Option<&Budget>, o: &T::ExecOpts) -> Result<Fiber, T::KernelError> {
    let env = env_in(&o.env);
    let opts = fiber::Opts {
        script: &o.script,
        overflow: o.script_overflow.as_ref().map(|s| (s.utf16_units, s.utf8_bytes)),
        env: &env,
        replace_env: o.replace_env,
        cwd: o.cwd.as_deref(),
        bypass_custom: o.bypass_custom.as_deref(),
    };
    Fiber::spawn(shell, parent, &opts).map_err(mem_err)
}

impl E::GuestFiber for FiberRes {
    fn spawn(owner: E::ShellBorrow<'_>, parent: Option<E::FiberBorrow<'_>>, opts: T::ExecOpts) -> Result<E::Fiber, T::KernelError> {
        configured()?;
        let shell = owner.get::<ShellRes>().0.clone();
        let parent_live = parent.as_ref().map(|p| p.get::<FiberRes>().0.borrow().live_budget().clone());
        let f = spawn_fiber(shell, parent_live.as_ref(), &opts)?;
        Ok(E::Fiber::new(FiberRes(RefCell::new(f))))
    }

    fn write_stdin(&self, chunk: Vec<u8>, last: bool) -> Result<(), T::KernelError> {
        self.0.borrow_mut().write_stdin(&chunk, last).map_err(call_err)
    }

    fn run(&self, fuel: u64) -> T::Step {
        step_out(self.0.borrow_mut().run(fuel))
    }

    fn resume(&self, epoch: u64, outcome: T::HostOutcome) -> Result<T::Step, T::KernelError> {
        let step = self.0.borrow_mut().resume(epoch, outcome_in(&outcome)).map_err(call_err)?;
        Ok(step_out(step))
    }

    fn read_call_input(&self, epoch: u64, fd: u32, max: u32) -> Result<Vec<u8>, T::KernelError> {
        let mut f = self.0.borrow_mut();
        let d = f.read_call_input(epoch, fd, max as usize).map_err(call_err)?;
        Ok(d.to_vec())
    }

    fn push_call_data(&self, epoch: u64, channel: T::OutChannel, chunk: Vec<u8>) -> Result<(), T::KernelError> {
        self.0.borrow_mut().push_call_data(epoch, chan_in(&channel), &chunk).map_err(call_err)
    }

    fn assign_variable(&self, epoch: u64, name: Vec<u8>, value: Vec<u8>, subscript: Option<Vec<u8>>) -> Result<(), T::KernelError> {
        self.0.borrow_mut().assign_variable(epoch, &name, &value, subscript.as_deref()).map_err(call_err)
    }

    fn set_call_env(&self, epoch: u64, env: Vec<T::EnvVar>) -> Result<(), T::KernelError> {
        let e = env_in(&env);
        self.0.borrow_mut().set_call_env(epoch, &e).map_err(call_err)
    }

    fn cancel(&self, reason: T::CancelReason) -> T::ExecResult {
        let r = match reason {
            T::CancelReason::Aborted => fiber::CancelReason::Aborted,
            T::CancelReason::Deadline => fiber::CancelReason::Deadline,
            T::CancelReason::Limit => fiber::CancelReason::Limit,
        };
        result_out(&self.0.borrow_mut().cancel(r))
    }

    fn take_stdout(&self, max: u32) -> Vec<u8> {
        let mut f = self.0.borrow_mut();
        let v = f.stdout_chunk(max as usize).to_vec();
        f.stdout_consume(v.len());
        v
    }

    fn take_stderr(&self, max: u32) -> Vec<u8> {
        let mut f = self.0.borrow_mut();
        let v = f.stderr_chunk(max as usize).to_vec();
        f.stderr_consume(v.len());
        v
    }

    fn take_trace(&self, _max: u32) -> Vec<T::TraceEvent> {
        Vec::new()
    }

    fn take_coverage(&self, _max: u32) -> Vec<u16> {
        Vec::new()
    }
}

// --------------------------------------------------------------------------------------- oneshot

/// Minimal `exec-oneshot` (the session drivers of L1c build on it): a throwaway volume and shell, the
/// script run to completion; a host call is a `bad-state`.
fn oneshot(req: T::OneshotReq) -> Result<T::OneshotResult, T::KernelError> {
    configured()?;
    if req.config.trace || req.config.coverage {
        return Err(kerr(T::KernelErrorCode::InvalidConfig, "trace and coverage have no channel in exec-oneshot"));
    }
    let overrides = overrides_of(&req.config.limits);
    let mut lim = Resolved::of_profile(req.config.limits.profile as usize);
    for (n, v) in &overrides {
        if !lim.set(n, *v) {
            return Err(kerr(T::KernelErrorCode::InvalidConfig, "unknown limit name"));
        }
    }
    let vfs = rc(Vfs::create(lim.vals[limits::I_FS_BYTES], limits::id_of_index(limits::I_FS_BYTES)).map_err(mem_err)?)?;
    for f in &req.files {
        let w = Op::WriteFile { path: &f.path, data: &f.data, offset: Some(0), truncate: true, mode: Some(f.mode), mtime_ms: None };
        vfs.op(w).map_err(|_| kerr(T::KernelErrorCode::InvalidConfig, "cannot place an input file"))?;
    }
    let env = env_in(&req.config.env);
    let custom: Vec<&str> = req.config.custom_commands.iter().map(|s| s.as_str()).collect();
    let cfg = Config { profile: req.config.limits.profile as usize, overrides: &overrides, grants: req.config.grants.bits(), custom_commands: &custom, env: &env, cwd: req.config.cwd.as_deref() };
    let sh = rc(Shell::create(vfs.clone(), &cfg, mem::usage().ceiling).map_err(create_err)?)?;
    let mut f = spawn_fiber(sh, None, &req.opts)?;
    for (i, c) in req.stdin.chunks(1 << 20).enumerate() {
        let _ = i;
        f.write_stdin(c, false).map_err(call_err)?;
    }
    f.write_stdin(&[], true).map_err(call_err)?;
    let mut left = req.fuel;
    let result = loop {
        let slice = left.min(100_000);
        match f.run(slice) {
            Step::Done(r) => break r,
            Step::OutOfFuel => {
                left -= slice;
                if left == 0 {
                    break f.cancel(fiber::CancelReason::Limit);
                }
            }
            Step::Host { .. } => return Err(kerr(T::KernelErrorCode::BadState, "the script needs a host call")),
        }
    };
    let mut stdout = Vec::new();
    loop {
        let c = f.stdout_chunk(1 << 20);
        if c.is_empty() {
            break;
        }
        stdout.extend_from_slice(c);
        let n = c.len();
        f.stdout_consume(n);
    }
    let mut stderr = Vec::new();
    loop {
        let c = f.stderr_chunk(1 << 20);
        if c.is_empty() {
            break;
        }
        stderr.extend_from_slice(c);
        let n = c.len();
        f.stderr_consume(n);
    }
    let mut changed = Vec::new();
    let mut removed: Vec<Vec<u8>> = Vec::new();
    if let Ok(Ok_::Paths(paths)) = vfs.op(Op::AllPaths) {
        for p in paths.iter() {
            if let Ok(Ok_::Info(s)) = vfs.op(Op::Stat(p)) {
                if s.kind != fs::Kind::File {
                    continue;
                }
                if let Ok(Ok_::Data(d)) = vfs.op(Op::ReadFile { path: p, offset: 0, len: u32::MAX }) {
                    let same = req.files.iter().any(|x| x.path.as_slice() == p.as_slice() && x.data.as_slice() == d.as_slice());
                    if !same {
                        changed.push(T::OneshotFile { path: p.to_vec(), data: d.to_vec(), mode: s.mode });
                    }
                }
            }
        }
        for x in &req.files {
            if !paths.iter().any(|p| p.as_slice() == x.path.as_slice()) {
                removed.push(x.path.clone());
            }
        }
    }
    Ok(T::OneshotResult { outcome: result_out(&result), stdout, stderr, changed, removed })
}

super::kernel::export!(Kernel with_types_in super::kernel);
