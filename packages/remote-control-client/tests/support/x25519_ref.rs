//! A slow, simple X25519 (RFC 7748) used only to check the fixtures' ephemeral
//! seeds against the public keys and shared secret. Production code uses `ring`,
//! which cannot import a fixed private key.

type Fe = [u64; 4];

const P: Fe = [
    0xffff_ffff_ffff_ffed,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
    0x7fff_ffff_ffff_ffff,
];

fn is_zero(x: &[u64]) -> bool {
    x.iter().all(|l| *l == 0)
}

/// Reduce an arbitrary-length little-endian number modulo 2^255 - 19.
fn reduce(input: &[u64]) -> Fe {
    let mut x: Vec<u64> = input.to_vec();
    while x.len() < 4 {
        x.push(0);
    }
    loop {
        // hi = x >> 255, lo = x & (2^255 - 1)
        let mut hi = vec![0u64; x.len()];
        for i in 0..x.len() {
            let from = i * 64 + 255;
            let (limb, bit) = (from / 64, from % 64);
            if limb < x.len() {
                let mut v = x[limb] >> bit;
                if bit != 0 && limb + 1 < x.len() {
                    v |= x[limb + 1] << (64 - bit);
                }
                hi[i] = v;
            }
        }
        if is_zero(&hi) {
            break;
        }
        let mut lo = x.clone();
        lo.truncate(4);
        lo[3] &= 0x7fff_ffff_ffff_ffff;
        // x = lo + 19 * hi
        let mut out = vec![0u64; hi.len().max(4) + 1];
        let mut carry = 0u128;
        for (i, slot) in out.iter_mut().enumerate() {
            let h = u128::from(*hi.get(i).unwrap_or(&0)) * 19;
            let l = u128::from(*lo.get(i).unwrap_or(&0));
            let sum = h + l + carry;
            *slot = sum as u64;
            carry = sum >> 64;
        }
        x = out;
    }
    let mut r = [x[0], x[1], x[2], x[3]];
    // r < 2^255; subtract p once if r >= p.
    if ge(&r, &P) {
        let mut borrow = 0u128;
        for i in 0..4 {
            let (a, b) = (u128::from(r[i]), u128::from(P[i]) + borrow);
            if a >= b {
                r[i] = (a - b) as u64;
                borrow = 0;
            } else {
                r[i] = ((1u128 << 64) + a - b) as u64;
                borrow = 1;
            }
        }
    }
    r
}

fn ge(a: &Fe, b: &Fe) -> bool {
    for i in (0..4).rev() {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    true
}

fn add(a: &Fe, b: &Fe) -> Fe {
    let mut out = [0u64; 5];
    let mut carry = 0u128;
    for i in 0..5 {
        let s = u128::from(*a.get(i).unwrap_or(&0)) + u128::from(*b.get(i).unwrap_or(&0)) + carry;
        out[i] = s as u64;
        carry = s >> 64;
    }
    reduce(&out)
}

fn sub(a: &Fe, b: &Fe) -> Fe {
    // a + p - b
    let mut t = [0u64; 5];
    let mut carry = 0u128;
    for i in 0..4 {
        let s = u128::from(a[i]) + u128::from(P[i]) + carry;
        t[i] = s as u64;
        carry = s >> 64;
    }
    t[4] = carry as u64;
    let mut borrow = 0u128;
    for i in 0..5 {
        let bi = u128::from(*b.get(i).unwrap_or(&0)) + borrow;
        let ai = u128::from(t[i]);
        if ai >= bi {
            t[i] = (ai - bi) as u64;
            borrow = 0;
        } else {
            t[i] = ((1u128 << 64) + ai - bi) as u64;
            borrow = 1;
        }
    }
    reduce(&t)
}

fn mul(a: &Fe, b: &Fe) -> Fe {
    let mut out = [0u64; 8];
    for i in 0..4 {
        let mut carry = 0u128;
        for j in 0..4 {
            let cur = u128::from(out[i + j]) + u128::from(a[i]) * u128::from(b[j]) + carry;
            out[i + j] = cur as u64;
            carry = cur >> 64;
        }
        out[i + 4] = (u128::from(out[i + 4]) + carry) as u64;
    }
    reduce(&out)
}

fn invert(a: &Fe) -> Fe {
    // a^(p-2)
    let mut exp = P;
    exp[0] -= 2;
    let mut result: Fe = [1, 0, 0, 0];
    for bit in (0..255).rev() {
        result = mul(&result, &result);
        if (exp[bit / 64] >> (bit % 64)) & 1 == 1 {
            result = mul(&result, a);
        }
    }
    result
}

fn from_bytes(bytes: &[u8; 32]) -> Fe {
    let mut limbs = [0u64; 4];
    for (i, limb) in limbs.iter_mut().enumerate() {
        *limb = u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap());
    }
    limbs[3] &= 0x7fff_ffff_ffff_ffff;
    reduce(&limbs)
}

fn to_bytes(x: &Fe) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, limb) in x.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&limb.to_le_bytes());
    }
    out
}

fn cswap(swap: bool, a: &mut Fe, b: &mut Fe) {
    if swap {
        std::mem::swap(a, b);
    }
}

/// X25519(k, u) as in RFC 7748 section 5.
pub fn x25519(k: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let mut scalar = *k;
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;
    let x1 = from_bytes(u);
    let (mut x2, mut z2, mut x3, mut z3): (Fe, Fe, Fe, Fe) =
        ([1, 0, 0, 0], [0; 4], x1, [1, 0, 0, 0]);
    let a24: Fe = [121_665, 0, 0, 0];
    let mut swap = false;
    for t in (0..255).rev() {
        let kt = (scalar[t / 8] >> (t % 8)) & 1 == 1;
        swap ^= kt;
        cswap(swap, &mut x2, &mut x3);
        cswap(swap, &mut z2, &mut z3);
        swap = kt;
        let a = add(&x2, &z2);
        let aa = mul(&a, &a);
        let b = sub(&x2, &z2);
        let bb = mul(&b, &b);
        let e = sub(&aa, &bb);
        let c = add(&x3, &z3);
        let d = sub(&x3, &z3);
        let da = mul(&d, &a);
        let cb = mul(&c, &b);
        let s = add(&da, &cb);
        x3 = mul(&s, &s);
        let df = sub(&da, &cb);
        z3 = mul(&x1, &mul(&df, &df));
        x2 = mul(&aa, &bb);
        z2 = mul(&e, &add(&aa, &mul(&a24, &e)));
    }
    cswap(swap, &mut x2, &mut x3);
    cswap(swap, &mut z2, &mut z3);
    to_bytes(&mul(&x2, &invert(&z2)))
}

/// The curve's base point, u = 9.
pub fn base_point() -> [u8; 32] {
    let mut nine = [0u8; 32];
    nine[0] = 9;
    nine
}
