//! Pipeline probe: a minimal no_std component used to prove the link and to exercise every gate.
//! The canonical-ABI glue is written by hand (EVOL D16 fallback): the generated glue needs the `alloc`
//! crate and a global allocator, which this probe must not use.
#![cfg_attr(target_arch = "wasm32", no_std)]

#[cfg(target_arch = "wasm32")]
mod wasm {
    use core::arch::wasm32::{memory_grow, unreachable};
    use core::cell::UnsafeCell;

    const PAGE: usize = 65536;

    #[panic_handler]
    fn panic(_info: &core::panic::PanicInfo) -> ! {
        unreachable()
    }

    /// Bump allocator state. The module is single-threaded.
    struct Bump {
        cur: UnsafeCell<usize>,
        end: UnsafeCell<usize>,
        ret: UnsafeCell<[u32; 2]>,
    }
    unsafe impl Sync for Bump {}
    static BUMP: Bump = Bump {
        cur: UnsafeCell::new(0),
        end: UnsafeCell::new(0),
        ret: UnsafeCell::new([0; 2]),
    };

    /// Returns a non-null pointer to `size` bytes aligned to `align`; traps when memory is exhausted.
    unsafe fn bump_alloc(size: usize, align: usize) -> *mut u8 {
        if align == 0 || !align.is_power_of_two() {
            unreachable();
        }
        if size == 0 {
            return align as *mut u8;
        }
        let cur = &mut *BUMP.cur.get();
        let end = &mut *BUMP.end.get();
        let mut start = match cur.checked_add(align - 1) {
            Some(v) => v & !(align - 1),
            None => unreachable(),
        };
        if start == 0 || start.checked_add(size).map_or(true, |e| e > *end) {
            let need = match size.checked_add(align) {
                Some(n) => n,
                None => unreachable(),
            };
            let pages = need.div_ceil(PAGE);
            let first = memory_grow(0, pages);
            if first == usize::MAX {
                unreachable();
            }
            let base = first * PAGE;
            *end = base + pages * PAGE;
            start = match base.checked_add(align - 1) {
                Some(v) => v & !(align - 1),
                None => unreachable(),
            };
        }
        *cur = start + size;
        start as *mut u8
    }

    #[export_name = "cabi_realloc"]
    pub unsafe extern "C" fn cabi_realloc(
        old_ptr: *mut u8,
        old_len: usize,
        align: usize,
        new_len: usize,
    ) -> *mut u8 {
        let p = bump_alloc(new_len, align);
        if old_len != 0 && !old_ptr.is_null() {
            core::ptr::copy_nonoverlapping(old_ptr, p, if old_len < new_len { old_len } else { new_len });
        }
        p
    }

    #[export_name = "ping"]
    pub extern "C" fn ping(x: u32) -> u32 {
        x.wrapping_add(1)
    }

    #[export_name = "echo-bytes"]
    pub unsafe extern "C" fn echo_bytes(ptr: *const u8, len: usize) -> *mut u32 {
        let out = bump_alloc(len, 1);
        if len != 0 && !ptr.is_null() {
            core::ptr::copy_nonoverlapping(ptr, out, len);
        }
        let ret = &mut *BUMP.ret.get();
        ret[0] = out as u32;
        ret[1] = len as u32;
        ret.as_mut_ptr()
    }

    /// The result memory is bump-allocated and never reclaimed in the probe.
    #[export_name = "cabi_post_echo-bytes"]
    pub extern "C" fn post_echo_bytes(_ret: *mut u32) {}

    #[export_name = "grow"]
    pub extern "C" fn grow(pages: u32) -> u32 {
        let first = memory_grow(0, pages as usize);
        if first == usize::MAX {
            unreachable();
        }
        (first + pages as usize) as u32
    }
}
