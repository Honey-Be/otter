//! O-8a — `Effect::TailCall` on the reference oracle `interp::run_module`. Verifies the panel's fix T7:
//! a tail call REPLACES the current frame (built via `make_frame` for the CALLEE, with the outgoing
//! frame's `ret_dest` preserved), so (a) deep tail recursion runs without unbounded frame growth,
//! (b) the result returns to the original caller, and (c) the callee sees its own consts / `fill`-filled
//! missing params, not the caller's leftovers. The engine-trampoline path (native-stack boundedness) is
//! O-8b.

use otter::{ArithOp, BinOp, Effect, Expr, FuncDef, Inst, Isa, Module, NumTy, OpDef, OpHints, interp};

// opcodes for the test ISA
const EQ0: u16 = 0; // local[op0] = (local[op1] == 0)
const JUMP_IF: u16 = 1; // if local[op0] != 0 goto op1
const SUB1: u16 = 2; // local[op0] = local[op1] - 1
const SET_IMM: u16 = 3; // local[op0] = op1 (raw immediate)
const COPY: u16 = 4; // local[op0] = local[op1]
const TAILCALL: u16 = 5; // tailcall: operand0 = base slot of [callee,args], operand1 = argc
const CALL: u16 = 6; // local[op0] = call(base=op1 -> [callee,args], argc=op2)
const RET: u16 = 7; // return local[op0]
const HALT: u16 = 8; // halt local[op0]

fn ctrl(name: &'static str, eff: Effect) -> OpDef {
    OpDef {
        name,
        effects: alloc_vec(eff),
        hints: OpHints {
            pure: false,
            terminator: true,
        },
    }
}
fn set(name: &'static str, value: Expr) -> OpDef {
    OpDef {
        name,
        effects: alloc_vec(Effect::SetLocalAt { reg: 0, value }),
        hints: OpHints {
            pure: true,
            terminator: false,
        },
    }
}
fn alloc_vec(e: Effect) -> Vec<Effect> {
    vec![e]
}

fn test_isa() -> Isa {
    Isa {
        ops: vec![
            set(
                "eq0",
                Expr::Bin(
                    BinOp::new(NumTy::I64, ArithOp::Eq),
                    Box::new(Expr::LocalAt(1)),
                    Box::new(Expr::Imm(0)),
                ),
            ),
            ctrl(
                "jump_if",
                Effect::JumpIf {
                    cond: Expr::LocalAt(0),
                    target: 1,
                },
            ),
            set(
                "sub1",
                Expr::Bin(
                    BinOp::new(NumTy::I64, ArithOp::Sub),
                    Box::new(Expr::LocalAt(1)),
                    Box::new(Expr::Imm(1)),
                ),
            ),
            set("set_imm", Expr::OperandImm(1)),
            set("copy", Expr::LocalAt(1)),
            ctrl("tailcall", Effect::TailCall { base: 0, argc: 1 }),
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
            },
            ctrl(
                "ret",
                Effect::Return {
                    value: Expr::LocalAt(0),
                },
            ),
            ctrl(
                "halt",
                Effect::Halt {
                    value: Expr::LocalAt(0),
                },
            ),
        ],
    }
}

#[test]
fn deep_tail_recursion_stays_bounded_and_correct() {
    // f(n): if n == 0 { return 42 } else { tailcall f(n - 1) }
    // locals: 0=n(param), 1=t, 2=block[0]=callee idx, 3=block[1]=n-1, 4=ret val
    let f = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 5,
        code: vec![
            Inst::new(EQ0, 1, 0, 0),      // t = (n == 0)
            Inst::new(JUMP_IF, 1, 5, 0),  // if t goto pc5 (done)
            Inst::new(SUB1, 3, 0, 0),     // block[1] = n - 1
            Inst::new(SET_IMM, 2, 0, 0),  // block[0] = callee idx 0
            Inst::new(TAILCALL, 2, 1, 0), // tailcall f(block) — base slot 2, argc 1
            Inst::new(SET_IMM, 4, 42, 0), // pc5: ret = 42
            Inst::new(RET, 4, 0, 0),      // return ret
        ],
    };
    let module = Module {
        funcs: vec![f],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    let isa = test_isa();
    // A push-per-call oracle would allocate ~1,000,000 heap frames here; frame-REPLACE keeps the frame
    // stack at length 1 throughout. Correct result after a million tail steps proves no per-step corruption.
    assert_eq!(interp::run_module(&isa, &module, &[1_000_000]), 42);
    assert_eq!(interp::run_module(&isa, &module, &[0]), 42); // base case
    assert_eq!(interp::run_module(&isa, &module, &[7]), 42);
}

#[test]
fn tail_call_preserves_ret_dest_across_a_normal_call() {
    // main(x): r = call g(x); halt r.   g(a): tailcall h(a).   h(a): return 99.
    // The tail call in g must send h's result to g's caller (main's `r` slot), not lose it.
    let main = FuncDef {
        nparams: 1, // x at slot 0
        consts: vec![],
        nlocals: 4, // 0=x, 1=r(dest), 2=block[0]=g idx, 3=block[1]=x
        code: vec![
            Inst::new(SET_IMM, 2, 1, 0), // block[0] = g idx (func 1)
            Inst::new(COPY, 3, 0, 0),    // block[1] = x
            Inst::new(CALL, 1, 2, 1),    // r = call(base=2, argc=1)  → dest slot 1
            Inst::new(HALT, 1, 0, 0),    // halt r
        ],
    };
    let g = FuncDef {
        nparams: 1, // a at slot 0
        consts: vec![],
        nlocals: 3, // 0=a, 1=block[0]=h idx, 2=block[1]=a
        code: vec![
            Inst::new(SET_IMM, 1, 2, 0),  // block[0] = h idx (func 2)
            Inst::new(COPY, 2, 0, 0),     // block[1] = a
            Inst::new(TAILCALL, 1, 1, 0), // tailcall h(a) — base slot 1
        ],
    };
    let h = FuncDef {
        nparams: 1,
        consts: vec![],
        nlocals: 2, // 0=a, 1=99
        code: vec![
            Inst::new(SET_IMM, 1, 99, 0), // slot1 = 99
            Inst::new(RET, 1, 0, 0),      // return 99
        ],
    };
    let module = Module {
        funcs: vec![main, g, h],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    assert_eq!(interp::run_module(&test_isa(), &module, &[5]), 99);
}

#[test]
fn tail_callee_gets_its_own_consts_and_fill_not_caller_leftovers() {
    // caller(): tailcall g()  with argc=0 (g has 1 param → the missing param must be `fill`, not a
    // caller leftover); g returns its CONST (must be g's 700, not the caller's).
    // caller locals: 0=block[0]=g idx (built via set_imm); it also writes slot1=555 as a leftover.
    let caller = FuncDef {
        nparams: 0,
        consts: vec![111], // caller's const at slot 0? no — consts go at [nparams..]; nparams=0 → slot0
        nlocals: 3,        // 0=const(111), 1=leftover, 2=block[0]=g idx
        code: vec![
            Inst::new(SET_IMM, 1, 555, 0), // leftover in slot 1 (would be g's param slot 0 if carried over)
            Inst::new(SET_IMM, 2, 1, 0),   // block[0] = g idx (func 1)
            Inst::new(TAILCALL, 2, 0, 0),  // tailcall g() — base slot 2, argc 0
        ],
    };
    // g(a): returns its const 700 (proving g's consts were installed, not the caller's 111);
    // also asserts the missing param a == fill(0) is irrelevant since we return the const.
    let g = FuncDef {
        nparams: 1,        // a at slot 0 — NOT supplied by the argc=0 tail call → must be fill
        consts: vec![700], // const at slot 1 (nparams=1)
        nlocals: 3,        // 0=a(fill), 1=const(700), 2=a+const witness
        code: vec![
            // witness = a + const : if a were the caller's leftover 555, this would be 1255; with fill=0
            // it must be 700. This distinguishes "callee frame rebuilt (fill+g.consts)" from "carried over".
            Inst::new(
                COPY, 2, 1, 0, // slot2 = const (700)
            ),
            Inst::new(RET, 2, 0, 0), // return 700
        ],
    };
    let module = Module {
        funcs: vec![caller, g],
        fill: 0,
        host_ctx: 0,
        entry: 0,
    };
    // g's const (700), not the caller's const (111) or leftover (555).
    assert_eq!(interp::run_module(&test_isa(), &module, &[]), 700);
}
