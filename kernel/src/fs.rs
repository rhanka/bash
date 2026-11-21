//! First slice of the filesystem (spec section 6): an in-memory tree keyed by normalized absolute paths.
//! Content is charged to the volume budget (`maxFileSystemBytes`). Every operation outside the slice is
//! `unsupported` and belongs to L7a.
use crate::mem::{Budget, KChunks, KBytes, KMap, KVec, LimitId, MemError, MemResult};
use crate::util::bytes_in;
use core::cell::RefCell;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Code {
    NotFound,
    Exists,
    NotDirectory,
    IsDirectory,
    NotEmpty,
    Permission,
    Invalid,
    NoSpace,
    Unsupported,
}

#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub kind: Kind,
    pub size: u64,
    pub mode: u32,
    pub mtime_ms: i64,
    pub ino: u64,
}

/// A request, borrowed from the caller.
pub enum Op<'a> {
    Stat(&'a [u8]),
    Lstat(&'a [u8]),
    Exists(&'a [u8]),
    ReadDir(&'a [u8]),
    AllPaths,
    ReadFile { path: &'a [u8], offset: u64, len: u32 },
    WriteFile { path: &'a [u8], data: &'a [u8], offset: Option<u64>, truncate: bool, mode: Option<u32>, mtime_ms: Option<i64> },
    Mkdir { path: &'a [u8], recursive: bool, mode: Option<u32> },
    Remove { path: &'a [u8], recursive: bool, force: bool },
    /// Any operation of the contract that this lot does not implement.
    Other,
}

pub enum Ok_ {
    None,
    Flag(bool),
    Info(Stat),
    Data(KBytes),
    Paths(KVec<KBytes>),
    Entries(KVec<(KBytes, Kind)>),
}

struct Node {
    kind: Kind,
    data: KChunks,
    mode: u32,
    mtime_ms: i64,
    ino: u64,
}

struct Inner {
    map: KMap<KBytes, Node>,
    next_ino: u64,
}

/// A volume.
pub struct Vfs {
    budget: Budget,
    scratch: Budget,
    inner: RefCell<Inner>,
}

fn map_mem(e: MemError) -> Code {
    let _ = e;
    Code::NoSpace
}

impl Vfs {
    /// An empty volume holding only `/`. `fs_limit` is the id reported when the budget refuses.
    pub fn create(max_bytes: u64, fs_limit: LimitId) -> MemResult<Vfs> {
        let budget = Budget::root(fs_limit, max_bytes)?;
        let scratch = Budget::root(LimitId::HEADROOM, u64::MAX)?;
        let mut map = KMap::new_in(&budget);
        let root = bytes_in(b"/", &budget)?;
        map.try_insert(root, Node { kind: Kind::Dir, data: KChunks::new_in(&budget), mode: 0o755, mtime_ms: 0, ino: 1 })?;
        Ok(Vfs { budget, scratch, inner: RefCell::new(Inner { map, next_ino: 2 }) })
    }

    pub fn used_bytes(&self) -> u64 {
        self.budget.used()
    }

    /// Normalizes a path: absolute, `.` and `..` collapsed, never above `/`.
    pub(crate) fn resolve(&self, cwd: &[u8], path: &[u8]) -> Result<KBytes, Code> {
        if path.starts_with(b"/") {
            return self.norm(path);
        }
        let mut full = bytes_in(cwd, &self.scratch).map_err(map_mem)?;
        full.try_push(b'/').map_err(map_mem)?;
        full.try_extend_from_slice(path).map_err(map_mem)?;
        self.norm(&full)
    }

    fn norm(&self, path: &[u8]) -> Result<KBytes, Code> {
        let mut out = KBytes::new_in(&self.scratch);
        for comp in path.split(|&c| c == b'/') {
            if comp.is_empty() || comp == b"." {
                continue;
            }
            if comp == b".." {
                if let Some(p) = out.iter().rposition(|&c| c == b'/') {
                    out.truncate(p);
                }
                continue;
            }
            out.try_push(b'/').map_err(map_mem)?;
            out.try_extend_from_slice(comp).map_err(map_mem)?;
        }
        if out.is_empty() {
            out.try_push(b'/').map_err(map_mem)?;
        }
        Ok(out)
    }

    /// Parent of a normalized path (`/` for top-level entries). `None` for `/` itself.
    fn parent<'a>(&self, p: &'a [u8]) -> Option<&'a [u8]> {
        if p == b"/" {
            return None;
        }
        let i = p.iter().rposition(|&c| c == b'/')?;
        Some(if i == 0 { &p[..1] } else { &p[..i] })
    }

    fn prefix_of(p: &[u8], scratch: &Budget) -> Result<KBytes, Code> {
        let mut k = KBytes::new_in(scratch);
        k.try_extend_from_slice(p).map_err(map_mem)?;
        if p != b"/" {
            k.try_push(b'/').map_err(map_mem)?;
        }
        Ok(k)
    }

    pub(crate) fn stat_normalized(&self, path: &[u8]) -> Result<Stat, Code> {
        self.inner.borrow().map.get(path).map(stat_of).ok_or(Code::NotFound)
    }

    pub fn op(&self, op: Op<'_>) -> Result<Ok_, Code> {
        match op {
            Op::Stat(p) | Op::Lstat(p) => {
                let p = self.norm(p)?;
                self.stat_normalized(&p).map(Ok_::Info)
            }
            Op::Exists(p) => {
                let p = self.norm(p)?;
                Ok(Ok_::Flag(self.inner.borrow().map.contains_key(p.as_slice())))
            }
            Op::ReadDir(p) => self.read_dir(p),
            Op::AllPaths => {
                let inner = self.inner.borrow();
                let mut out = KVec::new_in(&self.scratch);
                for k in inner.map.keys() {
                    out.try_push(bytes_in(k, &self.scratch).map_err(map_mem)?).map_err(map_mem)?;
                }
                Ok(Ok_::Paths(out))
            }
            Op::ReadFile { path, offset, len } => {
                let p = self.norm(path)?;
                let inner = self.inner.borrow();
                let n = inner.map.get(p.as_slice()).ok_or(Code::NotFound)?;
                if n.kind == Kind::Dir {
                    return Err(Code::IsDirectory);
                }
                let len = (n.data.len().saturating_sub(offset)).min(len as u64) as usize;
                let mut bytes = KBytes::new_in(&self.scratch);
                bytes.try_resize(len, 0).map_err(map_mem)?;
                n.data.read_at(offset, bytes.as_mut_slice());
                Ok(Ok_::Data(bytes))
            }
            Op::WriteFile { path, data, offset, truncate, mode, mtime_ms } => self.write_file(path, data, offset, truncate, mode, mtime_ms),
            Op::Mkdir { path, recursive, mode } => self.mkdir(path, recursive, mode.unwrap_or(0o755)),
            Op::Remove { path, recursive, force } => self.remove(path, recursive, force),
            Op::Other => Err(Code::Unsupported),
        }
    }

    fn read_dir(&self, p: &[u8]) -> Result<Ok_, Code> {
        let p = self.norm(p)?;
        let inner = self.inner.borrow();
        let n = inner.map.get(p.as_slice()).ok_or(Code::NotFound)?;
        if n.kind != Kind::Dir {
            return Err(Code::NotDirectory);
        }
        let prefix = Self::prefix_of(&p, &self.scratch)?;
        let mut out = KVec::new_in(&self.scratch);
        for (k, v) in inner.map.iter() {
            if k.len() > prefix.len() && k.starts_with(prefix.as_slice()) {
                let rest = &k[prefix.len()..];
                if !rest.contains(&b'/') {
                    let name = bytes_in(rest, &self.scratch).map_err(map_mem)?;
                    out.try_push((name, v.kind)).map_err(map_mem)?;
                }
            }
        }
        Ok(Ok_::Entries(out))
    }

    fn write_file(&self, path: &[u8], data: &[u8], offset: Option<u64>, truncate: bool, mode: Option<u32>, mtime_ms: Option<i64>) -> Result<Ok_, Code> {
        let p = self.norm(path)?;
        let mut inner = self.inner.borrow_mut();
        if p.as_slice() == b"/" {
            return Err(Code::IsDirectory);
        }
        let parent = self.parent(&p).ok_or(Code::Invalid)?;
        match inner.map.get(parent) {
            None => return Err(Code::NotFound),
            Some(n) if n.kind != Kind::Dir => return Err(Code::NotDirectory),
            Some(_) => {}
        }
        let write = |n: &mut Node| -> Result<(), Code> {
            if n.kind == Kind::Dir {
                return Err(Code::IsDirectory);
            }
            let len = n.data.len();
            let start = match offset {
                Some(o) => o,
                None if truncate => 0,
                None => len,
            };
            let end = start.checked_add(data.len() as u64).ok_or(Code::NoSpace)?;
            // Chunk growth reserves first, so a refused write leaves the existing file intact.
            n.data.try_write_at(start, data).map_err(map_mem)?;
            if truncate {
                n.data.truncate(end);
            }
            if let Some(m) = mode {
                n.mode = m;
            }
            if let Some(t) = mtime_ms {
                n.mtime_ms = t;
            }
            Ok(())
        };
        if let Some(n) = inner.map.get_mut(p.as_slice()) {
            write(n)?;
        } else {
            let ino = inner.next_ino;
            let mut node = Node { kind: Kind::File, data: KChunks::new_in(&self.budget), mode: 0o644, mtime_ms: 0, ino };
            write(&mut node)?;
            let key = bytes_in(&p, &self.budget).map_err(map_mem)?;
            inner.map.try_insert(key, node).map_err(map_mem)?;
            inner.next_ino += 1;
        }
        Ok(Ok_::None)
    }

    #[cfg(feature = "test-commands")]
    pub(crate) fn allocate_file(&self, path: &[u8], len: u64) -> MemResult<bool> {
        let path = self.norm(path).map_err(|_| MemError::Ceiling)?;
        let mut inner = self.inner.borrow_mut();
        if inner.map.get(path.as_slice()).is_some_and(|n| n.kind == Kind::Dir) {
            return Ok(false);
        }
        if len > self.budget.cap().saturating_sub(self.budget.used()) {
            return Err(MemError::Limit(self.budget.limit()));
        }
        let path = bytes_in(&path, &self.budget)?;
        let ino = inner.next_ino;
        let mut data = KChunks::new_in(&self.budget);
        // Extending an empty sequence zero-fills its chunks after the allocation succeeds.
        data.try_write_at(len, &[])?;
        inner.map.try_insert(path, Node { kind: Kind::File, data, mode: 0o644, mtime_ms: 0, ino })?;
        inner.next_ino += 1;
        Ok(true)
    }

    fn mkdir(&self, path: &[u8], recursive: bool, mode: u32) -> Result<Ok_, Code> {
        let p = self.norm(path)?;
        let mut inner = self.inner.borrow_mut();
        if let Some(n) = inner.map.get(p.as_slice()) {
            return if recursive && n.kind == Kind::Dir { Ok(Ok_::None) } else { Err(Code::Exists) };
        }
        let mut chain: KVec<usize> = KVec::new_in(&self.scratch);
        let mut cur = p.as_slice();
        loop {
            let parent = match self.parent(cur) {
                Some(x) => x,
                None => break,
            };
            match inner.map.get(parent) {
                Some(n) if n.kind == Kind::Dir => break,
                Some(_) => return Err(Code::NotDirectory),
                None => {
                    if !recursive {
                        return Err(Code::NotFound);
                    }
                    chain.try_push(parent.len()).map_err(map_mem)?;
                    cur = parent;
                }
            }
        }
        // Create the missing ancestors from the top, then the directory itself.
        while let Some(len) = chain.pop() {
            self.insert_dir(&mut inner, &p[..len], mode)?;
        }
        self.insert_dir(&mut inner, &p, mode)?;
        Ok(Ok_::None)
    }

    fn insert_dir(&self, inner: &mut Inner, path: &[u8], mode: u32) -> Result<(), Code> {
        let key = bytes_in(path, &self.budget).map_err(map_mem)?;
        let ino = inner.next_ino;
        inner.map.try_insert(key, Node { kind: Kind::Dir, data: KChunks::new_in(&self.budget), mode, mtime_ms: 0, ino }).map_err(map_mem)?;
        inner.next_ino += 1;
        Ok(())
    }

    fn remove(&self, path: &[u8], recursive: bool, force: bool) -> Result<Ok_, Code> {
        let p = self.norm(path)?;
        let mut inner = self.inner.borrow_mut();
        let Some(n) = inner.map.get(p.as_slice()) else {
            return if force { Ok(Ok_::None) } else { Err(Code::NotFound) };
        };
        if p.as_slice() == b"/" {
            return Err(Code::Permission);
        }
        if n.kind == Kind::Dir {
            let prefix = Self::prefix_of(&p, &self.scratch)?;
            let mut doomed: KVec<KBytes> = KVec::new_in(&self.scratch);
            for k in inner.map.keys() {
                if k.starts_with(prefix.as_slice()) {
                    if !recursive {
                        return Err(Code::NotEmpty);
                    }
                    doomed.try_push(bytes_in(k, &self.scratch).map_err(map_mem)?).map_err(map_mem)?;
                }
            }
            for k in doomed.iter() {
                inner.map.remove(k.as_slice());
            }
        }
        inner.map.remove(p.as_slice());
        Ok(Ok_::None)
    }
}

fn stat_of(n: &Node) -> Stat {
    Stat { kind: n.kind, size: n.data.len(), mode: n.mode, mtime_ms: n.mtime_ms, ino: n.ino }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(max: u64) -> Vfs {
        crate::mem::testing::reset(1 << 30);
        Vfs::create(max, LimitId(30)).unwrap()
    }

    fn write(v: &Vfs, p: &[u8], d: &[u8]) -> Result<Ok_, Code> {
        v.op(Op::WriteFile { path: p, data: d, offset: Some(0), truncate: true, mode: None, mtime_ms: None })
    }

    fn read(v: &Vfs, p: &[u8]) -> Result<std::vec::Vec<u8>, Code> {
        match v.op(Op::ReadFile { path: p, offset: 0, len: 1 << 20 })? {
            Ok_::Data(d) => Ok(d.as_slice().to_vec()),
            _ => unreachable!(),
        }
    }

    #[test]
    fn write_read_stat_and_paths() {
        let v = vol(1 << 20);
        write(&v, b"/a.txt", b"hello").unwrap();
        assert_eq!(read(&v, b"/a.txt").unwrap(), b"hello");
        assert_eq!(read(&v, b"//x/../a.txt/.").unwrap(), b"hello");
        match v.op(Op::Stat(b"/a.txt")).unwrap() {
            Ok_::Info(s) => assert_eq!((s.kind, s.size, s.mode), (Kind::File, 5, 0o644)),
            _ => panic!(),
        }
        assert!(matches!(write(&v, b"/no/dir/f", b"x"), Err(Code::NotFound)));
        assert!(matches!(v.op(Op::Stat(b"/zzz")), Err(Code::NotFound)));
        assert!(matches!(v.op(Op::Exists(b"/../..")), Ok(Ok_::Flag(true))));
    }

    #[test]
    fn append_and_offset_and_truncate() {
        let v = vol(1 << 20);
        write(&v, b"/f", b"abc").unwrap();
        v.op(Op::WriteFile { path: b"/f", data: b"de", offset: None, truncate: false, mode: None, mtime_ms: None }).unwrap();
        assert_eq!(read(&v, b"/f").unwrap(), b"abcde");
        v.op(Op::WriteFile { path: b"/f", data: b"X", offset: Some(7), truncate: false, mode: None, mtime_ms: None }).unwrap();
        assert_eq!(read(&v, b"/f").unwrap(), b"abcde\0\0X");
        write(&v, b"/f", b"z").unwrap();
        assert_eq!(read(&v, b"/f").unwrap(), b"z");
    }

    #[test]
    fn mkdir_read_dir_remove() {
        let v = vol(1 << 20);
        v.op(Op::Mkdir { path: b"/d/e/f", recursive: true, mode: None }).unwrap();
        assert!(matches!(v.op(Op::Mkdir { path: b"/d", recursive: false, mode: None }), Err(Code::Exists)));
        assert!(matches!(v.op(Op::Mkdir { path: b"/x/y", recursive: false, mode: None }), Err(Code::NotFound)));
        write(&v, b"/d/b", b"1").unwrap();
        write(&v, b"/d/a", b"1").unwrap();
        match v.op(Op::ReadDir(b"/d")).unwrap() {
            Ok_::Entries(e) => {
                let names: std::vec::Vec<_> = e.iter().map(|(n, k)| (n.as_slice().to_vec(), *k)).collect();
                assert_eq!(names, [(b"a".to_vec(), Kind::File), (b"b".to_vec(), Kind::File), (b"e".to_vec(), Kind::Dir)]);
            }
            _ => panic!(),
        }
        assert!(matches!(v.op(Op::Remove { path: b"/d", recursive: false, force: false }), Err(Code::NotEmpty)));
        v.op(Op::Remove { path: b"/d", recursive: true, force: false }).unwrap();
        assert!(matches!(v.op(Op::Stat(b"/d/e")), Err(Code::NotFound)));
        assert!(matches!(v.op(Op::Remove { path: b"/d", recursive: false, force: false }), Err(Code::NotFound)));
        v.op(Op::Remove { path: b"/d", recursive: false, force: true }).unwrap();
        assert!(matches!(v.op(Op::Other), Err(Code::Unsupported)));
    }

    #[test]
    fn budget_gives_no_space() {
        let v = vol(4096);
        let contents = [42; 64];
        write(&v, b"/a", &contents).unwrap();
        let used = v.used_bytes();
        assert!(matches!(write(&v, b"/b", &[0u8; 4096]), Err(Code::NoSpace)));
        assert!(matches!(v.op(Op::Exists(b"/b")), Ok(Ok_::Flag(false))));
        assert!(matches!(v.op(Op::WriteFile {
            path: b"/a", data: &[0; 4096], offset: Some(7), truncate: true, mode: Some(0o600), mtime_ms: Some(17),
        }), Err(Code::NoSpace)));
        assert_eq!(v.used_bytes(), used);
        assert_eq!(read(&v, b"/a").unwrap(), contents);
        assert!(matches!(v.op(Op::Stat(b"/a")), Ok(Ok_::Info(Stat { mode: 0o644, mtime_ms: 0, .. }))));
    }

    #[cfg(feature = "test-commands")]
    #[test]
    fn test_file_allocator_charges_the_key_and_preserves_directories() {
        let v = vol(4096);
        assert_eq!(v.allocate_file(b"/", 0), Ok(false));
        assert_eq!(v.stat_normalized(b"/").unwrap().kind, Kind::Dir);
        let path = [b"/".as_slice(), &[b'x'; 4096]].concat();
        let used = v.used_bytes();
        assert_eq!(v.allocate_file(&path, 0), Err(MemError::Limit(LimitId(30))));
        assert_eq!(v.used_bytes(), used);
        assert!(matches!(v.stat_normalized(&path), Err(Code::NotFound)));
    }
}
