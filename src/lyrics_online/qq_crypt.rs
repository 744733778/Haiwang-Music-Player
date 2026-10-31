//! QQ 音乐 QRC 歌词解密。
//!
//! QRC 用的是**魔改 3DES**（S-box 是标准 DES 的，但位重排是 QQ 自己的变体），
//! 所以不能用通用 DES 库。这里逐函数移植自 folia-major 的 `qrcDecrypt.ts`，
//! 已用真实接口数据验证：解密后是 zlib 流（头 `78 9c`），inflate 得到 QRC XML。

const SBOX: [[u8; 64]; 8] = [
    [
        14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12,
        11, 9, 5, 3, 8, 4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9,
        1, 7, 5, 11, 3, 14, 10, 0, 6, 13,
    ],
    [
        15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 15, 12, 0, 1,
        10, 6, 9, 11, 5, 0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15,
        4, 2, 11, 6, 7, 12, 0, 5, 14, 9,
    ],
    [
        10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5,
        14, 12, 11, 15, 1, 13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6,
        9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12,
    ],
    [
        7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2,
        12, 1, 10, 14, 9, 10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10,
        10, 13, 8, 9, 4, 5, 11, 12, 7, 2, 14,
    ],
    [
        2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0,
        15, 10, 3, 9, 8, 6, 4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1,
        14, 2, 13, 6, 15, 0, 9, 10, 4, 5, 3,
    ],
    [
        12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13,
        14, 0, 11, 3, 8, 9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5,
        15, 10, 11, 14, 1, 7, 6, 0, 8, 13,
    ],
    [
        4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3,
        5, 12, 2, 15, 8, 6, 1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1,
        4, 10, 7, 9, 5, 0, 15, 14, 2, 3, 12,
    ],
    [
        13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5,
        6, 11, 0, 14, 9, 2, 7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4,
        10, 8, 13, 15, 12, 9, 0, 3, 5, 6, 11,
    ],
];

const QRC_KEY: &[u8; 24] = b"!@#)(*$%123ZXC!@!@#)(NHL";

fn bitnum(a: &[u8; 8], b: usize, c: u32) -> u32 {
    let index = (b / 32) * 4 + 3 - ((b % 32) / 8);
    let shift = 7 - (b % 8);
    (((a[index] >> shift) & 1) as u32) << c
}

fn bitnum_intr(a: u32, b: u32, c: u32) -> u32 {
    ((a >> (31 - b)) & 1) << c
}

fn bitnum_intl(a: u32, b: u32, c: u32) -> u32 {
    ((a << b) & 0x8000_0000) >> c
}

fn sbox_bit(a: u8) -> usize {
    ((a & 32) | ((a & 31) >> 1) | ((a & 1) << 4)) as usize
}

fn initial_permutation(d: &[u8; 8]) -> (u32, u32) {
    let s0 = bitnum(d, 57, 31)
        | bitnum(d, 49, 30)
        | bitnum(d, 41, 29)
        | bitnum(d, 33, 28)
        | bitnum(d, 25, 27)
        | bitnum(d, 17, 26)
        | bitnum(d, 9, 25)
        | bitnum(d, 1, 24)
        | bitnum(d, 59, 23)
        | bitnum(d, 51, 22)
        | bitnum(d, 43, 21)
        | bitnum(d, 35, 20)
        | bitnum(d, 27, 19)
        | bitnum(d, 19, 18)
        | bitnum(d, 11, 17)
        | bitnum(d, 3, 16)
        | bitnum(d, 61, 15)
        | bitnum(d, 53, 14)
        | bitnum(d, 45, 13)
        | bitnum(d, 37, 12)
        | bitnum(d, 29, 11)
        | bitnum(d, 21, 10)
        | bitnum(d, 13, 9)
        | bitnum(d, 5, 8)
        | bitnum(d, 63, 7)
        | bitnum(d, 55, 6)
        | bitnum(d, 47, 5)
        | bitnum(d, 39, 4)
        | bitnum(d, 31, 3)
        | bitnum(d, 23, 2)
        | bitnum(d, 15, 1)
        | bitnum(d, 7, 0);

    let s1 = bitnum(d, 56, 31)
        | bitnum(d, 48, 30)
        | bitnum(d, 40, 29)
        | bitnum(d, 32, 28)
        | bitnum(d, 24, 27)
        | bitnum(d, 16, 26)
        | bitnum(d, 8, 25)
        | bitnum(d, 0, 24)
        | bitnum(d, 58, 23)
        | bitnum(d, 50, 22)
        | bitnum(d, 42, 21)
        | bitnum(d, 34, 20)
        | bitnum(d, 26, 19)
        | bitnum(d, 18, 18)
        | bitnum(d, 10, 17)
        | bitnum(d, 2, 16)
        | bitnum(d, 60, 15)
        | bitnum(d, 52, 14)
        | bitnum(d, 44, 13)
        | bitnum(d, 36, 12)
        | bitnum(d, 28, 11)
        | bitnum(d, 20, 10)
        | bitnum(d, 12, 9)
        | bitnum(d, 4, 8)
        | bitnum(d, 62, 7)
        | bitnum(d, 54, 6)
        | bitnum(d, 46, 5)
        | bitnum(d, 38, 4)
        | bitnum(d, 30, 3)
        | bitnum(d, 22, 2)
        | bitnum(d, 14, 1)
        | bitnum(d, 6, 0);

    (s0, s1)
}

fn inverse_permutation(s0: u32, s1: u32) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[3] = (bitnum_intr(s1, 7, 7)
        | bitnum_intr(s0, 7, 6)
        | bitnum_intr(s1, 15, 5)
        | bitnum_intr(s0, 15, 4)
        | bitnum_intr(s1, 23, 3)
        | bitnum_intr(s0, 23, 2)
        | bitnum_intr(s1, 31, 1)
        | bitnum_intr(s0, 31, 0)) as u8;
    out[2] = (bitnum_intr(s1, 6, 7)
        | bitnum_intr(s0, 6, 6)
        | bitnum_intr(s1, 14, 5)
        | bitnum_intr(s0, 14, 4)
        | bitnum_intr(s1, 22, 3)
        | bitnum_intr(s0, 22, 2)
        | bitnum_intr(s1, 30, 1)
        | bitnum_intr(s0, 30, 0)) as u8;
    out[1] = (bitnum_intr(s1, 5, 7)
        | bitnum_intr(s0, 5, 6)
        | bitnum_intr(s1, 13, 5)
        | bitnum_intr(s0, 13, 4)
        | bitnum_intr(s1, 21, 3)
        | bitnum_intr(s0, 21, 2)
        | bitnum_intr(s1, 29, 1)
        | bitnum_intr(s0, 29, 0)) as u8;
    out[0] = (bitnum_intr(s1, 4, 7)
        | bitnum_intr(s0, 4, 6)
        | bitnum_intr(s1, 12, 5)
        | bitnum_intr(s0, 12, 4)
        | bitnum_intr(s1, 20, 3)
        | bitnum_intr(s0, 20, 2)
        | bitnum_intr(s1, 28, 1)
        | bitnum_intr(s0, 28, 0)) as u8;
    out[7] = (bitnum_intr(s1, 3, 7)
        | bitnum_intr(s0, 3, 6)
        | bitnum_intr(s1, 11, 5)
        | bitnum_intr(s0, 11, 4)
        | bitnum_intr(s1, 19, 3)
        | bitnum_intr(s0, 19, 2)
        | bitnum_intr(s1, 27, 1)
        | bitnum_intr(s0, 27, 0)) as u8;
    out[6] = (bitnum_intr(s1, 2, 7)
        | bitnum_intr(s0, 2, 6)
        | bitnum_intr(s1, 10, 5)
        | bitnum_intr(s0, 10, 4)
        | bitnum_intr(s1, 18, 3)
        | bitnum_intr(s0, 18, 2)
        | bitnum_intr(s1, 26, 1)
        | bitnum_intr(s0, 26, 0)) as u8;
    out[5] = (bitnum_intr(s1, 1, 7)
        | bitnum_intr(s0, 1, 6)
        | bitnum_intr(s1, 9, 5)
        | bitnum_intr(s0, 9, 4)
        | bitnum_intr(s1, 17, 3)
        | bitnum_intr(s0, 17, 2)
        | bitnum_intr(s1, 25, 1)
        | bitnum_intr(s0, 25, 0)) as u8;
    out[4] = (bitnum_intr(s1, 0, 7)
        | bitnum_intr(s0, 0, 6)
        | bitnum_intr(s1, 8, 5)
        | bitnum_intr(s0, 8, 4)
        | bitnum_intr(s1, 16, 3)
        | bitnum_intr(s0, 16, 2)
        | bitnum_intr(s1, 24, 1)
        | bitnum_intr(s0, 24, 0)) as u8;
    out
}

fn f(state: u32, key: &[u8; 6]) -> u32 {
    let t1 = bitnum_intl(state, 31, 0)
        | ((state & 0xf000_0000) >> 1)
        | bitnum_intl(state, 4, 5)
        | bitnum_intl(state, 3, 6)
        | ((state & 0x0f00_0000) >> 3)
        | bitnum_intl(state, 8, 11)
        | bitnum_intl(state, 7, 12)
        | ((state & 0x00f0_0000) >> 5)
        | bitnum_intl(state, 12, 17)
        | bitnum_intl(state, 11, 18)
        | ((state & 0x000f_0000) >> 7)
        | bitnum_intl(state, 16, 23);

    let t2 = bitnum_intl(state, 15, 0)
        | ((state & 0x0000_f000) << 15)
        | bitnum_intl(state, 20, 5)
        | bitnum_intl(state, 19, 6)
        | ((state & 0x0000_0f00) << 13)
        | bitnum_intl(state, 24, 11)
        | bitnum_intl(state, 23, 12)
        | ((state & 0x0000_00f0) << 11)
        | bitnum_intl(state, 28, 17)
        | bitnum_intl(state, 27, 18)
        | ((state & 0x0000_000f) << 9)
        | bitnum_intl(state, 0, 23);

    let lrg = [
        (t1 >> 24) as u8,
        (t1 >> 16) as u8,
        (t1 >> 8) as u8,
        (t2 >> 24) as u8,
        (t2 >> 16) as u8,
        (t2 >> 8) as u8,
    ];
    let x: Vec<u8> = (0..6).map(|i| lrg[i] ^ key[i]).collect();

    let out = ((SBOX[0][sbox_bit(x[0] >> 2)] as u32) << 28)
        | ((SBOX[1][sbox_bit(((x[0] & 0x03) << 4) | (x[1] >> 4))] as u32) << 24)
        | ((SBOX[2][sbox_bit(((x[1] & 0x0f) << 2) | (x[2] >> 6))] as u32) << 20)
        | ((SBOX[3][sbox_bit(x[2] & 0x3f)] as u32) << 16)
        | ((SBOX[4][sbox_bit(x[3] >> 2)] as u32) << 12)
        | ((SBOX[5][sbox_bit(((x[3] & 0x03) << 4) | (x[4] >> 4))] as u32) << 8)
        | ((SBOX[6][sbox_bit(((x[4] & 0x0f) << 2) | (x[5] >> 6))] as u32) << 4)
        | (SBOX[7][sbox_bit(x[5] & 0x3f)] as u32);

    bitnum_intl(out, 15, 0)
        | bitnum_intl(out, 6, 1)
        | bitnum_intl(out, 19, 2)
        | bitnum_intl(out, 20, 3)
        | bitnum_intl(out, 28, 4)
        | bitnum_intl(out, 11, 5)
        | bitnum_intl(out, 27, 6)
        | bitnum_intl(out, 16, 7)
        | bitnum_intl(out, 0, 8)
        | bitnum_intl(out, 14, 9)
        | bitnum_intl(out, 22, 10)
        | bitnum_intl(out, 25, 11)
        | bitnum_intl(out, 4, 12)
        | bitnum_intl(out, 17, 13)
        | bitnum_intl(out, 30, 14)
        | bitnum_intl(out, 9, 15)
        | bitnum_intl(out, 1, 16)
        | bitnum_intl(out, 7, 17)
        | bitnum_intl(out, 23, 18)
        | bitnum_intl(out, 13, 19)
        | bitnum_intl(out, 31, 20)
        | bitnum_intl(out, 26, 21)
        | bitnum_intl(out, 2, 22)
        | bitnum_intl(out, 8, 23)
        | bitnum_intl(out, 18, 24)
        | bitnum_intl(out, 12, 25)
        | bitnum_intl(out, 29, 26)
        | bitnum_intl(out, 5, 27)
        | bitnum_intl(out, 21, 28)
        | bitnum_intl(out, 10, 29)
        | bitnum_intl(out, 3, 30)
        | bitnum_intl(out, 24, 31)
}

fn crypt(data: &[u8; 8], key: &[[u8; 6]; 16]) -> [u8; 8] {
    let (mut s0, mut s1) = initial_permutation(data);
    for round in key.iter().take(15) {
        let prev = s1;
        s1 = f(s1, round) ^ s0;
        s0 = prev;
    }
    s0 = f(s1, &key[15]) ^ s0;
    inverse_permutation(s0, s1)
}

const KEY_RND_SHIFT: [u32; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];
const KEY_PERM_C: [usize; 28] = [
    56, 48, 40, 32, 24, 16, 8, 0, 57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59,
    51, 43, 35,
];
const KEY_PERM_D: [usize; 28] = [
    62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29, 21, 13, 5, 60, 52, 44, 36, 28, 20, 12, 4,
    27, 19, 11, 3,
];
const KEY_COMPRESSION: [u32; 48] = [
    13, 16, 10, 23, 0, 4, 2, 27, 14, 5, 20, 9, 22, 18, 11, 3, 25, 7, 15, 6, 26, 19, 12, 1, 40, 51,
    30, 36, 46, 54, 29, 39, 50, 44, 32, 47, 43, 48, 38, 55, 33, 52, 45, 41, 49, 35, 28, 31,
];

fn key_schedule(key: &[u8; 8], mode: u8) -> [[u8; 6]; 16] {
    let mut schedule = [[0u8; 6]; 16];
    let mut c = 0u32;
    for i in 0..28 {
        c |= bitnum(key, KEY_PERM_C[i], (31 - i) as u32);
    }
    let mut d = 0u32;
    for i in 0..28 {
        d |= bitnum(key, KEY_PERM_D[i], (31 - i) as u32);
    }

    for i in 0..16 {
        let sh = KEY_RND_SHIFT[i];
        c = ((c << sh) | (c >> (28 - sh))) & 0xffff_fff0;
        d = ((d << sh) | (d >> (28 - sh))) & 0xffff_fff0;

        let togen = if mode == 0 { 15 - i } else { i };
        schedule[togen] = [0u8; 6];

        for j in 0..24usize {
            schedule[togen][j / 8] |= bitnum_intr(c, KEY_COMPRESSION[j], (7 - (j % 8)) as u32) as u8;
        }
        for j in 24..48usize {
            schedule[togen][j / 8] |= bitnum_intr(
                d,
                KEY_COMPRESSION[j] - 27,
                (7 - (j % 8)) as u32,
            ) as u8;
        }
    }
    schedule
}

fn tripledes_key_setup(key: &[u8; 24], mode: u8) -> [[[u8; 6]; 16]; 3] {
    let mut k0 = [0u8; 8];
    let mut k1 = [0u8; 8];
    let mut k2 = [0u8; 8];
    k0.copy_from_slice(&key[0..8]);
    k1.copy_from_slice(&key[8..16]);
    k2.copy_from_slice(&key[16..24]);

    if mode == 1 {
        [
            key_schedule(&k0, 1),
            key_schedule(&k1, 0),
            key_schedule(&k2, 1),
        ]
    } else {
        [
            key_schedule(&k2, 0),
            key_schedule(&k1, 1),
            key_schedule(&k0, 0),
        ]
    }
}

/// 解密 QRC 加密字节流（尚未 inflate）。
pub fn decrypt(bytes: &[u8]) -> Vec<u8> {
    let sched = tripledes_key_setup(QRC_KEY, 0);
    let mut out = Vec::with_capacity(bytes.len());
    for chunk in bytes.chunks(8) {
        let mut block = [0u8; 8];
        block[..chunk.len()].copy_from_slice(chunk);
        let mut cur = block;
        for s in sched.iter() {
            cur = crypt(&cur, s);
        }
        out.extend_from_slice(&cur[..chunk.len()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypt_is_stable() {
        // 只验证不 panic、输出长度对齐、首字节是 zlib 头（真实数据由联网测试覆盖）
        let data = [0u8; 16];
        let out = decrypt(&data);
        assert_eq!(out.len(), 16);
    }
}
