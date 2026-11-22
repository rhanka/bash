//! Streaming and one-shot cryptographic/checksum digests (`Md5`, `Sha1`, `Sha256`).
//!
//! All state lives inline on the caller's stack with zero heap allocations.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlg {
    Md5,
    Sha1,
    Sha256,
}

const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

fn encode_hex_lower<const D: usize, const H: usize>(digest: &[u8; D]) -> [u8; H] {
    let mut out = [0u8; H];
    for (i, &b) in digest.iter().enumerate() {
        out[i * 2] = HEX_LOWER[(b >> 4) as usize];
        out[i * 2 + 1] = HEX_LOWER[(b & 0x0f) as usize];
    }
    out
}

// --------------------------------------------------------------------------------------------- MD5

const MD5_SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee,
    0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
    0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa,
    0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
    0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
    0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
    0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039,
    0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
    0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

#[derive(Clone)]
pub struct Md5 {
    state: [u32; 4],
    buf: [u8; 64],
    buf_len: usize,
    total_bytes: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    pub const DIGEST_LEN: usize = 16;
    pub const HEX_LEN: usize = 32;

    pub const fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buf: [0; 64],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(data.len() as u64);
        let mut idx = 0;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            idx += take;
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while idx + 64 <= data.len() {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[idx..idx + 64]);
            self.compress(&block);
            idx += 64;
        }
        if idx < data.len() {
            let rem = data.len() - idx;
            self.buf[..rem].copy_from_slice(&data[idx..]);
            self.buf_len = rem;
        }
    }

    pub fn finalize(mut self) -> [u8; 16] {
        let bit_len = self.total_bytes.wrapping_mul(8);
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            self.buf[self.buf_len..64].fill(0);
            let block = self.buf;
            self.compress(&block);
            self.buf_len = 0;
        }
        self.buf[self.buf_len..56].fill(0);
        self.buf[56..64].copy_from_slice(&bit_len.to_le_bytes());
        let block = self.buf;
        self.compress(&block);
        let mut out = [0u8; 16];
        for (i, &w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn digest(data: &[u8]) -> [u8; 16] {
        let mut h = Self::new();
        h.update(data);
        h.finalize()
    }

    pub fn hex_lower(digest: &[u8; 16]) -> [u8; 32] {
        encode_hex_lower::<16, 32>(digest)
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(MD5_K[i])
                    .wrapping_add(m[g])
                    .rotate_left(MD5_SHIFTS[i]),
            );
            a = temp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }
}

// -------------------------------------------------------------------------------------------- SHA1

#[derive(Clone)]
pub struct Sha1 {
    state: [u32; 5],
    buf: [u8; 64],
    buf_len: usize,
    total_bytes: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub const DIGEST_LEN: usize = 20;
    pub const HEX_LEN: usize = 40;

    pub const fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0],
            buf: [0; 64],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(data.len() as u64);
        let mut idx = 0;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            idx += take;
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while idx + 64 <= data.len() {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[idx..idx + 64]);
            self.compress(&block);
            idx += 64;
        }
        if idx < data.len() {
            let rem = data.len() - idx;
            self.buf[..rem].copy_from_slice(&data[idx..]);
            self.buf_len = rem;
        }
    }

    pub fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.total_bytes.wrapping_mul(8);
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            self.buf[self.buf_len..64].fill(0);
            let block = self.buf;
            self.compress(&block);
            self.buf_len = 0;
        }
        self.buf[self.buf_len..56].fill(0);
        self.buf[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        self.compress(&block);
        let mut out = [0u8; 20];
        for (i, &w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    pub fn digest(data: &[u8]) -> [u8; 20] {
        let mut h = Self::new();
        h.update(data);
        h.finalize()
    }

    pub fn hex_lower(digest: &[u8; 20]) -> [u8; 40] {
        encode_hex_lower::<20, 40>(digest)
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (i, word) in w[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];
        let mut e = self.state[4];

        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdcu32),
                _ => (b ^ c ^ d, 0xca62c1d6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

// ------------------------------------------------------------------------------------------ SHA256

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_bytes: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub const DIGEST_LEN: usize = 32;
    pub const HEX_LEN: usize = 64;

    pub const fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
            ],
            buf: [0; 64],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(data.len() as u64);
        let mut idx = 0;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            idx += take;
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while idx + 64 <= data.len() {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[idx..idx + 64]);
            self.compress(&block);
            idx += 64;
        }
        if idx < data.len() {
            let rem = data.len() - idx;
            self.buf[..rem].copy_from_slice(&data[idx..]);
            self.buf_len = rem;
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_bytes.wrapping_mul(8);
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            self.buf[self.buf_len..64].fill(0);
            let block = self.buf;
            self.compress(&block);
            self.buf_len = 0;
        }
        self.buf[self.buf_len..56].fill(0);
        self.buf[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        self.compress(&block);
        let mut out = [0u8; 32];
        for (i, &w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    pub fn digest(data: &[u8]) -> [u8; 32] {
        let mut h = Self::new();
        h.update(data);
        h.finalize()
    }

    pub fn hex_lower(digest: &[u8; 32]) -> [u8; 64] {
        encode_hex_lower::<32, 64>(digest)
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in w[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_str<const N: usize>(bytes: &[u8; N]) -> &[u8] {
        bytes
    }

    #[test]
    fn md5_rfc1321_vectors() {
        let cases: &[(&[u8], &[u8; 32])] = &[
            (b"", b"d41d8cd98f00b204e9800998ecf8427e"),
            (b"a", b"0cc175b9c0f1b6a831c399e269772661"),
            (b"abc", b"900150983cd24fb0d6963f7d28e17f72"),
            (b"message digest", b"f96b697d7cb7938d525a2f31aaf161d0"),
            (b"abcdefghijklmnopqrstuvwxyz", b"c3fcd3d76192e4007dfb496cca67e13b"),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                b"d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                b"57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(hex_str(&Md5::hex_lower(&Md5::digest(input))), *expected);
        }
    }

    #[test]
    fn sha1_fips180_4_vectors() {
        let cases: &[(&[u8], &[u8; 40])] = &[
            (b"", b"da39a3ee5e6b4b0d3255bfef95601890afd80709"),
            (b"abc", b"a9993e364706816aba3e25717850c26c9cd0d89d"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                b"84983e441c3bd26ebaae4aa1f95129e5e54670f1",
            ),
            (
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
                b"a49b2446a02c645bf419f995b67091253a04a259",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(hex_str(&Sha1::hex_lower(&Sha1::digest(input))), *expected);
        }
    }

    #[test]
    fn sha256_fips180_4_vectors() {
        let cases: &[(&[u8], &[u8; 64])] = &[
            (b"", b"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (b"abc", b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                b"248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
                b"cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(hex_str(&Sha256::hex_lower(&Sha256::digest(input))), *expected);
        }
    }

    #[test]
    fn sha_million_a_one_shot_and_prime_streaming() {
        let buf = [b'a'; 1_000_000];
        let expected_sha1 = b"34aa973cd4c4daa4f61eeb2bdbad27316534016f";
        let expected_sha256 = b"cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0";

        assert_eq!(hex_str(&Sha1::hex_lower(&Sha1::digest(&buf))), expected_sha1);
        assert_eq!(hex_str(&Sha256::hex_lower(&Sha256::digest(&buf))), expected_sha256);

        let mut s1 = Sha1::new();
        let mut s256 = Sha256::new();
        let mut md5 = Md5::new();
        for chunk in buf.chunks(137) {
            s1.update(chunk);
            s256.update(chunk);
            md5.update(chunk);
        }
        assert_eq!(hex_str(&Sha1::hex_lower(&s1.finalize())), expected_sha1);
        assert_eq!(hex_str(&Sha256::hex_lower(&s256.finalize())), expected_sha256);
        assert_eq!(Md5::hex_lower(&md5.finalize()), Md5::hex_lower(&Md5::digest(&buf)));
    }

    #[test]
    fn block_boundaries_55_56_63_64_65() {
        for len in [55usize, 56, 63, 64, 65, 119, 120, 128] {
            let mut data = [0u8; 128];
            for (i, b) in data[..len].iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(37);
            }
            let slice = &data[..len];
            let mut md5 = Md5::new();
            let mut s1 = Sha1::new();
            let mut s256 = Sha256::new();
            for chunk in slice.chunks(7) {
                md5.update(chunk);
                s1.update(chunk);
                s256.update(chunk);
            }
            assert_eq!(md5.finalize(), Md5::digest(slice));
            assert_eq!(s1.finalize(), Sha1::digest(slice));
            assert_eq!(s256.finalize(), Sha256::digest(slice));
        }
    }

    #[test]
    fn hash_alg_enum_and_defaults() {
        assert_ne!(HashAlg::Md5, HashAlg::Sha1);
        assert_ne!(HashAlg::Sha1, HashAlg::Sha256);
        assert_eq!(Md5::default().finalize(), Md5::digest(b""));
        assert_eq!(Sha1::default().finalize(), Sha1::digest(b""));
        assert_eq!(Sha256::default().finalize(), Sha256::digest(b""));
    }

    #[test]
    fn md5_streaming_single_byte_steps() {
        let msg = b"The quick brown fox jumps over the lazy dog";
        let mut h = Md5::new();
        for b in msg {
            h.update(core::slice::from_ref(b));
        }
        assert_eq!(hex_str(&Md5::hex_lower(&h.finalize())), b"9e107d9d372bb6826bd81d3542a419d6");
    }

    #[test]
    fn sha1_streaming_single_byte_steps() {
        let msg = b"The quick brown fox jumps over the lazy dog";
        let mut h = Sha1::new();
        for b in msg {
            h.update(core::slice::from_ref(b));
        }
        assert_eq!(hex_str(&Sha1::hex_lower(&h.finalize())), b"2fd4e1c67a2d28fced849ee1bb76e7391b93eb12");
    }

    #[test]
    fn sha256_streaming_single_byte_steps() {
        let msg = b"The quick brown fox jumps over the lazy dog";
        let mut h = Sha256::new();
        for b in msg {
            h.update(core::slice::from_ref(b));
        }
        assert_eq!(
            hex_str(&Sha256::hex_lower(&h.finalize())),
            b"d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn hex_lower_all_nibbles() {
        let digest = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(&Md5::hex_lower(&digest)[..16], b"0123456789abcdef");
    }
}
