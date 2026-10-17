//! `libm`: the math functions the guest imports, backed by the host's floating-point library.
//! ARM softfp: `double`s travel in core register pairs, `float`s as their bits in one register.

use crate::guest::wr32;
use crate::{Impl, Shared};
use bhpc_cpu::Cpu;

extern "C" {
    fn erf(x: f64) -> f64;
    fn erfc(x: f64) -> f64;
    fn tgamma(x: f64) -> f64;
    fn lgamma(x: f64) -> f64;
    fn logb(x: f64) -> f64;
    fn remainder(x: f64, y: f64) -> f64;
    fn remquo(x: f64, y: f64, q: *mut i32) -> f64;
}

/// `double f(double)`.
macro_rules! d1 {
    ($($id:ident $name:literal $f:expr;)*) => {$(
        fn $id(c: &mut Cpu, _: &Shared) {
            let x = c.args().f64(c);
            let f: fn(f64) -> f64 = $f;
            c.ret_f64(f(x));
        }
    )*};
}

/// `double f(double, double)`.
macro_rules! d2 {
    ($($id:ident $name:literal $f:expr;)*) => {$(
        fn $id(c: &mut Cpu, _: &Shared) {
            let mut a = c.args();
            let (x, y) = (a.f64(c), a.f64(c));
            let f: fn(f64, f64) -> f64 = $f;
            c.ret_f64(f(x, y));
        }
    )*};
}

/// `float f(float)`.
macro_rules! f1 {
    ($($id:ident $name:literal $f:expr;)*) => {$(
        fn $id(c: &mut Cpu, _: &Shared) {
            let x = c.args().f32(c);
            let f: fn(f32) -> f32 = $f;
            c.ret_u32(f(x).to_bits());
        }
    )*};
}

/// `float f(float, float)`.
macro_rules! f2 {
    ($($id:ident $name:literal $f:expr;)*) => {$(
        fn $id(c: &mut Cpu, _: &Shared) {
            let mut a = c.args();
            let (x, y) = (a.f32(c), a.f32(c));
            let f: fn(f32, f32) -> f32 = $f;
            c.ret_u32(f(x, y).to_bits());
        }
    )*};
}

d1! {
    m_acos "acos" f64::acos; m_asin "asin" f64::asin; m_atan "atan" f64::atan; m_cbrt "cbrt" f64::cbrt;
    m_cos "cos" f64::cos; m_cosh "cosh" f64::cosh; m_exp "exp" f64::exp; m_exp2 "exp2" f64::exp2;
    m_expm1 "expm1" f64::exp_m1; m_log "log" f64::ln; m_log10 "log10" f64::log10; m_log1p "log1p" f64::ln_1p;
    m_logb "logb" |x| unsafe { logb(x) }; m_sin "sin" f64::sin; m_sinh "sinh" f64::sinh;
    m_sqrt "sqrt" f64::sqrt; m_tan "tan" f64::tan; m_tanh "tanh" f64::tanh; m_acosh "acosh" f64::acosh;
    m_asinh "asinh" f64::asinh; m_atanh "atanh" f64::atanh; m_erf "erf" |x| unsafe { erf(x) };
    m_erfc "erfc" |x| unsafe { erfc(x) }; m_lgamma "lgamma" |x| unsafe { lgamma(x) };
    m_tgamma "tgamma" |x| unsafe { tgamma(x) }; m_floor "floor" f64::floor; m_ceil "ceil" f64::ceil;
    m_round "round" f64::round; m_trunc "trunc" f64::trunc; m_rint "rint" f64::round_ties_even;
    m_nearbyint "nearbyint" f64::round_ties_even;
}

d2! {
    m_atan2 "atan2" f64::atan2; m_fmod "fmod" |x, y| x % y; m_hypot "hypot" f64::hypot; m_pow "pow" f64::powf;
    m_remainder "remainder" |x, y| unsafe { remainder(x, y) }; m_fmin "fmin" f64::min; m_fmax "fmax" f64::max;
}

f1! {
    m_acosf "acosf" f32::acos; m_atanf "atanf" f32::atan; m_ceilf "ceilf" f32::ceil; m_cosf "cosf" f32::cos;
    m_expf "expf" f32::exp; m_floorf "floorf" f32::floor; m_logf "logf" f32::ln; m_roundf "roundf" f32::round;
    m_sinf "sinf" f32::sin; m_sqrtf "sqrtf" f32::sqrt; m_tanf "tanf" f32::tan;
}

f2! {
    m_atan2f "atan2f" f32::atan2; m_fminf "fminf" f32::min; m_fmaxf "fmaxf" f32::max;
}

/// `modf(x, double *iptr)`: fractional part returned, integral part stored.
fn m_modf(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let (x, p) = (a.f64(c), a.u32(c));
    let int = x.trunc();
    let frac = if x.is_infinite() { 0.0f64.copysign(x) } else { (x - int).copysign(x) };
    if c.mem().write_u64(p, int.to_bits()).is_err() {
        return crate::guest::fault(c, p, 8, true);
    }
    c.ret_f64(frac);
}

/// `frexp(x, int *exp)`: mantissa in [0.5, 1).
fn m_frexp(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let (x, p) = (a.f64(c), a.u32(c));
    let (m, e) = if x == 0.0 || !x.is_finite() {
        (x, 0)
    } else {
        // Scale subnormals into the normal range first.
        let (y, bias) = if x.abs() < f64::MIN_POSITIVE { (x * 2f64.powi(54), -54) } else { (x, 0) };
        let bits = y.to_bits();
        let e = ((bits >> 52) & 0x7FF) as i32 - 1022;
        (f64::from_bits((bits & !(0x7FF << 52)) | (1022 << 52)), e + bias)
    };
    if !wr32(c, p, e as u32) {
        return;
    }
    c.ret_f64(m);
}

fn scale(x: f64, n: i32) -> f64 {
    // Apply the exponent in two steps so large |n| neither overflows the intermediate nor loses subnormals.
    let n = n.clamp(-2200, 2200);
    let (a, b) = (n / 2, n - n / 2);
    x * 2f64.powi(a) * 2f64.powi(b)
}

/// `ldexp(x, n)` and `scalbn(x, n)`.
fn m_ldexp(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let (x, n) = (a.f64(c), a.u32(c) as i32);
    c.ret_f64(scale(x, n));
}

/// `remquo(x, y, int *quo)`.
fn m_remquo(c: &mut Cpu, _: &Shared) {
    let mut a = c.args();
    let (x, y, p) = (a.f64(c), a.f64(c), a.u32(c));
    let mut q = 0i32;
    // SAFETY: plain libm call writing one int.
    let r = unsafe { remquo(x, y, &mut q) };
    if wr32(c, p, q as u32) {
        c.ret_f64(r);
    }
}

/// `long lround(double)` / `lroundf(float)`.
fn m_lround(c: &mut Cpu, _: &Shared) {
    let x = c.args().f64(c);
    c.ret_u32(x.round() as i32 as u32);
}
fn m_lroundf(c: &mut Cpu, _: &Shared) {
    let x = c.args().f32(c);
    c.ret_u32(x.round() as i32 as u32);
}

/// `__fpclassifyd`: bionic's `FP_*` values.
fn m_fpclassifyd(c: &mut Cpu, _: &Shared) {
    use std::num::FpCategory::*;
    let x = c.args().f64(c);
    c.ret_u32(match x.classify() {
        Infinite => 0x01,
        Nan => 0x02,
        Normal => 0x04,
        Subnormal => 0x08,
        Zero => 0x10,
    });
}

fn m_isnan(c: &mut Cpu, _: &Shared) {
    let x = c.args().f64(c);
    c.ret_u32(u32::from(x.is_nan()));
}
fn m_isinf(c: &mut Cpu, _: &Shared) {
    let x = c.args().f64(c);
    c.ret_u32(u32::from(x.is_infinite()));
}
fn m_isnanf(c: &mut Cpu, _: &Shared) {
    let x = c.args().f32(c);
    c.ret_u32(u32::from(x.is_nan()));
}
fn m_isinff(c: &mut Cpu, _: &Shared) {
    let x = c.args().f32(c);
    c.ret_u32(u32::from(x.is_infinite()));
}
fn m_isfinitef(c: &mut Cpu, _: &Shared) {
    let x = c.args().f32(c);
    c.ret_u32(u32::from(x.is_finite()));
}
fn m_signbit(c: &mut Cpu, _: &Shared) {
    let x = c.args().f64(c);
    c.ret_u32(u32::from(x.is_sign_negative()));
}

pub(crate) const TABLE: &[(&str, Impl)] = &[
    ("acos", m_acos),
    ("asin", m_asin),
    ("atan", m_atan),
    ("cbrt", m_cbrt),
    ("cos", m_cos),
    ("cosh", m_cosh),
    ("exp", m_exp),
    ("exp2", m_exp2),
    ("expm1", m_expm1),
    ("log", m_log),
    ("log10", m_log10),
    ("log1p", m_log1p),
    ("logb", m_logb),
    ("sin", m_sin),
    ("sinh", m_sinh),
    ("sqrt", m_sqrt),
    ("tan", m_tan),
    ("tanh", m_tanh),
    ("acosh", m_acosh),
    ("asinh", m_asinh),
    ("atanh", m_atanh),
    ("erf", m_erf),
    ("erfc", m_erfc),
    ("lgamma", m_lgamma),
    ("tgamma", m_tgamma),
    ("floor", m_floor),
    ("ceil", m_ceil),
    ("round", m_round),
    ("trunc", m_trunc),
    ("rint", m_rint),
    ("nearbyint", m_nearbyint),
    ("atan2", m_atan2),
    ("fmod", m_fmod),
    ("hypot", m_hypot),
    ("pow", m_pow),
    ("remainder", m_remainder),
    ("fmin", m_fmin),
    ("fmax", m_fmax),
    ("acosf", m_acosf),
    ("atanf", m_atanf),
    ("ceilf", m_ceilf),
    ("cosf", m_cosf),
    ("expf", m_expf),
    ("floorf", m_floorf),
    ("logf", m_logf),
    ("roundf", m_roundf),
    ("sinf", m_sinf),
    ("sqrtf", m_sqrtf),
    ("tanf", m_tanf),
    ("atan2f", m_atan2f),
    ("fminf", m_fminf),
    ("fmaxf", m_fmaxf),
    ("modf", m_modf),
    ("frexp", m_frexp),
    ("ldexp", m_ldexp),
    ("scalbn", m_ldexp),
    ("remquo", m_remquo),
    ("lround", m_lround),
    ("lroundf", m_lroundf),
    ("__fpclassifyd", m_fpclassifyd),
    ("isnan", m_isnan),
    ("isinf", m_isinf),
    ("__isnanl", m_isnan),
    ("__isinfl", m_isinf),
    ("isnanf", m_isnanf),
    ("__isinff", m_isinff),
    ("__isfinitef", m_isfinitef),
    ("__signbit", m_signbit),
];

#[cfg(test)]
mod tests {
    use crate::tests_env::{Env, SCRATCH};

    fn split(x: f64) -> [u32; 2] {
        let b = x.to_bits();
        [b as u32, (b >> 32) as u32]
    }

    fn d(e: &mut Env, name: &str, args: &[f64]) -> f64 {
        let words: Vec<u32> = args.iter().flat_map(|x| split(*x)).collect();
        e.call(name, &words).as_f64()
    }

    fn f(e: &mut Env, name: &str, args: &[f32]) -> f32 {
        let words: Vec<u32> = args.iter().map(|x| x.to_bits()).collect();
        f32::from_bits(e.call(name, &words).r0)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * b.abs().max(1.0) || (a.is_nan() && b.is_nan())
    }

    #[test]
    fn unary_double_functions() {
        let mut e = Env::new();
        let cases: &[(&str, f64, f64)] = &[
            ("acos", 0.5, 0.5f64.acos()),
            ("asin", 0.5, 0.5f64.asin()),
            ("atan", 0.5, 0.5f64.atan()),
            ("cbrt", 27.0, 3.0),
            ("cos", 1.0, 1f64.cos()),
            ("cosh", 1.0, 1f64.cosh()),
            ("exp", 1.0, std::f64::consts::E),
            ("exp2", 10.0, 1024.0),
            ("expm1", 1e-3, 1e-3f64.exp_m1()),
            ("log", 1.0, 0.0),
            ("log10", 1000.0, 3.0),
            ("log1p", 1e-3, 1e-3f64.ln_1p()),
            ("logb", 1000.0, 9.0),
            ("sin", 1.0, 1f64.sin()),
            ("sinh", 1.0, 1f64.sinh()),
            ("sqrt", 2.0, 2f64.sqrt()),
            ("tan", 1.0, 1f64.tan()),
            ("tanh", 1.0, 1f64.tanh()),
            ("acosh", 2.0, 2f64.acosh()),
            ("asinh", 2.0, 2f64.asinh()),
            ("atanh", 0.5, 0.5f64.atanh()),
            ("erf", 0.5, 0.5204998778130465),
            ("erfc", 0.5, 0.4795001221869535),
            ("lgamma", 5.0, 24f64.ln()),
            ("tgamma", 5.0, 24.0),
            ("floor", -1.5, -2.0),
            ("ceil", -1.5, -1.0),
            ("round", -2.5, -3.0),
            ("trunc", -2.7, -2.0),
            ("rint", 2.5, 2.0),
            ("nearbyint", 3.5, 4.0),
        ];
        for (name, x, want) in cases {
            let got = d(&mut e, name, &[*x]);
            assert!(close(got, *want), "{name}({x}) = {got}, want {want}");
        }
    }

    #[test]
    fn binary_double_functions() {
        let mut e = Env::new();
        let cases: &[(&str, f64, f64, f64)] = &[
            ("atan2", 1.0, -1.0, 1f64.atan2(-1.0)),
            ("fmod", 7.5, 2.0, 1.5),
            ("hypot", 3.0, 4.0, 5.0),
            ("pow", 2.0, 10.0, 1024.0),
            ("remainder", 7.0, 2.0, -1.0),
            ("fmin", 1.0, 2.0, 1.0),
            ("fmax", 1.0, 2.0, 2.0),
        ];
        for (name, x, y, want) in cases {
            let got = d(&mut e, name, &[*x, *y]);
            assert!(close(got, *want), "{name}({x},{y}) = {got}");
        }
    }

    #[test]
    fn float_functions() {
        let mut e = Env::new();
        let un: &[(&str, f32, f32)] = &[
            ("acosf", 0.5, 0.5f32.acos()),
            ("atanf", 0.5, 0.5f32.atan()),
            ("ceilf", 1.2, 2.0),
            ("cosf", 1.0, 1f32.cos()),
            ("expf", 1.0, std::f32::consts::E),
            ("floorf", -1.2, -2.0),
            ("logf", 1.0, 0.0),
            ("roundf", 2.5, 3.0),
            ("sinf", 1.0, 1f32.sin()),
            ("sqrtf", 9.0, 3.0),
            ("tanf", 1.0, 1f32.tan()),
        ];
        for (name, x, want) in un {
            assert!((f(&mut e, name, &[*x]) - want).abs() < 1e-6, "{name}");
        }
        let bin: &[(&str, f32, f32, f32)] =
            &[("atan2f", 1.0, 1.0, std::f32::consts::FRAC_PI_4), ("fminf", 1.0, 2.0, 1.0), ("fmaxf", 1.0, 2.0, 2.0)];
        for (name, x, y, want) in bin {
            assert!((f(&mut e, name, &[*x, *y]) - want).abs() < 1e-6, "{name}");
        }
    }

    #[test]
    fn modf_frexp_ldexp_remquo() {
        let mut e = Env::new();
        let p = SCRATCH;
        let m = e.mem();
        // modf: fractional part keeps the sign; the integral part is stored.
        let mut args = split(-3.75).to_vec();
        args.push(p);
        assert_eq!(e.call("modf", &args).as_f64(), -0.75);
        assert_eq!(f64::from_bits(m.read_u64(p).unwrap()), -3.0);
        let mut args = split(f64::NEG_INFINITY).to_vec();
        args.push(p);
        let r = e.call("modf", &args).as_f64();
        assert!(r == 0.0 && r.is_sign_negative());
        assert_eq!(f64::from_bits(m.read_u64(p).unwrap()), f64::NEG_INFINITY);
        // frexp: normal, zero, infinity, subnormal.
        for (x, mant, exp) in [(8.0, 0.5, 4), (0.0, 0.0, 0), (f64::INFINITY, f64::INFINITY, 0), (5e-324, 0.5, -1073)] {
            let mut args = split(x).to_vec();
            args.push(p);
            assert_eq!(e.call("frexp", &args).as_f64(), mant, "frexp({x})");
            assert_eq!(m.read_u32(p).unwrap() as i32, exp, "frexp({x}) exponent");
        }
        // ldexp / scalbn including huge exponents.
        for name in ["ldexp", "scalbn"] {
            let mut args = split(1.5).to_vec();
            args.push(4);
            assert_eq!(e.call(name, &args).as_f64(), 24.0);
            let mut args = split(1.0).to_vec();
            args.push(100_000);
            assert_eq!(e.call(name, &args).as_f64(), f64::INFINITY);
            let mut args = split(1.0).to_vec();
            args.push((-100_000i32) as u32);
            assert_eq!(e.call(name, &args).as_f64(), 0.0);
        }
        // remquo.
        let mut args = split(7.0).to_vec();
        args.extend(split(2.0));
        args.push(p);
        assert_eq!(e.call("remquo", &args).as_f64(), -1.0);
        assert_eq!(m.read_u32(p).unwrap() & 7, 4);
        // Faulting out-pointers stop the guest.
        for name in ["modf", "frexp"] {
            let mut args = split(1.0).to_vec();
            args.push(0x10);
            assert!(e.try_call(name, &args).is_err(), "{name}");
        }
        let mut args = split(7.0).to_vec();
        args.extend(split(2.0));
        args.push(0x10);
        assert!(e.try_call("remquo", &args).is_err());
    }

    #[test]
    fn classification_and_rounding_to_int() {
        let mut e = Env::new();
        let dd = |e: &mut Env, n: &str, x: f64| e.call(n, &split(x)).r0;
        assert_eq!(dd(&mut e, "lround", -2.5) as i32, -3);
        assert_eq!(e.call("lroundf", &[2.5f32.to_bits()]).r0, 3);
        for (x, class) in [(f64::INFINITY, 1), (f64::NAN, 2), (1.0, 4), (5e-324, 8), (0.0, 0x10)] {
            assert_eq!(dd(&mut e, "__fpclassifyd", x), class, "{x}");
        }
        for n in ["isnan", "__isnanl"] {
            assert_eq!((dd(&mut e, n, f64::NAN), dd(&mut e, n, 1.0)), (1, 0));
        }
        for n in ["isinf", "__isinfl"] {
            assert_eq!((dd(&mut e, n, f64::INFINITY), dd(&mut e, n, 1.0)), (1, 0));
        }
        assert_eq!((dd(&mut e, "__signbit", -0.0), dd(&mut e, "__signbit", 1.0)), (1, 0));
        let ff = |e: &mut Env, n: &str, x: f32| e.call(n, &[x.to_bits()]).r0;
        assert_eq!((ff(&mut e, "isnanf", f32::NAN), ff(&mut e, "isnanf", 1.0)), (1, 0));
        assert_eq!((ff(&mut e, "__isinff", f32::INFINITY), ff(&mut e, "__isinff", 1.0)), (1, 0));
        assert_eq!((ff(&mut e, "__isfinitef", 1.0), ff(&mut e, "__isfinitef", f32::NAN)), (1, 0));
    }
}
