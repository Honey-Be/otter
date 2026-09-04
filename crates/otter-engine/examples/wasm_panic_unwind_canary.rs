//! O-12 CI canary — NOT a general-purpose example, do not use as one.
//!
//! Under ordinary CI conditions (host target, default flags) this is a harmless trivial binary that just
//! builds and does nothing interesting. Its actual job is narrower and only exercised by one specific CI
//! step (see `.github/workflows/ci.yml`, "O-12 wasm canary"):
//!
//! ```text
//! RUSTFLAGS="-C panic=unwind" cargo build -p otter-engine --example wasm_panic_unwind_canary \
//!     --target wasm32-unknown-unknown
//! ```
//!
//! This is EXPECTED TO FAIL AT LINK TIME, today, on this toolchain — `error: the crate 'panic_unwind'
//! does not have the panic strategy 'unwind'` — because `wasm32-unknown-unknown`'s pre-built std does not
//! ship an unwind-capable panic runtime (see the O-12 design doc §2.2's correction, panel Lens B). That
//! failure is EXPECTED and is what the CI step asserts. `otter-engine`'s own O-12 `compile_error!` gate
//! does NOT fire in this scenario (panic really is "unwind" here) — this canary exists precisely to catch
//! the gap that leaves open: a clean `cargo check -p otter-engine --target wasm32-unknown-unknown` under
//! an explicit `-C panic=unwind` is NOT proof that a real binary built the same way will actually link, or
//! that `catch_unwind` will work in it.
//!
//! **If this ever starts SUCCEEDING**, the CI step is written to fail loudly rather than silently accept
//! it: that would mean upstream has shipped wasm unwind-runtime support and the O-12 wasm argument (design
//! doc §2.2, and the "Known gaps" section of `otter_engine`'s module doc) needs re-review — not that this
//! canary should be deleted or the CI step relaxed.
fn main() {
    // Just enough of a real dependency on `otter_engine` to force it to actually link into this binary
    // (not merely produce an rlib, which — per the O-12 design doc's Lens B correction — does NOT exercise
    // panic-runtime selection at all; only a final linked artifact does).
    let trap = otter_engine::Trap { idx: 0 };
    println!("{trap:?}");
}
