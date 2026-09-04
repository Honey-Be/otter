# H-패키지: 출하-전 경화 1차 — 설계 노트 v2 (3-렌즈 패널 반영)

- v1 → v2: 3-렌즈 적대 패널(2026-09-04, 28건 — 치명 1·중대 12·경미 9·참고 6)을 전량 반영.
  핵심 전환: **H1a 깊이-단독 가드 기각** (패널 실측: 깊이×프레임 곱이 무계 — nlocals=1024 T1이
  2MiB 스레드에서 깊이 ~224에 abort, 기본값 256 도달 전; 스크래치 하네스 h1probe로 재현) →
  **바이트-예산 주가드 + 깊이 백스톱** 채택.
- 근거: RELEASE-GATE 감사 대장 + 오너 정책(강력 권장·비의무·비전수) + tsavorite 질의(H5).
- 스코프: H1(a/b/c) · H2 · H3 · H4 · H5. 비스코프: 상수 블라인딩, CET 썽크, SLS INT3(2차),
  shadow-stack 실기 CI 레인.
- **패키지 내 착지 순서(규범)**: H2(endbr64) → T1 golden 신설(캡처) → H1b(golden 위에서
  "frame≤4096 무변화" 검증) → H1c → H1a → H3 → H5 → H4. (패널 ②-3: 순서 미명문화 시
  golden이 H2와 자기모순.)

## H1a — 스택 가드 (엔진, ModuleEngine 경로 전용)

### 메커니즘 (v2: 2중 가드)

1. **바이트-예산 가드(주)**: `dispatch()` 진입 시 로컬 변수 주소를 SP 프록시로 취득.
   `depth: Cell<u32>`가 0→1 전이하는 최외곽 진입에서 `stack_base: Cell<usize>`에 기록;
   이후 각 진입에서 `base − sp > stack_budget`이면 기존 trapped 채널로 Trap.
   기본 `stack_budget = 1MiB`. 티어·프레임크기 무관하게 정확, 왕복당 뺄셈+비교 1회.
   rustdoc: 예산은 "otter **최외곽 진입점** 기준"(그 위 임베더 스택은 임베더 몫).
2. **깊이 백스톱(보조)**: `depth_limit` 기본 **4096** (폭주-재귀 백스톱 — 소형 프레임에서
   예산보다 먼저 걸리지 않도록 크게). 실측 근거(패널): 왕복 = T0 1,328B(nlocals-독립, locals가
   힙 Vec) / T1 ≈1,120B + 8×(nlocals+3+maxargc) / T2 1,136B(DCE 후). 1MiB 예산 ≈ 소형 프레임
   800+레벨(기존 v1 기본 256 대비 3배 완화 — JS-류 워크로드 회귀 제거), nlocals=1024 T1 ≈
   112레벨에서 **abort 대신 Trap**.
3. **API**: `set_stack_budget(usize)` / `stack_budget()` / `set_depth_limit(u32)` /
   `depth_limit()`. 0 금지(assert). budget 기본값에서 **컴파일러 첨두 예약**을 차감해 문서화
   (한도-근처 티어업 시 Cranelift 컴파일이 최심부에서 돎 — 대형 함수 1회 실측 상수 기재).
   rustdoc 캘리브레이션 문단: 2MiB(Rust 스레드)/8MiB(주 스레드)/**1MiB(wasm32-wasip1 rustc
   기본 — `-C link-arg=-zstack-size`로 조정)/128KiB(musl pthread foreign 스레드)** 전 케이스
   명기(tsavorite 실측 교훈 2026-09-04 반영: 임베딩 환경의 실제 스택이 기준).

### 트랩 의미론 (v2 정정 — §2.4가 아니라 O-10)

- 한도-도달은 **언와인드 없이** 플래그 set + fill 정상 반환 — 상태 무결성이 패닉-트랩보다
  강하게 성립. **O-10 clear_trap 계약 적용(회복 가능)**: 한도 재조정 + clear_trap 후 재시도
  가능. §2.4 폐기-전용은 wasm 트랩 전용(오인용 정정). 플래그-기반이라 비-try `run()`/
  `dispatch()` 경로에서도 abort를 막는 부수 이득(O-12 known-gap 부분 완화) — 긍정 명기.
- **`TrapReason` 도입**: `#[non_exhaustive] pub enum TrapReason { Panic, DepthLimit,
  StackBudget }` + `Cell<TrapReason>` + `trap_reason()` 게터. Trap 구조체 무변경.
  FFI-경계 3곳은 Panic, 가드는 각 사유 기록. rustdoc 4곳(trapped/trapped_idx/Trap/clear_trap)
  갱신 — 특히 clear_trap의 "결정론적 원인은 재시도 시 동일 재트랩" 문언에 가드-Trap 예외 명기.
  핀: 이미-trapped면 reason/idx 보존.

### 구현 3핀 (패널 ①-6 — 위반이 '자연스러운' 오답들)

- (i) **depth/base의 유일한 감소·해제자는 RAII Drop** — `clear_trap`은 건드리지 않는다
  (트리 중간 호출 가능). Drop에 `debug_assert!(depth > 0)`, Drop 내 panic 금지.
- (ii) **검사 순서 고정**: trapped → in_range → 가드(budget/depth). 한도-도달 세터는
  `!trapped.get()`일 때만 기록(기존 trapped_idx 보존 계약 유지).
- (iii) **카운트/검사는 dispatch() 함수 진입 1회** — tail 트램폴린 루프 반복은 비카운트
  (proper-tail-call 보장 회귀 방지). 핀 테스트: depth_limit=8에서 tail 재귀 10만 회 성공.

### 스코프 (패널 ①-7 명문화)

가드는 **ModuleEngine 경로 전용**. (a) CallMode::Table AOT 산물은 dispatch() 비경유 —
B10 비신뢰-입력-금지 규범에 위임(A6 대장에 잔존 명기). (b) `run_module`/`run_one` 직접
호출(오라클)은 힙 Vec 스택 — 무계 힙 성장(OOM abort) 잔존, 오라클-전용 규범에 위임.
(c) 단일함수 `otter::engine::Engine`은 재귀 네스팅 구조 부재(패널 실측: Call/TailCall
unreachable) — 가드 불요. (d) T0-고정(H5) 경로 포함 전 티어 경유 커버(패널 실측 확인).

### 게이트

- 음성(가드 발화): 한도-초과 재귀 → `try_dispatch` Err(Trap) + trap_reason 단정(in-process,
  abort 아님). 양성: 한도-내 깊은 재귀 전 티어 오라클 동일 — **`Builder::stack_size` 명시
  스레드(16MiB)에서 실행**(debug 프로파일 flaky 방지).
- 뮤테이션(가드 제거 → abort 재현): **current_exe 재실행 서브프로세스 하네스** — env 가드
  (`H1_DEPTH_CRASH=1`)된 숨김 #[test]를 `--exact`로 자식 실행, `ExitStatusExt::signal()`/
  exit 134 단정. (v1의 "L1a_gates 패턴 재사용"은 CARGO_BIN_EXE 기반이라 lib 크레이트에
  부적용 — 각색 명세로 교체.)
- Twasm 경로 왕복 실측(wasip1 + wasmtime 호스트) 1회 — 문서 수치 보강용(게이트 아님).

## H1b — T1 대형-프레임 스택 프로브

- **형상(확정)**: cranelift-미러 **sub-후-터치**(rsp 아래 원격 터치 금지 — 커널 스택-성장
  휴리스틱/valgrind/wasmtime#7454 근거). `frame > 4096`일 때만, 기존 `sub rsp, frame`
  **앞에** 삽입(기존 sub 바이트 불변): npages = frame/4096 내림.
  npages ≤ 4 언롤: `[48 81 EC 00 10 00 00 (sub rsp,4096); C7 04 24 00 00 00 00
  (mov dword [rsp],0)] × npages; 48 81 C4 imm32 (add rsp, npages*4096)`.
  npages > 4 루프: `B9 imm32 (mov ecx,npages); .L: sub rsp,4096; mov [rsp],0; FF C9
  (dec ecx); 75 EE (jnz .L); add rsp,npages*4096`. **스크래치는 rcx 한정** —
  rdi/rsi/rdx/rax는 프롤로그 시점 라이브(ABI 인자; 모듈 경로 스필은 프로브 뒤) — 금지.
- **전제 정정(패널 ②-2)**: 모듈 경로는 "하향 순차 초기화가 fail-stop 보장"이 **거짓** —
  sub 직후 첫 터치가 rdi/rsi/rdx 스필(프레임 최심부, locals보다 깊음)이라 frame > guard면
  **무성 손상 창이 이미 존재**(포인터 값이 인접 매핑에 기입된 후 locals 루프가 fault).
  H1b가 이를 해결하되, A6 대장·RELEASE-GATE의 해당 서술 정정을 착지 커밋에 포함.
- **골든/게이트**:
  1. **emit/map 분리 리팩터**: `fn emit_fn(...) -> Vec<u8>`(단일함수/모듈함수) 내부 도입,
     compile = emit + ExecBuf::map (릴리즈 동작 무변화) — 바이트 단정의 전제.
  2. 골든 코퍼스는 **주소-프리 한정**(순수 int ALU + jmp/halt 단일함수, CallMode::Table
     모듈함수 — movabs는 결정적 consts뿐) 또는 movabs imm64 홀 8B 마스킹.
  3. **구조 단정(킬러 게이트 — 뮤테이션 비공허성의 본체)**: 프롤로그 바이트 파서로
     frame > 4096 ⇒ 프로브 시퀀스 존재·각 sub ≤ 4096·sub 사이 터치 ≥ 1·누계+복원 == 정합,
     **첫 메모리-스토어가 프로브임**(무성-손상-창 봉인 단정); frame ≤ 4096 ⇒ 프로브 부재·
     golden 동일. 경계쌍 **4096/4112**(frame은 16의 배수 — 4097은 도메인 밖; 단일함수
     nlocals 512/513, 모듈함수 nlocals 509/510). 뮤테이션(제거/간격 8192/순서 역전) 전부
     이 단정에 죽음을 뮤테이션 레저에 대응표로 기재.
  4. 대형-프레임 실행 게이트 + **live-locals 변형 1본**(T2 프레임 성장 실증용).
- 티어 비대칭 1건 무해 명기: 정확히 4096에서 T1 무프로브 vs T2 프로브(probe_count=1).
  guard ≥ 1페이지 가정(guardsize(0) 임베더 제외)도 rustdoc 1줄.

## H1c — T2 cranelift inline probestack

- **플래그 원자쌍**: `[("enable_probestack","true"),("probestack_strategy","inline")]` 단일
  상수 배열(strategy 기본이 outline이라 반쪽 구성은 __cranelift_probestack 미해석 심볼로
  finalize 실패 — 세트 근거 명기). `probestack_size_log2` 기본(12) 유지.
- 실증 완료(패널): 0.132.3 x64에 inline 경로 실재(gen_inline_probestack, PROBE_MAX_UNROLL=4,
  sub-후-터치) + `with_flags` 최소 구성 API 지원 + new_module 단일 구성점이 전 컴파일 경로
  커버 + 소형 프레임(frame < 4096) T2 바이트 무변(probe_count 내림).
- **게이트**: (i) `Flags::to_string()` diff가 정확히 두 키뿐임을 단정(비접촉의 기계화);
  (ii) 프로브 존재: 대형-프레임 코퍼스에서 기본 vs 플래그 JITModule 코드 길이 차 > 0, 소형
  == 0 (또는 set_disasm vcode 단정). **코퍼스는 argc ≥ 520 Call 모듈 명시 편입** — T2
  프레임은 regalloc 산물이라 nlocals 비례가 아니고 명시 스택슬롯(argbuf)이 결정적 트리거.

## H2 — T1 endbr64 선방출

- 두 프롤로그(compile :416-420 / compile_module_func :757-760) 선두 `F3 0F 1E FA`.
  안전성 패널 검증 완료: 내부 rel32는 전부 code.len() 기반 동적 패치라 비영향; 비-CET NOP.
- **킬러 게이트**: 대표 코퍼스 전 함수(compile·compile_module_func·양 CallMode) 선두
  4바이트 == `F3 0F 1E FA` 단정.
- 착지 시 **RELEASE-GATE B1 근거 문장 교체**("0x1E 0건" → 선두 endbr64 존재+단정 테스트
  참조) 명시 항목. tsavorite 통지(조건 2 발화).

## H3 — install_* debug_assert (v2: 3함수 × 양 브랜치)

- `install_baseline`/`install_cranelift`: 원본+pushed **양 브랜치** 각각
  `debug_assert!(slot.is_none())`.
- **`install_wasm` 추가(패널 ③-3)**: 동일 위험 클래스 3번째 — WasmSlot 덮어쓰기는
  CompiledWasmFn::scratch Box drop인데 그 주소가 방출 코드에 상수로 베이킹(:747)돼
  dangling-scratch(T1 munmap-UAF의 데이터판 동형). 양 브랜치에
  `debug_assert!(matches!(&*slot.borrow(), WasmSlot::Untried))` — borrow는 단정 문 종료 시
  drop(기존 단일-식 borrow 규율 유지). 무-오탐 확인 완료(호출자 maybe_tier_up뿐,
  재컴파일/OSR 부재).

## H4 — 방출-opcode 인벤토리 게이트 (v2: 2단 + 모드-분리)

- (i) 산물별 opcode 집합 ⊆ 화이트리스트(22종). (ii) **코퍼스 전체 합집합 == 화이트리스트
  (집합 동등)** — 커버리지 붕괴·죽은 항목 즉시 적색. (iii) **모드-분리 단정**: Dispatch-모드
  산물에 ReturnCallIndirect/I64GeU/I32WrapI64 **부재**(A-모드 전용 명령의 JIT 표면 누출
  철조망 — tail-call 미지원 호스트 호환성).
- 코퍼스 요건(패널 실측 방출 조건): 양 CallMode × 점프 함수(Block/Loop/Br/BrTable/I64Eqz/
  Unreachable) × nparams ≥ 1(I64GtS/I64Load) × Dispatch TailCall(Drop) 전부 포함해야 (ii)가
  성립. 파서는 **aot.rs:250 body_ops를 tests/common으로 승격** 재사용(이중화 금지).

## H5 — T0-고정 지원-계약 승격 (v2: due() 헬퍼 + Engine 대칭 + t1=NEVER 완화)

- **`NEVER` 상수는 otter(L0)에 정의, otter-engine이 재수출** (의존 방향상 양 API 참조 가능한
  유일 배치).
- **판정 헬퍼 추출(패널 ③-1 — 뮤테이션 비공허성의 본체)**:
  `#[inline] fn due(c: u32, threshold: u32) -> bool { threshold != NEVER && c >= threshold }`
  — 3개 지점(:846/:867/:886)이 전부 이 함수 호출(사본 술어 금지 — no-dual-source 규율).
  단위 테스트가 **실함수**를 경계 전수 고정: (MAX,MAX)→false, (MAX,MAX−1)→true, (0,0)→true,
  (MAX−1,MAX)→false. 통합: NEVER 다수 호출 후 tier_of=="interpreter".
- **assert 완화(패널 ③-2 결정)**: `assert!(t1 <= t2 || t1 == NEVER)` — 3모드 rustdoc 표:
  둘 다 NEVER=T0 고정 / t2만 NEVER=T1 상한 / **t1=NEVER,t2=real=T2 직행(T1 RX 페이지
  무방출 — B2 표면 축소 관점 보안-유관)**. 각 모드 핀 테스트(t2=NEVER: "baseline" 안정·
  절대 "cranelift" 아님; t1=NEVER: "interpreter"→"cranelift" 직행·baseline_ptr 영구 None).
- `ModuleEngine::interpreter_only(isa, module)` = new(NEVER, NEVER).
- **Engine 대칭(패널 ③-5, 택 B)**: 단일함수 `Engine::call` 카운터를 `saturating_add`로 통일
  + 판정에 `!= NEVER` 가드(OSR backedge 카운터도 포화로 통일). H5 계약 문언이 두 API 모두에
  참이 되도록.
- 문서: new() rustdoc(0=첫 호출 컴파일 / NEVER=비승격 / 그 외 실문턱) + 기존 "must NEVER
  promote" 테스트(tailcall.rs:135-140 등 T0-핀 성격)를 NEVER/interpreter_only로 이관 +
  릴리즈 노트 1행(u32::MAX 의미 변경 — 실사용 0건 전수 grep 확인) + depth_limit의
  u32::MAX("실한도, 도달 불가")와 문턱 NEVER("명시 제외, 절대 비발화") 상호 참조 1행씩.

## 공정 (v2)

착지 순서: H2 → golden → H1b → H1c → H1a → H3 → H5 → H4. 구현 → 2-렌즈 독립 리뷰(렌즈 A:
가드·계약 의미론 재도출, 렌즈 B: 방출 바이트·게이트 비공허성 재검증 + 뮤테이션 레저 실행) →
커밋(otter main) → 대장 처분 갱신(RELEASE-GATE B1 문장 교체·A6 서술 정정·감사 대장 상태) →
tsavorite 통지(endbr64 조건 2 + H5 답: NEVER/interpreter_only 지원 계약). 산물 바이트 변경 =
H2(전 T1 +4B)/H1b(대형 프레임)/H1c(T2 대형 프레임) — 전부 차등 + 구조 단정 게이트.
