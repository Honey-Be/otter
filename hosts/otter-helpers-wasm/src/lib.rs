//! b′ engine-library wasm (O-16c′ design §4.1 — the L-2 promotion of the spike wrapper,
//! source essentially unchanged): exports the three otter helper symbols under the AOT
//! artifact's import names (O-14 design §2.2 / §12.3).
//!
//! The otter crate's helpers are `pub extern "C" fn` WITHOUT `#[no_mangle]`, so their symbols
//! are mangled and the §2.2 names (`otter_binop`/`otter_convert`/`otter_unop`) exist nowhere
//! else — §12.3 places the export duty on b′. This wrapper discharges it by verbatim
//! delegation (canonical-reuse contract: no re-implementation, one call each).
//!
//! Build: repo-pinned stable (plain `cargo` — rust-toolchain.toml governs), wasm32-wasip1,
//! panic=abort, no_std (no wasi-libc, no std, no EH). Two profiles via `just wasm-bprime`:
//! non-shared and +atomics (shared memory) — see the recipe and README for the measured,
//! profile-split artifact shapes (§2-1 B5 re-statement).

#![no_std]

use otter::Value;

/// §2.2: (import "otter_helpers" "otter_binop" (func (param i64 i64 i32 i32) (result i64)))
#[unsafe(no_mangle)]
pub extern "C" fn otter_binop(a: Value, b: Value, ty: u32, op: u32) -> Value {
    otter::otter_binop(a, b, ty, op)
}

/// §2.2: (import "otter_helpers" "otter_convert" (func (param i64 i32 i32) (result i64)))
#[unsafe(no_mangle)]
pub extern "C" fn otter_convert(a: Value, from: u32, to: u32) -> Value {
    otter::otter_convert(a, from, to)
}

/// §2.2: (import "otter_helpers" "otter_unop" (func (param i64 i32 i32) (result i64)))
#[unsafe(no_mangle)]
pub extern "C" fn otter_unop(a: Value, ty: u32, op: u32) -> Value {
    otter::otter_unop(a, ty, op)
}

/// panic=abort + no_std: any panic (helper tag-contract violation = backend bug, per the
/// helpers' docs) becomes a single deterministic wasm `unreachable` trap. No unwinding, no EH
/// instructions, no message formatting I/O.
#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// The otter crate is no_std + `alloc` (its IR types use Vec/String), so rustc demands a
/// `#[global_allocator]` in any std-less final artifact that has `alloc` in its crate graph —
/// even though the three helpers' call closure (BinOp::eval / UnOp::eval / NumTy / f16 codec)
/// is allocation-free. This trap-on-use allocator satisfies the linkage requirement AND turns
/// any future accidental allocation into a deterministic trap (runtime proof of the
/// allocation-free property; the 40,000-case differential gate would surface it immediately).
/// Real-allocator introduction is an explicit library-extension re-decision (SPK §4-5,
/// design §4.1) — it comes WITH a scratch-norm re-computation, never silently.
#[cfg(target_arch = "wasm32")]
mod no_alloc {
    use core::alloc::{GlobalAlloc, Layout};

    struct TrapAlloc;

    unsafe impl GlobalAlloc for TrapAlloc {
        unsafe fn alloc(&self, _: Layout) -> *mut u8 {
            core::arch::wasm32::unreachable()
        }
        unsafe fn dealloc(&self, _: *mut u8, _: Layout) {
            core::arch::wasm32::unreachable()
        }
    }

    #[global_allocator]
    static A: TrapAlloc = TrapAlloc;
}
