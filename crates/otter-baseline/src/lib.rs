//! L0 **baseline tier (T1)** — a copy-and-patch x86-64 stencil JIT for any [`otter::Isa`].
//!
//! This is the *fast-to-compile, cheap-to-run* tier of the meta-hybrid engine, sitting between the T0
//! interpreter and the T2 optimizing Cranelift backend. "Copy-and-patch" (Xu & Kjolstad, PLDI'21):
//! emit a fixed machine-code **template per IR node** and patch its holes (immediates, local
//! displacements, branch displacements) — no IR-level optimization, near-linear-time (microsecond)
//! compilation. We emit templates at runtime rather than `memcpy`-ing build-time-precompiled blobs
//! (same complexity class, no clang/build-time toolchain).
//!
//! Crucially, the emitter is **driven by the reifiable IR** ([`Effect`]/[`Expr`]), exactly like the
//! Cranelift backend — so this tier is *also* derived from the single source. The only hand-written,
//! x86-64-specific part is the primitive encoding table (the ALU ops, `mov`, branches), analogous to
//! the Cranelift backend's per-`BinOp` lowering. Add a bytecode op = data; it gets a baseline template
//! for free (as long as it composes existing primitives).
//!
//! Machine model (System V AMD64): compiled fn = `extern "C" fn(*const i64) -> i64`, args in `rdi`.
//! Locals live in the stack frame at `[rbp - 8*(i+1)]`; `rax`/`rcx` are scratch; expression trees use
//! the CPU stack (`push`/`pop`) for nesting. W^X is enforced (map RW → write → `mprotect` RX).
//!
//! `unsafe` (mmap/mprotect, code-pointer transmute) is concentrated here — a dangerous subsystem.
//! Verification (verify-before-implement): the differential tests assert `baseline == interpret` (the
//! T0 oracle) and `baseline == cranelift` over hand-written + randomized programs.
#![allow(unsafe_code)]
// This tier hand-emits x86-64 machine code, so it exists ONLY on x86-64. On every other target the crate
// is EMPTY (the `#![cfg]` compiles out all items) rather than a hard `compile_error!` — that lets a
// workspace build / a dependent (otter-engine) succeed on aarch64/riscv64/wasm and simply run without T1
// (see otter-engine's target-gated tier selection). Consumers reference this crate under the same cfg.
#![cfg(target_arch = "x86_64")]

use core::ptr;

use otter::engine::{Backend, Code};
use otter::{ArithOp, Effect, Expr, FuncDef, Inst, Isa, Module, NumTy, UnOp, Value};

// --- tiny x86-64 emit helpers -----------------------------------------------------------------------

fn emit(code: &mut Vec<u8>, bytes: &[u8]) {
    code.extend_from_slice(bytes);
}
fn emit_i32(code: &mut Vec<u8>, v: i32) {
    code.extend_from_slice(&v.to_le_bytes());
}
fn emit_i64(code: &mut Vec<u8>, v: i64) {
    code.extend_from_slice(&v.to_le_bytes());
}
fn patch_i32(code: &mut [u8], at: usize, v: i32) {
    code[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Stack displacement of local `i`: `[rbp - 8*(i+1)]`.
fn disp(i: usize) -> i32 {
    -(((i + 1) * 8) as i32)
}

/// Integer binop operand setup: with rhs in `rax` and lhs on the stack → `rcx = rhs`, `rax = lhs`.
fn setup_int(code: &mut Vec<u8>) {
    emit(code, &[0x48, 0x89, 0xC1]); // mov rcx, rax  (rhs)
    emit(code, &[0x58]); // pop rax        (lhs)
}

/// Float binop operand setup: with rhs in `rax` and lhs on the stack → `xmm1 = rhs`, `xmm0 = lhs`.
fn setup_float(code: &mut Vec<u8>) {
    emit(code, &[0x66, 0x48, 0x0F, 0x6E, 0xC8]); // movq xmm1, rax (rhs)
    emit(code, &[0x58]); // pop rax
    emit(code, &[0x66, 0x48, 0x0F, 0x6E, 0xC0]); // movq xmm0, rax (lhs)
}

// Canonical interpreter helpers (single source of truth). The baseline `call`s these for ops it does
// not hand-encode exactly — `f16` (no SSE f16), and the saturating/unsigned conversion edges — so the
// result is bit-identical to the interpreter and across the half/f16-intrinsic configs.
fn binop_addr() -> i64 {
    (otter::otter_binop as extern "C" fn(i64, i64, u32, u32) -> i64) as usize as i64
}
fn convert_addr() -> i64 {
    (otter::otter_convert as extern "C" fn(i64, u32, u32) -> i64) as usize as i64
}

/// `call <addr>` with rsp re-aligned to 16 around the call (save/restore via rbx, which the callee
/// preserves). Arguments must already be in the SysV registers; the result lands in `rax`.
fn emit_aligned_call(code: &mut Vec<u8>, addr: i64) {
    emit(code, &[0x53]); // push rbx
    emit(code, &[0x48, 0x89, 0xE3]); // mov rbx, rsp
    emit(code, &[0x48, 0x83, 0xE4, 0xF0]); // and rsp, -16
    emit(code, &[0x48, 0xB8]); // movabs rax, addr
    emit_i64(code, addr);
    emit(code, &[0xFF, 0xD0]); // call rax
    emit(code, &[0x48, 0x89, 0xDC]); // mov rsp, rbx
    emit(code, &[0x5B]); // pop rbx
}

/// Call `otter_binop`. Binop state: lhs on the stack, rhs in `rax` → rdi=lhs, rsi=rhs.
fn emit_call_binop(code: &mut Vec<u8>, ty: NumTy, op: ArithOp) {
    emit(code, &[0x5F]); // pop rdi  (lhs)
    emit(code, &[0x48, 0x89, 0xC6]); // mov rsi, rax (rhs)
    emit(code, &[0xBA]); // mov edx, ty.tag()
    emit_i32(code, ty.tag() as i32);
    emit(code, &[0xB9]); // mov ecx, op.tag()
    emit_i32(code, op.tag() as i32);
    emit_aligned_call(code, binop_addr());
}

/// Call `otter_convert`. Operand in `rax` → rdi=operand.
fn emit_call_convert(code: &mut Vec<u8>, from: NumTy, to: NumTy) {
    emit(code, &[0x48, 0x89, 0xC7]); // mov rdi, rax
    emit(code, &[0xBE]); // mov esi, from.tag()
    emit_i32(code, from.tag() as i32);
    emit(code, &[0xBA]); // mov edx, to.tag()
    emit_i32(code, to.tag() as i32);
    emit_aligned_call(code, convert_addr());
}

/// Call a baked host fn `f(x)` (`extern "C" fn(i64) -> i64`). Operand in `rax` → rdi. The address comes
/// from the fn pointer carried in the IR (no host table; the runtime resolves it at bytecode-build time).
fn emit_call_host1(code: &mut Vec<u8>, f: otter::HostFn1) {
    emit(code, &[0x48, 0x89, 0xC7]); // mov rdi, rax
    emit_aligned_call(code, f as usize as i64);
}

/// Call a baked host fn `f(x, y)` (`extern "C" fn(i64, i64) -> i64`). lhs on stack, rhs in `rax` →
/// rdi=lhs, rsi=rhs (same marshalling as [`emit_call_binop`]).
fn emit_call_host2(code: &mut Vec<u8>, f: otter::HostFn2) {
    emit(code, &[0x5F]); // pop rdi  (lhs)
    emit(code, &[0x48, 0x89, 0xC6]); // mov rsi, rax (rhs)
    emit_aligned_call(code, f as usize as i64);
}

/// `movabs rdi, imm64`.
fn emit_movabs_rdi(code: &mut Vec<u8>, v: i64) {
    emit(code, &[0x48, 0xBF]);
    emit_i64(code, v);
}

/// Call a baked host-with-context fn `f(host_ctx, x)` (`extern "C" fn(i64, i64) -> i64`) — the
/// [`otter::Expr::HostCtx1`] sibling of [`emit_call_host1`]. Operand `x` is in `rax` on entry (from
/// `emit_expr`). Staging is load-operand-first, THEN move it off `rax` into its destination register,
/// THEN load the `host_ctx` immediate — never the other order, since `movabs rax, host_ctx` would
/// clobber `x` before it's been moved anywhere: `rsi = rax` (x) first, `rdi = host_ctx` (immediate)
/// second. (Cranelift's SSA `iconst` has no such hazard — this ordering constraint is specific to this
/// hand-emitted, rax-scratch backend.)
fn emit_call_host_ctx1(code: &mut Vec<u8>, f: otter::HostFnCtx1, host_ctx: Value) {
    emit(code, &[0x48, 0x89, 0xC6]); // mov rsi, rax  (x — save before clobbering rax)
    emit_movabs_rdi(code, host_ctx); // rdi = host_ctx (immediate, loaded AFTER x is safely in rsi)
    emit_aligned_call(code, f as usize as i64);
}

/// Call a baked host-with-context fn `f(host_ctx, x, y)`. lhs (`x`) on the stack, rhs (`y`) in `rax` on
/// entry (from `emit_expr`, same marshalling as [`emit_call_host2`]). Staging: `y` off `rax` into `rdx`
/// FIRST (before anything touches `rax`), then `x` popped into `rsi`, then `host_ctx` loaded into `rdi`
/// last — see [`emit_call_host_ctx1`]'s doc for why this load-then-move-then-immediate order matters.
fn emit_call_host_ctx2(code: &mut Vec<u8>, f: otter::HostFnCtx2, host_ctx: Value) {
    emit(code, &[0x48, 0x89, 0xC2]); // mov rdx, rax  (y — save before clobbering rax)
    emit(code, &[0x5E]); // pop rsi  (x)
    emit_movabs_rdi(code, host_ctx); // rdi = host_ctx (immediate, loaded last)
    emit_aligned_call(code, f as usize as i64);
}

/// Re-normalize `rax` to integer `ty`'s slot form (sign/zero-extend the low `bits` bits).
fn norm_int_baseline(code: &mut Vec<u8>, ty: NumTy) {
    match ty {
        NumTy::I8 => emit(code, &[0x48, 0x0F, 0xBE, 0xC0]), // movsx rax, al
        NumTy::I16 => emit(code, &[0x48, 0x0F, 0xBF, 0xC0]), // movsx rax, ax
        NumTy::I32 => emit(code, &[0x48, 0x63, 0xC0]),      // movsxd rax, eax
        NumTy::U8 => emit(code, &[0x48, 0x0F, 0xB6, 0xC0]), // movzx rax, al
        NumTy::U16 => emit(code, &[0x48, 0x0F, 0xB7, 0xC0]), // movzx rax, ax
        NumTy::U32 => emit(code, &[0x89, 0xC0]),            // mov eax, eax (zero-extend)
        _ => {}                                             // I64/U64/floats: already normalized
    }
}

/// Convert `rax` (a value of type `from`) to type `to`, leaving the result in `rax`. The cases baseline
/// hand-encodes exactly (int↔int, signed-int→float, float↔float via SSE) are inline; the rest — anything
/// `f16`, every float→int (per-width saturation incl. NaN/over-i64), and unsigned-int→float — route
/// through the canonical interpreter helper, so **baseline now matches the interpreter on all inputs**
/// (no remaining edge-case exceptions).
fn emit_convert(code: &mut Vec<u8>, from: NumTy, to: NumTy) {
    // Route through the canonical helper the cases baseline doesn't hand-encode exactly:
    //   - anything involving f16 (no SSE f16);
    //   - float -> int (per-width saturation incl. NaN/over-i64);
    //   - unsigned int -> float (cvtsi2s* is signed).
    // The rest (int↔int, signed-int→float, float↔float) are hand-encoded below.
    let needs_helper = from == NumTy::F16
        || to == NumTy::F16
        || (from.is_float() && !to.is_float())
        || (!from.is_float() && to.is_float() && !from.is_signed_int());
    if needs_helper {
        emit_call_convert(code, from, to);
        return;
    }
    match (from.is_float(), to.is_float()) {
        (false, false) => norm_int_baseline(code, to), // int -> int
        (false, true) => {
            // signed int -> float
            if to == NumTy::F64 {
                emit(code, &[0xF2, 0x48, 0x0F, 0x2A, 0xC0]); // cvtsi2sd xmm0, rax
                emit(code, &[0x66, 0x48, 0x0F, 0x7E, 0xC0]); // movq rax, xmm0
            } else {
                emit(code, &[0xF3, 0x48, 0x0F, 0x2A, 0xC0]); // cvtsi2ss xmm0, rax
                emit(code, &[0x66, 0x0F, 0x7E, 0xC0]); // movd eax, xmm0 (zero-extend)
            }
        }
        (true, true) => {
            emit(code, &[0x66, 0x48, 0x0F, 0x6E, 0xC0]); // movq xmm0, rax
            if from == NumTy::F32 && to == NumTy::F64 {
                emit(code, &[0xF3, 0x0F, 0x5A, 0xC0]); // cvtss2sd xmm0, xmm0
                emit(code, &[0x66, 0x48, 0x0F, 0x7E, 0xC0]); // movq rax, xmm0
            } else if from == NumTy::F64 && to == NumTy::F32 {
                emit(code, &[0xF2, 0x0F, 0x5A, 0xC0]); // cvtsd2ss xmm0, xmm0
                emit(code, &[0x66, 0x0F, 0x7E, 0xC0]); // movd eax, xmm0 (zero-extend)
            } else {
                emit(code, &[0x66, 0x48, 0x0F, 0x7E, 0xC0]); // same float type: movq rax, xmm0
            }
        }
        // float -> int is always routed to the helper above (per-width saturation is fiddly by hand).
        (true, false) => unreachable!("float->int conversions route through the helper"),
    }
}

/// Evaluate `e` into `rax` (generic over the IR — the meta property). `host_ctx` is the word threaded to
/// any `HostCtx1`/`HostCtx2` — see [`emit_call_host_ctx1`]'s doc for why it is a DIFFERENT value than the
/// Dispatch-mode `ctx` slot (`spill_table`, see `compile_module_func`), even though both are plain `i64`
/// words that could otherwise be confused.
fn emit_expr(code: &mut Vec<u8>, inst: &Inst, host_ctx: Value, e: &Expr) {
    match e {
        Expr::Imm(v) => {
            emit(code, &[0x48, 0xB8]); // movabs rax, imm64
            emit_i64(code, *v);
        }
        Expr::ImmF64(f) => {
            emit(code, &[0x48, 0xB8]); // movabs rax, <f64 bits>
            emit_i64(code, f.to_bits() as i64);
        }
        Expr::OperandImm(i) => {
            emit(code, &[0x48, 0xC7, 0xC0]); // mov rax, imm32 (sign-extended) — matches `operand as i64`
            emit_i32(code, inst.operand(*i));
        }
        Expr::LocalAt(i) => {
            emit(code, &[0x48, 0x8B, 0x85]); // mov rax, [rbp+disp32]
            emit_i32(code, disp(inst.operand(*i) as usize));
        }
        Expr::Bin(op, a, b) => {
            emit_expr(code, inst, host_ctx, a);
            emit(code, &[0x50]); // push rax  (save lhs)
            emit_expr(code, inst, host_ctx, b); // rhs in rax
            match op.ty {
                // f16 arithmetic → canonical helper (no SSE f16); lhs on stack, rhs in rax
                NumTy::F16 => emit_call_binop(code, op.ty, op.op),
                NumTy::F64 | NumTy::F32 => {
                    let f64ty = op.ty == NumTy::F64;
                    setup_float(code); // xmm0 = lhs, xmm1 = rhs
                    match op.op {
                        ArithOp::Add => emit(
                            code,
                            if f64ty {
                                &[0xF2, 0x0F, 0x58, 0xC1]
                            } else {
                                &[0xF3, 0x0F, 0x58, 0xC1]
                            },
                        ),
                        ArithOp::Sub => emit(
                            code,
                            if f64ty {
                                &[0xF2, 0x0F, 0x5C, 0xC1]
                            } else {
                                &[0xF3, 0x0F, 0x5C, 0xC1]
                            },
                        ),
                        ArithOp::Mul => emit(
                            code,
                            if f64ty {
                                &[0xF2, 0x0F, 0x59, 0xC1]
                            } else {
                                &[0xF3, 0x0F, 0x59, 0xC1]
                            },
                        ),
                        ArithOp::Div => emit(
                            code,
                            if f64ty {
                                &[0xF2, 0x0F, 0x5E, 0xC1]
                            } else {
                                &[0xF3, 0x0F, 0x5E, 0xC1]
                            },
                        ),
                        ArithOp::Lt | ArithOp::Eq => {
                            // ucomisd/ucomiss xmm0, xmm1
                            emit(
                                code,
                                if f64ty {
                                    &[0x66, 0x0F, 0x2E, 0xC1]
                                } else {
                                    &[0x0F, 0x2E, 0xC1]
                                },
                            );
                            if op.op == ArithOp::Lt {
                                emit(code, &[0x0F, 0x92, 0xC0]); // setb al (CF: a<b incl unordered)
                            } else {
                                emit(code, &[0x0F, 0x94, 0xC0]); // sete al (ZF: equal incl unordered)
                            }
                            emit(code, &[0x0F, 0x9B, 0xC1]); // setnp cl (PF=0: ordered)
                            emit(code, &[0x20, 0xC8]); // and al, cl
                            emit(code, &[0x0F, 0xB6, 0xC0]); // movzx eax, al
                            return;
                        }
                    }
                    // arithmetic: move the result word back (low 32 for f32, with high 32 already 0)
                    emit(code, &[0x66, 0x48, 0x0F, 0x7E, 0xC0]); // movq rax, xmm0
                }
                _ => {
                    setup_int(code); // rax = lhs, rcx = rhs
                    match op.op {
                        ArithOp::Add => {
                            emit(code, &[0x48, 0x01, 0xC8]); // add rax, rcx
                            norm_int_baseline(code, op.ty);
                        }
                        ArithOp::Sub => {
                            emit(code, &[0x48, 0x29, 0xC8]); // sub rax, rcx
                            norm_int_baseline(code, op.ty);
                        }
                        ArithOp::Mul => {
                            emit(code, &[0x48, 0x0F, 0xAF, 0xC1]); // imul rax, rcx
                            norm_int_baseline(code, op.ty);
                        }
                        ArithOp::Lt => {
                            emit(code, &[0x48, 0x39, 0xC8]); // cmp rax, rcx
                            if op.ty.is_signed_int() {
                                emit(code, &[0x0F, 0x9C, 0xC0]); // setl al
                            } else {
                                emit(code, &[0x0F, 0x92, 0xC0]); // setb al (unsigned)
                            }
                            emit(code, &[0x0F, 0xB6, 0xC0]); // movzx eax, al
                        }
                        ArithOp::Eq => {
                            emit(code, &[0x48, 0x39, 0xC8]); // cmp rax, rcx
                            emit(code, &[0x0F, 0x94, 0xC0]); // sete al
                            emit(code, &[0x0F, 0xB6, 0xC0]); // movzx eax, al
                        }
                        ArithOp::Div => unreachable!("integer division not yet supported"),
                    }
                }
            }
        }
        Expr::Un(op, a) => {
            emit_expr(code, inst, host_ctx, a); // operand in rax
            match op {
                UnOp::Neg(ty) => match ty {
                    NumTy::F64 => {
                        emit(code, &[0x48, 0xB9]); // movabs rcx, sign-bit
                        emit_i64(code, i64::MIN);
                        emit(code, &[0x48, 0x31, 0xC8]); // xor rax, rcx
                    }
                    // Canonical `UnOp::eval` F32 arm is `from_f32(-as_f32(a))` = truncate to the
                    // low 32 bits (`w as u32`), flip bit 31, zero-extend — `mov eax, eax` performs
                    // exactly that truncation+zero-extension, and the 32-bit xor flips only bit 31,
                    // keeping the upper bits zero. (The previous full-width `xor rax, 0x8000_0000`
                    // preserved non-normalized high bits and diverged from T0/T2 on such inputs —
                    // caught by the W-0.5 Un(Neg)×11-NumTy differential fuzz.)
                    NumTy::F32 => {
                        emit(code, &[0x89, 0xC0]); // mov eax, eax (zero-extend low 32)
                        emit(code, &[0x35]); // xor eax, imm32
                        emit_i32(code, 0x8000_0000u32 as i32);
                    }
                    // W-0.5 (O-13 design §4.1 ③): IEEE binary16 sign flip, bit-identical to the
                    // canonical `UnOp::eval` arm `((a as u16) ^ 0x8000) as Value`. `movzx eax, ax`
                    // is exactly `a as u16` + zero-extension (writing eax clears bits 32..64,
                    // movzx from ax clears bits 16..32); the 32-bit `xor eax, 0x8000` flips only
                    // bit 15 and keeps the upper bits zero. The old fall-through to the integer
                    // arm (64-bit `neg rax`, no-op norm) matched T0's OLD wrong result only by
                    // coincidence — aligned here together with the canonical fix.
                    NumTy::F16 => {
                        emit(code, &[0x0F, 0xB7, 0xC0]); // movzx eax, ax
                        emit(code, &[0x35]); // xor eax, imm32
                        emit_i32(code, 0x8000);
                    }
                    _ => {
                        emit(code, &[0x48, 0xF7, 0xD8]); // neg rax
                        norm_int_baseline(code, *ty);
                    }
                },
                UnOp::Convert { from, to } => emit_convert(code, *from, *to),
            }
        }
        Expr::Host1(f, a) => {
            emit_expr(code, inst, host_ctx, a); // operand in rax
            emit_call_host1(code, *f);
        }
        Expr::Host2(f, a, b) => {
            emit_expr(code, inst, host_ctx, a);
            emit(code, &[0x50]); // push rax  (save lhs)
            emit_expr(code, inst, host_ctx, b); // rhs in rax
            emit_call_host2(code, *f);
        }
        Expr::HostCtx1(f, a) => {
            emit_expr(code, inst, host_ctx, a); // operand in rax
            emit_call_host_ctx1(code, *f, host_ctx);
        }
        Expr::HostCtx2(f, a, b) => {
            emit_expr(code, inst, host_ctx, a);
            emit(code, &[0x50]); // push rax  (save lhs)
            emit_expr(code, inst, host_ctx, b); // rhs in rax
            emit_call_host_ctx2(code, *f, host_ctx);
        }
    }
}

/// Compile `prog` to host machine code. Linear emission in pc order (sequential fall-through is free);
/// `JumpTo`/`JumpIf` emit a rel32 branch fixed up in a second pass once every pc's offset is known.
///
/// **No `Module`, so `host_ctx` is fixed at `0`** for any `HostCtx1`/`HostCtx2` in `prog` — this is the
/// `otter::engine::Engine` (single-function tier-up) compiled path; see
/// `otter::interp::interpret_from`'s doc for the matching interpreter-tier decision this mirrors, and
/// `otter_cranelift::compile_fn`'s doc for the T2 sibling of this same decision.
pub fn compile(isa: &Isa, prog: &[Inst], nlocals: usize, nargs: usize) -> BaselineCode {
    const HOST_CTX: Value = 0;
    let mut code: Vec<u8> = Vec::with_capacity(64 + prog.len() * 24);

    // prologue: push rbp; mov rbp,rsp; sub rsp, frame
    emit(&mut code, &[0x55]);
    emit(&mut code, &[0x48, 0x89, 0xE5]);
    let frame = ((nlocals * 8).div_ceil(16) * 16) as i32; // 16-byte aligned
    emit(&mut code, &[0x48, 0x81, 0xEC]);
    emit_i32(&mut code, frame);

    // initialize locals: first `nargs` from the args pointer (rdi), the rest to zero
    for i in 0..nlocals {
        if i < nargs {
            emit(&mut code, &[0x48, 0x8B, 0x87]); // mov rax, [rdi+disp32]
            emit_i32(&mut code, (i * 8) as i32);
            emit(&mut code, &[0x48, 0x89, 0x85]); // mov [rbp+disp32], rax
            emit_i32(&mut code, disp(i));
        } else {
            emit(&mut code, &[0x48, 0xC7, 0x85]); // mov qword [rbp+disp32], imm32(0)
            emit_i32(&mut code, disp(i));
            emit_i32(&mut code, 0);
        }
    }

    // body
    let mut pc_off = vec![0usize; prog.len()];
    let mut fixups: Vec<(usize, usize)> = Vec::new(); // (rel32 field offset, target pc)
    for (pc, inst) in prog.iter().enumerate() {
        pc_off[pc] = code.len();
        for eff in &isa.op(inst.op).effects {
            match eff {
                Effect::SetLocalAt { reg, value } => {
                    emit_expr(&mut code, inst, HOST_CTX, value);
                    emit(&mut code, &[0x48, 0x89, 0x85]); // mov [rbp+disp32], rax
                    emit_i32(&mut code, disp(inst.operand(*reg) as usize));
                }
                Effect::JumpTo { target } => {
                    emit(&mut code, &[0xE9]); // jmp rel32
                    let f = code.len();
                    emit_i32(&mut code, 0);
                    fixups.push((f, inst.operand(*target) as usize));
                }
                Effect::JumpIf { cond, target } => {
                    emit_expr(&mut code, inst, HOST_CTX, cond);
                    emit(&mut code, &[0x48, 0x85, 0xC0]); // test rax, rax
                    emit(&mut code, &[0x0F, 0x85]); // jne rel32
                    let f = code.len();
                    emit_i32(&mut code, 0);
                    fixups.push((f, inst.operand(*target) as usize));
                }
                Effect::Halt { value } => {
                    emit_expr(&mut code, inst, HOST_CTX, value);
                    emit(&mut code, &[0xC9, 0xC3]); // leave ; ret  (returns rax)
                }
                // the baseline tier does not speculate; a guard is a transparent no-op here
                Effect::Guard { .. } => {}
                // Function calls run on the interpreter oracle (`run_module`); baseline only ever
                // compiles call-free single functions, so this is never reached.
                Effect::Call { .. } | Effect::Return { .. } | Effect::TailCall { .. } => {
                    unimplemented!("Call/Return/TailCall lowering is not implemented (single-function tier)")
                }
            }
        }
    }

    // resolve branches: rel32 = target_offset - (field_offset + 4)
    for (f, target_pc) in fixups {
        let rel = pc_off[target_pc] as i64 - (f as i64 + 4);
        patch_i32(
            &mut code,
            f,
            i32::try_from(rel).expect("branch displacement out of i32 range"),
        );
    }

    // SAFETY: `code` is well-formed host machine code for `extern "C" fn(*const i64) -> i64`.
    let buf = unsafe { ExecBuf::map(&code) };
    BaselineCode { buf, nargs }
}

// --- W^X executable buffer --------------------------------------------------------------------------

struct ExecBuf {
    ptr: *mut u8,
    len: usize,
}

impl ExecBuf {
    /// Map `code` into fresh memory as RW, copy it, then flip to RX (W^X — never W+X at once).
    ///
    /// # Safety
    /// `code` must be valid host machine code for the intended call signature.
    unsafe fn map(code: &[u8]) -> Self {
        // SAFETY: sysconf with a static query name has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as usize;
        let len = code.len().max(1).div_ceil(page) * page;
        // SAFETY: anonymous private RW mapping.
        let p = unsafe {
            libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert!(p != libc::MAP_FAILED, "mmap failed");
        let p = p.cast::<u8>();
        // SAFETY: dst is our mapping of `len >= code.len()` bytes.
        unsafe { ptr::copy_nonoverlapping(code.as_ptr(), p, code.len()) };
        // Construct the RAII guard BEFORE the fallible `mprotect` call below, not after: if `mprotect`
        // fails and the `assert!` panics, `buf`'s `Drop` (`munmap`) still runs during unwind, so the
        // mapping is released rather than leaked. Building `ExecBuf { ptr, len }` only AFTER a successful
        // `mprotect` (the naive order) would leave `p`/`len` as plain locals with no `Drop` at the moment
        // of that panic — the RW mapping would then leak for the rest of the process, and a caller that
        // retries the same (deterministically failing) compile after un-poisoning its engine (see
        // `otter-engine`'s `ModuleEngine::clear_trap`) would leak one mapping per retry. See otter-engine's
        // O-10 design doc, panel Lens A finding #1.
        let buf = ExecBuf { ptr: p, len };
        // SAFETY: flip the whole mapping to RX; W is dropped (W^X).
        let rc = unsafe { libc::mprotect(p.cast(), len, libc::PROT_READ | libc::PROT_EXEC) };
        assert!(rc == 0, "mprotect RX failed");
        buf
    }
}

impl Drop for ExecBuf {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly the region we mapped.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// A baseline-compiled function: an executable buffer plus its arity.
pub struct BaselineCode {
    buf: ExecBuf,
    nargs: usize,
}

impl BaselineCode {
    /// Call the compiled function. Result equals the interpreter's (verified by the differential tests).
    pub fn call(&self, args: &[Value]) -> Value {
        assert!(
            args.len() >= self.nargs,
            "baseline fn needs >= {} args",
            self.nargs
        );
        let argp: *const i64 = if args.is_empty() {
            ptr::null()
        } else {
            args.as_ptr()
        };
        // SAFETY: compiled with signature `extern "C" fn(*const i64) -> i64`; it reads only the first
        // `nargs` (<= args.len()) slots; the code stays mapped in `self.buf`.
        let f: extern "C" fn(*const i64) -> i64 = unsafe { core::mem::transmute(self.buf.ptr) };
        f(argp)
    }
}

impl Code for BaselineCode {
    fn run(&self, args: &[Value]) -> Value {
        self.call(args)
    }
}

// ---------------------------------------------------------------------------------------------------
// Module (function-call) lowering — baseline (T1) call lowering, the x86-64 mirror of
// `otter-cranelift::compile_module`. Same read-only fn-pointer-table ABI, so the two backends' table
// entries are interchangeable: `extern "C" fn(table: *const usize, args: *const i64, argc: i64) -> i64`
// (System V: rdi=table, rsi=args, rdx=argc; result in rax). Calls lower to an aligned indirect call
// through `table[callee_idx]`; the callee word in a local is a raw function index (run_module / cranelift
// convention). Per the B1.4 design panel: spill (table,args,argc) to fixed frame slots so the existing
// rax/rcx scratch + push/pop expression model is untouched; keep the per-call argument buffer INSIDE
// the rbp-relative frame (a pre-scanned max-argc region) so the alignment dance can't corrupt it.
// ---------------------------------------------------------------------------------------------------

/// The uniform compiled-function ABI: `(table, args, argc) -> result`.
type ModuleEntry = extern "C" fn(*const usize, *const i64, i64) -> Value;

/// A baseline-compiled [`otter::Module`]: one executable buffer per function + the function-pointer
/// table threaded into every call.
pub struct BaselineModule {
    #[allow(dead_code)] // kept alive so the code stays mapped
    bufs: Vec<ExecBuf>,
    table: Box<[usize]>,
    entry: usize,
}

impl BaselineModule {
    /// Run the module's entry function with `args`. Equal to `interp::run_module` for the same module.
    pub fn run(&self, args: &[Value]) -> Value {
        let argp: *const i64 = if args.is_empty() {
            ptr::null()
        } else {
            args.as_ptr()
        };
        // SAFETY: every table entry was compiled with the `ModuleEntry` signature; the bufs + table are
        // kept alive in `self`; the entry reads only the first `argc` args.
        let f: ModuleEntry = unsafe { core::mem::transmute(self.table[self.entry]) };
        f(self.table.as_ptr(), argp, args.len() as i64)
    }
}

/// How a compiled function lowers its `Call`s (and, for `Dispatch`, its `TailCall`s) — see
/// `otter_cranelift::compile_dispatch_fn`. `Table` = indirect call through the read-only fn-ptr table
/// (param 0 = table; AOT whole-module); `TailCall` is NOT lowered in this mode (out of scope — design
/// §8.4/§8.8 pick C3: a separate static AOT product, not the `ModuleEngine`). `Dispatch(dispatch_addr,
/// tail_addr)` = call the baked Rust dispatcher `dispatch(ctx, callee_idx, args, argc)` for `Call`, or the
/// `tail_signal` trampoline-signal helper (same ABI) for `TailCall` (param 0 = engine `ctx`; the adaptive
/// engine, so the callee runs at its current tier — O-8c-2).
#[derive(Clone, Copy)]
enum CallMode {
    Table,
    Dispatch(usize, usize),
}

/// Compile an entire [`otter::Module`] to x86-64. Each function is a separate `ExecBuf`; the table holds
/// their entry addresses. (No adaptive tiering — the whole module at the baseline tier; verified
/// bit-identical to `interp::run_module`.)
pub fn compile_module(isa: &Isa, m: &Module) -> BaselineModule {
    let bufs: Vec<ExecBuf> = m
        .funcs
        .iter()
        .map(|f| compile_module_func(isa, f, m.fill, m.host_ctx, CallMode::Table))
        .collect();
    let table: Box<[usize]> = bufs.iter().map(|b| b.ptr as usize).collect();
    BaselineModule {
        bufs,
        table,
        entry: m.entry,
    }
}

/// A single function compiled for the **adaptive engine** (baseline / T1 tier): ABI
/// `extern "C" fn(ctx, args, argc) -> i64`, each `Call` lowered to `dispatch(ctx, callee_idx, args,
/// argc)`. Mirror of `otter_cranelift::CompiledDispatchFn`.
pub struct CompiledDispatchFn {
    #[allow(dead_code)]
    buf: ExecBuf,
}

impl CompiledDispatchFn {
    /// The raw code pointer (`extern "C" fn(*const u8 ctx, *const i64 args, i64 argc) -> i64`).
    pub fn code_ptr(&self) -> *const u8 {
        self.buf.ptr
    }
}

/// Compile function `func_idx` (its `FuncDef` is `f`, and `fill`/`host_ctx` are the owning module's fill
/// and [`otter::Module::host_ctx`] words) for the adaptive engine (T1). `dispatch_addr` is the address of
/// the engine's `extern "C" fn(ctx, callee_idx, args, argc) -> i64` dispatcher, baked into every Call.
/// `tail_addr` is the address of the engine's `tail_signal` trampoline-signal helper (same ABI), baked
/// into every `TailCall` (O-8c-2 — proper tail calls at T1; mirrors `otter_cranelift::compile_dispatch_fn`;
/// see `.claude-research-library/otter-o8-tailcall-design.md` §8.1/§8.8).
///
/// Takes `f: &FuncDef` / `fill: Value` directly rather than `&otter::Module` (O-9: `ModuleEngine::push_func`
/// injects functions that never live inside an `otter::Module` at all — see the O-9 design doc §1.1).
/// `func_idx` is accepted for signature symmetry with `otter_cranelift::compile_dispatch_fn` (which needs
/// it to name the JIT-internal function symbol); this backend's stencil emission has no per-function
/// symbol table, so it doesn't need `func_idx` itself. `host_ctx` (O-11) rides alongside `fill` for the
/// same reason as `otter_cranelift::compile_dispatch_fn` — see its doc.
pub fn compile_dispatch_fn(
    isa: &Isa,
    _func_idx: usize,
    f: &FuncDef,
    fill: Value,
    host_ctx: Value,
    dispatch_addr: usize,
    tail_addr: usize,
) -> CompiledDispatchFn {
    let buf = compile_module_func(
        isa,
        f,
        fill,
        host_ctx,
        CallMode::Dispatch(dispatch_addr, tail_addr),
    );
    CompiledDispatchFn { buf }
}

/// `mov rax, qword imm64`.
fn emit_movabs_rax(code: &mut Vec<u8>, v: i64) {
    emit(code, &[0x48, 0xB8]);
    emit_i64(code, v);
}
/// `mov rax, [rbp+disp32]` / `mov [rbp+disp32], rax`.
fn emit_load_rax(code: &mut Vec<u8>, d: i32) {
    emit(code, &[0x48, 0x8B, 0x85]);
    emit_i32(code, d);
}
fn emit_store_rax(code: &mut Vec<u8>, d: i32) {
    emit(code, &[0x48, 0x89, 0x85]);
    emit_i32(code, d);
}
/// An rsp-aligned indirect `call rax` (target already in `rax`; preserved across the alignment via the
/// rbx idiom — the callee preserves rbx). Result lands in `rax`.
fn emit_aligned_call_rax(code: &mut Vec<u8>) {
    emit(code, &[0x53]); // push rbx
    emit(code, &[0x48, 0x89, 0xE3]); // mov rbx, rsp
    emit(code, &[0x48, 0x83, 0xE4, 0xF0]); // and rsp, -16
    emit(code, &[0xFF, 0xD0]); // call rax
    emit(code, &[0x48, 0x89, 0xDC]); // mov rsp, rbx
    emit(code, &[0x5B]); // pop rbx
}

fn compile_module_func(isa: &Isa, f: &FuncDef, fill: Value, host_ctx: Value, call_mode: CallMode) -> ExecBuf {
    let nlocals = f.nlocals;
    let prog = &f.code;

    // pre-scan the widest call so the in-frame argument buffer is sized once. MUST include `TailCall`
    // (design §8.8 pick C1, blocking): a function containing only `TailCall`s and no ordinary `Call`
    // (the typical `(let loop ...)` tail-recursion shape) would otherwise get `maxargc=0`, leaving no
    // frame slots reserved at `argbuf0` — the TailCall arg-marshaling loop below would then write past
    // the frame into unreserved/adjacent stack space.
    let mut maxargc = 0usize;
    for inst in prog {
        for eff in &isa.op(inst.op).effects {
            if let Effect::Call { argc, .. } | Effect::TailCall { argc, .. } = eff {
                maxargc = maxargc.max(inst.operand(*argc) as usize);
            }
        }
    }
    // frame layout (all via `disp(slot) = -8*(slot+1)`, so `emit_expr`'s local access is unchanged):
    //   [0 .. nlocals)               locals
    //   nlocals, +1, +2              spilled table / args / argc
    //   nlocals+3 .. +3+maxargc      per-call argument buffer
    //
    // NOTE (O-11): `spill_table` (below) is UNRELATED to `host_ctx` (this fn's parameter), despite the
    // Dispatch-mode comment further down calling that spill slot "ctx" too — `spill_table` holds the
    // engine's own bookkeeping pointer (Table mode: the fn-ptr table; Dispatch mode: the `ModuleEngine`
    // pointer forwarded to `dispatch`/`tail_signal`), never dereferenced by this function's own code and
    // never passed to a host builtin. `host_ctx` is the embedder-owned `otter::Module::host_ctx` word,
    // baked as a fresh immediate at each `HostCtx1`/`HostCtx2` call site (`emit_call_host_ctx1`/`2` — see
    // their docs) — it has no spill slot of its own and is never confused with `spill_table`.
    let spill_table = nlocals;
    let spill_args = nlocals + 1;
    let spill_argc = nlocals + 2;
    let argbuf0 = nlocals + 3;
    let total_slots = nlocals + 3 + maxargc;
    let frame = ((total_slots * 8).div_ceil(16) * 16) as i32;

    let mut code: Vec<u8> = Vec::with_capacity(64 + prog.len() * 28);

    // prologue
    emit(&mut code, &[0x55]); // push rbp
    emit(&mut code, &[0x48, 0x89, 0xE5]); // mov rbp, rsp
    emit(&mut code, &[0x48, 0x81, 0xEC]); // sub rsp, frame
    emit_i32(&mut code, frame);
    // spill the ABI registers
    emit(&mut code, &[0x48, 0x89, 0xBD]); // mov [rbp+disp], rdi (table)
    emit_i32(&mut code, disp(spill_table));
    emit(&mut code, &[0x48, 0x89, 0xB5]); // mov [rbp+disp], rsi (args)
    emit_i32(&mut code, disp(spill_args));
    emit(&mut code, &[0x48, 0x89, 0x95]); // mov [rbp+disp], rdx (argc)
    emit_i32(&mut code, disp(spill_argc));
    // initialize all locals to `fill`
    emit_movabs_rax(&mut code, fill);
    for i in 0..nlocals {
        emit_store_rax(&mut code, disp(i));
    }
    // copy parameters, each guarded by argc (absent args stay `fill`)
    for i in 0..f.nparams {
        emit_load_rax(&mut code, disp(spill_argc)); // rax = argc
        emit(&mut code, &[0x48, 0x3D]); // cmp rax, imm32
        emit_i32(&mut code, i as i32);
        emit(&mut code, &[0x0F, 0x8E]); // jle rel32 (skip — arg absent)
        let j = code.len();
        emit_i32(&mut code, 0);
        emit(&mut code, &[0x48, 0x8B, 0x8D]); // mov rcx, [rbp+disp(spill_args)]
        emit_i32(&mut code, disp(spill_args));
        emit(&mut code, &[0x48, 0x8B, 0x81]); // mov rax, [rcx + 8*i]
        emit_i32(&mut code, (i * 8) as i32);
        emit_store_rax(&mut code, disp(i)); // locals[i] = arg i
        let after = code.len();
        patch_i32(&mut code, j, (after - (j + 4)) as i32);
    }
    // bake the constant pool into its slots
    for (jc, c) in f.consts.iter().enumerate() {
        emit_movabs_rax(&mut code, *c);
        emit_store_rax(&mut code, disp(f.nparams + jc));
    }

    // body — linear emission with a rel32 second-pass for branches
    let mut pc_off = vec![0usize; prog.len()];
    let mut fixups: Vec<(usize, usize)> = Vec::new();
    for (pc, inst) in prog.iter().enumerate() {
        pc_off[pc] = code.len();
        for eff in &isa.op(inst.op).effects {
            match eff {
                Effect::SetLocalAt { reg, value } => {
                    emit_expr(&mut code, inst, host_ctx, value);
                    emit_store_rax(&mut code, disp(inst.operand(*reg) as usize));
                }
                Effect::JumpTo { target } => {
                    emit(&mut code, &[0xE9]); // jmp rel32
                    let fx = code.len();
                    emit_i32(&mut code, 0);
                    fixups.push((fx, inst.operand(*target) as usize));
                }
                Effect::JumpIf { cond, target } => {
                    emit_expr(&mut code, inst, host_ctx, cond);
                    emit(&mut code, &[0x48, 0x85, 0xC0]); // test rax, rax
                    emit(&mut code, &[0x0F, 0x85]); // jne rel32
                    let fx = code.len();
                    emit_i32(&mut code, 0);
                    fixups.push((fx, inst.operand(*target) as usize));
                }
                // Halt ends the module; Return ends a frame — both are a native return here, since each
                // function is its own frame and the caller resumes after its `call`.
                Effect::Halt { value } | Effect::Return { value } => {
                    emit_expr(&mut code, inst, host_ctx, value);
                    emit(&mut code, &[0xC9, 0xC3]); // leave ; ret
                }
                Effect::Guard { .. } => {} // baseline does not speculate
                // O-8c-2: TailCall lowers to the SAME dispatcher-trampoline signal cranelift uses (O-8c-1)
                // — marshal args exactly like `Call` below (reusing the existing argbuf-reversal
                // marshaling), but call `tail_signal` instead of `dispatch` and fall straight into the
                // existing Return/Halt epilogue instead of storing a `dest` (design §8.1/§8.8 pick C2).
                // This must NEVER be `Call`+`Return` (a native call that then returns) — that would grow
                // the native stack per tail step and violate proper tail recursion; the actual emission
                // below is a call to `tail_signal` immediately followed by `leave;ret`, so this native
                // frame is torn down and control returns to the nearest `dispatch()` trampoline loop
                // (otter-engine), which re-enters the callee without recursing.
                Effect::TailCall { base, argc } => {
                    let base_slot = inst.operand(*base) as usize;
                    let n = inst.operand(*argc) as usize;
                    // Marshal args locals[base+1 .. base+1+n] into the in-frame argument buffer — IDENTICAL
                    // reversal convention to `Call` below (see its comment): store arg `k` at slot
                    // `argbuf0 + (n-1-k)` so the ascending-address view the callee reads matches.
                    for k in 0..n {
                        emit_load_rax(&mut code, disp(base_slot + 1 + k));
                        emit_store_rax(&mut code, disp(argbuf0 + (n - 1 - k)));
                    }
                    match call_mode {
                        CallMode::Table => unimplemented!(
                            "TailCall is not lowered in Table (whole-module AOT) mode — the ModuleEngine \
                             (Dispatch mode) is the only O-8c target; see design §8.4/§8.8 pick C3"
                        ),
                        CallMode::Dispatch(_dispatch_addr, tail_addr) => {
                            // tail_signal(ctx, callee_idx, args, argc) — same register marshaling as the
                            // Dispatch `Call` case below, just targeting `tail_addr` instead of
                            // `dispatch_addr`.
                            emit(&mut code, &[0x48, 0x8B, 0xBD]); // mov rdi, [rbp+disp(ctx)]
                            emit_i32(&mut code, disp(spill_table)); // (slot holds ctx in this mode)
                            emit(&mut code, &[0x48, 0x8B, 0xB5]); // mov rsi, [rbp+disp(base)] (callee idx)
                            emit_i32(&mut code, disp(base_slot));
                            if n > 0 {
                                emit(&mut code, &[0x48, 0x8D, 0x95]); // lea rdx, [rbp+disp(argbuf0+n-1)]
                                emit_i32(&mut code, disp(argbuf0 + (n - 1)));
                            } else {
                                emit(&mut code, &[0x31, 0xD2]); // xor edx, edx
                            }
                            emit(&mut code, &[0x48, 0xC7, 0xC1]); // mov rcx, imm32 (argc)
                            emit_i32(&mut code, n as i32);
                            emit_movabs_rax(&mut code, tail_addr as i64); // rax = tail_signal address
                        }
                    }
                    emit_aligned_call_rax(&mut code);
                    // `tail_signal`'s return value (the module's `fill`) is ALREADY in rax right here —
                    // `emit_aligned_call_rax`'s post-call cleanup (`mov rsp,rbx; pop rbx`) does not touch
                    // rax (design §8.8 pick C2) — so, unlike Return/Halt (which `emit_expr` a value into
                    // rax first), there is nothing to (re)compute or reload: fall straight into the SAME
                    // epilogue bytes Return/Halt use.
                    emit(&mut code, &[0xC9, 0xC3]); // leave ; ret
                }
                Effect::Call { dest, base, argc } => {
                    let dest_slot = inst.operand(*dest) as usize;
                    let base_slot = inst.operand(*base) as usize;
                    let n = inst.operand(*argc) as usize;
                    // Marshal args locals[base+1 .. base+1+n] into the in-frame argument buffer. Frame
                    // slots grow DOWNWARD (higher slot index ⇒ lower address), but the callee reads
                    // args as an ASCENDING array `args[k] = *(args_ptr + 8k)`. So store arg `k` at slot
                    // `argbuf0 + (n-1-k)` and point `args_ptr` at the deepest (lowest-address) slot —
                    // making the in-memory order ascending. (Cranelift's stack-slot buffer is naturally
                    // ascending; only the rbp-relative frame needs this reversal.)
                    for k in 0..n {
                        emit_load_rax(&mut code, disp(base_slot + 1 + k));
                        emit_store_rax(&mut code, disp(argbuf0 + (n - 1 - k)));
                    }
                    match call_mode {
                        CallMode::Table => {
                            // fnptr = table[callee]; fnptr(table, args, argc)  → rdi,rsi,rdx
                            emit(&mut code, &[0x48, 0x8B, 0xBD]); // mov rdi, [rbp+disp(table)]
                            emit_i32(&mut code, disp(spill_table));
                            if n > 0 {
                                emit(&mut code, &[0x48, 0x8D, 0xB5]); // lea rsi, [rbp+disp(argbuf0+n-1)]
                                emit_i32(&mut code, disp(argbuf0 + (n - 1)));
                            } else {
                                emit(&mut code, &[0x31, 0xF6]); // xor esi, esi
                            }
                            emit(&mut code, &[0x48, 0xC7, 0xC2]); // mov rdx, imm32 (argc)
                            emit_i32(&mut code, n as i32);
                            emit(&mut code, &[0x48, 0x8B, 0x8D]); // mov rcx, [rbp+disp(base)] (callee idx)
                            emit_i32(&mut code, disp(base_slot));
                            emit(&mut code, &[0x48, 0x8B, 0x04, 0xCF]); // mov rax, [rdi + rcx*8]
                        }
                        CallMode::Dispatch(addr, _tail_addr) => {
                            // dispatch(ctx, callee_idx, args, argc)  → rdi,rsi,rdx,rcx
                            emit(&mut code, &[0x48, 0x8B, 0xBD]); // mov rdi, [rbp+disp(ctx)]
                            emit_i32(&mut code, disp(spill_table)); // (slot holds ctx in this mode)
                            emit(&mut code, &[0x48, 0x8B, 0xB5]); // mov rsi, [rbp+disp(base)] (callee idx)
                            emit_i32(&mut code, disp(base_slot));
                            if n > 0 {
                                emit(&mut code, &[0x48, 0x8D, 0x95]); // lea rdx, [rbp+disp(argbuf0+n-1)]
                                emit_i32(&mut code, disp(argbuf0 + (n - 1)));
                            } else {
                                emit(&mut code, &[0x31, 0xD2]); // xor edx, edx
                            }
                            emit(&mut code, &[0x48, 0xC7, 0xC1]); // mov rcx, imm32 (argc)
                            emit_i32(&mut code, n as i32);
                            emit_movabs_rax(&mut code, addr as i64); // rax = dispatch fn address
                        }
                    }
                    emit_aligned_call_rax(&mut code);
                    emit_store_rax(&mut code, disp(dest_slot)); // dest = result
                }
            }
        }
    }
    for (fx, target_pc) in fixups {
        let rel = pc_off[target_pc] as i64 - (fx as i64 + 4);
        patch_i32(
            &mut code,
            fx,
            i32::try_from(rel).expect("branch displacement out of i32 range"),
        );
    }

    // SAFETY: `code` is well-formed host machine code for the `ModuleEntry` signature.
    unsafe { ExecBuf::map(&code) }
}

/// The copy-and-patch baseline JIT as an [`otter::engine::Backend`] — the T1 tier of the engine.
pub struct BaselineBackend;

impl Backend for BaselineBackend {
    fn name(&self) -> &'static str {
        "baseline"
    }
    fn compile(&self, isa: &Isa, prog: &[Inst], nlocals: usize, nargs: usize) -> Box<dyn Code> {
        Box::new(compile(isa, prog, nlocals, nargs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otter::interp::{interpret, interpret_with};
    use otter::sample::{self, ADD, HALT, LT, MOV_IMM, MUL, SUB};

    #[test]
    fn baseline_matches_interp_samples() {
        let isa = sample::isa();
        for n in [0, 1, 2, 10, 100, 1000, 100_000] {
            let p = sample::sum_program(n);
            assert_eq!(
                compile(&isa, &p, 5, 0).call(&[]),
                interpret(&isa, &p, 5),
                "sum({n})"
            );
        }
        for n in 0..=12 {
            let p = sample::factorial_program(n);
            assert_eq!(
                compile(&isa, &p, 4, 0).call(&[]),
                interpret(&isa, &p, 4),
                "fact({n})"
            );
        }
    }

    /// Float + conversion ops via SSE: baseline must agree with the interpreter bit-for-bit. (F2I is
    /// tested on finite in-range inputs; `cvttsd2si` truncation matches there. NaN/overflow saturation
    /// for the baseline F2I is a documented follow-up — the interpreter and Cranelift tiers saturate.)
    #[test]
    fn baseline_matches_interp_float() {
        let isa = sample::isa();
        let prog = sample::float_expr_program(); // (x + y) * x
        let code = compile(&isa, &prog, 3, 2);
        let cases: [(f64, f64); 6] = [
            (2.0, 3.0),
            (-1.5, 0.5),
            (0.0, -0.0),
            (1e10, -2.0),
            (f64::INFINITY, 1.0),
            (f64::NAN, 1.0),
        ];
        for (x, y) in cases {
            let args = [x.to_bits() as i64, y.to_bits() as i64];
            assert_eq!(
                code.call(&args),
                interpret_with(&isa, &prog, 3, &args),
                "baseline≠interp for ({x},{y})"
            );
        }
        let conv = sample::convert_roundtrip_program();
        let cc = compile(&isa, &conv, 2, 1);
        for n in [0i64, 1, 7, -3, 1000, -100_000] {
            assert_eq!(cc.call(&[n]), 2 * n, "2*{n}");
        }
    }

    /// Width family (narrow + unsigned ints, f32, conversions): baseline must match the interpreter
    /// bit-for-bit on in-range inputs (the saturation-edge caveats are documented on `emit_convert`).
    #[test]
    fn baseline_matches_interp_width() {
        use otter::sample::{
            CONV_F32_F64, CONV_F64_U8, CONV_I64_F32, CONV_U8_I64, F32_ADD, F32_LT, F32_MUL, HALT, I8_ADD,
            I32_ADD, U8_ADD, U32_ADD, U64_LT,
        };
        let isa = sample::isa();
        let f32b = |x: f32| x.to_bits() as i64;
        let bin = [
            (I8_ADD, 100i64, 100i64),
            (U8_ADD, 200, 100),
            (I32_ADD, 2_000_000_000, 2_000_000_000), // i32 overflow wrap
            (U32_ADD, 4_000_000_000i64, 1_000_000_000),
            (U64_LT, -1, 1),
            (U64_LT, 1, -1),
            (F32_ADD, f32b(1.5), f32b(2.25)),
            (F32_MUL, f32b(3.0), f32b(-0.5)),
            (F32_LT, f32b(1.0), f32b(2.0)),
            (F32_LT, f32b(f32::NAN), f32b(0.0)),
        ];
        for (op, a, b) in bin {
            let prog = vec![Inst::new(op, 2, 0, 1), Inst::new(HALT, 2, 0, 0)];
            let j = compile(&isa, &prog, 3, 2).call(&[a, b]);
            assert_eq!(j, interpret_with(&isa, &prog, 3, &[a, b]), "op {op} a={a} b={b}");
        }
        let un = [
            (CONV_F64_U8, 300.0_f64.to_bits() as i64), // saturates to 255 via clamp
            (CONV_F64_U8, (-5.0_f64).to_bits() as i64), // -> 0
            (CONV_U8_I64, 0xFF),
            (CONV_I64_F32, 12345),
            (CONV_F32_F64, (1.5_f32).to_bits() as i64),
        ];
        for (op, a) in un {
            let prog = vec![Inst::new(op, 1, 0, 0), Inst::new(HALT, 1, 0, 0)];
            let j = compile(&isa, &prog, 2, 1).call(&[a]);
            assert_eq!(j, interpret_with(&isa, &prog, 2, &[a]), "conv {op} a={a}");
        }
    }

    /// f16 (via helper) + the previously-documented edges now RESOLVED (via helper): float→int
    /// saturation (NaN/overflow) and unsigned-u64→float. baseline must match the interpreter exactly.
    #[test]
    fn baseline_f16_and_resolved_edges() {
        use otter::sample::{
            CONV_F16_F64, CONV_F64_F16, CONV_F64_U8, CONV_U64_F64, F2I, F16_ADD, F16_MUL, HALT,
        };
        let isa = sample::isa();
        let bin = [(F16_ADD, 0x3C00i64, 0x3800i64), (F16_MUL, 0x4000, 0x3800)];
        for (op, a, b) in bin {
            let prog = vec![Inst::new(op, 2, 0, 1), Inst::new(HALT, 2, 0, 0)];
            assert_eq!(
                compile(&isa, &prog, 3, 2).call(&[a, b]),
                interpret_with(&isa, &prog, 3, &[a, b]),
                "f16 {op}"
            );
        }
        let un = [
            (CONV_F64_F16, 0.1_f64.to_bits() as i64),
            (CONV_F16_F64, 0x3C00),
            (CONV_F64_U8, f64::NAN.to_bits() as i64), // NaN -> 0
            (CONV_F64_U8, 1e9_f64.to_bits() as i64),  // overflow -> 255
            (F2I, f64::NAN.to_bits() as i64),         // NaN -> 0
            (F2I, 1e300_f64.to_bits() as i64),        // overflow -> i64::MAX
            (F2I, (-1e300_f64).to_bits() as i64),     // -overflow -> i64::MIN
            (CONV_U64_F64, -1),                       // u64::MAX -> f64
            (CONV_U64_F64, i64::MIN),                 // 2^63 -> f64
        ];
        for (op, a) in un {
            let prog = vec![Inst::new(op, 1, 0, 0), Inst::new(HALT, 1, 0, 0)];
            assert_eq!(
                compile(&isa, &prog, 2, 1).call(&[a]),
                interpret_with(&isa, &prog, 2, &[a]),
                "conv {op} a={a}"
            );
        }
    }

    #[test]
    fn baseline_matches_interp_with_args() {
        let isa = sample::isa();
        let p = sample::sum_to_program();
        let code = compile(&isa, &p, 5, 1);
        for n in [0i64, 1, 2, 10, 100, 1000, 100_000] {
            let want = interpret_with(&isa, &p, 5, &[n]);
            assert_eq!(code.call(&[n]), want, "n={n}");
            assert_eq!(want, n * (n + 1) / 2);
        }
    }

    /// Differential fuzz: random straight-line programs, baseline vs interpreter (the T0 oracle).
    #[test]
    fn baseline_matches_interp_fuzz() {
        let isa = sample::isa();
        const NREGS: usize = 6;
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        for _ in 0..2000 {
            let mut prog = Vec::new();
            for r in 0..NREGS {
                prog.push(Inst::new(MOV_IMM, r as i32, (next() % 21) as i32 - 10, 0));
            }
            let body = 3 + (next() % 12) as usize;
            for _ in 0..body {
                let opcode = match next() % 4 {
                    0 => ADD,
                    1 => SUB,
                    2 => MUL,
                    _ => LT,
                };
                let dst = (next() as usize % NREGS) as i32;
                let a = (next() as usize % NREGS) as i32;
                let b = (next() as usize % NREGS) as i32;
                prog.push(Inst::new(opcode, dst, a, b));
            }
            prog.push(Inst::new(HALT, (next() as usize % NREGS) as i32, 0, 0));

            let j = compile(&isa, &prog, NREGS, 0).call(&[]);
            let i = interpret(&isa, &prog, NREGS);
            assert_eq!(j, i, "baseline {j} vs interp {i}\nprog={prog:?}");
        }
    }

    /// O-12 regression (design doc §6's required test, panel-endorsed by all three review lenses): a
    /// `Module` `otter::validate::check_module` approves must run panic-free AND produce the SAME result
    /// on all three tiers — a strictly stronger property than "doesn't panic" (it also catches a
    /// validated-but-corrupted program silently drifting from the oracle on just one tier). This is the
    /// only differential safety net for §1.4's finding (`otter-baseline`'s unchecked `disp()` — a
    /// panic-LESS local-index memory-corruption class no amount of `catch_unwind` can touch): blindly
    /// mutate `Inst` operands/opcodes (crossing every `ValidationError` variant's rejection boundary many
    /// times over, including the exact shapes `otter::validate::tests` exercises individually), gate
    /// EXECUTION on `check_module`'s verdict, and require T0/T1/T2 agreement on everything that passes.
    /// `compile_module` (both backends) uses `CallMode::Table`, which does not lower `TailCall` — a
    /// pre-existing, documented gap (`.claude-research-library` O-8 design doc §8.4/§8.8 pick C3), not
    /// something O-12 is meant to close — so this fuzzer only ever emits `Call`, never `TailCall`.
    #[test]
    fn validate_gated_module_fuzz_all_tiers_agree() {
        use otter::validate::check_module;
        use otter::{BinOp, OpDef, OpHints};

        const MOV_IMM: u16 = 0;
        const ADD: u16 = 1;
        const SUB: u16 = 2;
        const LT: u16 = 3;
        const COPY: u16 = 4;
        const CALL: u16 = 5;
        const HALT: u16 = 6;
        const RET: u16 = 7;

        // Deliberately NO `JumpTo`/`JumpIf` op in this ISA (unlike `call_isa()` in `otter-engine`'s own
        // tests, which this is otherwise modeled on) — `check_func`/`check_module` only prove absence of
        // *indexing* panics (module doc, "What this does NOT prove"), never TERMINATION; a jump target
        // that survives validation (in-bounds) can still encode a perfectly valid, indefinitely-looping
        // backward branch, which no amount of index-range checking can rule out (that's undecidable in
        // general, and explicitly out of `check_func`'s scope). A blind opcode/operand fuzzer that CAN
        // synthesize a check_module-approved infinite loop WILL eventually do so given enough trials, and
        // then hang this test forever — not a defect in `otter`, a scoping bug in a fuzzer that reaches
        // for a tool outside its stated guarantees. Every function body here is a straight-line DAG (at
        // most one `Call`, no backward control flow of any kind is even REPRESENTABLE in this ISA), which
        // rules out non-termination by construction rather than by probability. Jump-target validation
        // itself is already covered directly and cheaply by `otter::validate::tests::jump_target_*`.
        fn call_isa() -> Isa {
            let pure = OpHints {
                pure: true,
                terminator: false,
            };
            let term = OpHints {
                pure: false,
                terminator: true,
            };
            let alu = |name, op| OpDef {
                name,
                effects: vec![Effect::SetLocalAt {
                    reg: 0,
                    value: Expr::Bin(
                        BinOp::new(NumTy::I64, op),
                        Box::new(Expr::LocalAt(1)),
                        Box::new(Expr::LocalAt(2)),
                    ),
                }],
                hints: pure,
            };
            Isa {
                ops: vec![
                    OpDef {
                        name: "mov_imm",
                        effects: vec![Effect::SetLocalAt {
                            reg: 0,
                            value: Expr::OperandImm(1),
                        }],
                        hints: pure,
                    }, // 0
                    alu("add", ArithOp::Add), // 1
                    alu("sub", ArithOp::Sub), // 2
                    alu("lt", ArithOp::Lt),   // 3
                    OpDef {
                        name: "copy",
                        effects: vec![Effect::SetLocalAt {
                            reg: 0,
                            value: Expr::LocalAt(1),
                        }],
                        hints: pure,
                    }, // 4
                    OpDef {
                        name: "call",
                        effects: vec![Effect::Call {
                            dest: 0,
                            base: 1,
                            argc: 2,
                        }],
                        hints: OpHints {
                            pure: false,
                            terminator: false,
                        },
                    }, // 5
                    OpDef {
                        name: "halt",
                        effects: vec![Effect::Halt {
                            value: Expr::LocalAt(0),
                        }],
                        hints: term,
                    }, // 6
                    OpDef {
                        name: "ret",
                        effects: vec![Effect::Return {
                            value: Expr::LocalAt(0),
                        }],
                        hints: term,
                    }, // 7
                ],
            }
        }
        // silence "never constructed with this variant read" style unused-const lints for opcodes this
        // particular base module doesn't use directly (blind mutation may still swap an opcode field TO
        // one of these — they exist in the ISA on purpose, see the module doc above).
        let _ = (MOV_IMM, SUB, LT);

        fn base_module() -> Module {
            let callee = FuncDef {
                nparams: 1,
                consts: vec![1],
                nlocals: 3,
                code: vec![Inst::new(ADD, 2, 0, 1), Inst::new(RET, 2, 0, 0)],
            };
            let entry = FuncDef {
                nparams: 1,
                consts: vec![1], // func index 1 = callee
                nlocals: 4,
                code: vec![
                    Inst::new(COPY, 2, 0, 0),
                    Inst::new(CALL, 3, 1, 1),
                    Inst::new(HALT, 3, 0, 0),
                ],
            };
            Module {
                funcs: vec![entry, callee],
                fill: 0,
                host_ctx: 0,
                entry: 0,
            }
        }

        let isa = call_isa();
        // Sanity: the UNMUTATED baseline module must itself validate and (elsewhere, implicitly, via the
        // loop below reusing it as mutation base) agree across tiers.
        assert_eq!(check_module(&isa, &base_module()), Ok(()));

        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };

        fn adversarial_i32(next: &mut dyn FnMut() -> u32, nlocals_hint: i32) -> i32 {
            match next() % 8 {
                0 => -1,
                1 => i32::MIN,
                2 => i32::MAX,
                3 => nlocals_hint,
                4 => nlocals_hint.saturating_add(1),
                5 => nlocals_hint.saturating_mul(1000),
                6 => 0,
                _ => (next() % 32) as i32 - 8,
            }
        }

        let mut checked_ok = 0usize;
        let mut checked_err = 0usize;

        for _ in 0..800 {
            let mut m = base_module();
            let nmuts = 1 + (next() % 4) as usize;
            for _ in 0..nmuts {
                let fidx = (next() as usize) % m.funcs.len();
                let f = &mut m.funcs[fidx];
                if f.code.is_empty() {
                    continue;
                }
                let iidx = (next() as usize) % f.code.len();
                let nlocals_hint = f.nlocals as i32;
                if next() % 4 == 0 {
                    // Occasionally in-range, mostly not — but NEVER synthesize a NEW `Call` site this
                    // way. `check_module`/`check_func` deliberately do NOT (and structurally cannot)
                    // validate a `Call`'s `callee` VALUE (a runtime local read, not a static property —
                    // see `check_func`'s own module doc, "What this does NOT prove"); `ModuleEngine`'s
                    // `in_range` check is what handles that layer, and this test intentionally bypasses
                    // `ModuleEngine` to exercise the raw oracle/raw Table-mode-compiled tiers directly. A
                    // freshly-synthesized `Call` whose `base`/`argc` operands are whatever the ORIGINAL
                    // (non-call) instruction happened to carry is unconstrained by anything `check_module`
                    // checks and can trivially resolve, at runtime, to a self-referential (infinite
                    // recursion / native stack overflow — NOT a panic `catch_unwind` can contain) or
                    // otherwise wild callee index. That is a real, but ALREADY-DOCUMENTED, out-of-scope
                    // limitation of `check_func` (not an O-12 regression) — this fuzzer must not blunder
                    // into exercising it un-intentionally, since doing so tests something this module
                    // never promised, not a genuine tier-agreement regression.
                    let mut candidate = (next() % 4096) as u16;
                    if candidate == CALL {
                        candidate = 4096; // still exercises OpcodeOutOfRange, never becomes a new Call
                    }
                    f.code[iidx].op = candidate;
                } else if f.code[iidx].op == CALL {
                    // The ONE pre-existing `Call` site (see `base_module`): only ever corrupt `dest`
                    // (slot 0). `base`/`argc` (slots 1/2) stay exactly as `check_func`'s own unit tests
                    // already exercise for `CallSlotOutOfRange` (`call_slot_out_of_range_argc_too_big_
                    // rejected`/`call_slot_negative_argc_rejected`/`tailcall_slot_out_of_range_rejected`)
                    // — this differential test's OWN job is cross-tier agreement on what `check_module`
                    // approves, not re-proving that unit-level result, so it can afford to keep this one
                    // call site's callee resolution fixed at its well-formed, non-recursive value (`consts
                    // = [1]` = the callee function's index — `entry`'s `consts` is never mutated, and
                    // `f.consts` is out of this loop's reach entirely) for the same reason above.
                    let cur = f.code[iidx].operands[0];
                    f.code[iidx].operands[0] = if next() % 3 == 0 {
                        adversarial_i32(&mut next, nlocals_hint)
                    } else {
                        cur.wrapping_add((next() % 7) as i32 - 3)
                    };
                } else {
                    let slot = (next() % 3) as usize;
                    let cur = f.code[iidx].operands[slot];
                    let mut new_val = if next() % 3 == 0 {
                        adversarial_i32(&mut next, nlocals_hint)
                    } else {
                        cur.wrapping_add((next() % 7) as i32 - 3) // small in-range-ish jitter
                    };
                    // `entry`'s local[1] holds the ONE `Call`'s callee-index CONST (see `base_module`;
                    // `f.consts` itself is never mutated by this fuzzer). Every writing op in this ISA
                    // uses operand slot 0 as its destination register (mov_imm/add/sub/lt/copy all have
                    // `reg: 0` — see `call_isa`), so a slot-0 mutation on `entry`'s code (fidx == 0) is
                    // capable of retargeting SOME OTHER instruction's write to ALSO land on local[1],
                    // clobbering the const AFTER frame-init but BEFORE the (unmutated, protected) `Call`
                    // reads it — an indirect route to the exact same runtime-self-recursion hazard the
                    // dedicated `CALL` branch above guards against directly, discovered empirically (this
                    // comment exists because a first version of this test hung on it). Steer strictly away
                    // from ever placing a destination write on exactly local[1] to close that indirect
                    // route too, while leaving every other destination (dest corruption is exactly what
                    // exercises `LocalOutOfRange`) and `callee`'s own locals (which hold no such reserved
                    // slot — it never `Call`s anything) fully fuzzable.
                    if fidx == 0 && slot == 0 && new_val == 1 {
                        new_val = 2;
                    }
                    f.code[iidx].operands[slot] = new_val;
                }
            }

            if check_module(&isa, &m).is_err() {
                checked_err += 1;
                continue; // expected/common: correctly rejected a malformed mutation
            }
            checked_ok += 1;

            for n in [0i64, 1, 5, -3] {
                let oracle = otter::interp::run_module(&isa, &m, &[n]);
                let base = otter_baseline_compile_module_result(&isa, &m, n);
                let jit = otter_cranelift::compile_module(&isa, &m).run(&[n]);
                assert_eq!(
                    base, oracle,
                    "baseline disagreed with the interp oracle on a check_module-approved mutant\n\
                     n={n}\nmodule={m:?}"
                );
                assert_eq!(
                    jit, oracle,
                    "cranelift disagreed with the interp oracle on a check_module-approved mutant\n\
                     n={n}\nmodule={m:?}"
                );
            }
        }

        // Sanity on the fuzzer itself: both outcomes must actually occur, or this test is silently
        // vacuous (either corruption never triggers a rejection, or check_module never lets anything
        // through — either way the differential assertions above would never run for real).
        assert!(
            checked_ok > 0,
            "check_module never approved a mutant in 800 tries — this test exercised nothing"
        );
        assert!(
            checked_err > 0,
            "check_module never rejected a mutant in 800 tries — the corruption pool is too weak"
        );
    }

    /// `compile_module` (Table mode) as a plain fn so the fuzz test above reads as a flat list of "run
    /// this tier" calls, matching `run_module`/`compile_module` (cranelift)'s own call shape.
    fn otter_baseline_compile_module_result(isa: &Isa, m: &Module, n: Value) -> Value {
        compile_module(isa, m).run(&[n])
    }
}
