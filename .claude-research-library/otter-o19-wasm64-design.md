Status: DONE (2026-09-04 — §7 전 증분 착지: C0–C2 커밋 6104656, C3–C6 본 증분. 게이트 (a)(b)(c) 전부 충족 — (a) C1/C2, (b) C4 `wasm64-smoke` 컴파일 카나리아, (c) C3 `a64-diff` 레인(A64==A32==오라클 + 명시-옵트인 음성 대조 + table64 벡터 go()→7). DESIGN v2 본문은 2렌즈 패널 반영판 그대로 보존)

# O-19 설계 노트 — wasm64: addrtype 추상화 + T0 스모크 + wasmtime L1 실행

상위 문서: `.claude-research-library/otter-wasm-scope-escalation-research.md` §1.4·§2.3(정본 입력), `otter-o13-wasm-emit-design.md`(방출기 정본), `otter-o17-shared-memory-l0.md`(shared 파라미터화 선례), `otter-o14-aot-core-design.md`(AOT 계약). 스파이크 정본: O-19 S1 보고(otter T0 wasm64 실물 — M1~M4, 증거물 스크래치 `o19-s1/`)·S2 보고(memory64 방출 프로브 — M1~M4, 증거물 스크래치 `o19-s2/`) — **증거물은 C0로 연구 라이브러리에 영속화한다**(§7; 스크래치는 세션-휘발, S2는 실행 로그 미보존 상태 [패널 실측]). 두 스파이크 모두 커밋 0건·레포 소스 무수정·spikes/ 무접촉.

표기 규율: **[실측]** = 스파이크/공식 소스 확인, **[추정]** = 실측 기반 추론, **[결정]** = 본 설계의 선택(근거 병기). 귀속: 오너 결정은 인용만, 스파이크 발견은 스파이크에, 설계 선택은 본 문서에 귀속.

---

## §0. 개정 요약 (v2 — 2026-09-04, 2렌즈 패널 반영)

2렌즈 적대 패널(scope: 인용 전수 실물 대조 + S2 재실행 + (b) 스모크 재현 / tech: 게이트 집행력 심사) 결과를 전건 처리했다. 패널의 생존 판정("처분표 7건 [실측] 전부 실재, 과대 스코프 없음, 형상 설계·실측 기반 견고, L-2 정합은 fail-closed 강화")은 해당 절 무수정 유지의 근거로 인용한다.

**must-fix 4건 — 전건 반영**

| # | 지적 | 처분 | 반영 위치 |
|---|---|---|---|
| MF-1 (scope-1) | 게이트 (c)가 L1 정의문("wasmtime memory64+**table64** 실행", 연구 §2.3:131)의 table64 절반을 무해소·무기록(무언 치환) | C3 차분 레인에 **table64 실행 벡터 1개 추가**(S2 프로브 형상 재사용) — 산물-레벨 게이트 편입, 오너 확인 항목 불요. 방출 산물은 혼합형 유지 | §1.2(c)·§4·§6-2·§7 C3 |
| MF-2 (scope-2) | C6 정정 스윕 과소 — 연구문서 스테일 5개 행(:130·:131·:186·:188·**:204**) 잔존, :204는 게이트 몫 정의와 충돌([[check-completeness-periodically]] 문서판 위반) | C6을 **전수 스윕**으로 확장(5개 행 전부 + :204를 처분표(§5) 정합 문안으로) | §7 C6 |
| MF-3 (tech M1) | record==live(R1/R2)의 A64 커버리지가 부재하고 현 설계상 구조적 불가 — EmitOpts 비확장 [결정]으로 A64 live 쌍둥이 공급원 소멸(`aot.rs:597-650` live twin = JIT 표면 재방출, `compiled_ty_norm` A32 하드코딩) | 패널 제시 옵션 **(b) 채택**: A64를 record==live에서 **명시 면제** + **modulo-addrtype 차분 게이트** 신설, A64 corpus는 R-게이트 불포함 명시(포함 시 CI-승격 R2 `aot.rs:2692` 즉시 적색) | §2.6(신설)·§6-2·§7 C2 |
| MF-4 (tech M2) | §2.3 바이트-불변 핀이 공허 — O-17 선례는 동일-빌드 진입점 간 비교라 공통-모드 바이트 변화에 전맹, AOT 표면은 legacy 진입점 부재로 자기비교 | 핀 정의를 **크로스-커밋 골든 벡터**로 교체: pre-O-19 HEAD에서 양 표면 corpus 바이트/해시 채취 → C1 체크인·단정 | §2.3(교체)·§6-1·§7 C1 |

**should-fix 8건 — 7 반영 / 1 부분 반영**: scope-3(스파이크 증거 휘발) → **C0 신설**(증거 영속화, o13/o16c 선례; §1.4 수치는 로그-검증 가능한 1.47×로 핀). scope-4(C6(ii) 문안 비대칭) → 문서-실측 괴리 병기(§7 C6). scope-5(J-mode 신규 노출 무시험) → §6-4 (vi) 신설. scope-6(check --linked memory64 무검사) → linked 교차술어 확장(§2.5·C5). tech-S1(단일-Config 규율 충돌) → §4 명시 해소(단일 빌더 파라미터화 + 기존 레인 무변경 단정). tech-S2(음성 메시지 귀속 오류) → §4·§6-4 문자열 핀 금지·Err 단정. tech-S3(C5 스키마 미결정) → §2.5 `#[serde(default)]` + `addr: u32` 존치 근거 명기. tech-S4((b) 검증 비대칭) → **부분 반영**: §1.2에 "(b)=컴파일 카나리아" 명기는 채택, 대안(§3.2의 C4 편입)은 **기각** — 상시 nightly+build-std 실행 레인의 비용 대비 게이트 가치 부족(패널 자신이 "수용 가능" 판정), §9-2로 닫음.

**note 처분**: scope-N7(`:1570` 소재 정정 — §2.1), scope-N8/tech-S2(메시지 변형 귀속 — §4), scope-N9(nightly 핀 지시 정정, 값의 정본 = justfile `wasm-nightly` 변수 — §3.1), scope-N10/tech-S3(병합 — §2.5) 전부 반영. tech-N1(legacy 진입점 u32 존치+내부 업캐스트) 채택 — §2.2 #6. tech-N2(2^63 캡 타당) → §9-1 닫힘. tech-N3(혼합형 정확) → 무수정 유지 근거. tech-N4(C2 분할 여지) → §9-3 이월.

---

## §1. 목표·비목표 — 게이트 몫 확정

### §1.1 오너 스코프 (인용)

오너 재확정 2026-09-04, 조건부-게이트 답변 문안 축자: **"wasm64는 L1 wasmtime-실행까지 + addrtype 추상화"**. O-19는 상향 stable 게이트의 마지막 하드 항목이다.

### §1.2 게이트 몫 확정 (스파이크 근거)

예비 독법 (a)(b)(c)가 S1·S2로 전부 검증되어 그대로 확정한다.

- **(a) lowering 코어의 addrtype 파라미터화** — 방출물이 memory64 형상으로 나올 수 있게 한다. 변경 지점은 S2-M2가 `crates/otter-wasmemit/src/lib.rs` 전수로 확정(§2.2) [실측]. 노출 표면은 **AOT(`AotOpts`) 전용** — JIT 표면(`EmitOpts`/`DISPATCH_TY`/임포트 테이블)은 v1 비대상(§2.1) [결정].
- **(b) otter T0의 wasm64 빌드 스모크** — S1-M1이 3형상 그린을 실측했고, 최소형(라이브러리 `core,alloc` 빌드, 소스 무수정)이 스모크로 충분 [실측]. justfile 레시피 + CI 레인(§3). **검증 수준 명기** [v2, 패널 tech-S4 채택]: (b)의 반복 검증은 **컴파일 카나리아**이며, rustc-산출 wasm64 코드의 실행 실증은 S1 증거물(비반복, C0로 영속화)로 갈음한다 — 어떤 CI 레인도 rustc-산출 wasm64를 상시 실행하지 않는 (a)(c) 대비 검증 강도 비대칭을 여기 기록한다(§9-2).
- **(c) wasmtime memory64 L1 실행 실증** — 기계는 S1-M2(게스트 실행)·S2-M4(방출 형상 왕복, 48.0.1과 45.0.3 양 트레인)로 이미 실증 [실측]. 구현 단계 게이트는 **"`compile_module` A64 산물로 동일 왕복"** + **table64 실행 벡터 1개**(§4)다. 후자는 v2 추가 [패널 MF-1]: 정본 사다리의 L1 정의문은 "wasmtime memory64+**table64** 실행"(연구 §2.3:131)인데 방출 산물은 혼합형(table64:false, §2.1)이므로, table64 절반을 스파이크-레벨 증거(S1-M3·S2 table64.go()→7)에만 두는 무언 치환 대신 C3 레인의 직접 벡터로 산물-레벨 게이트에 편입한다.

### §1.3 비목표·조건부 (각 해소 조건 명기)

| 항목 | 지위 | 근거 | 해소 조건 |
|---|---|---|---|
| 엔진-on-wasm64 (J-mode) | **조건부 — 게이트 밖 백로그** | S1-M1보너스가 핀 nightly-2026-08-31에서 `-Cpanic=unwind`+exnref 그린(빌드·링크·validate·런타임 catch_unwind)을 실측, otter-engine 탑승까지 실증 — 종전 "기계적으로 못 탄다"는 근거는 소멸. 단 rustc 플랫폼 문서는 여전히 미지원 선언 → **"핀-실측 그린, 업스트림 계약 아님"**. 따라서 분류는 '외부 차단'이 아니라 정직하게 '미착수'(오너 문안대로 게이트 밖) [실측] | rustc 문서상 wasm64 unwind 지원 전환, 또는 오너가 핀-실측 기반 채택을 결정(그 경우 W-4 2레인 카나리아 규율 편입 필수) |
| wasm64 × WASI (L2) | **외부 차단 — 게이트 밖** (오너 확정) | wasi-sdk "wasm64 미지원·계획 없음", wasi-libc PR#444 계류, `wasm64-wasip*` 타깃 부재 [실측, §1.4] | WASI Subgroup의 wasm64 ABI 결정 + rustc 타깃 출현 |
| wasm64 × 컴포넌트 모델 | **외부 차단 — 게이트 밖** (오너 확정) | CM canonical ABI의 memory64 lowering 미표준화(CM#22) [실측, §1.4] | CM#22 표준화 |
| L3 웹 배포 | 관망 | Safari 전면 미지원 → Baseline 불가 [실측, §1.4] | Safari memory64 출하 |
| A64 성능 레인 | 비목표 | 아래 §1.4 | — |

### §1.4 성능 정직성 기재

wasm64는 wasm32보다 **항상 느리다**: guard-page 생략 불가 → 명시 bounds check, 문서 밴드 1.2–1.8× [실측, 연구 §1.4]. S1-M4가 T0 디스패치 루프(2천만 인스트)로 재확인: native 256ms / wasm32 356ms / wasm64 523ms — **w64/w32 = 1.47×**(보존 로그 `run-final.log` 기준 단일 값; v1의 "1.45–1.47×" 밴드는 S1 보고 문안으로 현재 로그만으로는 하한 검증 불가 [패널 scope-3] — C0 영속화가 보고·로그를 함께 고정), 크기세 +8.55%(37181/34253) [실측]. 4GiB+ 힙 실수요 전까지 전환 동기는 음수다. **이 게이트의 근거는 성능이 아니라 오너의 스코프 결정이다** — O-19는 "wasm64를 기본으로 켠다"가 아니라 "형상이 가능함을 실증하고 폭 전제를 파라미터로 만든다"이다. A64는 벤치 기본 레인에 넣지 않는다 [결정].

---

## §2. addrtype 추상화 설계

### §2.1 파라미터 형상

- **표면**: `AotOpts`에 `memory64: bool` 필드 + `with_memory64(bool)` 빌더 신설(`#[non_exhaustive]`라 필드 추가 자유; `scratch_base: u32→u64` 폭 변경은 API-breaking이나 publish=false라 수용) [실측 S2-M2 #8]. 내부적으로는 방출기 전반에 addrtype(A32/A64)이 흐른다 — `Types` 인터닝에 addrtype을 주입해 타입 테이블 자체가 파라미터가 된다(§2.2 #1).
- **`EmitOpts`(JIT 표면)는 확장하지 않는다** [결정]. 근거: J-mode의 메모리는 엔진 자신의 메모리인데 엔진-on-wasm64는 §1.3 조건부(`hosts/otter-host-wasmtime/src/main.rs:191,292`의 `otter_jit.compile(ptr:u32,len:u32)` 계약; JIT 임포트 표면은 `crates/otter-wasmemit/src/lib.rs:1570 memory_and_table_imports` — 소재 명기는 v2 정정 [패널 scope-N7: v1 문안이 호스트 main.rs 인용에 붙어 오독 유발]) [실측 S2-M3]. O-17이 shared를 `EmitOpts`+`AotOpts` 양쪽에 파라미터화한 선례와 달리, A64는 소비자가 AOT뿐이다.
- **테이블 형상 = 혼합형(memory64 메모리 + table64:false 테이블)** [결정]. 근거: S2-M1-3이 혼합형의 유효성+실행(42)을 실측 — addrtype은 메모리별/테이블별 독립이고, **테이블·call_indirect 기계는 1바이트도 안 바뀐다**(기존 `i32.wrap_i64` `:899/:951`, elem offset `i32_const` `:2319`, 선언 테이블 `:2298-2304` 전부 그대로) [실측]. table64는 별개 옵트인도 아니어서(wasmtime `wasm_memory64` 노브가 겸장 [실측 S2-M1-4]) 혼합형이 검증 표면 최소·차분 최소다. table64 채택은 폭만의 문제로 이월(S1-M3: fn-ptr==테이블-인덱스 정체성은 table64에서도 성립, 폭만 i64 [실측]).
- **shared × memory64 = v1 fail-closed 거부** [결정]. `SHARED_MEMORY_IMPORT_MAX_PAGES=65536`(`:185`)은 wasm32 전용 상수 — 조합은 별도 실측 전까지 `AotOpts` 검증 단계에서 거부 [실측 근거 S2-M2 #11].

### §2.2 변경 지점 전수 (S2-M2 — `crates/otter-wasmemit/src/lib.rs`, 전부 [실측])

**바뀌는 곳 (주소 폭이 박힌 지점)**

1. `:449 COMPILED_TY (&[I32,I32,I64],&[I64])` — args 포인터(param 1)가 A64에서 I64. ctx(param 0)도 포인터이므로 동반 → 타입 테이블이 addrtype-파라미터화(`Types`에 주입).
2. `:793` `marshal_call_args`의 `I32Const(cx.scratch_addr as i32)` → A64에서 `I64Const` — S2-M1-2가 강제: mem64에 I32Const 주소는 **validation error "expected i64, found i32"**.
3. `:896, :948` `emit_table_call`/`emit_table_tailcall`의 scratch_addr 상수 — 동일.
4. `:1497-1502` 프롤로그 `LocalGet(ARGS_PARAM); I64Load` — 명령 불변, 포인터 타입은 #1로 따라옴.
5. `:1545-1562 push_memory_import` — `memory64: true` 분기(O-17 shared와 같은 자리·같은 방식).
6. `:672 Ctx.scratch_addr: u32`, `:1338/:1367/:1436` 파라미터, `:333 Refusal::ScratchOutOfAddressSpace{scratch_addr:u32}` → **u64**(A32에서도 u64 표현이 상위호환). **단, 공개 legacy 진입점(`compile_dispatch_fn`)의 시그니처는 u32 존치 + 내부 업캐스트** [결정 v2, 패널 tech-N1 채택]: u64 폭은 opts 경로·내부 `Ctx`에만 — "JIT 표면 비확장"(§2.1)과의 긴장(확장은 아니나 시그니처 파괴) 해소, O-17 핀의 의미 보존.
7. `:1451` 경계 `> 1<<32` → addrtype별 경계(checked_add로 오버플로 처리; A64 상한은 §2.4).
8. `:1629 AotOpts.scratch_base: u32` → u64 + `with_memory64` 빌더.
9. `:1742 AotScratchEntry.addr: u32`, `:1791 scratch_region: (u32,u64)` → u64 기반 — "끝==2^32 합법" 에지의 A64 재현("끝==2^64")은 §2.4의 캡으로 봉합.
10. `:2178` cursor(u64) 및 in-loop 협착·경계 검사 — A64 폭으로.
11. `:185` shared 상수 — §2.1의 fail-closed 거부.

**안 바뀌는 곳 (혼합형 근거, S2-M1-3)**: `:768` call_indirect 슬롯 리터럴, `:899/:951 I32WrapI64`, `:2298-2304` 선언 테이블(table64:false), `:2319` elem offset, BINOP/UNARY/HOST* 타입(값 전용), br_table/$state, `MemArg` 구조(`offset`이 이미 u64 — API 변경 불요, 합법 범위만 확장 [실측 S2-M1-5]). `:456 DISPATCH_TY`·`:1570` JIT 임포트 표면 — v1 비대상 불변.

규모 추정: **O-17급 소-중형 증분**(방출기 ~15개 지점 + 타입 파라미터화 + A64 차분 레인) [추정, S2 판정].

### §2.3 기본값 불변 규율 — 크로스-커밋 골든 벡터 핀 (v2 교체 — 패널 tech-M2)

v1은 O-17 선례(`default_opts_are_byte_identical_to_the_legacy_entry_point`) 축자 재적용을 명시했으나 패널이 메커니즘의 공허성을 입증했다: 선례 테스트(`tests/refusal_and_structure.rs:434`)는 **같은 빌드 안에서** legacy vs opts 진입점을 비교하므로, C1의 u64-확폭·인터닝 개편이 유발할 수 있는 **공통-모드 바이트 변화에 전맹**이고, AOT 표면은 legacy 진입점 자체가 없어 `AotOpts::new(b)` vs `.with_memory64(false)`가 자기비교(완전 공허)다. "파라미터 배관이 방출을 못 건드림"의 실제 증인은 크로스-커밋 고정물뿐이다.

**[결정 v2]**: 핀의 정의를 **크로스-커밋 골든 벡터**로 교체한다 — C1 착수 직전 pre-O-19 HEAD에서 대표 corpus(**양 표면**: `compile_dispatch_fn` 산물 + `compile_module` 산물)의 방출 바이트/해시를 채취해 C1에 체크인하고, 테스트가 A32 기본값 방출을 골든 벡터와 단정한다. 진입점-간 무드리프트 테스트(O-17 동형)는 보조로 유지하되 §6-1의 정의는 골든 벡터다. 유지 비용: 향후 의도적 방출 변경 시 벡터 갱신이 명시 diff로 드러난다(그것이 목적). 기존 구조 단정 방침은 v1과 동일: `tests/refusal_and_structure.rs:256,266,357`(`!memory64`/`!table64`)은 addrtype-조건부로 전환, `tests/aot.rs:2266,2279`(table64:false)는 혼합형 채택으로 **A64에서도 유지**.

### §2.4 스크래치 경계 (A64)

"끝==2^32 합법" 에지(`:2178` 주석의 근거)가 A64에서는 "끝==2^64"로 재현되어 u64 배타적 end로 표현 불가 [실측 S2-M2 #9]. **[결정]**: A64 스크래치 배정 상한을 **2^63으로 캡**하고 초과는 `ScratchOutOfAddressSpace` Refusal — u64 표현이 전 구간에서 충분해지고, u128 도입·표기 이원화를 피한다. 실용 손실 0 [추정: 2^63 바이트 선형 메모리를 제공하는 실행 환경은 현존하지 않음]. 완전 충실(2^64) 대안은 §9-1.

### §2.5 L-2 memory64-거부 도구와의 정합 해소

- **충돌 없음, L-2 거부 존속** [실측 S2-M3]. 거부 지점 `hosts/otter-aot-link/src/facts.rs:206`(`owned_memory`)은 메모리를 **소유한** 모듈(b′·링크 산물)에만 발화. O-19 승인 형상 = **pre-link 산물 + 호스트 직접 헬퍼 임포트**(otter-aot-e2e 네이티브 스텁 형상; S2-M4가 실증)는 메모리를 *임포트*하므로 그 seam에 도달하지 않는다. L-2 거부는 "링크 파이프라인은 wasm32"라는 **여전히 참인 불변식**의 수호로 존속한다 — A64×b′ 정적 링크는 wasi-libc wasm64 부재로 차단(L2 외부 차단과 동일 뿌리, v1 전-헬퍼 정책이라 헬퍼-무 프로그램은 사실상 없음) [실측 S2-M3].
- **emit-aot의 A64 사이드카 산출은 명시 에러로 거부** [결정]. `hosts/otter-aot-link/src/schema.rs:105,115`(addr/base u32)가 A64를 표현할 수 없으므로, 조용한 절단 대신 fail-closed. 외부 블로커 해소로 memory64 산물이 링크/사이드카 체인에 들어가는 날 스키마 확장.
- **invariance 검사 신설** [결정, S2 권고 채택]: `validate_aot_artifact`는 섹션 화이트리스트 전용이라 memory64 pre-link 산물을 오늘도 통과시킨다 [실측]. manifest에 `memory64: bool` 필드를 추가하고 record==live 게이트가 바이트의 memory64 플래그와 대조 — wasm32 manifest × memory64 바이트 조합을 거부.
- **C5 스키마 처분 명기** [v2 — 패널 tech-S3·scope-N10]: memory64 필드는 Rust `AotManifest`와 **JSON 사이드카**(`hosts/otter-aot-link/src/schema.rs`) 양측에 추가하되, 사이드카 측은 `#[serde(default)]`(default=false)로 선언 — `deny_unknown_fields` 구조체에 필드를 *추가*하는 것이므로 구 사이드카(필드 부재)는 default로 파싱되고, default=false여도 record==live 대조가 memory64 바이트 × 구/false manifest를 거부하므로 **fail-closed 불변**. `ScratchEntry.addr: u32`·`ScratchRegion.base: u32`는 **존치** [결정]: emit-aot의 A64 사이드카 산출 자체가 위 항목으로 거부되므로 A64 값이 스키마에 도달할 수 없다 — 스키마 확장은 링크 체인 개방(외부 블로커 해소) 시점의 일이다.
- **check --linked 교차술어에 memory64 추가** [v2 — 패널 scope-6]: `linked.rs:98 defined_memories: Vec<bool>`은 sharedness만 캡처해 링크 산물의 memory64 거부가 check --scratch 경유 `owned_memory` 단층에만 의존한다 — L-6 sharedness 교차술어와 동형으로 linked 측에 memory64 술어를 추가해 계층을 완결(C5).

### §2.6 record==live 게이트의 A64 처분 (v2 신설 — 패널 tech-M1)

record==live(R1/R2)의 live 쌍둥이는 `compile_dispatch_fn_with`(JIT 표면) 재방출로 생성되고(`tests/aot.rs:597-650`), `compiled_ty_norm`은 `[I32,I32,I64]` 하드코딩, 윈도 리터럴 형상(`table_call_window` 등)은 A32 `i32.const scratch` 형상이다 [실측, 패널]. §2.1이 EmitOpts 비확장을 [결정]한 순간 **A64 live 쌍둥이의 공개 공급원이 소멸**하므로 방치하면 양자택일로 실패한다: (i) A64 corpus를 R-게이트에 넣으면 CI-승격 R2(`aot.rs:2692` "CI-promoted by INCLUSION") 즉시 적색, (ii) 안 넣으면 새 방출 형상이 record==live 커버리지 0으로 출하 — 제2 방출 경로 금지 규율이 막으려는 바로 그 리스크.

**[결정 v2 — 패널 제시 옵션 (b) 채택]**: A64를 record==live에서 **명시 면제**하고 대체 구조 게이트로 **modulo-addrtype 차분 게이트**를 신설한다 — 동일 IR의 A32/A64 산물을 정규화-연산자열 수준에서 비교해, 차이가 §2.2 열거 지점(주소 상수 I32Const↔I64Const, 타입 테이블 폭, memory64 임포트 플래그, 경계 상수)에**만** 나타나고 그 외 전 구간 동일함을 단정한다. A64 corpus의 R-게이트 **불포함**도 명시 단정한다(위 (i) 방지).

옵션 (a)(A64 live-쌍둥이 공급원 신설) 기각 근거: EmitOpts 확장은 §2.1 결정과 자기모순이고, 테스트-전용 내부 훅은 제2 방출 표면의 위장 신설 — 단일-lowering 규율의 자기부정이다. modulo-addrtype 게이트는 A64 형상이 동일 lowering에서 파생됨을 직접 단정하므로 제2-경로 드리프트를 잡는 집행력이 있다(addrtype 파라미터화 자체는 명령 선택 지점 분기·이미터 본체 비복제로 단일-lowering과 양립 [패널 판정]). **재개 조건**: J-mode A64 개방(§1.3 ① 해소) 시 live 쌍둥이가 자연 발생 — 그때 A64를 record==live에 편입.

---

## §3. wasm64 T0 스모크 설계 (S1 실측 반영)

### §3.1 빌드 형상

- **스모크 정본 = S1-M1 최소형**: `cargo +nightly-2026-08-31 build --locked -p otter --target wasm64-unknown-unknown -Zbuild-std=core,alloc` — no_std+alloc, half 2.7.1 포함, **소스 무수정 그린** [실측]. justfile 레시피(`wasm64-smoke` 계열) + CI 레인 신설, `x86_64-unknown-none` 빌드 레인과 동렬. nightly 핀은 **O-13 §7.1 핀 절차** 편입 — 값의 정본은 justfile `wasm-nightly := "nightly-2026-08-31"` 변수 1곳(justfile:123, "bump THERE first" 주석 기존) [v2 정정, 패널 scope-N9: v1의 "§7.1식"은 본 문서에 §7.1이 없어 댕글링].
- 정본을 라이브러리 빌드로 두는 이유 [결정]: no_std 게스트 형상은 `core::arch::wasm64` **전체가 unstable**(`simd_wasm64`, rust#90599)이라 nightly feature 1개가 추가로 필요(wasm32와의 비대칭)하고 [실측 S1], 스모크의 목적(“T0가 wasm64에서 컴파일된다”)에 게스트 링크는 불요.
- **unwind 형상은 스모크에 넣지 않는다** [결정]: ⑦은 핀-실측 그린일 뿐 업스트림 계약이 아니므로(§5), 게이트를 문서-스테일 실측에 걸지 않는다.

### §3.2 실행 검증 (확장판 — §4 게이트와 공유)

std 게스트 형상(S1-M1③): `RUSTFLAGS="-Cpanic=abort -C link-arg=--export-table" cargo +nightly-2026-08-31 build --release --target wasm64-unknown-unknown -Zbuild-std=std,panic_abort`(std가 할로케이터 제공, I/O 무동작) [실측]. 검증 2경로(S1-M2 확립):

1. typed 반환값 == 네이티브 오라클(동일 smoke_core를 호스트에서도 실행; S1 값 1001007).
2. 게스트가 반환한 주소로 export memory에서 8바이트 LE 판독 == 오라클.

### §3.3 panic=abort 발현 기록

문서화된 패닉(OOB 등) → `UnreachableCodeReached` trap 발현 [실측 S1-M1]. abort 빌드는 EH 명령 0 [실측]. T0(otter)는 catch_unwind 의존 0이므로 abort 형상에서 의미론 손실 없음. (참고 실측: 엔진 게스트는 unwind 형상에서 `try_dispatch`가 불량 opcode 패닉을 Err(Trap)으로 격리+`clear_trap` 재사용 — O-12 보증이 wasm64 게스트 내에서 성립함을 S1-M2가 확인. 게이트 항목은 아님.)

---

## §4. L1 실행 게이트 — wasmtime memory64 배선

- **배선 위치**: `hosts/otter-aot-e2e`(wasmtime 48, 락파일 시드·--offline) [결정]. S2-M4가 이 하네스 형상(pre-link 산물 + 네이티브 헬퍼 스텁)의 기계를 실증했고, 참조 J-mode 호스트는 A64 비대상(§2.1)이라 건드리지 않는다.
- **게이트 판정문**: `compile_module` A64 산물이 (i) `wasm-tools validate --features all` 통과 (ii) wasmtime 48 인스턴스화·실행 (iii) 왕복 값 == 동일 IR의 A32 산물 == 네이티브 오라클 (iv) 음성 대조 통과 (v) **table64 프로브 벡터 wasmtime 실행**(아래) — 를 모두 만족하면 (c) 충족. S2-M4의 실측 왕복(`rt(addr=8, 0x0123456789ABCDEF)`, baked `i64.const 16`, 혼합형 go(24,41)→42)이 참조 벡터.
- **table64 실행 벡터** [v2 — 패널 MF-1]: L1 정의문(연구 §2.3:131 "wasmtime memory64+**table64** 실행")의 table64 절반을 산물-레벨 게이트에 편입 — C3 차분 레인에 table64 프로브 벡터 1개(S2 프로브 형상 재사용: table64 테이블 간접호출 `go()→7`, 비용 미미)를 추가해 wasmtime 실행을 단정한다. 방출 산물 자체는 혼합형 유지(§2.1 근거 불변) — 이 벡터는 손수 형상 프로브다.
- **Config 규율** [결정, S1·S2 권고 채택]: 48.0.1·45.0.3 **양 트레인 모두 default Config로 memory64/table64 컴파일 OK** — 연구문서 §1.4의 "기본 false 옵트인" 서술은 스테일 [실측 S2-M1-6, 정정은 §7-C6]. 그럼에도 코드에는 **명시 `wasm_memory64(true)` + 음성 대조**(명시 `false` → A64 산물 거부)를 함께 박는다 — 기본값 재반전 방어.
  - **단일-Config 규율과의 정합** [v2 — 패널 tech-S1]: otter-aot-e2e의 `reference_config`(main.rs:736-751 — "reference-host Config VERBATIM", "SINGLE Config builder for every mode"; :470-474 L-4 dedup 처방)는 **복제·분기하지 않는다** — 단일 빌더에 memory64 파라미터를 추가하고, 기존 전 레인이 false로 호출되어 산출 Config가 무변경임을 단정 테스트로 고정한다. 참조 J-mode 호스트는 A32 불변(§2.1).
  - **음성 메시지 귀속 정정** [v2 — 패널 tech-S2·scope-N8]: v1이 인용한 "memory64 must be enabled for 64-bit tables"는 S1 NEGCTRL2 — **table64를 가진 rustc 게스트**의 메시지다. 혼합형 산물의 `wasm_memory64(false)` 거부 사유는 "…for **64-bit memories**" 변형 [패널 재실행 실측]. 음성 테스트는 **문자열 핀 금지, Err 단정** — 틀린 쪽 문자열을 핀하는 미스파이어 방지.
- **호스트 수신측(전제 ⑥)**: 임포트 메모리는 `MemoryType::new64` 필수 — 32-bit 호스트 메모리는 `incompatible import type for otter::memory` 링크 거부 [실측 S2-M4]. 이 거부 자체를 음성 테스트로 고정.

---

## §5. 32-bit 전제 7건 처분표

| # | 전제 (연구 §1.4) | 처분 | 근거 |
|---|---|---|---|
| ① | `otter_jit.compile(ptr:u32,len:u32)` 계약 | **비대상 — 게이트 밖 백로그** | J-mode 메모리는 엔진 자신의 것, 엔진-on-wasm64는 §1.3 조건부. S1이 "기계적으로 못 탄다"를 뒤집었으므로 분류는 '외부 차단'이 아니라 정직하게 '미착수' [실측 S1-M3 판정] |
| ② | `scratch_addr: u32` + Refusal 타입 | **O-19 해소** | §2.2 #6·#7·#9·#10 — u64화 + addrtype별 경계(+§2.4 캡) |
| ③ | 주소 `I32Const` 방출 | **O-19 해소** | A64에서 `I64Const`(§2.2 #2·#3). mem64에 I32Const는 validation error로 강제됨 [실측 S2-M1-2] |
| ④ | `call_indirect` 인덱스 i32 + 헬퍼 슬롯 u32 | **비대상 — 무변경 이월** | 혼합형 채택(§2.1)으로 v1에서 변경 자체가 불요 — 테이블 기계 1바이트 불변 [실측 S2-M1-3]. table64 채택 시 재개(폭만의 문제) |
| ⑤ | fn-ptr==테이블-인덱스가 wasm32-한정 실측 | **해소(성질 확장 실측) — 소비자는 게이트 밖** | S1-M3: wasm64/table64에서도 성립(레이아웃 동일, 0번 널 예약, 호스트 `table.get`==게스트 간접호출==네이티브 미러; 폭만 i64) [실측]. 실질 소비자(J-mode-on-wasm64)는 ①과 같은 백로그 — S1이 '조건부 이관' 권고를 철회했으므로 그대로 기록(이 귀속의 검증 가능성은 C0 증거 영속화가 담보 [v2, 패널 scope-3]) |
| ⑥ | 임포트 memory/table 32-bit 정의 | **부분 해소** | memory: 방출 측 `push_memory_import` A64 분기(§2.2 #5) + 호스트 수신측 `MemoryType::new64`(§4) [실측 S2-M4]. table 임포트는 JIT 표면 — ①과 함께 비대상 |
| ⑦ | `-Cpanic=unwind` 레시피 (rustc 문서상 wasm64 미지원) | **조건부 이관 — 게이트 밖** | S1-M1보너스: 핀 nightly-2026-08-31에서 빌드·링크·validate·런타임 catch_unwind 전부 그린, exnref tag `(param i64)` [실측] — 그러나 rustc 플랫폼 문서는 여전히 미지원 선언 → **"핀-실측 그린, 업스트림 계약 아님"**. 채택 시 기존 W-4 2레인 카나리아 규율 편입이 전제조건. 해소 조건은 §1.3 |

---

## §6. 테스트·뮤테이션

1. **기본값 골든-벡터 핀**(§2.3 v2): A32 기본값 방출 == pre-O-19 HEAD 채취 골든 벡터(양 표면: dispatch-fn + compile_module 산물). C1부터 존재해야 한다 — 크로스-커밋 고정물만이 "파라미터 배관이 방출을 못 건드림"의 실제 증인 [패널 tech-M2 반영]. 진입점-간 무드리프트(O-17 동형)는 보조 단정.
2. **A64 차분 레인**: 동일 IR → A32/A64 이중 방출 → 양쪽 wasmtime 실행 값 동일 + 네이티브 오라클 일치(기존 differential 하네스 패턴 재사용) + **modulo-addrtype 차분 게이트**(§2.6: 차이는 §2.2 열거 지점에만) + **A64 corpus의 R-게이트 불포함 명시 단정**(§2.6) + **table64 프로브 벡터**(§4). 헬퍼 임포트 형상은 pre-link + 네이티브 스텁(S2-M4 형상).
3. **구조 테스트 addrtype-조건부화**: `refusal_and_structure.rs:256,266,357` — A32에서 `!memory64`/`!table64` 유지, A64에서 memory64∧!table64(혼합형) 단정. `aot.rs:2266,2279`는 무수정 유지.
4. **음성 테스트 전수**: (i) A64×shared → 거부 (ii) 호스트 명시 `wasm_memory64(false)` → A64 산물 거부(**Err 단정, 문자열 핀 금지** — §4 귀속 정정) (iii) 32-bit 호스트 메모리 → `incompatible import type` (iv) emit-aot A64 사이드카 → 명시 에러 (v) memory64 바이트 × `memory64:false`/구 manifest → record==live 게이트 거부 (vi) **J-mode 참조 호스트의 memory64-형상 모듈 거부 고정** [v2 — 패널 scope-5; **착지 정정(마감 리뷰 F1)**: 원문 기대값 "-1"은 오류 — 실측 거부 클래스는 **-2**(§5.2 화이트리스트는 섹션-id만 검사라 i64 memory *임포트*를 통과시키고, `Module::new`도 48 default-ON이라 파싱 통과 — 유일한 하중 층은 `Instance::new`의 임포트 타입 불일치). 핀 테스트 `memory64_shaped_module_is_refused_at_instantiation_not_parse`(hosts/otter-host-wasmtime, 마감 리뷰 라이더로 신설)가 3사실(파싱 통과·화이트리스트 통과·인스턴스화 거부)을 전부 고정 — wasmtime 범프가 임포트 매칭을 완화하면 시끄럽게 발화].
5. **경계 테스트**: memarg offset 2^32(mem64 유효 / mem32 거부 [실측 S2-M1-5]), A64 스크래치 2^63 캡 Refusal, scratch_base 상위 32비트 비영(非零) 주소의 I64Const 정확성.
6. **뮤테이션 스윕**: addrtype 분기 지점(I64Const↔I32Const, memory64 플래그, 경계 상수, 타입 테이블 폭)을 각각 뒤집었을 때 1·2·3이 잡는지 확인. [[check-completeness-periodically]] 규율에 따라 커밋 전 전 wiring 지점(타입 테이블·임포트 빌더·경계 검사·match arm) 스윕.
7. **T0 스모크 CI 레인**(§3.1) + 하네스 Config 변경 0 확인(45가 default-ON이므로 [실측 S2-M1-6] — 단 §4의 명시 옵트인은 e2e 코드에만).

---

## §7. 증분 분할 (각 1커밋)

| # | 증분 | 내용 | 게이트 몫 |
|---|---|---|---|
| C0 — **DONE**(4dfd725) | 스파이크 증거 영속화 [v2 신설 — 패널 scope-3] | S1 `run-final.log` + 아티팩트 목록 + S2 출력 매트릭스(M1~M4, 재실행 산출 포함)를 `otter-o19-spike-results.md`로 연구 라이브러리에 영속화(`otter-o13-w0-spike-results.md`·`otter-o16c-spike-results.md` 선례). **최우선 착수** — 스크래치는 세션-휘발, S2 로그는 현재 미보존 | 증거 위생 |
| C1 — **DONE**(6104656) | addrtype 배관 | `AotOpts.with_memory64` + `Types` addrtype 주입 + u32→u64 폭 변경(§2.2 #1,#6~#10; legacy 진입점 u32 존치 — tech-N1) — 방출 무변화, **골든-벡터 핀 동반**(벡터는 착수 직전 pre-O-19 HEAD 채취, §2.3 v2) | (a) 전반부 |
| C2 — **DONE**(6104656 — C2a/C2b 분할 없이 1커밋, §9-3 이월 판단은 착수 시 '비분할'로 닫힘) | A64 방출 본체 | I64Const 주소(#2,#3), memory64 임포트(#5), §2.4 경계, shared×A64 거부(#11), 구조 테스트 조건부화 + **record==live A64 명시 면제 + modulo-addrtype 차분 게이트 + R-게이트 불포함 단정**(§2.6). 게이트 작업 추가로 1커밋 규모 초과 시 C2a(방출 본체)/C2b(게이트 정합) 분할 [§9-3] | (a) 완결 |
| C3 — **DONE**(2026-09-04 — otter-aot-e2e `a64-diff` 모드, `wasm-aot-e2e` 레시피 확장 편승 + reference_config 무드리프트 단정 테스트) | L1 실행 왕복 | otter-aot-e2e에 A64 차분 레인 + 단일-Config 빌더 memory64 파라미터화(기존 레인 무변경 단정) + 음성 대조(Err 단정) + **table64 프로브 벡터**(§4) | (c) 완결 |
| C4 — **DONE**(2026-09-04 — `just wasm64-smoke` + CI wasm-eh-pinned 스텝(전용 target dir, 러닝 후 제거) + O-13 §7.1 범프 절차 6항 편입) | T0 스모크 | justfile `wasm64-smoke` 레시피 + CI 레인(§3.1) — 컴파일 카나리아(§1.2 (b) 검증 수준) | (b) 완결 |
| C5 — **DONE**(2026-09-04 — 사이드카 `memory64` `#[serde(default)]` + linked memory64 교차술어 + F2 음성 2건(tests/o19_c5_gates.rs)) | L-2 정합 | manifest `memory64` 필드(사이드카 `#[serde(default)]`, `addr:u32` 존치 — §2.5) + record==live 대조 + emit-aot A64 사이드카 명시 거부 + **check --linked 교차술어 memory64**(§2.5) | 방어선 |
| C6 — **DONE**(2026-09-04 — 연구문서 :63/:64 + 5행(:130·:131·:186·:188·:204) + 의존그래프 행 정정, lib.rs compile_error! 뉘앙스 정밀화, 본 Status 갱신; roadmap.md의 wasm64 T3/T0-전용 서술은 엔진-배포-티어 주장이라 무수정 정합 확인) | 문서 정정 — **전수 스윕** [v2 확장 — 패널 MF-2] | 연구문서 스테일 행 전부: (i) :130 L0 행·:188 막힘 행 — "panic=unwind 미지원/불가(O-12 전제 파괴)" → "핀 nightly-2026-08-31 실측 그린(문서 스테일), 업스트림 보증 없음" (ii) :131 L1 행·:186 — "옵트인 플래그만"/"`Config::wasm_memory64` 옵트인" → "48.0.1·45.0.3 기본 수용 실측 — 단 48 config.rs:1220 문서는 여전히 'false by default' 선언, 실제 default-ON은 wasmparser 0.258 `WasmFeatures`(features.rs:227 `memory64=true`) 경유"로 **문서-실측 괴리 병기** [패널 scope-4 — §8-1 방어 근거 강화] (iii) **:204 O-19 행** — "32-bit 전제 7건 해소(주소 폭 파라미터화)" → 처분표(§5) 정합 문안("해소 2.5건 + 비대상/이관 4.5건, 게이트 몫 = (a)(b)(c)" [패널 대차 인용]) — 정본 2문서가 마지막 하드 게이트 내용에 대해 일치하도록 (iv) 본 노트 Status 갱신 | 정본 위생 |

의존: **C0 최우선(휘발성)** → C1→C2→C3, C4·C5는 C2 이후 임의 순서, C6은 최후. C3 그린 = 게이트 (c) 판정.

## §8. 위험

1. **wasmtime 기본값 재반전**: memory64 default-ON은 48.0.1/45.0.3 실측일 뿐 계약이 아님 — 특히 48 config.rs:1220 자체 문서가 여전히 "false by default"를 선언(실제는 wasmparser 0.258 기본 경유 [패널 scope-4 실물 확인])하는 문서-실측 괴리가 재반전 개연성을 높인다. §4의 명시 옵트인+음성 대조가 방어. [실측 기반]
2. **rustc wasm64 Tier 3 취약성**: 핀 범프 시 `-Zbuild-std` 빌드가 예고 없이 깨질 수 있음 — C4 스모크가 카나리아 역할. O-13 §7.1 핀 범프 절차에 wasm64 레인 재확인 항목 추가(값의 정본 = justfile `wasm-nightly`, §3.1). [추정]
3. **unwind 문서-실측 괴리의 양방향성**: 업스트림이 명시적으로 막을 수도(⑦ 재-차단), 지원 선언할 수도(엔진 백로그 개방) 있음 — 어느 쪽이든 §1.3 표의 해소 조건 재평가로 처리, O-19 게이트에는 무영향(스모크가 unwind 비의존, §3.1). [실측 기반]
4. **half의 wasm64 컴파일 성공 ≠ §3.4 b′ 헬퍼 오브젝트 빌드 가능**: 전자는 S1 실측, 후자는 여전히 [추정] — 단 v1 승인 형상이 pre-link+호스트 헬퍼 임포트라 O-19 범위에서는 미소비. A64×b′ 정적 링크는 어차피 차단(§2.5).
5. **§2.4 캡의 이론적 표현력 제한**: 2^63 초과 스크래치는 Refusal — 실용 손실 0 판단이나 완전 충실파의 이견 가능(§9-1).
6. **CI 디스크**: build-std 레인의 target 비대(S1 이탈 사례가 tmpfs 만포화) — CI 레인에 target 정리 스텝 동반. [실측 기반]

## §9. 열린 결정 (v2 — 2건 닫힘, 1건 이월)

1. **A64 스크래치 상한 = 2^63 캡 — 닫힘**: 패널(tech-N2)이 타당 판정("u64 전 구간 충분, checked_add, u128/표기 이원화 회피"). 캡 확정(§2.4), 완전 충실 대안 폐기.
2. **§3.2 guest-std 실행 스모크의 CI 상시화 — 닫힘(비상시)**: 게이트 (c)는 C3로 충족, (b)는 컴파일 카나리아로 §1.2에 명기 [패널 tech-S4 부분 채택 — C4 편입 대안은 상시 nightly+build-std 실행 레인 비용 대비 기각, S1 실행 실증은 C0 영속화 증거로 갈음].
3. **C2 분할(C2a 방출 본체 / C2b 게이트 정합) — 이월**: §2.6 게이트 작업이 C2에 추가되어 1커밋 규모 재평가 필요, 착수 시 판단 [패널 tech-N4].
