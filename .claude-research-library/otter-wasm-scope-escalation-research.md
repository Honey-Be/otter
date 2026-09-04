Status: DRAFT (2026-09-03, 오너 검토 전)

# otter wasm 스코프 상향(컴포넌트/WASIp3 · atomics/shared memory · wasm64) 리서치 종합

작성: 2026-09-03. 5개 조사 축([wasip3] [component-dynamic] [threads-atomics] [wasm64] [aot-gap]) 종합.
표기 규율: **[실측]** = 공식 소스/레포 라이브 확인, **[추정]** = 실측 기반 추론/미검증. 축 결과가 상충하면 §1.6에 기록하고 봉합하지 않음. 비어 있는 축 없음(5축 전부 조사 결과 수령).

---

## §0. 오너 지시 원문 기록 (2026-09-03)

1. **WASI 목표 상향**: 기존 O-13의 WASI 지원 목표를 **컴포넌트 모델 + WASIp3**로 상향한다.
2. **atomics + shared memory 목표 추가**.
3. **wasm32/wasm64 불문** 적용.
4. 오너 관찰: **"wasm-emit은 개념상 AOT"** — 이 관찰의 타당성 검증이 [aot-gap] 축의 과제였다(§3에서 판정).

---

## §1. 2026-09 생태계 실측 요약 (축별)

### §1.1 WASIp3 / WASI 0.3 ([wasip3] 축)

- **WASI 0.3.0 정식 릴리즈 2026-06-11** (0.3.1: 2026-08-11), 0.3.x 패치를 2개월 주기 후방호환 릴리즈 트레인으로 운영. [실측]
- 핵심 변화: native async(`async func`/`stream<T>`/`future<T>`)가 **Component Model canonical ABI의 1급 프리미티브**로 이동, `wasi:io` 패키지 전면 삭제, `wasi:sockets` 7→2 인터페이스 통합, `wasi:http`에 service/middleware 월드 신설, 0.2/0.3 컴포넌트 한 런타임 동시 실행(후방호환). [실측]
- **wasmtime**: 46.0.0(2026-06-22)이 최종 WASI 0.3.0 + `component-model-async` **기본 활성**(릴리즈 노트 직접 인용); 47.0.4(2026-08-20)에 p3 streams 보안 패치; 최신 48.0.1(2026-08-24). 현행 참조 호스트 45는 RC 스냅샷 + 플래그 필요 상태. [실측]
- **이중 상태 주의**: 런타임 ABI는 46+에서 기본 활성이나, 임베딩 크레이트 `wasmtime-wasi` 48.0.1의 `p3` 모듈은 문서상 여전히 **"experimental, unstable and incomplete"**(p2는 안정). [실측]
- **rustc**: `wasm32-wasip3` 타깃 **존재**(PR #147205, 2025-10-07 머지, Rust 1.92 마일스톤), 현재 **Tier 3**(std 표기상 ✓, rustup 배포 std 없음 → nightly `-Zbuild-std` 필요 — otter 현행 파이프라인과 동형). Tier 2 승격 MCP(compiler-team#1001)는 FCP 종료+disposition-merge이나 2026-09-03 현재 문서표 미반영. wasip3 타깃은 **컴포넌트를 직접 출력**(코어 모듈 아님). [실측]
- **wasm64-WASI 타깃은 어느 티어에도 없음** — CM memory64 이슈 해결이 선행 조건이라고 rustc 문서 명시. [실측]
- 툴체인: wit-bindgen 0.61.1(p3 async 지원), jco p3 전 기능 지원, cargo-component는 p3 언급 전무(방향성은 plain cargo + wasip2/p3 타깃). [실측]
- 마이그레이션: p1→p2 어댑터는 성숙; **p1→p3 직접 어댑터는 계획 없음**(단일 출처 — golang/go#77141, 신뢰도 중간); p3 native를 원하면 게스트 재빌드가 사실상 유일 경로. [실측+추정]
- **미검증 조합**: otter 게스트의 `-Cpanic=unwind`+exnref 빌드가 wasm32-wasip3에서 동작하는지 — 어떤 소스도 답하지 않음, 스파이크 필요. [추정/미검증]

출처: https://github.com/WebAssembly/WASI/releases/tag/v0.3.0 · https://bytecodealliance.org/articles/WASI-0.3 · https://wasi.dev/releases/wasi-p3 · https://github.com/bytecodealliance/wasmtime/releases/tag/v46.0.0 · https://github.com/bytecodealliance/wasmtime/releases · https://docs.rs/wasmtime-wasi/latest/wasmtime_wasi/ · https://doc.rust-lang.org/nightly/rustc/platform-support.html · https://doc.rust-lang.org/nightly/rustc/platform-support/wasm32-wasip3.html · https://github.com/rust-lang/rust/pull/147205 · https://github.com/rust-lang/compiler-team/issues/1001 · https://github.com/bytecodealliance/wit-bindgen/releases · https://github.com/bytecodealliance/cargo-component/releases · https://github.com/golang/go/issues/77141 · https://dev.to/fermyon/looking-ahead-to-wasip3-5aem · https://component-model.bytecodealliance.org/running-components/wasmtime.html

### §1.2 컴포넌트 모델 × 런타임 동적 컴파일 ([component-dynamic] 축)

- **표준 동적 컴파일/인스턴스화 경로: 현재 없음.** WASI#477(2022)은 S-inactive; component-model#401(dlopen, compile-from-bytes)은 설계 토론 단계(마지막 활동 2026-01-21, JIT용 인터페이스는 별도 옵션으로 분리 방향); component-model#423(runtime instantiation)은 BA 공식 로드맵 "The Road to Component Model 1.0"(2026-06-08)에 있으나 **"Not all of these will land before 1.0"** — 1.0 미보장, 드래프트 부재. Linking.md는 runtime dynamic linking을 명시적 유예로 규정. [실측]
- **shared-everything dynamic linking 규약은 안정** — 단 링크 모듈 집합은 **컴포넌트 정의 시점 정적 선언** 한정("runtime dynamic linking...is not possible to do purely within wasm" 스펙 명문). `wasm-tools component link`의 dlopen 스타일도 링크 타임 임베드 집합 내 선택만 가능. [실측]
- **컴포넌트 경계의 코어 자원 공유는 스펙 수준 금지**: "must not share linear memory across component instance boundaries"; 코어 memory/table은 canonical ABI 경계를 통과 불가. [실측]
- **wasmtime 임베더 탈출구 없음**: `component::LinkerInstance::module(name, &Module)`은 인스턴스화-시점 코어 모듈 주입만(재인스턴스화=상태 소실≠tier-up); `component::Instance` 메서드 전수 확인 결과 **컴포넌트 내부 코어 인스턴스의 memory/table 접근 공식 API 부재**. 우회는 wasmtime 내부 포크뿐(비지원·불건전, sound성 CVE 전례 CVE-2025-64345). [실측 — API 표면 전수 확인]
- **jco(JS 호스트)에서는 런타임 JIT가 원리상 성립**: `--instantiation` 모드의 `getCoreModule`/`instantiateCore` 교체로 컴포넌트 내부 코어 memory/table이 임베더 손에 옴. [실측]
- wasmer 7.0의 WASIX dlopen은 실동 사례이나 비표준·CM 아님; WAMR은 CM 미지원. [실측]
- **판정 (b)**: 네이티브 CM 호스트(wasmtime류) 위 "컴포넌트로서의 otter"에 한해 **AOT-모드가 유일 경로 — 확실성 높음**(스펙 명문 + API 전수 확인). 유보 2건: (i) CM runtime-instantiation 1.x 표준화 시 재개방(시점 미정), (ii) JS 호스트(jco)는 지금도 런타임 JIT 가능. 표준화 대기는 최소 1~2년 이상 [추정 — 이 문장만 추측으로 명기됨].

출처: https://github.com/WebAssembly/WASI/issues/477 · https://github.com/WebAssembly/component-model/issues/401 · https://github.com/WebAssembly/component-model/issues/423 · https://github.com/WebAssembly/component-model/blob/main/design/mvp/Linking.md · https://github.com/WebAssembly/component-model/blob/main/design/mvp/examples/SharedEverythingDynamicLinking.md · https://bytecodealliance.org/articles/the-road-to-component-model-1-0 · https://docs.rs/wasmtime/latest/wasmtime/component/struct.LinkerInstance.html · https://docs.rs/wasmtime/latest/wasmtime/component/struct.Instance.html · https://bytecodealliance.github.io/jco/transpiling.html · https://wasmer.io/posts/dynamic-linking-in-wasm-wasix · https://wasmer.io/posts/wasmer-7 · https://github.com/bytecodealliance/wasmtime/issues/10491 · https://advisories.gitlab.com/cargo/wasmtime/CVE-2025-64345/ · https://github.com/bytecodealliance/wasm-micro-runtime

### §1.3 threads / atomics / shared memory ([threads-atomics] 축)

- **코어 threads 제안(shared linear memory + atomics): Phase 4**, 전 주요 엔진 출하. 단 Wasm 3.0(2025-09-17) 공식 기능 목록엔 미포함(별도 트랙). **공유 가능한 것은 memory뿐** — 테이블의 cross-thread 공유는 현행 스펙에서 불가(Overview.md 명문). otter 현행 계약(동일 agent 내 여러 인스턴스가 같은 funcref 테이블 임포트)은 합법 [추론]. [실측]
- **shared-everything-threads: Phase 1** — shared funcref 테이블·`thread.spawn_indirect`(otter 계약과 정확히 동형)가 여기 포함되나, **wasmtime 미구현**(#9466 "Unimplemented"), V8 실험·미출하. 도착 시점 예측 불가(수년 단위) [추론]. [실측]
- **rustc**: `wasm32-wasip1-threads` Tier 2, std 완전 지원(+atomics std 배포됨; 단 `-Cpanic=unwind`는 여전히 `-Zbuild-std` 필수). "wasip1-threads + panic=unwind + exnref + build-std" 4중 조합은 **공식 보증 없음 [미검증 — 로컬 스모크 테스트가 게이트]**. [실측+미검증]
- **wasi-threads는 사망 확정**: 제안 자체가 legacy 선언, **wasmtime 47.0.0(2026-07-20)에서 wasi-threads·`wasi-common` 제거**. 참조 호스트 45엔 아직 있으나 호스트 업그레이드 순간 wasip1-threads 게스트 구동 불가 — **이 축의 가장 계약-파괴적 사실**. [실측]
- wasmtime의 threads 지원은 Tier 2(shared memory와 리소스 리밋 통합 불량, **pooling allocator 미지원**, fuzzing WIP); EH·memory64는 Tier 1. [실측]
- p3의 동시성은 **협조적(cooperative)** — 진짜 병렬성은 shared-everything-threads 코어 도착 후 통합 설계(CM Concurrency.md); canonical ABI는 현재 shared memory와 궁합 나쁨 [부차 소스 보강, 추정]. [실측+추정]
- 브라우저: SAB/shared Memory는 COOP+COEP 필수, Baseline 성숙; **`WebAssembly.Table`은 워커 간 공유 불가** → 글루의 공유 테이블은 워커당 재구축만 성립 [추론]. [실측]
- 단계 판정: (a) 수동적 shared-memory 호환 = **완전 성숙, 지금 가능**(emit의 memory-type 파라미터화 + 스모크 테스트가 전부); (b) IR atomic ops = 성숙(단 단일 스레드 엔진에선 가치 낮음); (c) 멀티스레드 엔진 = 미성숙(외부 의존 Phase 1 + 전 엔진 미구현 + 내부 Cell/RefCell 대공사). [실측+추론]

출처: https://github.com/WebAssembly/threads/blob/main/proposals/threads/Overview.md · https://github.com/WebAssembly/proposals · https://github.com/WebAssembly/shared-everything-threads/blob/main/proposals/shared-everything-threads/Overview.md · https://webassembly.org/news/2025-09-17-wasm-3.0/ · https://doc.rust-lang.org/nightly/rustc/platform-support/wasm32-wasip1-threads.html · https://doc.rust-lang.org/beta/rustc/platform-support/wasm32-unknown-unknown.html · https://github.com/rust-lang/rust/issues/132416 · https://github.com/rust-lang/rust/issues/146721 · https://docs.wasmtime.dev/stability-wasm-proposals.html · https://github.com/bytecodealliance/wasmtime/issues/9466 · https://github.com/bytecodealliance/wasmtime/blob/release-47.0.0/RELEASES.md · https://github.com/WebAssembly/wasi-threads · https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md · https://github.com/pulseengine/meld/issues/298 · https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer · https://issues.chromium.org/issues/42204563 · https://chromestatus.com/feature/5163209685467136

### §1.4 memory64 / wasm64 ([wasm64] 축)

- **memory64: Phase 4, upstream 병합 완료, Wasm 3.0 정식 포함**(table64 포함 — 테이블 addrtype i32/i64). Chrome 133+/Firefox 134+ 기본 지원, **Safari 전 버전 미지원**(27 TP까지) → 웹 Baseline 불가. [실측]
- **wasmtime**: memory64 **Tier 1**(table64는 26.0부터). ~~단 `Config::wasm_memory64`는 **기본 false — 옵트인**~~ **[O-19 정정 — 스파이크 정본 `otter-o19-spike-results.md` S2-M1-6]**: 45.0.3·48.0.1 **양 트레인 default Config로 memory64/table64 수용 실측** — 종전 "옵트인" 서술은 스테일. 단 **문서-실측 괴리 병기**: 48 config.rs:1220 자체 문서는 여전히 "false by default"를 선언하고, 실제 default-ON은 wasmparser 0.258 `WasmFeatures`(features.rs:227 `memory64=true`) 경유 [패널 scope-4 실물 확인] — 재반전 개연성의 근거라, 제품 배선은 **명시 `wasm_memory64(true)` + 음성 대조**로 방어(O-19 설계 §4). 참조 호스트 45로 버전상 충분. [실측]
- **rustc**: 유일한 wasm64 타깃은 `wasm64-unknown-unknown`(**Tier 3**, `-Zbuild-std` 필수, I/O 전부 무동작). ~~**치명 캐비앳: "does not support `panic=unwind` at this time"** — otter 게스트의 `-Cpanic=unwind`+exnref 레시피 직격, O-12(fail-closed catch_unwind) 전제 파괴~~ **[O-19 정정 — S1-M1보너스]**: 핀 nightly-2026-08-31에서 `-Cpanic=unwind`+exnref **빌드·링크·validate·런타임 catch_unwind 전부 실측 그린**(otter-engine 자체 탑승 포함 — O-12 보증이 wasm64 게스트 안에서 성립) — rustc 문서의 미지원 선언은 **문서 스테일**. 단 문서가 여전히 미지원을 선언하므로 **핀-실측 그린일 뿐 업스트림 계약 아님** — 엔진-on-wasm64는 '외부 차단'이 아니라 '미착수 백로그'로 분류(채택 시 W-4 2레인 카나리아 편입 전제, O-19 설계 §1.3). `wasm64-wasip*` 타깃은 존재하지 않음. [실측]
- **막힌 층은 rustc가 아니라 WASI ABI**: wasi-sdk 공식 "wasm64 미지원·계획 없음"; wasi-libc PR #444(wasm64 지원)는 "새 preview1 ABI 신설은 WASI Subgroup 논의 필요"로 계류(최근 활동 2025-10-11) — preview1은 사실상 동결 ABI. **CM canonical ABI의 memory64 lowering도 미표준화**(CM #22) → 컴포넌트에 wasm64 코어 모듈을 담을 수 없음. [실측]
- 방출기 측 블로커 없음: wasm-encoder 0.258(현행 버전)은 memory64/table64/shared 전부 지원. [실측]
- **성능 동기는 음수**: 64-bit 메모리는 guard-page 생략 불가 → 명시적 bounds check 1.2×–1.8× 슬로다운(wasmtime·SpiderMonkey 공히 확인). [실측]
- **레포 실측 — 32-bit 전제 7건**: ① `otter_jit.compile(ptr:u32,len:u32)` 계약(hosts/otter-host-wasmtime/src/main.rs:134), ② `scratch_addr: u32`+Refusal 타입(crates/otter-wasmemit/src/lib.rs:180,485), ③ 주소 `I32Const` 방출(:621,633 — memory64에선 validation error), ④ `call_indirect` 인덱스 i32(:577-585)+헬퍼 슬롯 u32(:93-102), ⑤ **"fn 포인터==테이블 인덱스" 실측이 wasm32 한정**(crates/otter-engine/src/lib.rs:13-14,702), ⑥ 호스트 임포트 memory/table 32-bit 정의, ⑦ `-Cpanic=unwind` 레시피(위 캐비앗). 재검증 최우선은 ⑤·⑦, 둘 다 스파이크로만 판정 가능. [실측]
- 주의: 2차 소스(byteiota)의 "Wasm 3.0 스펙 2026-06-13 landing" 주장 vs 1차(webassembly.org) 2025-09-17 완성 발표 — W3C REC 트랙 후속 절차 여부 미확인 [추정 구간].

출처: https://github.com/WebAssembly/memory64 · https://webassembly.org/news/2025-09-17-wasm-3.0/ · https://github.com/WebAssembly/memory64/blob/main/proposals/memory64/Overview.md · https://caniuse.com/wf-wasm-memory64 · https://chromestatus.com/feature/5070065734516736 · https://webkit.org/blog/17333/webkit-features-in-safari-26-0/ · https://docs.wasmtime.dev/stability-wasm-proposals.html · https://bytecodealliance.org/articles/wasmtime-26.0 · https://docs.wasmtime.dev/api/wasmtime/struct.Config.html · https://doc.rust-lang.org/rustc/platform-support/wasm64-unknown-unknown.html · https://github.com/WebAssembly/wasi-sdk/issues/185 · https://github.com/WebAssembly/wasi-sdk/issues/212 · https://github.com/WebAssembly/wasi-libc/pull/444 · https://github.com/WebAssembly/memory64/issues/44 · https://github.com/bytecodealliance/wasmtime/issues/3594 · https://github.com/emscripten-core/emscripten/issues/15508 · https://github.com/WebAssembly/component-model/issues/22 · https://docs.rs/wasm-encoder/0.258.0/wasm_encoder/struct.MemoryType.html · https://github.com/bytecodealliance/wasmtime/issues/6094 · https://github.com/WebAssembly/memory64/issues/3 · https://spidermonkey.dev/blog/2025/01/15/is-memory64-actually-worth-using.html · https://github.com/rust-lang/llvm-project/pull/180

### §1.5 AOT-모드 갭 레포 전수 실측 ([aot-gap] 축 — §3의 정본)

- 측정 소스: `crates/otter-wasmemit/src/lib.rs`(1,126 LOC 전체 판독), `crates/otter-engine/src/lib.rs`(2,620 LOC 중 wasm-emit cfg 구역), `crates/otter-cranelift/src/lib.rs`(`compile_module`/`CallMode`), `crates/otter/src/lib.rs`, 설계 문서 `.claude-research-library/otter-o13-wasm-emit-design.md` §2/§4/§5/§11/§14, `otter-o8-tailcall-design.md` §8.10. 전부 실판독. [실측]
- 요지: 방출기는 구조적으로 배치 컴파일러(순수 lowering, `no_std`+`forbid(unsafe_code)`, 함수 임포트 0, EH 명령 방출 0)이나, 현행 방출물은 **모든 교차-함수 엣지와 모든 산술이 라이브 엔진의 네이티브 헬퍼**(module_dispatch/tail_signal/otter_binop·convert·unop)를 통과 — 엔진 없이는 상수/이동/점프 외 계산 불능. 상세는 §3. [실측]

### §1.6 축 간 모순·확신도 조정 기록 (임의 봉합 금지 원칙)

1. **wasmtime 46의 플래그 필요 여부 — 상충**: [threads-atomics] 축은 "46이 최종 0.3.0 첫 구현 (`-Sp3 -W component-model-async=y`)"로 플래그를 병기; [wasip3] 축은 v46.0.0 릴리즈 노트 직접 인용으로 "WASI 0.3.0 by default + component-model-async enabled by default"(기본 활성·플래그 불요). 릴리즈 노트 직접 인용이 1차 소스이나, 두 기술을 모두 기록하고 호스트 업그레이드 증분에서 실기동으로 판정한다(봉합하지 않음).

   **실기동 판정 부기 (O-16a — 2026-09-03, wasmtime 48.0.1 호스트 범프에서 판정; 배선은 O-16f 몫)**: 상충은 **관측 층위 차이**였다 — 둘 다 자기 층위에선 사실. ① cargo-feature 층위: `component-model-async`는 46+에서 wasmtime **기본 feature 집합에 포함**(48 crates/wasmtime/Cargo.toml 실측) → default-features 임베딩·CLI에선 추가 스위치 불요([wasip3] 축의 "by default" 인용은 이 층위). ② 런타임 층위: 디폴트는 feature 컴파일-인에 **조건부** — `WasmFeatures::CM_ASYNC` 디폴트 = `cfg!(feature="component-model-async")`(48 config.rs 실측), `Config::wasm_component_model_async` API 자체가 같은 feature 게이트라 feature가 빠지면 켤 스위치도 없다. ③ 참조 호스트의 실제 그래프(default-features=false + wasmtime-wasi `p1`, 48.0.1 범프 후 `cargo tree -e features` 실측): 해소 feature = {async, component-model, cranelift, gc, gc-drc, runtime, std} — **`component-model-async` 부재**. wasmtime-wasi 측도 기본 = `p1`+`p2`이고 p3는 별도 opt-in feature(`p3 = ["wasmtime/component-model-async", "wasmtime/component-model-bytes"]`), p3 모듈은 48.0.1 문서가 "Experimental, unstable and incomplete" 자기-선언. **판정**: 48 임베딩 기준 결정 스위치는 런타임 플래그가 아니라 **컴파일-타임 cargo feature 게이트**다 — [threads-atomics] 축의 CLI 플래그 서술은 임베딩 결론에 부적용. O-16f의 p3 배선 착수 요건 = wasmtime-wasi `p3` feature 옵트인 + p3 전용 linker 경로(현행 호스트에선 비활성이 올바른 기본). 부수 실측: `wasm_exceptions`는 48에서 존속·`gc` feature 게이트·기본 ON(47+).
2. **확신도 상향(모순 아님)**: [wasip3] 축의 블로커 1("wasmtime이 기존 컴포넌트 인스턴스 메모리에 새 코어 모듈을 링크하는 경로 없음")은 "분석적 판단 — 실측 아님"으로 표기됐으나, [component-dynamic] 축이 `component::Instance` API 표면 전수 확인으로 **실측 확정**. 이 문서는 실측으로 취급한다.
3. **Wasm 3.0 시점 — 소스 간 상충**: 1차(webassembly.org) 2025-09-17 vs 2차(byteiota) "2026-06-13 landing". 1차를 기준으로 하되 W3C REC 절차 여부는 미확인으로 남김.
4. 비어 있는 축: 없음.

---

## §2. 상향 목표별 '의미할 수 있는 것' 선택지 트리

### §2.1 목표 ① — 컴포넌트 모델 + WASIp3 지원

```
p3/CM 지원
├─ A. 게스트-as-컴포넌트, T0(인터프리터)만
│    · 게스트를 wasm32-wasip3로 재빌드 → 컴포넌트 출력 → wasmtime 46+에서 구동. Twasm JIT 티어는 이 모드에서 봉인.
│    · 외부 의존: rustc wasip3 Tier 3(-Zbuild-std, Tier 2 승격 승인·반영 대기) / wasmtime-wasi p3 임베딩 API experimental
│    · 성숙도: 게스트 타깃 존재하나 최신-불안정 스택; panic=unwind+exnref×wasip3 조합 미검증(스파이크 게이트)
├─ B. A + AOT 모드 (빌드 타임 전-모듈 방출 → shared-everything 정적 링크 → 컴포넌트화)
│    · 오너 관찰("wasm-emit은 개념상 AOT")의 실현 경로. 규약(shared-everything dynamic linking)이 정확히 이 형태를 지원. [실측]
│    · 외부 의존: wasm-tools component link(안정), tail-call(wasm 3.0 — wasmtime 45 실측 미검증)
│    · 성숙도: 규약 성숙 / otter측 신규 작업량은 §3·§5 (증분 8개, +600~1,000 LOC 추정)
├─ C. B + 호스트-사이드채널 JIT (컴포넌트 내부 memory/table에 런타임 모듈 주입)
│    ├─ C-1. 네이티브 CM 호스트(wasmtime): 불성립 — 스펙 금지 + API 부재 [실측 확정]. 재개방 조건: CM#423 표준화(1.0 미보장, 1~2년+ 추정)
│    └─ C-2. JS 호스트(jco transpile + custom instantiateCore): 지금 성립 [실측] — 단 JS 호스트 한정 + jco 글루 커스터마이즈 필요
└─ D. 듀얼 아키텍처: 코어-모듈 JIT 모드(현행 O-13, 계속 정본) + 컴포넌트 AOT 모드(B) 병행
     · [component-dynamic] 축 결론이 명시 권고한 형태 — 두 세계를 모두 커버. C-2는 D 위의 선택적 확장.
```

### §2.2 목표 ② — atomics + shared memory

```
atomics/shared memory 지원
├─ (a) 수동적 shared-memory 호환 (L0): memory 임포트의 shared 플래그+명시적 max 파라미터화, 엔진은 단일 스레드 유지
│    · 외부 의존: threads Phase 4 + wasmtime 기본 활성 — 완전 성숙. 지금 실현 가능(방출 코드 변경 거의 0 + 스모크 테스트)
├─ (b) IR atomic op 추가 (L1): atomic load/store/rmw/cmpxchg/wait/notify lowering, 엔진 단일 스레드 유지
│    · 외부 의존: 동일 — 성숙. 단 단일 스레드 엔진에선 의미론적 가치 낮음 → (c)의 선행 투자로만 정당화 [추론]
├─ (c-1) 스레드-로컬 엔진 복제 모델 (L2): 워커/스레드당 엔진+테이블 완전 복제, shared memory만 공유
│    · 외부 의존: SAB+COOP/COEP(브라우저)·wasmtime SharedMemory(pooling allocator 포기) — 성숙하나 otter 계약 재설계 필요
│    · 내부 의존: Cell/RefCell 전면 재설계(대공사) + 스폰 경로(wasi-threads 사망, p3는 협조적 — 병렬 스폰 표준 부재)
└─ (c-2) 진짜 shared-everything (L3): cross-thread 공유 funcref 테이블 + thread.spawn_indirect
     · 외부 의존: shared-everything-threads Phase 1 + wasmtime 미구현 + V8 실험 — 미성숙, 관망(#9466 추적)
```

### §2.3 목표 ③ — wasm32/wasm64 불문

```
wasm64 지원
├─ L0. addrtype 추상화 + wasm64 T0-only 스모크 빌드 — DONE (O-19 C1/C2/C4)
│    · wasm64-unknown-unknown + -Zbuild-std 빌드는 소스 무수정 그린 실측(S1-M1; `just wasm64-smoke` 상시화). WASI 없음(println 무동작)은 불변.
│    · [O-19 정정] panic=unwind는 핀 nightly-2026-08-31 실측 그린(§1.4 정정 — 문서 스테일, 업스트림 보증 없음); §1.4의 32-bit 전제 7건은 처분 완료(해소 2.5 + 비대상/이관 4.5 — O-19 설계 §5 처분표가 정본)
├─ L1. wasmtime memory64+table64 실행 — DONE (O-19 C3: A64 차분 레인 + 명시-옵트인 음성 대조 + table64 벡터 go()→7, 산물-레벨). [O-19 정정] "옵트인 플래그만"은 스테일 — 45/48 기본 수용 실측(§1.4 정정; 제품 배선은 방어적 명시 옵트인). 블로커 없음 [실측]
├─ L2. wasm64 + WASI — 표준에서 막힘: wasi-libc PR#444 계류(WASI Subgroup 결정 대기), CM canonical ABI memory64 미표준화(CM#22)
│    → 컴포넌트 AOT 전략도 wasm64와 조합 불가(canonical ABI가 32-bit 전용인 동안)
└─ L3. 웹 배포 — Safari 전면 미지원으로 Baseline 불가. 관망
· 성능 주의: bounds-check 비용으로 wasm64는 wasm32보다 항상 느림(1.2–1.8×) — 4GiB+ 힙 실수요 전까지 전환 동기 음수 [실측]
```

---

## §3. AOT-모드 갭 분석 ([aot-gap] 레포 실측을 정본으로)

### §3.1 헬퍼 의존 전수표 요지 — 순수성과 자가-lowering

방출 코드의 `call_indirect` 타깃은 정확히 5+4종(`HelperIdx` 5 + Host1/2/HostCtx1/2; wasmemit:81-103, :287-297). [실측]

| 분류 | 항목 | AOT 판정 |
|---|---|---|
| 결정적·저위험 하강 가능 | 정수 Add/Sub/Mul/Lt/Eq, int↔int/int→float 변환, Neg 전 타입(F16 `^0x8000` xor 포함) | ~175–245 LOC. norm(부호확장/마스크)은 cranelift `norm_int` 동형 |
| 하강 가능·위험 | f32/f64 산술(Add/Sub/Mul/Div) + float→int(trunc_sat + `lt_s`+`select` 클램프) | ~80–120 LOC + **§11-16 이월 위험: wasm NaN 정규화 vs 네이티브 헬퍼의 페이로드 비트 divergence** — 별도 증분 격리 필수 |
| 하강 불가(영구 헬퍼) | f16 산술/변환 전부(`half` 소프트플로트; wasm에 f16 op 부재 — 설계 §4 명문) | 헬퍼를 wasm 오브젝트로 빌드해 아티팩트에 **정적 링크**(AOT-6)가 "정본 재구현 금지" 계약(otter:224-227) 보존의 유일한 길 |
| 하강 개념 부적용(AOT에서 소멸) | `module_dispatch`(#7 — counts/tiers/RefCell/catch_unwind), `tail_signal`(#8) | 티어업·트램폴린·trapped 포이즌 전부 소멸; tail은 `return_call_indirect`로 대체 |
| 원리적 하강 불가 | Host1/2/HostCtx1/2(임베더 네이티브 fn) | **함수 임포트로 반전**(현행 "함수 임포트 0" 규범의 유일한 필연적 예외) — 호스트-빌트인 없는 IR만 완전 자기완결 |
| 정책 재정의만 | 정수 Div(현행도 Refused) | AOT엔 T0 폴백이 없어 Refusal이 **전-모듈 컴파일 실패**로 승격돼야 함 |

핵심 규율: v1 전면-헬퍼의 근거("헬퍼 = 정본 의미론, 제2의 손 semantics 없음", wasmemit:495-496)가 하강 순간 깨지므로, **하강분마다 11-NumTy 헬퍼-오라클 차등이 게이트**(기존 differential.rs 1,104 LOC 하네스 패턴 재사용). [실측]

### §3.2 호출 간접 — AOT에서 디스패처 없이 성립하는가

**성립한다.** [실측] IR 콜리는 레지스터 런타임 값(raw 함수 인덱스)이고, cranelift `CallMode::Table`(cranelift:1046-1052)이 디스패처-0 형태를 이미 구현. wasm에선 더 자연스러움: 모듈 내 선언 funcref 테이블 + element 세그먼트로 "otter 함수 인덱스 = 테이블 슬롯"을 만들면 콜리 값이 그대로 `call_indirect` 피연산자. 엔진 트램폴린 업무 분해: 티어업 카운팅·best_code 선택·tail_pending 트램폴린·catch_unwind 격리·복사-즉시 변형·트랩 채널 = **전부 소멸**(현행이 `return_call`을 안 쓴 유일한 이유 "콜리가 T0일 수 있음"의 전제가 통째로 사라짐); `fill` 규약·argc-가드 프롤로그 = **잔존**; 콜리 범위검사 = **형태 변경**(call_indirect의 테이블 경계+타입 trap이 대신 — fail-closed `fill` 반환에서 trap으로 실패 모드 변화, 명시 가드 방출 시 보존 가능 → 정책 결정 필요, §6-6). 스크래치 재사용의 재귀 안전성은 "프롤로그 args 선복사(wasmemit:1047-1061) 후 콜사이트 도달" 구조로 성립. [실측]

### §3.3 전-모듈 방출 API 갭 (`compile_dispatch_fn` → `compile_module`)

현행 시그니처(wasmemit:978-987)는 함수당 1모듈. 필요 변경 7건: ① 다함수 1모듈(`Types` 인터닝은 이미 모듈-스코프라 그대로 승격), ② 내부 링크(선언 테이블+element; `COMPILED_TY` `(ctx,args,argc)` ABI는 ctx를 죽은 파라미터로 유지해 lowering 델타 최소화), ③ **`scratch_addr` 파라미터 소멸 → 방출기 소유 메모리 맵**(함수별 정적 스크래치 자체 배정), ④ `resolve`/`HelperIdx` 의미 변경(내부 인덱스 또는 임포트명), ⑤ memory/table **소유권 반전**(임포트 2행 → 선언+export), ⑥ **Refusal 총체화**(`AotError { func_idx, refusal }` 전-모듈 실패 — cranelift가 `unimplemented!`로 죽는 것의 값-화), ⑦ 엔트리/패키징(`m.entry` export, WASI `_start` 래퍼는 임베딩 결정). [실측]

### §3.4 계약 델타

- `otter_jit.compile` 임포트·§5.1 호스트 계약(에러 코드/grow-with-init/add-only/동기 Store/브라우저 글루) **전체가 AOT 아티팩트에 무관**. `--export-table --growable-table` 링크 요건도 소멸.
- **exnref/§7 빌드 의존이 아티팩트에 안 남음 (소스 확정)**: 방출 명령 전수 census = `{Block, Br, BrTable, CallIndirect, Drop, Else, End, I32Const, I64Const, I64Eqz, I64GtS, I64Load, I64Store, If, LocalGet, LocalSet, Loop, Return, Unreachable}` — EH 명령 0건. §7 nightly 핀 체계(W-4 2레인 CI 포함)는 게스트-내-엔진(O-12) 귀속이라 **순수 AOT 산물은 전부 탈출**. 단서: f16 헬퍼의 stable+panic=abort wasm 빌드 성립 여부는 [추정 — 실측 필요].
- 남는 호스트 요건: funcref 테이블+call_indirect, sign-extension/trunc_sat(코어 2.0), **tail-call(wasm 3.0 — wasmtime 45 지원 미검증)**, 엔진 구현 캡 사전검사 유효. [실측+미검증]

### §3.5 `CallMode::Table` / §8.10 정합

cranelift `compile_module` = "전 함수 선선언 → 균일 ABI → 테이블 간접호출 → 테이블+엔트리 소유 핸들"이며 wasm AOT는 그 wasm판(1:1 대응). **§8.10 네이티브 TailCall 예외의 근거("진짜 꼬리호출은 위험한 신규 설계")가 wasm에선 성립하지 않음** — `return_call_indirect`가 표준 명령이라 wasm-AOT는 갭-프리로 출발 가능하고 §8.10 예외를 확장할 정당화가 없다. 정책 위치: O-13 §14.3-5가 AOT를 스코프 배제로 기록했으므로 **AOT 모드는 O-13의 연장이 아니라 신규 스코프**이며, [otter-tier-parity-policy]상 "티어"가 아니라 패리티 의무 대상 아님(단 갭-프리 출발이 저비용). [실측]

### §3.6 '개념상 AOT' 판정 (오너 관찰 검증)

**참(구조)**: 순수 lowering·코드 포인터 형성 0·함수 임포트 0·EH 방출 0·전 호출 테이블-간접 — 링크 모델이 이미 내장. **아직 아님(계산)**: 함수당 1모듈 + 모든 산술/교차-함수 엣지가 라이브 엔진 통과 — AOT-1..3으로 제어·호출 절반, AOT-4..6으로 산술 절반이 닫힘. 후자 없이 '개념상 AOT'는 호출 그래프에 한해서만 참. 총 거리: 증분 8개, 백엔드 +600~1,000 LOC(+55~90%), 차등 하네스 +500~1,000 LOC, 링크 배관 +100~300 LOC [추정]. [실측+추정]

---

## §4. 의존성·성숙도 위험 표

| 상태 | 항목 | 근거 |
|---|---|---|
| **안정** | 코어 threads/atomics (Phase 4, 전 엔진) · memory64/table64 (Wasm 3.0, wasmtime Tier 1) · shared-everything dynamic linking 규약(정적 선언 한정) · wasm-encoder 0.258의 memory64/shared 지원 · p1→p2 어댑터 · wit-bindgen 0.61.1 · SAB/COOP+COEP (Baseline) | §1.1–§1.4 [실측] |
| **안정이나 주의** | wasmtime shared memory Tier 2 (pooling allocator 미지원, fuzzing WIP) · `Config::wasm_memory64` **문서-실측 괴리** [O-19 정정 — §1.4]: 문서 "false by default"(48 config.rs:1220) vs 45/48 실측 default-ON(wasmparser 0.258 경유) — 재반전 주의, 제품은 명시 옵트인+음성 대조 · wasm64 성능 페널티 1.2–1.8×(S1-M4 실측 1.47×) · wasmtime 45의 tail-call 실측 미검증 | §1.3, §1.4, §3.4 |
| **실험적/불안정** | `wasmtime-wasi` p3 임베딩 API ("experimental, unstable and incomplete") · rustc `wasm32-wasip3` Tier 3(-Zbuild-std; Tier 2 승격 승인·미반영) · jco 사이드채널 JIT(성립하나 글루 커스텀) · "wasip1-threads+unwind+exnref+build-std" 4중 조합(보증 없음) · wasip3×panic=unwind+exnref(미검증) · f16 헬퍼 stable wasm 빌드(추측) | §1.1–§1.3, §3.4 |
| **막힘 (외부 표준)** | CM 런타임 인스턴스화/compile-from-bytes (CM#423/#401 — 1.0 미보장, 네이티브 호스트 JIT 불성립) · cross-thread 공유 funcref 테이블 (shared-everything-threads Phase 1, wasmtime 미구현) · wasm64×WASI (wasi-libc PR#444 계류, preview1 동결) · wasm64×컴포넌트 (CM#22 canonical ABI memory64 미표준화) · ~~wasm64×panic=unwind (rustc 미지원)~~ [O-19 정정 — §1.4: 이 행에서 **제외**. 핀 nightly-2026-08-31 실측 그린(문서 스테일)이라 '외부 표준 막힘'이 아니라 '핀-실측 그린·업스트림 보증 없음'(미착수 백로그) — 채택 시 카나리아 편입 전제] · Safari memory64 | §1.2–§1.4 |
| **사망 트랙 (신규 의존 금지)** | wasi-threads / `wasi-common` (wasmtime 47에서 제거) · p1→p3 직접 어댑터(계획 없음) | §1.3, §1.1 |

---

## §5. 후보 증분 구조 (O-14.. 가칭, 선결 관계 포함)

전제: 아키텍처는 §2.1-D **듀얼 모드**(코어-모듈 JIT = 현행 정본 유지 + 컴포넌트 AOT 신설)를 작업 가설로 한다(확정은 §6-2 오너 몫).

| 가칭 | 내용 | 선결 | 성격 |
|---|---|---|---|
| **O-14 AOT-코어** | §3의 AOT-1(compile_module 골격: N함수+공유 타입 섹션, lowering 무변경) → AOT-2(내부 링크: 선언 테이블+element, 방출기 소유 스크래치 맵) → AOT-3(TailCall→`return_call_indirect`, 트램폴린 소멸) | 오너 스코프 승인(§6-1) + **wasmtime 45 tail-call 실측 스파이크**(미검증) + AOT 계약 규범 신설(element/data 허용 — §5.2 화이트리스트는 JIT 계약 전용) | 신규 스코프. 위험 낮음~중간 |
| **O-15 AOT-산술 자립** | AOT-4(결정적 하강: 정수 Add/Sub/Mul/Lt/Eq·int 변환·Neg 전 타입) → AOT-5(f32/f64 산술+trunc_sat — NaN 차등 게이트+헬퍼-폴백 스위치) → AOT-6(f16 등 잔여 헬퍼의 wasm 오브젝트 정적 링크) → AOT-7(Host* → 함수 임포트 반전) → AOT-8(Refusal→AotError 총체화+패키징+AOT 계약 문서) | O-14. 각 하강분마다 11-NumTy 헬퍼-오라클 차등 게이트 필수 | **승격 지점(중요)**: 네이티브 op 하강은 지금까지 'perf 유보'(v1 전면-헬퍼, W-후속)였으나, **AOT 모드가 스코프에 들어오는 순간 '기능 선결'로 승격된다** — 라이브 엔진 없는 아티팩트는 AOT-4..6 없이 상수/이동/점프 외 계산 불능(§3.6 실측). 단 Twasm(JIT 티어) 쪽에서는 동일 하강이 여전히 perf-only이며 의무 아님 — 승격은 AOT 모드 한정 |
| **O-16 컴포넌트/WASIp3** | (a) 참조 호스트 45→46+/48 업그레이드(선행: wasip1-threads/wasi-threads 의존 0 확인 — 47에서 제거됨; §1.6-1 플래그 상충의 실기동 판정 포함) · (b) **스파이크: wasm32-wasip3 × panic=unwind+exnref 게스트 빌드**(verify-before-implement 게이트) · (c) AOT 산물의 shared-everything 링크(`wasm-tools component link`) → 컴포넌트화 · (d) (선택) T0-only 게스트-as-컴포넌트 · (e) (선택) jco 사이드채널 JIT 모드(JS 호스트 한정) | (c)는 O-14 필수·O-15 사실상 필수(산술 자립 없는 컴포넌트는 무의미); (a)(b)는 독립 선행 가능; (e)는 (c) 이후 | 네이티브 CM 호스트에서 런타임 JIT 불성립이 실측 확정이므로 **AOT 경로가 CM 호환의 유일한 실질 경로** |
| **O-17 shared-memory L0** | memory 임포트의 shared/max 파라미터화(emit 경로) + §7 빌드에 `+atomics` 얹은 exnref 스모크 테스트(공식 보증 없는 조합 — 게이트) | 없음 — O-13 후속 소작업, **지금 착수 가능**. 규율: wasip1-threads/wasi-threads에 신규 의존 절대 금지 | 성숙 의존만. 위험 낮음 |
| **O-18 IR atomics L1** | atomic load/store/rmw/cmpxchg/wait/notify IR op + wasm-emit lowering(엔진 단일 스레드 유지) | O-17 + **L2(스레드-로컬 복제 모델) 착수 결정(§6-7)** — 단일 스레드 엔진 단독으론 투자 정당화 부족 | 보류성 |
| **O-19 addrtype 추상화 + wasm64 스모크** | [O-19 정정 — 처분표 정합(설계 §5가 정본): "전제 7건 해소"가 아니라 **해소 2.5건(②③ + ⑥ memory 측) + 비대상/이관 4.5건(① J-mode 백로그 · ④ 혼합형 무변경 이월 · ⑤ 성질 확장 실측, 소비자 게이트 밖 · ⑥ table 측 · ⑦ 핀-실측 그린 조건부 이관)**. 게이트 몫 = (a) addrtype 파라미터화(AOT `AotOpts` 표면 전용) + (b) wasm64 T0 컴파일 카나리아 + (c) wasmtime memory64 L1 실행(+table64 벡터). 선행 스파이크 2건 완료: ⑤ wasm64 성립 실측(S1-M3), ⑦ panic=unwind 핀-실측 그린(S1-M1보너스 — O-12 대체 설계 불요 판명) | 외부 표준 막힘(§4)으로 wasm64×WASI·wasm64×컴포넌트는 불가 — L1(wasmtime 실행)까지만 닫힘. 성능 동기 음수(게이트 근거는 오너 스코프 결정) | **오너 재확정 2026-09-04: 상향 stable 게이트의 마지막 하드 항목**("wasm64는 L1 wasmtime-실행까지 + addrtype 추상화") — 종전 '관망성(오너 §6-8)' 분류는 이 재확정으로 대체. C0–C6 전 증분 착지 |

의존 그래프 요약: **O-17은 즉시·독립** / **O-14→O-15→O-16(c)** 직렬이 컴포넌트 목표의 크리티컬 패스 / O-16(a)(b)는 병행 선행 가능 / O-18은 관망(L2 결정 대기) / ~~O-19는 관망(표준 대기)~~ [O-19 정정 — 위 O-19 행과 동일 근거: 오너 재확정 2026-09-04으로 게이트 항목 승격, C0–C6 착지. wasm64×WASI·×컴포넌트 층(L2+)만 표준 대기 존속]. atomics×컴포넌트 교차(shared memory in components)는 wasmtime#9466로 통합된 미완 트랙이라 O-16과 O-17의 산물은 당분간 교차 불가로 취급한다. [실측]

---

## §6. 오너 결정 필요 항목

1. **AOT 모드 신규 스코프 승인(O-14..15)** — O-13 설계(§14.3-5)가 명시적으로 스코프 배제한 항목의 재개방이므로 스코프 확장은 오너 결정.
2. **듀얼 아키텍처 확정(코어-모듈 JIT 정본 유지 + 컴포넌트 AOT 병행)** — 엔진 산출물의 형태와 지원 매트릭스를 규정하는 최상위 아키텍처 결정.
3. **참조 호스트 wasmtime 45→46+/48 업그레이드 시점** — 47의 wasi-threads/`wasi-common` 제거로 계약-파괴 가능성이 있는 기반 교체라 시점·조건은 오너 몫.
4. **불안정 스택 수용선** — rustc wasip3 Tier 3(-Zbuild-std)·wasmtime-wasi p3 experimental API 위에 구축할지는 릴리즈 안정성 정책([release-rowhammer-gate]류)과 직결.
5. **f32/f64 네이티브 하강의 NaN 페이로드 divergence 처리(수용 vs 헬퍼-폴백 스위치 영구 유지)** — 수치 의미론 정본 계약("헬퍼=정본")의 변경이라 오너 승인 사안.
6. **AOT 실패 모드 의미론(콜리 범위검사: 명시 가드로 fail-closed 보존 vs 표준 wasm trap 수렴; Refusal→전-모듈 실패 승격)** — 관측 가능한 실패 계약의 재정의.
7. **멀티스레드 엔진(L2 복제 모델) 착수 여부/시점** — Cell/RefCell 전면 재설계 대공사 + 계약 재설계의 투자 결정이며 O-18의 선결.
8. **wasm64 투자 수준(순수 관망 vs O-19 선투자)** — 성능 동기가 실측상 음수이고 표준이 막힌 목표의 우선순위 배분은 오너 몫.
9. **jco 사이드채널 JIT(JS 호스트 한정) 채택 여부** — 지원 호스트 매트릭스(네이티브 vs JS)의 확장이라 임베딩 전략 결정.

---

*작성 규율 확인: 실측/추정 구분 표기 완료(§1 각 항목, §3, §4), 축 간 모순 §1.6에 기록(봉합 없음), 비어 있는 축 없음. 정본 관계: §3은 [aot-gap] 레포 실측을 정본으로 함.*
