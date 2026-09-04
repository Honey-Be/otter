Status: SPIKE RESULTS (2026-09-04, O-16c′ 설계 입력) + §6 해소 부기 (구현 완료 — §5 상충 6건 전건 해소)

# O-16c′ 스파이크 결과 종합 — b′(엔진-라이브러리 wasm 빌드) + b″(M2a 정적 링크 도구 체인)

정본 설계 문서: `otter-o14-aot-core-design.md` v2.3 (§12 서빙 인터페이스, §2.2 헬퍼 심볼, §11 유보 3건). 본 문서는 두 스파이크의 결과를 병치·종합한 O-16c′ 설계 입력이다. 실측/추정 구분은 원문 표기를 보존한다. 양 스파이크가 상충하는 지점은 봉합하지 않고 §5에 기록했다.

- b′ 작업 디렉터리: `/tmp/claude-1000/-home-ybi-neosurf/c49cc604-13f0-41fb-be11-1aeaed5f7a8c/scratchpad/o16c-bprime/` (래퍼 크레이트 `otter-helpers-wasm/`, 하네스 `bprime-runner/`, 산물 `otter_helpers_wasm.stripped.wasm`, WAT 덤프 `lib.wat`, 로그 `COMMANDS.log`, `runner-output.log`)
- b″ 작업 디렉터리: `/tmp/claude-1000/-home-ybi-neosurf/c49cc604-13f0-41fb-be11-1aeaed5f7a8c/scratchpad/o16c-bdblprime/` (스크래치 크레이트 `aotgen/`, 대역 `stubs/`, 축자 커맨드 `commands.log`)
- 도구 버전 [실측]: rustc 1.96.0(레포 핀), wasm-tools 1.246.2, binaryen version_132(릴리스 바이너리), wasm-ld LLD 22.1.3, wasmtime 크레이트 48.0.1(e2e 호스트와 동일 feature set + `component-model`)

---

## §1. b′ 결과 — 엔진-라이브러리 wasm 빌드 (stable + panic=abort)

**판정: b′ 성립** [실측]. AOT 산물의 헬퍼 3종 임포트를 제공하는 wasm 라이브러리가 **stable 1.96(레포 핀 그대로) + wasm32-wasip1 + panic=abort + no_std**로 빌드되고, EH 명령 0, 산물 4,158 B(stripped), 40,000케이스 차등에서 네이티브 정본과 비트 동일, 산물-형상 소비자에 대한 링크 스모크 통과. **§7 nightly 핀 체계 완전 탈출 확인** — 단 shared-memory 프로파일에만 유보 1건(§1 B-표 B5 비고 및 §4-7).

### B1–B5 표

| ID | 항목 | 판정 | 핵심 결과 (전부 [실측], B1은 소스/바이너리 귀속) |
|---|---|---|---|
| B1 | 최소 구성 | otter 크레이트 단독으로 충분, **otter-engine 불요** | 헬퍼 3종(`crates/otter/src/lib.rs:201/206/228`)의 호출 폐포 = `BinOp::eval`/`UnOp::eval`/`convert`/`NumTy::{from_tag,norm}`/f16 코덱 — 전부 동일 파일. 의존은 `half` 2(default-features=false)뿐. 최종 바이너리 함수 census **8개**(export 3 + from_tag/norm + UnOp::eval + core panic_fmt/panic_bounds_check)로 닫힌-세계 증명. otter-engine 불요 3중 근거: ① 의존 방향 단방향(otter-engine/Cargo.toml:15) ② AOT 산물 임포트 전수에 dispatch/tail 없음(otter-wasmemit/src/lib.rs:2249–2277, §2.2 확언과 소스 일치) ③ **기계적 배제** — O-12 `compile_error!`(otter-engine/src/lib.rs:146–154)가 panic=abort에서 발화 → otter-engine은 이 프로파일에 포함 불가능(엔진 포함 = unwind 요구 = EH = nightly 회귀). T0 동반도 게이트 경로에 불요(모드 문서 §2.3-②·§5b 결정 B) — **게이트용 M2a 엔진-라이브러리 = 헬퍼 3종 + 메모리 소유가 전부** |
| B2 | thin 래퍼 + 빌드 | 성립 | 함수당 1줄 위임 × 3, `#[unsafe(no_mangle)] pub extern "C"`(2024 에디션), `#![no_std]` + panic_handler(→`core::arch::wasm32::unreachable`). 비자명 요구 1건: otter가 no_std+alloc이라 `#[global_allocator]` 요구 → **trap-on-use 할당자**로 충족(링키지 + 무할당 런타임 증명 겸용, 40,000케이스 미발화 = 실측 무할당). **cdylib → 순수 링크드 wasm 모듈이 정답 형상**(no_std라 wasi-libc/`_start`/WASI 임포트 전무 — 임포트 섹션 자체가 없음). EH 명령 **0**(전 WAT grep: try/catch/throw/rethrow/delegate/try_table = 0), `wasm-tools validate --features=all,-exceptions,-legacy-exceptions` 통과. export 4종 실물: `memory`, `otter_binop`, `otter_convert`, `otter_unop`(§12.3 의무 명세와 축자 일치) |
| B3 | f16 | 비트 동일 | `half` 2.7.1 경유, wasmtime 48.0.1 vs 네이티브 정본 차등 **40,000케이스 0 불일치**(binop 11타입×6op(int×Div 제외), convert 11×11 전 쌍, unop Neg 11타입; 특수값 + splitmix64 난수 200/조합). 대표값: f16 `1.0+1.0=0x4000`, `1.0/3.0=0x3555`, Neg NaN `0x7E01→0xFE01`(페이로드 보존), f64 `0.1→f16 0x2E66` |
| B4 | panic 경로 | 적합(fail-stop 계약) | 헬퍼는 무패닉 계약이 아님 — 문서화된 fail-stop("out-of-range tag is a backend bug (panics)", lib.rs:226–227). 도달 가능 패닉 전수 3종(OOB 태그 인덱싱 / int×Div `unreachable!` lib.rs:311 / unop 미지 태그 lib.rs:231), 프로브 전부 **결정적 `Trap::UnreachableCodeReached`**, 트랩 후 인스턴스 생존·후속 호출 정상. 태그는 compile_module이 검증한 IR에서 베이크된 즉값 → 도달 = 방출기 버그, 즉시-트랩은 O-12 규율의 올바른 발현. **단 헬퍼 트랩과 산물 가드 트랩은 구별 불가**(둘 다 UnreachableCodeReached) → §4-6 |
| B5 | 크기·링크-준비 형상 | 성립 | cdylib .wasm 4,388 B / **stripped 4,158 B**(gzip 1,690 B) / +atomics 변형 4,921 B / staticlib .a 3.1 MB(미링크 오브젝트, 참고치). 표면: 임포트 **0** / export memory+헬퍼 3. `(memory 16)` — **data 세그먼트 0**, 섀도 스택만 [0,1MiB), `__stack_pointer`=1048576 → 1 MiB 위 전 구간 라이브러리 미접촉 + trap-할당자로 동적 점유 0. 링크 스모크: O-14 산물 임포트 표면 축자 소비자를 라이브러리 export에 배선 → 3검사 전부 기대값 일치. 메모리 서브타이핑 성립(산물 min0 ≤ 실물 16; shared: min0/max65536 ↔ `(memory 17 65536 shared)` 정확 일치). 비고: +atomics는 stable에서 빌드·검증 성립하나 "not stably supported" **경고** — 보증 없는 통과 |

### 성립 레시피 (축자)

```
# Cargo.toml: crate-type=["cdylib","staticlib"]; otter path-dep(default features=half-codec);
#   [profile.release] panic="abort", opt-level="s", lto=true, codegen-units=1; [workspace]
# src/lib.rs: #![no_std]; #[unsafe(no_mangle)] extern "C" 위임 3종; panic_handler→wasm32::unreachable; trap GlobalAlloc
cargo +1.96.0 build --release --target wasm32-wasip1
wasm-tools strip target/wasm32-wasip1/release/otter_helpers_wasm.wasm -o otter_helpers.wasm
wasm-tools validate --features=all,-exceptions,-legacy-exceptions otter_helpers.wasm   # EH-프리 게이트
# shared 프로파일: RUSTFLAGS="-Ctarget-feature=+atomics,+bulk-memory -Clink-arg=--shared-memory -Clink-arg=--max-memory=4294967296"
```

---

## §2. b″ 결과 — M2a 정적 링크 도구 체인

**판정: 도구 체인 성립(확정)** [실측]. `binaryen wasm-merge`(코어 정적 결합) → `wasm-tools component embed`+`component new`(컴포넌트화). 결합 산물이 wasmtime 48에서 인스턴스화·`otter_entry(0,0,0)` 실행 **T0 오라클 비트-동일**, 2M 꼬리재귀 64 KiB 스택 완주, 컴포넌트 API 경유 실행까지 전부 그린. 단, **대역 b′는 손-WAT**(헬퍼 3종은 정본 태그에 가치-충실 구현 — 덕분에 "실행 성립"을 넘어 오라클-동일 단언) — 실물 b′(Rust→wasm) 결합은 재실행 항목(§5-5).

입력 산물(L0, aotgen gen — compile_module 실물 [실측]): `calc.wasm`(595B; 임포트 = `otter.memory` min0 + `otter_helpers.{otter_binop,otter_convert,otter_unop}` + `otter_host.{h_add7,hc_xor}`; 매니페스트 entry_slot=0, nfuncs=2, helper_slots=[2,3,4], host_slots=[5,6], scratch_region=[0x10000,0x10008), table_size=7; 오라클 `0x84f8bd6484f8c001`), `deep.wasm`(726B; 2M 상호 꼬리재귀, 오라클 `0x1`), `calc_shared.wasm`(`(memory 0 65536 shared)` 임포트 형상).

### L1–L5 표

| ID | 항목 | 판정/결과 (전부 [실측]) |
|---|---|---|
| L1 | 도구 후보 | **wasm-merge 132 = 유일한 코어-결합기(성립)** — 주의 ① 기본 MVP 피처라 플래그 필수(`--enable-tail-call` `--enable-nontrapping-float-to-int`, 공유판 `--enable-threads` 추가; 미지정 시 Fatal) ② **fail-closed 아님**: 제공자 누락 시 exit 0으로 임포트 침묵 잔존 → 잔존-임포트 0 검사는 b″ 자체 게이트 몫. **wasm-ld(LLD 22) 불성립**(`not a relocatable wasm file` — 완성 모듈엔 linking 섹션 없음; wasm-ld의 자리는 b′ 내부). **wasm-tools component link 불성립**(`dylink.0` PIC 규약 전제 — 무재배치·즉값-베이크 산물(§2.5-iii)과 규약 자체가 상충, 개명으로 구제 불가). **compose/wac 불성립(범주 착오)**(컴포넌트 조합 = shared-nothing → `otter.memory` 공유 원리적 불가). **component new 단독**은 링크 도구 아님·컴포넌트화 단계로는 성립 |
| L2 | 이름공간 매핑 | wasm-merge 규약: **임포트 모듈-문자열 == 등록 인스턴스-이름**, 필드명은 export명과 타입-검사 매칭. 3-이름공간 유지 시 제공자 모듈 3개 필요(분할-제공자로 그린). 단일 b′ 3중 등록 우회는 **기각 실증**(`Export name conflict: memory` Fatal — 침묵 성공 아닌 시끄러운 실패). **단일 이름공간 기계 개명**(산물의 3개 모듈-문자열 → `otter_lib`; 프로덕션은 매니페스트 `imports` 정본으로 import 섹션만 바이트 재작성, code 섹션 불변) + 단일 등록으로 그린·오라클-동일. → §3-1 |
| L3 | 메모리/테이블 합성·스크래치 | 링크 후 메모리 1개 — **라이브러리 소유**, 산물 임포트가 그 정의에 배선(§2.2 "최종 소유자는 엔진-라이브러리" 결정이 도구 수준에서 실현; 공유판 동일). 테이블: 선언 테이블(min=max)·active element·슬롯 항등 매핑 **보존**(함수 인덱스만 결합-공간 재번호, 슬롯 값 공간 불변; 실행 오라클-동일이 증거) — §2.5-iii와 정합. **단 바이트 보존 아님**: wasm-merge는 binaryen IR 왕복 재인코딩(+producers 섹션) → 링크-후 검증은 의미-수준만 가능. 스크래치: **"메모리-소유 모듈의 active data 세그먼트 = 예약 마커" 성립**(마커 결합 후 보존; 도구 옵션에 의한 예약은 부재) → 예약은 b′ 빌드가 만들고 b″는 정적 검증만. 위반 발현 실측: 커버리지 위반(1페이지 소유자)은 첫 마셜링 store에서 결정적 trap(`memory fault at 0x10000`) — §2.5-ii 축자. 결합 산물 export는 **합집합**(8종 잔존) → 표면 최소화 필요 시 export 축소 후처리 별도 |
| L4 | 결합 산물 실행 | wasmtime 48.0.1(참조-호스트 Config): `calc_split.wasm`(701B, 잔존 임포트 0) → `otter_entry(0,0,0)` = **0x84f8bd6484f8c001 == T0 오라클**(~10.4µs). `deep_split.wasm` → **2M 깊이 상호 return_call_indirect를 64 KiB wasm 스택으로 완주**, 0x1 == 오라클(~16ms, 연료 정상 과금) — 꼬리호출 유계성이 결합을 통과해 보존. `calc_shared_split.wasm`(공유 메모리 정의판) 오라클-동일. 음성 대조 2건: 부분-링크물 잔존-임포트 검사 거부(fail-closed 게이트 형상) + 커버리지-미달판 결정적 trap |
| L5 | 컴포넌트화 | `component new`는 결합 모듈에 **어댑터 없이 성립**(WASI 임포트 0). 단 그냥 하면 **빈 world**(코어 export 자동 승격 없음). 엔트리 리프트 실물 체인: WIT world(`export otter-entry: func(ctx: u32, args: u32, argc: u64) -> s64` — canonical ABI 하강 = 정확히 `(i32,i32,i64)->i64`) + **코어 export 개명 `otter_entry`→`otter-entry`**(WIT는 kebab-case 강제) → `component embed --world` → `component new` → 검증 그린 → **wasmtime 48 컴포넌트 API 인스턴스화·호출 그린**(calc 오라클-동일, deep 2M 완주). 컴포넌트-정의-시점 정적 선언 형태(MODE §2.1-④) 실물 확인 |

### 확정 도구 체인 (축자)

```
compile_module → (매니페스트-구동 임포트 개명, 선택)
  → wasm-merge --enable-tail-call [--enable-threads] <b′.wasm NAME…> <산물 main>
  → b″ 자체 게이트(잔존 임포트 0 + scratch 정적 2검사 + 의미-수준 검증)
  → (배포 형태 따라) export 축소
  → component embed --world … + component new
```

(피처 플래그 최소: `--enable-tail-call` `--enable-nontrapping-float-to-int`, 공유판 `--enable-threads` 추가. 게이트를 도구에 위임할 수 없음 — wasm-merge는 침묵 부분-링크.)

---

## §3. §11 유보 3건 해소 입력

### §3-1. §11-1 임포트 이름공간 최종 문자열 → **개명 불요, 그대로 확정 가능**

- b′ [실측]: 라이브러리 export는 평이름(`otter_binop`), 산물 임포트는 `otter_helpers.otter_binop` — 인스턴스화-그래프 배선에서 매핑 자명(링크 스모크 실증). §2.2 문자열 개명 불요 판단에 참고.
- b″ [실측]: §2.2 문자열은 wasm-merge 하에서 **그대로 유효** — b′를 3모듈로 쪼갤 필요 없이, (권고) b″가 산물 쪽 모듈-문자열을 단일 이름공간으로 **기계 개명**(매니페스트 `imports` 정본, import 섹션만 바이트 재작성) + b′ 등록명 1개로 닫힘. 매니페스트가 임포트 전량을 나르므로 개명-내성 설계(§10-1)가 정확히 작동함을 실증.
- 종합: **§11-1은 "방출기 문자열 현행 유지 + b″ 개명 단계"로 해소.** 단 실물 b′는 `otter_host.*`를 제공하지 않으므로 개명 범위·host 제공 형태에 미해소 잔여(§5-4).

### §3-2. §11-2 매니페스트 직렬화 → **JSON 한 층 충분의 근거 데이터**

- b″ [실측]: b″가 실제 소비한 필드 전수 = `imports`(개명·게이트) / `scratch_region`(정적 검사) / `shared_memory`(제공자 선택·피처 플래그) / `entry_slot`·`table_size`(검증). 이 소비 형태가 "JSON 한 층이면 충분"의 근거.

### §3-3. §11-3 스크래치 예약 메커니즘 → **분업 확정: b′가 만들고 b″가 정적 검증** (단 양 스파이크 답이 다름 — §5-1)

- b′ [실측]: 헬퍼-only b′는 데이터 0·할당 0 → `scratch_base ≥ 0x100000`(비공유; +atomics는 ≥ 0x110000)이면 겹침 원천 불가 — **예약 메커니즘 불요, 수치 규범 1줄**. 라이브러리 확장(실할당자 등) 시 재심.
- b″ [실측]: **"메모리-소유 모듈의 active data 세그먼트 = 예약 마커" 성립** → b″ 정적 2검사 가능: (i) 겹침 금지 = 라이브러리 전 data 세그먼트 ∩ `manifest.scratch_region` = ∅(마커 제외) + b′ 전역 심볼(힙 베이스 등) 대조, (ii) 커버리지 = 메모리 min 페이지 ≥ ⌈end/65536⌉. 도구 옵션에 의한 예약은 부재. 실패 시 1차 대응 = 재방출(§2.5-iv, base 인자만 변경)로 체인과 무마찰.
- 종합: 메커니즘의 **자리**는 확정(b′ 빌드 산출물 속 정적 증거 + b″ 검증). **형태**(수치 규범 1줄 vs 마커 data 세그먼트)는 상충 기록 유지 — §5-1/2/3 참조. 마커 채택 시 커버리지(min 페이지) 문제까지 자동 강제된다는 이점 있음(단 이 상호작용은 [추정], §5-3).

---

## §4. O-16c′ 설계 제약·위험 목록

1. **b″ 파이프라인 확정형 + 자체 게이트 의무** [실측]: §2 축자 체인. 가장 중요한 의무 조항 — wasm-merge는 제공자 누락 시 exit 0 침묵 부분-링크이므로 **잔존-임포트 0 게이트를 도구에 위임할 수 없다.**
2. **b′에 추가 요구(§12.3 초과분)** [실측 근거]: (a) scratch 마커 data 세그먼트(또는 동등한 정적 예약 증거)를 b′ 산물에 포함 — b″ 정적 검사 성립 조건 (b) b′ 등록명 1개면 충분(산물 쪽 개명이 기계적) (c) EH-프리는 여전히 b′ 몫 — b′ 단독은 실증됐으나 결합물 실증은 실물 합류 대기(§5-5).
3. **개명 단계 규율** [실측]: 프로덕션 b″ 개명은 매니페스트 `imports` 정본으로 import 섹션만 바이트 재작성(code 섹션 불변). 스파이크의 텍스트 왕복(print|sed|parse)은 실증용.
4. **링크-후 바이트 게이트 원리적 불가** [실측]: wasm-merge 산물은 재인코딩물(+custom 섹션) — pre-link 검증기/R1·R2 바이트 게이트를 링크-후로 연장 불가. 링크-후 게이트는 의미-수준 + 실행-차등(오라클 비교)으로 설계.
5. **트랩 할당자 결정 승계** [실측]: 게이트 프로파일은 trap-alloc 유지 권고(무할당 증명 겸용). 엔진-라이브러리 확장 시 실할당자 + 스크래치 규범 재산정 동반.
6. **헬퍼 트랩 vs 가드 트랩 진단 구분 불가** [실측]: 둘 다 `UnreachableCodeReached`. 진단 구분 필요 시 O-16c′에서 헬퍼 트랩 전용 표지(예: 별도 trap 함수 export) 검토.
7. **shared/+atomics stable 경고 → O-W 워치리스트 등재 필요** [실측]: stable에서 빌드·검증 성립하나 "not stably supported" 경고 — 보증 없는 통과. 경고→에러 전환 시 shared 프로파일만 nightly/build-std 재논의(비공유 게이트 경로는 무영향). b″ 쪽도 공유판에 `--enable-threads` 필요 — 실물 b′ +atomics 빌드와의 정합은 미실측(§5-5).
8. **export 합집합 → 축소 후처리** [실측]: 결합 산물 export 8종 잔존(라이브러리 memory/헬퍼/호스트 + 산물 otter_entry/otter_table) — 표면 최소화 필요 시 별도 단계.
9. **컴포넌트화 함정 2건** [실측]: `component new` 단독은 빈 world — WIT world 명시 + 코어 export kebab-case 개명(`otter_entry`→`otter-entry`) 필수. 엔트리 ABI의 canonical 하강은 `(i32,i32,i64)->i64`로 정확 일치.
10. **wasm-merge 피처 플래그 필수** [실측]: 기본 MVP — 미지정 시 Fatal. 최소 `--enable-tail-call` + `--enable-nontrapping-float-to-int`(대역 convert 유래 — 실물 b′의 요구 플래그 전수는 합류 시 재확인), 공유판 `--enable-threads`.
11. **b″ 입력 형상** [실측]: cdylib 모듈(임포트 0, memory 소유-export)이 정답 짝 — staticlib/wasm-ld flat-merge는 산물의 무재배치 성질(§2.5-iii)과 부정합(양 스파이크 상호 확인: b′ 권고 + b″ wasm-ld 불성립 실측). wasm-ld의 자리는 b′ 내부(rustc→wasm-ld) 뿐.
12. **§7 nightly 탈출 주장의 최종 단계** — b′ 단독 산물은 [실측] 탈출 확인. 전체-체인(실물 b′ 결합물) 단위의 EH-프리 실증은 재실행 항목(§5-5) — 완료 전까지 "최종 산물 탈출"은 [추정, 근거 강함].

---

## §5. 모순/미해소 (봉합하지 않고 기록)

1. **스크래치 예약 형태 상충**: b′ 결론은 "예약 메커니즘 불요 — 수치 규범 1줄(`scratch_base ≥ 0x100000`)이면 겹침 원천 불가". b″ 결론은 "b′ 산물에 **마커 data 세그먼트**를 요구(b″ 정적 검사의 성립 조건)". 그리고 b′ 실물 산물은 **data 세그먼트 0**이다 — 즉 b″의 요구를 현행 b′ 레시피는 충족하지 않는다. 상호 배타는 아니나(마커 추가는 소폭 수정), 마커 추가 시 b′ 산물 형상·크기 재측정이 필요하고 "data 0·무할당" 성질 서술도 갱신해야 한다. 결정은 O-16c′ 몫.
2. **스크래치 수치 상충**: b″가 실측에 쓴 매니페스트는 `scratch_region=[0x10000,0x10008)` — b′ 실물의 섀도 스택 [0,0x100000)와 **겹친다**. 대역 b′(WAT, 스택 없음)에서는 무해했으나 실물 b′ 합류 시 이 base는 위반. 더 중요한 함정: b″ 정적 검사 (i)은 data 세그먼트 기준이라 **섀도 스택 겹침을 못 잡는다**(스택은 세그먼트가 아님) — b″가 부기한 "b′ 전역 심볼(힙 베이스 등) 대조" 항목이 이를 커버해야 하며, 실물 b′의 `__stack_pointer`=1048576 기준 대조는 미실측. O-16c′는 스크래치 규범 수치(≥ 0x100000 / +atomics ≥ 0x110000)를 매니페스트 생성 측(compile_module 호출자) 규범으로 명문화해야 한다.
3. **커버리지 상충**: b′ 실물 메모리는 `(memory 16)` = 정확히 섀도 스택 1 MiB — 스크래치 규범(base ≥ 0x100000)을 채택하면 b″ 커버리지 검사(min ≥ ⌈end/65536⌉)에 **현행 b′는 불합격**(min=16 < 17). 해소 후보: ① 마커 data 세그먼트를 scratch 구간에 두면 active 세그먼트가 min 페이지를 자동 상향 강제(§5-1 채택 시 부수 해결 — 단 이 상호작용은 [추정], rustc/wasm-ld 링크 인자 실측 필요) ② 초기 메모리 상향 링크 인자 ③ 커버리지 게이트가 memory.grow 의미론을 수용하도록 재정의. 미결.
4. **otter_host 제공자 미정**: b″의 단일-이름공간 개명 그린은 대역이 **host까지 한 모듈에서 제공**한 형상(otter/otter_helpers/otter_host 전부 → `otter_lib`). 실물 b′는 헬퍼+memory만 제공하고 host를 제공하지 않는다(B1의 최소 형상 확정과 정합). 따라서 실물 체인에서 개명 범위는 otter+otter_helpers까지이고, `otter_host.*`는 (a) 별도 제공자 모듈로 wasm-merge에 등록하거나 (b) 잔존-임포트로 남겨 컴포넌트-수준 임포트로 리프트해야 한다 — (b)는 "잔존-임포트 0 게이트"와 충돌(게이트에 host 예외 규정 필요)하고, (a)는 host 함수의 정본 소재(임베더 제공물)와 층위가 안 맞을 수 있다. L4 그린(잔존 0)은 대역-host 덕분이었음을 명시 기록. O-16c′ 설계 결정 필요.
5. **실물 b′ × b″ 결합 미실행** (b″ 정직 표기 승계, b′ 산물이 이제 존재하므로 즉시 재실행 가능 — 전부 기존 커맨드 재사용으로 충분): ① 결합물 EH-프리 실증(§7 탈출의 최종-산물 단위 주장) ② wasm-ld가 만든 b′의 섀도 스택 글로벌·힙 심볼과 scratch_region의 실제 겹침 검사 ③ +atomics b′ ↔ `--enable-threads` 결합 정합(§12.1 shared_memory 행) ④ 실물 b′ convert 경로가 요구하는 wasm-merge 피처 플래그 전수 재확인.
6. **이름공간 "자명" 판단의 층위 차이**(경미, 기록만): b′의 "매핑 자명" 판단은 **런타임 인스턴스화-그래프 배선** 기준(링크 스모크), b″ 확정 체인은 **정적 wasm-merge** 기준 — 후자에서는 개명 단계가 실제 의무다(단일 파일 3중 등록 기각 실증). 최종 체인이 b″이므로 O-16c′ 문서는 b″ 기준으로 서술할 것.

---

## §6. 해소 결과 부기 (O-16c′ 구현 완료 — L-6 기록; §5 원문은 규율대로 비봉합 보존)

§5의 상충 6건은 설계 문서(`otter-o16c-componentization-design.md`) §2가 결정하고 증분 L-1a~L-6이 실물화했다. 각 항: **결정 → 착지 증분(커밋)**.

1. **§5-1 예약 형태** → **마커 data 세그먼트 채택, 생산 주체는 otter-aot-link `inject-marker`**(rustc/wasm-ld 아님 — 설계 §2-1; 폭-분기 W∈{0,8,≥16} 의미론 동반). 착지: L-1b(주입기+검사, `5b2ea5e`) + L-2(형상 재측정 프로파일-분리, `fbb1c2a`) + L-4(병합-후 보존 실측, `ca2e06c`).
2. **§5-2 수치 규범 소재** → **3층: 호출자 규범 명문 + b′ 사이드카(heap_floor 공식에 memory_min 항 — 패널 M1 정정) 정본 + b″ 검사 (iii) 신설**(설계 §2-2). 섀도-스택 함정은 검사 (iii)이 직접 봉합 — 실물 대조 실측: L-4(0x100000/0x110000 사이드카 상대 그린 + base=0x10000 위반판 (iii) 적색). 파급 정정 3건(픽스처 규범값·SCRATCH_BASE 0x110000·MEMORY_PAGES 18) 전건 L-4 착지.
3. **§5-3 커버리지** → **후보 ① 채택하되 min-상향의 주체를 주입기로 옮겨 [추정] 소거**(구성적 이행 — 설계 §2-3) + 독립 검사 (ii) 병치. 착지: L-1b/L-4. L-6 뮤테이션 ③이 발현 양쪽을 실측: 마커-실존 산물은 인스턴스화 시점 적색(벨트-앤-서스펜더), 마커-제거판은 E8 축자 재현(첫 마셜링 store trap, 0x110000).
4. **§5-4 otter_host 제공자** → **후보 (b) 잔존-임포트 → 컴포넌트-수준 리프트 정본**(잔존 게이트는 정확-일치 = 로 정밀화 — 설계 §2-4; 후보 (a) 기각 사유 동봉). 착지: L-1a(게이트) + **L-5(`6a22004`) — 리프트 실측 성공**: 규약 `$root`+kebab, wasmtime 48 컴포넌트 Linker host-배선 오라클-동일. 본 절이 예고한 "게이트에 host 예외 규정 필요"는 manifest otter_host 집합과의 정확-일치로 착지, "층위" 우려는 (b)가 회피.
5. **§5-5 재실행 4항** → **전부 L-4 게이트로 일괄 편입**(설계 §2-5). 착지: L-4 전건 그린 — ① 결합물·컴포넌트 EH-프리(§4-12 [추정] → 최종-산물 단위 [실측] 승격), ② 검사 (iii) 실물 첫 실측, ③ shared 전-체인 + 확장 실측(**정정 1건: `__tls_base`는 wasm-merge가 미참조 전역으로 소거** — 설계 §0 착지 부기 1), ④ 플래그 전수 이진 탐색(**census 신규: shared는 `--enable-bulk-memory` 필수** — 대역 손-WAT가 못 보던 사실; 5종 drop-one 프로브로 매 실행 검증).
6. **§5-6 서술 기준** → **b″ 정적 wasm-merge 기준 채택**(설계 §2-6; 개명은 import 섹션만 바이트 재작성 + 모드별 불변 게이트). 착지: L-1a rename/invariance + L-4 실물 체인. O-14 §11-1 종결 동반.

부기: §4-12의 "[추정, 근거 강함]"(최종 산물 nightly 탈출)은 L-4에서 [실측]으로 승격 — 결합물·축소물·컴포넌트 전건이 `validate --features=all,-exceptions,-legacy-exceptions` + census 0 통과, L-6 뮤테이션 ⑧이 거부 방향(try_table 주입 → 적색)을 실증해 게이트 비공허성까지 닫음.
