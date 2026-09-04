# O-12 설계 — panic=abort 하 fail-closed 경화

Status: **DONE**(구현+3렌즈 패널+2렌즈 독립 리뷰 통과+제가 직접 추가 결함 1건 수정, 커밋됨). tsavorite가
요구사항이 아니라 검증 중 발견해 공유한 사안(§5b) — panic="abort" 빌드에서 `otter-engine`의
`catch_unwind` 기반 fail-closed 보장이 완전히 무력화된다는 문제.

## 0. 스코프 재판단 (핵심)

이전 세션에서 "옵션 3(사전검증)을 옵션 1/2보다 우선"이라 잠정 기록했으나, 실측 결과에 비추어 뒤집었다:
**옵션 3을 "전면 완료"하는 건 이번 티켓 스코프로 비현실적**(malformed-bytecode 도달 가능 패닉 사이트가
최소 27곳, Cranelift 라이브러리 자체의 내부 패닉 표면은 원리적으로 열거 불가능)이었다. 대신:

- **옵션 2(빌드타임 강제)를 필수·최소 산출물로 확정** — `#[cfg(panic = "abort")] compile_error!(...)`가
  안정 Rust에서 실제로 동작함을 실측 확인(5개 시나리오), `otter-engine` 한 곳에만 게이트를 둬도 링크
  단위 안에서 panic 전략을 못 섞는다는 Rust 자체 제약 덕분에 `otter-baseline`/`otter-cranelift`/`otter`
  전체 빌드 그래프를 전이적으로 커버.
- **옵션 3은 `otter::validate` 모듈(신규, opt-in, additive)로 좁게 스코프** — opcode/local-index(Call.dest
  포함)/jump-target/call-slot/frame-size 5개(+제가 추가한 frame-layout까지 6개) 클래스를 실행 없이 정적
  증명. `push_func`/`ModuleEngine::new`은 API 파괴 없이 그대로 유지(강제 연결은 별도 후속 패널 리뷰 대상
  으로 명시적으로 이관).
- **조사 중 O-12 원 스코프보다 심각한 인접 결함 발견**: `otter-baseline`(T1)은 malformed local-index
  피연산자에 대해 **패닉조차 하지 않고** 스택 프레임 밖 메모리를 조용히 읽고/쓰는 코드를 컴파일한다 —
  catch_unwind로 절대 못 막는(예외 자체가 안 남) 진짜 메모리 안전성 구멍. 이게 옵션 3을 완전 보류하지
  않고 좁은 부분집합을 포함시켜야 하는 결정적 근거가 됐다.

## 1. 옵션 2 — `#[cfg(panic = "abort")] compile_error!` (otter-engine에만)

실측 검증: RUSTFLAGS/워크스페이스 profile/개별 패키지 profile(Cargo가 panic의 package-level override
자체를 거부함을 확인) 등 여러 경로로 우회 불가능함을 확인. `otter-baseline`/`otter-cranelift`는 `catch_unwind`
를 한 곳도 안 써서 게이트 대상이 아니고, `otter`(no_std, 베어메탈 `x86_64-unknown-none` CI 타깃)는
언와인딩 런타임 지원이 없는 게 정상이라 게이트를 **절대 넣지 않음**(넣으면 기존 CI를 깨뜨림).

**패널이 잡은 결함**(Lens B): 설계 초안의 wasm32 관련 서술("명시적 unwind 요청도 막힌다")이 실측 없이
쓰였고 부정확함 — 실제로는 `otter-engine` 단독 `cargo check`/`build`(rlib)는 통과하고, 실패는 **최종
바이너리 링크 시점**에 rustc 자체의 무관한 메시지(`panic_unwind does not have the panic strategy
unwind`)로만 나타남. **반영**: 문서 정정 + CI에 wasm 카나리아 예제(`wasm_panic_unwind_canary.rs`) 신설해
이 조합이 여전히 링크 실패하는지 감시(업스트림이 미래에 wasm unwind를 stable화하면 카나리아가 깨져
재검토 신호를 줌). 워크스페이스 루트 `Cargo.toml`에 `panic="unwind"`를 지금 명시하라는 패널 권고는
**의도적으로 거부**(구현자가 직접 재검증) — 베어메탈 `otter`(no_std) CI와 충돌 위험이 있고, no_std
스텝이 오늘 통과하는 건 rlib만 만들고 최종 바이너리를 안 링크하기 때문(우연이지 안전의 증거가 아님).

## 2. 옵션 3 — `otter::validate::check_func`/`check_module` (신규 opt-in 모듈)

`ValidationError`: `EmptyCode`/`FrameTooLarge`/`OpcodeOutOfRange`/`LocalOutOfRange`(Call.dest 포함)/
`JumpTargetOutOfRange`/`CallSlotOutOfRange`/`EntryOutOfRange`/**`FrameLayoutTooSmall`**(제가 리뷰 발견 후
직접 추가, §4). `local_ok`/`jump_target_ok`/`call_slot_ok`는 부호 우선 확인 → `i64` 승격 비교(검사기
자신이 검사 대상과 같은 오버플로 버그를 갖지 않도록). `Effect`/`Expr` 순회는 wildcard 없는 exhaustive
match(신규 IR variant 추가 시 컴파일 에러로 강제 검토).

**3렌즈 패널이 잡은 결함**(전부 반영):
- Lens A: `otter::opt::constant_fold`이 인벤토리에서 누락(동일 취약 클래스, 현재는 테스트 전용이라 미도달
  이지만 향후 재사용 시 위험) → 독스트링 경고 추가. `otter-baseline`의 `((total_slots*8) as i32)` frame-size
  narrowing cast가 §1.6보다 심각(음수 되면 `sub rsp`가 `add rsp`로 뒤집혀 return-address 인접 스택 스매싱
  primitive) → `FrameTooLarge`(`MAX_SAFE_NLOCALS` 상수) 신설.
- Lens C: `Effect::Call`의 `dest`가 `LocalOutOfRange`/`CallSlotOutOfRange` 어디에도 명시 안 됨 → `LocalOutOfRange`
  가 명시적으로 커버하도록 구현. 검사기 자체의 오버플로 안전성 미명시 → sign-first+i64 승격 패턴으로 구현.
  baseline/cranelift의 `Call`/`TailCall` `argc` 오버플로(§1.4와 동급 심각도, 인벤토리 미발견) → `CallSlotOutOfRange`
  가 `argc<=nlocals`를 전이적으로 보장해 부수적으로 닫힘, `FrameTooLarge`와 "동급 필수 짝"으로 문서화.
  `nlocals` 자체 상한 부재(§1.6과 자기모순) → `FrameTooLarge`.

## 3. CI 회귀 게이트 2개 신설

호스트 `panic=abort` 거부 확인 + wasm32 `panic=unwind` 명시 요청이 여전히 링크 실패하는지 확인(업스트림
변화 감시용 tripwire, §1 참고).

## 4. 구현 후 2렌즈 독립 리뷰 — 제가 직접 발견·수정한 추가 결함 1건

Review A(빌드타임 가드 실효성+스코프 정직성): 결함 0건 — 5개 시나리오 전부 재현, "반영하지 않음"으로
명시한 B-3(워크스페이스 전역 `panic="unwind"`) 거부 결정의 기술적 근거(Cargo가 panic의 package override
를 금지한다는 것)까지 직접 재실측해 확인.

Review B(패닉 제거 정확성+회귀): **CONFIRMED 결함 1건, 심각** — `check_func`/`check_module`이
`nparams + consts.len() <= nlocals`(FuncDef 자신의 독스트링이 명시하는 프레임 레이아웃 불변식,
`[params][consts][temporaries]`)를 전혀 검증하지 않음. 이 불변식을 어긴 `FuncDef`(예: `nparams=5,
consts.len()=8, nlocals=1`)는 다른 모든 검사를 통과하면서도 interp/cranelift에서 즉시 인덱스 OOB
패닉, **baseline에서는 패닉조차 없이 스택 손상**(O-12가 막으려던 바로 그 최악의 실패 클래스)을 낸다.
새로 추가된 800회 차등 fuzz 테스트도 `nparams`/`consts`를 변이시키지 않아 이 결함을 구조적으로
못 잡는다는 점까지 지적됨.

**제가 직접 수정**(워크플로 종료 후, 커밋 전): `ValidationError::FrameLayoutTooSmall { nparams, consts_len,
nlocals }` 신설, `check_func`에 `f.nparams > nlocals || f.consts.len() > nlocals - f.nparams`(뺄셈 기반,
덧셈 오버플로 회피 — Lens C의 sign-first 규율을 이 신규 체크에도 그대로 적용) 검사 추가, 모듈 "무엇을
증명하는가" 절에 새 커버리지 명시, Review B의 정확한 재현 케이스 + 경계값(정확히 일치) + 오버플로
스트레스(`nparams=usize::MAX`) 3개 회귀 테스트 추가. `cargo test -p otter --all-targets`(39+3 통과) +
`cargo test --workspace`(58개 블록 그린) + `clippy --workspace --all-targets -D warnings`(무경고) +
`fmt --all --check`(통과) + no_std 베어메탈 빌드(`x86_64-unknown-none`, 여전히 성공) 재확인 후 커밋.

## 5. 최종 정직한 회계

**확실히 막은 것**: panic="abort" 프로파일이 `otter-engine`(및 전이적으로 전체 빌드 그래프)을 컴파일하는
것 자체를 컴파일 타임에 차단 — 실측 검증됨. `otter::validate::check_func`/`check_module`을 실제로 호출
하는 경로에서는 opcode/local(Call.dest 포함)/jump-target/call-slot/frame-size/**frame-layout** 6개 클래스
전부가 정적으로 증명됨(리뷰 발견분 포함 최종 확정).

**부분적으로 막은 것**: `check_func`/`check_module`은 opt-in — 아무도 강제로 안 부른다. 이게 §1.4의
메모리 손상 취약점을 막는 유일한 현재 수단이므로 tsavorite를 포함한 소비자에게 적극 권고할 필요.

**알려진 채로 남긴 것**: `run()`(non-`try_`)의 무보호 최상위 구간(O-10 이전부터의 기존 갭). `otter::interp::run_module`/
`run_one`을 `ModuleEngine` 없이 직접 호출하는 경로(catch_unwind 전무). Cranelift 라이브러리 자체의 내부
패닉(제3자 코드, 원리적으로 영구 잔존 — panic=unwind+catch_unwind가 유일한 완화책). `otter-cranelift`의
`.expect()` 11곳을 Result 전파로 바꾸는 것(후속 티켓 후보). `check_func`를 `push_func`/`ModuleEngine::new`에
강제 연결할지(API 파괴 변경, 별도 패널 리뷰 필요 — 다음 담당자에게 명시적으로 이관).

이번 O-12는 "malformed bytecode가 절대 프로세스를 죽이지 못한다"를 완성하지 않는다. 완성한 것은 "panic
전략이 이 약속을 조용히 무효화하지 못한다"(실측 검증)와 "가장 심각한 인접 결함들을 막을 도구가 존재하며
opt-in으로 즉시 쓸 수 있다"(리뷰로 드러난 프레임 레이아웃 갭까지 포함해 확정)는 것이다.
