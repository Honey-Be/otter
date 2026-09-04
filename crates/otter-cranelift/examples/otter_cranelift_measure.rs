//! B0 — generic meta-VM: interpreter (T0 oracle) vs Cranelift JIT, from the SAME ISA definition.
//! Run: `cargo run -p otter-cranelift --example otter_cranelift_measure --release`
//!
//! Correctness is proven by the differential tests; this is a stopwatch. The point is that *neither*
//! tier was hand-written per-op — both consume `sample::isa()`.

use otter::interp::interpret;
use otter::sample;
use otter_cranelift::jit_run;
use std::time::Instant;

fn main() {
    let isa = sample::isa();
    let n = 50_000_000i32;
    let prog = sample::sum_program(n);
    let expected = (n as i64) * (n as i64 + 1) / 2;

    let t = Instant::now();
    let i = interpret(&isa, &prog, 5);
    let d_interp = t.elapsed();

    let t = Instant::now();
    let j = jit_run(&isa, &prog, 5); // includes Cranelift compile time
    let d_jit = t.elapsed();

    println!("=== B0 meta-VM: interpreter vs Cranelift JIT (one ISA, two tiers) ===");
    println!("sum 1..={n} = {expected}; agree: {}", i == j && i == expected);
    println!("interpret         : {:.2} ms", d_interp.as_secs_f64() * 1e3);
    println!("jit (compile+run) : {:.2} ms", d_jit.as_secs_f64() * 1e3);
    println!(
        "speedup (interp/jit): {:.1}x",
        d_interp.as_secs_f64() / d_jit.as_secs_f64()
    );
    println!("\nNeither tier is hand-written per-op: both derive from `sample::isa()` (the single source).");
}
