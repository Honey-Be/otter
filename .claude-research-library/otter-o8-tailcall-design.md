# O-8 설계 — `Effect::TailCall` (R7RS proper tail call)

Status: DESIGN (pre-implementation, verify-before-implement — otter 코어 변경: Effect+interp+3백엔드+엔진
디스패처). 요청: tsavorite §1(blocker). 목표: **꼬리 위치 호출이 호출 스택(네이티브·인터프리터 둘 다)을 소비하지
않는다** → 무한 꼬리재귀가 유계 스택으로 실행(R7RS §3.5 요구).

## 1. 추가할 것

```rust
/// 현재 프레임을 교체하는 호출. 피연산자 규약은 `Call`과 동일: `base` 슬롯에 `[callee, arg0..arg(argc-1)]`.
/// `dest` 없음 — 결과가 현재 함수의 반환값이 된다. terminator.
TailCall { base: u8, argc: u8 },
```
`OpHints{terminator:true}`. 프론트엔드(예: tsavorite, 또는 neosurf WASM 꼬리호출)가 꼬리 위치 `Call`을
`TailCall`로 방출.

## 2. 핵심 난점

ModuleEngine은 **모든 호출을 `module_dispatch`(Rust fn) 경유**로 라우팅한다(`otter-engine/src/lib.rs`
`dispatch`/`module_dispatch`). 그래서 순진한 "꼬리호출=디스패처 재귀"는 **디스패치마다 네이티브 프레임 1개**를
쌓아 유계성이 깨진다. cranelift `return_call`조차 디스패처 프레임을 재사용할 뿐 콜리는 디스패처가 다시 호출하므로
유계가 안 됨. **디스패처 간접이 네이티브 꼬리호출을 무력화**한다 — 이게 설계의 중심.

## 3. 채택 설계 (A): 디스패처 트램폴린 (전 티어 통일)

꼬리호출을 "값 반환" 대신 **"다음에 실행할 (callee, args) 요청"** 신호로 바꾸고, 디스패처가 루프로 되풀이한다.
현재 함수는 꼬리호출 시 **정상적으로 리턴**(프레임 해체)해 트램폴린 루프까지 올라오고, 루프가 콜리를 **새 프레임
없이 재진입**한다 → 동시에 네이티브 프레임 = 디스패처 1 + 함수 1로 유계.

- **신호 채널 (TLS, 트랩 채널과 동형)** [A1, 채택 후보]: thread-local `PENDING_TAIL: Option<(callee: usize,
  args: Vec<Value>)>`. 꼬리호출 op은 args를 **안정 버퍼(TLS의 Vec)로 복사** 후 sentinel(`fill`) 반환. 트램폴린이
  각 호출 후 TLS를 확인: Some면 pop해서 재진입, None이면 정상 반환값. **ABI 무변경**(CompiledFn 시그니처 유지) —
  현행 트랩 poison TLS와 같은 패턴이라 세 티어 통일 용이. args를 caller 프레임(곧 소멸)에서 TLS Vec로 복사하는
  시점이 관건(프레임 해체 전, 콜리 읽기 전).
  - 대안 A2: CompiledFn 반환형을 태그 유니온으로. ABI 변경이 세 티어에 파급 → 기각(A1이 덜 침습적).

- **`module_dispatch` / `dispatch` 트램폴린 루프**:
  ```
  loop {
    result = 현재-best-티어로 idx 실행 (또는 interp run_one)
    if let Some((next, args)) = PENDING_TAIL.take() { idx = next; args = args; count/tier-up; continue }
    else { return result }
  }
  ```
  꼬리호출은 가장 가까운 디스패처 루프로 unwind해 재진입. 비꼬리 중첩 호출은 각자 디스패처 프레임+자기 루프를
  가진다. tier-up 카운팅은 트램폴린 재진입마다 수행(꼬리재귀 루프도 핫해지면 T2 승격).

- **순수 오라클 `interp::run_module`**: 자체 프레임 Vec 소유 → 꼬리호출 시 **현 프레임 in-place 교체**(locals를
  `[callee,args]`+consts로 재초기화, pc=0). 트램폴린 불요, 진짜 유계. (엔진과 메커니즘은 다르나 결과·유계성 동일 —
  차등 테스트는 결과 동등만 요구.)

## 4. 티어별

- **T0 인터프리터**: run_module=프레임 in-place 교체. run_one(엔진용, 단일 프레임+closure 위임)=TailCall 시
  PENDING_TAIL 세팅 + fill 반환 → 디스패처 트램폴린이 처리.
- **T1 baseline (x86_64)**: tsavorite 수용안대로 **proper tail call 미구현**. baseline `compile_dispatch_fn`이
  TailCall op을 만나면 **컴파일 거부**(→ 엔진이 그 함수를 T1 건너뛰고 T0 유지, 이후 T2). 절대 "Call+Return으로
  하강"하지 않음(스택 성장 → R7RS 위반). 엔진 `maybe_tier_up`: baseline 거부 시 T1 스킵.
- **T2 cranelift**: 첫 컷은 트램폴린 신호(PENDING_TAIL 세팅 후 리턴) — T0-엔진과 통일. 네이티브 `return_call`은
  후속 perf(디스패처 우회 필요, 별도 증분). 꼬리재귀 루프는 디스패처 왕복/iteration 비용(네이티브 return_call보다
  느리나 tsavorite의 자체 트램폴린보다 나음 — otter 밖으로 안 나감).

## 5. 검증

- **결정적 게이트**: 깊은 꼬리재귀(예: `countdown(10_000_000)` 꼬리재귀)가 **스택오버플로 없이** 완주 →
  유계성 실증(틀리면 오버플로로 실패). interp(run_module) + 엔진(트램폴린) + (T2 승격 후) 모두.
- **크로스-티어 차등**: 동일 모듈을 run_module/baseline(거부 시 스킵)/cranelift/ModuleEngine에서 결과 비트동일.
- **혼합**: 꼬리호출 + 비꼬리 중첩 호출 섞인 트리(트램폴린이 올바른 루프로 unwind).
- **T1 스킵 확인**: TailCall 포함 함수가 T1로 승격 안 되고 T0→T2로 가는지(tier_of).
- 게이트: 워크스페이스 그린 + clippy -D + fmt. (엔진 unsafe/FFI 인접 → 구현 후 적대적 리뷰.)

## 6. 위험 (패널 집중)

1. **유계성 실증 위치**: 트램폴린이 정말 프레임을 해체 후 재진입하나? args 복사 시점(프레임 소멸 전, 콜리 읽기 전).
   PENDING_TAIL Vec 재사용/할당이 꼬리재귀 루프의 hot path에 매 iteration 할당을 넣지 않는지(성능).
2. **중첩 디스패처 루프 vs 꼬리 unwind**: 꼬리호출이 "가장 가까운" 트램폴린으로만 unwind하는가? 비꼬리 호출이
   중간에 있으면 그 프레임은 유지돼야(정상). run_one이 fill 반환으로 트램폴린에 신호하는데, 그 fill이 **정상 fill
   반환값**과 구별되나? (트랩 poison처럼 값이 아니라 TLS 플래그로 구별 — 필수.)
3. **트랩 채널과의 상호작용**: TailCall 도중 트랩(poison) 시 PENDING_TAIL 정리? 트램폴린이 trapped 확인 우선.
4. **run_module 프레임 교체 정확성**: locals 레이아웃 `[params][consts][temps]` 재초기화, consts 보존, argc≠nparams
   처리, fill 초기화.
5. **baseline 거부 경로**: compile_dispatch_fn이 TailCall 발견 시 깔끔히 거부하고 엔진이 폴백하는가? 부분 컴파일
   상태가 남지 않는지.
6. **T1 미구현이 정확성에 영향 없음**: 같은 프로그램이 T0/T2에선 유계, T1은 스킵 → 항상 유계 보장(T1로는 안 감).
7. **재진입(otter→호스트→otter) 시 PENDING_TAIL 오염**: 스레드로컬이라 스레드별 분리되나, 재진입 시 저장·복원 필요?
   (트랩 채널의 run 격리와 동일 이슈.)

## 6b. 패널 픽스 (must-fix, 구현 전 반영)

패널1(트램폴린 유계성) — 유계성 자체는 세 티어+오라클 모두 성립 확인. 아래는 반드시 반영:

- **[픽스 T1 — 하드 불변식, 세 티어] `TailCall`은 언제나 "PENDING 세팅 + 리턴"이지 `dispatch(callee); return`이
  절대 아님.** terminator라 run_one 루프가 즉시 return → run_one 프레임 해체 후 트램폴린 재진입(유계). 만약 Call처럼
  `dispatch` 재귀로 하강하면 차등 테스트는 통과하나 깊은재귀에서 오버플로. baseline뿐 아니라 run_one·cranelift에도
  명시.
- **[픽스 T2 — 안전, PENDING clear on trap] `fill`이 정상/트랩poison/꼬리sentinel 3의미.** 구별은 값이 아니라
  플래그. 트램폴린 검사 우선순위 = **`trapped` → PENDING → 값**. **`module_dispatch`의 `catch_unwind` 에러 암과
  poison 조기반환(`dispatch` 초입 `trapped||oob`)에서 PENDING을 반드시 폐기(take & drop).** 안 하면 PENDING 세팅
  직후 패닉 시 스테일 Some이 다음 top-level run에서 죽은 콜리를 스테일 args로 재진입(정확성/안전 결함).
- **[픽스 T4 — perf, 버퍼 재사용] PENDING을 `Option<(usize,Vec<Value>)>`(호출마다 alloc)이 아니라 재사용 TLS
  스크래치로**: `PT_PENDING: Cell<bool>`, `PT_CALLEE: Cell<usize>`, `PT_ARGS: RefCell<Vec<Value>>`. 꼬리호출 핸들러=
  `buf.clear(); buf.extend_from_slice(&locals[base+1..base+1+argc])`(clear는 capacity 유지 → 워밍업 후 무재할당) +
  callee/flag 세팅. T2 무할당 꼬리루프에 힙 alloc 주입 금지(요점). 트랩 채널의 Cell/RefCell 패턴과 동형.
- **[픽스 T3 — run_module aliasing] in-place 프레임 교체 전 args를 임시 `Vec`로 복사** 후 재초기화(현행 `Call`
  경로가 이미 `let mut argv = ...; push(locals[base+1+k])` 하는 것과 동일). 소스 arg 슬롯이 목적 param 슬롯과 겹쳐
  덮어쓰기되는 것 방지.
- **[픽스 T5 — baseline 거부 캐시] `compile_dispatch_fn`을 fallible로** + 거부 시 `baseline[idx]=None`이면 매 호출
  재시도(t1..t2 구간 O(n) 낭비) → 거부를 **캐시**(per-func `baseline_refused: Cell<bool>` 또는 `Some(Refused)` 상태)해
  1회만 시도. mmap/코드버퍼 할당 전에 TailCall 스캔으로 거부(버퍼 누수 방지).
- **[픽스 T6 — 검증 티어별 격리] 깊은재귀 게이트는 결과동등만 보는 차등으로는 엔진 트램폴린 유계버그를 못 잡음**
  (결과 동일). **티어별 분리 실행**: T0전용(t1/t2를 10M 위로 세팅해 미승격) + T2(t2 낮게 조기승격), 각각 `tier_of`로
  의도 티어 도달 확인 후 오버플로 없이 완주 assert.

패널2(run_module·크로스티어) + 패널3(baseline·트랩·재진입) — 추가 must-fix:

- **[픽스 T7 — 핵심 정확성, run_module] in-place 교체 = `make_frame(callee, snapshot(args)) + ret_dest 보존`.**
  순진한 재작성은 오답: (1a) `argc<nparams`면 누락 param을 `fill`로 안 채움, (1c) callee consts를 caller 것으로 안
  바꿈, (1d) temps 크기/`fill` 미초기화(패닉 또는 스테일), (2) `ret_dest`를 0으로 리셋(반환 대상 오류). **올바른 구현
  = 기존 `Call` 핸들러에서 push만 뺀 것**: callee+args를 임시 `Vec`로 스냅샷(현 `Call`이 이미 하는
  `argv=...; push(locals[base+1+k])`) → `make_frame(module, callee, argv)` → `frame.ret_dest = 나가는
  stack[top].ret_dest`(보존!) → `stack[top] = frame`(push 아님). §6.4 4항목을 "위험"에서 "테스트된 하드 불변식"으로
  승격. (aliasing 1b는 param 최하단+forward copy면 안전 — decoy.)
- **[픽스 T8 — baseline 거부 캐시, sticky] `None`은 untried/refused를 혼동** → 거부된(꼬리호출 포함) 함수가 매
  dispatch마다(t1..t2 구간, 그리고 **T2 설치 후에도**) baseline 재스캔(핫패스 O(ops) 낭비). tri-state
  `enum Tier { Untried, Refused, Compiled(_) }` 또는 per-func `baseline_refused: Cell<bool>`를 guard에 추가.
- **[픽스 T9 — 거부는 값, 패닉 금지] baseline 거부는 `unimplemented!()`(패닉) 금지** — top-level entry tier-up은
  `catch_unwind` 밖(`run`)이라 run 밖으로 unwind, nested는 catch되어 **유효 프로그램이 엔진 poison**. `compile_dispatch_fn`을
  fallible(Result/Option)로, **pre-scan(maxargc 순회)에서 TailCall 발견 시 mmap/버퍼 할당 전 거부**. AOT
  `compile_module`/`BaselineBackend`(공개 API, ExecBuf 반환)는 깨끗한 거부 경로가 없음 → **baseline AOT는 꼬리호출
  모듈 미지원**(엔진 경로만; 테스트가 꼬리호출 모듈을 baseline AOT로 라우팅 금지). cranelift AOT는 signal 하강 가능.
- **[픽스 T10 — 트랩/재진입 run 격리, RAII 가드] `PENDING_TAIL`(스레드-TLS)이 엔진별 `trapped`보다 오래 삶** →
  트랩 unwind가 트램폴린 drain을 건너뛰면 스테일 `Some`이 **다음 top-level run/다른 엔진으로 누수**(스퓨리어스
  꼬리호출 = 오답). heapbridge `CtxGuard`/`PENDING` clear-at-`run_with`-entry 패턴 복사: (i) `run`/dispatch 트램폴린
  **진입 시 save+clear, drop(=unwind 포함) 시 restore**하는 RAII 가드, (ii) `module_dispatch`의 `catch_unwind` 에러
  암 + poison 조기반환에서 `PENDING` clear, (iii) 트램폴린 루프가 **매 iteration `trapped` 먼저 확인** 후 PENDING
  소비. 버퍼 재사용(T4)과 save/restore는 버퍼 스냅샷으로 양립(가드가 flag+callee+args 스냅샷).
- **[픽스 T11 — exhaustive match] `Effect::TailCall` 추가 시 ~9개 `match eff` 사이트가 non-exhaustive**
  (otter interp 2 fn, baseline 2, cranelift 3 등). 전부 arm 추가: interp(run_module=프레임교체, run_one=signal),
  baseline(refuse), cranelift(signal 하강).

**패널 종합 판정**: 트램폴린 설계는 세 티어+오라클 모두 **유계 확인**. 정확성은 T7(make_frame 등가+ret_dest) +
T2(flag 판별) + 불변식(T1: signal-return, baseline reject) + T10(run 격리) 하에 크로스티어 비트동일. 거절 항목 없음.

## 7. 증분

- **O-8a** [DONE `24daa5d`]: Effect::TailCall + run_module 프레임교체 + 깊은-재귀 테스트(오라클만). 코어 최소.
- **O-8b** [DONE `67522e8`]: 엔진 트램폴린 + T0 유계성 증명 + `interp_only` pre-scan(꼬리호출 함수는 T1/T2
  영구 미승격). **구현이 설계 초안에서 개선**: PENDING_TAIL을 스레드-TLS가 아니라 `run_one`의 **로컬 out-param**
  (`TailReq`)으로 구현 — 패널 픽스 T10(run-격리 RAII 가드)이 통째로 불필요해짐(전역/스레드 상태 자체가 없음).
- **O-8c** [REQUIRED — 후속 아님, 정책 변경]: §8 참조. [[otter-tier-parity-policy]] 정책(2026-09-02, otter
  전용: 티어 간 기능 패리티 갭은 stable 릴리즈에서 절대 불허, 성능 격차만 허용)에 의해 "T1/T2는 꼬리호출
  영구 미지원"은 **stable 릴리즈 차단 사유로 재분류**. O-8b의 "T0에서만 정확+유계, 미가속"은 개발 중 임시
  스테이징으로는 허용되나 최종 상태로는 불허.

## 8. O-8c — 컴파일 티어(T1/T2) 꼬리호출 완전 지원 (정책 필수)

Status: DESIGN (구현 전 — 적대적 패널 필수. baseline은 hand-rolled x86-64, cranelift는 unsafe 코드 생성 =
위험 서브시스템). 목표: T1(baseline)·T2(cranelift)가 `Effect::TailCall`을 **정확히·유계하게** 하강해, 꼬리호출을
포함한 함수도 정상 승격(tier-up)되고 세 티어(T0/T1/T2) 전부 기능 동등(성능만 차이).

### 8.1 핵심 메커니즘 — "baked helper + 즉시 반환" (ABI 무변경)

O-8b의 `run_one` out-param 패턴을 컴파일 코드로 확장하되, **컴파일된 fn의 ABI(`CompiledFn`/`CompiledDispatchFn`)는
전혀 바꾸지 않는다.** 대신 `dispatch_addr()`(기존에 이미 `Call`이 `module_dispatch`를 호출하도록 굽는 것과 동일
패턴)처럼 **새 baked 헬퍼 함수 하나**를 추가한다:

```rust
// otter-engine
extern "C" fn tail_signal(eng: *const ModuleEngine, callee: i64, args: *const Value, argc: i64) -> Value {
    let engine = unsafe { &*eng };
    // argc<=0 (음수 포함) || args.is_null() ⇒ 빈 슬라이스 — module_dispatch와 동일한 방어 패턴(§8.8 픽스 C4).
    // `n = argc as usize`만으로 음수를 걸러내면 wrap으로 거대한 길이가 되어 OOB read.
    let slice: &[Value] = if argc <= 0 || args.is_null() {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(args, argc as usize) }
    };
    // [픽스 U1, §8.9] module_dispatch와 동일하게 catch_unwind로 감싼다 — 이것도 JIT 코드가 직접 호출하는
    // extern "C" 진입점이므로 "트랩이 프로세스를 abort시키지 않는다"는 보장이 여기도 적용돼야 한다.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut buf = engine.tail_args.borrow_mut();
        buf.clear();
        buf.extend_from_slice(slice);       // 컴파일 프레임이 살아있는 동안 안전 복사 (아직 return 전)
        drop(buf);
        engine.tail_callee.set(callee as usize);
        engine.tail_pending.set(true);      // 마지막 문장 — 패닉 시 세팅 안 됨(트램폴린은 정상값 분기로 감)
    }));
    if outcome.is_err() { engine.trapped.set(true); }
    engine.module.fill                      // 반환값은 버려짐 — 판별자는 값이 아니라 tail_pending 플래그
}
// tail_signal의 시그니처는 기존 DispatchPtr(otter-engine에 이미 정의됨)와 바이트 단위로 동일 — 새 타입 불요
// (§8.8 픽스 C5; 이전 초안의 `DispatchPtr2`는 미정의 오탈이었다).
fn tail_signal_addr() -> usize { tail_signal as DispatchPtr as usize }
```

`ModuleEngine`에 필드 3개 추가(스레드-로컬 아님, **엔진별** `Cell`/`RefCell` — O-8b가 이미 증명한 "쓰기 직후 즉시
읽기" 불변식으로 재진입 안전, §8.3 참조): `tail_pending: Cell<bool>`, `tail_callee: Cell<usize>`,
`tail_args: RefCell<Vec<Value>>`.

**baseline(T1) 코드생성**: `TailCall{base,argc}`는 **기존 `Call` 하강과 거의 동일한 코드를 재사용** — 이미 검증된
인자 마셜링(`argbuf0` 역순 채우기, `ctx` 스필 슬롯 재사용)을 그대로 쓰고, 호출 타깃만 `dispatch_addr()` 대신
`tail_signal_addr()`로, `emit_aligned_call_rax` 직후 곧장 `leave;ret`만 방출(`dest_slot` 저장 생략). **주의: 이건
Return/Halt의 코드를 문자 그대로 재사용하는 게 아니다** — Return/Halt는 `emit_expr(value)`로 rax를 다시 계산 후
`leave;ret`하지만, `TailCall`엔 평가할 `Expr`가 없다. 대신 `tail_signal` 호출 직후 rax에는 **이미 그 리턴값(=`fill`)이
들어있다**(`emit_aligned_call_rax`의 호출-후 정리 명령 `mov rsp,rbx; pop rbx`가 rax를 건드리지 않으므로) — 그 값을
그대로 흘려보내는 것뿐, 재계산도 재적재도 불필요(§8.8 픽스 C2). 또한 `argbuf0` 프레임 슬롯을 정하는 사전스캔
(`compile_module_func`의 `maxargc` 계산)은 현재 `Effect::Call`만 보므로 **`Effect::TailCall`의 `argc`도 포함하도록
확장 필수**(§8.8 픽스 C1 — 안 하면 tail-only 함수의 인자 마셜링이 미확보 프레임 영역을 덮어씀). 신규 어셈블리는
사실상 "함수 콜 하나 + 조기 리턴"뿐. 리스크가 낮아진 이유: 바이트 단위로 새로 쓰는 게 아니라 이미 3-백엔드
차등으로 검증된 Call 마셜링 경로를 그대로 탄다.

**cranelift(T2) 코드생성**: 동일 구조 — 기존 `CallMode::Dispatch` 인자 마셜링 재사용, `call_indirect`로
`tail_signal_addr()` 호출, 결과 무시, `b.ins().return_(&[fill])`(기존 Return 하강과 동일 코드). **필수:** `TailCall`
하강 arm은 `is_terminator()`(기본블록 리더 도출이 유일하게 참조하는 함수 — `OpHints.terminator`는 자기일관성
검사에만 쓰이고 실제 블록 분할엔 관여 안 함)의 매치 목록에도 등록해야 한다(§8.8 픽스 U2) — 안 하면 debug
빌드는 `debug_assert_eq!` 즉시 패닉, release 빌드는 인접 꼬리호출 두 개가 한 블록에 합쳐져 cranelift verifier가
거부(→`catch_unwind` 밖에서 패닉, §8.9 Lens B 참조).

### 8.2 엔진 dispatch 루프 통합

O-8b의 "먼저 1회 시도 후 별도 트램폴린 루프"를 **단일 루프**로 정리(컴파일/인터프리트 양쪽을 균일하게 순회):

```rust
fn dispatch(&self, mut idx: usize, args: &[Value]) -> Value {
    let mut owned: Vec<Value> = Vec::new();
    let mut cur: &[Value] = args;
    // [픽스 U4] TailReq는 루프 밖에서 1회만 만들고 매 iteration `tail.pending = false`로 리셋 + `mem::swap`
    // 으로 args 버퍼를 재사용 — O-8b의 "꼬리루프는 iteration당 무할당"을 컴파일 경로(§8.1 tail_args)뿐 아니라
    // 인터프리트 경로에서도 유지한다(루프 안에서 매번 `TailReq::default()`를 새로 만들면 이 불변식이 깨짐 —
    // 최초 설계 초안의 실수였고, 실제 구현은 처음부터 올바르게 hoist되어 있었다. §8.9 참조).
    let mut tail = otter::interp::TailReq::default();
    loop {
        if self.trapped.get() || idx >= self.module.funcs.len() { return self.module.fill; }
        self.counts[idx].set(self.counts[idx].get().saturating_add(1));
        self.maybe_tier_up(idx);
        if let Some(p) = self.best_code(idx) {
            let v = self.call_compiled(p, cur);
            if !self.tail_pending.get() { return v; }
            self.tail_pending.set(false);                  // 즉시 소비 — 재진입 불변식 유지(§8.3)
            idx = self.tail_callee.get();
            core::mem::swap(&mut owned, &mut self.tail_args.borrow_mut());
            cur = &owned;
            continue;
        }
        tail.pending = false;                               // O-8b 그대로: run_one 경로는 로컬 out-param 유지
        let v = otter::interp::run_one(&self.isa, &self.module.funcs[idx], cur, self.module.fill,
            &mut |callee, cargs| self.dispatch(callee, cargs), &mut tail);
        if !tail.pending { return v; }
        idx = tail.callee;
        core::mem::swap(&mut owned, &mut tail.args);
        cur = &owned;
    }
}
```

`run_one` 경로는 O-8b의 로컬 `TailReq`를 그대로 유지(엔진 필드 불요 — 이미 안전하고 간단함); 컴파일 경로만 엔진
필드(`tail_pending`/`tail_callee`/`tail_args`)를 쓴다. 두 메커니즘이 공존하지만 `dispatch()` 루프가 판별자만
통일해서 소비하므로 호출자 입장에선 투명.

### 8.3 재진입 안전성 논증 (패널 집중 대상)

`ModuleEngine`은 `RefCell` 필드를 이미 갖고 있어 **본질적으로 `!Sync`**(동일 인스턴스를 두 스레드가 동시에 갖는
것 자체가 Rust 타입 시스템상 불가 — unsafe `impl Sync`를 아무도 하지 않았음). 그래서 "엔진별(스레드-전역 아닌)
`Cell` 필드"는 **동시 접근이 아니라 재진입(reentrant, 같은 스레드) 접근만** 문제 삼으면 된다.

핵심 불변식: **`tail_pending`은 오직 "컴파일 함수가 `tail_signal`을 호출하고 즉시 리턴하는 순간"과 "그 직후
`dispatch()`가 `call_compiled` 반환 직후 읽는 순간" 사이에서만 `true`다.** 이 구간 사이에는 **다른 어떤
otter 실행도 끼어들 수 없다**(Rust `extern "C"` 호출이 동기적으로 리턴해야 다음 코드가 실행되므로). 그리고
`dispatch()`는 `tail_pending`을 읽자마자(다른 어떤 것도 하기 전에) `false`로 리셋한다 — 그래서 중첩된
`dispatch()` 호출(예: A가 정상 호출로 B를 부르고, B가 꼬리호출로 C를 부르는 경우) 각각이 "자신의" 신호만
소비하고, 자신이 리턴할 때는 항상 `tail_pending == false`를 보장한 채로 리턴한다 — 이게 A의 바깥 루프가 스테일
신호를 절대 못 보는 이유. (O-8b 패널 T10이 지적한 "TLS가 엔진별 trapped보다 오래 삶"이라는 위험은, 이 필드가
**애초에 스레드-로컬이 아니라 엔진 필드**라서 다른 엔진으로 새는 경로 자체가 없다 — 하지만 "이 엔진 안에서
트랩이 `tail_pending=true`와 `dispatch()`의 소비 사이에 끼어들 수 있는가"는 패널이 직접 검증해야 함.)

### 8.4 단계적 승격 자격 (interp_only → 세분화)

`interp_only: Box<[bool]>`를 **티어별** 자격으로 세분화(스테이징: T2 먼저 구현 후 그 함수들에 대해서만 T2 허용,
T1은 계속 차단 → T1 구현 후 완전 해제):

```rust
struct TierCaps { skip_t1: bool, skip_t2: bool } // 둘 다 false = 완전 승격 가능(최종 상태)
```
pre-scan 로직은 O-8b와 동일(함수 바디에 `TailCall` 존재 여부), 다만 결과를 `maybe_tier_up`의 T1/T2 분기 각각에서
독립적으로 참조. **AOT 단일함수 API(`otter_baseline::compile`, cranelift `compile_fn`)는 범위 밖**: 이들은 이미
Call/Return 자체를 지원 안 함("interpreter-only tier" — 콜-프리 함수 전용이라는 기존 카브아웃)이므로 TailCall도
같은 이유로 미지원 — 다른 제품(콜-프리 전용 API)이라 정책 대상이 아님.

**정정(§8.8 픽스 C3)**: `compile_module`(Table 모드, `otter_baseline`/`otter_cranelift` 둘 다)은 위 카브아웃에
포함되지 **않는다** — 이미 `Effect::Call`을 지원한다(재귀/전방참조용 전체-모듈 함수포인터 테이블; §6b가 이미
"baseline AOT는 꼬리호출 모듈 미지원"이라 쓴 것 자체가 Call은 이미 된다는 방증). `compile_module`이 정책 대상
밖인 진짜 이유는 "Call 미지원"이 아니라 **`ModuleEngine`과 무관한 별도의 정적 전체-프로그램 AOT 제품**이기
때문 — `TailCall`은 그 경로에서 여전히 `unimplemented!()`(패닉)로 남고, `interp_only`류 사전 가드가 없어
TailCall 포함 모듈을 넘기면 호출자 책임 하에 패닉한다(명시적 범위 밖 결정; 가드 부재는 알려진 갭으로 기록,
O-8c 완료 조건 아님). **`ModuleEngine` 경로(= `compile_dispatch_fn`)만 정책 적용 대상.**

### 8.5 자기-꼬리호출 최적화 (범위 밖, 후속 perf — O-8d)

`f`가 자기 자신을 꼬리호출하는 경우(`(let loop ...)` 관용구), 트램폴린 없이 **네이티브 백워드 점프**(파라미터
재적재 + 함수 엔트리로 jmp)로 완전 무오버헤드 컴파일 가능(디스패처 왕복 없음). 정확성·유계성은 §8.1의 일반
메커니즘만으로 이미 충족(정책 요구는 이걸로 만족) — 이건 순수 성능 최적화라 O-8d로 유보, 스코프에서 제외.

### 8.6 위험 (패널 집중)

1. **`tail_signal` 인자 포인터 수명**: `args`가 컴파일 함수 자신의 프레임(스택) 안 마셜링 버퍼를 가리킴 —
   헬퍼가 `extend_from_slice`로 복사를 끝내기 **전에** 컴파일 함수가 리턴(프레임 해체)하면 안 됨. 호출 규약상
   헬퍼 호출은 동기적(콜이 리턴해야 다음 코드 실행)이므로 구조적으로 안전해야 하나, baseline 어셈블리가 헬퍼
   호출 후 스택을 정리하는 순서(콜 命令 자체가 이미 프레임 안에서 실행되므로 안전) 재확인 필요.
2. **재진입 중 `tail_args` RefCell 이중 borrow**: `tail_signal`이 `borrow_mut()`하는 동안 같은 호출 체인
   안에서 재차 `tail_signal`이 불릴 수 있는가? (헬퍼 자체는 즉시 반환하는 leaf 함수라 없어야 하나, panic-unwind
   경로 확인.)
3. **트랩과 `tail_pending`의 순서**: 컴파일 함수 실행 중 패닉(`catch_unwind`)이 `tail_signal` 호출 직후~
   `dispatch()`의 읽기 직전 사이에 발생 가능한가? (동기 호출이라 이 구간엔 다른 코드가 없어야 정상이나, panic이
   `tail_signal` 헬퍼 **안에서** 나면? borrow_mut 실패 시 panic 가능 — 그 경우 `tail_pending`이 아직 false라
   안전한지 확인.)
4. **baseline 콜 규약 정확성**: 기존 Call 코드 재사용 시 `dest_slot` 저장을 생략하는 지점에서 레지스터/스택
   정리가 Return/Halt 에필로그와 정확히 맞물리는지(스택 포인터 불일치 등 x86-64 특유 위험).
5. **cranelift 콜 규약 정확성**: `call_indirect` 후 결과 무시 + `return_`가 Cranelift의 블록/터미네이터 규칙과
   충돌 없는지(같은 블록에서 콜 다음 바로 return 터미네이터).
6. **티어 세분화 회귀**: `TierCaps`가 T2만 구현된 중간 상태에서 T1 분기가 여전히 올바르게 차단되는지(T1으로
   잘못 승격되면 `unimplemented!()` 패닉).
7. **크로스티어 결정적 게이트**: 1M 깊이 꼬리재귀가 **T1 승격 상태**·**T2 승격 상태** 각각에서 네이티브 스택
   오버플로 없이 완주 + `run_module` 오라클과 결과 동일(패널 T6 원칙의 컴파일 티어 버전).

### 8.7 증분

- **O-8c-1 (T2 cranelift)**: tail_signal 헬퍼 + 엔진 필드 + dispatch 루프 통합 + cranelift TailCall 하강 +
  `TierCaps{skip_t2}` 세분화(T1은 계속 차단) + 결정적 테스트(T2 승격 확인 + 1M 깊이 무오버플로).
  cranelift가 IR API라 hand-asm보다 위험 낮음 — 먼저 구현해 메커니즘을 검증.
- **O-8c-2 (T1 baseline)**: 동일 메커니즘을 baseline hand-asm에 적용(O-8c-1에서 검증된 패턴 재사용) +
  **`compile_module_func`의 `maxargc` 사전스캔에 `Effect::TailCall` 포함(§8.8 픽스 C1 — blocking)** +
  `TierCaps` 완전 해제(T1도 승격 가능) + 결정적 테스트(T1 승격 확인 + 1M 깊이 무오버플로) + 전 티어 크로스
  차등(T0/T1/T2/run_module 전부 동일 결과, 각 티어에 실제로 도달했는지 tier_of로 확인).
- 완료 시 [[otter-tier-parity-policy]] 요구 충족 — O-8 전체가 stable-릴리즈 조건 만족.

### 8.8 패널 픽스

세 렌즈(A=재진입 안전성, B=cranelift T2 코드생성, C=baseline T1 코드생성+단계적 롤아웃) 공격 결과 — 메커니즘
자체(§8.1~8.3의 "baked helper + 즉시 반환" 트램폴린)는 **깨지지 않음**: `tail_pending`이 true인 구간(= `tail_signal`의
`set(true)` 이후부터 `dispatch()`가 읽는 순간까지)엔 순수 기계어 에필로그(leave;ret / cranelift `return_`)만
있고 Rust 코드가 전혀 끼지 않으므로, O-8b 패널 T10류("PENDING이 트랩보다 오래 삶")가 여기선 구조적으로 재발
불가(엔진 필드 스코프라 스레드/run 간 누수 경로 자체가 없고, 그 좁은 구간엔 패닉 표면도 없음) — **확인, 픽스
불요**. `tail_args` RefCell 이중 borrow(위험#2)도 같은 이유로 도달 불가 — **확인, 픽스 불요**. Lens B(cranelift):
`call_indirect` 시그니처가 `tail_signal`과 정확히 일치하고(픽스 C5 참조), 인자버퍼가 콜사이트마다 새
`stack_slot`이라(전역 사전스캔에 안 걸림) baseline류 프레임 크기 버그가 원천적으로 없음 — **확인, 픽스 불요**.
아래는 그래도 반드시 반영:

- **[픽스 C1 — 심각/blocking, baseline 프레임 크기] `compile_module_func`의 `maxargc` 사전스캔이 `Effect::Call`만
  매치하고 `Effect::TailCall`을 보지 않는다.** 일반 `Call` 없이 `TailCall`만 있는 함수(전형적 `(let loop ...)`
  꼬리재귀 관용구가 정확히 이 모양)는 `maxargc=0`이 되어 `argbuf0` 이후 슬롯이 프레임에 전혀 확보되지 않는다 —
  TailCall의 인자 마셜링이 미확보 영역(레드존/인접 슬롯)에 써서 스택을 손상시킨다. 사전스캔 매치에
  `Effect::TailCall { argc, .. }`도 추가해야 O-8c-2가 성립(§8.1 갱신 반영, O-8c-2 착수 전 필수).
- **[픽스 C2 — baseline 에필로그, rax 출처 명시] TailCall 하강은 Return/Halt의 코드를 문자 그대로 재사용하지
  않는다** — Return/Halt는 `emit_expr(value)`로 rax를 재계산 후 `leave;ret`하지만 `TailCall`엔 평가할 `Expr`가
  없다. `emit_aligned_call_rax`(tail_signal 호출) 직후 rax에는 이미 그 리턴값(`fill`)이 들어있음(호출-후 정리
  명령 `mov rsp,rbx; pop rbx`가 rax 비파괴)을 근거로 곧장 `leave;ret`만 방출 — §8.1 원문("기존 에필로그로 즉시
  낙하")이 이 전제를 명시하지 않아 "Return/Halt 코드 경로를 그대로 호출한다"는 오독 여지가 있었다(수정됨).
- **[픽스 C3 — §8.4 사실관계 정정] `compile_module`(Table 모드)은 카브아웃 대상이 아니다 — 이미 `Effect::Call`을
  지원한다**(재귀/전방참조용 전체-모듈 함수포인터 테이블; §6b가 이미 "baseline AOT는 꼬리호출 모듈 미지원"이라
  쓴 것 자체가 Call은 이미 동작한다는 방증 — §8.4 원문의 "call-free라 미지원"은 §6b와 모순). 카브아웃은 진짜
  콜-프리 단일함수 API(`otter_baseline::compile`, cranelift `compile_fn`)만 대상. `compile_module`이 정책 밖인
  이유는 "Call 미지원"이 아니라 `ModuleEngine`과 무관한 별도 정적 AOT 제품이기 때문 — `TailCall`은 거기서
  여전히 `unimplemented!()`(패닉)로 남고 `interp_only`류 가드가 없다(알려진 갭으로 기록, O-8c 완료 조건 아님).
- **[픽스 C4 — tail_signal 방어적 인자 검사] 슬라이스 구성을 `n==0`(post-cast) 대신 `argc<=0 || args.is_null()`로**
  — 음수 `argc`를 `as usize`로만 걸러내면 wrap으로 거대한 길이가 되어 `from_raw_parts`가 OOB를 읽는다.
  `module_dispatch`가 이미 쓰는 방어 패턴과 통일(실도달 가능성은 낮음 — argc는 컴파일러가 굽는 상수이나, 이
  코드베이스의 "malformed bytecode에도 fail-closed" 원칙과 일관되게).
- **[픽스 C5 — 오탈, 타입 재사용] `tail_signal as DispatchPtr2`의 `DispatchPtr2`는 미정의** — `tail_signal`
  시그니처가 기존 `DispatchPtr`(`otter-engine`에 이미 정의)와 바이트 단위로 동일하므로 그걸 재사용.

**패널 종합 판정**: 메커니즘(트램폴린 재진입 안전성, cranelift 콜 규약)은 유효 — 거절 항목 없음. C1이 유일한
blocking 결함(구현 전 §8.1 사전스캔 문구에 이미 반영); C2·C4·C5는 §8.1 본문/코드에 직접 반영 완료; C3은 §8.4를
정정. O-8c-1(T2)은 이 픽스들과 무관하게 그대로 진행 가능(C1은 O-8c-2 baseline에만 해당).

### 8.9 사후검토 — 워크플로 배관 버그 + 실제 3-패널 결과 회수·교차검증 (2026-09-02)

**배관 버그**: O-8c 워크플로 스크립트의 `parallel()` 호출에 이중 래핑 버그가 있었다(`[a,b,c].map(p => () => p)`가
각 원소를 "그 자체를 반환하는 함수"로 한 번 더 감싸버려, `parallel()`이 각 프라미스가 아니라 그 프라미스를 감싼
클로저를 기다린 뒤 그 결과를 반환 — Synthesize 단계에 실제 세 렌즈 텍스트 대신 `undefined`가 전달됨). Synthesize
에이전트는 빈 입력을 감지하고 스스로 소스를 재검토해 self-review를 만들어 §8.8을 작성했다 — 진행은 막히지
않았으나, **실제 3-패널이 낸 상세 결과가 Synthesize/이후 구현 단계에 전혀 전달되지 않았다.**

워크플로 완료 후 이 배관 실패를 알아채고 `journal.jsonl`에서 세 렌즈의 **전체 원문**을 직접 회수해 교차검증한
결과:

- **Lens A(재진입) 실제 발견 — §8.8 self-review가 놓친 것, [픽스 U1 — 확인 후 즉시 수정, 안전 결함]
  `tail_signal`이 `module_dispatch`와 달리 `catch_unwind`로 감싸이지 않음.** 모듈 최상단 독스트링의
  "트랩이 프로세스를 절대 abort시키지 않는다"는 절대 보장은 진입점 단위가 아니라 "JIT 코드에서 직접 호출되는
  모든 `extern "C"` 진입점" 단위인데, O-8c가 `tail_signal`이라는 **두 번째** 그런 진입점을 신설하면서 동일
  보호를 빠뜨렸다. 현재는 트리거 경로가 없음(leaf 함수, 재진입 불가, `argc`가 `u8` 상한이라 OOM 비현실적 —
  Lens A 자신도 "confirmed-but-dormant"로 채점)이나, "앞으로도 단순하게 유지된다"는 검사로 보장되는 게 아니라
  구조적으로 보장돼야 한다는 이 코드베이스의 기준에 맞지 않아 **직접 수정**: `module_dispatch`와 동일하게
  `catch_unwind(AssertUnwindSafe(...))` + 실패 시 `trapped.set(true)`(단, `tail_pending`은 마지막 문장이라
  패닉 시 세팅 안 됨 → 트램폴린이 정상적으로 `false` 분기로 값을 반환하고, 다음 dispatch부터 poison 적용 —
  기존 module_dispatch 패닉 처리와 완전히 동형). `crates/otter-engine/src/lib.rs`에 반영, 전체 게이트 재확인
  (워크스페이스 58그룹 그린, clippy -D, fmt, no_std 클린).
- **Lens A "보너스 발견"(§8.2 의사코드의 `TailReq::default()`를 루프 **안**에 두어 O-8b의 "꼬리루프 무할당"
  불변식이 깨진다는 지적) — 실제 구현은 이미 올바름(확인, 코드 수정 불요).** T2 구현 에이전트가 §8.2의 결함
  있는 의사코드를 문자 그대로 베끼지 않고 `let mut tail = TailReq::default();`를 루프 **밖**에 hoist해 `tail.pending
  = false`로 리셋 + `mem::swap`으로 버퍼 재사용하는 형태로 구현했다(§8.2 의사코드 자체는 오기였음 — 아래에서 정정).
- **Lens B(cranelift) 실제 발견 — self-review가 완전히 놓쳤으나 다행히 T2 구현 중 별도로 재발견·수정됨:
  `is_terminator()`가 `Effect::TailCall`을 모름(blocking).** cranelift의 기본블록 리더 도출은
  `OpHints.terminator`가 아니라 `is_terminator()`의 닫힌 `matches!(JumpTo|JumpIf|Halt|Return)`로만 결정되는데
  `TailCall`이 빠져 있어, debug 빌드는 `debug_assert_eq!`가 즉시 패닉(§8.7이 요구하는 결정적 게이트 실행 즉시
  발생), release 빌드는 리더 배치가 조용히 틀려 인접한 꼬리호출 두 개가 한 블록에 합쳐지고 cranelift verifier가
  이를 거부 → `compile_dispatch_fn`(→`maybe_tier_up`→`dispatch`) 경로에서 **`catch_unwind` 밖에서** 패닉(엔진의
  "절대 abort 안 함" 보장을 프로세스 수준에서 위반할 뻔한 경로). 실제로는 T2 구현 에이전트가 코드를 작성하며
  이 문제를 **독립적으로 재발견해 `is_terminator`에 `TailCall` arm을 추가**했다(설계 문서가 지시해서가 아니라
  현장에서 컴파일이 실제로 깨져서) — 결과적으로 안전하지만, 이건 설계·리뷰 파이프라인이 아니라 우연한 이중
  방어가 막아준 것이다. **[픽스 U2 — 문서 갱신] §8.1에 "TailCall 하강 arm은 `is_terminator()`에도 등록해야
  block-leader 도출이 성립"을 명시 규범으로 승격**(이미 코드는 맞게 반영됨 — `crates/otter-cranelift/src/lib.rs`
  의 `is_terminator`가 `TailCall`을 포함).
- **Lens C(baseline) 실제 발견 2건 — 하나는 self-review와 우연히 일치(C1), 하나는 더 정밀한 근거로 보강:**
  (a) maxargc 사전스캔 결함은 Lens C도 "CONFIRMED, severe"로 독립 발견 — self-review의 C1과 내용 일치(둘 다
  같은 코드를 읽었으니 당연) — **이미 반영**. (b) `compile_module`(Table 모드)이 real 테스트/bench에서 이미
  13곳 이상 호출되는데 `TailCall`에 사전스캔·가드 없이 `unimplemented!()`로 남아있다는 지적(C3보다 훨씬 구체적
  근거 — 실제 호출 사이트 목록 제시). **[픽스 U3 — 평가, 코드 변경 불요로 판단]** 재검토 결과: 이 경로는
  JIT-FFI 경계를 넘어 실행되는 `module_dispatch`/`tail_signal`류(패닉이 프로세스를 abort시킬 수 있는 위험군)와
  달리 **평범한 Rust 함수 호출 경로**(컴파일 단계, `panic=unwind` 하의 통상 패닉 — 해당 테스트/호출만 실패, 이
  코드베이스가 막으려는 "malformed bytecode로 프로세스 전체 abort"에 해당하지 않음)이고, 이미 명확한 패닉
  메시지("...the ModuleEngine (Dispatch mode) is the only O-8c target; see design §8.4/§8.8 pick C3")와
  rustdoc(각 파일의 `compile_dispatch_fn`/`CallMode` 문서)으로 스코프가 문서화됨 — §8.4의 원래 스코프 판단(별도
  정적 AOT 제품, `ModuleEngine` 정책 대상 아님)은 유효하다고 재확인. 프론트엔드가 실제로 TailCall을 방출하기
  시작하면 그때 이 API에도 사전스캔+가드를 추가할지 재논의(현재는 알려진·문서화된 갭으로 기록, O-8c 완료
  조건 아님).
- **[픽스 U4 — §8.2 의사코드 정정]** 원문의 `let mut tail = otter::interp::TailReq::default();`가 루프 안에
  있던 것을 실제 구현(루프 밖 hoist + `tail.pending=false` 리셋 + `mem::swap`)에 맞게 정정.

**공정 교훈(향후 워크플로 작성 시 적용)**: `parallel()`/`pipeline()` 결과를 다음 단계 프롬프트에 문자열
보간하기 전에, 최소 `log()`로 각 결과의 길이/타입을 찍어 "진짜 값이 들어왔는지" 스모크체크할 것 — 이번처럼
빈 입력이 조용히 "그럴듯한 자체 생성 텍스트"로 대체되면 실패가 안 보인다(에이전트가 알아서 복구해 결과 자체는
"성공"으로 보고되므로). 이 세션에서는 최종 커밋 전 사람(나)이 journal.jsonl 원문을 직접 대조해 잡았지만,
매번 이렇게 대조하는 게 기본 안전망이 아니라 예외 처리가 되어야 한다.

## 8.10 AOT(`CallMode::Table`) TailCall 갭 — 정책 예외 + wasm 실사 (2026-09-02, 사용자 결정)

O-8c 완료 후에도 `CallMode::Table`(AOT 단일-모듈 컴파일, `otter_baseline::compile_module` /
`otter_cranelift::compile_module`)은 여전히 `TailCall`에 `unimplemented!()`. 근본 원인: 이 경로엔 **디스패처가
아예 없다** — `Call`이 `fnptr = table[callee]; fnptr(...)`로 콜리를 **직접** 네이티브 호출한다
(`otter-cranelift/src/lib.rs:944-958`, baseline도 동형). O-8c의 트램폴린("baked helper 호출 후 즉시 반환 →
디스패처가 신호 소비")은 신호를 받을 디스패처가 있어야 성립하는데 Table 모드엔 그게 없어 **메커니즘 자체를
못 쓴다**. 메우려면 진짜 네이티브 꼬리호출(cranelift `return_call`/`return_call_indirect`, baseline은 손수
`jmp` 기반 프레임 핸드오프)이 필요 — O-8c와 별개의, 더 위험한 신규 설계.

**사용자 결정 (2026-09-02)**:

- **[정책 예외 신설] 네이티브 백엔드(아키텍처 불문 — x86-64/AArch64/RV64GC 등) 대상 AOT(`CallMode::Table`)의
  기능 갭은 tier-parity 정책의 "반드시 메워야 함" 대상에서 제외한다.** 근거(§8.4/§8.8 픽스 C3와 정합, tsavorite
  §6도 독립적으로 동일 결론 — "otter는 in-process JIT이지 링크 가능한 AOT 네이티브 백엔드가 아니다",
  `otter-cranelift`에 `cranelift-object` 의존 없음으로 확인): `CallMode::Table`은 `ModuleEngine`의 적응형
  T0→T1→T2와 무관한 **별개의 정적 제품**(디스패처 없음, 티어 승격 없음, 컴파일 시점에 백엔드 하나 고정) —
  애초에 "티어"라는 개념이 성립하지 않아 그 사이의 "패리티"도 성립하지 않는다. 기존 §8.4의 스코프 판단을
  정책 차원에서 확정.
- **[wasm은 개별 검토 → 실사 결과 = "메울 갭 자체가 없음"]**: `otter-cranelift`를 wasm32-unknown-unknown ·
  wasm32-wasip2(WASI) 양쪽에 단독 빌드 시도 — **둘 다 컴파일 실패**(`cranelift-jit`의 전이 의존 `memmap2`/
  `region`이 wasm 스텁 구현에서 애초에 컴파일 안 됨; WASI도 무관 — 스텁이 arity조차 안 맞음). 즉
  **wasm에서는 cranelift AOT(Table 모드)가 TailCall 이전에 아예 존재하지 않는다** — O-8c와 무관하게 이미
  그랬던, 훨씬 더 근본적인 상태. "메울 갭"이 성립하지 않으므로 tail-call 관련 추가 작업 불필요.
- **[실사 중 발견된 별개 갭 — O-7 후속, tail-call과 무관] `otter-cranelift`는 크레이트 레벨 `#![cfg]` 게이트가
  없다**(`otter-baseline`은 `#![cfg(target_arch = "x86_64")]`로 비-x86_64에서 빈 크레이트인 것과 대조). 현재는
  `otter-engine`의 Cargo.toml이 wasm에서 `otter-cranelift` 의존 자체를 안 걸어 `ModuleEngine` 경로는 안전하지만,
  **`otter-cranelift`를 직접(엔진 밖에서) wasm 타깃으로 빌드하면 여전히 깨진다** — 랜드마인. O-7과 동일 패턴
  (`#![cfg(not(target_family = "wasm"))]` 류)으로 크레이트 자체를 wasm에서 빈 크레이트화할지는 별도 후속
  항목으로 기록(O-7b 후보) — 이번 O-8c 스코프 밖, tail-call 갭과 무관.
- **[후속 리서치 반영, 2026-09-02] wasm 예외조항의 프레이밍 정정 — "영구 moot"이 아니라 "wasm-emit 백엔드
  부재 조건부 moot".** §8.10 확정 직후 "cranelift-jit을 wasm에서 어떻게든 쓸 방법이 있는가"를 별도로 4갈래
  리서치(전문: `.claude-research-library/wasm-emit-backend-research.md`). 결론: (1) cranelift-jit가 wasm
  샌드박스 **내부**에서 실행되는 것은 wasm 스펙의 Harvard 아키텍처(실행가능 메모리 획득 경로 부재) 때문에
  구조적으로 영구 불가능 — 이건 진짜 "영구"가 맞음. (2) Cranelift 자체를 wasm **타깃**으로 재사용하는 것도
  불가능(wasmtime#2566, 메인테이너 확인, 2021년부터 미착수) — 손수 작성한 전용 백엔드만 유일한 길. (3) 그러나
  **"wasm-emit backend"**(otter IR을 네이티브 대신 새 wasm 모듈로 lowering해 호스트의 `WebAssembly.compile`에
  넘기는 4번째 코드생성 전략, [[arch-support-tiers]]에 이미 있던 아이디어)는 아키텍처적으로 타당하고
  **.NET Jiterpreter가 정확히 이 패턴의 프로덕션 선례**임을 확인 — `wasm-encoder`(바이트 인코딩)·`waffle`
  (CFG→구조적 제어흐름)이라는 프리아트도 존재해 "완전 백지"가 아님(규모는 `otter-cranelift`급, ~1500-2500
  LOC 추정). **결론: "wasm=T0 영구고정"은 확정 정책으로 삼지 않는다.** 오늘 시점 사실관계(cranelift AOT가
  wasm에 존재하지 않으니 TailCall 갭도 없다)는 안 바뀌지만, wasm-emit이 실제로 만들어지면 그것도 하나의
  "티어"가 되어 O-8c와 동일한 패리티 요구(TailCall 포함)가 그대로 적용돼야 한다 — §8.10의 wasm 예외조항은
  **"wasm-emit 백엔드가 존재하지 않는 동안에만 유효"**로 재해석할 것. wasm-emit 자체는 O-7c급 미래 프로젝트
  후보로 로드맵에만 기록, 지금 구현 착수 아님(tsavorite의 실제 요청은 O-7로 이미 충족돼 저비용·고우선순위가
  아님). [[otter-tier-parity-policy]] 메모도 동일하게 갱신.
- **[소멸 부기, 2026-09-03 (O-13 W-6)] 위 wasm 예외조항은 소멸했다.** 2026-09-03 O-13 W-5 착지로
  wasm-emit(**Twasm**)이 실제 티어가 되어("wasm-emit 백엔드 부재"라는) 이 조항의 전제가 해소됨. Twasm은
  [[otter-tier-parity-policy]]의 **완전 적용 대상**이며, 실제로 전 IR 변형(**TailCall 포함**)을 lowering한다
  — 갭 실증은 `otter-o13-wasm-emit-design.md` §4(Effect 8/8·Expr 10/10·NumTy 11/11, `unimplemented!` 잔존 0),
  실동작 증명은 같은 문서 §8(c) E2E 7항(1M-깊이 꼬리재귀 완주 포함, 전부 그린). 이 소멸은 wasm 조항에
  한정된다: **`CallMode::Table`(AOT) 네이티브 예외(본 절 첫 번째 결정)는 별개로 존속**한다 — Twasm은
  `ModuleEngine`(Dispatch 모드) 티어이지 AOT 경로가 아니므로 두 결정은 서로 독립.
