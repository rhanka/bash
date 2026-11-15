//! Test-only commands (spec 5.3). Compiled only with the `test-commands` feature, so the release kernel
//! does not contain these names.
use super::*;
use crate::fs::{Code, Ok_, Op};
use crate::shell::{G_ENTROPY, G_HOST_FS, G_NETWORK, G_REAL_CLOCK, G_REAL_SLEEP};
use crate::util::parse_u64;

const T_HOSTFS: u8 = 1;
const T_LAZY: u8 = 2;
const T_FETCH: u8 = 3;
const T_SLEEP: u8 = 4;
const T_CLOCK: u8 = 5;
const T_ENTROPY: u8 = 6;

impl Fiber {
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

    fn t_ok(&mut self, r: Result<(), Fail>) -> Flow {
        match r {
            Ok(()) => Flow::Next,
            Err(f) => Flow::Fail(f),
        }
    }

    pub(super) fn exec_test(&mut self, name: &[u8], args: &[&[u8]]) -> Option<Flow> {
        let num = |i: usize| args.get(i).and_then(|a| parse_u64(a));
        let flow = match name {
            b"__out" | b"__err" => {
                let Some(n) = num(0) else { return Some(self.t_bad(name)) };
                let c = if name == b"__out" { Chan::Stdout } else { Chan::Stderr };
                self.last_exit = 0;
                let r = self.fill_out(c, b'a', n);
                self.t_ok(r)
            }
            b"__stdin" => {
                let start = self.stdin_pos.min(self.stdin.len());
                self.stdin_pos = self.stdin.len();
                let data = core::mem::replace(&mut self.stdin, KBytes::new_in(&self.scratch));
                let r = self.write_out(Chan::Stdout, &data[start..]);
                self.stdin = data;
                self.last_exit = 0;
                self.t_ok(r)
            }
            b"__alloc" => {
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
            b"__spin" => Flow::Spin,
            b"__exit" => {
                let Some(n) = num(0) else { return Some(self.t_bad(name)) };
                self.last_exit = n as i32;
                Flow::Exit
            }
            b"__setenv" => {
                let (Some(k), Some(v)) = (args.first(), args.get(1)) else { return Some(self.t_bad(name)) };
                match self.env.set(k, v, true) {
                    Ok(()) => {
                        self.last_exit = 0;
                        Flow::Next
                    }
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            b"__write" => {
                let (Some(p), Some(t)) = (args.first(), args.get(1)) else { return Some(self.t_bad(name)) };
                let r = self.shell.root.op(Op::WriteFile { path: p, data: t, offset: Some(0), truncate: true, mode: None, mtime_ms: None });
                self.fs_exit(r.map(|_| ()), p)
            }
            b"__cat" => {
                let Some(p) = args.first() else { return Some(self.t_bad(name)) };
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
            b"__hostfs" => {
                let (Some(id), Some(p)) = (num(0), args.get(1)) else { return Some(self.t_bad(name)) };
                if !self.shell.granted(G_HOST_FS) {
                    return Some(self.t_denied());
                }
                match bytes_in(p, &self.scratch) {
                    Ok(path) => Flow::Host(Pending::Test(T_HOSTFS, 0), HostReq::FsStat { mount_id: id as u32, path }),
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            b"__lazy" => {
                let Some(p) = args.first() else { return Some(self.t_bad(name)) };
                if !self.shell.granted(G_HOST_FS) {
                    return Some(self.t_denied());
                }
                match bytes_in(p, &self.scratch) {
                    Ok(path) => Flow::Host(Pending::Test(T_LAZY, 0), HostReq::LazyFile(path)),
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            b"__fetch" => {
                let Some(u) = args.first() else { return Some(self.t_bad(name)) };
                if !self.shell.granted(G_NETWORK) {
                    return Some(self.t_denied());
                }
                match bytes_in(u, &self.scratch) {
                    Ok(url) => Flow::Host(Pending::Test(T_FETCH, 0), HostReq::Fetch { url }),
                    Err(e) => Flow::Fail(Fail::Mem(e)),
                }
            }
            b"__sleep" => {
                let Some(ms) = num(0) else { return Some(self.t_bad(name)) };
                if self.shell.granted(G_REAL_SLEEP) {
                    Flow::Host(Pending::Test(T_SLEEP, 0), HostReq::Sleep(ms))
                } else {
                    self.shell.virt_ms.set(self.shell.virt_ms.get().saturating_add(ms));
                    self.last_exit = 0;
                    Flow::Next
                }
            }
            b"__clock" => {
                if self.shell.granted(G_REAL_CLOCK) {
                    Flow::Host(Pending::Test(T_CLOCK, 0), HostReq::Clock)
                } else {
                    let ms = self.shell.virt_ms.get();
                    self.last_exit = 0;
                    let r = self.say(Chan::Stdout, |s| write!(s, "{ms}\n"));
                    self.t_ok(r)
                }
            }
            b"__entropy" => {
                let Some(n) = num(0) else { return Some(self.t_bad(name)) };
                if self.shell.granted(G_ENTROPY) {
                    Flow::Host(Pending::Test(T_ENTROPY, 0), HostReq::Entropy(n as u32))
                } else {
                    // Fixed-seed generator: advance it by the words that would have been produced.
                    for _ in 0..n.min(1 << 16).div_ceil(8) {
                        self.shell.next_random();
                    }
                    self.last_exit = 0;
                    let r = self.say(Chan::Stdout, |s| write!(s, "{n}\n"));
                    self.t_ok(r)
                }
            }
            b"__trap" => {
                #[cfg(target_arch = "wasm32")]
                core::arch::wasm32::unreachable();
                #[cfg(not(target_arch = "wasm32"))]
                panic!("__trap");
            }
            _ => return None,
        };
        Some(flow)
    }

    fn fs_exit(&mut self, r: Result<(), Code>, path: &[u8]) -> Flow {
        match r {
            Ok(()) => {
                self.last_exit = 0;
                Flow::Next
            }
            Err(c) => {
                self.last_exit = 1;
                let p = core::str::from_utf8(path).unwrap_or("?");
                let msg = match c {
                    Code::NotFound => "No such file or directory",
                    Code::NoSpace => "No space left on device",
                    Code::IsDirectory => "Is a directory",
                    _ => "error",
                };
                let r = self.say(Chan::Stderr, |s| write!(s, "cat: {p}: {msg}\n"));
                self.t_ok(r)
            }
        }
    }

    pub(super) fn complete_test(&mut self, code: u8, _aux: u64, o: Outcome<'_>) -> Result<(), CompleteError> {
        let fail = CompleteError::Fail;
        if let Outcome::Denied = o {
            self.last_exit = 126;
            return self.write_out(Chan::Stderr, b"denied\n").map_err(fail);
        }
        self.last_exit = 0;
        match (code, o) {
            (T_HOSTFS, Outcome::Fs(r)) => {
                let t = r.unwrap_or("ok");
                self.say(Chan::Stdout, |s| write!(s, "{t}\n")).map_err(fail)
            }
            (T_LAZY, Outcome::Completed) => {
                let body = core::mem::replace(&mut self.body, KBytes::new_in(&self.scratch));
                let r = self.write_out(Chan::Stdout, &body);
                self.body = body;
                r.map_err(fail)
            }
            (T_FETCH, Outcome::Fetched(status)) => {
                self.say(Chan::Stdout, |s| write!(s, "{status}\n")).map_err(fail)?;
                let body = core::mem::replace(&mut self.body, KBytes::new_in(&self.scratch));
                let r = self.write_out(Chan::Stdout, &body);
                self.body = body;
                r.map_err(fail)
            }
            (T_FETCH, Outcome::NetFailed(c)) => {
                self.last_exit = 1;
                self.say(Chan::Stderr, |s| write!(s, "fetch: {c}\n")).map_err(fail)
            }
            (T_SLEEP, Outcome::Completed) => Ok(()),
            (T_CLOCK, Outcome::Clock(ms)) => self.say(Chan::Stdout, |s| write!(s, "{ms}\n")).map_err(fail),
            (T_ENTROPY, Outcome::Data(d)) => {
                let n = d.len();
                self.say(Chan::Stdout, |s| write!(s, "{n}\n")).map_err(fail)
            }
            (_, Outcome::Failed(m)) => {
                self.last_exit = 1;
                self.write_out(Chan::Stderr, m).map_err(fail)
            }
            (_, Outcome::LimitExceeded(n)) => {
                let name = str_in(n, &self.scratch).map_err(|e| CompleteError::Fail(Fail::Mem(e)))?;
                Err(CompleteError::Fail(Fail::Named(name)))
            }
            _ => Err(CompleteError::BadOutcome),
        }
    }
}
