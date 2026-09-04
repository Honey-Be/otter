# otter 외부 재사용화 계획 (프로젝트 무관 메타-VM 프레임워크로)

Status: PLAN (2026-09-01, 사용자 요청). 목표: `otter`(+ `otter-baseline`/`otter-cranelift`/`otter-engine`)를
neosurf 밖의 다른 프로젝트에서도 의존·사용할 수 있는 독립 프레임워크로 만든다. neosurf는 otter의 첫 소비자로 남는다.

## 0. 현황 진단 (좋은 출발점)

- **이미 언어무관·프로젝트무관**: otter* 4개 크레이트는 `neosurf-*`를 **전혀 참조하지 않음**(DESIGN.md 로드맵
  주석 1건 제외). 값은 `Value=i64`(임베더 소유), 호스트 확장은 fn 포인터(`Host1/2`), init 워드 `fill`도 임베더
  소유 → 설계가 이미 "임베더 계약" 기반. neosurf(JS)와 곧 나올 WASM 프론트엔드가 **같은 otter를 공유**하는 게 증거.
- **이미 프레임워크 명명**: `otter`, `otter-baseline`, `otter-cranelift`, `otter-engine`(프로젝트 접두어 없음).
  프로젝트 전용 크레이트만 `neosurf-*`(jsvalue/gc/runtime/wasm).
- **의존 최소**: otter=`half`(optional, f16)뿐. baseline=`libc`. cranelift=`cranelift-* 0.132`. engine=백엔드들.
- **라이선스**: BSD-2-Clause-Patent(허용적, 외부 재사용 적합).

## 1. 외부 재사용을 막는 실제 장애물 (고쳐야 할 것)

1. **크레이트 간 `path`-only 의존** [BLOCKER]: `otter = { path = "../otter" }`는 워크스페이스 내부에서만 유효 →
   외부 프로젝트가 crates.io/git로 못 당김. 각 의존을 `path` + `version` 병기로(로컬 개발=path, 배포=version).
2. **버전 0.0.0 + publish=false** [BLOCKER]: otter*가 워크스페이스 버전(0.0.0) 상속 + publish 금지. otter*는
   **워크스페이스와 독립된 semver**(예: 0.1.0)를 각자 지정(`version.workspace=true` 해제) + `publish=true`.
3. **`#[non_exhaustive]` 0개** [semver 취약]: 공개 enum(`Expr`/`Effect`/`ArithOp`/`NumTy`/`UnOp`, `Isa`/`OpDef`
   /`Inst` 필드)이 exhaustive → 변형 하나 추가가 외부 코드에 breaking. 향후 변형 추가가 잦은 IR enum에
   `#[non_exhaustive]` 부여(단 otter 내부/neosurf는 여전히 전 변형 매치 — 같은 크레이트/워크스페이스는 예외).
4. **README 없음** [DX]: DESIGN.md만 존재. crates.io 랜딩용 README + 최소 "otter 위에 언어 얹기" 예제 필요.
5. **아키텍처별 티어 가용성 미문서화**: interp(otter)=전 아키텍처, baseline=**x86_64 전용**(compile_error),
   cranelift=Cranelift 타깃(x86_64/aarch64/riscv64/s390x). 외부 소비자가 타깃별로 티어를 골라야 함 → 명시.
6. **MSRV/toolchain**: stable 1.96 + edition 2024. 외부 소비자에 MSRV 공표(`rust-version`은 이미 있음).

## 2. 배포 전략 (권장 + 대안)

- **권장(1단계): "워크스페이스 유지 + 배포가능화"** — otter*를 neosurf 워크스페이스에 그대로 두되 path+version
  병기·독립 semver·publish=true·non_exhaustive·README를 갖춘다. 그러면 **동일 소스가** (a) neosurf 로컬 빌드,
  (b) 외부 프로젝트의 crates.io 의존, (c) 외부 프로젝트의 git 의존(`git=..., branch=...`, 서브디렉터리)로 **모두**
  쓰인다. 마찰 최소, 저장소 분리 불요.
- **대안(2단계, 필요 시): 별도 저장소 분리** — otter 릴리스 케이던스를 neosurf에서 완전 분리해야 할 때
  `git subtree`로 `otter/` 를 독립 repo로 추출(히스토리 보존), neosurf는 git/crates.io 의존으로 역참조. 1단계로
  대부분의 재사용 수요가 충족되면 보류.
- **실제 crates.io 게시는 outward-facing/비가역 액션** → 별도 사용자 확인 하에 수행(이 계획은 게시 준비까지만).

## 3. 작업 증분 (O-1..O-6; 각각 그린 테스트 + no_std + clippy/fmt)

- **O-1 배포가능 메타데이터**: otter* 4개에 독립 `version`(0.1.0) 지정, `publish=true`, 크레이트 간 의존을
  `{ path="..", version="0.1" }` 병기. `cargo package -p otter --no-verify` 드라이런으로 게시가능성 확인.
- **O-2 API 안정화**: IR 공개 enum/구조체에 `#[non_exhaustive]`(neosurf/otter 내부 매치는 워크스페이스 예외로
  유지). 공개 항목 99개 rustdoc 커버리지 채우기(`#![warn(missing_docs)]` 도입 후 무경고화).
- **O-3 README + 예제**: `crates/otter/README.md`(무엇/왜/임베더 계약/티어/아키텍처 표) + `examples/toy_lang.rs`
  (in-crate `sample` ISA를 승격: Value/fill/host-op/Isa/Module → interp+baseline+cranelift 실행 최소 예제).
- **O-4 독립 빌드 게이트**: CI/justfile에 "neosurf 없이 otter만" 빌드/테스트 스텝
  (`cargo build -p otter -p otter-baseline -p otter-cranelift -p otter-engine` + 각자 test). 역결합 회귀 방지
  (otter*가 실수로 neosurf-*를 import하면 실패하도록).
- **O-5 아키텍처 매트릭스 문서화**: README/DESIGN에 티어×아키텍처 표(interp=all, baseline=x86_64,
  cranelift=CL 타깃). `arch-support-tiers` 메모리와 정합. baseline 미지원 아키텍처에서 engine이 baseline을
  건너뛰고 interp→cranelift로 tier-up 하는지 확인(가용 티어만 등록).
- **O-6 (보류) 저장소 분리**: 필요 시 git subtree 추출 + 역의존 전환. 1단계 충분하면 미실행.

## 3b. 외부 소비자 요구 (tsavorite / R7RS Scheme) — 실증된 확장 항목

첫 외부 소비자 `tsavorite`(R7RS-small Scheme)가 otter를 JIT/AOT 계층으로 쓰려고 gap 문서를 보냄
(`.local-requests-from/tsavorite/1788257155-otter-gaps-for-r7rs-hosting.md`, 회신
`.local-replies-to/tsavorite/re-1788257155-...md`). 정적 독해 전부 정확함을 소스 대조로 검증. 수용 항목
(설계+적대적 리뷰 게이트, 재사용 트랙 편입, 우선순위 순):

- **O-7 (=§5) 티어 타깃 게이팅** [DONE `407edff`]: otter-baseline을 비-x86_64에서 crate-root `#![cfg]`로 빈
  크레이트화(compile_error 제거) + otter-engine 백엔드를 target-gated 의존성으로(T1=`x86_64+OS`,
  T2=`비-wasm+OS`) + 티어 필드/사용처 cfg 게이트. 크로스빌드 검증: x86_64=T0+T1+T2, aarch64=T0+T2,
  wasm32-wasip2=T0. [[arch-support-tiers]]와 정합. (feature 플래그는 target-gate로 충분해 미도입.)
- **O-8 (=§1) `Effect::TailCall{base,argc}`** [O-8a `24daa5d` DONE + O-8b `67522e8` DONE + **O-8c REQUIRED
  (미착수)**]: 프레임 교체 호출(dest 없음). 3-렌즈 적대적 패널 통과(설계 `otter-o8-tailcall-design.md`,
  픽스 T1~T11). **O-8a**: Effect 변종 + run_module 프레임 교체(make_frame 등가+ret_dest 보존) + 1M-깊이
  오라클 테스트. **O-8b**: 엔진 트램폴린(run_one `TailReq` out-param 신호+리턴, 디스패처 플랫 루프 재진입,
  버퍼 swap 무재할당) + `interp_only` pre-scan(꼬리호출 함수는 T1/T2 미승격, T0 유지) + 1M-깊이 **네이티브
  스택 유계** 엔진 테스트. TLS 불요(out-param이 per-dispatch 로컬 → 패널 T10 run-격리/T4 TLS 이슈 회피 —
  당초 설계보다 개선). T1/T2 백엔드는 TailCall `unimplemented!`(interp_only라 도달 불가).
  **[[otter-tier-parity-policy]](2026-09-02) 제정으로 O-8c 재분류: "유보 가능한 perf" → "stable 릴리즈 필수"**
  (otter 전용 정책: 티어 간 기능 패리티 갭은 성능 차이와 달리 절대 불허 — "T0는 TCE 되는데 T1/T2는 안 됨"이
  정확히 금지 사례). O-8c 설계 = `otter-o8-tailcall-design.md` §8: baked `tail_signal` 헬퍼(기존 Call 마셜링
  경로 재사용 + 즉시 에필로그 낙하, ABI 무변경) + 엔진별(스레드-로컬 아닌) Cell/RefCell 신호 필드 + dispatch
  루프 통합 + `TierCaps{skip_t1,skip_t2}` 세분 승격자격. 증분 O-8c-1(T2 cranelift 먼저, IR API라 저위험) →
  O-8c-2(T1 baseline, 검증된 패턴 재사용). AOT 단일함수 API는 범위 밖(이미 Call 자체 미지원인 별개 제품이라
  정책 대상 아님). 자기-꼬리호출 네이티브 루프 최적화는 O-8d(순수 perf, 정책과 무관)로 유보.
- **O-9 (=§3) `ModuleEngine::push_func`** [DONE, 2026-09-02]: add-only 불변식을 함수 테이블로 확장.
  원본/push 인덱스 공간 분리(`base()` 경계), `pushed: RefCell<Vec<Box<FuncDef>>>`(개별 힙-박싱으로 주소
  안정성), `pushed_counts`/`pushed_baseline`/`pushed_cranelift` 병렬 성장 저장소. 재진입 명시적 지원(계약).
  설계+3렌즈 패널(5+3+5건 결함 발견, 전부 반영)+구현+2렌즈 리뷰(결함 0건, Miri로 신규 unsafe 검증) 통과.
  상세: `otter-o9-push-func-design.md`.
- **O-10 (=§4) `try_dispatch -> Result<Value,Trap>` + `clear_trap`** [DONE, 2026-09-02]: `dispatch()`/`run()`
  무변경, `try_dispatch`가 반환값이 아니라 `trapped` 플래그 전이로 트랩/OOB/정상값을 구별. `clear_trap`은
  `trapped`/`trapped_idx`/`tail_pending` 3필드만 리셋(RAII+add-only 덕분에 이걸로 충분함을 전수조사로 확인).
  설계+3렌즈 패널(Critical 1건: top-level 호출이 무방비였던 것 — `try_dispatch` 자체 `catch_unwind`로 수정;
  otter-baseline `ExecBuf` mmap/mprotect 순서 버그도 함께 수정)+구현+2렌즈 리뷰(CONFIRMED 2건, 둘 다 제가
  직접 rustdoc 정정+회귀 테스트 추가로 마무리) 통과. 상세: `otter-o10-try-dispatch-clear-trap-design.md`.
- **O-11 (=§2) `HostFnCtx1/2` + `Expr::HostCtx1/2`** [DONE, 2026-09-02]: tsavorite 원안(ModuleEngine 내부
  ctx 포인터 재사용)을 조사 후 기각(Table 모드·순수 오라클엔 그 배관 자체가 없음) — 대신 `Module`에
  `fill`과 동형인 `host_ctx: Value` 필드 추가(컴파일-타임 상수로 3티어+Table+Dispatch+push_func 전부
  균일 지원, ABI 무변경). 이름을 `ctx`가 아니라 `host_ctx`로 정한 건 패널이 지적한 대로 Dispatch 모드의
  기존 엔진 부기용 포인터("ctx"/`opaque_ptr`)와의 혼동을 막기 위함. 설계+3렌즈 패널(HIGH 1건: 두 렌즈가
  독립적으로 `otter::engine::Engine`의 Module-less 컴파일 경로(`compile_fn`/`compile_resumable`/
  `compile`) 누락을 발견 — host_ctx=0 고정으로 반영 + 그 외 5건)+구현+2렌즈 리뷰(0 confirmed 결함, 레지스터
  레벨 ABI 재검산+실재컴파일로 검증) 통과. 상세: `otter-o11-hostfnctx-design.md`.
- **O-12 (=§5b) `panic="abort"`에서 fail-closed 상실** [DONE, 2026-09-02]: 이전 "옵션3 우선" 방침을 실측
  후 뒤집음 — 옵션3 전면완료는 비현실적(패닉 사이트 27곳+, Cranelift 내부 패닉은 원리적 열거불가)이라
  **옵션2(빌드타임 `#[cfg(panic="abort")] compile_error!`, otter-engine에만, 전이적으로 전체 커버)를
  필수로 확정 + 옵션3은 `otter::validate::check_func`/`check_module`(신규 opt-in 모듈, 6개 검증 클래스)로
  좁게 스코프**. 조사 중 O-12 원 스코프보다 심각한 인접 결함 발견: otter-baseline(T1)은 malformed
  local-index에 패닉조차 없이 스택 메모리를 조용히 손상시킴(catch_unwind로 원천 봉쇄 불가, 사전검증만이
  막음) — 이게 옵션3 부분 포함의 결정적 근거. 3렌즈 패널+구현+2렌즈 리뷰(CONFIRMED 1건: `check_func`가
  `nparams+consts.len()<=nlocals` 불변식을 안 봄 — 제가 직접 `FrameLayoutTooSmall` 추가해 수정) 통과.
  [[release-rowhammer-gate]]·S5와는 별도 카테고리(가용성 vs 기밀성)로 정리. 상세:
  `otter-o12-panic-abort-hardening-design.md`.
- **§6 GC 루트 계약 (답변 완료)**: 현재 계약 = (b)+**핸들 간접**. otter 로컬은 이동 포인터가 아니라 안정 핸들
  인덱스(핸들 테이블=포워딩 테이블)라 프레임 순회 불요. (a) 프레임-walk 훅은 미제공·비committed. [[gc-allocator-plan]].
- **doc 픽스(반영 완료)**: `Effect::Call` stale 주석 수정(세 티어 모두 Call 하강).
- **O-13 (=신규, wasm-emit backend + WASI 지원) [REQUIRED — otter stable 릴리즈의 최종 선결조건, 사용자
  결정 2026-09-02]**: `.claude-research-library/wasm-emit-backend-research.md` 리서치가 실증한 4번째 코드생성
  전략(otter IR을 네이티브 대신 새 wasm 모듈로 lowering → 호스트에 넘겨 실제 기계어 생성 위임, .NET
  Jiterpreter가 프로덕션 선례) — otter가 stable을 태그하기 전 **반드시 완료돼야 하는 마지막 항목**으로 승격.
  범위는 브라우저/JS-host 경로만으로는 불충분: **WASI 호스트에서도 동작해야 함**이 요구사항의 핵심(리서치가
  확인한 "WASI엔 동적 컴파일 임포트의 표준 동등물이 없음"이라는 갭을 otter 자신이 자체 계약으로 메워야 함
  — WASI 표준화를 기다리지 않는다). 하위 항목(설계 단계에서 확정할 것, 초안):
  - **O-13a 설계**: otter `Isa`/`Module` → wasm 모듈 바이트 lowering(`wasm-encoder` 활용), CFG→구조적 제어흐름
    (relooper류, `cfallin/waffle` 프리아트 참고), 호스트 함수/테이블 링킹(Andy Wingo 2022 공유 linear
    memory+funcref 테이블 import 패턴). 위험 서브시스템([[verify-before-implement-dangerous-subsystems]]
    적용 — otter-cranelift급 신규 unsafe/FFI 인접 코드) → 설계 문서 + 적대적 패널 선행, O-8/O-8c와 동일 절차.
  - **O-13b "동적 컴파일 호스트" 자체 계약 정의**: WASI가 표준화하지 않았으므로 **neosurf/otter가 정의하는
    커스텀 import 인터페이스**(예: `env.otter_compile(ptr,len) -> funcidx` 류, 공유 memory+funcref table 기반)를
    사양화. 브라우저는 이 계약의 JS 구현 하나로 충족(공짜); **WASI는 참조 구현이 필요** — 최소 하나의 실제
    WASI 런타임(wasmtime 유력, Rust-first 정책과 정합)에 이 계약을 구현하는 호스트 shim을 otter* 생태계
    일부로 제공해야 "WASI도 지원"이 실증됨(단순 사양 문서만으로는 불충분).
  - **O-13c 구현 + 티어 편입**: 백엔드가 완성되면 [[otter-tier-parity-policy]]의 완전 적용 대상이 됨(예외
    없음 — TailCall 포함 모든 기존 IR 기능이 이 백엔드에서도 T0/T1/T2와 동등해야 stable 승격 가능). §8.10의
    wasm 예외조항은 O-13 완료 시점에 **자동 소멸**(더 이상 "cranelift AOT가 wasm에 없다"는 이유로 면제되는
    게 아니라, wasm-emit이 그 자리를 채우는 실제 티어가 되기 때문).
  - **[2026-09-02 설계 완료]**: 설계 v2(3렌즈 적대적 패널 반영 최종본)가
    `otter-o13-wasm-emit-design.md`로 확정됨. 핵심: §2 가설(wasm32에서 Rust fn 포인터 = funcref 테이블
    인덱스, transmute-호출 = call_indirect → 기존 call_compiled/주소-베이킹 관례가 소스 무변경 성립)이
    로컬 실측 + 패널 독립 재현으로 검증됨. 제어흐름은 디스패처-루프(loop+br_table) v1, 연산은 전면 헬퍼
    위임(정수 Div만 Refuse), TailCall은 기존 tail_signal 트램폴린 유지. 패널이 1차안의 인자 마셜링을
    이중 반증(크기 오독 CRITICAL + 차용-수명 unsound)해 함수별 스크래치 + module_dispatch 복사-즉시
    변형으로 전면 재설계됐고, Neg(F16) 3티어 불일치(T0=T1≠T2, 셋 다 IEEE 오답)를 실측 발견해 정본
    수정(W-0.5)이 선결 co-requisite로 승격됨. 증분 W-0~W-6, 규모 ~3.4k-5.6k LOC. **사용자 결정 대기
    2건**(§12): nightly build-std 승인 여부, Neg(F16) 수정 방향.
  - **순서상 위치**: 다른 모든 항목(O-9~O-12) 완료 후 마지막에 착수 — "최종 선결조건"이라는 표현대로 O-12
    다음, stable 태깅 직전 게이트. 지금 당장 설계 착수 아님, 트래킹만.

우선순위: O-7(DONE) → O-8(DONE) → O-9(DONE) → O-10(DONE) → O-11(DONE) → O-12(DONE) → **O-13(최종 게이트, stable 태깅 직전, 유일한 남은 항목)**. neosurf B2와 별개
트랙, 사용자 지시 시 착수.

## 3c. 해결됨 — call/cc (tsavorite 응답 수신, 2026-09-02)

tsavorite의 원본 gap 문서(§1~§7)에 `call/cc` 항목이 없어 후속 문의 발송했고, **tsavorite가 명확히 회신**
(`.local-replies-from/tsavorite/re-1788315812-followup-call-cc.md`): otter 레벨 continuation 지원은
**불필요** — full continuation을 만들면 가속 이득이 사라지고 만들지 않으면 동작 안 하는 근본 충돌 때문에,
자체적으로 heap 계속(CPS류)을 구현하는 방향으로 확정. **otter 트랙에 call/cc 관련 항목 없음** — 이전 초안이
가정했던 "O-13+ 급 설계"는 발생하지 않음, 그 번호는 위 wasm-emit 항목에 재배정.

## 4. 불변식 / 주의

- **임베더 계약 고정**: `Value=i64`·`fill` 임베더 소유·host op fn 포인터·`Isa`(opcode u16 인덱스)·`Module{funcs,fill,
  entry}`·`interp::run_module`/티어 백엔드 ABI는 외부 공개 계약 → semver 준수(0.x 동안도 CHANGELOG로 추적).
- **neosurf 역결합 절대 금지**: otter*는 앞으로도 `neosurf-*` 무의존 유지(O-4 게이트가 강제). WASM/JS 프론트엔드
  등 언어별 코드는 전부 `neosurf-*`(runtime/wasm) 쪽에.
- **f16 feature 계약**: `half-codec`(기본, stable) vs `f16-intrinsic`(nightly)은 **바이트코드 결과 불변**이어야
  함(교차-config 테스트가 게이트) — 외부 소비자에게도 이 보장 명시.
- **순서**: 이 계획은 문서화까지. 실제 O-1..O-5 구현과 crates.io 게시는 B2(WASM) 진행과 별개 트랙으로, 사용자
  지시 시 착수. **당면 작업은 B2 W0 S0-C(명령어 디코더)로 즉시 복귀**.
