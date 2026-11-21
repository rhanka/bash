//! Adapter between the generated contract types and the kernel proper. Conversions copy: the contract
//! types own `Vec`s and `String`s from the global allocator, the kernel works on `kernel::mem` containers.
use super::kernel::exports::sentropic::bash::engine as E;
use super::kernel::exports::sentropic::bash::types as T;
use super::rt;
use crate::fiber::{self, CallError, Chan, Fiber, HostReq, Outcome, Status, Step};
use crate::fs::{self, Code, Ok_, Op, Vfs};
use crate::limits::{self, Resolved};
use crate::mem::{self, Budget, KRc, LimitId, MemError};
use crate::shell::{Config, CreateError, ProcessInfo, Shell};
use crate::util::Env;
use crate::mem::abi::{String, Vec};
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

static CONFIGURED: AtomicBool = AtomicBool::new(false);

pub struct Kernel;

fn copy<T: Copy>(items: &[T]) -> Result<Vec<T>, MemError> {
    let mut out = Vec::new();
    out.try_reserve_exact(items.len()).map_err(|_| MemError::Ceiling)?;
    out.extend_from_slice(items);
    Ok(out)
}

fn copy_text(s: &str) -> Result<String, MemError> {
    let mut out = String::new();
    out.try_reserve_exact(s.len()).map_err(|_| MemError::Ceiling)?;
    out.push_str(s);
    Ok(out)
}

fn lossy_text(bytes: &[u8]) -> Result<String, MemError> {
    if let Ok(s) = core::str::from_utf8(bytes) {
        return copy_text(s);
    }
    let len = bytes.utf8_chunks().map(|c| c.valid().len() + 3 * usize::from(!c.invalid().is_empty())).sum();
    let mut out = String::new();
    out.try_reserve_exact(len).map_err(|_| MemError::Ceiling)?;
    for c in bytes.utf8_chunks() {
        out.push_str(c.valid());
        if !c.invalid().is_empty() {
            out.push('\u{fffd}');
        }
    }
    Ok(out)
}

fn collect<I, T, U>(items: I, mut convert: impl FnMut(T) -> Result<U, MemError>) -> Result<Vec<U>, MemError>
where
    I: ExactSizeIterator<Item = T>,
{
    let mut out = Vec::new();
    out.try_reserve_exact(items.len()).map_err(|_| MemError::Ceiling)?;
    for item in items {
        out.push(convert(item)?);
    }
    Ok(out)
}

fn copy_bounded(bytes: &[u8], max: usize) -> Result<Vec<u8>, MemError> {
    let bytes = &bytes[..bytes.len().min(max)];
    copy(bytes).or_else(|_| mem::reserve_scope(|| copy(bytes)))
}

// Each chunk is lifted and released before the next call. Try a smaller chunk when the reserve
// has no room for the usual fallback.
fn drain_copy(bytes: &[u8]) -> Result<Vec<u8>, MemError> {
    let bytes = &bytes[..bytes.len().min(1 << 20)];
    copy(bytes).or_else(|_| copy_bounded(bytes, 16 << 10)).or_else(|_| copy_bounded(bytes, 1024))
}

fn diagnostic_text(text: &str) -> Result<String, MemError> {
    copy_text(text).or_else(|_| {
        let text = &text[..text.floor_char_boundary(256)];
        mem::reserve_scope(|| copy_text(text))
    })
}

fn kerr(code: T::KernelErrorCode, detail: &str) -> T::KernelError {
    T::KernelError { code, detail: diagnostic_text(detail).unwrap_or_default() }
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
        // The dedicated reserve and ordinary arena each need at least one Wasm page.
        if !CONFIGURED.load(Relaxed) && memory_ceiling_bytes < mem::footprint().max(2 * 65_536) {
            return Err(kerr(T::KernelErrorCode::InvalidConfig, "ceiling below the allocator minimum or current footprint"));
        }
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
        let encode = || {
            let features = collect(crate::cmd::COVERAGE_FEATURES.iter(), |s| copy_text(s))?;
            rt::prepare_lowering(&[(features.len(), 8, 4)])?;
            Ok::<_, MemError>(features)
        };
        encode().unwrap_or_default()
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

fn kind_out(k: fs::Kind) -> T::NodeKind {
    match k {
        fs::Kind::File => T::NodeKind::File,
        fs::Kind::Dir => T::NodeKind::Directory,
    }
}

fn op_path(op: T::FsOp) -> Vec<u8> {
    match op {
        T::FsOp::Stat(p) | T::FsOp::Lstat(p) | T::FsOp::Exists(p) | T::FsOp::ReadDir(p) | T::FsOp::ReadLink(p) | T::FsOp::RealPath(p) | T::FsOp::ReadDirPlus(p) => p,
        T::FsOp::ReadFile(r) => r.path,
        T::FsOp::WriteFile(w) => w.path,
        T::FsOp::Mkdir(m) => m.path,
        T::FsOp::Remove(r) => r.path,
        _ => Vec::new(),
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

fn fs_ok(o: Ok_) -> Result<T::FsOk, MemError> {
    Ok(match o {
        Ok_::None => T::FsOk::None,
        Ok_::Flag(b) => T::FsOk::Flag(b),
        Ok_::Info(s) => T::FsOk::Info(T::Stat { kind: kind_out(s.kind), size: s.size, mode: s.mode, mtime_ms: s.mtime_ms, dev: 0, ino: s.ino }),
        Ok_::Data(d) => T::FsOk::Data(copy(&d)?),
        Ok_::Paths(p) => {
            let paths = collect(p.iter(), |x| copy(x))?;
            rt::prepare_lowering(&[(paths.len(), 8, 4)])?;
            T::FsOk::Paths(paths)
        }
        Ok_::Entries(e) => {
            let entries = collect(e.iter(), |(n, k)| Ok(T::DirEntry { name: copy(n)?, kind: kind_out(*k) }))?;
            rt::prepare_lowering(&[(entries.len(), 12, 4)])?;
            T::FsOk::Entries(entries)
        }
    })
}

impl E::GuestVfs for VfsRes {
    fn create(max_bytes: u64) -> Result<E::Vfs, T::KernelError> {
        configured()?;
        let v = Vfs::create(max_bytes, limits::id_of_index(limits::I_FS_BYTES)).map_err(mem_err)?;
        rt::new_resource(VfsRes(rc(v)?), |ptr| {
            // SAFETY: `ptr` is an owned VfsRes with the layout expected by the generated destructor.
            unsafe { E::Vfs::from_handle(Self::_resource_new(ptr)) }
        }).map_err(mem_err)
    }

    fn op(&self, req: T::FsOp) -> T::FsResult {
        self.0.op(to_op(&req)).and_then(|o| fs_ok(o).map_err(|_| Code::NoSpace))
            .map_err(|c| T::FsError { code: fs_code(c), path: op_path(req) })
    }

    fn mount_vfs(&self, path: Vec<u8>, _source: E::VfsBorrow<'_>) -> Result<(), T::FsError> {
        Err(T::FsError { code: T::FsErrorCode::Unsupported, path })
    }

    fn mount_host(&self, path: Vec<u8>, _mount_id: u32) -> Result<(), T::FsError> {
        Err(T::FsError { code: T::FsErrorCode::Unsupported, path })
    }

    fn unmount(&self, path: Vec<u8>) -> Result<(), T::FsError> {
        Err(T::FsError { code: T::FsErrorCode::Unsupported, path })
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

fn env_out(e: &Env) -> Result<Vec<T::EnvVar>, MemError> {
    collect(e.iter(), |(name, entry)| env_var(name.bytes(), entry))
}

fn env_var(name: &[u8], entry: &crate::util::EnvValue) -> Result<T::EnvVar, MemError> {
    Ok(T::EnvVar { name: copy(name)?, value: copy(&entry.value)?, exported: entry.exported })
}

fn env_in(e: &[T::EnvVar]) -> Result<Vec<(&[u8], &[u8], bool)>, T::KernelError> {
    collect(e.iter(), |x| Ok((x.name.as_slice(), x.value.as_slice(), x.exported))).map_err(mem_err)
}

fn create_err(e: CreateError) -> T::KernelError {
    match e {
        CreateError::Invalid(m) => kerr(T::KernelErrorCode::InvalidConfig, m),
        CreateError::Mem(m) => mem_err(m),
    }
}

fn overrides_of(l: &T::Limits) -> Result<Vec<(&str, u64)>, T::KernelError> {
    collect(l.overrides.iter(), |o| Ok((o.name.as_str(), o.value))).map_err(mem_err)
}

fn create_shell(vfs: KRc<Vfs>, config: &T::ShellConfig) -> Result<Shell, T::KernelError> {
    let overrides = overrides_of(&config.limits)?;
    let env = env_in(&config.env)?;
    let custom = collect(config.custom_commands.iter(), |s| Ok(s.as_str())).map_err(mem_err)?;
    let commands = config.commands.as_ref().map(|names| collect(names.iter(), |s| Ok(s.as_str()))).transpose().map_err(mem_err)?;
    let cfg = Config {
        profile: config.limits.profile as usize, overrides: &overrides, grants: config.grants.bits(),
        custom_commands: &custom, env: &env, cwd: config.cwd.as_deref(),
        default_layout: config.default_layout, commands: commands.as_deref(),
        process_info: config.process_info.as_ref().map(|p| ProcessInfo { pid: p.pid, ppid: p.ppid, uid: p.uid, gid: p.gid }),
        coverage: config.coverage,
    };
    Shell::create(vfs, &cfg, mem::usage().ceiling).map_err(create_err)
}

impl E::GuestShell for ShellRes {
    fn create(root: E::VfsBorrow<'_>, config: T::ShellConfig) -> Result<E::Shell, T::KernelError> {
        configured()?;
        let vfs = root.get::<VfsRes>().0.clone();
        let sh = create_shell(vfs, &config)?;
        rt::new_resource(ShellRes(rc(sh)?), |ptr| {
            // SAFETY: `ptr` is an owned ShellRes with the layout expected by the generated destructor.
            unsafe { E::Shell::from_handle(Self::_resource_new(ptr)) }
        }).map_err(mem_err)
    }

    fn register_command(&self, name: String) -> Result<(), T::KernelError> {
        self.0.register(&name).map_err(mem_err)
    }

    fn env(&self) -> Vec<T::EnvVar> {
        let encode = || {
            let env = env_out(&self.0.env)?;
            rt::prepare_lowering(&[(env.len(), 20, 4)])?;
            Ok::<_, MemError>(env)
        };
        encode().unwrap_or_default()
    }

    fn cwd(&self) -> Vec<u8> {
        copy(&self.0.cwd).or_else(|_| copy_bounded(&self.0.cwd, 4096)).unwrap_or_default()
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

fn result_header(r: &fiber::ExecResult) -> T::ExecResult {
    let name = match &r.limit_name {
        Some(name) => Some(name.as_str()),
        // Ordinary memory exhaustion can prevent the fiber from saving the limit name.
        None if matches!(r.status, Status::LimitExceeded) => Some("memory-ceiling"),
        None => None,
    };
    T::ExecResult {
        exit_code: r.exit, status: status_out(r.status),
        limit_name: name.and_then(|name| diagnostic_text(name).ok()),
        env: Vec::new(),
        stdout_kind: if r.stdout_bytes { T::StdoutKind::Bytes } else { T::StdoutKind::Text },
    }
}

fn result_out(r: &fiber::ExecResult, changed: usize, removed: usize) -> Result<T::ExecResult, MemError> {
    let mut result = result_header(r);
    result.env = env_out(&r.env)?;
    rt::prepare_lowering(&[(result.env.len(), 20, 4), (changed, 20, 4), (removed, 8, 4)])?;
    Ok(result)
}

fn transfer_limit(stdout_bytes: bool) -> T::ExecResult {
    T::ExecResult {
        exit_code: 126,
        status: T::ExecStatus::LimitExceeded,
        limit_name: diagnostic_text("memory-ceiling").ok(),
        env: Vec::new(),
        stdout_kind: if stdout_bytes { T::StdoutKind::Bytes } else { T::StdoutKind::Text },
    }
}

fn req_out(r: &HostReq) -> Result<T::HostReq, MemError> {
    Ok(match r {
        HostReq::FsStat { mount_id, path } => {
            let ops = collect(core::iter::once(path), |p| Ok(T::FsOp::Stat(copy(p)?)))?;
            // fs-op contains a discriminant and its largest arm (the write-file record).
            rt::prepare_lowering(&[(ops.len(), 72, 8)])?;
            T::HostReq::FsBatch(T::FsBatchReq { mount_id: *mount_id, ops })
        }
        HostReq::LazyFile(p) => T::HostReq::LazyFile(copy(p)?),
        HostReq::Custom { name, args, cwd, env, input_len } => {
            let call = T::CommandCall {
                name: lossy_text(name)?,
                args: collect(args.iter(), |a| copy(a))?,
                cwd: copy(cwd)?,
                env: collect(env.iter(), |(name, entry)| env_var(name, entry))?,
                fds: Vec::new(),
                input_len: *input_len,
                xpg_echo: false,
            };
            rt::prepare_lowering(&[(call.args.len(), 8, 4), (call.env.len(), 20, 4)])?;
            T::HostReq::CustomCommand(call)
        }
        HostReq::Fetch { url } => T::HostReq::Fetch(T::FetchReq {
            method: copy_text("GET")?,
            url: lossy_text(url)?,
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
    })
}

fn step_out(f: &mut Fiber, s: Step) -> T::Step {
    let result = match s {
        Step::Done(r) if r.status != Status::Completed => {
            return T::Step::Done(mem::reserve_scope(|| result_out(&r, 0, 0)).unwrap_or_else(|_| result_header(&r)));
        }
        Step::Done(r) => result_out(&r, 0, 0).map(T::Step::Done),
        Step::OutOfFuel => return T::Step::OutOfFuel,
        Step::Host { epoch, req } => req_out(&req).map(|req| T::Step::HostCall(T::PendingCall { epoch, req })),
    };
    result.unwrap_or_else(|_| {
        f.fail_abi_transfer();
        T::Step::Done(transfer_limit(f.stdout_bytes()))
    })
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
        T::OutChannel::StdoutBytes => Chan::StdoutBytes,
        T::OutChannel::Stderr => Chan::Stderr,
        T::OutChannel::Body => Chan::Body,
    }
}

fn spawn_fiber(shell: KRc<Shell>, parent: Option<&Fiber>, o: &T::ExecOpts) -> Result<Fiber, T::KernelError> {
    let env = env_in(&o.env)?;
    let args = collect(o.args.iter(), |a| Ok(a.as_slice())).map_err(mem_err)?;
    let opts = fiber::Opts {
        script: &o.script,
        overflow: o.script_overflow.as_ref().map(|s| (s.utf16_units, s.utf8_bytes)),
        env: &env,
        args: &args,
        raw_script: o.raw_script,
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
        let parent = parent.as_ref().map(|p| p.get::<FiberRes>().0.borrow());
        let f = spawn_fiber(shell, parent.as_deref(), &opts)?;
        rt::new_resource(FiberRes(RefCell::new(f)), |ptr| {
            // SAFETY: `ptr` is an owned FiberRes with the layout expected by the generated destructor.
            unsafe { E::Fiber::from_handle(Self::_resource_new(ptr)) }
        }).map_err(mem_err)
    }

    fn write_stdin(&self, chunk: Vec<u8>, last: bool) -> Result<(), T::KernelError> {
        self.0.borrow_mut().write_stdin(&chunk, last).map_err(call_err)
    }

    fn run(&self, fuel: u64) -> T::Step {
        let mut f = self.0.borrow_mut();
        let step = f.run(fuel);
        step_out(&mut f, step)
    }

    fn resume(&self, epoch: u64, outcome: T::HostOutcome) -> Result<T::Step, T::KernelError> {
        let mut f = self.0.borrow_mut();
        let step = f.resume(epoch, outcome_in(&outcome)).map_err(call_err)?;
        Ok(step_out(&mut f, step))
    }

    fn read_call_input(&self, epoch: u64, fd: u32, max: u32) -> Result<Vec<u8>, T::KernelError> {
        let mut f = self.0.borrow_mut();
        let d = f.call_input_chunk(epoch, fd, max as usize).map_err(call_err)?;
        let bytes = drain_copy(d).map_err(mem_err)?;
        f.consume_call_input(bytes.len());
        Ok(bytes)
    }

    fn push_call_data(&self, epoch: u64, channel: T::OutChannel, chunk: Vec<u8>) -> Result<(), T::KernelError> {
        self.0.borrow_mut().push_call_data(epoch, chan_in(&channel), &chunk).map_err(call_err)
    }

    fn assign_variable(&self, epoch: u64, name: Vec<u8>, value: Vec<u8>, subscript: Option<Vec<u8>>) -> Result<(), T::KernelError> {
        self.0.borrow_mut().assign_variable(epoch, &name, &value, subscript.as_deref()).map_err(call_err)
    }

    fn set_call_env(&self, epoch: u64, env: Vec<T::EnvVar>) -> Result<(), T::KernelError> {
        let e = env_in(&env)?;
        self.0.borrow_mut().set_call_env(epoch, &e).map_err(call_err)
    }

    fn cancel(&self, reason: T::CancelReason) -> T::ExecResult {
        let r = match reason {
            T::CancelReason::Aborted => fiber::CancelReason::Aborted,
            T::CancelReason::Deadline => fiber::CancelReason::Deadline,
            T::CancelReason::Limit => fiber::CancelReason::Limit,
        };
        let result = self.0.borrow_mut().cancel(r);
        mem::reserve_scope(|| result_out(&result, 0, 0)).unwrap_or_else(|_| result_header(&result))
    }

    fn take_stdout(&self, max: u32) -> Vec<u8> {
        take_output(&mut self.0.borrow_mut(), false, max as usize)
    }

    fn take_stderr(&self, max: u32) -> Vec<u8> {
        take_output(&mut self.0.borrow_mut(), true, max as usize)
    }
    fn take_trace(&self, _max: u32) -> Vec<T::TraceEvent> {
        Vec::new()
    }

    fn take_coverage(&self, max: u32) -> Vec<u16> {
        let mut f = self.0.borrow_mut();
        let hits = copy(f.coverage_hits((max as usize).min(1 << 19))).unwrap_or_default();
        f.consume_coverage(hits.len());
        hits
    }
}

fn take_output(f: &mut Fiber, stderr: bool, max: usize) -> Vec<u8> {
    let count = f.output_segment_len(stderr).min(max.min(1 << 20));
    let mut bytes = Vec::new();
    let mut reserve = |size| bytes.try_reserve_exact(size);
    let size = if reserve(count).is_ok() {
        count
    } else {
        let Some(size) = mem::reserve_scope(|| [16 << 10, 1024].into_iter()
            .map(|n| n.min(count)).find(|&n| reserve(n).is_ok())) else { return bytes; };
        size
    };
    f.drain_output(stderr, size, |s| bytes.extend_from_slice(s));
    bytes
}

fn all_output(f: &mut Fiber, stderr: bool) -> Result<Vec<u8>, MemError> {
    let len = f.output_len(stderr);
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| MemError::Ceiling)?;
    f.drain_output(stderr, len, |s| bytes.extend_from_slice(s));
    Ok(bytes)
}

// --------------------------------------------------------------------------------------- oneshot

/// Minimal `exec-oneshot` (the session drivers of L1c build on it): a throwaway volume and shell, the
/// script run to completion; a host call is a `bad-state`.
fn oneshot(req: T::OneshotReq) -> Result<T::OneshotResult, T::KernelError> {
    configured()?;
    if req.config.trace || req.config.coverage {
        return Err(kerr(T::KernelErrorCode::InvalidConfig, "trace and coverage have no channel in exec-oneshot"));
    }
    let overrides = overrides_of(&req.config.limits)?;
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
    let sh = rc(create_shell(vfs.clone(), &req.config)?)?;
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
    let stdout = all_output(&mut f, false).map_err(mem_err)?;
    let stderr = all_output(&mut f, true).map_err(mem_err)?;
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
                        changed.try_reserve(1).map_err(|_| mem_err(MemError::Ceiling))?;
                        changed.push(T::OneshotFile { path: copy(p).map_err(mem_err)?, data: copy(&d).map_err(mem_err)?, mode: s.mode });
                    }
                }
            }
        }
        for x in &req.files {
            if !paths.iter().any(|p| p.as_slice() == x.path.as_slice()) {
                removed.try_reserve(1).map_err(|_| mem_err(MemError::Ceiling))?;
                removed.push(copy(&x.path).map_err(mem_err)?);
            }
        }
    }
    let outcome = result_out(&result, changed.len(), removed.len()).map_err(mem_err)?;
    Ok(T::OneshotResult { outcome, stdout, stderr, changed, removed })
}

super::kernel::export!(Kernel with_types_in super::kernel);
