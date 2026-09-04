# O-17 설계 노트 — 수동적 shared-memory 호환 (L0)

상태: **구현 완료** (2026-09-03, 4중 조합 스파이크 그린 → GO 판정에 따른 전 스코프 구현).
상위 문서: `.claude-research-library/otter-o13-wasm-emit-design.md` — 본 노트는 그 §5.2를 **v1.1**로 개정한다(§5.2에 개정 부기 있음). 스파이크 정본: O-17 스파이크 보고(4중 조합 실측, M1–M5; 증거물은 스크래치 — 저장소 추적 파일 무수정 확인).

## §1. 목표 — L0 "수동적" 호환이란 무엇이고 무엇이 아닌가

오너 상향 목표 ②의 L0: 게스트(엔진-인-wasm 아티팩트)가 **+atomics(공유 선형 메모리)로 빌드되어도** 방출 모듈이 링크·동작한다. 즉:

- **이다**: 방출 모듈의 `otter.memory` 임포트 타입을 게스트 메모리의 공유 여부에 맞춰 파라미터화(§4) — 공유 메모리는 *링크 표면*의 문제일 뿐, 방출되는 함수 바디는 비트 단위 동일하다(구조 테스트로 고정).
- **아니다**: 동시 실행. 엔진은 단일 스레드를 유지한다(§6). 스레드 스폰·에이전트 복수화·atomic 방출 명령은 전부 스코프 밖(L2 관망 — §8).

## §2. 스파이크 실측 요지 (M1–M5, verify-before-implement 게이트)

측정: nightly-2026-08-31(§7 핀), wasm32-wasip1(비-threads 타깃), wasmtime 48.0.1.

1. **M1**: `+atomics,+bulk-memory` 리터럴 조합은 **불성립** — std가 wasi에서 errno를 무조건 `#[thread_local]`로 선언하고, atomics∧bulk-memory일 때만 LLVM이 실 TLS reloc을 방출 → 프리빌트 단일스레드 wasi-libc의 .bss errno와 충돌(`R_WASM_MEMORY_ADDR_TLS_SLEB cannot be used against non-TLS symbol 'errno'`). **성립 변형**: V4 = `+atomics,-bulk-memory`(TLS 하강 차단, atomics 유지 — 실 atomic 명령 21개 포함) 및 V3 = `+bulk-memory`만(atomics 없는 shared). 두 변형 모두 `--shared-memory --max-memory=1GiB --no-check-features` 링크로 성공.
2. **M2**: 산출물 `(memory 17 16384 shared)`(limits flags 0x03), try_table 438~442개 보존 — **exnref × atomics 공존 확인**, `wasm-tools validate --features all` OK.
3. **M3**: wasmtime 48 기동 요건 3종(§5) + `Extern::SharedMemory` 분기 필요. in-wasm catch_unwind 그린(E2E-4-TRAP-OK).
4. **M4**: 무패치 방출(비공유 임포트)은 `incompatible import type` → `-2` sticky Refused(재시도 없음, T0 강등 정답) — **강등 경로가 실측으로도 fail-safe**. 임포트만 shared+max로 패치하면 **E2E-ALL-OK 완전 녹색**(6/6 컴파일 + reject 모드 녹색) ⇒ §4의 방출 수정 + §5의 호스트 3종이 **충분 조건**.
5. **M5**: 임포트 매칭 = 표준 limits 서브타이핑. **선언min ≤ 실제min ∧ 선언max ≥ 실제max ∧ shared 정확 일치(양방향 무강제)**. shared는 max 선언이 검증 필수. `memory.grow`는 메모리 **타입** 불변 ⇒ 안전 방출 형상 = **(shared, min=0, max=65536)**.

## §3. §7 빌드 레시피 (게스트 측 — justfile `wasm-atomics-rustflags`가 값의 정본)

- **정본 채택(스모크 게이트용)**: **V4** — `wasm-eh-rustflags` + `-Ctarget-feature=+atomics,-bulk-memory,+mutable-globals -C link-arg=--shared-memory -C link-arg=--max-memory=1073741824 -C link-arg=--no-check-features`. V4를 스모크 정본으로 삼는 이유: **엔진 자기-인지(§4.2)가 cfg!(target_feature="atomics")이므로 +atomics 빌드만이 shared 방출 경로를 실제로 밟는다**(스파이크 권고는 V3 정본/V4 스모크였으나, **증분 설계 지시**(O-17 워크플로 과업 정의 — 오너 지시 아님, 최종 리뷰 B#2 귀속 정정)가 자기-인지를 cfg! 컴파일 타임으로 고정했고 그 선택 하에서 V3 정본은 자기모순(cfg!가 V3를 못 잡아 영구 T0)이라 스모크 정본은 V4로 낙착 — V3의 지위는 §7 잔여 갭 참조).
- `-bulk-memory`는 최적화가 아니라 **load-bearing**(M1의 TLS 충돌 회피). `+atomics`는 nightly에서 "unstable feature" 경고(§7이 이미 nightly라 무해).

### §3.1 `--no-check-features` 핀 결정 (§7.1식 기록)

- **왜 필요**: rust-std의 프리빌트 wasi-libc 오브젝트(및 Rust 자신의 cgu 오브젝트)가 atomics/shared feature 플래그를 안 실어 lld의 per-object feature 검사가 링크를 거부한다(`'atomics' feature must be used in order to use shared memory`). build-std는 wasi-libc를 재컴파일하지 못한다(소스/C측).
- **무엇을 끄나**: lld의 "모든 오브젝트가 선언 feature와 정합해야 한다" 안전망 전체. 이 안전망이 지키는 실질은 *다중 스레드에서 비-atomics 코드가 공유 메모리를 만지는 사고*인데, **L0은 동시 실행이 원리적으로 없으므로**(§6) 건전하다.
- **재평가 조건**: L2(실제 동시 실행)로 상향하는 순간 이 플래그는 재심사 대상 — 그때의 유일한 완전 경로는 커스텀 wasi-libc(atomics+TLS errno) 빌드다(§7-3). §7.1 핀 범프 절차 때 함께 재확인.

## §4. 방출 memory 임포트 타입 결정 규칙

### 4.1 형상 (otter-wasmemit — `EmitOpts::shared_memory`)

| 게스트 메모리 | 방출 임포트 | 근거 |
|---|---|---|
| 비공유(기본) | `(memory 0)` — min 0, max 무선언 | 기존 §5.2 그대로 — 비공유 클래스의 보편 슈퍼타입. **바이트 단위 무회귀**(테스트 고정) |
| 공유(+atomics 빌드) | `(memory 0 65536 shared)` | M5: shared는 max 필수, 65536페이지(4 GiB) ≥ 모든 wasm32 실제 max, min 0 ≤ 모든 실제 min, shared 일치 ⇒ **모든 wasm32 공유 실메모리와 매칭**. grow해도 타입 불변이라 안정 |

sharedness는 서브타이핑상 **양방향 무강제**(비공유 실메모리를 shared 선언으로도, 그 역으로도 임포트 불가) — 이것이 "하나의 만능 형상"이 존재하지 않고 **파라미터화가 정확히 맞는 설계**인 이유다. 테이블 임포트는 무변경(테이블 공유는 shared-everything-threads 영역 — 스코프 밖).

- **API 결정**: `compile_dispatch_fn`의 8-파라미터 시그니처(설계 §9 고정, cranelift/baseline과 정신적 패리티)는 불변. 신설 `compile_dispatch_fn_with(..., EmitOpts)` + `#[non_exhaustive] EmitOpts { shared_memory: bool }`(빌더 `with_shared_memory`) — 기존 콜사이트(테스트 3종 + 엔진) 전부 무churn, 미래 형상 스위치는 EmitOpts에 승차. 기본값 동일성은 `default_opts_are_byte_identical_to_the_legacy_entry_point` 테스트로 고정.

### 4.2 엔진의 자기-인지 — `cfg!(target_feature = "atomics")` (컴파일 타임)

게스트 자신이 자기 메모리의 공유 여부를 알아야 방출 형상을 고를 수 있다. otter-engine은 **컴파일 타임** 신호 `cfg!(target_feature = "atomics")`를 쓴다(`compile_wasm` 4단계): §7 atomics 레시피는 게스트 전체(otter-engine 포함)를 `+atomics`로 빌드하며 `--shared-memory` 링크와 항상 동행하므로, **지원되는 §7 구성 전부에서 플래그와 실제 공유 여부가 일치**한다. 불일치 구성의 거동은 §7(잔여 갭) — 어느 방향의 오판이든 호스트의 정확-일치 매칭이 `-2` → sticky Refused → 영구 T0로 흡수한다: **티어업 손실일 뿐, 오컴파일은 구조적으로 불가능**(fail-safe 방향).

## §5. 호스트 요건 (wasmtime 48 참조 호스트 — 전부 M3 실측)

1. **crate feature `threads`** — atomics/shared **검증** 게이트(`features.set(THREADS, cfg!(feature="threads"))`). 없으면 shared는 물론 **비공유 +atomics 게스트(V1)조차** `threads support is not enabled`로 로드 실패. feature가 있으면 검증은 기본 ON(명시 `wasm_threads()` 호출 불필요 — 실측 일치).
2. **`Config::shared_memory(true)`** — **기본 OFF** 노브(48 신설 아님 — 최종 리뷰가 45.0.3에 동일 시그니처·기본값·문서로 실재함을 소스 확인; 업스트림 문서 사유 "enough deficiencies", 도입 PR 귀속은 미검증이라 기재하지 않음): 검증을 통과해도 인스턴스화에서 `shared memory support is disabled for this engine`으로 실패. **함정 항목**이므로 참조 호스트에 주석과 함께 명시.
3. **`Extern::SharedMemory` 분기** — shared 게스트의 memory export는 `Extern::Memory`가 아니라 `SharedMemory`로 온다: §5.3-1 읽기(`SharedMemory::data()` = `&[UnsafeCell<u8>]`에서 복사 — 단일-에이전트 규범으로 경합 부재, §6)와 §5.3-3 배선(`Extern::SharedMemory(m.clone())`) 양쪽 분기. wasmtime-wasi p1은 wiggle `GuestMemory::Shared`로 이미 대응(실측).
- **보안 게이트 기록**: wasmtime threads는 **tier-2**(upstream 명시: 보안 CVE 미발급 영역) — [[release-rowhammer-gate]]의 릴리스 보안 게이트 목록에 "wasmtime `threads` feature 활성 = tier-2 표면" 항목으로 등재할 것(본 노트가 그 근거 문서).

## §6. 단일-스레드 유지 규범 (L0 불변식)

공유 선형 메모리는 **링크 호환일 뿐, 실행 모델을 바꾸지 않는다**: 엔진은 단일 에이전트·단일 스레드로만 실행되고, 스레드 스폰 표면 자체가 존재하지 않는다(§7 게스트는 비-threads 타깃 wasm32-wasip1). 따라서:

- §2.5 **스크래치 버퍼 경합은 L0에서 원리적으로 부재**한다 — 경합에는 두 번째 실행 주체가 필요한데 그런 주체가 없다. 방출 코드의 스크래치 스토어, 호스트의 `compile` 중 게스트 메모리 read(§5.3-7/§13-4가 게스트 스택을 동결) 모두 단일 실행 흐름 안에서 일어난다.
- 참조 호스트의 `UnsafeCell` 복사(§5-3)가 안전한 근거도 동일 — 동시 기록자가 존재하지 않는다.
- 이 규범이 깨지는 순간(동시 실행 도입) 본 노트의 안전 논증 전체가 재심사 대상이다 — 그것이 §8의 L2 경계다.

## §7. 잔여 갭 — 정직 기록 (차단 지점 + 해소 조건)

1. **V3(atomics 없는 shared 게스트)의 자기-인지 실패 (과소 탐지)**: `cfg!(target_feature="atomics")`가 false → 비공유 임포트 방출 → `-2` sticky → **영구 T0**(동작은 정확, 티어업만 상실). *차단 지점*: 컴파일 타임에 "링크가 --shared-memory였는지"를 알려주는 cfg가 존재하지 않음. *해소 조건*: otter-engine crate feature(예: `shared-memory`)로 명시 오버라이드, 또는 §5.1 계약에 호스트 제공 런타임 감지 표면(예: `otter_jit.memory_kind`) 추가 — 어느 쪽이든 계약 표면 변경이라 별도 증분. V4 레시피가 스모크 정본인 현 시점에서 V3는 지원-비보증 구성.
2. **V1(+atomics·비공유 링크)의 과잉 탐지**: cfg는 true인데 실메모리는 비공유 → shared 임포트 → `-2` sticky → 영구 T0. O-17 이전에는 (threads-enabled 호스트에서) 무패치 완전 녹색이던 구성이므로 **의도된 맞바꿈**이다: +atomics를 켜는 유일한 §7 사유가 공유 메모리이므로 V1은 지원 구성이 아니며, fail-safe 강등으로 수용. *해소 조건*: 1과 동일(명시 오버라이드 표면).
3. **완전 `+atomics,+bulk-memory` 게스트 불성립**: M1의 wasi-libc TLS-ABI 충돌(빌드 불가 — 방출기/호스트와 무관). *해소 조건*: 커스텀 wasi-libc(atomics+TLS errno) 빌드 — wasip1-threads 타깃/wasi-thread-spawn ABI 의존 **없이** 가능함은 확인됐으나 별도 에스컬레이션(L2 관망 묶음).
4. **하네스의 shared 실행 경로 부재**: otter-wasmemit 차등 하네스(wasmtime 45)는 mock 메모리가 비공유라 비공유 형상만 실행한다 — shared 형상은 **방출 바이트의 임포트 섹션 단위 검증**(`shared_memory_opt_changes_only_the_memory_import_shape`: shared/min/max 값 + "임포트 섹션 밖 바이트 동일" + wasmparser 완전 검증)과 **E2E 스모크**(`just wasm-e2e-atomics`, 실 wasmtime 48)로 커버. 하네스 45에 shared mock을 넣는 것은 wasmtime 45의 threads 표면 검토가 필요해 보류(차등 실행의 가치는 바디 lowering인데 바디는 형상과 무관하게 비트 동일 — 커버리지 공백은 임포트 섹션뿐이고 그건 위 단위 테스트가 정확히 본다).

## §8. 금지 규칙과 L2 경계

- **wasip1-threads / wasi-threads 신규 의존 절대 금지** (사망 트랙): wasmtime 47에서 wasi-threads가 제거되었고(우리 호스트는 48) 업스트림 방향은 shared-everything-threads/component-model 스레딩이다. O-17의 전 구현은 이 금지를 준수한다 — 비-threads 타깃(wasm32-wasip1) + 링커 플래그만으로 성립하며, `wasi_thread_start`/thread-spawn ABI 어디에도 닿지 않는다.
- **L2(관망)와의 경계**: L2 = 실제 동시 실행(멀티 에이전트/스레드). 그 순간 필요한 것들 — 커스텀 wasi-libc(§7-3), TLS 실지원, 스크래치·엔진 상태의 동기화 재설계, `--no-check-features` 재심사(§3.1), atomic 방출 명령 검토 — 은 전부 본 증분과 분리된 별도 트랙이다. L0는 그 어떤 것도 선취하지 않되, **막지도 않는다**(형상 파라미터화는 L2에서도 그대로 유효).

## §9. 게이트 (구현 시점 실측)

- `cargo test --workspace` / `just lint` / `just no-std-check` — 무회귀.
- `just wasm-e2e` — 비공유 기본 경로 3모드 무회귀(방출 기본값 불변의 실증).
- `just wasm-e2e-atomics` (신설) — V4 게스트 빌드 + 참조 호스트: E2E-1(기동)/E2E-2(티어업)/E2E-3(오라클)/E2E-4(catch_unwind)/E2E-ALL-OK + `HOST-SHARED-MEMORY` 마커(실제 shared 경로 실증) + 컴파일 계정 일치. CI `wasm-eh-pinned` 레인에 스텝 추가(캐시 영향 검토 완료 — RUSTFLAGS별 유닛 공존, ci.yml 주석 참조).
- otter-wasmemit 단위: 신설 형상 테스트 2종 포함 전 그린.
