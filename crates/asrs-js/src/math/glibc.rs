// Ported from the IBM Accurate Mathematical Library in GNU C Library (sysdeps/ieee754/dbl-64),
// as bundled by V8 under third_party/glibc.
// Copyright (C) 2001-2022 Free Software Foundation, Inc.
// Licensed under the GNU Lesser General Public License, version 2.1 or (at your option) any
// later version. See licenses/LGPL-2.1.txt and THIRD_PARTY_NOTICES.md.

#![allow(
    clippy::approx_constant,
    clippy::assign_op_pattern,
    clippy::eq_op,
    clippy::excessive_precision,
    clippy::needless_range_loop
)]

use super::bits::{hi, lo};
use super::sincostab::SINCOSTAB;

const S1: f64 = f64::from_bits(0xbfc5555555555555);
const S2: f64 = f64::from_bits(0x3f81111111110ece);
const S3: f64 = f64::from_bits(0xbf2a01a019db08b8);
const S4: f64 = f64::from_bits(0x3ec71de27b9a7ed9);
const S5: f64 = f64::from_bits(0xbe5addffc2fcdf59);
const BIG: f64 = f64::from_bits(0x42c8000000000000);
const HP0: f64 = f64::from_bits(0x3ff921fb54442d18);
const HP1: f64 = f64::from_bits(0x3c91a62633145c07);
const MP1: f64 = f64::from_bits(0x3ff921fb58000000);
const MP2: f64 = f64::from_bits(0xbe4dde973c000000);
const PP3: f64 = f64::from_bits(0xbc8cb3b398000000);
const PP4: f64 = f64::from_bits(0xbacd747f23e32ed7);
const HPINV: f64 = f64::from_bits(0x3fe45f306dc9c883);
const TOINT: f64 = f64::from_bits(0x4338000000000000);

const SN3: f64 = -1.66666666666664880952546298448555E-01;
const SN5: f64 = 8.33333214285722277379541354343671E-03;
const CS2: f64 = 4.99999999999999999999950396842453E-01;
const CS4: f64 = -4.16666666666664434524222570944589E-02;
const CS6: f64 = 1.38888874007937613028114285595617E-03;

fn taylor_sin(xx: f64, x: f64, dx: f64) -> f64 {
    let poly = ((((S5 * xx + S4) * xx + S3) * xx + S2) * xx) + S1;
    let t = (poly * x - 0.5 * dx) * xx + dx;
    x + t
}

fn table(u: f64) -> (f64, f64, f64, f64) {
    let k = ((lo(u) as i32) << 2) as usize;
    (
        f64::from_bits(SINCOSTAB[k]),
        f64::from_bits(SINCOSTAB[k + 1]),
        f64::from_bits(SINCOSTAB[k + 2]),
        f64::from_bits(SINCOSTAB[k + 3]),
    )
}

fn do_cos(mut x: f64, mut dx: f64) -> f64 {
    if x < 0.0 {
        dx = -dx;
    }
    let u = BIG + x.abs();
    x = x.abs() - (u - BIG) + dx;
    let xx = x * x;
    let s = x + x * xx * (SN3 + xx * SN5);
    let c = xx * (CS2 + xx * (CS4 + xx * CS6));
    let (sn, ssn, cs, ccs) = table(u);
    let cor = (ccs - s * ssn - cs * c) - sn * s;
    cs + cor
}

fn do_sin(mut x: f64, mut dx: f64) -> f64 {
    let xold = x;
    if x.abs() < 0.126 {
        return taylor_sin(x * x, x, dx);
    }
    if x <= 0.0 {
        dx = -dx;
    }
    let u = BIG + x.abs();
    x = x.abs() - (u - BIG);
    let xx = x * x;
    let s = x + (dx + x * xx * (SN3 + xx * SN5));
    let c = x * dx + xx * (CS2 + xx * (CS4 + xx * CS6));
    let (sn, ssn, cs, ccs) = table(u);
    let cor = (ssn + s * ccs - sn * c) + cs * s;
    (sn + cor).copysign(xold)
}

fn reduce_sincos(x: f64) -> (i32, f64, f64) {
    let t = x * HPINV + TOINT;
    let xn = t - TOINT;
    let y = (x - xn * MP1) - xn * MP2;
    let n = (lo(t) & 3) as i32;
    let mut t1 = xn * PP3;
    let t2 = y - t1;
    let mut db = (y - t2) - t1;
    t1 = xn * PP4;
    let b = t2 - t1;
    db += (t2 - b) - t1;
    (n, b, db)
}

fn do_sincos(a: f64, da: f64, n: i32) -> f64 {
    let retval = if n & 1 != 0 {
        do_cos(a, da)
    } else {
        do_sin(a, da)
    };
    if n & 2 != 0 { -retval } else { retval }
}

pub fn sin(x: f64) -> f64 {
    let k = hi(x) & 0x7fffffff;
    if k < 0x3e500000 {
        x
    } else if k < 0x3feb6000 {
        do_sin(x, 0.0)
    } else if k < 0x400368fd {
        let t = HP0 - x.abs();
        do_cos(t, HP1).copysign(x)
    } else if k < 0x419921FB {
        let (n, a, da) = reduce_sincos(x);
        do_sincos(a, da, n)
    } else if k < 0x7ff00000 {
        let (n, a, da) = branred(x);
        do_sincos(a, da, n)
    } else {
        x / x
    }
}

pub fn cos(x: f64) -> f64 {
    let k = hi(x) & 0x7fffffff;
    if k < 0x3e400000 {
        1.0
    } else if k < 0x3feb6000 {
        do_cos(x, 0.0)
    } else if k < 0x400368fd {
        let y = HP0 - x.abs();
        let a = y + HP1;
        let da = (y - a) + HP1;
        do_sin(a, da)
    } else if k < 0x419921FB {
        let (n, a, da) = reduce_sincos(x);
        do_sincos(a, da, n + 1)
    } else if k < 0x7ff00000 {
        let (n, a, da) = branred(x);
        do_sincos(a, da, n + 1)
    } else {
        x / x
    }
}

const TOVERP: [f64; 75] = [
    10680707.0, 7228996.0, 1387004.0, 2578385.0, 16069853.0, 12639074.0, 9804092.0, 4427841.0,
    16666979.0, 11263675.0, 12935607.0, 2387514.0, 4345298.0, 14681673.0, 3074569.0, 13734428.0,
    16653803.0, 1880361.0, 10960616.0, 8533493.0, 3062596.0, 8710556.0, 7349940.0, 6258241.0,
    3772886.0, 3769171.0, 3798172.0, 8675211.0, 12450088.0, 3874808.0, 9961438.0, 366607.0,
    15675153.0, 9132554.0, 7151469.0, 3571407.0, 2607881.0, 12013382.0, 4155038.0, 6285869.0,
    7677882.0, 13102053.0, 15825725.0, 473591.0, 9065106.0, 15363067.0, 6271263.0, 9264392.0,
    5636912.0, 4652155.0, 7056368.0, 13614112.0, 10155062.0, 1944035.0, 9527646.0, 15080200.0,
    6658437.0, 6231200.0, 6832269.0, 16767104.0, 5075751.0, 3212806.0, 1398474.0, 7579849.0,
    6349435.0, 12618859.0, 4703257.0, 12806093.0, 14477321.0, 2786137.0, 12875403.0, 9837734.0,
    14528324.0, 13719321.0, 343717.0,
];

fn branred_part(x: f64) -> (f64, f64, f64) {
    const T576: u64 = 0x63f0000000000000;
    const TM24: f64 = f64::from_bits(0x3e70000000000000);
    const BIG: f64 = f64::from_bits(0x4338000000000000);
    const BIG1: f64 = f64::from_bits(0x4358000000000000);
    let mut r = [0.0f64; 6];
    let mut sum = 0.0;
    let k = ((((hi(x) >> 20) & 2047) - 450) / 24).max(0) as usize;
    let mut gor = f64::from_bits(T576 - (((k as u64) * 24) << 52));
    for (i, ri) in r.iter_mut().enumerate() {
        *ri = x * TOVERP[k + i] * gor;
        gor *= TM24;
    }
    for ri in r.iter_mut().take(3) {
        let s = (*ri + BIG) - BIG;
        sum += s;
        *ri -= s;
    }
    let mut t = 0.0;
    for i in 0..6 {
        t += r[5 - i];
    }
    let mut bb = (((((r[0] - t) + r[1]) + r[2]) + r[3]) + r[4]) + r[5];
    let s = (t + BIG) - BIG;
    sum += s;
    t -= s;
    let b = t + bb;
    bb = (t - b) + bb;
    let s = (sum + BIG1) - BIG1;
    sum -= s;
    (b, bb, sum)
}

fn branred(x: f64) -> (i32, f64, f64) {
    const TM600: f64 = f64::from_bits(0x1a70000000000000);
    const SPLIT: f64 = 134217729.0;
    const HP0: f64 = f64::from_bits(0x3FF921FB54442D18);
    const HP1: f64 = f64::from_bits(0x3C91A62633145C07);
    const MP1: f64 = f64::from_bits(0x3FF921FB58000000);
    const MP2: f64 = f64::from_bits(0xBE4DDE9740000000);
    let x = x * TM600;
    let t = x * SPLIT;
    let x1 = t - (t - x);
    let x2 = x - x1;
    let (b1, bb1, sum1) = branred_part(x1);
    let (b2, bb2, sum2) = branred_part(x2);
    let mut sum = sum1 + sum2;
    let mut b = b1 + b2;
    let bb = if b1.abs() > b2.abs() {
        (b1 - b) + b2
    } else {
        (b2 - b) + b1
    };
    if b > 0.5 {
        b -= 1.0;
        sum += 1.0;
    } else if b < -0.5 {
        b += 1.0;
        sum -= 1.0;
    }
    let s = b + (bb + bb1 + bb2);
    let t = ((b - s) + bb) + (bb1 + bb2);
    let b = s * SPLIT;
    let t1 = b - (b - s);
    let t2 = s - t1;
    let b = s * HP0;
    let bb = (((t1 * MP1 - b) + t1 * MP2) + t2 * MP1) + (t2 * MP2 + s * HP1 + t * HP0);
    let s = b + bb;
    let t = (b - s) + bb;
    ((sum as i32) & 3, s, t)
}
