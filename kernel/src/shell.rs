//! The `shell` resource: immutable configuration plus the persistent state read by `env()` and `cwd()`.
use crate::fs::Vfs;
use crate::limits::{self, Resolved};
use crate::mem::{Budget, KBytes, KRc, KString, KVec, LimitId, MemError, MemResult};
use crate::util::{bytes_in, str_in, Env};
use core::cell::{Cell, RefCell};

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

/// Configuration borrowed from the caller.
pub struct Config<'a> {
    pub profile: usize,
    pub overrides: &'a [(&'a str, u64)],
    pub grants: u8,
    pub custom_commands: &'a [&'a str],
    pub env: &'a [(&'a [u8], &'a [u8], bool)],
    pub cwd: Option<&'a [u8]>,
}

pub struct Shell {
    pub root: KRc<Vfs>,
    pub lim: Resolved,
    pub grants: u8,
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
        Ok(Shell { root, lim, grants: cfg.grants, custom: RefCell::new(custom), env, cwd, live, meta, virt_ms: Cell::new(0), rng: Cell::new(0x9E37_79B9_7F4A_7C15) })
    }

    pub fn register(&self, name: &str) -> MemResult<()> {
        let mut c = self.custom.borrow_mut();
        if c.iter().any(|s| s.as_str() == name) {
            return Ok(());
        }
        c.try_push(str_in(name, &self.meta)?)
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
