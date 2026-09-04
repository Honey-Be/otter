# wasm-emit backend — 실사 리서치 (2026-09-02)

Status: RESEARCH ONLY (구현 착수 아님). 계기: §8.10에서 `otter-cranelift`가 wasm32-unknown-unknown·
wasm32-wasip2 양쪽 다 단독 빌드 실패 확인 후, "정말 영구 불가능인지, 대안이 있는지" 4갈래 병렬 리서치.
otter-tier-parity-policy 메모의 wasm 예외 조항 프레이밍에 직접 영향.

## 1. cranelift-jit가 wasm 호스트 내부에서 실행되는 것 — 구조적으로 영구 불가능

wasm 코어 스펙은 코드/데이터 분리(Harvard 아키텍처)를 강제하며, 게스트가 실행가능 메모리를 얻거나 새
네이티브 코드를 생성·링크하는 메커니즘이 **어느 제안 단계에도 없음**. `memmap2`는 wasm에서 "컴파일은 되나
런타임에 항상 `Unsupported` 반환"이 **의도된 설계**(issue #17, #160/#161로 재확인 — "to-do 아님, `std::fs::File`류
처럼 영구 스텁"). `region`은 `#[cfg(unix)]`/`#[cfg(windows)]` 외 폴백조차 없고 41개 이슈 중 wasm 언급 0건(요청된
적도 없음). 유일한 스펙 레벨 후보 `func.new`(WebAssembly/jit-interface)는 **Phase 1**(공식 논의 시작 단계,
2025-10~) — 수년 단위로 먼 미래.

업계 전수조사(QuickJS/Javy, Lua-wasm 계열, V8 `--jitless`, SpiderMonkey Portable Baseline Interpreter)에서
예외 없이 전부 "wasm에 호스팅되면 순수 인터프리터로 강등" 패턴 확인 — 세계적으로 wasm 샌드박스 **내부**에서
네이티브 머신코드 JIT을 실행하는 실사례는 전무. `otter-cranelift`의 빌드 실패는 이 구조의 필연적 증상이지
otter 고유 버그가 아님.

## 2. Cranelift 자체를 wasm "타깃"으로 재사용 — 불가능, 손수 작성한 전용 백엔드만 유일한 길

`cranelift-codegen`의 ISA 목록: x86_64/aarch64/s390x/riscv64 + Pulley뿐. **Pulley는 wasm이 아님** — Cranelift
자신을 위해 발명한 **레지스터 기반 사설 바이트코드**(기존 `MachBackend` 프레임워크에 얹기 위해 일부러
레지스터 머신으로 설계). wasmtime#2566("Add Webassembly backend")이 정확히 이 요청이며 **2021-01 개설, 2026년
현재도 미착수·미배정** — 리드 메인테이너(cfallin, bjorn3)가 직접 "wasm은 스택 머신+구조적 제어흐름이라
기존 백엔드 인프라 재사용 불가, 완전 별도 프레임워크 필요"라 확인(가장 최근 재확인 2026-05-22).

가장 근접한 실물: **`cfallin/waffle`**(cfallin 본인 프로젝트, **2026-07-23까지 활발히 유지보수 중**) — CLIF를
소비하는 독립 SSA↔구조적-wasm 왕복 컴파일러(SpiderMonkey급 복잡한 실제 wasm 왕복 검증됨). Cranelift 기능이
아니라 별도 라이브러리이고 CLIF 전용이라 otter IR엔 그대로 못 씀 — 그러나 CFG→구조적 제어흐름(relooper류
문제) 절반의 실질적 프리아트.

## 3. 업계 전수조사에서 나온 세 번째 패턴 — "emit-and-hand-to-host"(우리 아이디어와 정확히 일치)

전수조사가 확인한 3대 전략(순수 인터프리트 / 게스트 자체를 빌드타임에 wasm으로 AOT / **런타임에 코드를 만들어
호스트에 넘기기**) 중 세 번째가 이미 프로덕션에 있음:

- **.NET Jiterpreter**(.NET 8+, shipped): IL 인터프리터가 정상 동작하다 핫 트레이스를 **런타임에 새 wasm
  바이트코드로 만들어** 호스트의 `WebAssembly.compile`에 넘김. Microsoft 공식 설명: "replacing [interpreter
  bytecodes] with tiny blobs of WebAssembly code."
- **CheerpJ**(풀 OpenJDK JVM, 브라우저): 핫 Java 바이트코드를 **JS 소스**로 컴파일해 브라우저 JS 엔진 자신의
  JIT에 위임 — 타깃만 wasm 대신 JS 소스인 동형 패턴.

즉 "게스트 샌드박스가 자체 생성 네이티브 코드를 실행 못 하면, 임베더 환경이 **자체적으로** JIT/AOT할 수 있는
표현(wasm 바이트 또는 JS 소스)으로 코드를 생성해 신뢰된 호스트에 실제 기계어 생성을 위임"하는 게 업계 유일한
실전 해법 — 우리 메모리(`arch-support-tiers`, 2026-06-16)의 "wasm-emit backend" 아이디어와 정확히 일치.

## 4. otter에 적용 시 실현 가능성 — 아키텍처적으로 타당, 신규 문제 두 축

기존 티어(interp/baseline/cranelift)가 전부 따르는 동일한 `Effect`/`Expr` 재귀 walk 패턴의 자연스러운 4번째
백엔드. `Expr::Imm`→`i64.const`, `LocalAt`/`SetLocalAt`→`local.get`/`local.set`(cranelift보다 오히려 단순 —
레지스터 할당을 호스트 wasm 엔진에 위임), 정수 `Bin`/`Un`→네이티브 wasm opcode. `Effect::TailCall`은
**WASM 자체의 tail-call 제안**(`return_call`/`return_call_indirect`, 이미 표준·호스트 엔진 구현됨)에 직결 —
호스트 엔진에서 "공짜"로 얻음(단, 이건 wasm-emit 전용 이득이고 §8.10에서 예외로 남긴 `CallMode::Table`
네이티브 백엔드 갭엔 영향 없음).

**규모**: `otter-cranelift`(1597줄, unsafe 5곳)와 같은 자릿수(~1500-2500 LOC lowering부) — `wasm-encoder`
(Bytecode Alliance, wasmparser 자매품)가 바이트 인코딩 절반을 이미 해결(우리 `crates/wasm`의 디코더는
단방향이라 인코더로 재사용 불가 — 지식은 재사용, 코드는 신규 or `wasm-encoder` 의존 권장). `otter-baseline`이
지고 있는 `mmap`/`mprotect`/W^X 위험 서브시스템 축은 완전히 사라짐(메모리 보호는 호스트 wasm 엔진 소관).

**신규 문제 두 축**(기존 두 백엔드가 겪은 적 없음):
1. **CFG→구조적 제어흐름**: otter의 임의 `JumpTo`/`JumpIf`를 wasm의 `block`/`loop`/`br`(상대 깊이 참조만
   허용)로 재구성 — relooper류 문제, 일반 CFG는 reducibility 보장 없음(설계 시 유의). `waffle`/Ramsey
   알고리즘이 프리아트.
2. **호스트 함수/테이블 링킹**: cranelift는 프로세스 내 실주소를 즉치로 굽지만(`f as usize as i64`), 새로
   만든 wasm 모듈은 그럴 수 없음 — Andy Wingo(2022, "Just-in-time code generation within WebAssembly")의 패턴:
   호스트가 미리 채운 **공유 linear memory + funcref 테이블을 import** 받아 `call_indirect`(테이블 인덱스)로
   대체. cranelift의 "주소를 굽는다" 관례와 구조적으로 동형이라 리스크 낮음.
3. (부가) `Backend`/`Code::run` 트레잇은 동기(synchronous)인데 브라우저 `WebAssembly.compile`/`instantiate`는
   Promise 기반 — 작은 함수 단위 JIT라면 동기 생성자(`new WebAssembly.Module`/`Instance`)로 우회 가능.

## 5. JS-host 의존성 — 실재하나 생각보다 좁음; tsavorite엔 실질적 제약일 수 있음

정확한 프레이밍: "동적 컴파일 import를 제공하는 **협조적 호스트**가 필요" — 반드시 JS는 아님(Wingo의 데모는
Python+wasmtime 호스트가 직접 그 역할의 커스텀 import를 배선한 사례). 그러나 **WASI 표준엔 이 동등물이 없음**
(WASI#477 2022년 개설 후 미해결, component-model#401/#423 2024년 개설 후 미해결 — "일반적 dlopen류 동적 링킹은
컴포넌트 모델 범위 밖, WASI가 표준화해야 하나 안 됨"이 공식 답변).

- **neosurf 자신**([[neosurf-scope-minimal-embeddable-core]] — 1차 타깃 브라우저 임베드): JS 호스트가 항상
  있어 문제없이 적용 가능.
- **tsavorite**: gap 문서 §5는 "빌드 실패 방지 + T0 폴백"만 요청 — 브라우저(JS)인지 순수 WASI인지 명시
  안 함(범용 언어 런타임 이식성 목표로 aarch64/wasm 나열). wasm-emit이 그들에게 쓸모 있으려면 **직접 협조적
  임베더**(예: 자체 wasmtime 기반 런처에 동적 컴파일 import 배선)를 만들어야 함 — 일반 "아무 WASI 런타임에
  .wasm 얹기" 배포에서는 오늘처럼 T0 폴백. 단, 이게 §5의 **현재 요청 자체를 막지는 않음**(그건 wasm-emit
  존재 여부와 무관하게 이미 O-7로 충족됨).

## 6. 결론 및 권고

- **(a) "wasm=T0 영구 고정"을 확정 정책으로 삼지 않는다** — Angle 4가 실행 가능한 경로를 구체적으로 실증했으므로
  미리 문을 닫는 건 과도하게 비관적.
- **(b) wasm-emit을 O-7c급 미래 프로젝트 후보로 로드맵에 남긴다** — 지금 당장 구현 착수는 아님(tsavorite의
  실제 요청인 "빌드 실패 방지+T0 폴백"이 훨씬 저비용·고우선순위이고 O-7로 이미 충족). 이 문서 자체가 "완전
  미지수"였던 걸 "구체적 프리아트(wasm-encoder/waffle/Wingo 링킹 패턴) 확보된 스코프된 리스크"로 낮춘 것.
- **(c) `otter-tier-parity-policy` 메모의 wasm 예외 조항 프레이밍 수정**: "moot, permanent" → **"moot pending
  wasm-emit backend; revisit if/when built"**. 오늘 시점 사실관계(cranelift-jit는 여전히 wasm에서 빌드 안 됨,
  T2가 wasm에 없으니 gap 없음)는 안 바뀜 — 다만 "영구 종결"이라는 뉘앙스가 부정확해졌으므로 조건부로 재표기.
  wasm-emit이 실제로 만들어지면 그것도 "티어"가 되어 O-8c 같은 패리티 요구가 그대로 적용돼야 함.
- **(d) 스펙 레벨 `func.new`**: Phase 1이라 수년 후 — 지금 액션 아이템 아님, 모니터링만.

## 참고 문헌

- wasmtime#2566 "Add Webassembly backend" (2021-01 개설, 미착수)
- wasmtime#12598 (2026-02-15 종료, bjorn3의 직접 답변)
- `cfallin/waffle` (SSA↔구조적-wasm, 2026-07-23까지 유지보수 중)
- `teymour-aldridge/cranelift_codegen_wasm` (2022-08 이후 방치, 미완성)
- WebAssembly/jit-interface `func.new` 제안 (Phase 1, Explainer.md)
- `memmap2` issue #17/#160/#161, `region` `src/os/mod.rs`
- WASI#477 (2022 개설, 미해결), component-model#401/#423 (2024 개설, 미해결)
- Andy Wingo, "Just-in-time code generation within WebAssembly" (2022)
- .NET Jiterpreter (dotnet/runtime#78428), CheerpJ 아키텍처 문서
- Shopify Javy 엔지니어링 글, V8 `--jitless`(v8.dev), SpiderMonkey PBI(cfallin.org)
