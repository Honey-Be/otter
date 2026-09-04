Status: DESIGN v2.3 — **구현 완료** (2026-09-04 — A-5 착지: §9 A-1..A-5 + A-C1 전 행 DONE. 잔여는 §11의 O-16c′-트리거 3건과 A-5 착지분에 대한 적대적 리뷰뿐)

# O-14 설계 — AOT 코어: `compile_module` + 내부 링크 + `return_call_indirect`

정본 관계 및 확정 결정 (귀속 명기):
- **선행 정본**: `otter-o13-wasm-emit-design.md`(현행 백엔드 계약 §2/§4/§5, 이하 "O-13"), `otter-wasm-scope-escalation-research.md` §3(AOT 갭 전수, 이하 "ESC"), `otter-wasm-mode-architecture-alternatives.md`(이하 "MODE"), `alt-wasm-runtimes-code-reuse-research.md` §4-5(선행 사례), `crates/otter-wasmemit/src/lib.rs`(현행 구현 전량 판독), O-14 스파이크 결과(2026-09-03, wasmtime 45.0.2 하네스-cfg + 48.0.1 호스트-cfg 양 환경 M1–M5 전부 PASS — 이하 "[스파이크]").
- **오너 결정 (MODE §5b 인용 — 자체 결정 아님)**: ① A-모드 게이트 프로파일 = **M2a 엔진-라이브러리 정적 링크**(결정 A: "2제품(M2a-게이트) 종합안 베이스 채택"), ② `Refusal→AotError` 전-모듈 실패 승격 비준(결정 C-②), ③ 콜리 범위검사 **명시 가드 fail-closed 모드 간 통일** 비준(결정 C-③), ④ NaN 페이로드 결정은 O-15 시점 이연(결정 C-④), ⑤ "동적 코드 실행은 격리 환경 경유·극히 제한" 규범 명문화 의무(결정 B — §7에서 이행).
- **AOT 계약의 element/data 허용**(§5.2 화이트리스트는 JIT 계약 전용): ESC §5 O-14 행이 규정하고 본 워크플로 지시가 확정 전달한 사항. §5b 원문에는 없으므로 오너 §5b에 귀속하지 않는다.
- **심판 이식 요소 (MODE §4.2)**: record==live 바이트-동일 CI 게이트([hybrid-first] 발), AOT⊂JIT 부분성-일관성 계약 + `AotError{func_idx, refusal}`([consumer-first] 발), WAMR XIP/WasmEdge universal-binary 선행 사례(alt-runtimes §4-5).
- 표기: **[실측]** = 스파이크/레포 직접 확인, **[추정]** = 미검증 추론. 설계 선택마다 근거를 명기한다.

---

## §0. 개정 요약 (v2 — 3렌즈 패널 abi-link/semantics/scope 반영)

**must-fix 3건 (전건 반영)**:
1. **record==live 게이트의 type_index 발산** (3렌즈 공통 최상위): live(함수당 모듈, 바디 첫-사용 순서 lazy 인터닝) vs record(N-in-1, 모듈-전역 누적 + 임포트 조립 선행)의 `call_indirect` type_index 즉값이 일반적으로 갈라져 "바이트 동일"이 원리적 불성립 — Call/TailCall 0개 함수(R1)조차 헬퍼/호스트 사이트에서 적색. 게이트를 **T-정규화 바이트 동일**로 재정의(§5.2): 하네스가 양 산물 type 섹션으로 각 사이트 즉값을 구조적 타입 키로 재사상 후 "사이트별 구조 동등 + 그 외 바이트 동일" 단언(전 타입 1-byte LEB라 길이 불변). 상충 기록: abi-link ⓐ(정규화 비교) vs ⓑ(선-인터닝+재작성) 중 **ⓐ 채택** — ⓑ의 live 측 선-인터닝은 §4.3 바이트-핀 계약과 충돌(scope 렌즈 지적)하고 함수별 첫-사용 순서는 상호 배타적이라 방출기 측 회피 자체가 불가. 양 방출기 동결 유지, 정규화는 게이트 하네스 몫. §1.1-1/§3.4-1/§5.1 문구 연동 정정.
2. **콜리 가드 관측-동등의 wasm32 반례** (semantics): wasm32-배포 JIT 제품/T0의 콜리 변환은 검사-전 `as usize` 하위 32비트 절단(`module_dispatch` otter-engine:1153, `tail_signal` :1283, T0 interp [실측 재확인])이라 x≥2³² ∧ x mod 2³² < nfuncs 워드에서 f_{x mod 2³²}를 호출 — 본 설계의 전폭 `i64.ge_u` 가드(fill)와 발산. 처방: **엔진 측 전폭 검사 co-requisite 신설**(§3.2a + §9 A-C1, W-0.5 Neg(F16) 선례 동형; 네이티브 무변경) — 대안(카브아웃 상신)은 오너 §5b C-③(fail-closed 모드 간 통일) 비준 취지와 상충하므로 기각(보수 선택: 전폭 가드가 정본, 절단 경로가 결함). §3.1 [실측] 과일반화 정정 + §6 배포-폭 조건 명기.
3. **선행기술 절 부재** (abi-link/scope): 오너 §4b-⑤ 비준("O-14 설계 문서의 선행기술 절 필수 반영") 이행 누락 — **§7b 신설**: WAMR XIP(채택 논거 + 재배치=재방출 결정), WasmEdge universal-binary(v1 기각 사유 명기), run-mode 게이팅(비적용 판정), alt-runtimes §5-5 결정 입력 3항 소화.

**should-fix 처리 (기각 0, 부분 이연 1)**: §5.2 R2 창 완결(TailCall 에필로그·센티널 고정값·`local.set` 접미 창-밖 명기), §4.1 매니페스트 보강(`host_slots`·`AotImport` 타입 정본=wasm 바이트·리졸버 최초-해소 캐시 결정성·`emitter_version`; **부분 이연**: 빌드-해시 fail-closed 전체 규율은 대조 상대(b′ 산물)가 실존하기 전 규정 불가라 O-16c′ 이연 — 최소 앵커만 지금), §2.2 헬퍼 명명 근거 정정(no_mangle 부재 [실측] — b′ 신규 export 의무로 등재), §2.5/§4.2 스크래치 4건(커버리지 의무·8-정렬 기계 강제·넘침 조건 정정 base+Σ>2³²·재배치=재방출 공인; **상충 기록**: `ScratchRegionOverflow` 모듈-수준 변형은 순차 배정 하 함수-수준 검사에 포섭되어 **삭제** — semantics '포섭' 관찰 채택, abi-link 조건-오기 정정과 정합), §2.7 [실측]→config-플래그 층위 한정 재표기, §6 포함-주장 매개변수 한정 + 역방향 원천 5종 정정 + 리졸버 쌍 고정 + push_func 제외 명기, §9 A-1 잠정 export·A-2 중간 형상 규범·A-C1 행 신설, §4.2 단일-패스 결정성 규범, §8.3 뮤테이션 ⑦(⊂ 비공허성) 추가, §8.4 ValidationFailed 추가/TooManyLeaders 값-수준 대체/UnsupportedEffect 제외, §2.5 서로소 주장 약화 + 잔존 불변식 §5.1 체크리스트 편입, §7-3 자구 방어화, §2.1 검증기 pre-link 전용 명기, §11-3 교차 등재.

**v2.1 — A-C1/A-1 착지 소급 정정 (2026-09-03, 구현 리뷰 조건)**: 구현이 설계보다 옳았거나 설계에
표현 결함이 있던 4건을 정본에 흡수한다(코드 변경 없음 — 문서-코드 발산 해소가 A-2 착수 조건이었다).
① §5.2 센티널 "5-byte LEB"는 **오기** — wasm-encoder 최소 SLEB로 실제 1바이트(0x7E/0x7F); 결정성·
상호구별은 성립하나 A-3의 R2 창 정의가 폭 5를 전제하면 재심. ② `scratch_region`의 end를 **u64**로
(§4.6 넘침이 `>` 2³²라 `== 2³²`가 합법인데 u32로는 표현 불가 — 절단 시 예약 의무 과소보고). ③
`AotError::TooManyTableSlots` 추가(`TooManyLeaders` 동형 anti-wrap 총체성 가드). ④ `AotImportKind`에
`Table`(A-1 잠정, A-2에서 소멸) 추가 + `Helper(HelperIdx)` → `Helper(HelperKind)` 명명 적응(실존 pub
`HelperIdx`는 5-슬롯 구조체라 충돌). §3.2a co-requisite(A-C1)는 **구현 시 정련**됐다: 사이트별 nfuncs
비교 대신 단일 헬퍼 `interp::callee_index`의 폭-포화(`usize::try_from(word as u64)
.unwrap_or(usize::MAX)`) + 기존 범위검사 경로 유입 — nfuncs 이중-소스를 회피하고 새 에러 종류 0,
fill/패닉 클래스 보존으로 §9 A-C1 게이트 행을 충족한다(두 리뷰 렌즈 모두 "설계 위반 아닌 취지-우월
실현"으로 판정).

**v2.3 — A-5 착지 부기 (2026-09-04, 구현 완료)**: 설계 이탈 0. 재량 결정·문언-초과분 및 잔여물 정리 5건을 기록한다(⑤는 재량 결정이 아닌 A-2 잔여의 HEAD 잠복 적색 해소).
① **E2E 배치 결정**(§8.2가 미규정하던 사항): 신설 워크스페이스-제외 크레이트 `hosts/otter-aot-e2e`
(레시피 `just wasm-aot-e2e`, CI wasm-eh-pinned 레인) — 참조 호스트 서브커맨드안은 기각(2제품 분리
MODE §4.1 + 호스트-트레인 락파일 위생 + 의존 집합 서로소; 크레이트 Cargo.toml/README에 근거 축자).
산물은 사전 방출이 아니라 **런타임 `compile_module` 생성**(단일 lowering 정본 유지). ② **§8.2 문언
초과분**: 2M 유계를 비공유(48판 M2 재확인)에 더해 **공유 형상에서도** 실행(AOT48-E2E-4 — §2.7 실측화
겸용), 음성 대조도 공유 형상에서 실행(4번 페이즈와 단일-축 대조가 되도록). ③ **semver 마감 재량
처분 3건**: `HelperKind`/`AotArtifact`/`HostImportName`은 **의도적 exhaustive**(각 타입 doc에 근거
명기 — 헬퍼 집합은 `helper_slots: [u32;3]`에 동결·다운스트림 extern-배선은 조용한 와일드카드가 아닌
시끄러운 컴파일 실패가 옳음 / 뮤테이션 하네스가 다운스트림에서 조작 산물을 구성해야 함 / 리졸버가
값을 구성해야 함); `#![warn(missing_docs)]` 승격으로 pub 표면 doc 완결성을 기계 강제(clippy -D
warnings 게이트 경유). ④ **§12 신설**: O-16c′ 서빙 인터페이스 정리 절 — 신규 규범 0, 기존 계약의
소비자-관점 재편성(상충 시 원 절 정본). ⑤ **A-2 잔여물 정리(무회귀 게이트가 적발)**:
`demos/otter-wasm-demo/Cargo.lock`이 A-2의 otter-wasmemit `wasmparser` dep 추가를 미반영한 채로
남아 `wasm-e2e`/`wasm-e2e-atomics`의 `--locked` 빌드가 HEAD에서 적색이었다(A-2..A-4는 이 두
레시피를 게이트 목록에 두지 않아 잠복). 오프라인 재생성으로 해소 — 추가분은 워크스페이스 핀과
동일한 `wasmparser 0.258.0`(+`bitflags 2.13.1`) 뿐, 그 외 항목 무변경. 두 게이트 재실행 그린.

---

## §1. 목표·비목표

### 1.1 목표 (O-14 = ESC §3.3의 AOT-1..3 + ⑥ 총체화)

`crates/otter-wasmemit`에 **전-모듈 빌드-타임 방출** 진입점 `compile_module`을 신설한다:

1. **N함수 1모듈** — 함수당 1모듈(현행 `compile_dispatch_fn`)을 승격. `Types` 인터닝은 이미 모듈-스코프 구조라 그대로 승격된다(ESC §3.3-① [실측]) — 단 이 승격은 type_index 즉값의 함수-간 재배치를 낳으므로(임포트 조립 선행 + 전역 누적 순서) 게이트는 §5.2의 T-정규화로 이를 흡수한다(패널 정정).
2. **내부 링크** — 모듈-내 선언 funcref 테이블 + active element 세그먼트로 "**otter 함수 인덱스 = 테이블 슬롯**" 성립(ESC §3.2 [실측] + [스파이크] M3/M5). 콜리 런타임 값이 그대로 `call_indirect` 피연산자가 된다.
3. **`TailCall` → 가드 + `return_call_indirect`** — 엔진 트램폴린(module_dispatch/tail_signal) 소멸. 유계성은 wasm 표준 꼬리호출이 담당([스파이크] M2: 상호재귀 2,000,000/2,000,001 깊이를 **64 KiB wasm 스택**으로 완주, 음성 대조 plain `call_indirect`판은 `Trap::StackOverflow`).
4. **실패 총체화** — 어떤 함수든 방출 불가면 `AotError{func_idx, refusal}` 전-모듈 실패(오너 비준 §5b C-②). AOT엔 T0 폴백 슬롯이 없으므로 sticky-Refused 강등이 존재하지 않는다(ESC §3.1 "정책 재정의만" 행).
5. **record==live 규율**(§5) + **AOT⊂JIT 부분성-일관성 계약**(§6) — 이중 정본 봉쇄와 모드-간 관측 동등의 게이트화.

### 1.2 비목표 (경계 명시)

| 항목 | 귀속 | O-14 산물이 서빙하는 인터페이스 |
|---|---|---|
| 스파이크 b′: 엔진-라이브러리 wasm 빌드(stable/panic=abort) | O-16c′ | O-14 산물의 **함수 임포트 이름 목록**(§2.2, 매니페스트에 기계-가독 수록)이 b′ 빌드의 export 요구 명세가 된다. **신규 의무(패널 정정, §2.2)**: 헬퍼 3종은 현재 `#[no_mangle]` 부재로 심볼이 맹글링되어 동명 export가 어디에도 실존하지 않음 [실측] — b′가 export 속성(`#[unsafe(no_mangle)]`/`#[export_name]`) 부여 또는 래퍼 방출로 §2.2 이름을 export해야 한다. 타입 정본은 산물의 wasm type 섹션(§4.1) |
| 스파이크 b″: M2a 링크 도구 | O-16c′ | bytes + 매니페스트(엔트리 슬롯·테이블 슬롯 맵·스크래치 예약 구간·임포트 목록)가 링커의 전체 입력. 링커 의무 = 임포트 해소 + 스크래치 구간 예약(§2.5) |
| 컴포넌트화(shared-everything link) | O-16c′ | O-14 산물은 코어 모듈 — 컴포넌트-정의-시점 정적 선언 형태가 규약이 정확히 허용하는 형태(MODE §2.1-④) |
| O-15 산술 자립(네이티브 op 하강·f16 정적 링크·M2b 엔진-리스) | 병렬 트랙(오너 §5b 결정 A: M2a는 O-15 불요 — 헬퍼는 임포트로 남고 정적 링크 시 엔진-라이브러리 export에 해소) | 헬퍼 호출 형상(§3.4)은 O-15가 와도 사이트 단위로만 교체되도록 국소화 |
| 상수-콜리 직접 `call`/자기-꼬리 `return_call` 최적화 | perf 후속 | 없음(§3.1 근거) |
| NaN 페이로드 divergence 처리 | O-15 시점(오너 §5b C-④) | 없음 — M2a에선 헬퍼=정본이 구성상 보존되어 크리티컬 패스에서 소멸(MODE §2.3-②) |
| 동적 코드/eval류의 A-모드 지원 | O-16 설계(격리 규범 §7) | 없음 — A-모드 산물엔 동적 코드 경로가 원리적으로 없음(§7) |

---

## §2. 산물 형상 (모듈 구조 규범)

### 2.1 섹션 구성

방출: **type, import, function, table, export, element, code** — start 없음(O-13 §5.2 규범 유지: 인스턴스화는 게스트 코드를 실행하지 않는다), **data 없음**(§2.6), custom 없음(v1; 진단용 name 섹션은 열린 결정 아님 — 미방출 확정, 매니페스트가 진단 채널).

**AOT판 화이트리스트(산물 검증기)**: {custom(0), type(1), import(2), function(3), **table(4)**, export(7), **element(9)**, code(10), **data(11)+datacount(12)**} — O-13 §5.2/§5.3-2의 JIT 화이트리스트에 table/element/data를 **계약 수준에서** 추가한다(ESC §5 O-14 행; JIT 화이트리스트는 JIT 계약 전용으로 불변). 추가 검사 1건 필수: **element의 대상 테이블 = 모듈-선언 테이블**([스파이크] M5 대조 프로브 — element→임포트 테이블은 코어 wasm으로 합법이고 인스턴스화가 호스트 슬롯 0을 **말없이 덮어씀**(센티널 42→게스트 f0 결과 8 실증). 즉 이 금지는 엔진이 강제해 주지 않는 **계약 강제**여야 한다). O-14 방출기는 구성상 임포트 테이블이 없어 자동 충족하지만, 검증기는 방어적으로 검사한다(설계 권고 4의 이행). **적용 범위(패널 명기)**: 이 화이트리스트·검증기는 **pre-link 산물 전용**이다 — M2a 링크 후 산물은 global(섀도 스택 포인터)·소유 memory 등을 필연적으로 갖게 되어 대상이 아니며, 링크-후 검증은 O-16c′ b″ 관할이다.

### 2.2 임포트 (이름공간·명명 규칙)

```wat
(import "otter" "memory" (memory 0))                          ;; [기본] — O-13 §5.2와 동일 형상
(import "otter" "memory" (memory 0 65536 shared))             ;; AotOpts.shared_memory=true (O-17 교차, §2.7)
(import "otter_helpers" "otter_binop"   (func (param i64 i64 i32 i32) (result i64)))
(import "otter_helpers" "otter_convert" (func (param i64 i32 i32) (result i64)))
(import "otter_helpers" "otter_unop"    (func (param i64 i32 i32) (result i64)))
(import "otter_host" "<임베더-등록명>" (func …))               ;; Host1/2/HostCtx1/2 대상, 0개 이상
```

- **memory는 임포트로 유지, table만 소유권 반전** — ESC §3.3-⑤의 "memory/table 임포트 2행 → 선언" 중 테이블만 반전하는 중간 형상이 실측 성립([스파이크] M3: 임포트 memory + 모듈-내 `(table 2 2 funcref)`+element, 매 홉 임포트-메모리 카운터 2,000,001 정확). 근거: M2a에서 선형 메모리의 최종 소유자는 엔진-라이브러리 빌드이고 그 결정은 링커(b″) 몫 — 방출기가 소유를 선점하면 링크 자유도를 잃는다(설계 권고 5의 채택). 임포트 형상은 O-13 §5.2의 보편-슈퍼타입 논증을 그대로 상속.
- **헬퍼 3종은 함수 임포트** — 이름은 Rust fn 이름 축자(`otter_binop`/`otter_convert`/`otter_unop`, `crates/otter/src/lib.rs:201/206/228`의 공개 FFI 헬퍼)를 따르는 **본 문서의 명명 선택**이다. **정정(패널)**: 이들 `pub extern "C" fn`에는 `#[no_mangle]`/`#[export_name]`이 **없어** 심볼이 맹글링된다 [실측: grep] — JIT 모드는 fn 포인터==테이블 등재라 이름이 불요했으므로 이 이름의 export는 오늘 어디에도 실존하지 않는다. 따라서 "M2a 정적 링크 시 엔진-라이브러리 wasm 빌드의 동명 export에 해소"(오너 §5b 결정 A의 M2a 성질: "헬퍼 = 정본 의미론" 구성상 보존 — MODE §2.3-②)는 구성상 자동 성립이 아니라 **b′에 부과되는 신규 export 의무**(export 속성 부여 또는 래퍼 방출)이며 §1.2 b′ 행에 등재했다. `dispatch`/`tail` 임포트는 **없다**(트램폴린 소멸, §3; A-2 중간 형상의 잠정 존속은 §9 규범 참조).
- **호스트 fn은 함수 임포트** — IR이 나르는 fn 포인터는 빌드-타임 네이티브 포인터라 JIT의 항등 리졸버가 성립하지 않으므로(ESC §3.3-④), `compile_module`의 이름 리졸버(§4)가 포인터→임포트명을 공급한다. 원리적 하강 불가 항목의 필연적 임포트 반전(ESC §3.1 표)이며, **호출 형상은 반전하지 않는다**(§3.4 — 임포트는 링키지, 호출은 테이블 간접 유지).
- 이름공간 문자열(`otter_helpers`/`otter_host`)은 본 문서의 설계 선택이다(오너 결정 아님). b″ 도구의 링크 규약(wasm-ld `env` 관례 vs wasm-tools 모듈명 규약)이 개명을 요구할 수 있으나, 매니페스트가 임포트 전량을 기계-가독으로 나르므로 개명은 기계적이다 — §11-1로 유보.

### 2.3 선언 테이블 + element ("otter 함수 인덱스 = 테이블 슬롯")

```
(table T T funcref)   ;; T = nfuncs + 3(헬퍼) + nhosts, min=max 고정
(elem (i32.const 0) func …)  ;; MVP 인코딩: Elements::Functions + Active{table: None}
```

- 슬롯 배치: `[0, nfuncs)` = 방출 함수(슬롯 j ↦ 모듈 함수 인덱스 `nimports + j` — 함수 인덱스 공간은 임포트 선행이므로 element가 이 매핑을 흡수하고, **테이블 슬롯은 otter 인덱스와 항등**), `[nfuncs, nfuncs+3)` = binop/convert/unop 임포트, `[nfuncs+3, T)` = 호스트 임포트. active element 1개, offset 0, 전 슬롯 일괄 등록. **호스트 슬롯 도출 규칙(규범, 패널 보강)**: `otter_host` 임포트 순서 = 리졸버 **최초-해소 순서**(§4.1 결정성 규칙) = 테이블 슬롯 순서이며, k번째 호스트 임포트의 슬롯 = `nfuncs+3+k` — 매니페스트 `host_slots`가 이 규범의 기계-가독 사본을 나른다(§5.2 R\* 주입의 정본 채널).
- min=max 고정 + 전 슬롯 초기화 + **가드 경계 = nfuncs**(§3.2)이므로 미초기화 슬롯 도달 경로 자체가 없다([스파이크] M4 비고: 가드 경계=NFUNCS면 null-슬롯 trap 경로 도달 불가; 설계 권고 3). typed funcref/expression element 불요([스파이크] 캐비앗 5).
- 콜리 런타임 값(레지스터의 raw 함수 인덱스)이 항등 매핑으로 그대로 유효 — 엔진 디스패치가 쓰는 인덱스 의미론과 동일 값 공간이라 IR 무변경(ESC §3.2 [실측]).
- 헬퍼/호스트 슬롯은 가드 경계 **밖**이므로 게스트 콜리 값으로는 도달 불가 — Call/TailCall로 헬퍼·호스트 fn을 직접 부르는 위조가 형상 수준에서 차단된다(부수 보안 성질, 본 문서 설계 선택).

### 2.4 export

```wat
(export "otter_entry" (func $f_entry))   ;; m.entry의 함수, CompiledFn 타입 (i32 i32 i64)->i64
(export "otter_table" (table 0))
```

- `otter_entry`: 임베더/래퍼(`_start` 래퍼는 임베딩 결정 — ESC §3.3-⑦)의 진입점. `ctx` 파라미터는 A-모드에서 **죽은 파라미터**(불투명 전달만; ESC §3.3-② — `(ctx,args,argc)` ABI를 유지해 lowering 델타 최소화). 호출자는 0을 넘긴다(계약 문서화).
- `otter_table`: b″ 링크·조합(스왑류)·개별 함수 호출(매니페스트 슬롯 맵 경유)의 표면. 함수별 export는 방출하지 않는다(표면 최소화 — [minimal-surface] "금도금 금지" 이식, MODE §4.2).
- 스크래치는 export하지 않는다 — 예약 의무는 매니페스트가 나른다(§2.5).

### 2.5 방출기-소유 스크래치 메모리 맵 (`scratch_addr` 파라미터 소멸 — ESC §3.3-③)

- `compile_module`은 함수별 마셜링 스크래치를 **자체 배정**한다: 옵션 `AotOpts.scratch_base`(u32, 8-정렬 필수)에서 시작해 함수 오름차순으로 `8 * scratch_words(f)` 바이트씩 **서로소 구간** 순차 배치. 크기 산정은 현행 `scratch_words`/`site_argc` 단일 소스를 그대로 사용(§2.5-1 무-이중-소스 규칙 상속 — lib.rs `max_scratch_words`).
- **함수별 서로소를 선택**(전-모듈 공유 max-크기 단일 스크래치 대비): 공유안도 "콜리 프롤로그가 즉시 로컬로 선복사" 구조로 안전 논증이 성립하지만([추정] — 마셜링 store와 콜리 프롤로그 load 사이에 개입 코드가 없다는 현행 lowering 형상에 의존), 그 논증은 향후 lowering 변경(예: 마셜링과 호출 사이 헬퍼 호출 삽입)에 침묵으로 깨진다. **주장 정밀화(패널 정정)**: 서로소가 소거하는 것은 **교차-함수** 간섭뿐 — 자기재귀/재진입에선 같은 함수의 활성화들이 같은 scratch_f를 공유하므로 "마셜링 store와 소비 호출 사이에 otter-수준 재진입 가능 호출이 없다"는 불변식은 여전히 load-bearing이다(네이티브 cranelift가 안전한 이유는 활성화별 스택 슬롯 — O-13 §2.5 A#2 계열). 즉 공유-max 대비 파손 클래스가 **진부분집합**이 되는 것이지 논증이 소멸하는 것이 아니다 — 이 불변식을 §5.1 리뷰 체크리스트에 편입(R2 창 핀이 사실상 집행). 공유-스크래치 축소는 perf/size 후속으로 명기.
- 재귀 안전성: 자기재귀는 "프롤로그 args 선복사 후 콜사이트 도달" 구조로 성립(ESC §3.2 [실측]). **Rust 차용 문제는 A-모드에서 원리적으로 소멸** — module_dispatch가 없어 스크래치 위 `&[Value]` 형성 지점이 0이다(O-13 §13-5의 두-복사-창이 공집합이 됨). 잔존 규범은 §2.5의 링커 의무 하나.
- **링커/임베더 의무(계약, 패널 보강으로 4건 완결)**: (i) **겹침 금지**: 매니페스트의 `scratch_region = [scratch_base, end)`를 엔진-라이브러리의 어떤 할당(힙·섀도 스택·데이터)과도 겹치지 않게 **예약**해야 한다(MUST) — 위반 발현은 침묵 메모리 오염(O-12가 최악 클래스로 규정한 형상). (ii) **커버리지**: `scratch_region` 전체가 인스턴스화 시점 메모리 경계 내에 있거나 첫 호출 전 grow되어야 한다(MUST) — 위반 발현은 첫 마셜링 store의 **결정적 wasm trap**(겹침과 발현 모드가 다름 — 두 모드 병기가 b″ 기계 검증 설계의 입력). (iii) **재배치 불가**: 산물은 테이블-슬롯·스크래치 주소가 즉값으로 베이크된 **무재배치 산물**이다 — flat-merge형 링크(테이블 병합/슬롯 이동) 불가, 컴포넌트 instantiation-graph형 정적 결합에 정합(§7b-①). (iv) **공인 재배치 메커니즘 = 재방출**: base 재선정이 필요하면 링커가 새 `scratch_base`로 `compile_module`을 재호출한다 — 방출이 결정적(§5.2)이라 안전하며, "bytes+매니페스트가 링커의 전체 입력"(§1.2) 계약과 정합. 전부 b″ 게이트에서 기계 검증할 것 — O-16c′로 이월하되 의무 자체는 본 계약이 규정.
- 함수별 `ScratchOutOfAddressSpace` 검사는 배정 주소 기준으로 유지(현행 `> 2^32` 경계 로직 재사용). **넘침 조건 정정(패널)**: 올바른 조건은 `base + Σspan > 2^32`(합 단독 아님 — base가 크면 합이 작아도 넘친다)이며, 배정이 func_idx 오름차순 순차라 이 조건은 **첫 위반 함수**에서 `AotError::Func{ScratchOutOfAddressSpace}`로 결정적으로 발화한다 — 별도 모듈-수준 변형은 이 검사에 포섭되어 두지 않는다(§4.2). `scratch_base` 8-정렬 위반은 `AotError::ScratchBaseMisaligned`로 기계 강제(§4.2 — MUST의 무패닉 이행).

### 2.6 data 사용 여부

O-14 방출물은 **data 세그먼트를 방출하지 않는다**. 근거: 스크래치는 read-before-write가 없고(마셜링 store가 항상 선행), consts는 프롤로그 `i64.const`로 베이킹(현행 형상 유지 — lib.rs 프롤로그). data **허용**은 AOT 계약 층위의 규범이며(§2.1 — 장래 링크 산물·O-15 f16 테이블 등을 위한 여지), 방출기 출력이 계약 허용치보다 좁은 것은 정상이다.

### 2.7 shared-memory 게스트(O-17)와의 교차

`AotOpts.shared_memory`가 memory 임포트 형상을 O-17과 동일 규칙으로 전환한다(`(memory 0)` ↔ `(memory 0 65536 shared)` — `EmitOpts::shared_memory`/`SHARED_MEMORY_IMPORT_MAX_PAGES`의 보편-슈퍼타입 논증 그대로 상속). M2a에선 엔진-라이브러리가 `+atomics`로 빌드되는지가 결정 변수이고 그 인지는 빌드 시스템(b′) 몫 — 방출기는 선언만 한다. 단일-에이전트 규범(수동적 링크 호환 L0, 동시 실행 계약 밖)은 O-13 §5.2 v1.1 그대로 유지. [스파이크] 호스트-cfg(48, `shared_memory(true)`+threads)에서 M1–M5 전부 PASS — 단 **재표기(패널 정정)**: 이는 **config-플래그 층위**의 공존 확인일 뿐이다(M1–M5 어느 모듈도 shared 메모리를 임포트하지 않음 — M3는 비공유 `(memory 1)` [실측: modgen]). shared-임포트 × 선언-테이블 × `return_call_indirect` 실물 조합은 **A-5에서 실측 완료** [실측: hosts/otter-aot-e2e AOT48-E2E-4 — `(memory 0 65536 shared)` 임포트를 실물 `SharedMemory`에 링크한 단일 산물에서 2M-깊이 상호 꼬리재귀가 64 KiB wasm 스택으로 완주; AOT48-E2E-2가 같은 형상의 산술/호스트/비-꼬리 Call 경로를 오라클-동일로 확인].

---

## §3. 호출 lowering

### 3.1 `Call` — 가드 + `call_indirect` (테이블 간접이 기본)

콜리는 레지스터의 **런타임 값**이므로 테이블 간접이 기본이다(ESC §3.2). 상수-콜리 직접 `call` 최적화는 비목표(§1.2) — 근거: (i) IR에 상수-콜리 판별 채널이 없고(콜리는 항상 `l(base)` 로컬), (ii) record==live 규율(§5)의 창 형상이 사이트마다 갈라져 게이트 비용이 즉증, (iii) perf 이득은 미실측.

```
;; 마셜링 (현행 emit_dispatch_call 1단계와 동일 — site_argc 단일 소스, i64.store offset=8k)
local.get l(base) ; i64.const NFUNCS ; i64.ge_u
if (result i64)
  i64.const FILL                                  ;; OOB/음수 콜리 → fill이 호출 결과
else
  local.get $ctx ; i32.const scratch_addr ; i64.const n
  local.get l(base) ; i32.wrap_i64
  call_indirect (type COMPILED_TY)                 ;; (i32 i32 i64)->i64, 테이블 0
end
local.set l(dest)
```

- **fail-closed 의미론(오너 §5b C-③ 비준의 AOT 형상 확정)**: OOB/음수 콜리는 trap이 아니라 **fill이 그 호출의 결과값**이 되어 `l(dest)`에 저장 — 네이티브/JIT 엔진 dispatch의 OOB→fill 폴백(O-13 W-3 "하네스 OOB-콜리 fill 폴백 미러")과 관측 동등. `i64.ge_u` 하나가 음수 워드를 흡수한다([스파이크] M4: 음수(-3)는 비가드 시 거대 인덱스로 `TableOutOfBounds`였고, 가드판은 OOB(5)/음수(-3)/null-슬롯(2) 전부 trap 없이 FILL(-9999) 반환; 부호 검사 별도 불요 — 설계 권고 2).
- **관측-동등의 정밀화(패널 must-fix 정정)**: 위 동등은 **네이티브 엔진(usize=64)·64-bit 하네스에서 실측**된 것이다. wasm32-배포 JIT 제품/T0의 콜리 변환은 범위검사 **전**에 `as usize` 하위 32비트 절단을 수행하므로(`module_dispatch` otter-engine:1153, `tail_signal` :1283, T0 interp otter:937 [실측]), x≥2³² ∧ x mod 2³² < nfuncs인 콜리(예: −2³² → 하위 32비트 0 → f0 호출)에서 본 전폭 가드(fill)와 발산한다 — M4의 −3은 하위 비트가 커서 우연히 일치했을 뿐이며, "ge_u 하나가 음수 흡수"는 전폭 실행체 한정 사실이다. 해소는 §3.2a co-requisite: **전폭 가드가 정본이고 절단 경로가 결함**이다(오너 §5b C-③ fail-closed 모드 간 통일 비준 정합 — 카브아웃 대안은 비준 취지와 상충하여 기각).
- Call의 가드는 TailCall의 가드-후-return과 달리 `if (result i64)/else` 형태여야 한다(가드의 `return`은 전체 함수 반환이 되어 오답). `BlockType::Result(I64)`는 신규 **블록타입 형상**이지 신규 명령이 아니다(census 무증가; 정직 기록). 이 형상은 스펙 표준이나 스파이크 M4는 가드-후-return형과 비가드형만 실행 — [추정→A-3에서 실측화](패널 표기 권고).
- 인자 개수 `n`은 컴파일타임 상수(`site_argc`), 콜리 프롤로그의 argc-가드가 부재 인자 fill 유지/잉여 인자 무시를 현행과 동일하게 처리 — 관측 동등 유지.

### 3.2 `TailCall` — 가드 + `return_call_indirect` ([스파이크] M4 시퀀스 축자)

```
;; 마셜링 동일
local.get l(base) ; i64.const NFUNCS ; i64.ge_u
if ; i64.const FILL ; return ; end                 ;; fail-closed: fill이 꼬리 결과로 즉시 반환
local.get $ctx ; i32.const scratch_addr ; i64.const n
local.get l(base) ; i32.wrap_i64
return_call_indirect (type COMPILED_TY)
```

- 가드의 `return`이 앞서 탈출하므로 terminator 제약과 충돌 없음 — 검증·실행 양 환경 성립 [스파이크] M4. 트램폴린 소멸의 정당성: "콜리가 T0일 수 있다"는 현행 `return_call` 미사용의 유일 근거가 A-모드에선 전제째 소멸(ESC §3.2). 유계성은 [스파이크] M2가 실물 확정(2M 깊이, 64 KiB 스택, fuel 공존 무문제).
- OOB 꼬리콜리의 fill 반환은 JIT 트램폴린이 OOB 콜리에서 fill을 산출하는 것과 관측 동등(모드 간 통일 — 오너 §5b C-③).
- wasmtime의 trap 종별 구분(`IndirectCallToNull` vs `TableOutOfBounds`)은 가드 채택으로 산물 표면에서 사라지므로 어떤 계약도 이에 의존하지 않는다([스파이크] 캐비앗 4).

### 3.2a 엔진 측 전폭 범위검사 co-requisite (패널 must-fix — W-0.5 Neg(F16) 선례 동형)

`module_dispatch`/`tail_signal`/T0 interp의 콜리 캐스트(`idx as usize`) **앞**에 i64 전폭 범위검사를 신설한다(u64로 `idx < 한계` 검사 후 캐스트 — §3.1 정밀화의 해소 지점). 네이티브(usize=64)에선 동작 무변경(기존 `in_range`가 이미 전폭 도달)이고, wasm32 빌드의 잠복 절단-분기(O-13이 남긴 native↔wasm32 티어 간 사전 분기)를 동시 해소한다 — 이로써 §6의 관측-동등 계약이 배포 폭 전체에서 성립한다. otter-wasmemit 밖(`crates/otter`/`crates/otter-engine`) 변경이므로 §9의 독립 co-requisite 증분 **A-C1**으로 착지하며 A-3(가드 반전)의 선행 게이트다. 게이트: 전폭 OOB 콜리(2³²+k with k<nfuncs, −2³²) 값-수준 회귀를 T0/네이티브 엔진 테스트에 추가(64-bit 실행체에서도 "검사가 캐스트에 선행"함을 단언 가능) + §6 차등 코퍼스에 전폭 OOB 케이스 필수 포함.

### 3.3 방출 명령 census 증가분

`{I64GeU, I32WrapI64, ReturnCallIndirect}` 3종([스파이크] 설계 권고 2 — If/Return/I64Const는 기존 census에 존재, 기존 census의 유일 비교 op는 I64GtS라 I64GeU가 신규). EH 명령 0 유지 — **A-모드 산물은 exnref/§7 nightly 핀 체계에서 완전 탈출**(ESC §3.4 census [실측]; 단 M2a 최종 산물의 EH-프리 여부는 엔진-라이브러리 빌드(b′)에 달림 — O-16c′ 몫). **[O-16c′ L-4/L-6 갱신 — 유보 해소]**: b′ 실물(hosts/otter-helpers-wasm, stable 1.96 + panic=abort) 합류로 **M2a 최종 산물 단위의 EH-프리가 [실측]** — 결합물·export-축소물·컴포넌트 전건이 `wasm-tools validate --features=all,-exceptions,-legacy-exceptions` + EH census 0을 통과하고(`just wasm-aot-link-e2e`/`wasm-aot-component` 상시 게이트), 거부 방향은 L-6 뮤테이션 ⑧(try_table 주입 → 적색)이 실증. nightly-핀 탈출 주장은 최종-산물 단위로 승격 완료(O-16c′ 설계 §5.5/§10).

### 3.4 `Host*`·`HostCtx*` 및 헬퍼 — "임포트는 링키지, 호출은 테이블 간접"

**결정**: Host1/2/HostCtx1/2와 binop/convert/unop 헬퍼의 호출 형상은 현행 그대로 `i32.const <슬롯>; call_indirect`를 유지하고, 대상 함수만 **임포트로 조달해 element로 자기 테이블에 등재**한다(§2.3). 근거:
1. **record==live 보존(§5)**: 직접 `call <임포트 인덱스>`로 반전하면 헬퍼/호스트 호출마다 바디 바이트가 JIT 방출과 갈라져 바이트-동일 게이트가 헬퍼-사용 함수 전체에서 죽는다. 테이블 간접 유지 시 갈라지는 것은 Call/TailCall 사이트 + T-정규화가 흡수하는 type_index 재배치뿐(§5.2, 패널 정정 반영).
2. **M2a 정합(ESC §3.3-④의 해소)**: ESC의 "임포트명 반전"은 엔진-리스 전제의 서술 — M2a에선 임포트가 정적 링크로 엔진-라이브러리 export에 해소되므로(오너 §5b 결정 A) "무엇을 부르나"는 임포트가, "어떻게 부르나"는 테이블이 담당하는 분리가 가능하고, 이 분리가 단일-lowering 규율(MODE §3 3안 합의 "방출기 분리 금지")을 지킨다.
3. element에 임포트 함수 인덱스 등재는 코어 wasm 합법([실측] — 함수 인덱스 공간은 임포트 포함이며 [스파이크] M5의 element 검증이 같은 인코딩 계열; 임포트-등재 형상 자체의 실물 확인은 §9 A-2 게이트에 포함 [추정→A-2에서 실측화]).

`host_ctx`는 현행대로 `i64.const` 베이킹(HostCtx1/2 선행 인자, opaque `ctx`와 혼용 금지 규율 불변). 정수 Div는 현행 Refusal이 그대로 `AotError`로 승격(ESC §3.1 — T0 폴백 부재).

### 3.5 잔존/소멸 대차 (ESC §3.2 업무 분해의 이행 확인)

소멸: 티어업 카운팅·best_code·tail_pending 트램폴린·catch_unwind 격리·복사-즉시 변형·트랩 채널·`otter_jit.compile` 계약 전체·`--export-table --growable-table` 링크 요건(ESC §3.4). 잔존: `fill` 규약·argc-가드 프롤로그·로컬 fill 초기화·consts 베이킹(전부 현행 프롤로그 바이트 그대로 — §5의 동일성 대상). 형태 변경: 콜리 범위검사(명시 가드 — §3.1/3.2).

---

## §4. API

### 4.1 `compile_module` 시그니처

```rust
/// 전-모듈 빌드-타임 방출 (O-14). otter-cranelift `compile_module(isa, m)`의 wasm판 —
/// CallMode::Table 1:1 대응(모드≠티어, MODE §3 합의).
pub fn compile_module(
    isa: &Isa,
    m: &otter::Module,                               // funcs/fill/host_ctx/entry 소유 (cranelift 동형)
    resolve_host: &dyn Fn(usize) -> Option<HostImportName>, // 호스트 fn 포인터 → 임포트명 (§2.2)
    opts: &AotOpts,
) -> Result<AotArtifact, AotError>

#[non_exhaustive] pub struct AotOpts { pub scratch_base: u32, pub shared_memory: bool }
// AotOpts::new(scratch_base) + with_* 빌더 — EmitOpts와 동일 전방호환 자세. scratch_base에
// 디폴트를 두지 않는 이유: 주소 0 근방은 엔진-라이브러리 데이터가 점유할 개연이 높아
// "안전한 디폴트"가 존재하지 않는다 — 명시 강제가 §2.5 예약 의무를 호출자에게 상기시킨다.

pub struct AotArtifact { pub bytes: Vec<u8>, pub manifest: AotManifest }

/// no_std+alloc — 호스트 임포트명 newtype (alloc::string::String; 패널 N4 확정)
pub struct HostImportName(pub String);

#[non_exhaustive] pub struct AotImport {
    pub module: String, pub name: String,
    pub kind: AotImportKind,   // Memory | Table(A-1 잠정, A-2에서 소멸) | Helper(HelperKind) | Host { arity: u8, ctx: bool }
    //         ^^ A-1 착지 정정: (a) `Helper(HelperIdx)`는 실존 pub `HelperIdx`(5-슬롯 구조체)와
    //            이름 충돌·의미 불일치라 판별자 enum `HelperKind`로 명명 적응; (b) A-1 산물은 아직
    //            `otter.table`을 임포트하므로 매니페스트=산물-기술 원칙상 `Table` 변형이 필요하다
    //            (A-2의 선언 테이블 전환과 함께 소멸).
    // 함수 타입(HOST1/HOST2/HOSTCTX1/2 상이)의 **정본은 산물 type 섹션(wasm 바이트)** —
    // kind는 b′ export 명세용 편의 사본이며 불일치 시 bytes가 우선한다(패널 보강).
}

#[non_exhaustive] pub struct AotManifest {
    pub entry_slot: u32,                     // = m.entry (항등 매핑)
    pub nfuncs: u32,                         // 가드 경계 (§3)
    pub helper_slots: [u32; 3],              // binop/convert/unop 테이블 슬롯
    pub host_slots: Vec<u32>,                // k번째 호스트 임포트의 테이블 슬롯 = nfuncs+3+k —
                                             // §2.3 도출 규범의 기계-가독 사본, §5.2 R* 주입의 정본 채널(패널 보강)
    pub imports: Vec<AotImport>,             // 전량 — b′ export 요구 명세
    pub scratch: Vec<AotScratchEntry>,       // func_idx → (addr, words)
    pub scratch_region: (u32, u64),          // [base, end) — 링커 예약 의무 (§2.5)
                                             // A-1 착지 정정: end만 u64. §4.6 넘침 조건이 `>` 2³²라
                                             // `base+Σspan == 2³²`가 합법인데 u32로는 표현 불가 —
                                             // 절단하면 예약 의무를 1바이트 과소보고(§2.5가 막으려는
                                             // 침묵-오염 클래스)한다. 경계 케이스는 A-1 테스트가 실증.
    pub table_size: u32,
    pub shared_memory: bool,
    pub emitter_version: &'static str,       // env!("CARGO_PKG_VERSION") — 산물↔엔진-라이브러리
                                             // 세대-대조(빌드-해시 fail-closed 규율)의 최소 앵커(패널 보강)
}
```

- 입력을 `otter::Module`로 받는 것은 cranelift `compile_module(isa: &Isa, m: &OtterModule)`(레포 [실측] — fill/host_ctx/entry를 모듈이 나름)과의 API 패리티. 함수별 8-파라미터 나열의 모듈판 증식을 원천 회피.
- `resolve_host`가 `Option<u32>`(JIT 리졸버)가 아니라 이름을 반환하는 이유: 빌드-타임 네이티브 포인터는 인덱스화 불가(§2.2). `None` ⇒ `AotError::Func{ refusal: HostFnUnresolved }`.
- **리졸버 결정성(규범, 패널 보강)**: `resolve_host`는 서로 다른 호스트 fn 포인터당 **정확히 1회**, func_idx 오름차순·바디 내 사이트 순서의 최초-사용 시점에 호출되고 결과는 캐시된다(같은 포인터 재해소 금지) — 비순수 리졸버라도 임포트 목록·이름·슬롯 배정이 결정적이 되어 §2.3 슬롯 도출과 §5.2 게이트 재현성 양쪽을 지탱한다. 서로 다른 포인터가 같은 이름으로 해소되면 `DuplicateHostImportName`(검출은 dup 발생 사이트의 함수 검사 중 — 단일-패스 순서에 자연 정렬, §4.2).
- 매니페스트는 v1에서 **Rust 값**이다 — 직렬화 포맷은 소비자(b″)가 정해질 때(§11-2). **명시 이연(패널 상신의 부분 수용)**: MODE §4.2 [hybrid-first] 이식 요소의 나머지 반쪽인 "매니페스트 빌드-해시 fail-closed"(산물↔엔진-라이브러리 세대 불일치를 b″가 감지)는 대조 상대(b′ 산물)가 실존하기 전엔 해시 대상을 규정할 수 없어 O-16c′로 이연한다 — `emitter_version` 필드가 그 최소 앵커로 지금 들어간다.

### 4.2 `AotError` 총체화 (오너 §5b C-② 비준의 이행)

```rust
#[non_exhaustive]
pub enum AotError {
    /// 입력 계약 위반 — 함수 검사에 앞선 최우선 검사 (§2.5 8-정렬 MUST의 기계 강제; 패널 보강)
    ScratchBaseMisaligned { base: u32 },              // scratch_base % 8 != 0
    /// 함수 단위 실패 — 기존 Refusal을 값째 승격. 결정적 순서: func_idx 오름차순의 첫 실패.
    /// 스크래치 주소공간 넘침(base+Σspan > 2^32)은 순차 배정의 첫 위반 함수에서
    /// Func{ScratchOutOfAddressSpace}로 포섭된다 — 별도 모듈-수준 변형 없음(§2.5 정정).
    Func { func_idx: usize, refusal: Refusal },
    /// 모듈 단위 실패 (함수 검사 전부 통과 후 검사):
    EntryOutOfRange { entry: usize, nfuncs: usize },
    DuplicateHostImportName { name: HostImportName, first_func_idx: usize, dup_func_idx: usize },
    /// A-1 착지 추가 — anti-wrap 총체성 가드(`TooManyLeaders` 동형). 테이블 슬롯 수
    /// `nfuncs + 3 + nhosts`가 u32를 넘으면 슬롯 즉값이 침묵 wrap 캐스트 = 미스컴파일이 된다.
    /// 실제 도달에는 ≈2³² 함수/포인터가 필요해 사실상 테스트 불가하지만, "`Ok` ⇒ 검증-통과 모듈"
    /// 계약을 확률적이 아니라 전역적으로 유지하기 위해 존재한다(§4.6 `TooManyLeaders` 선례 축자).
    TooManyTableSlots { needed: u64 },
}
```

- **무패닉 계약 상속**: `compile_module`은 어떤 입력에도 패닉하지 않는다(well-formed `Isa` 전제는 전 티어 공통 신뢰 경계 그대로). `Err`는 절대 반쪽 모듈을 남기지 않는다(현행 `compile_dispatch_fn` 계약의 승격).
- 함수별 검사는 현행 파이프라인 축자 재사용: `check_func` → §4.6 구조 사전검사(1-5 전부 + TooManyLeaders/Scratch 검사) — 신규 사전검사 0. 즉 **AOT의 함수-수용 술어 = JIT의 것과 동일 코드**이며(단 (리졸버, 스크래치 기하)로 매개변수화 — §6 한정 조건), 이것이 §6 부분성-일관성의 구성적 절반이다.
- **단일-패스 결정성(규범, 패널 보강)**: 함수별 **전체** 수용 술어(`check_func` → §4.6 사전검사 → `resolve_host` → 스크래치 순차 배정·경계 검사)를 func_idx 오름차순 **단일 패스**로 완주하고 첫 실패에서 즉시 반환한다. 크기 선-스캔 후 방출하는 2-패스 구현은 **금지** — 패스 간 실패 순서가 갈라져(선-스캔이 f5의 ValidationFailed를 f2의 IntegerDiv보다 먼저 봄) "결정적 첫 실패" 약속을 깬다. 서로소 순차 배정은 선-스캔 없이 성립한다(함수 f의 base = 앞 함수들의 누적으로 즉시 확정).

### 4.3 `EmitOpts`와의 관계, semver·non_exhaustive 규율

- `compile_dispatch_fn`/`compile_dispatch_fn_with`/`EmitOpts`/`Refusal`/`HelperIdx`/`scratch_words`는 **무변경**(구조 테스트의 바이트 핀 유지). `compile_module`은 내부적으로 동일 바디 방출기를 호출-lowering 모드 매개변수(내부 enum, §5.1)로 공유하고, 임포트 조립만 자체 수행. `EmitOpts::shared_memory`의 의미론을 `AotOpts.shared_memory`가 그대로 상속(형상 코드 공유).
- semver: otter-wasmemit은 path-dep pre-1.0(otter-reusability-plan의 블로커 목록 관할) — 본 증분은 **additive-only**(신규 pub 아이템만, 기존 시그니처 0 변경). `AotOpts`/`AotManifest`/`AotError` 전부 `#[non_exhaustive]`(O-2 규율 상속). `Refusal`을 `AotError::Func`가 값째 재사용하므로 Refusal 변형 추가는 AOT 표면에 자동 전파(이중 열거 금지).

---

## §5. record==live 규율 (이중 정본 봉쇄 — MODE §4.2 [hybrid-first] 이식)

### 5.1 구조 규율 (리뷰 게이트)

`compile_module`은 **별도 방출기를 갖지 않는다**: `emit_straightline_body`/`emit_dispatcher_body`/`lower_expr`/`emit_dispatch_call`을 내부 호출-lowering 모드(`Dispatch`(현행) vs `Table{nfuncs}`(신설)) 매개변수로 공유한다 — otter-cranelift `CallMode::Dispatch/Table` 선례의 1:1 이식(레포 [실측]: cranelift는 이미 단일 `lower_module_func`에 CallMode를 꽂는다). 모드 스위치가 갈라도 되는 지점은 **마셜링-후 호출 시퀀스 + 모듈-전역 타입 인터닝이 낳는 type_index 재배치**(§5.2 T-정규화가 흡수)뿐이다(패널 정정 — "호출 시퀀스만"은 부정확했음). 이 규율 위반(제2 방출 경로 신설)은 코드 리뷰 차단 사유로 명문화한다. **리뷰 체크리스트(§9 A-5 적대적 리뷰와 공유)**: ① 제2 방출 경로 부재, ② 창 정의 확장 커밋 여부(§10-3), ③ "마셜링 store와 소비 호출 사이 otter-수준 재진입 가능 호출 없음" 불변식 유지(§2.5 잔존 불변식 — R2 창 핀이 사실상 집행), ④ 귀속 정확성.

### 5.2 바이트 게이트 (CI) — **T-정규화 바이트 동일** (패널 must-fix 재정의)

정본 정의: 함수 j의 **live 바디** = `compile_dispatch_fn(isa, j, f, fill, host_ctx, H*, R*, S*)`의 code-섹션 바디, 여기서 canonical 주입 H\* = `HelperIdx{binop/convert/unop: 매니페스트 helper_slots, dispatch: 0xFFFF_FFFE, tail: 0xFFFF_FFFF}`(**센티널 고정값** — 창 바이트 결정성의 요건, 패널 보강; 두 값 모두 가드 경계 밖이고 서로 다른 바이트로 인코딩되어 상호 구별된다. *A-1 착지 정정: 초안의 "5-byte LEB"는 오기 — wasm-encoder는 최소 SLEB를 쓰므로 두 센티널은 각각 1바이트(0x7E/0x7F)로 인코딩된다. 결정성·상호구별은 그대로 성립하나, A-3의 R2 창 정의가 폭 5를 전제하면 재심 대상이다*), R\* = 매니페스트 `host_slots`(§2.3 도출 규범의 사본), S\* = 매니페스트 `scratch`. **record 바디** = `compile_module` 산물에서 wasmparser(기존 dev-dep)로 절단한 함수 j 바디.

**T-정규화 (게이트의 전제 — 패널 must-fix)**: live(함수당 모듈)의 타입 인터닝은 그 함수 바디의 첫-사용 순서(COMPILED_TY=0 선점 후 on-demand)를, record(N-in-1)는 임포트 조립이 선행하는 모듈-전역 누적 순서를 따르므로 `call_indirect`/`return_call_indirect`의 type_index 즉값은 **일반적으로 갈라진다** — 함수별 첫-사용 순서는 상호 배타적이라(f0: binop-먼저 vs f1: convert-먼저) 어떤 전역 순서를 골라도 방출기 측 회피가 원리적으로 불가하고, live 측 선-인터닝은 §4.3 바이트-핀 계약과 충돌한다. 따라서 정규화는 **게이트 하네스 몫**이다(양 방출기 동결 유지): 하네스가 양 산물의 type 섹션을 읽어 각 사이트의 즉값을 **구조적 타입 키로 재사상**한 뒤, "사이트별 구조 동등 + 그 외 바이트 동일"을 기계 단언한다(wasmparser로 검사 가능; 관여 타입 전종 ≤8이라 즉값 전부 1-byte LEB → 재사상은 길이-불변이고 diff 오프셋을 흔들지 않는다). record의 인덱스가 선행 함수 이력에 의존해도 구조 키 비교라 재현성 문제가 없다(순환 없음).

- **게이트 R1 (전문 T-동일)**: Call/TailCall 0개인 함수 — record 바디 == live 바디, **T-정규화 바이트 동일**. (프롤로그·디스패처 루프·Bin/Un/Host* 사이트 전부 포함 — §3.4의 테이블-간접 유지가 이를 성립시킨다.)
- **게이트 R2 (창 국소화, 창 정의 완결 — 패널 보강)**: Call/TailCall 포함 함수 — T-정규화 후 두 바디의 diff가 **마셜링-후 호출 창**에만 국한됨을 창-대조로 검증.
  - **Call 창**: Dispatch 측 `local.get ctx; local.get base; i32.const S; i64.const n; i32.const 0xFFFF_FFFE; call_indirect DISPATCH_TY` ↔ Table 측 §3.1 시퀀스. 공통 접미 `local.set l(dest)`는 **창 밖**(양 모드 동일 바이트 — lib.rs Call 방출 형상 [실측]).
  - **TailCall 창**: Dispatch 측은 `call_indirect`(센티널 0xFFFF_FFFF) 뒤 **에필로그 `drop; i64.const FILL; return`까지 포함**(lib.rs :797-803/:948-954 방출 형상 [실측] — 에필로그가 Table 측 §3.2로 통째 대체되므로 창에 속한다) ↔ Table 측 §3.2 시퀀스.
  - 마셜링 store 열은 양 모드 동일하므로 diff 시작점이 결정적이다. `DISPATCH_TY`는 live에만 존재하는 타입이나 창 내 즉값이라 대조 대상이 아니며, 타입 섹션 차이는 T-정규화가 소화한다.
- **게이트 R3 (뮤테이션 증거)**: 비-호출 lowering을 Table 모드에서만 변조하는 뮤테이션이 R1/R2를 적색으로 만드는지 1회 실증·기록(§8.3).
- 정직 기록: 전-함수 전문 바이트 동일은 **불가능**하다 — 트램폴린 소멸(O-14의 목적)이 호출 사이트를 필연적으로 가르고, 모듈-전역 인터닝이 type_index를 재배치한다(후자는 T-정규화가 흡수하는 **정의된 발산**). [hybrid-first]의 원형(H-모드, 엔진 동반이라 전문 동일)과 다른 이 완화는 A-모드의 구조적 귀결이며, R1+R2+구조 규율(§5.1)의 합이 "제2의 손 semantics 없음"을 봉쇄한다.

---

## §6. AOT⊂JIT 부분성-일관성 계약 (MODE §4.2 [consumer-first] 이식)

계약 문구(정본): **AOT가 수용한 프로그램은 JIT/T0와 관측 동등하고, AOT의 거부는 `AotError`로 명시적이며, AOT의 수용 집합은 JIT의 수용 집합에 포함된다.** **전제(매개변수 한정 — 패널 정정)**: 함수-수용 술어는 동일 코드이나 (리졸버, 스크래치 기하)로 매개변수화되므로, 포함·동등 주장은 §5.2 canonical 주입 + **리졸버 도메인 포함**(AOT 리졸버가 이름을 주는 호스트 fn 집합 ⊆ JIT 리졸버가 인덱스를 주는 집합) 하에서 성립한다 — 무조건적 "코드상 불가능"은 과잉 주장이었다.

- **포함(⊂)은 구성적**(위 전제 하): 함수-수용 술어가 동일 코드(§4.2)이므로 전제 하에서 "JIT-거부인데 AOT-수용"은 발생하지 않는다 — 게이트는 회귀 방지로 fuzz 코퍼스에서 이 방향을 단언하고(비공허성은 §8.3-⑦ 뮤테이션이 실증), 하네스는 단일 호스트-등록 테이블에서 JIT 리졸버(ptr→인덱스)와 AOT 리졸버(ptr→이름)를 **쌍으로 파생**해 도메인 포함을 구성으로 고정한다. **역방향(JIT-수용인데 AOT-거부)의 원천은 5종**(패널 정정 — "모듈-수준으로만"은 거짓이었음): 모듈-수준 3종(`ScratchBaseMisaligned`/`EntryOutOfRange`/`DuplicateHostImportName`) + 함수-수준 2종(`HostFnUnresolved` — 리졸버 인자 차이, `ScratchOutOfAddressSpace` — AOT 자체-배정 주소 파라미터화) — 전부 명시적 `AotError`다.
- **비교 우주(스코프 명기 — 패널 보강)**: push_func는 제외한다 — A-모드 산물엔 구조적으로 부재(§10-8)하고, JIT `in_range`는 pushed 세그먼트를 포함해 인덱스 공간 자체가 AOT 가드 경계(nfuncs)와 다르다.
- **배포 폭(§3.2a 연동)**: 관측-동등은 A-C1 co-requisite 착지 후 wasm32-배포 JIT/T0 포함 전 폭에서 성립한다 — 그 전까지는 64-bit 실행체 한정 실측(§3.1 정밀화). 차등 코퍼스에 전폭 OOB 콜리 케이스(2³²+k, −2³²) 포함 필수.
- **관측 동등 차등 게이트**: 코퍼스+fuzz의 AOT-수용 모듈마다 3-방식 실행 비교 — (i) T0 오라클, (ii) JIT-형 함수별 방출 + 기존 mock-테이블 하네스, (iii) AOT 전-모듈 인스턴스. 비트 동일 + fail-closed 형상(OOB/음수 콜리 → fill) 동등까지 포함.
- **카브아웃(기존 의미론, 신규 아님)**: 중첩-프레임 `Halt`는 run_module만 모듈-종료로 취급하고 전 컴파일 티어는 Return과 동일(O-13 §8(a) 오라클 스코프) — AOT도 컴파일-티어 측이다. 차등은 O-13과 동일하게 중첩-Halt 프로그램을 오라클 비교에서 제외하거나 티어-대-티어로 구성.

---

## §7. '동적 코드 격리·극히 제한' 규범 명문화 (오너 §5b 결정 B의 이행)

오너 원문(§5b-B 인용): "동적 코드 실행은 어떤 식으로든 격리된 환경을 통해서만, 극히 제한적으로 지원."

1. **A-모드 산물에는 동적 코드 경로가 원리적으로 없다**: `otter_jit.compile` 임포트 0, start 섹션 0, 방출 코드는 전량 빌드-타임 확정, 산물 자체에 코드-생성 능력 0(순수 lowering의 산출물일 뿐). `compile_module`은 **빌드-타임 도구**이며 A-모드 산물의 런타임에서 도달 가능한 어떤 표면도 아니다.
2. **격리 규범의 적용 경계**: 이 규범이 구속하는 것은 A-모드 산물의 배포 맥락(특히 CM/컴포넌트)에서 eval류·런타임 스크립트가 필요해지는 **미래 지점**이다 — 그 경우 컴포넌트 자체 샌드박스 안의 T0 인터프리터 실행 또는 호스트 격리가 있는 사이드채널 경유 + 극히 제한된 스코프로만 설계하며, 자유형 런타임 코드젠을 CM 맥락에 심는 방향은 금지된다(오너 §5b-B 운영화 문구 축자). 그 설계 자체는 O-16 소관 — O-14는 "산물이 이 규범을 위반할 수단을 갖지 않음"을 형상으로 보장하고(위 1), 산물 검증기(§2.1)가 start-부재·화이트리스트를 기계 확인한다.
3. J-모드(JIT)의 런타임 방출에 대해 이 규범은 **이미 충족**되어 있다(패널 자구 방어화 — "대상이 아니다"가 아님): 오너 일반 규범("동적 코드 실행은 격리 환경 경유로만")을 O-13 계약(§5.1-§5.3의 샌드박스 격리·호스트-중개 compile·화이트리스트·add-only 규범)이 이미 이행하는 별도 제품이다(MODE §4.1 2제품 골격) — J-모드를 규범 밖으로 두는 해석은 금지.

---

## §7b. 선행기술 (오너 §4b-⑤ 비준의 이행 — alt-runtimes §4-권고5 / §5-5; 패널 must-fix로 신설)

alt-runtimes §4b 오너 결정(2026-09-03)이 권고 5를 전량 수용하며 집행 형태를 "O-14 설계 문서의 선행기술 절"로 못박았다. 3요소 각각의 처분:

1. **WAMR XIP (execute-in-place) — 채택(논거 인용)**: XIP의 "무재배치 AOT = 함수 포인터 테이블 간접호출 + 인트린식→런타임 함수 치환"은 본 설계의 §2.3(선언 테이블 간접) + §3.4(헬퍼/호스트 임포트 치환, 호출은 테이블 간접 유지)와 실질 동형이다 [실측: xip.md] — 독립 도달한 동일 해법이 선행 사례로 실증됨. 재배치 결정: 본 산물은 **무재배치**(테이블 슬롯·스크래치 주소 즉값 베이크, §2.5-iii)이며 재배치 수요는 XIP식 런타임 패치가 아니라 **재방출**(§2.5-iv — 결정적 방출이라 안전)로 해소한다. §10-2(예약 실패)의 global-간접 폴백 의존도가 이 결정으로 하락.
2. **WasmEdge universal-binary (커스텀 섹션 패키징+버저닝) — v1 기각(사유 명기)**: 본 산물은 custom 섹션 없음·매니페스트=Rust 값(§2.1/§4.1)으로 universal-binary의 "산출물 내 패키징+버저닝"과 정면 배치된다. 기각 사유: 소비자(b″)의 입력 형식이 미정인 시점에 직렬화·패키징 포맷을 선점하는 것은 [minimal-surface] 금도금 금지 위반이고, 세대-대조의 상대편(b′ 산물)도 부재하다. 이연처: §11-2(직렬화 포맷)·§4.1 빌드-해시 이연 — O-16c′ 착수 시 universal-binary의 버저닝 설계를 그 시점의 선행 사례로 재인용할 것. `emitter_version` 필드(§4.1)가 버저닝 축의 최소 앵커로 지금 반영됨.
3. **run-mode 안전 게이팅("기본 인터프리터·AOT 명시 opt-in") — 비적용(판정 기록)**: A-모드 산물에는 실행-모드 선택 자체가 없다 — 코드는 전량 빌드-타임 확정이고 런타임 폴백 슬롯이 부재하다(§1.1-4). 해당 안전 축은 우리 스택에선 J/A 2제품 분리(MODE §4.1)와 §7 격리 규범이 담당한다.

**alt-runtimes §5-5의 결정 입력 3항 소화**: wasm 출력 vs 네이티브 출력 = **wasm 출력 확정**(본 설계 전체; wamr 축의 Tier-C bare-metal 잔존 가치 시나리오가 유효해짐), 재배치 유무 = **무재배치 + 재방출**(위 1), run-mode 게이팅 = **비적용**(위 3).

---

## §8. 테스트 전략

### 8.1 차등 (하네스 — wasmtime 45, 기존 핀)

- **Config 델타 0으로 재사용**([스파이크] M1/설계 권고 1): tail_call은 45(config.rs:2413 `features |= TAIL_CALL`)·48(`WasmFeatures::WASM3`, config.rs:2532) 모두 기본 ON — `wasm_tail_call()` 호출 불요, 기존 fuel-metering 형상(consume_fuel+set_fuel)과 `return_call_indirect` 공존 무문제(2M 홉 정상 과금 실측). 단 **전략=cranelift 전제**(Winch는 tail-call 미지원 — [스파이크] 캐비앗 2)를 하네스·엔진-캡 사전검사 문서 양쪽에 명기.
- 3-방식 차등(§6) + record==live R1/R2(§5.2) + 2-모듈 인스턴스화(헬퍼-제공 스텁 모듈이 `otter_helpers.*`/`otter_host.*`를 export — b′ 부재 상태에서 임포트 표면의 해소 가능성을 실증하는 O-14 몫의 최소 형태).
- fail-closed 3형상: OOB(nfuncs 이상)/음수 워드/in-range 정상 — [스파이크] M4 재현을 회귀로 고정. null-슬롯은 가드 경계=nfuncs로 도달 불가이므로 "도달 불가" 자체를 단언(테이블 과할당 변조와 짝).

### 8.2 1M+ 꼬리재귀 유계 / E2E (호스트 48 cfg) — **A-5 착지: hosts/otter-aot-e2e / `just wasm-aot-e2e`**

- 상호재귀 2M 깊이 + **64 KiB wasm 스택** 완주 + 음성 대조(plain `call_indirect`판 `Trap::StackOverflow`) — [스파이크] M2의 회귀화. O-8 결정적 게이트 혈통의 AOT판. *(45-하네스판은 A-3의 aot.rs 영구 테스트; 48판은 A-5 E2E의 AOT48-E2E-3/-5 — 음성 대조는 공유 형상에서 실행해 AOT48-E2E-4와 tail-vs-nontail 단일-축 대조를 이룬다.)*
- E2E: 호스트-cfg(48: `runtime,cranelift,gc,gc-drc,std,threads` + `wasm_exceptions(true)`+`shared_memory(true)`)에서 인스턴스화 + `otter_entry` 실행 + shared_memory=true 형상 링크. 산물이 EH-프리이므로 EH 설정은 산물엔 불요하나 호스트 형상 그대로 검증(실배포 형상 정합). *(A-5: 엔트리 호출은 §12.4 v1 정본 `(ctx=0, args=0, argc=0)` 그대로; 임포트 표면 전종 — memory + 헬퍼 3종 + otter_host 2종 — 을 오라클-동일로 확인. 마커 5종 + ALL-OK grep 게이트, CI wasm-eh-pinned 레인.)*

### 8.3 뮤테이션 후보 (게이트가 실제로 무는지의 증거)

① 가드 제거 → OOB 케이스 trap화로 적색, ② `i64.ge_u`→`i64.gt_s` → 음수-콜리 케이스 적색, ③ element offset 0→1 → 항등-매핑 차등 적색, ④ 스크래치 구간 겹침 주입 → 기하 핀 적색(*A-4 착지 정정: 초안이 광고한 "재귀/중첩 마셜링 차등 적색"은 현행 lowering에서 원리적으로 불발 — 마셜링 store가 콜리 프롤로그에서 즉시 소비되어 총겹침조차 실행 관측 불가(§2.5 자신의 "공유안도 안전 논증 성립" 양보와 정합). 실적색 채널은 구조·매니페스트·`assert_scratch_runs_disjoint` 기하 핀이며 A-4가 3-방식 파이프라인에 이를 배선함*), ⑤ Table-모드 비-호출 lowering 변조 → record==live R1/R2 적색(§5.2-R3), ⑥ element 대상을 임포트-테이블로 변조(검증기 단위 테스트) → 산물 검증기 적색([스파이크] M5 대조 프로브의 방어 실증), ⑦ `compile_module`이 공유 사전검사를 우회(check_func만 수행)하도록 변조 → "AOT-수용 ∧ JIT-거부" 코퍼스 케이스 적색(§6 ⊂-방향 게이트의 비공허성 실증 — 패널 보강).

### 8.4 AotError 회귀

Refusal 전 변형의 승격 경로 — **ValidationFailed**(가장 흔한 승격 경로, 패널 지적으로 추가)/정수 Div/HostFnUnresolved/FallsOffEnd/TrailingJumpIf/ControlEffectNotLast/WasmLocalsLimitExceeded/Scratch 각 실물-입력 1케이스 이상. 예외 2건 명기(패널 보강): `TooManyLeaders`는 실물 입력 생성이 사실상 불가(≥2³¹ 인스트럭션 — O-13 자체 기록)라 **값-수준 매핑 단위 테스트**(Refusal 값 → `AotError::Func` 승격의 검사)로 대체 허용; `UnsupportedEffect`는 도달불가 방어 마커라 제외. + 모듈-수준 3종(ScratchBaseMisaligned/Entry/중복명 — §4.2 개정 반영) + 결정적 첫-실패 순서(단일-패스 규범 §4.2) + "Err ⇒ 산물 0바이트" 단언.

---

## §9. 증분 분할 (각 1커밋 규모 — 설계-패널-구현-리뷰 워크플로 단위)

| 증분 | 스코프 | 게이트 |
|---|---|---|
| **A-1** `compile_module` 골격 | N함수 1모듈(Types 인터닝 모듈-승격) + `AotError` 총체화(§4.2 단일-패스 규범 포함) + `AotOpts`/매니페스트 골격 + 스크래치 자체-배정(§2.5) + **잠정 export `otter_entry`**(차등 실행 표면 — 패널 보강: export 없는 N-in-1은 인스턴스화 후 호출 불가라 게이트가 공허해짐; `otter_table`은 A-2). 호출 lowering은 **무변경**(디스패치-형 유지, canonical 슬롯·센티널 §5.2) — ESC AOT-1 대응 | N-in-1 차등 그린 + record==live **전문 T-정규화** 바이트 게이트(§5.2 — 호출부 포함, A-3 전이라 전문 성립) + AotError 회귀(§8.4) |
| **A-2 — DONE (2026-09-04)** 내부 링크 | 선언 테이블+active element(항등 매핑+헬퍼/호스트 등재) + 함수 임포트 신설(§2.2 명명) + export `otter_table` 추가 + 산물 검증기(AOT 화이트리스트+element 자기-테이블 검사) — ESC AOT-2 대응. **중간 형상 규범은 표 아래 명기(착지 시 슬롯 위치 정정됨)** | 2-모듈 인스턴스화 차등 그린(임포트-등재 element 실측화 — §3.4-3 **해소 확인**) + M3-형상 회귀(memory 임포트 유지) + 뮤테이션 ③⑥ 전부 그린(2렌즈 독립 재실행, 결함 0건) |
| **A-C1** 엔진 전폭 검사 co-requisite (§3.2a — otter/otter-engine 측) | `module_dispatch`/`tail_signal`/T0 콜리 캐스트 **앞** i64 전폭 범위검사(네이티브 무변경, wasm32 잠복 절단-분기 해소 — 패널 must-fix 처방) | 전폭 OOB 콜리 값-수준 회귀(2³²+k, −2³²) + 기존 oob 3종 무회귀 — **A-3 착수 게이트** |
| **A-3 — DONE (2026-09-04)** 호출 반전 | Call→가드+`call_indirect`(§3.1), TailCall→가드+`return_call_indirect`(§3.2), dispatch/tail 임포트·슬롯 소멸(테이블 `T+2`→`T`, host_slots `nfuncs+3+k` 복귀), 내부 CallLowering 모드 매개변수화(§5.1 — JIT 경로 바이트 무변경 핀 유지) — ESC AOT-3 대응 | 2M 유계(64 KiB)+음성 대조(동일 가드·TailCall→Call만 교체로 `return_call_indirect` 격리 — 문언의 "등" 범위 내 정련) + fail-closed **4형상**(전폭 OOB 2³²+k가 A-C1 절단-해저드 정조준, Call·TailCall 양쪽 FILL) + `if (result i64)` 형상 실측화 해소 + 재귀/중첩 마셜링 차등 + record==live **R2 창-국소화** 전환 그린(+R3 뮤테이션 증거 선행 착수 — CI 승격 자체는 A-4 잔존). 2렌즈 리뷰 결함 0건 |
| **A-4 — DONE (2026-09-04)** 게이트 경화 | record==live R1/R2/R3 CI 승격(기존 `--workspace` 포함 확인 + 게이트 레지스트리 주석 + 스킵-방지 메타가드 — 과잉 배선 기각 근거 기록) + AOT⊂JIT 3-방식 차등 배선(§6 — 리졸버 쌍 고정을 단언이 아닌 구성으로, 스크래치 기하는 매니페스트 주입으로 실체화; JIT-형 레그는 all-wasm 해석) + 뮤테이션 ①②④⑦ 실행·영구 레저 기록(⑤는 A-3의 영구 테스트가 대체; ④의 실적색 채널은 §8.3 정정 참조) | 뮤테이션 전건 적색 실증 기록(리뷰가 3건 독립 재수행으로 레저 축자 일치 확인) + 3-방식 비트동일 코퍼스+fuzz 그린 + ⊂-거부 8형상 값-동일. 2렌즈 리뷰 머지 가(비차단 3건 — 허위 인라인 주석·메타가드 cfg_attr 우회·§8.3-④ 문구 — 커밋 전 전건 반영) |
| **A-5 — DONE (2026-09-04)** E2E+마감 | 호스트 48 cfg E2E(§8.2 — shared-임포트 × 테이블 × `return_call_indirect` 실물 조합 실측화, §2.7 **해소 확인**; 신설 `hosts/otter-aot-e2e` + `just wasm-aot-e2e` + CI wasm-eh-pinned 스텝 — 배치 결정·문언-초과분은 §0 v2.3 부기) + semver/non_exhaustive 마감(`missing_docs` 기계 강제 + exhaustive 3건 재량 처분 — §0 v2.3-③) + O-16c′ 서빙 인터페이스 문서(**§12 신설**: 매니페스트 계약·링커 의무 4건 §2.5·b′ export 요구 명세(§1.2 신규 의무 포함)·**argc>0 엔트리 호출 시 args-블록 메모리 구간의 의무 소유자 = 임베더 명기**(v1 정본 entry 호출은 argc=0 — 프롤로그 argc-가드로 args=0 안전; 패널 보강)) + 본 문서 상태 갱신 | E2E 그린(마커 5종+ALL-OK, 1차 실행 그린) + 기존 전 게이트 무회귀(workspace test/lint/no-std/wasm-cross-check/wasm-e2e/wasm-e2e-atomics) + 적대적 리뷰(§5.1 체크리스트 ①–④) **수행·통과 (2026-09-04, 2렌즈)**: ①②③④ 전건 PASS(제2 방출 경로 0·창 정의 무확장·마셜링 불변식 유지·귀속 전건 정확), 게이트 7종 독립 재현 그린, E2E 뮤테이션(element offset 변조 → 적색) 실증, 결함 = 비차단 2건(증분 오귀속 주석 2곳 + §0 계수 자구 — 커밋 전 반영). **O-14 마감 게이트 충족** |

**A-2 중간 형상 (규범 — 패널 보강, 게이트 실행 가능성의 요건)**: A-3 전이므로 dispatch/tail이 잠정 존속한다 — 테이블 크기 `T+2`, dispatch/tail 등재, 둘은 `otter_helpers` 이름공간의 **잠정 임포트**. 2-모듈 게이트의 인스턴스화 순서 순환(헬퍼-제공 스텁이 동시에 `otter_table` 임포트자가 되는 구조)은 **dispatch/tail을 하네스 호스트-글루 함수로 해소**해 절단한다: 스텁 모듈은 헬퍼/호스트만 export하고, 글루는 AOT 모듈 인스턴스화 **후** 테이블 참조를 획득하는 2단 배선(기존 mock-하네스와 동형). 이 형상은 A-3에서 통째 소멸하는 잠정물이며 계약 표면이 아니다.

**A-2 착지 정정 (2026-09-04, 구현 리뷰 조건 — 코드 변경 없음, 문서-코드 발산 해소)**: 위 문단의 "슬롯
`T`/`T+1`"(호스트 임포트 뒤)은 §4.2 단일-패스·선-스캔-금지 규범과 실제로 충돌한다 — `T = nfuncs+3+nhosts`는
전 모듈 호스트 리졸브가 끝나야 확정되는데, `Call`/`TailCall` 사이트의 `call_indirect` 즉값은 그보다 훨씬
앞선 시점(해당 함수의 바디 방출 시)에 베이크돼야 하기 때문이다(가변폭 LEB128 즉값의 사후 패치도 현재
파이프라인과 불합치). **착지 형상**: dispatch/tail을 helpers 직후 **고정 슬롯**(`nfuncs+3`, `nfuncs+4` —
nhosts와 무관하게 즉시 계산 가능)에 배치하고 호스트는 `nfuncs+5`부터 시작한다. 테이블 총 크기는 문언과
동일하게 `T+2`로 보존(내부 배치 순서만 교환). 어떤 게이트도 "호스트가 트램폴린 앞/뒤"라는 하드코드 가정에
의존하지 않음(전부 매니페스트에서 슬롯 값을 직접 읽음 — 2렌즈 리뷰 독립 재확인). 이 형상은 A-3에서 통째
소멸하는 잠정물이므로 §4.2와의 정합만 확보하면 되고, 정확한 내부 위치는 계약 표면이 아니다.

---

## §10. 위험

1. **임포트 명명 ↔ b″ 링크 규약 불일치**: wasm-ld/`wasm-tools` 계열의 이름공간 관례가 §2.2 문자열과 어긋날 수 있음 [추정]. 완화: 매니페스트가 임포트 전량을 나르므로 개명은 기계적(§11-1) — O-14는 개명-내성 설계만 보장.
2. **스크래치 구간 예약 실패**: b′ 엔진-라이브러리의 할당기가 고정 구간 예약을 지원 못 하면 침묵 오염 위험(§2.5). 완화: 계약 MUST(겹침·커버리지 두 발현 모드 §2.5-i/ii) + b″ 기계 검증 이월 + **1차 대응 = 재방출**(링커가 새 base로 `compile_module` 재호출 — §2.5-iv 공인, XIP 대비 결정 §7b-①)로 global-간접 폴백 의존도 하락. 최악 시 대안(global-임포트 간접 `global.get+add`)은 record==live의 I32Const 형상을 깨므로 그때 재설계 사안으로 명시(§11-3 교차 등재 — 패널 보강).
3. **record==live 창 규율의 침식**: 향후 lowering 변경이 창 밖 diff를 만들면 게이트가 막지만, 창 정의 자체를 넓히는 "게이트 완화 커밋"은 사람이 막아야 함 — 리뷰 체크리스트 항목화(§5.1).
4. **전략=cranelift 전제**: Winch tail-call 미지원([스파이크] 캐비앗 2). 현 대상 환경(하네스 45·호스트 48·T1 아키 전부 cranelift)에선 무관하나 엔진-캡 사전검사 문서에 명기 — 미래 Winch 채택 시 A-모드 하드 게이트.
5. **네이티브 CallMode::Table과의 갭 비대칭**: 네이티브 Table AOT는 TailCall `unimplemented!`(§8.10 예외 존속)인데 wasm AOT는 갭-프리 출발 — 패리티 의무 대상은 아니나(ESC §3.5) 문서상 혼동 소지. DESIGN.md 티어 표 갱신 시 각주로 처리.
6. **Halt 의미론**: 중첩-Halt의 컴파일-티어 붕괴는 기존 의미론이나 전-모듈에선 노출 빈도 증가 [추정] — §6 카브아웃과 차등 제외 규칙으로 관리.
7. **하네스 45 ↔ 호스트 48 이중 트레인**: 기존 정책(O-13 §5.5 부기) 그대로 — tail-call은 양 트레인 기본 ON 실측이라 추가 위험 없음. 45.0.2/45.0.3 패밀리 내 안정 확인([스파이크] 캐비앗 3).
8. **min=max 테이블의 불변성**: A-모드 산물엔 push_func류 동적 추가가 없다(설계 귀결). 조합(A+swap류) 수요가 생기면 테이블 export 경유 재구성이 가능하나 O-14 계약 밖 — 문서화만.

---

## §11. 열린 결정 (최소화 — 3건, 전부 O-16c′ 트리거) — **전건 종결 (O-16c′ 구현 완료, 2026-09-04)**

1. **임포트 이름공간 최종 문자열**(§2.2의 `otter_helpers`/`otter_host`): b″ 링크 도구의 규약 확인 후 확정. 매니페스트 경유 기계적 개명이 가능하므로 O-14 산물 형상을 블로킹하지 않음 — O-16c′ 착수 시 판정. **→ 종결(O-16c′ §2-6, L-1a/L-4 착지)**: 방출기 문자열 현행 유지 + b″ `rename` 단계(매니페스트-구동, import 섹션만 바이트 재작성, 등록명 `otter_lib`) — **방출기 무변경**. `otter_host.*`는 잔존 → 컴포넌트-수준 리프트가 정본(코어 규약 `$root`+kebab, L-5 실측).
2. **매니페스트 직렬화 포맷**: v1은 Rust 값(§4.1). JSON/바이너리 고정은 소비자(b″)의 입력 형식이 정해지는 O-16c′ 시점에. 빌드-해시 fail-closed 전체 규율도 동반(§4.1 명시 이연 — universal-binary 버저닝을 그 시점 선행 사례로 재인용, §7b-②). **→ 종결(O-16c′ §3.1, L-1b 착지)**: JSON 사이드카 한 층 — 스키마·ser/de는 `hosts/otter-aot-link` lib `schema` 모듈 단독 소유(생산 측은 path-dep로 동일 타입 소비, 이중-정본 봉쇄). 세대-대조 정본은 b′ 사이드카 콘텐츠-해시(`artifact_sha256`, fail-closed); 산물 전체 해시 체계·버저닝은 O-16c′ §9-3에 유보 승계(변별력 0 정직 명기 동반).
3. **스크래치 예약 메커니즘 실증**(§10-2 교차 등재 — 패널 보강): b′ 엔진-라이브러리의 고정 구간 예약 가능 여부는 O-16c′ b″에서 판명. 실패 시 1차 대응은 재방출(§2.5-iv)이고, global-간접 폴백 채택은 §5.2 게이트의 I32Const 형상 재심을 동반하는 조건부-재설계 사안(가장 비싼 미검증 항목의 정직 표기 — baked 주소 형상 자체는 현행 JIT과 동일하므로 O-14 내 검증 항목은 아님). **→ 종결(O-16c′ §2-1/§2-2/§2-3, L-1b/L-2/L-4 착지)**: 예약 = `inject-marker`의 active 마커 세그먼트(폭-분기) + 구성적 min-상향, 정적 증거 3검사(마커·커버리지·**하한 대조 base ≥ 사이드카 heap_floor** — 섀도-스택 겹침의 직접 방어선)로 기계 검증, 실물 b′ 상대 실측 그린(heap_floor 0x100000/0x110000). global-간접 폴백은 불필요로 판명 — 재방출 경로만 공인 유지.

---

## §12. O-16c′ 서빙 인터페이스 (A-5 신설 — 소비자 계약 정리)

O-14 산물(`AotArtifact` = bytes + manifest)이 O-16c′의 b′(엔진-라이브러리 wasm 빌드)·b″(M2a 링크
도구)와 임베더에게 서빙하는 인터페이스의 단일 정리. **신규 규범 0** — §1.2/§2.2/§2.4/§2.5/§4.1의
기존 계약을 소비자 관점으로 재편성한 것이며, 문구가 상충하면 원 절이 정본이다.

**실물 체인 교차참조 (O-16c′ 구현 완료 — L-6 부기)**: 본 절이 규정한 소비자가 전부 레포 실물로
실존한다 — b′ = `hosts/otter-helpers-wasm`(+`just wasm-bprime`: §12.3 export 의무 이행 실측 —
export 4종 `memory`/`otter_binop`/`otter_convert`/`otter_unop`), b″ = `hosts/otter-aot-link`
(+`just wasm-aot-tools`), 결합 체인 = `just wasm-aot-link-e2e`(§12.2 의무 4건의 기계 검증:
check --scratch (i)/(ii)/(iii)), 컴포넌트화 = `just wasm-aot-component`, 임베더 참조 구현 =
`hosts/otter-aot-e2e`(§12.4 v1 정본 호출 축자 + run-linked/run-component). §12.1 표의
`shared_memory` 행("sharedness 정확 일치")은 L-6에서 `check --linked`의 기계 술어로 승격
(산물 실제 memory sharedness == manifest 교차 검사). 정본 설계·대차:
`otter-o16c-componentization-design.md` §10.

### 12.1 매니페스트 계약 (필드별 소비자 의무 — 타입 정의는 §4.1, 값 규범은 §2.3/§2.5)

| 필드 | 값 정의 | 소비자 의무 |
|---|---|---|
| `entry_slot` | `= m.entry` (테이블-슬롯 공간 값 — wasm 함수 인덱스 아님) | 개별-함수 호출/검증 표면의 키. 엔트리 실행 자체는 `otter_entry` export로 충분 |
| `nfuncs` | `Call`/`TailCall` 가드 경계 (§3.1/§3.2) | 재해석·재정의 금지 — 콜리 값 공간 상한이 전 사이트에 즉값 베이크된 무재배치 산물 |
| `helper_slots` | binop/convert/unop 테이블 슬롯 `[nfuncs, nfuncs+3)` | b″ 조합·스왑류 작업에서도 가드 경계 밖(게스트 콜리로 도달 불가) 성질을 보존할 것 (§2.3) |
| `host_slots` | k번째 호스트 임포트 슬롯 `= nfuncs+3+k` (최초-해소 순서 = 임포트 순서) | 이름→슬롯 대응의 기계-가독 정본 (§2.3 도출 규범의 사본) |
| `imports` | 전 임포트, 임포트-섹션 순서 | **b′ export 요구 명세**(12.3). `kind`는 편의 사본 — 함수 타입 정본은 산물 type 섹션, 불일치 시 bytes 우선 (§4.1) |
| `scratch` | `func_idx → (addr, words)` | 진단·검증용. `words == 0` 엔트리의 `addr`은 예약 대상 아님(빈 span — lib.rs doc) |
| `scratch_region` | `[base, end)`, `end`는 u64 (`== 2³²` 합법) | **12.2의 예약 의무 대상 구간** |
| `table_size` | 선언 테이블 크기 `T = nfuncs+3+nhosts`, min=max | 동적 슬롯 추가 없음(§10-8) — 조합 수요는 테이블 export 경유 재구성, O-14 계약 밖 |
| `shared_memory` | memory 임포트 형상 (§2.7) | b′의 `+atomics` 빌드 여부와 정합 필수 — sharedness는 정확 일치, 불일치는 인스턴스화 실패(M5) |
| `emitter_version` | 방출기 `CARGO_PKG_VERSION` | 산물↔엔진-라이브러리 세대-대조의 최소 앵커 — 전체 빌드-해시 fail-closed 규율은 O-16c′ 이연(§4.1/§11-2) |

### 12.2 링커/임베더 의무 4건 (§2.5 i–iv 요약 — 정본은 §2.5)

(i) **겹침 금지**: `scratch_region`을 엔진-라이브러리의 어떤 할당(힙·섀도 스택·데이터)과도 겹치지
않게 예약(MUST — 위반 발현: 침묵 메모리 오염, O-12 최악 클래스). (ii) **커버리지**: 구간 전체가
인스턴스화 시점 메모리 경계 내이거나 첫 호출 전 grow(MUST — 위반 발현: 첫 마셜링 store의 결정적
trap; (i)과 발현 모드가 달라 b″ 기계 검증이 둘을 구분 설계). (iii) **재배치 불가**: 테이블 슬롯·
스크래치 주소가 즉값 베이크된 무재배치 산물 — flat-merge형 링크(슬롯 이동) 불가. (iv) **공인
재배치 = 재방출**: base 재선정은 새 `scratch_base`로 `compile_module` 재호출(방출 결정적 — §5.2).
전부 b″ 게이트에서 기계 검증할 것(O-16c′ 이월, 의무 자체는 본 계약이 규정).

### 12.3 b′ export 요구 명세 (§1.2/§2.2)

- **헬퍼 3종 export 의무 (신규 의무 — §1.2/§2.2 [실측])**: `otter_binop`/`otter_convert`/
  `otter_unop`은 현재 `#[no_mangle]`/`#[export_name]` 부재로 심볼이 맹글링되어 동명 export가
  어디에도 실존하지 않는다 — b′가 export 속성(`#[unsafe(no_mangle)]`/`#[export_name]`) 부여 또는
  래퍼 방출로 §2.2 이름을 export해야 한다.
- **호스트 fn**: `otter_host.<임베더-등록명>`별 export(또는 링크-시 해소) — `manifest.imports`가
  전체 목록이자 유일 정본.
- **memory**: M2a의 선형 메모리 최종 소유자는 엔진-라이브러리(§2.2) — 산물은 임포트 형상만
  선언하며 공유 여부는 `manifest.shared_memory`가 나른다(b′의 `+atomics` 빌드 인지는 빌드 시스템
  몫 — §2.7).
- **타입 정본**: 각 임포트의 wasm 함수 타입은 산물 type 섹션(바이트)이 정본 — `AotImportKind`의
  arity/ctx 라벨은 서술적 편의(§4.1).
- 이름공간 문자열(`otter_helpers`/`otter_host`)은 §11-1 유보 — b″ 규약 확정 시 개명은 매니페스트
  경유 기계적.

### 12.4 엔트리 호출 계약 (임베더)

- **v1 정본 호출 = `otter_entry(ctx=0, args=0, argc=0)`** (§2.4): `ctx`는 죽은 파라미터(불투명
  전달만), argc=0이면 프롤로그 argc-가드가 args 포인터를 결코 역참조하지 않으므로 args=0이
  안전하다. A-5 E2E가 이 형상 축자로 검증(AOT48-E2E-1/-2).
- **argc>0 호출 시 args-블록 메모리 구간의 의무 소유자 = 임베더** (패널 보강의 명문화): 임베더가
  자기 관할 구간 — 12.2의 `scratch_region` 및 엔진-라이브러리 할당과 서로소인 — 에 argc개의 i64
  워드를 마셜링하고 그 주소를 `args`로 넘긴다. 구간 유효성은 **호출 경계 전체**에서 임베더가
  보장한다(콜리 프롤로그의 즉시 선복사는 lowering 내부 사항이며 계약 표면이 아니다). 게스트-측
  대안: 인자를 즉값으로 받는 0-param 부트 함수를 IR에 두면 마셜링이 전부 게스트 안에서 일어나
  임베더 의무가 소멸한다(A-5 E2E의 깊이-인자 전달 방식).
- **개별 함수 호출**: `otter_table` export + `manifest` 슬롯 맵 경유(§2.4) — 동일한 args-블록
  의무가 적용된다.

### 12.5 §11 유보 상태 요약 (전부 O-16c′ 트리거 — 정본은 §11) — **전건 종결**

① 임포트 이름공간 최종 문자열(§11-1 — 개명-내성만 O-14가 보장), ② 매니페스트 직렬화 포맷 +
빌드-해시 fail-closed 전체 규율(§11-2 — universal-binary 버저닝을 그 시점 선행 사례로 재인용,
§7b-②), ③ 스크래치 예약 메커니즘 실증(§11-3 — 실패 시 1차 대응 재방출 §2.5-iv, global-간접
폴백은 §5.2 게이트 재심을 동반하는 조건부-재설계). **→ 3건 전부 종결(§11 각 항의 착지 부기 —
방출기 무변경·JSON 스키마 단독 소유·마커+3검사; 잔여 유보의 승계처는 O-16c′ §9-3뿐).**

---

*작성 규율 확인: 스파이크 실측 인용은 M1–M5·캐비앗·설계 권고 번호로 축자 귀속, [실측]/[추정] 구분 표기, 오너 결정은 전부 MODE §5b 인용으로만 귀속(element/data 허용은 ESC §5+워크플로 지시 귀속으로 분리, 선행기술 절은 alt-runtimes §4b 귀속), 설계 선택(§2.2 명명·§2.3 슬롯 배치·§2.5 서로소·§3.4 하이브리드·§5.2 게이트 정의·§4.1 시그니처)은 자체 결정임을 명기. v2 규율: 3렌즈 패널 must-fix 전건 반영(§0), should-fix 기각 0·부분 이연 1(빌드-해시 — 사유 §4.1), 패널 간 상충 2건은 §0에 기록 후 보수 선택(T-정규화 게이트-측 해소, 모듈-수준 스크래치 변형 삭제-포섭), 패널 [실측] 과일반화 정정 2건(§2.7·§3.1) 반영.*
