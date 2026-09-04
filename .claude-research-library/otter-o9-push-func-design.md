# O-9 설계 — `ModuleEngine::push_func`

Status: **DONE**(구현+2렌즈 적대적 리뷰 통과, 커밋됨). 목표: 실행 중인 `ModuleEngine`에 새 `FuncDef`를 런타임에
주입하고 그 인덱스를 반환한다(REPL/eval 유스케이스). 이 크레이트의 핵심 불변식인 "컴파일드 코드는
add-only"를, "함수 테이블 자체도 add-only"로 확장하는 작업이다.

시그니처: `pub fn push_func(&self, f: FuncDef) -> usize`.

이 문서는 워크플로(`wf_1e804e57-aef`, 2026-09-02)가 산출한 설계 + 3렌즈 적대적 패널 + 구현 + 2렌즈 적대적
리뷰의 결과를 기록한다. otter-o8-tailcall-design.md와 동일한 verify-before-implement 절차(설계 → 패널 →
구현 → 리뷰)를 O-9에도 적용했다.

## 1. 저장소 확장 전략 (채택안)

`otter::Module`은 한 글자도 바꾸지 않는다(대안: `Module.funcs` 자체를 성장 가능하게 만드는 안은 블라스트
반경이 너무 커서 기각 — 오라클/단일함수엔진/AOT Table모드/`crates/runtime`/수십 개 테스트가 전부 "평범한
`Vec<FuncDef>`"를 가정). 대신 `ModuleEngine`에 논리적 인덱스 공간을 `module.funcs.len()`(= `base()`)을
경계로 "원본 구간"(`idx < base`, 오늘과 100% 동일, 절대 안 자람)과 "push 구간"(`idx >= base`)으로 나누는
병렬 저장소를 추가한다:

- `pushed: RefCell<Vec<Box<FuncDef>>>` — 개별 힙-박싱. 바깥 `Vec`가 재할당돼도 `Box` 포인터 값만 복사되고
  `FuncDef` 자신의 힙 블록 주소는 불변 — `run_one`이 중첩 호출 내내 쥐고 있는 `&FuncDef`가 재진입 중
  `push_func`가 그 `Vec`을 재할당시켜도 무효화되지 않는다는 핵심 요구사항을 만족.
- `pushed_counts: RefCell<Vec<Cell<u32>>>` — 원소 자체는 주소 안정성이 필요 없음(어떤 코드도 `Cell`을
  중첩 호출 너머로 들고 있지 않으므로 인덱스 안정성만 있으면 충분).
- `pushed_baseline`/`pushed_cranelift: RefCell<Vec<RefCell<Option<CompiledDispatchFn>>>>` — 원본
  `baseline`/`cranelift`(고정 크기 `Box<[RefCell<Option<..>>]>`)와 달리 성장형이라 이중 `RefCell` 중첩이
  필요. 주소 안정성은 `Box`/`Vec` 레이아웃이 아니라 `CompiledDispatchFn::code_ptr()`가 **별도 할당된 실행
  버퍼**(baseline=`ExecBuf`/mmap, cranelift=`JITModule` 소유 코드 버퍼)를 가리키는 값-복사 raw pointer를
  반환한다는 사실에서 나온다(구현 단계에서 review agent가 Miri로 실측 확인).

원본 필드(`counts`/`baseline`/`cranelift`)는 타입·초기화 로직 불변 — 원본 함수 실행(호출의 압도적 다수)은
`idx < base` 분기에서 borrow 비용 0, push된 함수만 그 대가를 낸다(핫패스 성능 보존).

부수 리팩터: `otter_baseline::compile_dispatch_fn`/`otter_cranelift::compile_dispatch_fn`(+cranelift 내부
`lower_module_func`)의 시그니처를 `(isa, m: &Module, func_idx, ...)`에서 `(isa, func_idx, f: &FuncDef,
fill: Value, ...)`로 좁혔다 — push된 함수는 `otter::Module` 안에 살지 않으므로 컴파일러가 `&Module`을
요구하면 안 된다. cranelift의 whole-module AOT 경로(`compile_module`, `Table` 콜모드)의 기존 호출부도
`m.fill`을 넘기도록 갱신(패널 Lens C가 "호출부 2곳"이라는 설계 초안의 주장이 실제로는 3곳+임을 발견 — 반영
완료, review agent가 grep으로 워크스페이스 전체 재확인).

## 2. 재진입성 계약 (채택안)

**재진입을 명시적으로 지원한다.** push_func는 이미 실행 중인 dispatch 체인 안에서(예: 미래 hostcall이
otter 실행 도중 새 함수를 주입) 호출될 수 있다. 근거: (1) 저장소 설계 자체가 이미 재진입 가능하도록
만들어져 있어 추가 비용이 없다, (2) 이 크레이트의 존재 이유가 "임의로 깊은 재진입 호출 체인이 공유 참조만
쥐고 있어도 sound하다"는 것이라 예외를 두면 철학과 모순된다, (3) REPL/eval 유스케이스 자체가 재진입
시나리오다, (4) 반환 타입이 순수 `usize`(Option/Result 아님)라 "위반 시 안전 실패"를 표현할 방법이 없다.

## 3. 이중 borrow / add-only 위반 시나리오 (패널이 지적, 반영 완료)

설계 초안은 "규율이 깨지면 최악은 정의된 패닉"이라고만 서술했으나, 패널(Lens A finding #1, HIGH)이 두 개의
**서로 다른** 규율을 구분해야 함을 지적했다:

- **규율 A(이중-borrow 회피)**: 위반 시 `RefCell::borrow_mut()`가 **패닉**(정의된 실패, `catch_unwind`가
  FFI 경계에서 잡아 `trapped`로 전환).
- **규율 B(add-only)**: `pushed`의 기존 슬롯을 절대 교체/제거하면 안 된다. 위반 시 `func_ref`가 이미
  `Ref` 가드를 드롭한 뒤 반환한 raw-pointer-derived `&FuncDef`가 **댕글링**된다 — 이건 패닉이 아니라
  **조용한 UB**로, `catch_unwind`가 절대 못 잡는다.

구현에 반영: `pushed` 필드 독스트링과 `push_func` rustdoc 양쪽에 "이 슬롯은 엔진이 사는 한 절대 교체·제거
금지 — 위반 시 UB, 컴파일러가 막아주지 않는다"를 명문화. 미래에 "이미 push된 함수 재정의" 기능이 필요해지면
이 필드를 재사용하지 말고 별도 설계(세대 카운터/에폭 기반 무효화)로 처리하도록 갈래를 남겼다.

또한 패널(Lens B finding #1, MEDIUM)이 지적한 "push_func의 4단계 순차 append가 원자적이지 않다" —
`pushed`/`pushed_counts`/`pushed_baseline`/`pushed_cranelift`를 개별적으로 `borrow_mut()`하면, 그 사이에
(규율 A 위반으로) 패닉이 나면 네 저장소 길이가 어긋난 채 `trapped` 표시 없이 남는다는 지적도 반영: 구현은
**네 개의 `borrow_mut()` 가드를 모두 먼저 획득한 뒤에만** push한다(push_func 본문이 콜백을 부르지 않는
순수 코드라 이 가드들을 동시에 쥔 창에서 재진입이 원천적으로 불가능 — review agent가 재확인).

## 4. panic=unwind 전제 (패널이 지적, 반영 완료)

패널(Lens A finding #3, MEDIUM-HIGH)이 지적: "패닉이지 abort 아니다"라는 재진입 안전성 논거 전체가
`panic = "unwind"` 빌드 전제에 의존하는데, 이 파일은 `target_os = "none"`(베어메탈, T0만 남는 타겟)을
지원 대상으로 명시하면서도 그 타겟의 panic 전략을 확인한 적이 없었다. 워크스페이스에 `panic = "abort"`
설정이 없음(review agent가 grep 재확인)을 근거로 오늘은 안전하지만, 이건 타입 시스템이 아니라 프로파일
설정 한 줄에 의존하는 보장이라 어떤 CI 게이트도 지키지 않는다. 반영: 모듈 최상단 독스트링에 "이 파일의
모든 트랩-아닌-abort 보장은 `panic = \"unwind\"`를 전제한다"는 하드 인바리언트를 O-9 문단으로 명문화하고,
이를 빌드타임에 강제하는 게이트 도입을 **O-12로 별도 추적**(기존 O-12 "panic=abort가 fail-closed를
무력화"와 정확히 같은 위험 범주).

## 5. JIT 콜 타겟 금지 (패널이 지적, 반영 완료)

패널(Lens B finding #3)이 지적: `push_func`(및 미래의 다른 `&self` mutator)는 JIT'd 코드가 직접 호출하는
baked call target이 되어서는 절대 안 된다 — `module_dispatch`/`tail_signal`만 `catch_unwind`로 감싸진
`extern "C"` 진입점이기 때문이다. 오늘은 이 위험이 물리적으로 불가능하다(hostcall effect가 아직 없고,
push_func는 baked call target 목록에 없음 — grep 확인). 반영: 모듈 최상단 독스트링에 하드 인바리언트로
명문화하고, **O-11(HostFnCtx)의 수락 기준에 하드 게이트로 승격** — O-11이 hostcall 트램폴린을 도입할 때
반드시 자체 `catch_unwind` 래핑 `extern "C"` 진입점을 통해서만 엔진 mutator를 호출해야 한다.

## 6. 테스트 (패널이 커버리지 갭 3건 지적, 전부 반영 완료)

패널(Lens C)이 지적한 3건 — (1) 테스트 (e): 트램폴린이 실제로 컴파일드 티어에 도달한 상태에서
push-세그먼트 콜을 실행함을 강제하지 않음 → `tier_of` 사전 단언 추가. (2) 테스트 (f): "함수 A"가
원본인지 push된 함수인지 불명확해 Box 주소-안정성(대안 C의 핵심 주장)을 실제로 스트레스하지 못할 위험 →
push된 함수를 먼저 주입하고 그 실행 도중 추가로 여러 번(재할당을 강제할 만큼) push하도록 구체화. (3) 실제
패닉→catch_unwind→trapped 체인을 실측하는 테스트 없음(Lens A finding #4) → 신규 테스트로 추가.

최종 테스트 스위트(12개, 기존 4개 + O-9 신규 8개, 전부 통과 확인):
`pushed_func_matches_oracle_across_tiers`, `pushed_func_tiers_up_monotonically`,
`push_func_does_not_disturb_an_already_tiered_original_function`,
`consecutive_push_func_indices_are_dense_and_correct`,
`original_trampoline_calls_a_pushed_func_via_runtime_callee`,
`oob_callee_returns_fill_not_abort_after_push`,
`push_func_reentrant_during_nested_dispatch_survives_realloc`(재할당 스트레스, 대안 C 실증),
`pushed_double_borrow_is_a_caught_trap_not_an_abort`(패닉→trap 체인 실측).

## 7. 검증 (2026-09-02)

- 3렌즈 적대적 패널(재진입 안전성/패닉-abort 안전성/완전성+테스트) — 5+3+5건 결함 발견, 전부 구현에 반영.
- 구현 후 2렌즈 독립 리뷰(재진입+unsafe 정확성, 정확성+테스트 커버리지) — **CONFIRMED/PLAUSIBLE 결함 0건**.
  두 리뷰 에이전트 모두 보고서를 무비판적으로 믿지 않고 직접 `cargo test`/`clippy`/`fmt`/`git diff`를
  재실행해 수치를 재현했고, 한 에이전트는 신규 `unsafe` 블록(`func_ref`, 1곳)을 **Miri**(Stacked Borrows +
  Tree Borrows 양쪽)로 직접 실행해 UB 없음을 확인했다.
- 내(주 세션)가 직접: `git diff`로 세 크레이트 변경분 전문 대조, `cargo test -p otter-engine -p
  otter-baseline -p otter-cranelift --all-targets`(24개 테스트, tailcall 회귀 스위트 포함, 전부 통과),
  `cargo clippy --all-targets -- -D warnings`(무경고), `cargo fmt --all --check`(통과), `cargo build
  --workspace`(그린) 재확인 후 커밋.

남은 항목(O-9 스코프 밖, 명시적으로 후속 과제로 남김): `dispatch`가 crate-private이라 외부 크레이트가 push된
함수를 실행할 공개 경로가 아직 없음 — 얇은 `pub fn call(&self, idx, args) -> Value` 래퍼가 조만간 필요
(패널 Lens A finding #7). 트랩된 엔진 위에서 push_func는 계속 성공하지만, O-10(`clear_trap`) 착지 후 트랩
전후로 섞여 push된 함수들의 상태 재개가 안전한지는 O-10 설계 시점에 재확인 필요.
