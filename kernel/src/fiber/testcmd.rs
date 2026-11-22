//! Test-only commands (spec 5.3). Compiled only with the `test-commands` feature, so the release kernel
//! does not contain these names.
use super::*;
use crate::fs::{BatchOk, Code, Ok_, Op};
use crate::shell::{G_ENTROPY, G_HOST_FS, G_NETWORK, G_REAL_CLOCK, G_REAL_SLEEP};
use crate::util::parse_u64;

/// The names of the test-only commands, matched by index so that they exist as data in this build only.
const CMDS: [&[u8]; 20] = [b"__out", b"__err", b"__stdin", b"__alloc", b"__spin", b"__exit", b"__setenv", b"__write", b"__cat", b"__hostfs", b"__lazy", b"__fetch", b"__sleep", b"__clock", b"__entropy", b"__trap", b"__fsalloc", b"__work", b"__walk", b"__fsbatch"];
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
            _ => return None,
        };
        Some(flow)
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
            (T_CLOCK, Outcome::Clock(ms)) => self.say(Chan::Stdout, |s| write!(s, "{ms}\n")).map_err(fail),
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
