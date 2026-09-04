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

## 평가 이력

| 릴리즈 | 일자 | 판정 | 비고 |
|---|---|---|---|
| v1.0.0-testing.1 | 2026-09-04 | 조건부 통과(조건 2건 처분 완료) | 1차 평가(게이트 발효 릴리즈) |
