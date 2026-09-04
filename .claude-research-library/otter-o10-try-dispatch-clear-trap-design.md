# O-10 설계 — `ModuleEngine::try_dispatch` / `try_run` / `clear_trap`

Status: **DONE**(구현+2렌즈 적대적 리뷰 통과+제가 직접 2건 추가 수정, 커밋됨). 목표: tsavorite 원본 요청
§4("트랩과 정상값을 구별할 수 없고, 트랩이 영구적임")를 해결한다.

**전제(오해 방지)**: 여기서 다루는 "트랩"은 순전히 `module_dispatch`/`tail_signal`이 `extern "C"` 경계에서
`catch_unwind`로 잡는 malformed-bytecode/내부 버그 패닉이다. tsavorite의 Scheme 레벨 예외(`raise`/`guard`/
`with-exception-handler`)는 otter가 처리해줄 대상이 아니라 tsavorite 자신이 일반 Call/Return으로 otter
위에 구현할 것 — 이 설계와 무관하다.

## 1. 핵심 설계

- **`Trap { idx: usize }`**: 트랩을 값 채널과 분리하는 마커. `idx`는 패닉을 잡은 가장 가까운 경계
  (`module_dispatch`/`tail_signal`/`try_dispatch` 자신의 경계)에서 처리 중이던 함수 인덱스 — 완전한
  백트레이스가 아니라 근사 진단 힌트.
- **`try_dispatch(&self, idx, args) -> Result<Value, Fill>`**: `dispatch()`를 전혀 바꾸지 않고, 그 위에
  **반환값이 아니라 `trapped` 플래그의 전이**로 판별하는 얇은 wrapper. `fill`이 정상값으로도 흔히 반환되는
  임베더 도메인이라 "값이 fill과 같은지"로는 절대 판별 불가 — 이게 원 요청의 핵심 문제.
- **`clear_trap(&self)`**: `trapped`/`trapped_idx`/`tail_pending` 세 필드만 리셋. 근본 원인은 안 고침(같은
  조건이면 재발) — 이는 버그가 아니라 명시된 계약.

## 2. 패널이 잡은 결함 (설계 단계, 3렌즈 — 전부 구현에 반영)

1. **[Critical, Lens C] `try_dispatch`가 top-level(비중첩) 호출을 자체 `catch_unwind`로 감싸지 않으면
   tsavorite REPL의 헤드라인 유스케이스(방금 push한 함수를 즉시 호출)에서 malformed-bytecode 패닉이
   `Err(Trap)`이 아니라 생 Rust panic으로 샌다** — `module_dispatch`/`tail_signal`은 컴파일된 프레임이
   콜스택에 있어야만 열리는데, 새로 push된 함수를 처음 호출하는 순간은 정확히 그 프레임이 없는 경우다.
   **반영**: `try_dispatch` 자신이 `self.dispatch(idx,args)`를 `catch_unwind`로 감싸도록 구현(설계 원안의
   "얇은 wrapper"보다 한 겹 더 두꺼움, `dispatch()` 내부는 여전히 무변경).
2. **[Medium, Lens A] `otter-baseline::ExecBuf::map`의 mmap/mprotect 순서 버그**: `mprotect` 실패 시
   `assert!`가 패닉하면 `ExecBuf`(Drop 담당)가 아직 생성 전이라 매핑이 새고, O-10의 `clear_trap`+재시도
   패턴이 이를 "반복 가능한 누수"로 격상시킨다. **반영**: `ExecBuf { ptr, len }`을 `mprotect` 호출 **전에**
   구성하도록 순서 변경 — panic 시에도 언와인드 중 `Drop`(munmap)이 정상 실행.
3. **[Medium, Lens B] 제안된 트랩 테스트가 "플래그 기반 판별"과 "값 기반(`v==fill`) 판별"을 구별 못 함**
   (pass-through 트램폴린이라 우연히 `v==fill`). **반영**: 바깥 프레임이 nested trap의 fill 결과에 산술
   연산을 가해 반환값을 fill과 다르게 만드는 테스트(`try_run_discriminates_by_flag_not_by_return_value`)
   추가 — 나이브한 `v==fill` 구현이면 실패하는 유일한 테스트 형태.
4. **[Medium, Lens C] 기존 (e) 테스트가 "티어업 컴파일 자체의 결정론적 패닉" 재트랩 시나리오를 검증 안 함**
   (외부 RefCell 경합으로 대체됨). **반영**: 진짜 malformed opcode(9999)로 `compile_dispatch_fn`이 실제
   `isa.op()`에서 패닉하는 `clear_trap_retrap_loop_pins_to_the_same_backend_forever` 테스트 신규 추가 — T2
   결정론적 패닉이 T1 폴백 기회조차 없이 영원히 재발함을 실증(3회 재시도, `tier_of=="interpreter"` 고정).
5. 기타 저심각도 결함(§6 위험#1 문구 강화, "completely unaffected" 문구 범위 한정, pushed 세그먼트 OOB를
   `try_dispatch` 자체로 검증하는 테스트 추가, "tail_signal 경로 전용" 오기 정정) — 전부 반영.

## 3. 구현 후 2렌즈 독립 리뷰 결과 + 제가 직접 발견·수정한 2건

두 리뷰 에이전트 모두 `cargo test`/`clippy`/`fmt`를 직접 재실행해 보고서 수치를 재현했고, 한 에이전트는
`clear_trap_retrap_loop_pins_to_the_same_backend_forever`를 단독 실행해 실제 `index out of bounds` 패닉
메시지를 3회 육안 확인, 다른 에이전트는 진짜 FFI 체인(JIT'd 코드 → `module_dispatch` → `isa.op(9999)` 패닉
→ `catch_unwind`)을 직접 추적했다. CONFIRMED 결함 2건(둘 다 안전성 결함이 아니라 테스트/문서 정밀도 문제):

1. **`Trap::idx`의 rustdoc이 과장**: "nested call이면 더 깊을 수 있다"고만 썼는데, 실제로는 그 nested
   call이 **컴파일된 프레임의 FFI 경계를 실제로 거친 경우에만** 참 — 순수 T0 인터프리터로만 이어지는
   nested call은 아무리 깊어도 `try_dispatch`의 바깥쪽 경계에서 잡혀 **최외곽 idx**로 수렴한다(제가 직접
   확인 후 rustdoc 수정: "compiled frame을 거친 경우에만 deeper, 순수 인터프리터 재귀는 outermost idx").
2. **"이미 트랩된 엔진에 다른 idx로 호출" 분기가 어떤 테스트로도 커버 안 됨** — 리뷰가 임시 테스트로
   동작 자체는 계약대로 정확함을 확인했으나(순수 커버리지 공백, 버그 아님), 회귀 방지용 정식 테스트가
   없었음. 제가 직접 `try_dispatch_on_an_already_trapped_engine_reports_the_original_trap_idx_not_the_new_call_idx`
   테스트를 추가(idx=1을 트랩시킨 뒤 `clear_trap` 없이 idx=2로 호출 → 여전히 `Err(Trap{idx:1})`이지 idx=2가
   아님을 확인, 이어서 실제 clear_trap 후 idx=2가 정상 동작함도 확인).

두 수정 모두 `cargo test -p otter-engine -p otter-baseline -p otter-cranelift --all-targets`(22+16+9+3,
전부 통과) + `clippy --all-targets -D warnings`(무경고) + `fmt --check`(통과) + `build --workspace`(그린)
로 재검증 후 커밋.

## 4. API 계약 요약

- `dispatch()`/`run()`은 완전히 무변경(내부 구조·시그니처 그대로).
- `try_dispatch`/`try_run`은 세 가지 케이스를 정확히 구별: (1) 이미 트랩됨 → `Err(Trap{원래idx})`, (2) 이번
  호출 중 새로 트랩(FFI 경계 경유든 top-level이든) → `Err(Trap{해당idx})`, (3) OOB(원본+O-9 pushed 세그먼트
  포함) → `Ok(fill)`, 트랩 아님(기존 동작 그대로 보존).
- `clear_trap`은 `ModuleEngine` 자신의 필드에 대해서만 안전을 보장(찢어진 상태 없음, RAII+add-only
  불변식 덕분) — 백엔드가 소유한 프로세스 자원(예: 실행 가능 메모리 매핑)까지 보장하지는 않으며, 그건 각
  백엔드의 책임(`ExecBuf::map` 수정이 그 구체 사례).
- 재트랩 루프(같은 idx가 결정론적으로 계속 트랩)는 otter-engine이 자동으로 막지 않는다 — `Trap.idx`라는
  최소 진단 정보만 주고, 백오프/블랙리스트 정책은 embedder(tsavorite) 몫으로 명시적으로 남긴다.
