Status: DRAFT (2026-09-03, 오너 검토 전)

# 대체 WASM 런타임 코드 차용/참고 가능성 리서치 — Wasmi / WasmEdge / WAMR (vs wasmtime 기준)

4축 병렬 조사(wasmi / wasmedge / wamr 웹 라이브 실측 + repo-fit 레포 실측)의 종합. 표기 규율:
**[실측]** = 1차 출처(저장소/헤더/문서/API/레포 소스) 직접 확인, **[실측(레포)]** = 이 저장소 HEAD 직접 확인,
**[추정]** = 간접 추론/제작자 주장, **[판단]** = 종합자의 평가. 축 간 모순은 §부록에 봉합 없이 기록.

---

## §0 오너 지시 기록

> wasmtime 이외 런타임(특히 Wasmi / WasmEdge / WAMR)에서 코드 차용/참고 가능성을 리서치하라.

배경 맥락 [실측(레포)]:
- B2(`crates/wasm`, neosurf-wasm)는 S0-A/B/C까지 커밋 완료, **다음 증분 = S0-D 검증기+FuncLayout**, W1(runtime lowering) 미착수.
- otter O-13 wasm-emit은 wasmtime 45 참조 호스트(`hosts/otter-host-wasmtime`, 552줄)로 `just wasm-e2e` 7항 그린.
- 질문의 실질: ① 대체 참조 호스트가 성립하는가(H1–H7), ② B2/otter의 어느 증분에 무엇을 어떤 형태(벤더링/기법/오라클)로 가져올 수 있는가.

---

## §1 런타임별 프로파일 (아키텍처·라이선스·활동성·Rust 임베딩)

### 1.1 Wasmi (wasmi-labs/wasmi) — 순수 Rust 인터프리터
- **버전**: **2.0.0 (2026-09-01, 이틀 전 릴리즈)** — 실행 엔진 전면 재설계, v1.0 대비 geomean ~2.2x(Apple M2 Pro, **제작자 벤치**) [실측: CHANGELOG/releases/v2.0 블로그].
- **아키텍처**: v0.32(2024)에서 스택→**레지스터 IR 단일 패스 재작성 인터프리터**로 전환(상수 전파+op-fusion, Eager/Lazy/LazyTranslation 3모드, `Module::new_unchecked`, JIT-bomb 내성 선형 번역) [실측: v0.32 블로그, PR #367]. v2.0: 어큐뮬레이터 3-레지스터(ireg/freg32/freg64) IR, 64-bit 고정 스택 셀, 디스패치 4종+`auto-dispatch`, lock-free append-only CodeMap, 단일 연속 DSA 인스턴스 할당 [실측: v2.0 블로그].
- **no_std**: `wasmi`/`core`/`ir`/`collections` 전부 `#![no_std]`+alloc, std는 opt-in feature; `Memory::new_static`, `prefer-btree-collections`, feature 해제 시 200–300kB 감축 [실측: 각 lib.rs, CHANGELOG]. 단 `wasmi_wasi`는 std 전용(wasmtime `wasi-common` v36 어댑터 991 LOC) [실측].
- **규모**: 엔진 47.3k LOC(번역기 15.9k+실행기 8.5k) / `wasmi_ir` 7.2k / `wasmi_core` 7.6k / `wasmi_collections` 4.3k [실측 LOC 카운트]. 디코더/검증기는 상류 `wasmparser 0.228`(main 고정; 1.x는 0.239) [실측].
- **활동성**: 연 ~16 릴리즈, 사실상 1인 유지(Robbepop 1,166 커밋), Stellar(Soroban) 펀딩+프로덕션 채택, 2024-11 보안 감사 [실측]. 리스크: 단일 유지자·단일 스폰서 [추정].
- **라이선스**: **MIT OR Apache-2.0** [실측: crates.io + LICENSE 파일]. MSRV 1.86/edition 2024 — neosurf 툴체인(stable ≥1.96) 호환 [실측].
- 부수: 순수 Rust 인터프리터 공간에서 성숙도·성능·케이던스 독주 — tinywasm(587★, 활발하나 하위 호환급 [추정]), stitch(~9개월 휴면 [실측]) [실측].
- 출처: github.com/wasmi-labs/wasmi (README 지원표·CHANGELOG·releases) · wasmi-labs.github.io/blog v2.0/v0.32/v1.0 · issues #1137/#1514 · docs.rs/wasmi · crates.io/crates/wasmi · 로컬 클론 소스 실측(main @ 2026-09-01).

### 1.2 WasmEdge (WasmEdge/WasmEdge) — C++ / LLVM AOT·JIT
- **버전**: 안정 0.17.1(2026-07), 개발 **0.18.0-alpha.1(2026-08-28)** — EH-AOT/JIT 완성, lazy JIT [실측: releases/Changelog]. CNCF **Sandbox**(2021 가입, 미승급) [실측].
- **아키텍처**: C++ 코어(cxx11 ABI 고정). LLVM AOT(네이티브 .so 또는 **universal wasm binary** = wasm 커스텀 섹션에 네이티브 코드 내장, AOT 바이너리 v3) + LLVM JIT + 0.18-alpha lazy JIT. **0.17.0부터 기본 실행 모드 = 인터프리터**(`--run-mode=aot` 명시 opt-in) [실측: Changelog].
- **컴포넌트 모델**: 로더 완성(0.17.0)·밸리데이터 광범위·**실행기는 맹아** + WASI p2/p3 부재 → 2026-09 현재 실제 CM 실행/wasip2 호스팅 불가 [실측: Changelog grep + 공식 문서].
- **Rust SDK**: 공식 wasmedge-rust-sdk — 저장소는 활발(당일 커밋)하나 **crates.io 발행 21개월 공백**(sdk 0.14.0/2024-06이 마지막) → git-dependency 강제. async는 async-wasi로 지원, **no_std 불가**(C++ 링크) [실측: crates.io API + GitHub API].
- **확장점**: C API host module + 공식 플러그인 ABI(`wasmedge_plugin.h` 219줄), 0.18-alpha부터 버전드 심볼(`WASMEDGE_0.16/0.17/0.18`) [실측: 헤더].
- **라이선스**: **Apache-2.0**(SPDX 헤더 명기) [실측].
- 출처: github.com/WasmEdge/WasmEdge releases/Changelog · wasmedge.org/docs proposals · include/api/wasmedge/*.h 직접 확인 · crates.io API · cncf.io.

### 1.3 WAMR (wasm-micro-runtime) — C / 임베디드 지향 4-모드
- **저장소 이전**: bytecodealliance → **wasm-micro-runtime org**(구 URL 리다이렉트, README "arrived to its new home") [실측]. 최신 **WAMR-2.4.5(2026-06-29)** — CVE 3건 보안 픽스 전용(WASI poll_oneoff 힙 오버플로 등); main은 당일도 push [실측].
- **아키텍처**: classic interp / **fast interp**(로드타임 슬롯-ID 스택 시뮬레이션→내부 IR, 4-오퍼랜드 공간, 바이트코드 퓨전, LEB128 프리디코드; 벤더 주장 CoreMark +150% [실측=벤더 공표치 인용]) / AOT(고유 파일 포맷, **XIP 모드** = flash/ROM 무재배치 실행) / JIT(Fast JIT+LLVM JIT+Multi-tier, 공식 성숙도 Tier C) [실측: Intel 설계 문서 + tiered_support.md].
- **풋프린트**: interp ~56–59K / AOT 런타임 ~29.4K / libc-builtin ~3.7K; Zephyr·NuttX·ESP-IDF·bare-metal, Xtensa/RISCV/ARC 등 [실측=벤더 공표치].
- **컴포넌트 모델**: 공식 로드맵 없음; 외부 기여 파서-전용 PR #4889(open, CI 비활성, 병합 불투명) [실측]. — 기존 레포 기록 `otter-wasm-scope-escalation-research.md:42` "WAMR은 CM 미지원 [실측]"과 일치 [실측(레포)].
- **Rust SDK**: 공식이나 얇음 — **crates.io 404**(git 의존만), 테이블 접근·async 미노출, std 전제 [실측/추정]. otter 계약 수준은 C 헤더 직접 FFI 필요 [실측].
- **라이선스**: **Apache-2.0 WITH LLVM-exception** [실측: README].
- 출처: github.com/wasm-micro-runtime/wasm-micro-runtime (stability_wasm_proposals.md · build_wamr.md · tiered_support.md · wasm_c_api.md · xip.md · multi_module.md · shared_heap.md · wasm_export.h) · issues #3753/#3685/#2144 · PR #4949(unmerged)/#4889 · Intel fast-interp 설계 문서.

### 1.4 wasmtime 45 (기준) — 별도 웹 축 없음(빈 축 명시)
이번 워크플로에 wasmtime 전용 조사 축은 없었다. 기준 열의 판정은 전부 **레포측 실측**(참조 shim 실물 + `just wasm-e2e` 7항 그린 + 설계 정본 `otter-o13-wasm-emit-design.md`)에서 도출 [실측(레포)]. wasmtime 최신 동향(46+/48)은 O-16 항목으로 별도 추적 중.

---

## §2 호스트 능력 교차표 (H1–H7)

부기 [실측(레포)]: 문자열 "H1..H7" 체크리스트는 레포에 존재하지 않는다(grep 0건) — 이는 리서치측 구성물이며, 각 행의 규범 정본은 `otter-o13-wasm-emit-design.md` §5.1–§5.5와 `hosts/otter-host-wasmtime/src/main.rs`다.

| # | 요건 | wasmtime 45 (기준) | Wasmi 2.0.0 | WasmEdge 0.17.1 / 0.18-alpha | WAMR 2.4.5 |
|---|---|---|---|---|---|
| **H1** | **exnref 정본 EH** (`try_table`/`throw_ref`) | ✓ — `Config::wasm_exceptions(true)` + crate feature `gc`+`gc-drc` 전제, load-bearing [실측(레포): shim:109-116, 설계:311] | **✗ 미구현** — Tag 임포트에서 `panic!`(import.rs:85), `EXCEPTIONS` WasmFeature 자체 부재, 이슈 #1137 2년+ 미착수 [실측] | **△** — 인터프리터 ✓(0.14+, 0.16부터 default-on, legacy EH는 제거·정본 단일화); **AOT/JIT은 0.18.0-alpha.1에서야 완성**(0.17.x는 `AOTNotImpl` 실패) [실측: Changelog] | **✗ 미구현** — 공식 문서 "unimplemented"; legacy EH만 classic-interp 전용·기본 off; exnref 구현 PR 0건 [실측] |
| — | **H1 판정** | 기준 | **현행 otter 게스트 구동 자체 불가** — §7 빌드 산물은 feature-off여도 exnref 포함(W-4 실물 try_table 397개소, 설계:256·:431) → 파싱/임포트 단계 거부 [실측(레포)+실측] | 인터프리터 호스팅만 안정판에서 가능; **JIT-모드 호스트는 0.18 계열 필수** [실측+판단] | **현행 otter 게스트 구동 자체 불가** — 로더가 exnref 타입/try_table 옵코드 미인지로 파싱 실패 [실측 근거+확실한 추론] |
| **H2** | WASI p1 (p2/p3/CM) | p1 ✓ (wasmtime-wasi) [실측(레포)]; CM 성숙 [추정—축 부재] | p1 ✓ **std 전용**(wasi-common v36 어댑터); no_std에선 WASI 불가; p2/CM 부재 [실측] | p1 ✓ 성숙; **p2/p3 언급 0건**; CM 실행기 미완 [실측] | p1 ✓(libc-wasi 내장 ~21.4K); p2는 RFC만(#3685); CM 파서 PR만 [실측] |
| **H3** | `otter_jit.compile` 계약(재진입 **동기** 컴파일·인스턴스화, memory/table 임포트 배선, funcref **table grow-with-init**, 섹션 화이트리스트 가능) | ✓ — 참조 shim 축자 구현 + E2E 7항 그린 [실측(레포)] | **✓(API 전부 존재)** — wasmtime-미러 API(Module::new/Linker/Instance/Table::grow/Memory) 실측; 8단계 이식은 "기계적" [실측(API)+**추정**(grow-with-init의 init 파라미터는 미실측 — §부록 모순3)] | ✓ — 8단계 전부 공식 C API로 매핑 확인(Store 명명 등록→import 해석, TableInstanceGrow/SetData, ValueGenFuncRef); 단 grow+set 2단계형 → 널-슬롯 창 재검토 필요 [실측: 헤더+판단] | **✗ 부분탈락** — **호스트측 table grow/write API 부재**(wasm-c-api 문서가 명시 부정); multi-module은 load-time 링킹 전용·JIT 미지원; 게스트측 헬퍼 우회는 계약 재설계 [실측] |
| **H4** | tail-call (`return_call*`, O-14 요건) | ✓ [실측(레포): E2E 1M 꼬리재귀] | ✓ — ≥0.28, 기본 활성 [실측] | ✓ — 0.10+, 0.16 default-on; **단 호스트/크로스모듈 경계 버그가 0.17.2-alpha/0.18-alpha에서 갓 수정** [실측] | ✓ — `WAMR_BUILD_TAIL_CALL`(기본 off); 모드 제한 명시 없음 [실측] / 전 모드 동작은 [추정] |
| **H5** | threads / memory64 / GC (O-17 대비) | threads·GC 지원 [추정—축 부재]; v1 계약은 비공유 메모리 한정 [실측(레포): 설계:267] | **threads ✗(#777)·GC ✗(#775)·func-ref ✗(#774)** / memory64 ✓·simd ✓·relaxed-simd ✓ [실측] → **O-17 불가** | threads ✓(0.10.1+, 비default) / memory64 ✓(0.17 default-on) / GC ✓(0.16 default-on, 검증 수정 지속) [실측] | 전부 있으나 **모드 편차 큼**: memory64=classic interp+AOT 전용, GC=fast-jit 미지원+`exn` 타입 미지원(EH 교차 불가), threads 기본 off [실측] |
| **H6** | Rust 임베딩 SDK | wasmtime crate — 성숙, crates.io, 락스텝 핀(45↔cranelift 0.132) [실측(레포)] | **네이티브 Rust 그 자체**·no_std ✓·crates.io ✓·MSRV 1.86 호환; async ✗(대신 resumable calls+fuel) [실측] | 공식 SDK, 코드 활발하나 **crates.io 21개월 공백 → git-dep 강제**; API 불안정 자기 고지; no_std ✗ [실측] | 공식이나 얇음: **crates.io 404**, 테이블 API 미노출 → C FFI 직접 필요; no_std 언급 없음 [실측] |
| **H7** | 라이선스 (레포 = BSD-2-Clause-Patent, 퍼미시브 인바운드만 [실측(레포)]) | Apache-2.0 WITH LLVM-exception [추정—축 부재, 통념] | **MIT OR Apache-2.0** — 벤더링/차용 자유(고지 유지) [실측] | **Apache-2.0** — 이식·파생 자유(고지); 단 C++라 문자 벤더링 착지면 부재 [실측+실측(레포)] | **Apache-2.0 WITH LLVM-exception** — 동일; C라 착지면 부재 [실측+실측(레포)] |

**H1 종합 판정 [판단]**: exnref는 대체 호스트 자격의 **선결 하드 게이트**(설계:256 — "EH 지원은 §7 빌드 산물의 전제")다. **Wasmi와 WAMR는 H1 단독 탈락으로 현행 otter 게스트를 아예 못 돌린다**(파싱 단계 거부, 부분 실행조차 불가). WasmEdge만 인터프리터 한정으로 안정판 통과, JIT-모드는 0.18-alpha 의존. 우회로(게스트 `-Cpanic=abort` exnref-프리 변형)는 §7 빌드 계약과 O-12 fail-closed 전제를 건드리므로 오너 결정 사안(§6-1).

---

## §3 차용 평가 — 서브시스템 × 형태 판정

형태 정의: **벤더링** = 코드 파일 직수입(라이선스 고지 동봉) / **기법 참고** = 설계·알고리즘·버그 이력의 재구현 / **차등 오라클** = 테스트 인프라의 비교 대상 / **부적합**.

레포측 수용 제약(전부 [실측(레포)], repo-fit ③): 코어 착지면은 no_std+`forbid(unsafe_code)`+무패닉+의존 0(`crates/wasm` 명문 정책); 에러는 `WasmError{kind,offset}` 3버킷 번역 필수; 무거운 의존은 dev-dep 또는 워크스페이스-제외 `hosts/`·`demos/`만; C/C++는 **문자 벤더링 착지면이 레포에 없음**.

| 서브시스템 (착지면) | Wasmi | WasmEdge | WAMR | 판정 근거 |
|---|---|---|---|---|
| **B2 S0-D 검증기+FuncLayout** (`crates/wasm/src/validate.rs` 신설 — 다음 증분) | **기법 참고**(자체 검증기 엣지케이스: 스택-다형성, br_table arity, dead-code 라벨) + 코드 직수입은 3버킷/offset/무패닉으로 재작성 필요 | 기법 참고(검증 수정 이력 = 함정 목록) | 기법 참고(하위) | 알고리즘 자체는 설계가 이미 스펙 부록 방식 지정 [실측(레포)] — 차용 가치는 코드가 아니라 **경계 사례** [판단]. 판정 오라클로는 이미 트리에 있는 wasmparser 0.258이 최저비용(신규 의존 0) [실측(레포)] |
| **B2 W1 스택→레지스터 번역** (`crates/runtime/src/wasm_frontend.rs` 신설) | **기법 참고 최상급** — v0.32/v2.0 번역기(15.9k LOC)가 "동일 언어·동일 문제"의 유일 선행 사례; `height_before`→`stackbase+k` 설계와 동형 [실측+추정(동형성)]. `wasmi_ir`(7.2k, no_std)은 **조건부 벤더링 후보** — 단 의존-0 정책과 충돌, W1 설계 확정 후 판단(§6-3) | 부적합(LLVM 코드젠 — 방향 상이) | 기법 참고 — fast-interp 슬롯-ID/4-오퍼랜드/퓨전 [실측] | 엔진 본체는 결합도 높아 부분 벤더링 비추 — 설계 이식이 정답 [판단, wasmi 축 동일 결론] |
| **B2/W1 결정적 수치 의미론** (`wasm_ops.rs`+`wasm-trapbridge` 신설) | **차등 대조+부분 벤더링 후보** — `wasmi_core`(7.6k, no_std, UntypedVal·수치·트랩) | 기법 참고 | 차등 대조(SIMDe/libm 처리) | nearest(half-even)/NaN min-max/trunc_sat/narrow-first가 대조 최고 가치 지점 [실측(레포): 설계 :67-76] |
| **B2 스펙테스트/퍼즈** (`crates/wasm/tests/` + dev-dep) | **차등 오라클(중)** — 순수 Rust dev-dep 1개, cmake 무, `--locked` 무충돌; W1 하네스 신설 시 +1 오라클 층 | 부적합(오라클로는 비용 최고 — C++ 빌드시스템) | 부적합(동일) | .wast 러너 구조+assert_* → 3버킷 매핑은 dev-dep 전용으로 차용(라이브러리 의존 0 유지) [실측(레포)] |
| **otter T0·백엔드 실행 루프** | **기법 참고** — v2.0 디스패치 4종+auto-dispatch, 64-bit 셀, lock-free CodeMap, **Rust 1.92 DestinationPropagation 함정**(CoreMark ~50% 사례) | 기법 참고(exnref 인터프리터 구현+버그 이력: try_table 핸들러 수명, stale handler drop #5252, exnref payload 보존) | 기법 참고 — fast-interp 프리디코드·핸들러 프리바인딩·classic/fast 2모드 병행 전략(B2의 "정확성 레퍼런스+고속 경로" 구도와 동형) | 전부 재구현 참고 — 코드 직수입 대상 아님 [판단] |
| **otter AOT 모드 (O-14)** | 해당 없음(AOT 없음) | **기법 참고** — universal wasm binary(커스텀 섹션 내장, 버전드) + run-mode 게이팅(기본 인터프리터, AOT 명시 opt-in) = 산출물 포맷·안전 기본값의 직접 선례 | **기법 참고** — XIP/indirect-mode(무재배치 AOT: 함수 포인터 테이블 간접호출+인트린식 런타임 치환)가 O-14/O-15 문제 설정과 정확히 일치, "임베디드 지향" 유일 선행 사례 | [실측: 각 문서] + [판단] |
| **임베디드 호스트 시나리오** (`hosts/otter-host-<runtime>` 신설) | **조건부 유일 후보** — exnref-프리 게스트 변형 전제 시 no_std JIT-모드 제2 호스트로 유일 현실 후보; O-17(threads)과는 교차 불가 | 부적합(no_std 불가, C++ 링크) | 러너로는 **부적합**(H1+H3 이중 탈락); Tier-C bare-metal 잔존 가치는 abort-프로파일+O-14 wasm 출력 전제의 장기 저순위 [추정] | 실질 권고: 임베디드 호스트 역할은 **자체 B2 엔진이 담당**, WAMR는 풋프린트 벤치 기준선 [판단, wamr 축 결론 채택] |
| **CM+JIT 사이드채널 단일 호스트** | 부적합(H1) | **조건부 2순위, 현시점 채택 불가** — JIT면은 0.18.0 안정판이 재평가 트리거; CM면은 실행기 미완+p2 부재로 불가 → CM은 wasmtime 유지 | 부적합(CM 로드맵 부재) | "CM+코어 사이드채널 동시 제공"은 어느 대체 런타임으로도 충족 불가 [판단, 3축 일치] |

**라이선스 호환성 교차(레포측 BSD-2-Clause-Patent와) [실측(레포)+실측]**:
- Wasmi(MIT OR Apache-2.0): 벤더링·부분 차용 모두 걸림 없음. 조건 = LICENSE 사본+저작권 고지 벤더 디렉터리 동봉. 파서 경유 시 wasmparser는 별도 `Apache-2.0 WITH LLVM-exception`(호환, 고지 추가).
- WasmEdge(Apache-2.0)/WAMR(Apache-2.0 WITH LLVM-exception): 알고리즘 이식·파생 자유(NOTICE/고지 의무만). 단 C/C++ → 레포에 문자 벤더링 착지면 자체가 없고, WAMR는 2026년 CVE 3건(fast-interp/로더 계열 반복) — C 코드 직수입은 보안 표면 수입 [실측] → **"설계·알고리즘·테스트벡터 참고 한정"이 실질** [판단, 2축+레포측 일치].

---

## §4 권고 상위 5건 + 비권고

### 권고 (우선순위순)

1. **[차등 오라클, 즉시] S0-D 검증기 판정 오라클 배선** — 이미 트리에 있는 `wasmparser 0.258`(otter-wasmemit dev-dep + Cargo.lock)을 `crates/wasm` dev-dep로 확장해 `decode()`+신설 `validate.rs` 판정 vs `wasmparser::validate` 판정의 **분류-차등**(malformed/invalid/unsupported 3버킷 대조)을 S0-D와 동시 착지. Wasmi 자체 검증기의 엣지케이스(스택-다형성 pop, br_table arity, dead-code 라벨)를 테스트벡터로 이식. **신규 라이브러리 의존 0, 예상 규모: 테스트 파일 1–2개 + 벡터 수십 건** [실측(레포): 비용 최저 확인].

2. **[기법 참고] Wasmi v0.32/v2.0 번역기 설계 이식 → W1** — 스택→레지스터 단일 패스(상수 전파, op-fusion, Eager/Lazy/unchecked 모드, JIT-bomb 내성 선형 번역)를 `crates/runtime/src/wasm_frontend.rs`(신설, W1.0–W1.4) 설계에 반영. 코드 직수입 없음 — 블로그 2편+번역기 소스(15.9k LOC) 정독 기반 설계 차용. 유일한 "Rust·레지스터 머신·동일 문제" 선행 사례. **예상 규모: 설계 문서 증보 + W1 구현 시 상시 참조** [실측+추정(동형성)].

3. **[차등 오라클] Wasmi 2.0을 W1 실행 오라클로 dev-dep 추가** — W1 크로스-티어 하네스(계획: otter interp/baseline/cranelift) 위의 **+1 외부 오라클**. 순수 Rust 단일 dev-dep, cmake/네이티브 lib 무, hermetic CI(`--locked`) 무충돌. 주의: wasmi main의 wasmparser 0.228과 레포 0.258의 중복 버전 공존 관리(§부록 모순2). **예상 규모: dev-dep 1 + 하네스 오라클 층 1개(W1 하네스 신설과 동시)** [실측(레포)+실측].

4. **[기법 참고] WasmEdge exnref 구현+버그 이력 → EH 함정 목록** — legacy 제거·정본 단일화 선례, try_table 핸들러 스택 수명(branch 시 stale handler drop #5252), exnref payload 보존, EH×AOT 프레임 간 예외 전파(#5168). B2가 향후 EH 디코딩/검증으로 스코프 확장할 때와 otter 게스트 §7 빌드 검증 양쪽의 **"뭘 틀리기 쉬운가" 체크리스트**로 채록. C++ 코드 직수입 불가 — 이슈/커밋 이력 채굴 형태. **예상 규모: 함정 목록 문서 1건(연구 라이브러리)** [실측+판단].

5. **[기법 참고] WAMR XIP + WasmEdge universal-wasm/run-mode → O-14 AOT 산출물 포맷 설계 입력** — 무재배치 AOT(함수 포인터 테이블 간접호출, 인트린식→런타임 함수 치환), AOT 산출물의 wasm 커스텀 섹션 패키징+버저닝, "기본 인터프리터·AOT 명시 opt-in" 안전 게이팅. O-14 설계 착수 시 선행 사례 절 필수 반영. **예상 규모: O-14 설계 문서의 선행기술 절** [실측: xip.md·Changelog].

### 비권고 (하지 말 것)

- **WAMR/WasmEdge C·C++ 코드 벤더링 또는 FFI 링크**: 레포에 문자 착지면 부재(no_std+forbid(unsafe) Rust) + WAMR 2026년 CVE 3건의 보안 표면 수입 + 스택 방침·safety 논거 충돌 [실측(레포)+실측].
- **WAMR를 otter_jit 대체 호스트로 채택**: H1(exnref 파싱 실패) + H3(호스트측 table grow API 부재) 이중 탈락 — 우회는 계약 재설계 비용 [실측].
- **WasmEdge 0.17.x를 JIT-모드 호스트로 채택**: exnref×AOT/JIT가 알파에만 존재(`AOTNotImpl`), tail-call 경계 버그 갓 수정, Rust SDK git-dep 강제. **재평가 트리거 = 0.18.0 안정판** [실측].
- **wasmi 엔진 본체(47k)의 부분 벤더링**: 결합도 높음 — 통짜 dev-dep(오라클) 또는 설계 이식만 [실측+판단].
- **exnref-프리 게스트 변형을 오너 결정 없이 도입**: §7 빌드 계약(설계:256)과 O-12 fail-closed 전제를 건드림 — §6-1로 상신 [실측(레포)+추정].

---

## §4b 오너 결정 (2026-09-03): §4 권고 상위 5건 **전량 수용**

§4의 1–5번 전부 채택 확정. 집행 시점: ①=S0-D 증분과 동시, ②③=W1 설계/하네스 신설 시, ④=문서
작업이라 임의 시점(연구 라이브러리에 함정 목록 문서 신설), ⑤=O-14 설계 문서의 선행기술 절. 비권고
목록은 그대로 기각 유지. 오너 지시로 6위 이하 확장 목록을 §7에 신설.

## §7 확장 후보 목록 — 6위 이하 (오너 요청, 2026-09-03)

상위 5건에 밀려 종합에서 탈락했던 후보 전량을 4축 원본에서 회수해 가치/비용 순으로 재배열. 형태
표기는 §3과 동일. **전부 미채택 상태** — 각 착지 증분 시점에 개별 채택 판단.

| # | 후보 | 출처 축 | 형태 | 착지 | 비고 |
|---|---|---|---|---|---|
| 6 | **.wast 스펙테스트 러너 구조** — 공식 testsuite의 assert_malformed/invalid/trap 분류를 우리 3버킷 에러 모델로 매핑, dev-dep 전용(`wat`은 이미 트리) | repo-fit | 기법+dev-dep | `crates/wasm/tests/` (S0-D~W1) | repo-fit 축 자체 Top5의 3위였으나 종합 컷 탈락. 비용 낮음 |
| 7 | **`wasmi_core` 수치 의미론 단위 대조(+부분 벤더링 후보)** — UntypedVal·nearest(half-even)·NaN min/max·trunc_sat·트랩 (7.6k LOC, no_std) | wasmi·repo-fit | 차등 대조·조건부 벤더링 | `wasm_ops.rs`+`wasm-trapbridge` (W1.1/W1.3) | 권고 3(엔진 통짜 오라클)과 별개의 **단위-수준** 대조 |
| 8 | **WAMR fast-interp 사전 변환 파이프라인** — 슬롯-ID 스택 시뮬레이션, 4-오퍼랜드 공간, 바이트코드 퓨전, LEB128 프리디코드, 핸들러 프리바인딩 + classic/fast 2-모드 병행 전략 | wamr | 기법 참고 | B2 실행기·otter T0 | WAMR 축 자체 Top3의 1위였으나 종합에선 XIP만 채택됨. 벤더 공표 CoreMark +150% |
| 9 | **Wasmi v2.0 실행 루프 엔지니어링 일습** — auto-dispatch 4종, 64-bit 고정 셀, 어큐뮬레이터 3-레지스터 IR, lock-free append-only CodeMap, 단일 연속 DSA 인스턴스 할당 | wasmi | 기법 참고 | otter T0·B2 인터프리터 | 권고 2(번역기)와 별개의 **실행기측** 노하우. Rust 1.92 DestinationPropagation 함정 → 툴체인-범프 회귀 목록에 디스패치 벤치 편입(즉시 실행 가능한 소항목) |
| 10 | **디코더 퍼즈 + wasm-smith 활용 패턴** — 현행 `crates/wasm` 퍼즈 0 상태 해소 | repo-fit | 기법+dev-dep | `crates/wasm` 퍼즈 신설 (S0-D 이후) | otter-wasmemit의 LCG+CFG 퍼즈와 상호 보완 |
| 11 | **Wasmi 번역 모드 설계** — Eager/Lazy/LazyTranslation + `Module::new_unchecked`(검증이 스타트업 20–40%), JIT-bomb 내성 선형 번역 | wasmi | 기법 참고 | B2 스타트업 설계 | 권고 2에 부분 포함되나 스타트업-지연 축은 독립 가치 |
| 12 | **WasmEdge 런타임 링킹 모델 청사진** — Store 명명 등록→import 해석→funcref/table 조작의 공식 C API 흐름(`otter_jit` 계약과 동형) | wasmedge | 기법 참고 | 미래 제2 호스트(0.18 트리거 발화 시) | WasmEdge 축 자체 Top3의 2위. 트리거 전 채록만 |
| 13 | **WasmEdge lazy-JIT 설계** — 함수 첫 호출 시 컴파일, LLVM 모듈 배치 재사용, 코드주소 영속화 (0.18-alpha) | wasmedge | 기법 참고 | AOT+JIT 조합 지점(M3 프리히트/웜스타트) | 모드-아키텍처 결정 C의 착수 트리거 발화 시 선행 사례 |
| 14 | **WAMR 모듈러 풋프린트 엔지니어링** — 빌드 플래그 기능 게이팅(3.7K~59K), tiered_support 문서 체계 + WAMR를 임베디드 풋프린트 벤치 기준선으로 | wamr | 기법+벤치 기준선 | neosurf 임베더블 코어 전반·벤치 하네스 | WAMR 축 자체 Top3의 3위. 문서/정책 모델 |
| 15 | **WasmEdge 플러그인 ABI 버전드 심볼 설계** — `WASMEDGE_0.16/0.17/0.18` 하위호환 shim | wasmedge | 기법 참고 | `otter_jit.version`(예약) v2 계약 버전 협상 설계 | 계약 v2 논의 시점에 |
| 16 | **Wasmi resumable calls + fuel metering 설계** — async 부재의 대체 패턴 + 비용 커스터마이즈 | wasmi | 기법 참고 | B2 중단/재개·리소스 미터링 | 임베더블 코어의 실행 예산 제어 설계 |
| 17 | **Wasmi no_std 임베딩 장치** — `Memory::new_static`, `prefer-btree-collections`, feature 해제 시 200–300kB 감축 | wasmi | 기법 참고 | B2 no_std 임베딩 세부 | |
| 18 | **Wasmi를 otter-wasmemit 하네스 제2 실행체로** — §8(a)는 EH 불요라 H1 무관 성립 | repo-fit | 차등 오라클 | `crates/otter-wasmemit/tests/` | `Harness`의 wasmtime 타입 직결합 해체 비용 중상 — 저순위 |
| 19 | **WasmEdge GC/CM 밸리데이터 수정 이력 채굴** | wasmedge | 기법 참고 | 미래 GC/CM 확장 시 함정 목록 | 현행 스코프 밖 — 관망 |
| 20 | **OSS-Fuzz 운영 패턴** (WasmEdge 컴포넌트 퍼저) | wasmedge | 기법 참고 | CI 퍼즈 운영 | 저순위 |

**별도 분류 — 순위 밖 (선결 결정 대기, §6과 동일)**: `wasmi_ir`+`wasmi_collections` 벤더링(§6-3,
W1 설계 확정 시 재론 — no_std 자립 크레이트라 벤더링 자체는 자유), Wasmi 임베디드 제2 호스트(§6-1
exnref-프리 게스트 변형 오너 결정 선행), WasmEdge JIT-모드 제2 호스트(§6-2, 0.18.0 안정판 트리거).

## §5 진행 중인 모드-아키텍처 연구와의 교차점

(정본: `otter-wasm-scope-escalation-research.md`의 O-16/O-17 작업 목록, `wasm-w0w1-design.md` 스테이징 — 전부 [실측(레포)])

1. **O-17(shared-memory + atomics×exnref)과의 충돌**: Wasmi는 threads ✗(#777)·GC ✗ — O-17 이후의 게스트와 **구조적으로 교차 불가**. 따라서 "Wasmi 임베디드 제2 호스트" 노선은 O-17 이전 스냅샷 게스트 또는 별도 abort-프로파일에 한정되는 **시한부 선택지**다. O-17을 확정 추진한다면 대체 호스트 후보는 WasmEdge(threads ✓)만 남고, 그마저 no_std 불가 [실측+판단].
2. **O-16(wasmtime 46+/48 + wasip3×unwind 스파이크)**: 세 런타임 모두 WASI p2/p3 부재 [실측] → wasip3 방향이 확정되면 **대체 호스트 후보군 전멸** → wasmtime 단일 호스트 의존이 심화되고, 그만큼 **자체 B2 엔진(`crates/neosurf-wasm`)의 전략적 가치가 상승**한다. B2가 장기적으로 "우리가 통제하는 제2 실행체"가 되는 그림이 강화됨 [판단].
3. **§8(a) 차등 하네스의 제2 실행체**: otter-wasmemit 하네스는 EH 불요(설계:310)라 exnref 관문이 회피됨 — Wasmi를 방출-모듈 실행 차등의 제2 실행체로 쓰는 것은 H1과 무관하게 성립. 단 `Harness`의 wasmtime 타입 직결합 해체 비용(중상) [실측(레포)].
4. **인터프리터 루프 엔지니어링의 즉효 입력**: Wasmi v2.0의 Rust 1.92 `DestinationPropagation` MIR 최적화가 디스패치 분기 예측을 붕괴시킨 사례는 otter T0/B2 인터프리터와 **벤치 하네스(회귀 게이트)**에 직접 관련 — 툴체인 범프 시 디스패치 벤치를 회귀 목록에 포함할 근거 [실측: v2.0 블로그 + 레포 벤치 하네스 정책].
5. **O-14 AOT 산출물 포맷 결정**: §4-5의 WAMR XIP/WasmEdge universal-binary가 "wasm 출력 vs 네이티브 출력, 재배치 유무, run-mode 게이팅" 결정의 선행 입력. wamr 축의 잔존 가치 시나리오(Tier-C bare-metal에서 otter AOT 산출물 구동)는 O-14가 wasm 출력을 택할 때만 유효 [실측+추정].

---

## §6 오너 결정 필요 항목

1. **exnref-프리(-Cpanic=abort) 게스트 빌드 변형 신설 여부** — Wasmi(및 장기적으로 WAMR) 임베디드 호스트 노선의 전제 조건. 반대 논거: §7 빌드 계약 단일성 훼손 + O-12(catch_unwind fail-closed) 전제와 충돌 [추정, wamr 축]. 찬성 논거: no_std 임베디드 배포에서 유일한 JIT-모드 제2 호스트 확보 [판단, wasmi 축]. **결정 전까지 §4-3(오라클)까지만 진행하고 hosts/otter-host-wasmi는 착수 금지 권고.**
2. **WasmEdge 0.18.0 안정판 재평가 트리거 등록** — JIT-모드 제2 참조 호스트(다양성 검증용) 재평가를 0.18.0 안정 릴리즈에 걸지 여부.
3. **`wasmi_ir`/`wasmi_collections` 벤더링 vs 참고 한정** — 벤더링은 라이선스상 자유이나 `crates/wasm` 의존-0 명문 정책 및 자체 IR(otter 레지스터 IR) 존재와 긴장. W1 설계 확정 시점에 재론 권고 [판단].
4. **Wasmi 오라클의 wasmparser 이중 버전 수용** — §부록 모순2의 `--locked` 관리 비용 수용 여부(경미하나 버전 핀 규율 관련).

---

## 부록: 규율 준수 기록

### 빈 축
- **wasmtime 전용 조사 축 없음** — 기준 열은 레포측 실측(shim+E2E+설계 정본)으로만 구성. wasmtime 자체의 H5/H7 셀은 [추정] 표기함.
- 성능 수치는 전부 **벤더/제작자 공표치 인용**(Wasmi ~2.2x, WAMR CoreMark +150%) — 자체 측정 없음. 채택 결정 전 자체 벤치 하네스 실측 필요.
- WAMR wamrc의 native object/LLVM IR 출력 옵션은 이번 세션 미확인 [추정, wamr 축 자체 표기].

### 축 간 모순/미해소 (봉합하지 않음)
1. **WAMR Rust SDK의 canonical org 불일치**: wamr 축 H6 본문은 `wasm-micro-runtime/wamr-rust-sdk`(공식 org 이전)라 하나 동일 축 출처 목록은 `github.com/bytecodealliance/wamr-rust-sdk` 링크 — 리다이렉트로 추정되나 미확인 [추정].
2. **wasmparser 버전 트레인 3중 분기**: wasmi main=0.228 고정(1.x는 0.239) vs 레포 dev-dep=0.258 [실측 양측]. Wasmi를 dev-dep 오라클로 추가하면 Cargo상 중복 버전 공존(허용되나 `--locked`·버전 핀 규율의 관리 대상). wasmi 축은 이를 비용으로 계상하지 않았고 repo-fit 축의 "무충돌" 판정은 빌드 성립 기준 — 관리 비용 자체는 남는다 [판단].
3. **Wasmi H3 "8단계 이식 기계적" [추정] vs grow-with-init 요건**: repo-fit이 요구하는 W-5 확정형은 `table.grow(1, init=f)` 1단계(널-funcref 관측 창 소거)인데, wasmi 축은 `Table::grow`의 **init 파라미터 유무를 명시 실측하지 않았다**(set 가능만 확인). docs.rs상 wasmi `Table::grow`도 init 값을 받는 것으로 보이나 [추정] — hosts 착수 전 반드시 실측 확인.
4. **wasi-common 버전 격차**: wasmi_wasi가 감싸는 wasi-common은 v36.0.0 [실측], 레포 참조 호스트는 wasmtime 45 [실측(레포)] — 9개 메이저 트레인 차이. Wasmi 경유 WASI 동작이 wasmtime 45 대비 어긋날 수 있는 잠재 지점(현행 계약에선 WASI 직배선이라 영향 제한적) [판단].
5. **WAMR CM 판정의 시점 정합**: 기존 레포 기록("CM 미지원 [실측]")과 이번 웹 실측(파서-전용 PR #4889 open) 일치 — 모순 아님, 재확인 완료로 기록.

### 주요 출처 총람
wasmi 축: github.com/wasmi-labs/wasmi(README/CHANGELOG/releases/#1137/#1514) · wasmi-labs.github.io/blog(v2.0/v0.32/v1.0) · docs.rs/wasmi · crates.io · 로컬 클론 실측 · tinywasm/stitch repos.
wasmedge 축: github.com/WasmEdge/WasmEdge(releases/Changelog) · wasmedge.org/docs proposals · include/api/wasmedge/*.h · crates.io API(wasmedge-sdk/-sys) · api.github.com(wasmedge-rust-sdk) · cncf.io.
wamr 축: github.com/wasm-micro-runtime/wasm-micro-runtime(stability_wasm_proposals/build_wamr/tiered_support/wasm_c_api/xip/multi_module/shared_heap/embed_wamr/wasm_export.h) · #3753/#3685/#2144/#4949/#4889 · releases(2.4.5) · wamr-rust-sdk · Intel fast-interp 설계 문서.
repo-fit 축(전부 이 저장소 HEAD): `crates/wasm/src/*` · `.claude-research-library/wasm-w0w1-design.md` · `.claude-research-library/otter-o13-wasm-emit-design.md`(§5.1–5.5/§7/§13/§14) · `hosts/otter-host-wasmtime/src/main.rs` · `crates/otter-wasmemit/tests/*` · `Cargo.toml`/`LICENSE`/`justfile` · `.claude-research-library/otter-wasm-scope-escalation-research.md`.
