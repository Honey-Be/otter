# O-11 설계 — `HostFnCtx1`/`HostFnCtx2` + `Expr::HostCtx1`/`HostCtx2`

Status: **DONE**(구현+3렌즈 패널+2렌즈 독립 리뷰 통과, 커밋됨). tsavorite 원본 요청 §2("호스트 빌트인에
컨텍스트 포인터가 없음")에 대한 응답.

## 1. tsavorite 원안 기각 + 채택안

tsavorite는 "otter-engine이 이미 컴파일된 코드에 불투명 ctx(`*const ModuleEngine`)를 넘기는 배관이 있으니
그걸 재사용하라"고 제안했으나, 직접 코드를 조사한 결과 **이 배관은 Dispatch 모드(`ModuleEngine`)에서만
존재**하고, 그마저도 "엔진 자신의 부기용 포인터"이지 임베더 값이 아니었다. Table 모드(whole-module AOT)의
param 0은 otter 내부 함수포인터 테이블이고, 순수 오라클 인터프리터에는 ctx 개념 자체가 없었다.

**채택안**: `Module`에 `fill`과 나란히 `host_ctx: Value` 필드를 추가 — 컴파일-타임 상수로 취급해 interp는
런타임에 읽고, T1/T2는 각 콜사이트에서 즉시값(immediate)으로 굽는다. `fill`이 이미 이 방식으로 오라클/
Table/Dispatch/`ModuleEngine`/O-9 `push_func` 전부에서 검증된 선례라, `CompiledFn`/`ModuleEntry`/
`CompiledDispatchFn::code_ptr` 등 기존 ABI를 전혀 안 바꾸고 순수 additive로 구현 가능했다.

## 2. 이름을 `ctx`에서 `host_ctx`로 바꾼 이유

3렌즈 패널 중 하나(Lens A)가 새 임베더 워드를 원안대로 `ctx`라 부르면, Dispatch 모드에서 이미 "ctx"라
불리는 `ModuleEngine` 부기용 포인터(`opaque_ptr`)와 같은 스코프·같은 뉘앙스로 공존해 타입 체커가 못 잡는
실수(예: 엔진 포인터를 실수로 host 콜에 넘김) 여지가 생긴다고 지적했다. 구현 단계에서 전면 `host_ctx`로
개명해 근본적으로 차단했다 — 두 값은 물리적으로도 다른 저장 위치(스택 스필 슬롯 vs 명령어 인코딩 내
즉시값)를 쓴다.

## 3. 패널이 잡은 핵심 결함 — `otter::engine::Engine`(Module-less 단일함수 티어링) 경로 누락

3렌즈 중 두 렌즈(A, B)가 **독립적으로 동일한 HIGH 결함**을 발견했다: 설계 초안의 통합 지점 스윕이
`lower_module_func`/`compile_module_func`(Module 기반)만 다뤘는데, `otter-cranelift::compile_fn`/
`compile_resumable`과 `otter-baseline::compile`이라는 **완전히 별도의, Module 없는** 컴파일 경로가
`lower_expr`/`emit_expr`를 부르고 있었다 — 이건 `otter::engine::Engine`(`ModuleEngine`과는 별개의
단일함수 티어링 서브시스템, `Backend`/`OsrBackend` 트레이트)이 소비하는 실제 프로덕션 인접 코드였다.
이 경로를 빠뜨리면 `Engine::call`이 스스로 문서화한 "티어 무관 결과 동일성" 불변식이 HostCtx 도입과
동시에 조용히 깨질 뻔했다. **반영**: 이 세 함수(+인터프리터 쪽 `interpret_from`/`interpret_until_osr`,
이미 설계가 인지하고 있던 대칭 사례) 전부 `host_ctx=0`으로 고정 — `Engine` 레이어는 애초에 `fill`
개념도 없다는 기존 선례를 그대로 연장한 것. 전용 회귀 테스트(`engine_hostctx_fixed_at_zero_across_all_tiers`)
로 T0→T1→T2 전체 승격 파이프라인에서 검증.

## 4. 그 외 패널 결함 (전부 반영)

- **[MEDIUM] baseline(T1) 레지스터 스테이징 순서 미명세** — x가 이미 rax에 있는 상태에서 host_ctx를
  rax에 movabs하면 x가 소실될 위험. 반영: `x→rsi 먼저, host_ctx→rdi 나중`(HostCtx1), `y→rdx, x(스택
  복원)→rsi, host_ctx→rdi`(HostCtx2) 순서로 정확히 구현 + rustdoc 명시. Cranelift는 SSA라 이 위험이
  구조적으로 없음.
- **[MEDIUM] Table 모드 지원 여부에 대한 설계 문서 내부 모순** — 지원 비용이 실측상 0(콜사이트 즉시값)
  이므로 **완전 지원으로 확정**(unimplemented!() 폴백 채택 안 함). O-8c의 TailCall과 달리 Table 모드도
  전부 지원.
- **[MEDIUM] 비-x86_64 CI에서 T1이 조용히 스킵되는 "3티어 일치" 테스트** — 신규 크로스티어 테스트를
  `otter-baseline` 크레이트 의존 파일(`three_tier.rs`)에 배치해 T1 검증이 구조적으로 x86_64 한정임을
  보장, `otter-engine` 신규 테스트는 기존 관례대로 최종 상태(`"cranelift"`)만 단언.
- **[MEDIUM] 이종 티어 caller/callee 경계 + TailCall 조합 미검증** — 신규 테스트로 T0 caller가 사전
  warmup된 T2 callee를 `Call`/`TailCall` 양쪽으로 호출해도 host_ctx가 정확한지 검증(엔진의 전역 임계값
  모델상 "T2 caller→T0 callee" 조합은 구조적으로 구성 불가능함을 확인해 그 방향은 제외).
- **[LOW] 극단값(전체 64비트 폭) 테스트 부재** — `i64::MIN`/`i64::MAX`/`-1i64` 등 최상위 비트가 켜진
  값을 오라클·크로스티어 테스트에 포함(작은 값으로는 baseline의 32비트 절단 버그가 안 잡힘).
- **[LOW] 다중 인스턴스 비간섭 테스트가 순차 인터리브만 검증, 동시성 범위 미명시** — 부분 반영(순차
  인터리브 유지, `ModuleEngine`의 `!Sync`는 O-11 이전부터의 기존 사실이라 이번 스코프 밖으로 명시).
- **[ADVISORY] rustdoc에 provenance/미래 타깃 가정 미기재** — `fill`도 동일한 문서화 공백을 이미 갖고
  있어 O-11 고유 문제가 아니라고 판단, 이번 범위에서 생략.

타입 선택(`Value` 통일 vs tsavorite의 `*mut c_void` 제안)은 패널 렌즈 C가 무손실 왕복(포인터 폭 ≤64비트,
x86_64/aarch64/riscv64/s390x 전부 해당)·기존 `fill`/`f as usize as i64` 패턴과의 정합성을 근거로 결함
없음 판정 — 설계 원안(`Value`) 그대로 채택.

## 5. 구현 후 2렌즈 독립 리뷰 결과

두 리뷰 에이전트 모두 CONFIRMED/PLAUSIBLE 결함 0건 — 진짜 재컴파일(소스 touch 후 clippy/fmt/no_std/
test 재현), x86-64 SysV ABI 레지스터 배정을 바이트 단위로 손수 재검산, `otter::Expr`를 destructure하는
전체 4곳(exhaustive match, 와일드카드 없음)을 grep으로 전수 확인, `Module`이 `Default`를 파생하지 않아
`host_ctx` 필드 누락 시 컴파일 자체가 실패한다는 점을 컴파일러 보증으로 활용해 41+곳 blast radius가 실제로
전부 채워졌음을 확인, 신규 테스트 6개의 기댓값을 바이트코드 시맨틱에서 직접 재유도해 우연한 통과가 아님을
검증. 유일한 지적(review A)은 rustdoc의 한 문장이 실제로는 발생하지 않는 가상의 레지스터 충돌 시나리오를
예로 든다는 사소한 서술 과장(기능 결함 아님, 등급 부여 안 함).

## 6. 최종 검증 (제가 직접, 커밋 전)

`cargo test -p otter -p otter-baseline -p otter-cranelift -p otter-engine --all-targets`(18+3+6+7+16+25+9+3,
전부 통과, O-8/O-9/O-10 회귀 없음) + `cargo test --workspace`(전체 그린) + `cargo clippy --workspace
--all-targets -- -D warnings`(무경고) + `cargo fmt --all --check`(통과) + `cargo build --workspace`(그린)
+ `host_ctx` 명명·Table 모드 배선을 diff에서 직접 대조 후 커밋.

## 7. 남은 갭 (O-11 스코프 밖, 명시적으로 기록)

- `runtime`/`runtime-heapbridge`(JS 런타임)는 `host_ctx: 0`(미사용)으로만 채움 — JS 런타임이 실제로
  HostCtx 빌트인을 쓸 계획(예: Realm/에이전트별 힙 분리)이 생기면 별도 통합 이슈.
- `ModuleEngine`의 `Send`/`Sync` 여부(스레드 간 이동/동시 접근 경계) 문서화 — O-11 이전부터의 기존
  갭, 이번에 새로 만든 문제 아님.
