//! Fixed-capacity stack big-unsigned-integer arithmetic for exact Dragon4 IEEE-754 formatting.
//!
//! Zero heap allocations; 42 x `u32` limbs = 1,344 bits, covering all IEEE-754 `f64` values
//! (`|x| < 2^1024` and `10^324` scaling with headroom for intermediate `x10` / `x8` shifts).

const CAP: usize = 42;

#[derive(Clone)]
pub(crate) struct BigUint {
    limbs: [u32; CAP],
    len: usize,
}

impl BigUint {
    pub(crate) fn from_u64(v: u64) -> Self {
        let mut limbs = [0u32; CAP];
        let lo = v as u32;
        let hi = (v >> 32) as u32;
        let len = if hi != 0 {
            limbs[0] = lo;
            limbs[1] = hi;
            2
        } else if lo != 0 {
            limbs[0] = lo;
            1
        } else {
            0
        };
        Self { limbs, len }
    }

    pub(crate) fn trim(&mut self) {
        while self.len > 0 && self.limbs[self.len - 1] == 0 {
            self.len -= 1;
        }
    }

    pub(crate) fn shl(&mut self, bits: u32) {
        if self.len == 0 || bits == 0 {
            return;
        }
        let word_shift = (bits / 32) as usize;
        let bit_shift = bits % 32;
        if bit_shift == 0 {
            for i in (0..self.len).rev() {
                self.limbs[i + word_shift] = self.limbs[i];
            }
            self.limbs[..word_shift].fill(0);
            self.len += word_shift;
        } else {
            let mut carry = 0u32;
            let new_len = self.len + word_shift;
            for i in (0..self.len).rev() {
                let cur = self.limbs[i];
                if i == self.len - 1 {
                    let top = cur >> (32 - bit_shift);
                    if top != 0 {
                        self.limbs[new_len] = top;
                        self.len = new_len + 1;
                    } else {
                        self.len = new_len;
                    }
                }
                let next_lo = if i > 0 { self.limbs[i - 1] >> (32 - bit_shift) } else { 0 };
                let _ = carry;
                carry = (cur << bit_shift) | next_lo;
                self.limbs[i + word_shift] = carry;
            }
            self.limbs[..word_shift].fill(0);
        }
    }

    pub(crate) fn mul_u32(&mut self, rhs: u32) {
        if self.len == 0 || rhs == 1 {
            return;
        }
        if rhs == 0 {
            self.len = 0;
            return;
        }
        let mut carry = 0u64;
        let rhs = rhs as u64;
        for i in 0..self.len {
            let prod = (self.limbs[i] as u64) * rhs + carry;
            self.limbs[i] = prod as u32;
            carry = prod >> 32;
        }
        if carry != 0 {
            self.limbs[self.len] = carry as u32;
            self.len += 1;
        }
    }

    pub(crate) fn mul_pow10(&mut self, mut exp: u32) {
        // Multiply by 10^9 chunks while possible, then smaller powers of 10.
        while exp >= 9 {
            self.mul_u32(1_000_000_000);
            exp -= 9;
        }
        if exp > 0 {
            let mut p = 1u32;
            for _ in 0..exp {
                p *= 10;
            }
            self.mul_u32(p);
        }
    }

    pub(crate) fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        if self.len != other.len {
            return self.len.cmp(&other.len);
        }
        for i in (0..self.len).rev() {
            if self.limbs[i] != other.limbs[i] {
                return self.limbs[i].cmp(&other.limbs[i]);
            }
        }
        core::cmp::Ordering::Equal
    }

    pub(crate) fn add(&self, other: &Self) -> Self {
        let max_len = self.len.max(other.len);
        let mut out = Self { limbs: [0u32; CAP], len: max_len };
        let mut carry = 0u64;
        for i in 0..max_len {
            let a = if i < self.len { self.limbs[i] as u64 } else { 0 };
            let b = if i < other.len { other.limbs[i] as u64 } else { 0 };
            let sum = a + b + carry;
            out.limbs[i] = sum as u32;
            carry = sum >> 32;
        }
        if carry != 0 {
            out.limbs[max_len] = carry as u32;
            out.len += 1;
        }
        out
    }

    pub(crate) fn sub_assign(&mut self, other: &Self) {
        let mut borrow = 0i64;
        for i in 0..self.len {
            let a = self.limbs[i] as i64;
            let b = if i < other.len { other.limbs[i] as i64 } else { 0 };
            let mut diff = a - b - borrow;
            if diff < 0 {
                diff += 1i64 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            self.limbs[i] = diff as u32;
        }
        self.trim();
    }

    /// Divides `self` by `den` when `self < 10 * den`, returning the quotient digit `0..=9`
    /// and leaving the remainder in `self`.
    pub(crate) fn div_digit(&mut self, den: &Self) -> u8 {
        let mut q = 0u8;
        for shift in [3u32, 2, 1, 0] {
            let bit = 1u8 << shift;
            if q + bit > 9 {
                continue;
            }
            let mut shifted = den.clone();
            shifted.shl(shift);
            if self.cmp(&shifted) != core::cmp::Ordering::Less {
                self.sub_assign(&shifted);
                q += bit;
            }
        }
        q
    }
}
