//! The `shell` resource: immutable configuration plus the persistent state read by `env()` and `cwd()`.
use crate::cmd::{CommandKind, CommandSpec, ALL_COMMANDS};
use crate::fs::{Code, Kind, Ok_, Op, Vfs};
use crate::limits::{self, Resolved};
use crate::mem::{Budget, KBytes, KMap, KRc, KString, KVec, LimitId, MemError, MemResult};
use crate::util::{bytes_in, fmt_in, str_in, Env};
use core::cell::{Cell, RefCell};
use core::fmt::Write;

pub const G_HOST_FS: u8 = 1;
pub const G_NETWORK: u8 = 2;
pub const G_REAL_CLOCK: u8 = 4;
pub const G_REAL_SLEEP: u8 = 8;
pub const G_ENTROPY: u8 = 16;
pub const G_CUSTOM: u8 = 32;

/// Why `Shell::create` refused.
#[derive(Debug)]
pub enum CreateError {
    Invalid(&'static str),
    Mem(MemError),
}

impl From<MemError> for CreateError {
    fn from(e: MemError) -> Self {
        CreateError::Mem(e)
    }
}

#[derive(Clone, Copy)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub gid: u32,
}

impl Default for ProcessInfo {
    fn default() -> Self { Self { pid: 1, ppid: 0, uid: 1000, gid: 1000 } }
}

#[cfg(target_arch = "wasm32")]
fn configure_headroom(bytes: u64) -> Result<(), CreateError> {
    use core::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static LARGEST_HEADROOM: AtomicU64 = AtomicU64::new(0);
    let largest = LARGEST_HEADROOM.load(Relaxed).max(bytes);
    crate::mem::set_headroom(largest).map_err(|_| CreateError::Invalid("headroom below current usage"))?;
    LARGEST_HEADROOM.store(largest, Relaxed);
    Ok(())
}

#[derive(Default)]
/// Configuration borrowed from the caller.
pub struct Config<'a> {
    pub profile: usize,
    pub overrides: &'a [(&'a str, u64)],
    pub grants: u8,
    pub default_layout: bool,
    pub commands: Option<&'a [&'a str]>,
    pub process_info: Option<ProcessInfo>,
    pub coverage: bool,
    pub custom_commands: &'a [&'a str],
    pub env: &'a [(&'a [u8], &'a [u8], bool)],
    pub cwd: Option<&'a [u8]>,
}

pub struct Shell {
    pub root: KRc<Vfs>,
    pub lim: Resolved,
    pub grants: u8,
    pub commands: KVec<&'static CommandSpec>,
    pub paths: RefCell<KMap<KBytes, KBytes>>,
    pub process_info: ProcessInfo,
    pub coverage: bool,
    pub custom: RefCell<KVec<KString>>,
    pub env: Env,
    pub cwd: KBytes,
    /// Root budget of live bytes (`maxLiveBytes`); every fiber of the shell charges a child of it.
    pub live: Budget,
    /// Unbudgeted scratch for small bookkeeping.
    pub meta: Budget,
    pub virt_ms: Cell<u64>,
    pub rng: Cell<u64>,
}

impl Shell {
    /// `ceiling` is the configured instance ceiling, used to refuse byte caps that cannot fit.
    pub fn create(root: KRc<Vfs>, cfg: &Config<'_>, ceiling: u64) -> Result<Shell, CreateError> {
        let mut lim = Resolved::of_profile(cfg.profile);
        for (name, v) in cfg.overrides {
            if !lim.set(name, *v) {
                return Err(CreateError::Invalid("unknown limit name"));
            }
        }
        if lim.byte_sum() > ceiling {
            return Err(CreateError::Invalid("byte limits do not fit the memory ceiling"));
        }
        #[cfg(target_arch = "wasm32")]
        configure_headroom(lim.headroom())?;
        let meta = Budget::root(LimitId::HEADROOM, u64::MAX)?;
        let live = Budget::root(limits::id_of_index(limits::I_LIVE_BYTES), lim.vals[limits::I_LIVE_BYTES])?;
        let mut env = Env::new_in(&meta);
        for (n, v, x) in cfg.env {
            env.set(n, v, *x)?;
        }
        let cwd = bytes_in(cfg.cwd.unwrap_or(b"/"), &meta)?;
        let mut custom = KVec::new_in(&meta);
        for c in cfg.custom_commands {
            custom.try_push(str_in(c, &meta)?)?;
        }
        let mut commands = KVec::new_in(&meta);
        for &spec in ALL_COMMANDS {
            if cfg.commands.is_none_or(|names| names.contains(&spec.name))
                && (spec.kind == CommandKind::Standard || cfg.grants & G_NETWORK != 0)
            {
                commands.try_push(spec)?;
            }
        }
        let shell = Shell {
            root, lim, grants: cfg.grants, commands, paths: RefCell::new(KMap::new_in(&meta)),
            process_info: cfg.process_info.unwrap_or_default(), coverage: cfg.coverage,
            custom: RefCell::new(custom), env, cwd, live, meta,
            virt_ms: Cell::new(0), rng: Cell::new(0x9E37_79B9_7F4A_7C15),
        };
        shell.layout(cfg.default_layout)?;
        Ok(shell)
    }

    pub fn register(&self, name: &str) -> MemResult<()> {
        let mut c = self.custom.borrow_mut();
        if c.iter().any(|s| s.as_str() == name) {
            return Ok(());
        }
        c.try_push(str_in(name, &self.meta)?)?;
        self.stub(name);
        Ok(())
    }

    fn stub(&self, name: &str) {
        if name.is_empty() || name.contains('/') || matches!(name, "." | "..") {
            return;
        }
        if let Ok(data) = fmt_in(&self.meta, |s| write!(s, "#!/bin/bash\n# Built-in command: {name}\n")) {
            for dir in ["/bin", "/usr/bin"] {
                if let Ok(path) = fmt_in(&self.meta, |s| write!(s, "{dir}/{name}")) {
                    let _ = self.file(path.as_str().as_bytes(), data.as_str().as_bytes());
                }
            }
        }
    }

    fn file(&self, path: &[u8], data: &[u8]) -> Result<Ok_, Code> {
        self.root.op(Op::WriteFile { path, data, offset: Some(0), truncate: true, mode: Some(0o644), mtime_ms: None })
    }

    fn mkdir(&self, path: &[u8]) -> Result<Ok_, Code> {
        self.root.op(Op::Mkdir { path, recursive: true, mode: Some(0o755) })
    }

    fn layout(&self, default_layout: bool) -> Result<(), CreateError> {
        // A host mount over `/` owns the layout: the facade seeds it through the adapter.
        if self.root.resolve_host_mount(b"/").is_some() {
            return Ok(());
        }
        let dirs: [&[u8]; 8] = [b"/bin", b"/usr", b"/usr/bin", b"/dev", b"/dev/fd", b"/proc", b"/proc/self", b"/proc/self/fd"];
        let invalid = |_| CreateError::Invalid("cannot initialize system filesystem layout");
        for dir in dirs {
            if matches!(self.root.stat_normalized(dir), Ok(s) if s.kind == Kind::File) {
                return Err(CreateError::Invalid("system directory is a file"));
            }
        }
        self.mkdir(b"/bin").map_err(invalid)?;
        self.mkdir(b"/usr/bin").map_err(invalid)?;
        if default_layout {
            self.mkdir(b"/home/user").map_err(invalid)?;
            self.mkdir(b"/tmp").map_err(invalid)?;
        }
        self.mkdir(b"/dev/fd").map_err(invalid)?;
        for path in [b"/dev/null".as_slice(), b"/dev/zero", b"/dev/stdin", b"/dev/stdout", b"/dev/stderr"] {
            self.file(path, b"").map_err(invalid)?;
        }
        self.mkdir(b"/proc/self/fd").map_err(invalid)?;
        let ProcessInfo { pid, ppid, uid, gid } = self.process_info;
        let status = fmt_in(&self.meta, |s| write!(s,
            "Name:\tbash\nState:\tR (running)\nPid:\t{pid}\nPPid:\t{ppid}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nGid:\t{gid}\t{gid}\t{gid}\t{gid}\n"))?;
        let proc: [(&[u8], &[u8]); 8] = [
            (b"/proc/version", b"Linux version 5.15.0-generic (just-bash) #1 SMP PREEMPT\n"),
            (b"/proc/self/exe", b"/bin/bash"),
            (b"/proc/self/cmdline", b"bash\0"),
            (b"/proc/self/comm", b"bash\n"),
            (b"/proc/self/status", status.as_str().as_bytes()),
            (b"/proc/self/fd/0", b"/dev/stdin"),
            (b"/proc/self/fd/1", b"/dev/stdout"),
            (b"/proc/self/fd/2", b"/dev/stderr"),
        ];
        for (path, data) in proc {
            self.file(path, data).map_err(invalid)?;
        }
        if self.cwd.as_slice() != b"/" {
            let _ = self.mkdir(&self.cwd);
        }
        for command in self.commands.iter() {
            self.stub(command.name);
        }
        for name in self.custom.borrow().iter() {
            self.stub(name.as_str());
        }
        Ok(())
    }

    pub fn command(&self, name: &[u8]) -> Option<&'static CommandSpec> {
        self.commands.binary_search_by(|spec| spec.name.as_bytes().cmp(name)).ok().map(|i| self.commands[i])
    }

    pub fn is_custom(&self, name: &[u8]) -> bool {
        self.custom.borrow().iter().any(|s| s.as_str().as_bytes() == name)
    }

    pub fn granted(&self, g: u8) -> bool {
        self.grants & g != 0
    }

    /// Next value of the fixed-seed generator used when `entropy` is not granted.
    pub fn next_random(&self) -> u64 {
        let mut x = self.rng.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng.set(x);
        x
    }
}
