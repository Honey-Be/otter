# otter 릴리즈 보안 게이트 (RELEASE-GATE)

모든 **testing/stable** 릴리즈는 출하 전 아래 취약점 클래스 목록에 대한 평가를 통과해야 한다
(main/unstable은 면제). 목록은 **append-only** — 새 클래스는 추가만 하고 삭제하지 않는다.
게이트의 목적은 "완전 방어 주장"이 아니라 **표면 추적과 릴리즈별 반복 점검 의무의 이행**이다.

## 체크리스트 (v1 — 2026-09-04 기준)

- **(a) DRAM/Rowhammer 계열**: Rowhammer · SMASH(JS-구동, TRR 우회) · GLitch(GPU/WebGL 경유)
- **(b) Spectre/천이실행**: v1/1.1/1.2 · Spectre-NG · SpectrePrime · SpectreRSB/ret2spec ·
  SpectreRewind · SMoTherSpectre(포트 경합)
- **(c) 캐시/타이밍**: Prime+Probe · SAB-as-clock("Fantastic Timers")
- **(d) 표면 등재부(append-only)**:
  - (d1) wasmtime `threads` crate feature — tier-2 표면 (2026-09-03 등재, `otter-o17-shared-memory-l0.md` §5)
  - (d2) wasmtime-wasi `p3` 모듈 — semver-면제·보안패치-비보장 표면 (2026-09-04 등재, `otter-o16f-p3-bridge.md` §6)

### 확장 등재 (2026-09-04, 오너 제공 "JIT 관련으로 mitigation이 필수적인 CPU 보안 취약점들" 목록 편입)

- **(b) 세부 확장** — Spectre/천이실행 클래스에 다음 세부를 append:
  Spectre v2/BTI(CVE-2017-5715) · Spectre v4/SSB(CVE-2018-3639) · Spectre-BHB(CVE-2022-23960) ·
  SLS(Straight-Line Speculation) · speculative type confusion · speculative stack smashing &
  스택 오버플로(방출 프레임 프로브 부재)
- **(e) MDS(마이크로아키텍처 버퍼 샘플링) 계열** — MSBDS/Fallout(CVE-2018-12126) ·
  MFBDS/ZombieLoad(CVE-2018-12130) · MLPDS/RIDL(CVE-2018-12127) · MDSUM(CVE-2019-11091)
- **(f) JIT 코드-방출 위생** — CFI 결여(CET IBT/ARM BTI 랜딩패드·전방 CFI) ·
  블라인드 JIT 스프레이→ASLR 무력화(상수 블라인딩 부재·코드 페이지 배치) ·
  런타임 코드-메모리 재사용 결함(UAF-of-code; 예시 CVE-2026-64507은 Linux 커널측 수정으로 실재 확인) ·
  shadow stack discrepancy(CET SS vs 트램폴린/언와인딩) · 레지스터 컨텍스트 경계 상태 누수
- **(g) 동시성/타이밍 확장** — 아키텍처적 race condition(atomic livelock · thread hopping) ·
  timer side-channel 일반화(루프-타이머·간접 시간-표면)
- **(c)-(ii) 의무 문구 개정**(강화 — append-only와 무충돌): "클럭 표면 목록화" →
  **"시간-드러내는 표면 전수 목록화"**(명시적 clock API + poll_oneoff 타임아웃 +
  fd/path_filestat 타임스탬프 + p3 monotonic-clock + 신규 배선 일체).

---

## 1차 평가: v1.0.0-testing.1 (2026-09-04)

**판정**: 조건부 통과(PASS with duties) — otter v1.0.0-testing.1 릴리즈 보안 게이트 1차 평가에서 exposed-open 클래스 없음. (a)(b)(c) 전 클래스가 임베더-구성-종속: otter는 임베더블 메타-VM 라이브러리로서 타이머 API·SAB API·스레드 스폰·GPU/웹 콘텐츠 실행 표면을 자체 제공하지 않으며(코어 no_std + forbid(unsafe_code), 단일-에이전트 규범), 공격 전제(고해상도 시계, 제2 실행 주체, 원시 메모리 주소지정)는 전부 임베더 또는 호스트 구성의 선택이다. (d) 등재 2건은 문서화된 완화(threads: 단일-에이전트 규범 + 동기 Store; p3: 기본-OFF feature 격리 + =48.0.1 락 핀)가 저장소에서 실측 확인됐다. 통과 조건 2건: ① CallMode::Table 전모듈 컴파일 경로의 callee 무경계 table 로드(/home/ybi/otter/crates/otter-cranelift/src/lib.rs:1046-1051)에 대한 지정 감사를 다음 릴리즈 전 완료할 것(v1-형상 가젯 후보이자 잠재적 아키텍처 OOB 표면), ② 게이트 근거 문서 otter-o17-shared-memory-l0.md가 otter 저장소 .claude-research-library/에 부재(정본은 neosurf 측) — 태그 전 동기화할 것.

### 통과 조건 처분 (평가자 제기 2건)

1. **CallMode::Table callee 무경계 table 로드** (`crates/otter-cranelift/src/lib.rs:1046-1051`,
   baseline 동형) — **지정 감사 결과: 이미 심사·수용된 갭**. 이 표면은 B-대장 **B10**(오너 기결정
   tier-parity 예외, 2026-09-02)과 동일 실체이며, 상위 검증 계층은 **존재하지 않는 것이 맞다**
   (Table 모드는 디스패처 미경유 직접 경로). 수용 전제 = **비신뢰 바이트코드를 이 경로에 절대
   투입하지 않는다**(B10 완화 규범). 본 게이트 관점 처분: B10 행에 Spectre-v1 가젯 후보 관점을
   병기하고, **전제(신뢰 입력 한정)가 깨지는 순간 즉시 재심**을 (b)클래스 릴리즈 의무에 편입.
2. **`otter-o17-shared-memory-l0.md` 저장소 부재** — 해소 완료: O-9~O-12/O-17/reusability 등
   누락 정본 6건을 태그 전 `.claude-research-library/`에 동기화(본 커밋).

### 클래스별 평가

#### (a) DRAM/Rowhammer 계열 — Rowhammer, SMASH(JS-구동 TRR 우회), GLitch(GPU/WebGL)

- **노출 판정**: `embedder-dependent`
- **근거**: otter 코어는 게스트(바이트코드)에 원시 선형메모리 주소지정 프리미티브를 제공하지 않는다: Value=i64 레지스터 IR이며, 컴파일 티어의 메모리 접근은 정적 오프셋(i*8) 프레임/인자 접근뿐이고 [실측: /home/ybi/otter/crates/otter-cranelift/src/lib.rs:517,896,1198], 임의 메모리 효과는 임베더가 등록하는 host fn(HostFn/HostCtx1/2) 경유로만 가능하다. 코어는 no_std + #![forbid(unsafe_code)]이고 타이머/클럭 API가 전무하다 [실측: /home/ybi/otter/crates/otter/src/lib.rs:27-29, 전 워크스페이스 grep에 Instant/SystemTime/clock API 부재]. 따라서 해머링에 필요한 고속·행-선택적 DRAM 접근 패턴은 (i) 임베더의 벌크-메모리 host fn 표면 또는 (ii) Twasm/A-모드 게스트의 wasm 선형메모리(호스트 런타임 wasmtime에 위임)를 통해서만 구성될 수 있다 — 노출은 임베더 구성 종속. GLitch는 GPU/WebGL 경유 공격인데 otter/hosts에 GPU 표면이 전무하므로 이 항목만은 사실상 not-exposed [실측: 저장소에 GPU/WebGL 코드 없음]. Rowhammer 계열의 근본 방어는 하드웨어(TRR/ECC/refresh) 층위이며 어떤 런타임도 소프트웨어만으로 완전 방어를 주장할 수 없다 — 게이트의 목적은 완화 주장 아닌 표면 추적.
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) crates/ + hosts/ 전체에서 게스트-제어 대용량 원시 메모리 접근 프리미티브(벌크 배열 R/W host fn, mmap 노출 등)와 GPU/DMA 표면 신규 추가 여부 스윕; (ii) wasmtime 핀 범프 시 업스트림 보안 권고(선형메모리 관련) 확인; (iii) 임베더 문서에 '비신뢰 바이트코드 + 대용량 메모리 host fn 조합 = 해머링 프리미티브' 경고 유지.

#### (b) Spectre/천이실행 — v1/1.1/1.2, Spectre-NG, SpectrePrime, SpectreRSB/ret2spec, SpectreRewind, SMoTherSpectre

- **노출 판정**: `embedder-dependent`
- **근거**: T1/T2는 checker를 통과한 (잠재적) 비신뢰 바이트코드를 임베더와 동일 주소공간의 네이티브 코드로 방출하므로, in-process Spectre 잔여 위험은 모든 in-process JIT와 동일하게 원리적으로 존재하며 otter는 스펙터 완화 코드(fence/masking)를 방출하지 않는다 [실측: otter-baseline/otter-cranelift에 spectre 완화 부재]. 신뢰 모델은 명문화됨: Isa(Expr 트리·host fn)는 신뢰, 바이트코드(FuncDef/Module)는 비신뢰로 checker가 검증 [실측: /home/ybi/otter/crates/otter/src/lib.rs:1589-1628]. 구체 표면 실측 2건: ① CallMode::Table(전모듈 AOT 컴파일 경로 compile_module)은 런타임 callee 워드로 `table[callee*8]` fnptr 로드를 emitter 내 경계검사 없이 수행 [실측: /home/ybi/otter/crates/otter-cranelift/src/lib.rs:1046-1051] — v1-형상 가젯 후보이자 잠재적 아키텍처 OOB 표면([추정] 상위 검증 계층의 존재 여부는 미확인, 별도 감사 필요 — 평가-시점 표기; 처분 1의 지정 감사에서 '부재 확정'으로 해소, otter/src/lib.rs:1616-1622); ② 프로덕션 경로 CallMode::Dispatch는 Rust 디스패처의 callee_index(A-C1 full-width 변환) 경계검사를 경유 — 순차 실행은 안전하나 '경계검사 후 사용'은 고전적 v1 형상. 완화 요소: 코어에 타이머 없음(관측 채널을 otter가 제공하지 않음), 단일 스레드·스폰 표면 전무(SMoTherSpectre 포트경합·SpectreRewind의 동시 측정 주체를 otter 안에서 구성 불가), 로컬/프레임 접근은 전부 정적 오프셋. Twasm/A-모드는 wasmtime/Cranelift 기본 스펙터 완화에 위임 [추정: 참조 호스트 Config는 wasm_exceptions+shared_memory만 설정하고 완화 설정을 건드리지 않으므로(main.rs:159-172 실측) 48 기본값 ON 가정]. O-8c tail-call은 컴파일-티어 ret 흐름을 조작하므로 RSB/ret2spec 관점의 지정 감사 대상.
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) 신규 emitter 패턴에서 게스트값-종속 주소지정 및 '경계검사-후-로드' 형상 감사(특히 CallMode::Table과 O-8c tail-call 트램폴린/ret 경로); (ii) 본 1차 평가의 코드-레벨 액션 아이템 — Table 모드 callee 무경계 table 로드(otter-cranelift/src/lib.rs:1046-1051) — 의 감사/해소 상태 추적; (iii) wasmtime/cranelift 핀 범프 시 스펙터 완화 기본값 유지 확인; (iv) 임베더 가이드(비신뢰 바이트코드는 프로세스 격리 권고) 유지; (v) 신형 천이실행 변종 공표 시 목록 append; (vi) B10 수용 전제(비신뢰 바이트코드를 CallMode::Table에 절대 투입하지 않음)가 깨지는 순간 — 그런 임베딩 요구의 등장 포함 — 본 클래스에서 즉시 재심.

#### (c) 캐시/타이밍 — Prime+Probe, SAB-as-clock(Fantastic Timers)

- **노출 판정**: `embedder-dependent`
- **근거**: SAB-as-clock: 공유메모리 카운터를 증가시킬 제2 실행 주체가 필수인데, otter L0 공유메모리는 '링크 호환일 뿐 실행 모델 불변'(단일 에이전트 규범)이고 스레드 스폰 표면이 전 저장소에 존재하지 않는다 [실측: o17 §6(정본 /home/ybi/neosurf/.claude-research-library/otter-o17-shared-memory-l0.md), 참조 호스트 main.rs:171 'this host merely LINKS shared memories, it never spawns']. 따라서 스톡 구성에서 SAB-급 시계는 구성 불가 — 단, 임베더가 동시성을 도입하는 순간(o17 §6/§8의 L2 경계) 안전 논증 전체가 재심사 대상이라는 조건부다. Prime+Probe: 고해상도 타이머가 전제 — otter 코어/워크스페이스 크레이트에는 시간 API가 전무하나 [실측: grep], 참조 호스트는 WASI p1 전체 표면을 동기 배선하므로 게스트가 clock_time_get에 접근 가능하고 [실측: main.rs:181 p1::add_to_linker_sync], p3-bridge 게이트 빌드는 wasi:clocks/monotonic-clock@0.3.0을 추가 배선한다 [실측: main.rs:606]. 즉 타이머 표면은 otter 자체가 아니라 호스트/임베더의 WASI 구성 선택이다(참조 호스트는 e2e 데모 심이지 경화된 프로덕션 호스트가 아님). 실효 해상도는 OS 종속 [추정].
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) 스레드-스폰/동시성 표면 부재 grep 게이트(crates/ + hosts/ 전체) — 깨지는 순간 o17 §6 규범대로 타이밍-채널 전면 재평가 발동; (ii) 배포되는 모든 호스트의 클럭 표면(WASI p1 clocks, p3 monotonic-clock) 목록화 및 릴리즈 노트 기재; (iii) Config::shared_memory(true) + 단일-에이전트 규범의 결합 상태(둘이 함께 유지되는지) 확인.

#### (d1) wasmtime `threads` crate feature — tier-2 표면 (otter-o17 §5 등재)

- **노출 판정**: `exposed-mitigated`
- **근거**: 참조 호스트에 실제 컴파일되는 활성 표면 [실측: /home/ybi/otter/hosts/otter-host-wasmtime/Cargo.toml:56 features=["threads"] + src/main.rs:172 Config::shared_memory(true)]. 필요 사유가 실측 기록됨: threads feature 없이는 WasmFeatures::THREADS가 강제 비활성이라 비공유 +atomics 게스트(§7 레시피)조차 로드 실패(Cargo.toml:48-53 주석). 업스트림은 threads를 tier-2로 분류('does not have the same guarantees', 해당 영역 CVE 미발급) — o17 §5가 릴리즈 보안 게이트 등재를 지시한 바로 그 근거. 완화: 단일-에이전트 규범(스폰 없음, o17 §6), 동기 Store 규범(async_support 미호출), 락파일 핀 wasmtime 48.0.1 [실측: hosts/otter-host-wasmtime/Cargo.lock]. Extern::SharedMemory 데이터 읽기의 UnsafeCell 복사 안전 논증도 단일-스레드 규범에 의존(o17 §5-3/§6) — 규범이 깨지면 이 항목도 재심사.
- **릴리즈별 반복 의무**: feature가 켜져 있는 한 등재 유지(append-only). 매 testing/stable 릴리즈: (i) wasmtime threads/shared-memory 영역 업스트림 이슈·권고 직접 점검(tier-2는 CVE로 발급되지 않을 수 있으므로 GitHub 이슈 트래커 확인 필수); (ii) 업스트림 tier 지위 변동 확인 — 변동 시 본 항목 재평가; (iii) 단일-에이전트 규범 불변 확인(위 (c)-(i) 게이트와 공유); (iv) 호스트 트레인 핀 범프 시 o17 §9 게이트(wasm-e2e-atomics) 재실행.

#### (d2) wasmtime-wasi `p3` 모듈 — semver-면제·보안패치-비보장 (otter-o16f §6 등재)

- **노출 판정**: `exposed-mitigated`
- **근거**: p3 모듈 자기선언 [실측: o16f §6 축자, 48.0.1 p3/mod.rs 인용]: 'Experimental, unstable and incomplete… not compliant with semver… Bug and security fixes limited to wasip3 will not be given patch releases'. 완화가 저장소에서 실측 확인됨: p3-bridge cargo feature는 기본 OFF이고 스톡 빌드 그래프는 pre-O-16f 호스트와 byte-identical(feature 격리), =48.0.1 락 핀 + --locked 빌드가 노출을 의도적 핀 범프로 한정 [실측: /home/ybi/otter/hosts/otter-host-wasmtime/Cargo.toml:63-87 RELEASE-GATE NOTE + Cargo.lock wasmtime-wasi 48.0.1]. 스톡 배포만 보면 not-exposed이나, feature가 저장소에 존재하고 게이트 빌드가 배포될 수 있는 한 o16f §6의 의무가 구속되므로 exposed-mitigated로 등재. feature ON 시 wasmtime/component-model-async + component-model-bytes가 빌드 그래프에 유입되는 점도 표면 확대 요소(Cargo.toml:76-77).
- **릴리즈별 반복 의무**: o16f §6 명시 의무 그대로, feature가 존재하는 한: 매 testing/stable 릴리즈 (i) p3 표면 업스트림 이슈 점검(보안패치 비보장이므로 패치 릴리즈를 기다리지 말고 이슈 자체를 추적); (ii) =48.0.1 락 핀 유지 — 범프 시 S2 경계 실측(E5, sync-Store latch 프로브 HOST-P3-STORE-SYNC-OK 포함) 재실행 의무; (iii) p3-bridge가 컴파일된 배포물의 존재 여부를 릴리즈 노트에 명시.

### 평가 기록 노트

- 문서 동기화 갭: 과제가 지정한 /home/ybi/otter/.claude-research-library/otter-o17-shared-memory-l0.md는 otter 저장소에 존재하지 않음. 정본은 /home/ybi/neosurf/.claude-research-library/otter-o17-shared-memory-l0.md — 독립 저장소 추출(A1) 시 누락으로 보이며, (d1) 게이트의 근거 문서이므로 태그 전 복사 권고. (처분 2로 해소 완료 — 누락 정본 6건 동기화.)
- 저장소 git 상태: main 브랜치에 커밋이 아직 없음(git log 실패). v1.0.0-testing.1은 /home/ybi/otter/Cargo.toml 워크스페이스 버전으로만 존재하며 본 평가는 워킹트리 기준(기존 태스크 #25 'A1: 초기 커밋 + 태그'와 일치). 본 평가 문서가 태스크 #26의 커밋 대상. (평가-시점 기록 — 이후 초기 커밋 91f4006 + testing 브랜치 생성으로 해소.)
- T1 W^X 실측: /home/ybi/otter/crates/otter-baseline/src/lib.rs:492-534 — mmap RW → 코드 복사 → mprotect RX 플립, W+X 동시 매핑 구간 없음(RAII 가드가 mprotect 실패 시에도 매핑 누수 방지).
- 코어 격리 실측: crates/otter는 no_std + #![forbid(unsafe_code)] (lib.rs:27-29); 워크스페이스 lint unsafe_code=warn으로 unsafe를 백엔드 플랫폼 심에 집중.
- Twasm(J-모드) 입력 방어 실측: 참조 호스트의 §5.2 섹션 화이트리스트(main.rs emitted_sections_whitelisted, 약 484-560행) — type(1)/import(2)/function(3)/export(7)/code(10)+custom(0)만 허용, start/element/data 금지로 otter_jit.compile 입력의 인스턴스화 창을 봉인. 이 방어의 경계도 문서에 정직 기록됨(인스턴스화 창만 봉인).
- 신뢰 모델 명문화 실측: Isa(Expr 트리·host fn)=신뢰, 바이트코드(FuncDef/Module)=비신뢰이며 checker가 비신뢰 부분을 검증(crates/otter/src/lib.rs 1589-1628). constant_fold는 비신뢰 prog에 대해 현재 test-only 도달이라는 자체 경고 주석 존재(lib.rs:1344-1349).
- [추정] 표기 항목 정리: ① wasmtime 48의 Cranelift 스펙터 완화 기본 ON 여부(호스트 Config가 미변경이므로 업스트림 기본값 가정), ② CallMode::Table 경로 상위에 callee 경계를 보장하는 검증 계층의 존재 여부(미발견 — 감사 필요 → 처분 1에서 '부재 확정'으로 해소, 평가-시점 표기 존치), ③ WASI clock_time_get의 실효 해상도(OS 종속).
- 게이트 목록은 부동(append-only): 본 1차 평가가 베이스라인. 신규 공격 클래스 공표 시 목록에 추가만 하고 삭제하지 않으며, 각 항목의 per_release_duty는 매 testing/stable 릴리즈에서 반복 수행된다.
- 호스트 트레인 이원화 주의: 워크스페이스 하니스 트레인(wasmtime 45 ↔ cranelift 0.132 lockstep)과 호스트 트레인(wasmtime 48.0.1)이 설계상 분리(hosts/otter-host-wasmtime/Cargo.toml:24-28) — 보안 권고 점검 시 두 트레인 모두 확인해야 함.

---

---

## 확장 평가 부기: v1.0.0-testing.1 (2026-09-04 — 오너 목록 편입분 평가)

**판정**: 확장 14클래스에서도 `exposed-open` 없음 — 1차 평가와 정합. 유일한 `exposed-mitigated`는 A6(스택 오버플로: check_module 통과 입력으로도 발동 가능하나 귀결이 fail-stop crash로 한정), `not-exposed` 4건(A5·B3·B4·B5), 나머지는 전부 임베더-구성-종속. 3-분석가 워크플로(방출기 전문 통독 + 명령 패턴 실측) 산출.

### (b) 세부 확장 — 천이실행

#### A1 — Spectre v2 / BTI (Branch Target Injection, CVE-2017-5715)

- **노출 판정**: `embedder-dependent`
- **근거**: 실측: 세 컴파일 티어 모두 방출 코드에 진성 간접 분기를 낸다. T1은 모든 호출을 register-indirect call rax로 방출 — emit_aligned_call의 0xFF 0xD0(otter-baseline/src/lib.rs:86)와 emit_aligned_call_rax의 0xFF 0xD0(:713). HostFn1/2·HostCtx1/2도 예외 없이 이 경로(:114-124,:140-155; 단 타깃은 신뢰 Isa host fn 주소를 movabs 즉치로 구운 상수라 데이터-제어 불가 = 약한 가젯). T2는 헬퍼/host/모듈호출 전부 call_indirect(otter-cranelift/src/lib.rs:55,83,95,111,130,1012,1052,1058). tail_signal 트램폴린의 함수포인터 호출은 엔진측 transmute(p) 후 f(...) 간접 호출(otter-engine/src/lib.rs:798-804), p는 add-only 등록 코드포인터(비게스트-제어). 강한 가젯은 B10: CallMode::Table의 게스트-인덱스 table 로드 뒤 간접 호출 — T1 mov rax,[rdi+rcx*8](:907) 후 call rax(:926), T2 imul_imm(callee,8);iadd;load(trusted)(:1046-1051) 후 call_indirect(:1052). otter는 retpoline/lfence/IBRS류 완화를 일절 방출하지 않음(전 크레이트 grep: spectre/lfence/retpoline 부재; new_module은 cranelift 플래그 무설정 기본값 :134-138, 따라서 cranelift의 spectre 완화 setting 기본 ON은 raw load에는 적용되지 않아 사실상 inert). 판정 근거: 간접 분기 자체는 otter 내 실존(확정)하나 (i) 관측 채널(고해상도 타이머)을 otter 코어가 미제공, (ii) 비신뢰 바이트코드→JIT 투입이 임베더-구성, (iii) 실 방어층(eIBRS/retpoline/IBPB)은 호스트 커널/마이크로코드 소관 — 공격 전제 집합이 임베더/호스트 조립. 단 소프트웨어 완화층이 otter에 부재(위임 아닌 부재)라는 점 명기: 전제가 조립되는 순간 otter 내에서 이를 막는 것이 없음. B10 전제(비신뢰 바이트코드를 Table 모드에 절대 투입 금지)가 깨지면 즉시 재심.
- **릴리즈별 반복 의무**: 매 testing/stable: (i) 신규 emitter 패턴에서 게스트-데이터-제어 간접 분기(특히 B10 Table 로드 계열)와 상수-타깃 간접 호출을 구분 감사; (ii) cranelift/wasmtime 핀 범프 시 간접분기 완화 기본값 변동 확인 및 otter가 여전히 무플래그 기본값을 소비하는지 확인; (iii) 임베더 가이드에 '비신뢰 바이트코드는 프로세스 격리 + 호스트 커널 eIBRS/retpoline 활성 전제' 경고 유지; (iv) aarch64 T1(cranelift, 중기)이 실배포되면 use_bti 기본 false(arm64.rs:52-56) 재평가.
- **감사/완화 후보**:
  - [중] B10 Table-모드 간접 호출 경로를 v2 가젯 관점에서 지정 감사 — 게스트-인덱스 table 로드+call이 존재하므로, 비신뢰 입력 투입 요구가 등장하는 순간 retpoline/thunk 방출 또는 경로 봉인 중 택일을 사전 설계
  - [하] T1 host/헬퍼 호출을 register-indirect call rax에서 call rel32(직접)로 전환 가능한지 검토 — 상수 타깃이므로 불필요한 BTB 엔트리를 줄일 수 있음(방어심화, 성능중립)

#### A2 — Spectre v4 / SSB (Speculative Store Bypass, CVE-2018-3639)

- **노출 판정**: `embedder-dependent`
- **근거**: 실측: store-then-load 프레임 재사용 패턴이 두 티어에 편재한다. T1은 프롤로그에서 ABI 레지스터를 프레임에 spill(mov [rbp+disp],rdi/rsi/rdx otter-baseline/src/lib.rs:762-767)한 뒤 파라미터 복사 시 즉시 재로드(:775-785), locals를 fill로 일괄 초기화(:770-772)한 뒤 본문에서 재로드, Call/TailCall 인자를 argbuf에 store(:889-890,:843-844)한 뒤 callee가 같은 슬롯을 read. T2도 stack_store(:992,:1037)로 argbuf에 쓴 뒤 callee가 읽음. rax 스크래치+push/pop 표현식 스택(setup_int :56-59, push rax :242) 또한 store-then-load. LFENCE/SSBD류 배리어는 방출되지 않음(grep 부재). 판정 근거: SSB는 선행 store를 우회해 stale 값을 투기적으로 로드하는 것으로, otter 프레임 재사용에서 이전 호스트 스택 프레임의 잔존 비밀이 그 슬롯에 남아 있으면 교차-도메인 누출 프리미티브가 될 수 있으나 (i) 고해상도 타이머 부재(otter 코어), (ii) SSBD는 MSR/prctl OS-레벨 제어(호스트 소관), (iii) 비신뢰 투입은 임베더 선택 — 전제 전부 임베더/호스트. 순차 실행 자체는 안전(프레임 접근 전부 정적 오프셋 disp(i), :51-53).
- **릴리즈별 반복 의무**: 매 testing/stable: (i) 신규 emitter에서 store-then-load 프레임/스크래치 재사용 형상 감사(특히 argbuf 마샬링과 spill 슬롯 재사용); (ii) 호스트 배포물의 SSBD 정책(prctl/커널 기본값) 릴리즈 노트 기재 권고; (iii) 임베더 가이드에 '민감 비밀을 호스트 스택에 둔 채 비신뢰 바이트코드 JIT 실행 금지' 경고 유지.
- **감사/완화 후보**:
  - [하] T1/T2 프레임 슬롯 재사용이 이전 호스트 스택 프레임 잔존값을 노출하는 범위 조사 — 컴파일 프레임 진입 시 초기화되지 않는 슬롯(argbuf 등)의 잔존 데이터 표면 확인

#### A3 — Spectre-BHB (Branch History Injection, CVE-2022-23960)

- **노출 판정**: `embedder-dependent`
- **근거**: 실측: 비신뢰 바이트코드의 조건 분기(T1 test rax;jne rel32 :456-459,:814-818; T2 brif :550,:960)와 A1의 간접 분기가 전부 프로세스 공유 분기이력(BHB/BTB)에 기여한다 — in-process JIT이므로 게스트 분기가 호스트와 동일 예측기 상태를 공유. otter는 BHB 정화 시퀀스(aarch64 CSDB/clearing, x86 BHI 완화)를 방출하지 않음(cranelift use_csdb 기본 false arm64.rs:57-60; otter 무플래그). 판정 근거: BHI는 A1/A2 익스플로잇 체인의 이력-오염 벡터로, in-process에서 원리적으로 유효하나 실 완화(eIBRS 보강, BHI_DIS_S, aarch64 clearing)는 호스트 커널/마이크로코드 소관이고 관측 타이머·비신뢰 투입은 임베더 구성. otter 자체는 오염 방지 코드를 내지 않으므로 전제 조립 시 in-otter 차단막 없음(A1과 동일 구조). 브라우저가 아니라 임베더블 라이브러리 관점: 제2 실행 주체 없이 단일 스레드이므로 교차-프로세스 BHI 훈련은 임베더가 동시성/외부 프로세스를 도입할 때만 성립.
- **릴리즈별 반복 의무**: 매 testing/stable: (i) A1 의무와 공유 — 간접/조건 분기 감사 시 BHB 오염 관점 병기; (ii) 호스트 커널 BHI 완화(BHI_DIS_S/eIBRS) 활성 전제를 임베더 문서에 유지; (iii) aarch64 티어 실배포 시 use_csdb/clearing 시퀀스 방출 여부 재평가.
- **감사/완화 후보**:
  - [하] aarch64 T1(cranelift) 실배포 시점에 use_bti+use_csdb 동시 활성 플래그 세트를 방출 설정으로 채택할지 사전 결정 — 현재 무플래그 기본값은 둘 다 off

#### A4 — SLS (Straight-Line Speculation)

- **노출 판정**: `embedder-dependent`
- **근거**: 실측: T1 하드-emitter는 ret/무조건 jmp 뒤에 INT3(0xCC)/트랩 패딩을 넣지 않는다 — 에필로그가 leave;ret(0xC9 0xC3)로 끝나고 그 뒤 아무것도 없으며(otter-baseline/src/lib.rs:464,:824,:876), JumpTo는 jmp rel32(0xE9)로 끝나고 뒤에 트랩 없음(:449,:807). 전 파일 통독으로 확인. cranelift는 aarch64에서 SLS 완화 옵션을 갖지만 otter는 플래그 무설정(cranelift 기본값)이라 방출 안 됨. 판정 근거: SLS는 주로 AMD/aarch64 관심사이며 익스플로잇에는 타이머+가젯이 필요(임베더/호스트 전제)하므로 노출은 임베더-구성 종속. 그러나 x86-64 T1 하드-emitter에 한해 ret/무조건 jmp 뒤 INT3 삽입은 otter가 완전히 자체 소유하는 저렴한 완화 후보 — A1/A2/A3(하드웨어/OS층)과 달리 otter-ownable. 이 점에서 감사 후보 우선순위를 medium으로 격상.
- **릴리즈별 반복 의무**: 매 testing/stable: (i) T1 신규 에필로그/무조건분기 방출 시 SLS 트랩-패딩 정책 준수 여부 감사; (ii) cranelift 핀 범프 시 aarch64 SLS 완화 기본값 확인; (iii) 완화 후보(아래) 처분 상태 추적.
- **감사/완화 후보**:
  - [중] T1 방출기에 SLS 하드닝 추가 검토 — 모든 ret(leave;ret) 및 무조건 jmp rel32 뒤에 INT3(0xCC) 삽입. otter 자체 소유의 저렴한 self-contained 완화이며 W^X 버퍼 크기만 소폭 증가. 게이트 등재와 별개로 실코드 작업 후보

#### A5 — speculative type confusion

- **노출 판정**: `not-exposed`
- **근거**: 실측: otter의 어떤 티어도 타입 피드백/인라인 캐시/hidden class/투기적 타입 가드를 쓰지 않는다(전 크레이트 grep: inline_cache/type_feedback/hidden_class 부재; 'shape'는 코드-형상 주석뿐). Value=i64 무타입 균일 레지스터 IR이며(otter/src/lib.rs Value=i64), 수치 타입 NumTy는 신뢰 Isa(OpDef)에 정적으로 박혀 있어 런타임 투기 대상이 아니다. Effect::Guard는 투기적 타입 판별이 아니라 deopt 정합 메커니즘 — 최적화 티어가 조건(예 i<limit)을 가정하고 실패 시 인터프리터로 bail(otter/src/lib.rs:492 'speculation marker: the optimizing tier may assume'; cranelift lower_expr Guard→brif→deopt block :1241-1261; baseline·wasmemit는 Guard를 no-op :467,:826, otter-wasmemit:1158). 판정 근거: 투기적으로 혼동될 '타입 시스템'이 런타임에 존재하지 않으므로 speculative type confusion은 not-exposed. 단, Guard의 brif는 '경계검사-후-사용' v1 형상의 조건 분기이므로, 그 투기 잔여 위험은 A1/기존 (b)클래스 v1 라인의 의무로 이미 포섭됨(타입 혼동이 아니라 조건 오예측). 교차 참조로만 기록.
- **릴리즈별 반복 의무**: 매 testing/stable: 향후 티어에 타입 피드백/인라인 캐시/speculative deoptimization-on-type이 도입되면 즉시 본 클래스를 not-exposed에서 재평가(그 순간 speculative type confusion 표면이 최초로 생김). 현 무타입 IR 불변 확인.
- **감사/완화 후보**:
  - [하] 향후 최적화 작업(인라인 캐시/모노모픽화/타입 특화)이 제안되면 speculative type confusion 표면 신설로 간주하고 본 게이트 항목을 재평가하도록 설계 리뷰 체크리스트에 편입

#### A6 — speculative stack smashing + stack overflow

- **노출 판정**: `exposed-mitigated`
- **근거**: 실측(비투기 실체 우선): (1) 대형 프레임 guard-page 스킵 — T1 프롤로그는 push rbp;mov rbp,rsp;sub rsp,frame(otter-baseline/src/lib.rs:757-760)로 스택 프로브가 없고, frame=((total_slots*8).div_ceil(16)*16)(:752). check_func의 상한 MAX_SAFE_NLOCALS=(i32::MAX)/32 ≈ 67.1M(otter/src/lib.rs:1747, 검사 :1866-1869)는 i32 오버플로 방지용이지 guard-page 스킵 방지용이 아님 — nlocals≈67M이면 frame≈537MB의 단일 sub rsp가 guard page를 건너뛴다. 다만 직후 locals 초기화 루프가 [rbp-8]부터 하향으로 전 슬롯을 순차 store(:770-772)하므로 guard page에서 먼저 SIGSEGV로 fail-stop(무성 손상 아님). **정정(H-패키지 착지 시)**: 모듈 경로는 sub 직후 첫 터치가 ABI 스필(rdi/rsi/rdx — 프레임 최심부, locals보다 깊음)이라 frame>4096에서 무성-손상 창이 존재했음 — H1b sub-후-터치 프로브(T1) + H1c inline probestack(T2)이 봉인(구조 단정: 첫 메모리-스토어가 프로브임). 아울러 엔진 2중 가드(바이트-예산 1MiB + depth 4096, H1a)로 통상 재귀는 abort 대신 회복-가능 Trap이 됨. (2) 깊은 비-tail 재귀 — 일반 Call은 module_dispatch→engine.dispatch→call_compiled→callee 재진입으로 네이티브(Rust+JIT) 스택을 키운다(otter-engine dispatch의 인터프리터 경로는 run_one 콜백으로 self.dispatch 재귀 :527-535). 트램폴린 루프는 TAIL 재귀만 정상 상수-스택화(:484-543)하고 일반 재귀는 네이티브-스택 바운드 → 오버플로 시 SIGSEGV/abort(catch_unwind 불가). 이는 문서화된 기지 갭(otter-baseline:1334 'infinite recursion / native stack overflow — NOT a panic catch_unwind can contain'). (3) T2 probestack 기본 false(cranelift-codegen-meta settings), otter 무플래그(:134-138)라 T2도 스택 프로브 없음. 판정 근거: 최악은 가용성(DoS)·fail-stop(프로세스 crash)으로 메모리 손상이 아니며, MAX_SAFE_NLOCALS 상한 + guard page + 순차 초기화가 blast radius를 crash로 한정 → exposed-mitigated. 그러나 재귀 깊이 무제한 + 양 티어 프로브 부재가 open edge라 감사 후보 상(高). 투기적 변종(speculative stack smashing = 스매싱된 복귀주소의 투기 읽기→transient 제어흐름)은 타이머+가젯 필요로 임베더-구성 종속 — 여기서는 전이실행 체인(A1/A4)에 병합. 비신뢰 바이트코드가 THE 입력인 otter 모델에서 (1)(2)는 check_module 통과 입력으로도 발동 가능함을 명기(checker는 종료성/재귀깊이를 증명하지 않음, otter/src/lib.rs:1616-1622 및 non-goals).
- **릴리즈별 반복 의무**: 매 testing/stable: (i) T1/T2 프롤로그 스택 프로브 부재 및 frame 크기 산식 감사(신규 슬롯 종류 추가 시 total_slots 재검토); (ii) 비-tail 재귀 깊이 제한/네이티브 스택 한계 도입 여부 추적; (iii) MAX_SAFE_NLOCALS 상한이 guard-page 스킵을 fail-stop으로 유지하는지(순차 초기화 불변) 확인; (iv) cranelift 핀 범프 시 probestack 기본값 변동 확인.
- **감사/완화 후보**:
  - [상] 비-tail 재귀 깊이 가드 도입 검토 — engine.dispatch의 일반 Call 경로에 재귀 깊이 카운터 또는 스택 한계 체크를 추가해 검증-통과 바이트코드가 네이티브 스택 오버플로(abort/DoS)를 유발하지 못하게 함. 문서화된 기지 갭의 실코드 완화 — **채택 완료(H-패키지 H1a)**: 2중 가드(바이트-예산 주가드 + 깊이 백스톱), 회복-가능 Trap(TrapReason)
  - [상] T1/T2 스택 프로브 부재 지정 감사 — MAX_SAFE_NLOCALS(≈67M) 상한을 현실적 값으로 낮추거나 T1 프롤로그에 stack-clash 방지 프로브(페이지별 터치) 추가, T2는 enable_probestack=true 플래그 채택 여부 결정 — **채택 완료(H-패키지 H1b/H1c)**: T1 sub-후-터치 프로브(frame>4096) + T2 `enable_probestack=true`+`probestack_strategy=inline` 원자쌍
  - [중] 대형 frame의 순차 초기화 fail-stop 불변을 회귀 테스트로 고정 — 초기화 순서나 슬롯 레이아웃 변경이 guard-page 스킵을 무성 손상으로 바꾸지 않는지 차등 검증

#### A7 — MDS 4종 (MSBDS/Fallout CVE-2018-12126, MFBDS/ZombieLoad CVE-2018-12130, MLPDS/RIDL CVE-2018-12127, MDSUM CVE-2019-11091)

- **노출 판정**: `embedder-dependent`
- **근거**: 실측: MDS는 CPU 내부 버퍼(store buffer, fill buffer, load port, uncacheable)에서의 마이크로아키텍처 샘플링으로, 완화는 전적으로 마이크로코드(문맥전환 시 VERW)+OS+SMT 비활성화 층위다 — otter는 임베더블 라이브러리로서 이 층에 방출할 코드가 없고 완화할 수도 없다. 교차-도메인 MDS 누출의 최강 전제는 SMT 형제 스레드 동거인데, otter는 단일-에이전트 규범으로 스레드를 스폰하지 않는다(전 저장소 스레드-스폰 표면 부재, 기존 게이트 (c)/(d1) 실측과 동형; otter-engine dispatch는 단일 &self 재진입 루프이지 병렬 아님 :475-543). 관측 타이머도 otter 코어 미제공. 판정 근거: 동일-스레드 MDS(VERW-gap 경유 in-process JIT↔host 경계 누출)는 원리적으로 존재하나 타이머 필요, 교차-도메인 강누출은 SMT 형제(제2 실행 주체) 필요 — SMT 구성·마이크로코드·타이머 전부 호스트/임베더 소관. 즉 otter는 스톡 구성에서 제2 측정 주체를 구성 불가하고(SMoTherSpectre와 동일 논증), 노출은 임베더가 동시성/SMT/외부 프로세스를 도입할 때만 성립 → embedder-dependent. Rowhammer류처럼 소프트웨어만으로 완전 방어를 주장할 수 없는 하드웨어층 클래스이며 게이트 목적은 표면 추적.
- **릴리즈별 반복 의무**: 매 testing/stable: (i) 스레드-스폰/동시성 표면 부재 grep 게이트(crates/+hosts/) — 깨지는 순간 SMT 형제 전제가 otter 내 구성 가능해지므로 본 클래스 전면 재평가; (ii) 배포 호스트의 SMT 정책·마이크로코드(MD_CLEAR/VERW) 활성 전제를 릴리즈 노트/임베더 문서에 기재; (iii) 단일-에이전트 규범과 타이머-부재 불변이 함께 유지되는지 확인((c) 클래스와 공유).
- **감사/완화 후보**:
  - [하] 임베더 가이드에 'MDS 방어는 호스트 마이크로코드(MD_CLEAR/VERW)+SMT 정책 소관이며, 비신뢰 바이트코드를 SMT 형제와 동거시키지 말 것' 명시 항목 유지 — 코드 작업 아닌 문서 의무이나 게이트 추적 대상

### (f) JIT 코드-방출 위생

#### B1. CFI 결여 (x86 CET IBT / 전방 CFI 부재)

- **노출 판정**: `embedder-dependent`
- **근거**: 실측 ①: T1 방출 함수의 프롤로그는 push rbp(0x55); mov rbp,rsp; sub rsp,frame으로 시작 — endbr64(F3 0F 1E FA) 랜딩패드 부재 [실측(평가-시점, 2026-09-04): /home/ybi/otter/crates/otter-baseline/src/lib.rs:415-421(단일함수 compile), 756-760(compile_module_func); 파일 전체 grep에 0x1E 바이트 자체가 0건 → **H-패키지(H2)에서 두 T1 프롤로그 선두에 endbr64(F3 0F 1E FA) 선방출 채택** — 단정 테스트 `endbr64_at_head_of_every_emitted_function`(양 진입점 × 양 CallMode 코퍼스 선두 4바이트 단정)이 게이트]. ②: T2 cranelift-codegen 0.132.3 x64 백엔드는 endbr 방출 코드가 전무하고 branch protection은 aarch64 BTI(use_bti)만 존재, x64는 무조건 BranchProtection::None [실측: ~/.cargo/registry/src/.../cranelift-jit-0.132.3/src/backend.rs:341-346; cranelift-codegen-meta-0.132.3/src/isa/arm64.rs:52; x64 트리 endbr grep 0건]. otter는 JITBuilder 기본 설정을 그대로 사용 [실측: /home/ybi/otter/crates/otter-cranelift/src/lib.rs:134-138]. ③: fn-pointer 호출 표면 — 모든 JIT 진입이 간접 호출이다: Rust→JIT transmute-fnptr [실측: otter-baseline/src/lib.rs:567,611; otter-engine/src/lib.rs:795-805 call_compiled; otter-cranelift/src/lib.rs:622,666,1126], JIT→JIT `call rax`(FF D0, Table 모드 table[callee] 로드 포함) [실측: otter-baseline/src/lib.rs:86,713,905-907; otter-cranelift/src/lib.rs:1046-1052], JIT→HostFn/dispatch/tail_signal도 전부 immediate 주소의 간접 호출 [실측: otter-baseline/src/lib.rs:114-155; otter-cranelift/src/lib.rs:77-132]. 워크스페이스 빌드에 cf-protection 플래그 부재 [실측: rust-toolchain.toml·justfile·Cargo.toml grep 0건] — 임베더가 자기 바이너리를 CET 빌드해도 otter JIT 페이지는 무방비. 귀결: CET-IBT 강제 환경에서는 최초 JIT 티어 호출의 간접 호출이 endbr64 없는 진입점에 착지 → #CP 폴트로 결정적·가시적 실패(침묵 우회가 아닌 가용성 실패; T0 인터프리터는 무관). IBT 비강제 환경에서는 방출 코드 전체가 전방-CFI 부재 코드 공간이 되나 악용에는 별도 제어흐름 하이재킹 프리미티브가 선행돼야 한다 — 공격 전제(하이재킹 프리미티브, CET 정책, 비신뢰 코드 투입)는 임베더/프로세스 구성 종속. otter가 사실상 임베딩 프로세스의 IBT 채택을 막는(비호환) 구조라는 점이 실질 귀결.
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) T1 방출 바이트 테이블과 cranelift 핀(현 0.132.3)에서 endbr64/x64 IBT 지원 상태 재확인 — 업스트림이 x64 IBT를 추가하면 즉시 채택 검토; (ii) 신규 간접 호출·간접 점프 방출 패턴 추가 여부 스윕(현재 간접 전이는 call rax 2개소 + cranelift call_indirect뿐); (iii) 임베더 문서에 'CET-IBT 강제 환경 비호환(첫 JIT 호출에서 결정적 #CP 실패, T0 폴백은 수동)' 고지 유지; (iv) 릴리즈 노트에 CET 호환성 상태 명시.
- **감사/완화 후보**:
  - [상] T1 두 프롤로그(otter-baseline/src/lib.rs:415-421, 756-760) 선두에 endbr64(F3 0F 1E FA) 4바이트 선방출 — 기능상 무해(비-CET CPU에서 NOP), 기존 차등 테스트로 동등성 검증. IBT-강제 임베더와의 전방 호환을 T1에 한해 즉시 확보 — **채택 완료(H-패키지 H2)**: 선방출 + 단정 테스트 `endbr64_at_head_of_every_emitted_function`
  - [중] T2 Cranelift 엔트리용 엔진측 endbr64 썽크 테이블(endbr64 + 직접 jmp rel32 → 실제 엔트리; 직접 점프는 IBT 비검사 대상) 도입 또는 cranelift x64 IBT 지원 업스트림 이슈 추적·기여
  - [중] 런타임 CET 상태 감지(/proc/self/status x86_Thread_features 또는 arch_prctl) 후 JIT 티어를 명시적 오류/T0 폴백으로 거부하는 fail-closed 진단 경로 — 현재는 첫 간접 호출의 #CP(SIGSEGV)로 사망
  - [하] HostFn 간접 호출에 대한 시그니처-태그형 전방 CFI(KCFI 유사)는 Isa=신뢰 전제상 과잉 — 위협모델 문서에 '호스트fn 포인터는 신뢰 입력'임을 명시하는 선에서 종결

#### B2. 블라인드 JIT 스프레이 / ASLR 무력화 (상수 블라인딩 부재 + 코드 페이지 배치)

- **노출 판정**: `embedder-dependent`
- **근거**: 실측 ①(상수→immediate 직행): 비신뢰 Module의 f.consts(i64)는 T1에서 movabs rax,imm64로 [실측: /home/ybi/otter/crates/otter-baseline/src/lib.rs:789-793 — 함수당 nlocals-nparams개, 8바이트 전량 공격자 제어], T2에서 iconst(types::I64,*c)로 [실측: /home/ybi/otter/crates/otter-cranelift/src/lib.rs:902-904] 방출 명령의 immediate에 그대로 박힌다. 비신뢰 명령 오퍼랜드(i32)도 OperandImm으로 imm32 직행 [실측: otter-baseline lib.rs:232-235; otter-cranelift lib.rs:305]. 상수 블라인딩·난수 NOP·코드기점 난수화는 두 방출기 어디에도 없음 [실측: 전문 판독]. checker는 인덱스·범위·프레임만 검증하며 상수 값은 검증 대상이 아니므로 [실측: /home/ybi/otter/crates/otter/src/lib.rs:1589-1628], '비신뢰 바이트코드 허용'이라는 명문 신뢰 모델 내부에서 스프레이 프리미티브가 무조건 성립한다(nlocals 상한 MAX_SAFE_NLOCALS≈67.1M → 함수 하나로 수백 MB급 공격자 바이트의 RX 상주 가능). ②(주소 무작위화): T1은 mmap(NULL 힌트) [실측: otter-baseline lib.rs:509-518], T2 cranelift-jit도 memmap2 map_anon → mmap(ptr::null_mut()) [실측: cranelift-jit-0.132.3/src/memory/system.rs:31; memmap2-0.2.3/src/unix.rs:56-57, 락 핀 실측] — 커널 mmap ASLR 기저 엔트로피는 받지만 연속 할당이 인접 배치되어 포인터 1개 누출 시 상대 주소 추정이 용이하고, 할당 단위 재난수화·가드 페이지·페이지 내 난수 오프셋이 없다. ③(RW→RX 전이 창): 양 티어 모두 RW 매핑→기록→RX 단일 플립, W+X 동시 구간 없음 [실측: otter-baseline lib.rs:504-536; cranelift-jit system.rs:96-108]. 창 내 변조는 제2 실행 주체+임의쓰기가 전제 — 단일-에이전트 규범의 스톡 구성에선 구성 불가. 귀결: 스프레이를 실행으로 전환하려면 (i) 비신뢰 바이트코드 투입 경로, (ii) 독립적 제어흐름 하이재킹, (iii) 주소 지식(otter는 게스트에 주소 누출 프리미티브를 제공하지 않음 — Value=i64 레지스터 IR)이 모두 필요하며 전부 임베더 구성/별도 취약점 종속. 단 프리미티브 자체는 otter 산물에 무조건 존재하고, T2 코드 페이지의 leak-by-design(B3)과 결합하면 프로세스 수명 내내 축적된다.
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) 비신뢰 값→immediate 직행 경로 목록(현재 consts·OperandImm·T1 Expr::Imm/ImmF64는 Isa 신뢰측) 갱신 — 신규 방출 패턴 스윕; (ii) 상수 블라인딩/배치 난수화 도입 상태 추적(미도입이면 미도입임을 평가서에 반복 기재); (iii) '비신뢰 바이트코드 = JIT 스프레이 프리미티브, 프로세스 격리 권고' 임베더 문서 유지((b)클래스 의무와 병합 운영); (iv) cranelift 핀 범프 시 JIT 메모리 할당·배치 방식 변화 확인.
- **감사/완화 후보**:
  - [중] per-engine 난수 키 상수 블라인딩 옵션 — T1은 consts 베이킹(lib.rs:789-793)과 OperandImm(232-235), T2는 iconst 3개소(305, 902-904)를 movabs(k^c)+xor 재료화로 후킹; 기존 3-티어 차등 테스트가 동등성 게이트
  - [중] checker에 비신뢰 모듈의 총 consts/코드 크기 모듈-레벨 상한 추가(현재 per-func nlocals 상한만 존재) — 스프레이 볼륨 제한 + 컴파일 자원 DoS 완화 겸용
  - [하] ExecBuf(otter-baseline lib.rs:494-544) 및 커스텀 JITMemoryProvider: 페이지 내 난수 시작 오프셋(난수 NOP 슬라이드) + 할당 간 PROT_NONE 가드 페이지

#### B3. 런타임 코드-메모리 재사용 결함 (UAF-of-code / munmap·RAII)

- **노출 판정**: `not-exposed`
- **근거**: 실측 ①(T1 해제 경로): ExecBuf는 Drop에서 munmap [실측: /home/ybi/otter/crates/otter-baseline/src/lib.rs:539-544]. ModuleEngine 슬롯은 add-only — 티어업은 새 코드 설치만 하고 기존 코드는 엔진 수명 동안 불해제 [실측: /home/ybi/otter/crates/otter-engine/src/lib.rs:25-26(모듈 독트린), 327-334(슬롯 불변 하드 문서), 867·886-887(is_none 가드가 재설치 차단), 903-928(install_*)]; clear_trap도 코드 무효화가 아니라 플래그 리셋뿐 [실측: lib.rs:58, 시험 2355행 'pins to the same backend forever']. compile은 동기·게스트 코드 비개입이라 is_none 검사와 install 사이 재진입 창 없음(단일-에이전트 규범). ②(유일한 산-코드-드롭): 단일함수 Engine의 티어업 교체 self.funcs[i].code=Some(code) [실측: /home/ybi/otter/crates/otter/src/lib.rs:2564-2572] — 옛 Box<dyn Code> 드롭→T1이면 munmap이나, &mut self 동기 경로라 교체 시점에 옛 코드의 in-flight 실행이 구조적으로 없고 BaselineCode는 raw ptr을 노출하지 않음 → UAF 불성립. ③(T2): cranelift-jit 0.132.3은 Drop 시 코드 페이지를 의도적으로 누수('leak memory to guarantee validity of function pointers', mem::forget)하고 해제는 unsafe free_memory 옵트인 [실측: cranelift-jit-0.132.3/src/memory/system.rs:140-155]; otter는 free_memory를 어디서도 호출하지 않음 [실측: otter-cranelift·otter-engine grep 0건] → T2 UAF-of-code는 구조적으로 불가능한 대신 CompiledFn/ResumableCode/CompiledDispatchFn 드롭마다 RX 페이지가 영구 잔존하며, run_resumable/osr_continue는 호출·OSR 이벤트당 새로 컴파일 [실측: otter-cranelift/src/lib.rs:1295-1315; otter/src/lib.rs:2588-2597] → 반복 시 무한 RX 누적(가용성 + B2 스프레이 잔존면 확대). ④: T1 mprotect 실패 시에도 RAII 가드가 매핑 누수 방지 [실측: otter-baseline lib.rs:526-535]. 예시 CVE-2026-64507류는 커널측 재사용 결함으로 otter 표면이 아니며, 클래스로서의 사용자층 코드-페이지 해제·재사용 경로는 실측상 부재 — add-only 설계가 UAF-of-code를 구조적으로 막는다는 주장이 코드에서 확인됨.
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) add-only 불변 스윕 — install_*/슬롯 교체·제거 API 신설, free_memory 호출 도입, ExecBuf 조기 드롭 경로 신설 여부 grep; (ii) 산-코드-드롭 경로 목록(현재: 단일함수 Engine 티어업 교체 1건) 갱신 + 각 경로의 '드롭 시점 in-flight 부재' 논증 재확인; (iii) cranelift 핀 범프 시 JIT 메모리 Drop 의미(누수→해제 전환 여부) 확인 — 전환되면 본 클래스 즉시 재심; (iv) T2 leak-by-design의 누적 특성을 임베더 문서(장수 프로세스에서의 재컴파일·OSR 반복 주의)에 유지.
- **감사/완화 후보**:
  - [상] install_baseline/install_cranelift(otter-engine/src/lib.rs:903-928)에 '기존 슬롯이 None' debug_assert 추가 — 미래 편집이 호출측 is_none 가드를 깨면 침묵 교체(T1이면 munmap-UAF 창)가 되는 것을 즉시 검출로 전환하는 1행 하드닝
  - [중] OSR/resumable 경로(run_resumable·osr_continue·단일함수 Engine OSR arm)의 이벤트당 JITModule 영구 누수 정량화 + 완화(동일 (prog,entry_pc) 컴파일 캐시 또는 이벤트 상한) — 가용성·RX 누적
  - [하] 단일함수 Engine 티어업 교체(otter/src/lib.rs:2571)의 '동기·비재진입 경로' 전제를 doc-불변으로 승격 — 향후 Code::run 중 재진입/코드 포인터 캐싱이 도입되면 UAF 창이 열림을 명시

#### B4. Shadow stack discrepancy (CET SS 하 call/ret 불균형)

- **노출 판정**: `not-exposed`
- **근거**: 실측 ①(T1 복귀 형태): 방출 코드의 복귀는 전부 leave;ret(C9 C3) [실측: /home/ybi/otter/crates/otter-baseline/src/lib.rs:464, 824, 876]; 간접 jmp는 방출되지 않음 — 파일 내 0xFF 오프코드는 call rax(FF D0) 2개소가 전부 [실측: lib.rs:86, 713, grep 전수], jmp는 함수 내부 rel32 직접분기(E9/0F 85)뿐 [실측: lib.rs:449-459, 806-818]. call/ret 완전 균형: TailCall조차 jmp가 아니라 call tail_signal 직후 leave;ret로 자기 네이티브 프레임을 정상 해체하고, otter-engine의 dispatch 트램폴린 루프가 새 call로 재진입한다(설계가 명시적으로 jmp-복귀를 배제) [실측: otter-baseline lib.rs:827-877; otter-engine/src/lib.rs:484-543(루프), 1243-1312(tail_signal)]. rsp 정렬(and rsp,-16 / mov rsp,rbx 복원, lib.rs:80-89·709-716)은 shadow stack 비관여. ②(T2): cranelift 네이티브 로워링은 call_indirect+return_만 사용하고 return_call(jmp 테일콜)은 미사용 [실측: /home/ybi/otter/crates/otter-cranelift/src/lib.rs:1000-1017 — TailCall이 call_indirect(tail_signal)+return_(fill)로 로워링]; return_call_indirect는 Twasm(O-14 AOT wasm 산물)에만 존재하며 그 실행은 호스트 런타임 위임. ③(언와인딩): catch_unwind 3개소가 전부 extern "C" 진입점 내부, 즉 JIT 프레임 경계보다 안쪽 [실측: otter-engine/src/lib.rs:1181(module_dispatch), 1287(tail_signal), 1054(try_dispatch)] — 패닉이 JIT 프레임을 관통해 언와인딩하는 경로가 존재하지 않고(관통했다면 SS 이전에 언와인드 정보 부재로 이미 비정상), 호스트fn은 extern "C"라 언와인드 탈출 시 abort(관통 아님). 귀결: CET shadow stack 강제 환경에서 방출 코드는 구조적으로 call/ret 균형이며 불일치 지점 미발견. [추정] SS-활성 실기(리눅스 6.6+ user shstk)에서의 전 스위트 통과는 미검증 — CI 레인 부재.
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) 방출기 변경 스윕 — ret-외 복귀, 간접 jmp, 네이티브 return_call 도입 여부(O-8c 후속·O-15 네이티브화가 최우선 감시 대상); 도입 시 본 클래스 즉시 재심; (ii) catch_unwind 신설 시 'JIT 프레임 관통 언와인드 부재' 논증 갱신; (iii) (b)클래스의 SpectreRSB/ret2spec·O-8c tail-call ret-흐름 감사 항목과 상호 참조 유지(동일 표면의 두 관점).
- **감사/완화 후보**:
  - [중] user-space shadow stack 활성 실기(Linux 6.6+, shstk-enabled glibc/커널) CI 레인에서 3-티어 차등 테스트 전 스위트 실행 — [추정](실기 미검증) 해소
  - [하] 방출 바이트 정적 게이트를 테스트로 자동화: T1 코드 스트림에 FF /4·/5(간접 jmp) 부재, 복귀 시퀀스가 C9 C3 단독임을 단정 — per-release 의무 (i)의 기계화

#### B5. 레지스터 컨텍스트 경계의 상태 누수 (방출코드↔호스트fn 레지스터 위생)

- **노출 판정**: `not-exposed`
- **근거**: 실측 ①(판독 프리미티브 부재): 게스트 바이트코드가 도달 가능한 IR은 Expr{Imm,ImmF64,OperandImm,LocalAt,Bin,Un,Host1/2,HostCtx1/2}·Effect{SetLocalAt,JumpTo,JumpIf,Halt,Guard,Call,Return,TailCall}가 전부로, 아키텍처 레지스터를 지명·판독할 수단 자체가 없다 [실측: /home/ybi/otter/crates/otter-baseline/src/lib.rs:222-402(emit_expr 전 arm), 442-473·800-929(effect 전 arm)]. ②(write-before-read): 모든 T1 템플릿은 스크래치를 판독 전 기록 — setup_int(rcx←rax 후 pop) [실측: lib.rs:56-59], setup_float [61-66], 정렬콜의 rbx(push 후 기록) [80-89, 709-716], 비교의 cl(setnp 선기록) [298-299], HostCtx 마샬링의 명시적 선기록 순서 주석 [133-155]. 프레임 슬롯도 전량 초기화: locals는 fill/0 [실측: lib.rs:423-434, 768-772], 파라미터는 argc-가드 [774-788], argbuf는 기록분(n)만 callee가 판독 [886-891]. T2는 SSA라 미정의 판독이 구조적으로 불가하고 전 var가 fill로 def [실측: /home/ybi/otter/crates/otter-cranelift/src/lib.rs:880-884]. ③(경계 방향별): 호스트fn→방출코드 복귀 시 caller-saved 레지스터(rcx,rdx,rsi,rdi,r8-r11,xmm)에 호스트 내부 값이 잔존한 채 비신뢰-유래 코드로 복귀하는 것은 사실이나(정리 코드 미방출 — 실측: emit_aligned_call 후처리는 rsp/rbx 복원뿐, lib.rs:87-88), ①·②에 의해 방출 코드가 그 잔존값을 아키텍처적으로 읽는 경로가 없다. 방출코드→호스트fn 방향(게스트 값의 레지스터 노출)은 호스트=신뢰 전제라 비해당. 잔존 레지스터의 과도-실행(transient) 표본화는 고해상도 타이머·제2 실행 주체 등 별도 관측 채널이 전제 — otter 스톡에 없는 표면이므로 (b)/(c) 클래스 의무로 위임(임베더 구성 종속). ④: otter는 자체 스레드/컨텍스트 전환을 수행하지 않으므로(단일-에이전트 규범) OS 문맥교환 레벨의 레지스터 위생은 커널 층위 소관.
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) 신규 방출 템플릿의 write-before-read 불변 리뷰(리뷰 체크리스트 고정 항목) — rax 결과 체인·ABI 파라미터 외 레지스터의 판독-선행 도입 여부; (ii) IR에 레지스터-판독·원시메모리·인트로스펙션 프리미티브 추가 여부 스윕 — 추가되는 순간 본 클래스 즉시 재심; (iii) 동시성/컨텍스트 전환 표면 도입 시 (c)-(i) 게이트와 연동해 transient 잔존 창 재평가.
- **감사/완화 후보**:
  - [중] 방출기 write-before-read 불변을 기계 검사로 고정 — 방출 바이트 스트림에 대한 템플릿 단위 단정 테스트(각 템플릿이 정의 전 판독하는 레지스터 집합이 {문서화된 입력}과 일치) 추가; 현재는 코드 주석·리뷰에만 의존
  - [하] 호스트fn 복귀 직후 caller-saved 스크래치 제로화(xor) 옵션 — transient 잔존 창 축소용 심층 방어; 아키텍처 경로가 없으므로 기본 OFF, (b)클래스 재심 시 재평가

### (g) 동시성/타이밍 확장

#### C1. 아키텍처적 race condition — atomic livelock · thread hopping (동시성 구조 결함)

- **노출 판정**: `embedder-dependent`
- **근거**: otter 자체에는 경합할 실행 주체가 없다: 전 crates/에서 std::thread/rayon/crossbeam/tokio·Mutex/RwLock·AtomicU*/AtomicI*·static 가변 상태·thread_local 사용 0건 [실측: 전수 grep — 'threaded' 히트는 전부 ctx 포인터 전달을 뜻하는 주석; 테스트의 spawn은 std::process::Command 서브프로세스, /home/ybi/otter/hosts/otter-aot-link/tests/l1a_gates.rs:670-690]. unsafe impl Send/Sync도 crates/+hosts/ 전체 0건 [실측: 전수 grep]. 네이티브 임베딩에서 엔진 상태는 전부 &self 뒤 Cell/RefCell 내부가변성 [실측: /home/ybi/otter/crates/otter-engine/src/lib.rs:281-343 — counts/baseline/cranelift/trapped/tail_pending/tail_callee/tail_args/pushed]이므로 !Sync가 컴파일러-강제되어 &ModuleEngine의 스레드 간 공유는 안전 Rust로 불가 — 단일-에이전트 '규범'이 네이티브 측에선 타입 시스템 강제다. tail_signal 윈도우 논증('컴파일드 측에 Rust 코드가 없는 창이라 어떤 otter 실행도 끼어들 수 없다')과 add-only 코드 등록의 무-race 논증은 단일 스레드 전제를 명문화하고 있다 [실측: otter-engine/src/lib.rs:17-38(모듈 doc, 특히 30-35), 505-517(트램폴린 즉시-소비), 1158/1252(module_dispatch/tail_signal)]. wasm atomics 방출 실측: otter-wasmemit이 방출하는 명령 전수 인벤토리는 22종(Block/Br/BrTable/CallIndirect/Drop/Else/End/I32Const/I32WrapI64/I64Const/I64Eqz/I64GeU/I64GtS/I64Load/I64Store/If/LocalGet/LocalSet/Loop/Return/ReturnCallIndirect/Unreachable)으로 atomic(0xFE 프리픽스) 계열 0건 [실측: /home/ybi/otter/crates/otter-wasmemit/src/lib.rs 전수 grep; 대표 방출부 745-987, 864-877, 1590-1613] — O-17 공유메모리 지원은 memory 임포트 TYPE 파라미터화뿐이다(EmitOpts::shared_memory, lib.rs:188-236; IR atomics는 O-18=D2로 이연). 단, wasm 배치에서는 단일-에이전트가 규범-강제일 뿐이다: wasmtime SharedMemory는 Send+Sync라 임베더가 안전 Rust만으로 2 스레드 × 2 인스턴스 × 동일 공유메모리(o17 §6/§8이 금지하는 L2 형상)를 구성할 수 있고, 그 경우 b′ 엔진의 비-atomic 코드가 공유메모리 위에서 경합한다 — justfile:75의 --no-check-features가 끈 lld 안전망이 정확히 이 사고를 지키던 망임을 o17 §3.1이 자인한다 [실측: /home/ybi/otter/.claude-research-library/otter-o17-shared-memory-l0.md:31-32, 58-64, 73-76]. 귀결은 wasm 샌드박스 내 게스트-상태 손상(호스트 프로세스 메모리 안전은 유지)이다 [추정: 결과 오염·크래시 수준, 호스트 탈출 아님]. b′ +atomics 프로파일 산물은 rustc/LLVM이 방출한 진짜 atomic 명령을 포함하나(otter 방출기 아님 [실측: justfile:61-75]) 단일-에이전트에선 무경합 — atomic livelock과 thread hopping은 모두 ≥2 실행 주체 전제라 스톡 구성에서 불성립. 스폰 경로 자체도 호스트 트레인에 부재: wasi-threads는 상류 제거(wasmtime 47), 호스트는 48.0.1 [실측: .claude-research-library/otter-wasm-scope-escalation-research.md:52; 참조 호스트 'never spawns' hosts/otter-host-wasmtime/src/main.rs:170-171]. wasmtime threads tier-2 표면은 (d1) 관할과 중첩(본 클래스는 위임하고 중복 등재하지 않음). D2(IR atomics)·D3(L2 멀티스레드 엔진)·C-12(공유 funcref 테이블)는 전부 이연/관망 [실측: /home/ybi/otter/docs/otter-v2-major-bump-register.md:29,36-37] — 어느 하나 착수가 본 클래스 전면 재심 트리거다.
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) otter-wasmemit 방출 명령 인벤토리 스윕 — atomic(0xFE) 계열 부재 확인(골든 벡터/전수 grep, 신규 Instruction:: 변형 추가 시 목록 갱신); (ii) (c)-(i) 스레드-스폰 부재 grep 게이트를 확장 실행: crates/+hosts/ 비테스트 코드에서 thread::spawn/Mutex/RwLock/Atomic*/static 가변/thread_local/unsafe impl Send|Sync 0건 확인 — 1건이라도 등장하면 o17 §6 규범대로 본 클래스+(c) 전면 재평가 발동; (iii) v2 레지스터 D2/D3/C-12 상태 확인 — 'watching/이연'에서 '착수'로 바뀌는 순간 재심 인벤토리(tail_signal 윈도우, RefCell 게시, add-only 등록, W^X RW-창, --no-check-features §3.1, 스크래치·엔진 상태 동기화) 전건 재감사; (iv) b′ +atomics 프로파일의 단일-에이전트 전제 문서(justfile 61-75 주석, o17 §6)와 heap_floor 사이드카(0x100000/0x110000) 불변 확인; (v) 호스트 트레인 핀 범프 시 스폰 표면 재출현 여부 확인(wasi-threads 부활·shared-everything-threads 구현 등 — (d1) 의무와 공유).
- **감사/완화 후보**:
  - [중] otter-wasmemit 테스트에 방출-명령 인벤토리 게이트 추가: 방출 바이트를 wasmparser로 재파스해 opcode 집합 ⊆ 화이트리스트(atomic 0xFE 부재 포함)를 단언 — 기존 §5.2 섹션 화이트리스트·unshared-table 단언(tests/refusal_and_structure.rs:272)과 같은 결의 회귀 철조망
  - [중] 임베더 가이드 경화: 'wasmtime SharedMemory(Send+Sync)로 안전 Rust만으로 L2 형상(다중 스레드 × 동일 공유메모리 × 복수 b′ 인스턴스)이 구성 가능하며 이는 o17 §6 위반'임을 출하 문서(README/사이드카 설명)에 명문화 — 현재 규범은 설계 문서와 호스트 주석(main.rs:170-171)에만 존재
  - [중] per_release_duty (ii)의 grep 게이트를 just 레시피+CI로 기계화(수동 스윕 의존 제거) — 스폰/동기화 프리미티브/unsafe impl Send|Sync 부재를 릴리즈 파이프라인에서 자동 판정
  - [하] otter-baseline ExecBuf::map(lib.rs:504-536)에 RX 플립 후 코드 바이트 재검증(verify-after-flip) 추가 검토: 멀티스레드 임베더 프로세스에서 RW 창(mmap RW→copy→mprotect RX 사이) 중 제3 스레드 기입을 탐지 — 공격자가 이미 in-process 임의 기입을 가진 시나리오라 방어 가치는 한계적이나 저비용
  - [하] ModuleEngine::dispatch 진입 시 tail_pending==false debug_assert 추가 검토: tail_signal 윈도우 불변식(모듈 doc 33-35행)의 기계 검증 — D3 착수 시 재설계 대상임을 코드 위치에서 표시

#### C2. timer side-channel 일반화 — 루프-타이머(방출 코드 자체를 카운터로) · 호스트 시간-표면 목록화 충분성

- **노출 판정**: `embedder-dependent`
- **근거**: 기존 (c)의 전제(otter는 시계를 제공하지 않음)를 재실측으로 재확인: crates/ 본체에 Instant/SystemTime/clock API 0건, 유일한 시간 사용은 dev 테스트(otter-wasmemit/tests/calls.rs:41,738)뿐 [실측]. 신규 관점 ① 루프-타이머: 단독(단일 에이전트)으로는 불성립 — otter IR에 메모리 load/store 프리미티브 자체가 없고(Expr는 Const/LocalAt/Host*/HostCtx* 계열, Effect는 SetLocalAt/Guard/Jump*/Call/TailCall/Halt/Return 전수 [실측: /home/ybi/otter/crates/otter/src/lib.rs:433-533]), 방출 코드의 메모리 접근은 (a) 프롤로그 args 로드 — 호출자 제공 포인터 + 정적 오프셋 i*8, argc 가드 [실측: otter-wasmemit/src/lib.rs:1595-1609], (b) Call 마샬링 store — 방출기 자기-배정 고정 scratch 주소 + 정적 오프셋 k*8 [실측: 같은 파일 864-877, marshal_call_args]뿐이라 게스트 바이트코드는 읽기든 쓰기든 주소를 선택할 수 없다. 즉 관측자 없는 순수 반복 카운터는 아무것도 측정하지 못한다. 단 제2 실행 주체가 존재하면(임베더 L2 구성) 방출 산물이 clock-비중립이 된다: 바이트코드 Call 루프는 매 반복 고정 shared-scratch 주소에 I64Store를 수행하고 그 주소는 매니페스트에 기록되므로(scratch 자기-배정, AotOpts §2.5 [실측: lib.rs:1731-1749]) atomic 명령 없이도 SAB-clock의 증분기 반쪽으로 기능할 수 있다 — 전제(제2 주체 + 공유메모리 링크)가 기존 (c) SAB-as-clock과 동일하므로 노출 판정도 동일하게 임베더-종속. 신규 관점 ② 목록화 의무의 범위 결함: 참조 호스트는 p1 '전' 표면을 동기 배선하므로 [실측: hosts/otter-host-wasmtime/src/main.rs:180 p1::add_to_linker_sync] 게스트가 닿는 시간-표면은 clock_time_get/clock_res_get만이 아니라 poll_oneoff(타임아웃 = 시계), fd_filestat_get/path_filestat_get(atim/mtim 타임스탬프 = 저해상도 시계)를 포함하고, p3-bridge 게이트 빌드는 wasi:clocks/monotonic-clock@0.3.0을 추가한다 [실측: main.rs:54,606]. 현행 (c)-(ii) 의무 문구('클럭 표면 목록화')는 명시적 clock API만 열거하게 읽혀 시간-드러내는 간접 표면을 놓칠 수 있다 — 문구를 '시간-드러내는 표면 전수'로 확장할 필요(신규 편입 사유). 기타: 호스트 fn의 완료-지연 자체가 암묵 시계가 될 수 있으나 이는 임베더가 등록하는 host fn의 속성이다 [추정 — 저장소 내 해당 host fn 없음]; wasmtime fuel은 차등 하니스 전용이고 게스트 비가시 [실측: otter-wasmemit/tests/aot.rs:887]. 실효 해상도는 OS 종속 [추정 — 기존 (c)와 동일 표기].
- **릴리즈별 반복 의무**: 매 testing/stable 릴리즈: (i) (c)-(ii) 의무를 확장 집행 — 배포되는 모든 호스트에 대해 '시간-드러내는 표면 전수' 목록화(clock_time_get/clock_res_get + poll_oneoff 타임아웃 + fd_filestat_*/path_filestat_* 타임스탬프 + p3 monotonic-clock + 신규 배선 일체)를 릴리즈 노트에 기재; (ii) crates/ 본체 시간-API 부재 grep(Instant/SystemTime/clock — 테스트 외 0건 유지) 확인; (iii) 방출 코드의 메모리 접근이 '호출자/방출기 지정 주소 + 정적 오프셋' 한정임을 신규 emitter 패턴 추가 시마다 재확인(게스트-선택 주소 store가 하나라도 생기면 루프-타이머 증분기 논증이 바뀌므로 본 클래스 재심); (iv) 공유메모리+제2주체 결합 감시는 (c)-(i)/(iii) 및 C1-(ii)와 공유 — 중복 실행하지 않고 결과를 상호 참조.
- **감사/완화 후보**:
  - [중] 참조 호스트 README + 릴리즈 노트 템플릿에 '시간-드러내는 표면' 표준 목록표 추가(p1 full-surface 배선이라는 사실 자체를 표면 선언으로 기재) — (c)-(ii) 의무 문구도 '클럭 표면'→'시간-드러내는 표면 전수'로 개정
  - [중] 클럭-프리 배선 레시피 제공 검토: p1::add_to_linker_sync 전체 배선 대신 시간-표면(clocks/poll/filestat)을 뺀 부분 배선 예시(또는 호스트 플래그)를 문서/데모로 제공 — 참조 호스트가 'e2e 데모 심'임을 감안하면 코드보다 문서 우선도 가
  - [하] otter-wasmemit 모듈 doc에 clock-비중립성 주석 추가: 공유메모리 + 제2 주체 구성에서는 방출 산물의 고정-주소 Call-마샬링 store(매니페스트 공개 주소)가 카운터 증분기로 쓰일 수 있음을 명기 — 완화가 아니라 표면 추적 목적
  - [하] [추정] 임베더 가이드에 host-fn 지연 채널 주석: 완료 시점이 실시간에 종속되는 host fn(I/O 등)을 등록하면 게스트에 암묵 시계를 제공하게 됨을 경고 — 저장소 내 실물 표면은 없어 문서 항목

### 확장 평가 기록 노트

- (A) 실측 방법: 세 방출기(otter-baseline/src/lib.rs 전문, otter-cranelift/src/lib.rs 전문, otter-engine/src/lib.rs 디스패처·트램폴린)와 코어 checker(otter/src/lib.rs:1589-1628, MAX_SAFE_NLOCALS :1747)를 직접 통독하고 명령 패턴(0xFF 0xD0 call rax, call_indirect, mov rax,[rdi+rcx*8], leave;ret 0xC9 0xC3, sub rsp)을 확인했다. 완화 부재는 전 크레이트 grep(spectre/lfence/retpoline/mfence/probestack/inline_cache 부재)으로 교차 확인.
- (A) cranelift 기본 설정 실측: new_module(otter-cranelift/src/lib.rs:134-138)은 JITBuilder 기본값만 사용하고 어떤 flag/settings도 설정하지 않는다(플래그 설정 grep 무결과). 따라서 enable_probestack=false(cranelift-codegen-meta settings 기본), aarch64 use_bti=false·use_csdb=false(arm64.rs:52-60)가 그대로 적용된다. enable_table/heap_access_spectre_mitigation은 기본 true이나 '(Only read by wasmtime-cranelift)'이고 cranelift heap/table legalization에만 적용되는데 otter는 raw load(MemFlags::trusted)만 쓰므로 B10 경로에 이 완화가 적용되지 않는다(사실상 inert) — 'exposed-mitigated로 오분류하지 말 것'.
- (A) B10 재확인: CallMode::Table의 무경계 table[callee] 로드는 T1(:907)·T2(:1046-1051) 동형이며 기존 게이트 처분(오너 기결정 tier-parity 예외, 비신뢰 입력 금지 규범)과 동일 실체. A1(v2 가젯)·A6(간접호출 전 게스트-인덱스 로드)에서 재등장하나 새 취약점이 아니라 동일 표면의 상이 관점. 대조: Twasm/A-모드(otter-wasmemit)는 Call/TailCall에서 i64.ge_u nfuncs 경계검사 후 call_indirect(:963-1020)로 fail-closed 바운드가 있어 native Table 모드와 달리 B10 무경계 갭이 없음 — 호스트 런타임(wasmtime) 타입체크+바운드에 위임.
- (A) 임베더블 라이브러리 관점 일관 적용: A1~A4·A7의 공격 전제(고해상도 타이머, 제2 실행 주체/SMT 형제, 비신뢰 바이트코드 투입 경로)는 전부 임베더/호스트 구성이지 otter 자체가 제공하는 표면이 아니다(코어 no_std+forbid(unsafe), 타이머 API 전무, 스레드 스폰 부재). 단 소프트웨어 완화층은 otter에 '부재'(위임이 아님)이므로 전제가 조립되면 in-otter 차단막이 없다는 점을 각 근거에 명기했다.
- (A) not-exposed는 A5(speculative type confusion)뿐 — 투기적으로 혼동될 런타임 타입 시스템(타입 피드백/IC/hidden class)이 otter에 존재하지 않음(Value=i64 무타입, NumTy는 신뢰 Isa 정적). Effect::Guard는 타입 투기가 아니라 deopt 정합용이며 그 brif의 v1 형상 잔여 위험은 기존 (b)클래스로 포섭.
- (A) exposed-open 클래스 없음. A6이 유일하게 otter 자체 모델(비신뢰 바이트코드=THE 입력)에서 검증-통과 입력으로도 발동 가능한 실체(대형 frame·깊은 비-tail 재귀)를 가지나, 결과가 가용성 DoS·fail-stop(프로세스 crash, 메모리 손상 아님)이고 MAX_SAFE_NLOCALS 상한+guard page+순차 초기화가 blast radius를 crash로 한정하므로 exposed-mitigated로 판정 — 단 재귀 깊이 무제한과 양 티어 스택 프로브 부재는 실코드 작업이 필요한 high 우선순위 감사 후보.
- (A) otter-ownable 완화(하드웨어/OS층이 아니라 otter가 직접 코드로 처리 가능한 것): A4(T1 ret/jmp 뒤 INT3 SLS 하드닝, medium)와 A6(비-tail 재귀 깊이 가드+스택 프로브/nlocals 상한 하향, high) 두 건. 나머지 A1/A2/A3/A7의 실 완화는 호스트 커널·마이크로코드·SMT 정책 소관이라 otter는 문서 의무(임베더 경고 유지)와 표면 추적이 주된 처분.
- (A) 게이트 편입 형식 제안: 기존 (b)Spectre/천이실행 행 아래 세부로 A1~A6를, MDS는 신규 (e)마이크로아키텍처 버퍼 샘플링 행으로 append-only 추가(삭제 금지 규범 준수). A5는 not-exposed이나 향후 최적화 도입 시 재평가 트리거로 등재 가치가 있음.
- (A) 미검증/추정 항목: [추정] wasmtime 48 Cranelift의 상위 spectre 완화 기본 ON 여부(호스트 Config 미변경 가정, 기존 게이트와 동일); [추정] 각 CPU/마이크로코드 세대별 MDS/SLS 실효 완화 상태(호스트 종속); aarch64 T1(cranelift)은 아직 미배포이므로 use_bti/use_csdb 관련 판정은 그 티어 실배포 시점 재평가 대상.
- (B) 평가 기준점: /home/ybi/otter 워킹트리 v1.0.0-testing.1, 락 핀 cranelift-jit 0.132.3 + memmap2 0.2.3(둘 다 레지스트리 소스 실측). 저장소는 read-only로 유지 — 본 평가는 분석만 수행, 수정 없음.
- (B) 판정 총괄: B1·B2 = embedder-dependent, B3·B4·B5 = not-exposed. 기존 게이트의 'exposed-open 없음' 구조와 정합 — 단 B1은 보안 결함과 별개로 CET-IBT 강제 임베더에서 결정적 가용성 실패(기능 비호환)라는 성격을 가지므로 릴리즈 노트 고지 대상.
- (B) Twasm/A-모드는 네이티브 코드를 전혀 방출하지 않음(순수 wasm 바이트 생성; mmap/mprotect/transmute/libc grep 0건 [실측: /home/ybi/otter/crates/otter-wasmemit/src/lib.rs]) — B군 전 클래스가 호스트 런타임(wasmtime 48 핀)에 위임된다. [추정] wasmtime 48 x64도 Cranelift 기반이라 endbr64 미방출로 추정 — (d1) 릴리즈 의무에 'wasmtime x64 CET 지원 상태 확인' 병기 권고.
- (B) B10 연동 실측 추가: 기존 게이트 문서가 'baseline 동형'이라고만 적은 T1 Table 모드의 무경계 table[callee] 로드의 정확 좌표는 /home/ybi/otter/crates/otter-baseline/src/lib.rs:905-907 (mov rcx,[rbp+disp(base)]; mov rax,[rdi+rcx*8]; call rax) — cranelift 측 1046-1051과 동일 실체이며 B1(전방 CFI 부재 간접 호출)·B2(스프레이 착지점) 관점이 같은 행에 겹침. B10 수용 전제(비신뢰 입력 금지)가 깨지면 B1/B2도 함께 재심.
- (B) 임베더/otter 구분 원칙(기존 게이트와 동일하게 적용): 타이머·제2 실행 주체·비신뢰 바이트코드 투입 경로·CET 정책은 전부 임베더 구성이고, otter 스톡은 단일-에이전트·무타이머·no_std 코어다. 반면 endbr64 부재(B1)와 상수 블라인딩 부재(B2)는 otter 산물 자체의 무조건적 속성으로 실측됨 — '노출'은 임베더 종속이지만 '결함의 존재'는 아니므로 audit_items의 코드 작업이 게이트 등재와 별개로 성립한다.
- (B) 게이트 등재 형식 제안: 본 5클래스를 RELEASE-GATE.md 체크리스트에 (e) 'JIT 코드-방출 위생' 그룹으로 append(append-only 원칙)하고, v1.0.0-testing.1 행에 본 평가를 부기. B1-상(endbr64 T1 선방출)과 B3-상(install_* debug_assert)은 코드 1~4바이트/1행 수준의 저위험 하드닝이므로 다음 testing 릴리즈 전 처리 후보.
- (B) 본 평가는 기존 태스크 #29(보안 게이트 확장 — JIT/CPU 취약점 클래스 편입)의 B그룹 입력에 해당.
- (C) 기존 게이트 대조 — C1: '부분 덮임'. 덮인 부분: (c)-(i) 스레드-스폰 부재 grep 게이트가 '제2 실행 주체 부재' 전제를, (c)-(iii)이 shared_memory(true)+단일-에이전트 규범 결합을, (d1)이 wasmtime threads tier-2 표면을 이미 관할. 안 덮인 신규 부분: ① 방출-명령 인벤토리(Twasm 산물의 atomics 부재)는 어떤 기존 의무에도 없음 — 실측으로 0건 확정(22종 인벤토리), 회귀 철조망 필요; ② D2/D3/C-12 레지스터 상태와 게이트의 결합(착수=재심 트리거)이 미편입; ③ 엔진 내부의 단일-스레드-전제 논증 목록(tail_signal 윈도우, RefCell 게시, add-only 등록, W^X RW-창)을 L2 재심 인벤토리로 등재하는 것; ④ 안전-Rust만으로 L2 위반 형상이 구성 가능하다는 사실(wasmtime SharedMemory=Send+Sync)의 명문화 — 규범의 강제 수단이 네이티브(타입 시스템 강제: Cell/RefCell !Sync + unsafe impl Send/Sync 0건)와 wasm(문서 규범뿐)에서 비대칭임이 이번 실측의 핵심 신규 발견.
- (C) 기존 게이트 대조 — C2: '대부분 덮임 + 신규 2건'. 덮인 부분: Prime+Probe·SAB-as-clock의 전제 분석과 클럭 표면 목록화 의무 자체는 (c)에 존재. 신규 ①(부분 덮임 성격): 방출 산물의 clock-비중립성 — otter-emitted 코드의 고정-주소 마샬링 store가 증분기 반쪽이 될 수 있다는 사실은 (c)에 미기재이나 공격 전제(제2 주체)는 동일하므로 판정 불변, 근거 문단 증보 대상. 신규 ②(순수 신규): (c)-(ii) 목록화 의무의 범위 결함 — 참조 호스트가 p1 '전' 표면 배선(main.rs:180)이므로 poll_oneoff 타임아웃·filestat 타임스탬프 같은 간접 시간-표면이 '클럭 표면'이라는 문구 밖에 있음. 의무 문구 개정('시간-드러내는 표면 전수')을 권고. 단독 루프-타이머(제2 주체 없이)는 otter 프리미티브만으로 구성 불가 — 시간 API 전무 + IR 메모리 접근 전무가 실측 근거.
- (C) 핵심 실측 기록(방출기): otter-wasmemit 방출 명령 전수 = Block/Br/BrTable/CallIndirect/Drop/Else/End/I32Const/I32WrapI64/I64Const/I64Eqz/I64GeU/I64GtS/I64Load/I64Store/If/LocalGet/LocalSet/Loop/Return/ReturnCallIndirect/Unreachable(22종, atomic 0건) [/home/ybi/otter/crates/otter-wasmemit/src/lib.rs 전수 grep]. I64Load는 프롤로그 args 로드(1595-1609)뿐, I64Store는 마샬링 scratch(864-877)뿐 — 둘 다 정적 오프셋. wasm A-모드 table 호출은 방출부에 경계검사+fill 폴백 내장(957-987, I64GeU→fill) — B10 네이티브 Table 모드 무가드와 대조되는 fail-closed 실측(B10 자체는 (b) 관할 유지, C그룹 비등재).
- (C) 핵심 실측 기록(동시성): crates/+hosts/ 비테스트 코드에 thread::spawn/Mutex/Atomic*/static 가변/thread_local/unsafe impl Send|Sync 전부 0건; 엔진 상태는 전부 Cell/RefCell(&self) — /home/ybi/otter/crates/otter-engine/src/lib.rs:281-343; 테스트 spawn은 서브프로세스(Command). b′ +atomics 산물의 atomic 명령은 rustc/LLVM 산(justfile:61-75), otter 방출기 산이 아님 — 이 구분(방출기 vs 링크 파트너)을 게이트 문안에 유지할 것.
- (C) 게이트 편집 제안: C1·C2를 (c)의 하위 확장으로 편입하기보다 별도 행(신규 클래스)으로 append하되, per_release_duty에서 (c)-(i)/(iii)·(d1)과의 공유 항목을 상호 참조로 묶어 이중 집행을 피할 것. (c)-(ii) 문구 개정은 append-only 원칙과 충돌하지 않음(의무 강화는 삭제가 아님).
- (C) [추정] 표기 항목 정리: ① L2 위반 형상에서의 손상 귀결은 '게스트-상태 오염·크래시 수준, 호스트 탈출 아님'(wasm 샌드박스 논증에 근거한 추정 — 실측 불가); ② host-fn 지연 채널(저장소 내 실물 없음); ③ WASI 시간-표면의 실효 해상도(OS 종속, 기존 (c)와 동일).
- (C) 저장소 상태 부기: 과제 지문의 B10 인용 행(otter-cranelift lib.rs:1046-1051)은 기존 게이트 표기이고 B-대장 정본(/home/ybi/otter/docs/otter-b-limitations-register.md:130)은 1047-1052로 표기 — 동일 실체, 본 평가에서는 재인용만 하고 재실측하지 않음(C그룹 관할 아님).

### 감사/완화 후보 대장 (게이트 등재와 별개의 실코드 작업)

**정책 (오너, 2026-09-04)**: 본 대장의 항목들은 **"가능한 한 출하 전 완화 방출 강력 권장" — 의무 아님.**
아울러 이 19행이 전수라는 보장은 전혀 없다(오너 명시) — 대장은 **비전수·open-ended**이며, 매
testing/stable 릴리즈 평가와 신규 방출-패턴 감사에서 후보를 계속 발굴해 append한다. 운영 규칙:
각 testing/stable 릴리즈는 대장 항목별 처분(채택 / 사유 있는 이연)을 기록하고, 채택분은
verify-before-implement 절차(산물 바이트 변경이므로 차등 게이트 필수)로 구현한다.

| 우선 | 출처 | 항목 요지 |
|---|---|---|
| 중 | A1 | B10 Table-모드 간접 호출 경로를 v2 가젯 관점에서 지정 감사 |
| 중 | A4 | T1 방출기에 SLS 하드닝 추가 검토 |
| 상 | A6 | 비-tail 재귀 깊이 가드 도입 검토 (채택 완료 — H-패키지 H1a) |
| 상 | A6 | T1/T2 스택 프로브 부재 지정 감사 (채택 완료 — H-패키지 H1b/H1c) |
| 중 | A6 | 대형 frame의 순차 초기화 fail-stop 불변을 회귀 테스트로 고정 |
| 상 | B1 | T1 두 프롤로그(otter-baseline/src/lib.rs:415-421, 756-760) 선두에 endbr64(F3 0F 1E FA) 4바이트 선방출 (채택 완료 — H-패키지 H2) |
| 중 | B1 | T2 Cranelift 엔트리용 엔진측 endbr64 썽크 테이블(endbr64 + 직접 jmp rel32 → 실제 엔트리; 직접 점프는 IBT 비검사 대상) 도입 또는 cranelift x64 IBT 지원 업스트림 이슈 추적·기여 |
| 중 | B1 | 런타임 CET 상태 감지(/proc/self/status x86_Thread_features 또는 arch_prctl) 후 JIT 티어를 명시적 오류/T0 폴백으로 거부하는 fail-closed 진단 경로 |
| 중 | B2 | per-engine 난수 키 상수 블라인딩 옵션 |
| 중 | B2 | checker에 비신뢰 모듈의 총 consts/코드 크기 모듈-레벨 상한 추가(현재 per-func nlocals 상한만 존재) |
| 상 | B3 | install_baseline/install_cranelift(otter-engine/src/lib.rs:903-928)에 '기존 슬롯이 None' debug_assert 추가 (채택 완료 — H-패키지 H3: install_wasm 포함 3함수 × 양 브랜치) |
| 중 | B3 | OSR/resumable 경로(run_resumable·osr_continue·단일함수 Engine OSR arm)의 이벤트당 JITModule 영구 누수 정량화 + 완화(동일 (prog,entry_pc) 컴파일 캐시 또는 이벤트 상한) |
| 중 | B4 | user-space shadow stack 활성 실기(Linux 6.6+, shstk-enabled glibc/커널) CI 레인에서 3-티어 차등 테스트 전 스위트 실행 |
| 중 | B5 | 방출기 write-before-read 불변을 기계 검사로 고정 |
| 중 | C1 | otter-wasmemit 테스트에 방출-명령 인벤토리 게이트 추가: 방출 바이트를 wasmparser로 재파스해 opcode 집합 ⊆ 화이트리스트(atomic 0xFE 부재 포함)를 단언 (채택 완료 — H-패키지 H4: 2단 게이트 ⊆ + 집합-동등, 모드-분리 단정 포함) |
| 중 | C1 | 임베더 가이드 경화: 'wasmtime SharedMemory(Send+Sync)로 안전 Rust만으로 L2 형상(다중 스레드 × 동일 공유메모리 × 복수 b′ 인스턴스)이 구성 가능하며 이는 o17 §6 위반'임을 출하 문서(README/사이드카 설명)에 명문화 |
| 중 | C1 | per_release_duty (ii)의 grep 게이트를 just 레시피+CI로 기계화(수동 스윕 의존 제거) |
| 중 | C2 | 참조 호스트 README + 릴리즈 노트 템플릿에 '시간-드러내는 표면' 표준 목록표 추가(p1 full-surface 배선이라는 사실 자체를 표면 선언으로 기재) |
| 중 | C2 | 클럭-프리 배선 레시피 제공 검토: p1::add_to_linker_sync 전체 배선 대신 시간-표면(clocks/poll/filestat)을 뺀 부분 배선 예시(또는 호스트 플래그)를 문서/데모로 제공 |

## 2차 평가: v1.0.0-testing.2 (2026-11-01)

**판정**: 통과(PASS) — exposed-open 클래스 없음. 1차 평가의 반복 의무(per_release_duty)를 `testing.1`(631801a) → 본 컷 변경분(H-패키지 a8bb6f0 + 의존성 보안 갱신 + 툴체인 상향)에 대해 수행했다. 이번 컷의 핵심은 **호스트 트레인이 보안 권고 다수에 노출된 상태(wasmtime/wasmtime-wasi 48.0.1)를 50.0.1로 해소**한 것이다.

### 근거 (모두 본 세션 실측)

- **업스트림 권고 점검 (의무 (d1)(ii) / (d2)(i) / (a)(ii) / (b)(iii))**: 전 락파일(루트 + hosts/* + demos/*, registry 패키지 241개)을 OSV(crates.io)로 일괄 조회 → 5개 패키지에 권고 존재: `wasmtime 48.0.1`(23건: GHSA-9pvw-hwfg-92h5, GHSA-mghf-wjfp-p5xm[CRITICAL, aarch64 br_table], RUSTSEC-2026-0315/0316/0325/0326/0327/0353~0359/0361~0364/0378/0379/0422~0424), `wasmtime-wasi 48.0.1`(7건: RUSTSEC-2026-0314/0321~0324/0352/0420), `wasmtime 45.0.3`(10건, 루트 dev-dep 오라클), `tokio 1.53.1`(RUSTSEC-2026-0386), `memmap2 0.2.3`(RUSTSEC-2026-0186). 전 wasmtime/wasmtime-wasi 항목의 patched 버전 ≤ 50.0.1(가장 높은 것: 0420~0424의 48.0.8/49.0.5/50.0.1) → **호스트 2종 + 루트 오라클을 50.0.1로 상향**(소스 변경 0). tokio는 1.54.1로 상향. 갱신 후 재조회: **209개 패키지 중 권고 1건**(memmap2 0.2.3, 아래).
- **잔여: `memmap2 0.2.3` (RUSTSEC-2026-0186, 경유: cranelift-jit 0.132.3 → otter-cranelift)**: 영향 함수는 `advise_range`/`unchecked_advise_range`/`flush_range`/`flush_async_range`로 한정. cranelift-jit-0.132.3 소스(`memory/system.rs`)와 otter 전 소스를 grep → **해당 함수 호출 0건**(도달 불가). 해소하려면 cranelift 0.132 → 0.136+ 상향(memmap2 ^0.9.11 요구) = T2 JIT 의존 범프로 별도 트랙(아래 후속).
- **Cranelift aarch64 `br_table`(CVE-2026-115893)**: otter T2가 쓰는 cranelift-codegen 0.132.3 계열도 이론상 영향권(aarch64 한정, >~256k 타깃 br_table). otter-cranelift는 br_table/jump-table을 **방출하지 않음**(grep 0건)이고 T2 실행 아키텍처는 x86-64뿐 → 현 구성 not-exposed. Twasm/A-모드가 방출하는 wasm `br_table`은 호스트 wasmtime이 컴파일하며 50.0.1은 수정판. **aarch64 T2 승격(Tier-1 로드맵) 시 본 항목 재심 의무.**
- **스윕 (의무 (a)(i), (c)(i), (f)-B1~B5)**: 631801a..a8bb6f0 변경분(crates/ 12파일 + tests)을 스윕 — 신규 시계 API 0, `unsafe impl Send|Sync` 0, `catch_unwind` 0, mmap/mprotect/호스트 fn 등록/WASI 링커 변경 0, `hosts/` 소스 변경 0. 스레드 스폰 1건은 **테스트 전용**(`tests/h1a_stack_guard.rs:223`, 스택 한도 검증용). 방출 바이트: 새 `0xFF` 오퍼랜드는 `dec ecx`(FF C9, 간접 jmp/call 아님)와 disp32 데이터뿐 → 간접 전이는 기존 `call rax` 2곳 + cranelift `call_indirect`만 유지(B4 불변).
- **H-패키지 효과**: B1(CFI) — T1 `endbr64` 선방출 채택(`endbr64_at_head_of_every_emitted_function` 단정), T2는 여전히 랜딩패드 없음(IBT 강제 + T2 조합은 t2=NEVER 우회); B3 — `install_*` add-only `debug_assert` 신설; (b) 스택 오버플로(방출 프레임 프로브 부재) — T1 sub-후-터치 + T2 inline probestack + 스택 이중 가드(1 MiB 예산 + 깊이 4096)로 fail-stop/회복가능 Trap화.
- **게이트 실행 (Rust 1.99.1, wasmtime 50.0.1, `--locked`)**: `just ci`(fmt + clippy -D + 워크스페이스 테스트 + no_std) · `wasm-e2e` · `wasm-aot-link-e2e` · `wasm-aot-component` · `wasm-aot-e2e` · `wasm64-smoke` · `wasm-e2e-atomics`(d1 의무 (iv)) · `wasm-e2e-p3`(d2 의무 (ii): `HOST-P3-STORE-SYNC-OK` 래치 프로브 + `NEGPROBE-P3-UNWIRED-OK`) **전부 EXIT=0**. 툴체인 1.99 clippy 신규 린트(`chunks_exact_to_as_chunks`) 2건은 테스트 코드 수정(동일 의미)으로 해소.
- **(d2) p3 핀 이동**: `=48.0.1` 락 핀 → **50.0.1 락 핀**(p3 표면은 여전히 semver-면제·보안패치 비보장 자기선언). S2 경계 실측(E5 래치 프로브)은 50.0.1에서 재실행되어 녹색. p3-bridge는 기본 OFF feature 격리 유지.
- **(c)(ii) 시간-드러내는 표면 목록 (변동 없음)**: 참조 호스트 = WASI p1 전체 표면 동기 배선(`clock_time_get`, `poll_oneoff` 타임아웃, fd/path_filestat 타임스탬프) + p3-bridge 빌드에서 `wasi:clocks/monotonic-clock@0.3.0`. 호스트 소스 변경 없음.

### 정직한 미수행/한계

- (d1)(d2)의 "GitHub 이슈 트래커 직접 점검"은 **수행하지 않았다** — 본 평가는 보안 권고 DB(RustSec/GHSA via OSV)와 crates.io 메타데이터 기준이다. tier-2(threads)·p3의 CVE 미발급 이슈는 이 경로로 보이지 않으므로 다음 릴리즈 전 수동 점검 의무로 이월.
- aarch64 실기 게이트는 없음(x86-64에서만 실행·검증).
- 위 OSV 조회는 조회 시점(2026-11-01) 스냅샷이다.

### 후속(다음 컷 이전 권장)

| 우선 | 항목 |
|---|---|
| 중 | cranelift 0.132 → 0.136+ 상향(memmap2 0.2.3 제거, br_table 수정 포함) — T2 JIT 의존 범프이므로 3-티어 차등 게이트 전체 재실행 필요 |
| 중 | 매 컷 전 OSV 락 스윕을 just 레시피로 기계화(이번에는 수동 스크립트) |
| 하 | (d1)(d2) GitHub 이슈 트래커 수동 점검 |

## 평가 이력

| 릴리즈 | 일자 | 판정 | 비고 |
|---|---|---|---|
| v1.0.0-testing.1 | 2026-09-04 | 조건부 통과(조건 2건 처분 완료) | 1차 평가(게이트 발효 릴리즈) |
| v1.0.0-testing.1 (확장 부기) | 2026-09-04 | 통과 — exposed-open 없음 | 오너 목록 14클래스 편입 평가 |
| v1.0.0-testing.2 | 2026-11-01 | 통과 — exposed-open 없음 | 2차 평가: H-패키지 + 호스트 트레인 wasmtime 48.0.1→50.0.1 보안 갱신 + 툴체인 1.99.1 |
