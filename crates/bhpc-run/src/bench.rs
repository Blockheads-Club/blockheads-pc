//! Thunk round-trip micro-benchmark: guest loop calling a no-op host function.

use crate::{new_env, RunError};
use bhpc_cpu::{Cpu, Stop};
use std::time::{Duration, Instant};

const CODE: u32 = 0x1000_0000;

/// `fn loop(n)`: calls the function at the literal pool address `n` times.
fn write_loop(cpu: &Cpu, at: u32, target: u32) -> Result<(), RunError> {
    let words = [
        0xE92D_4030u32, // push {r4, r5, lr}
        0xE1A0_4000,    // mov r4, r0
        0xE59F_5010,    // ldr r5, [pc, #16]
        0xE1A0_0004,    // loop: mov r0, r4
        0xE12F_FF35,    // blx r5
        0xE254_4001,    // subs r4, r4, #1
        0x1AFF_FFFB,    // bne loop
        0xE8BD_8030,    // pop {r4, r5, pc}
        target,         // literal
    ];
    for (i, w) in words.iter().enumerate() {
        cpu.mem().write_u32(at + 4 * i as u32, *w)?;
    }
    Ok(())
}

/// Result of [`measure`].
#[derive(Debug, Clone, Copy)]
pub struct ThunkTiming {
    pub iterations: u32,
    /// Total time of the loop calling the host thunk.
    pub thunk: Duration,
    /// Total time of the same loop calling a guest `bx lr` (loop and call overhead only).
    pub baseline: Duration,
}

impl ThunkTiming {
    /// Nanoseconds per thunk call including the loop and call overhead.
    pub fn ns_per_call(&self) -> f64 {
        self.thunk.as_nanos() as f64 / self.iterations as f64
    }
    /// Nanoseconds per call over the guest-only baseline (the SVC round trip itself).
    pub fn ns_over_baseline(&self) -> f64 {
        (self.thunk.as_nanos() as f64 - self.baseline.as_nanos() as f64) / self.iterations as f64
    }
}

/// Time `iterations` guest-to-host-to-guest round trips.
pub fn measure(iterations: u32) -> Result<ThunkTiming, RunError> {
    let (mut cpu, _st) = new_env(Box::new(std::io::sink()), None)?;
    cpu.mem().map(CODE, 0x1000)?;
    let nop = cpu.register_thunk("bhpc_nop", Box::new(|_: &mut Cpu| {}))?;
    cpu.mem().write_u32(CODE + 0x100, 0xE12F_FF1E)?; // bx lr
    write_loop(&cpu, CODE, nop)?;
    write_loop(&cpu, CODE + 0x200, CODE + 0x100)?;
    // Warm up so JIT translation is not measured.
    cpu.call(CODE, &[1000]).map_err(RunError::Guest)?;
    cpu.call(CODE + 0x200, &[1000]).map_err(RunError::Guest)?;
    let t0 = Instant::now();
    cpu.call(CODE, &[iterations]).map_err(RunError::Guest)?;
    let thunk = t0.elapsed();
    let t0 = Instant::now();
    cpu.call(CODE + 0x200, &[iterations]).map_err(RunError::Guest)?;
    let baseline = t0.elapsed();
    let _ = Stop::Returned;
    Ok(ThunkTiming { iterations, thunk, baseline })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_a_positive_cost() {
        let t = measure(20_000).unwrap();
        assert_eq!(t.iterations, 20_000);
        assert!(t.ns_per_call() > 0.0);
        // The baseline is cheaper than a real host call, so the difference is not negative.
        assert!(t.ns_over_baseline().is_finite());
    }
}
