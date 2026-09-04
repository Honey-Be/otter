//! T1 baseline (copy-and-patch) vs T2 (Cranelift): the tier trade-off — **compile latency** vs run
//! speed. Run: `cargo run -p otter-baseline --example otter_baseline_measure --release`
//!
//! The baseline tier exists to compile in *microseconds* (cheap to reach), accepting slower code than
//! the optimizing tier. This example shows that trade-off; correctness is proven by the tests.

use std::time::Instant;

use otter::interp::interpret_with;
use otter::sample;
use otter_baseline::compile as baseline_compile;
use otter_cranelift::compile_fn as cranelift_compile;

fn main() {
    let isa = sample::isa();
    let prog = sample::sum_to_program();
    let n = 2_000_000i64;
    let expected = n * (n + 1) / 2;

    // compile latency (median of many compiles)
    let iters = 2000;
    let t = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(baseline_compile(&isa, &prog, 5, 1));
    }
    let base_compile_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;

    let t = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(cranelift_compile(&isa, &prog, 5, 1));
    }
    let opt_compile_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;

    // run latency (compile once, call once with a big n)
    let base = baseline_compile(&isa, &prog, 5, 1);
    let opt = cranelift_compile(&isa, &prog, 5, 1);

    let t = Instant::now();
    let ri = interpret_with(&isa, &prog, 5, &[n]);
    let interp_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let rb = base.call(&[n]);
    let base_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let ro = opt.call(&[n]);
    let opt_ms = t.elapsed().as_secs_f64() * 1e3;

    println!("=== T1 baseline (copy-and-patch) vs T2 (Cranelift): the tier trade-off ===");
    println!("agree: {}", ri == rb && rb == ro && ri == expected);
    println!(
        "compile latency  baseline: {base_compile_us:.2} µs   cranelift: {opt_compile_us:.2} µs   (ratio {:.0}x)",
        opt_compile_us / base_compile_us
    );
    println!(
        "run sum 1..={n}   interp: {interp_ms:.2} ms   baseline: {base_ms:.3} ms   cranelift: {opt_ms:.3} ms"
    );
    println!(
        "\nThe baseline tier is cheap to *reach* (microsecond compile) — worth it for warm-but-not-hot\n\
         code; the optimizing tier pays a higher compile cost for faster code once a function is hot."
    );
}
