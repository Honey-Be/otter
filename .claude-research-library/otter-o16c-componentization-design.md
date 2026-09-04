Status: **구현 완료** (2026-09-04, L-1a~L-6 전 증분 착지 — §10 완료 대차; 설계 본문은 v2 원문 유지, 착지 부기는 §0-말미/§10)

# O-16c′ 설계 — M2a 정적 링크 파이프라인 + 컴포넌트화의 제품화

정본 관계 및 귀속 (명기):
- **선행 정본**: `otter-o16c-spike-results.md`(b′/b″ 실측 종합 — §4 제약 12건, §5 미봉합 상충 6건; 이하 "SPK"), `otter-o14-aot-core-design.md` v2.3(§12 서빙 인터페이스·§2.5 링커 의무 4건·§11 유보 3건; 이하 "O-14"), `otter-o13-wasm-emit-design.md` §7.1(외부 핀 유지보수 절차 선례; 이하 "O-13"). v2는 3렌즈 패널(conflicts/tooling/gates-scope, 2026-09-04)의 심사 반영분 — 처리 원장은 §0.
- **오너 결정 (MODE `otter-wasm-mode-architecture-alternatives.md` §5b 인용 — 자체 결정 아님)**: ① 결정 A — A-모드 게이트 프로파일 = **M2a 엔진-라이브러리 정적 링크**("2제품(M2a-게이트) 종합안 베이스 채택", 상시 재평가 규율 동반). ② 결정 B — 게이트 해석 = **프로그램-AOT로 충분**, 단 부가 규범 "**동적 코드 실행은 어떤 식으로든 격리된 환경을 통해서만, 극히 제한적으로 지원**"의 명문 반영 의무(§6.3에서 이행).
- 관련 실물 [실측]: `crates/otter-wasmemit`(compile_module:2059 / validate_aot_artifact:2402 / AotManifest:1742), `hosts/otter-aot-e2e`(48-트레인 E2E 선례 — 배치 근거 3항은 그 Cargo.toml 주석이 정본), 스파이크 산물 디렉터리(스크래치 `o16c-bprime/`·`o16c-bdblprime/` — COMMANDS.log/commands.log 축자, 본 문서의 커맨드 인용은 전부 이 로그 귀속. **패널 회차 신규 실측**: +atomics b′ 실물 `target-atomics/.../otter_helpers_wasm.wasm` 직접 덤프 — §0-3).
- 표기: **[실측]** = 스파이크/레포/패널 직접 확인, **[추정]** = 미검증 추론(전건 실측화 계획 동반). 설계 선택마다 근거를 명기한다. 스파이크 산물은 스크래치(휘발) 소재 — 본 증분이 레포 실물로 승격하기 전까지 재현 근거는 로그 축자뿐이다.

---

## §0. 개정 요약 (v2 — 3렌즈 패널 conflicts/tooling/gates-scope 반영)

### must-fix 7건 (전건 반영)

1. **heap_floor 공식이 +atomics에서 불건전 — 검사 (iii) fail-open** (conflicts M1 ≡ gates MF-2; 패널 신규 실측: +atomics 실물은 `__stack_pointer`=0x100000·active data 0(전부 passive)이라 구 공식 `max(스택포인터, data end)`가 0x100000을 산출 — SPK 규범 0x110000과 발산, 위반 base=0x100000이 그린 통과 = O-12 침묵-오염 클래스 재개방): 공식에 **`memory_min×65536` 항 추가**(§2-2 — passive 맹점 흡수, 양 프로파일 모두 SPK 규범값과 정확 일치) + L-2 게이트에 "사이드카 floor == 프로파일 기대 상수" 단언.
2. **마커 스킴이 폭<16 region에서 정의 불능** (conflicts M2; calc 실물 region 폭 8B [실측: calc.manifest], base==end−8이라 마커 2개 동시 배치 불가 → §5.2-(i) 명세대로면 영구 적색): **폭-분기 의미론 명세**(§2-1: W≥16 마커 2 / W==8 병합 `OTTRSCRM` 1 / W==0 마커 0·빈-region 분기) + §5.2-(i) 분기 연동. 상충 기록: 병합-매직 vs 16B 하한 패딩 — **병합-매직 채택**(보수: 패딩은 scratch_region 산정 = compile_module 의미 변경이라 본 증분의 otter-wasmemit additive 한계 초과).
3. **+atomics b′ 실물의 start·passive·TLS를 비공유 형상으로 과일반화** (conflicts M3 ≡ gates MF-1; 패널 신규 실측 — target-atomics 산물 직접 덤프: `(start $__wasm_init_memory)` + passive `$.rodata`(~300B 패닉 문자열, 0x100000에 memory.init) + `__tls_base` 실존, SPK B5 미기록): B5 재서술 **프로파일 분리**(§2-1/§4.2), §5.3에 **start 기계 검사 신설**(부재 또는 init-전용 화이트리스트), §6.3 "start 0" 문구 프로파일 분리, wasm-merge×(start+passive+TLS) [추정] 등재(§2-5-③). 상충 기록: 화이트리스트-허용 vs b′에서 소거 — **허용+기계 검사 채택**(보수: 소거는 패닉 문자열 제거 = build-std/panic_immediate_abort = nightly 회귀로 SPK §1 stable-탈출 성과와 정면 상충).
4. **EH-프리 게이트 비공허성 미실증** (gates MF-3; validate 거부 방향 실측 0회): 뮤테이션 ⑧(EH 명령 주입 → validate 적색)·⑨(사이드카 해시/세대 불일치 → 적색) 신설(§5.6).
5. **L-1 게이트의 wasmtime 인스턴스화가 §3.1 무-wasmtime 형상과 자기모순** (gates MF-4 ≡ conflicts S4): L-1 단위 검증 = **wasmparser Validator**(기존 의존, 신규 0), 인스턴스화 단언은 **L-4로 이관**(§7). 상충 기록: 이관처 L-2 vs L-4 vs dev-dep 수용 — **L-4 채택**(보수: wasmtime 실존처로 이관, 의존 신설 0).
6. **b′ 빌드 레시피에 `--locked`·락파일 규율 부재** (tooling M1; b′는 게이트 산출물 그 자체 — 비고정 해석은 사이드카 SHA·재측정 기록 전부 무효화, O-14 v2.3-⑤가 기록한 실패 클래스): §4.2에 `--locked` + Cargo.lock 커밋 + 워크스페이스 핀 수동 락스텝 명기(§3.1 문구 축자; 스파이크 Cargo.lock 실존 [실측] — 승격 1줄).
7. **JSON 사이드카 ser/de 소유자 미결정 — §3.1 의존 목록("…뿐")과 모순** (tooling M2 ≡ conflicts N4; 생산(e2e)/소비(otter-aot-link) 독립 수제 구현은 스키마-층 이중 정본): 스키마·직렬화·역직렬화 = **otter-aot-link lib 타깃 `schema` 모듈 단독 소유**(serde/serde_json — 자체 락파일 격리), 생산 측은 path-dep로 동일 타입 소비(§3.1). 기각 2안: otter-wasmemit serde-게이트(루트+demos 락파일 파급 = v2.3-⑤ 클래스 + "AotOpts doc 1줄" 선언 스코프 상충), 수제 JSON(§3 서두 sed/grep 기각과 동일 클래스).

### should-fix 18건 처리 (기각 0 — 전건 반영; 구현-형태 선택 2건은 대안 기각 사유 명기)

세대-대조 공허성 정직화 + 콘텐츠-해시 정본 승격(C-S1/T-S3 → §4.3), 사이드카 생성 순서 명세 — floor는 strip-전·해시는 최종 원판·확정은 strip-후(C-S2 → §2-2), L-3 → L-2 선행 재배열 — "L-2 ∥ L-3" 철회(C-S3/G-SF-4 → §7), 컴포넌트 형상 엔트리 = argc=0/게스트-부트 한정 명문화(C-S5 → §6.1/§2-4), 폴백 게이트-충족성의 오너 확인 항목화 — 자기 인증 문구 제거(C-S6 → §2-4/§9-4), 마커 주입의 산물별-단계화 — "게이트 표준 region 게이트판" 단수 서술 철회(C-S7 → §4.2/L-4), SHA 초기 핀 TOFU 절차(T-S1 → §3.2), 평 `cargo`로 스테이블 핀 단일 소재지 회복(T-S2 → §4.2), §6.2 검사의 wasmparser `component-model` 네이티브화 — CLI 텍스트 덤프 대조는 범프-공허화 클래스로 기각 + 범프마다 §5.6 재실행 명시(T-S4 → §6.2/§3.2), e2e MEMORY_PAGES 16→18 동반 상향(G-SF-1/T-N5 → §2-2/L-4), rename의 일반 매핑-테이블화로 host 모듈-문자열 규약 리스크 선제 흡수(G-SF-2 → §6.1/L-1a), 잔존-임포트 ⊆ → 정확 일치(=) 강화 — "정확히 일치하는 부분집합" 자기모순 문구 정리(G-SF-3 → §2-4/§5.1), L-1 증분 2분할 + shrink-exports·check --linked의 L-4 이동(G-SF-5 → §7), kebab 규칙 기계 강제(G-SF-6/C-N2 → §6.1), L-1 음성 커버리지를 전 검사 항목으로 확대 + 겹침-세그먼트 주입 음성(G-SF-7 → §7/§5.6-⑩).

### note 처리

export-모드 불변 게이트 별도 명세(C-N1 → §2-6), 자구·인용 정정 — main.rs:53→54, §5.4 "8종"의 대역-host 형상 귀속 명기, §2-4 [실측] 라벨 정밀화, "+atomics floor 상회"→"동일(≥ 충족)"(C-N3/G-N-1/G-N-5), shared-메모리 대상 active 마커 [추정] 명기 + L-1b/L-4 게이트 편입(C-N5/G-N-2 → §2-1/§7), 자체-결합기 기각 논거 교체 — SPK L1 과잉 귀속 정정(T-N1 → §3.2), wasm-tools 핀 출처 전환 부기 + 비-PATH 호출 + `--version` 단언(T-N2 → §3.2), hosts/ "실질 규율" 일반화 철회 + 배치 근거 축자 기록 의무 + ci.yml 캐시 목록 갱신(T-N3 → §3.1/§7), SHA 핀 x86_64-linux 한정 명기(T-N4 → §3.2), 결합물 경유 40k 차등 1회 재실행(G-N-3 → §5.3/L-4), check 실패 복구 경로 교차 참조(G-N-4 → §5.2).

### 착지 부기 (구현 완료 시점 — 설계 대비 이탈·발견 사항 원장; 상세 대차는 §10)

구현이 설계와 갈라졌거나 설계의 [추정]을 뒤집은 지점 전건(각 증분 리뷰가 판정, 커밋 메시지에 원기록):

1. **`__tls_base` 정정 (L-4 리뷰 발견 — §2-1/§4.2 B5 재서술·§2-5-③의 부분 정정)**: +atomics **b′ 산물**이 `__tls_base` 전역을 나른다는 것은 [실측] 그대로이나, **결합물에는 없다** — wasm-merge가 미참조 전역을 소거한다(실측; 오늘은 무해 — 아무도 참조하지 않음). "b′의 start·passive·TLS가 병합을 통과해 보존"이라는 구현 보고 초안의 주장은 리뷰가 이 점을 잡아 정정했다: 보존 실측은 start(init-전용, 화이트리스트 30 op)·passive `$.rodata`·shared-대상 active 마커까지이고, `__tls_base`는 소거가 실측이다.
2. **주입-병합 순서 반전 (L-4 — §5.5 체인 문구 대비)**: §5.5의 "merge→산물별 주입" 문구와 달리 주입은 **병합 전 b′ 마스터에** 수행한다 — §2-1/§4.2의 산물별-단계 규정과 주입기의 입력-SHA 짝 대조(fail-closed)가 이 순서를 강제하고, [추정 ①](주입-후 결합 보존)이 측정 가능한 주장이 되려면 마커가 병합을 **통과**해야 하기 때문(SPK §3-3의 실측 형상과 동일). 레시피 주석에 축자 기록.
3. **start 화이트리스트 확장 2건 (L-4 — §5.3 목록 대비)**: 실물 `__wasm_init_memory` 바디에 `br_table`(가드 분기 — 허용 클래스 br/br_if와 동류)·`i64.const`(atomic-wait 타임아웃 피연산자 — 허용 클래스 i32.const와 동류)이 실존해 화이트리스트에 추가. `atomic.fence`는 실측 부재라 미추가(fail-closed 유지). linked.rs 모듈-doc에 기록.
4. **wasm-merge 피처 census 신규 1건 (L-4 — SPK §2 census 대비)**: shared 프로파일은 `--enable-bulk-memory`가 추가로 필수 — 스파이크 대역 b′(손-WAT, passive 세그먼트 무)가 노출하지 못한 사실. 레시피가 5개 플래그 전건을 drop-one 프로브로 매 실행 기계 검증.
5. **뮤테이션 ⑥ 적색 소재 정정 (L-6 — §5.6-⑥ "embed 실패" 문구 대비)**: kebab 개명 생략 시 `component embed`는 **성공**한다(월드 메타데이터 부착만 수행) — 적색은 다음 단계 `component new`가 "failed to find export of function `otter-entry`"로 시끄럽게 발화(산출물 없음, fail-closed 유지). 레저(otter-aot-link tests/l6_mutation_ledger.rs) ⑥ 항에 기록.
6. **뮤테이션 ③ 발현 시점 발견 (L-6)**: 마커가 실존하는 산물의 min 언더사이즈는 실행 trap(E8)보다 **이른 인스턴스화 시점**에 적색(0x110000 active 마커 세그먼트 자체가 안 들어감) — §2-1/§2-3 벨트-앤-서스펜더의 설계 의도가 실측으로 확인된 형상. 마커 제거판에서는 E8 축자 재현("memory fault at wasm address 0x110000").
7. **wit 월드 2본화 (L-5 — §6.1 "host import 선언이 가산된다" 문구의 구현 형태)**: host-리프트 임포트를 기본 월드에 합치면 host-프리 산물이 미사용 임포트를 선언해야 해 정확-일치 규율과 상충 — `aot-artifact`(host-프리) + `aot-artifact-host`(calc 형상) 2월드로 착지(wit/aot-artifact.wit, 이탈 사유 동봉).
8. **sha2 의존 이탈 (L-1b — §3.1 의존 목록 대비)**: `artifact_sha256` 산출·대조에 RustCrypto sha2 추가 — 수제 SHA-256은 §3.1이 기각한 과잉-자체-구현 클래스, PATH `sha256sum` 셸-아웃은 §3.2가 금지한 침묵-시스템-도구 클래스라 양쪽 기각 논거로 정당화(Cargo.toml에 축자 기록).
9. **인스턴스화 단언 소재 (설계 §0-5 그대로 + L-5 편승)**: otter-aot-link는 wasmtime-프리 유지, `assert-instantiate`는 otter-aot-e2e에 착지(L-5에서 참조-호스트 config 중복 제거 편승).
10. **host-리프트 코어 모듈-문자열 규약 판명 (L-5 — [추정 ⑤] 해소값)**: `$root` + kebab 필드명. rename의 일반 매핑-테이블 설계(§6.1 선제 흡수)가 적중해 도구 재작업 0.

---

## §1. 목표·비목표

### 1.1 목표 — 게이트 산출물의 정의

O-16c′ = 스파이크가 확정한 체인(SPK §2 축자)

```
compile_module → (매니페스트-구동 임포트 개명) → wasm-merge → 자체 게이트 → export 축소 → component embed/new
```

을 **재현 가능한 도구·레시피·게이트**로 만드는 증분이다. 게이트 산출물(= 오너 상향 목표 ① 해석 (iv)~(vii)의 실물 — MODE §4.3 게이트 대차표의 "A/M2a 컴포넌트" 행이 규정하는 충족 수단):

1. **b′ 실물**: 레포-상주 엔진-라이브러리 크레이트 + 빌드 레시피(비공유·+atomics 양 프로파일, EH-프리, stable 1.96) — SPK §1 레시피의 제품화(§4).
2. **b″ 실물**: 링크 드라이버 도구(개명·마커 주입·게이트·export 축소는 자체 구현, wasm-merge만 외부) + 버전-핀 조달 레시피(§3).
3. **결합물 + 컴포넌트 양 형상**의 E2E 게이트: 실물 b′ × 실물 산물의 정적 링크 결합물과 그 컴포넌트 래핑이 T0 오라클과 비트-동일로 실행되고, 전-체인 EH-프리가 기계 검증됨(§5) — SPK §4-12([추정, 근거 강함] 잔존)의 [실측] 승격.
4. **SPK §5 상충 6건의 해소**(§2 — 본 설계의 심장)와 O-14 §11 유보 3건의 종결.

### 1.2 비목표

| 항목 | 처분 |
|---|---|
| O-16f p3-브리지(해석 (i)~(iii)) | **별도 증분** — 크리티컬 패스 병렬 항(MODE §4.1). 본 문서 무관여 |
| O-15/M2b 엔진-리스·산술 자립 | 병렬 트랙 유지(MODE §5b-A). b′는 헬퍼 3종+메모리 소유가 전부(SPK B1) |
| 동적 코드/eval류 지원 | 없음 — 산물에 경로 원리적 부재(O-14 §7-1 승계). §5b-B 규범 명문 반영만(§6.3) |
| 매니페스트 빌드-해시 fail-closed **전체** 규율 | 부분 이행 — b′ 사이드카 **콘텐츠-해시 결부**(§2-2·§4.3 — 해시가 정본, 버전 대조는 공허성 명기)까지만. 산물 전체(방출물 포함) 해시 체계는 배포 채널 확정 시(§9-3) |
| perf(export 축소를 넘는 크기 최적화, wasm-opt 패스) | 없음 — wasm-opt는 재인코딩 표면을 더 넓혀 §5.3 의미-검증 부담만 가중. 금도금 금지(MODE §4.2) |

---

## §2. SPK §5 상충 6건의 해소 결정

### §2-1. 스크래치 예약 형태 — **마커 data 세그먼트 채택, 단 생산 주체는 rustc가 아니라 결정적 주입 단계**

**결정**: b′-결합 체인 산물은 scratch 예약의 **정적 증거로 active data 세그먼트 마커**를 포함한다. **폭-분기 의미론(패널 M2[conflicts] 반영)** — region 폭 `W = end − base`(스크래치는 8워드 단위 배정이라 W ∈ {0, 8, 16k…}):
- **W ≥ 16**: 마커 2개 — `[base, base+8)` 매직 `"OTTRSCR0"`, `[end-8, end)` 매직 `"OTTRSCR1"`.
- **W == 8**(base == end−8 — calc 실물 형상 [실측: o16c-bdblprime calc.manifest `scratch_region=[0x10000,0x10008)`]): **병합 마커 1개** — `[base, end)` 매직 `"OTTRSCRM"`.
- **W == 0**(호출-무 프로그램): 마커 0·min-상향 no-op — 예약 대상이 없는 형상. §5.2-(i)는 명시 빈-region 분기로 통과하되 형상을 로그에 기록한다(침묵 통과 아님).

마커는 rustc/wasm-ld 링크 인자가 아니라 **otter-aot-link의 `inject-marker` 서브커맨드**(wasm-encoder 기반, 결정적)가 주입하며, 주입 시 **memory min을 `max(현행 min, ⌈end/65536⌉)`로 함께 상향**하고(shared 프로파일은 max=65536 한도 내 — 초과 시 주입 실패 = 시끄러운 에러), **주입 전 입력 b′ 원판의 SHA-256을 사이드카와 대조**한다(fail-closed 순서 — §2-2/§4.3). 주입은 링크 레시피의 **산물별 단계**다(region이 산물별이므로 — §4.2 패널 C-S7 정정).

**근거**:
- b″의 정적 검사가 성립하려면 산물 속 정적 증거가 필요하다는 b″ 결론(SPK §3-3 [실측] — "메모리-소유 모듈의 active data 세그먼트 = 예약 마커" 성립, wasm-merge 결합 후 보존 실측)을 채택한다. 수치 규범 1줄만으로는(b′ 결론) 검증기가 대조할 실물이 없어 게이트가 규범 문서를 신뢰하는 순환이 된다.
- **생산 주체를 주입 단계로 옮기는 이유**: rustc/wasm-ld로 특정 절대주소에 세그먼트를 놓는 것은 링커 스크립트 수준의 취약 공정이고, SPK §5-3이 [추정]으로 남긴 "active 세그먼트의 min-페이지 자동 상향(rustc/wasm-ld 링크 인자)"이라는 미실측 의존을 아예 소거한다 — 주입기가 min 상향을 **구성적으로** 수행하므로 [추정]은 "주입기 단위 테스트(wasmparser 검증) + 결합물 인스턴스화 실측(L-4 — 패널 MF-4로 이관 확정)"으로 대체된다. **→ [실측] 승격(L-1b 단위 + L-4 — [추정 ①] 해소)**: 주입 산물 3형상 전건 wasmtime 인스턴스화 수용(INSTANTIATE-OK ×3) + 마커·min 상향의 병합-후 보존(check --scratch가 결합물 상대로 그린; 주입은 병합 **전** b′ 마스터 단계 — §0 착지 부기 2).
- 꼬리 마커(`end-8`)를 두는 이유(W≥16): base 마커만으로는 구간 **말단**의 정적 증거가 없어 커버리지 강제가 주입기 신뢰에만 얹힌다 — 말단 바이트 실물이 있으면 §5.2-(ii) 검사가 세그먼트 존재만으로 이중 확인된다(벨트-앤-서스펜더). W==8에서는 base==end−8이라 병합 마커 1개가 양 역할을 겸한다(패널 M2 해소).
- **b′ 형상 재측정 의무 — 프로파일 분리(SPK §5-1 이행 + 패널 M3 정정)**: 주입 후 산물은 "data 세그먼트 0"이 아니게 되며, **+atomics는 주입 전에도 이미 아니다**. §4.2의 B5 재서술은 프로파일별로 갈라 쓴다 — 비공유 = "라이브러리 자체 데이터 0·start 0 + 예약 마커(주입물)" [실측: 스파이크 산물 재덤프, data/start 0건], **+atomics = "passive `$.rodata`(~300B 패닉 문자열) + `(start $__wasm_init_memory)`(0x100000에 memory.init·data.drop + 원자 초기화 가드) + `__tls_base` 전역 + 예약 마커(주입물)"** [실측: 패널 회차에 target-atomics 산물 직접 덤프 — SPK B5는 이 형상을 미기록했고 v1 문서는 비공유 형상을 과일반화했다]. 크기(비공유 4,158 B + 델타)·census·memory min은 L-2 게이트에서 프로파일별 재측정·기록. "무할당" 성질(trap-알로케이터)은 마커와 무관하게 불변.
- **shared memory 대상 active 마커의 합법성·엔진 수용은 [추정]**(패널 C-N5/G-N-2 — SPK L3 마커 실측은 비공유 스텁, E9 대역은 data 무): L-1b 단위 픽스처(wasmparser 수준)와 L-4-③(wasmtime 수용 + 병합-후 보존)이 실측화한다. **→ [실측] 승격(L-1b 픽스처 + L-4)**: shared-대상 OTTRSCRM 마커가 wasmparser 수준 합법, wasmtime 48 인스턴스화 수용(INSTANTIATE-OK), wasm-merge 병합-후 보존·check --scratch 그린까지 전건 실측 — [추정 ④] 해소.

기각 대안: (a) 수치 규범 단독 — 위 순환. (b) 링크 인자 예약(SPK §5-3 후보 ②) — 예약 사실이 scratch_region과 기계적으로 결부되지 않는 산발 권위. (c) memory.grow 의미론 수용(후보 ③) — 결정적 정적 게이트를 실행-시점 의미론으로 약화, fail-closed 원칙(SPK §4-1) 훼손. (d) 폭<16 region의 16바이트 하한 패딩(패널 M2 대안) — scratch_region은 방출기 산정치(매니페스트 정본)라 패딩은 compile_module 의미 변경 = 본 증분의 otter-wasmemit additive 한계(§2-2의 doc 1줄) 초과; 병합-매직이 산물 무변경으로 동일한 증거력을 준다.

### §2-2. 스크래치 수치 규범의 소재 — **compile_module 호출자 규범 + b′ 사이드카가 하한의 정본 + b″ 전역-심볼 대조 검사 신설**

**결정** (3층):
1. **규범 명문화**: "`AotOpts.scratch_base`는 링크 상대 b′의 **heap_floor 이상**이어야 한다(MUST)"를 compile_module **호출자** 규범으로 명문화한다 — 본 절 + otter-wasmemit `AotOpts::scratch_base` doc 1줄 부기(additive, L-4) + O-14 §12.2에 교차 참조 부기. 수치를 문서 상수로 동결하지 않는다 — 하한의 정본은 아래 사이드카다(라이브러리 세대마다 변한다).
2. **b′ 사이드카**: b′ 빌드 레시피가 산물 곁에 `otter_helpers.manifest.json`을 산출한다. **heap_floor 공식(패널 M1[conflicts]≡MF-2[gates] 정정)**: `heap_floor = max(__stack_pointer 초기값, 전 active data 세그먼트 end, memory_min × 65536)`(주입 마커 제외). **`memory_min` 항이 passive-세그먼트 맹점을 흡수한다**: +atomics 실물은 active 세그먼트 0(`.rodata`가 passive라 선언부에 주소 없음 — 실제 배치는 start의 `i32.const 0x100000; memory.init` 코드에 베이크)·`__stack_pointer`=0x100000이라 [실측: 패널 덤프], 구 공식(앞 두 항만)은 0x100000을 산출해 SPK §3-3/§5-2 규범값 0x110000과 발산 — 위반 base가 검사 (iii)을 그린 통과하는 fail-open(O-12 침묵-오염 클래스, §5-2 함정의 shared 재개방)이었다. wasm-ld는 배치된 정적 이미지를 memory min에 반영하므로 min 항이 보수적 상계를 보장한다: 산출 = 비공유 0x100000(=16p), +atomics 0x110000(=17p [실측: `(memory 17 65536 shared)`]) — 양 프로파일 모두 SPK 규범값과 정확 일치. 필드: `heap_floor`, `shared`, `memory_min/max`, `exports`, `otter_version`, `rustc_version`(기록 전용 — §4.3), **`artifact_sha256`**(strip-후 원판 바이트 — §4.3 정본 결부). **생성 순서(패널 C-S2 정정 — v1은 "strip 전 추출" 단일 서술로 해시 대상이 미정이었다)**: ① `lib-manifest --extract`(strip **전** 산물: floor 성분·exports 추출 — name 섹션 필요, §8-6) → ② `wasm-tools strip` → ③ `lib-manifest --finalize`(strip-후 **최종 원판** 바이트의 SHA-256 수록·사이드카 확정). 체인이 소비하는 바이트(= 링크 입력 원판)와 해시 대상이 일치하고, 마커 주입은 링크 단계의 산물별 후속이며 주입기가 입력 해시를 대조한다(§2-1).
3. **b″ 검사 편입(SPK §5-2가 요구한 미봉합 지점의 봉합)**: 정적 검사에 **(iii) 라이브러리-하한 대조**를 신설한다 — `manifest.scratch_region.base ≥ 사이드카.heap_floor`. 이것이 SPK §5-2의 핵심 함정("data 세그먼트 기준 검사는 섀도 스택 겹침을 못 잡는다 — 스택은 세그먼트가 아니다")의 직접 해소다. 실물 b′ 기준 대조는 미실측이므로 L-4 게이트에서 실측화한다(§2-5-②와 동일 항목). **공식 자체의 회귀 방어선(패널 M1 처방)**: L-2 게이트에 "산출 사이드카의 heap_floor == 프로파일 기대 상수(비공유 0x100000 / +atomics 0x110000)" 단언을 둔다.

**파급 정정 3건**: (a) 스파이크 b″ 매니페스트의 `scratch_region=[0x10000,0x10008)`은 실물 b′ 합류 시 위반 — 재현 픽스처를 규범값으로 갱신. (b) `hosts/otter-aot-e2e`의 `SCRATCH_BASE`는 현재 page 1(=0x10000, main.rs:**54** [실측 — v1의 ":53"은 doc 주석 행이었다, 패널 자구 정정]) — 호스트-글루 스텁 상대로는 무해했으나 실물 b′ 결합 형상에는 위반이므로 L-4에서 0x110000으로 상향한다(+atomics floor와 **동일**(≥ 충족)·비공유 floor는 상회 — 양 프로파일 공용; 패널 G-N-5 자구 정정). (c) **`MEMORY_PAGES` 동반 상향(패널 G-SF-1/T-N5 — 파급 누락분)**: 현행 16 [실측: main.rs:56] — base만 올리면 §5.2-(ii) 커버리지가 min ≥ 18페이지를 요구해 전 페이즈가 결정적 적색이므로 L-4에서 **18 이상**으로 동반 상향(check-completeness 규율 사안 — 공사 1회로 마감).

**근거**: 수치의 자연 소유자는 scratch_base를 정하는 자(= compile_module 호출자)다 — 방출기는 base를 받기만 하고(O-14 §2.5), b″는 사후 검증자다. 사이드카를 정본으로 두면 라이브러리 확장(실할당자 도입 등, SPK §4-5) 시 규범 수치가 자동 추종된다. 검사 (iii)은 fail-closed: 사이드카 부재 시 게이트 실패(추정 진행 금지).

### §2-3. 커버리지 해소 — **후보 ① 채택(마커의 min-페이지 강제), 단 §2-1대로 구성적 이행 + 독립 검사 병치**

**결정**: SPK §5-3 후보 ①(마커 세그먼트가 min 페이지를 자동 상향 강제)을 채택하되, "자동 상향"의 주체를 주입기로 옮겨 [추정]을 소거한다(§2-1). 독립적으로 b″ 정적 검사 **(ii) 커버리지**를 유지한다: 결합 산물의 메모리 min 페이지 ≥ ⌈scratch_region.end/65536⌉. 위반의 실행 발현(첫 마셜링 store의 결정적 trap — SPK E8 [실측]: `memory fault at 0x10000`)은 §5.5 뮤테이션의 음성 대조로 남긴다.

**근거**: ①은 예약 증거와 커버리지 보장을 한 실물(마커+min)로 묶어 산발 권위가 없다. ②(링크 인자 상향)는 §2-1 기각 사유와 동일. ③(grow 의미론 재정의)은 O-14 §2.5-(ii)가 이미 "첫 호출 전 grow"를 허용하지만, **게이트 산물**에 대해서는 정적 min 충족을 요구하는 것이 결정적 게이트의 올바른 강화다 — grow-의존 배포는 게이트 밖 임베더 재량으로 존치(계약 위반 아님, 게이트 미통과일 뿐임을 명기).

### §2-4. otter_host 제공자 — **잔존-임포트 → 컴포넌트-수준 리프트가 정본(후보 b), 잔존 게이트는 매니페스트-스코프 정확-일치로 정밀화**

**결정**:
1. **잔존-임포트 게이트의 정식 문구**(host 예외 규정): 결합 산물의 잔존 코어 임포트 집합은 **`manifest.imports`의 `otter_host` 항목 집합과 정확히 일치(=, 이름·타입 전건)**해야 하며, 그 외(otter.memory·otter_helpers.* 포함)는 0이다. **⊆ 완화 기각(패널 G-SF-3 강화 — v1의 "정확히 일치하는 부분집합"은 자기모순 문구였다)**: wasm-merge 등록 제공자에 otter_host는 없으므로 host 임포트는 해소될 수 없다 — 잔존이 진부분집합이면 임포트 **소실**(이상 징후)의 침묵 통과라 fail-closed 원칙 위반이다. 매니페스트에 없는 잔존 1건도, 매니페스트에 있는데 소실된 1건도 적색(SPK §4-1의 침묵-부분-링크 방어 불변). host-프리 산물(deep류)은 공집합 일치가 자연 포섭하며 `--require-self-contained`는 강조 모드로 존치.
2. **코어-결합 배포 형상**: 잔존 `otter_host.*`는 인스턴스화 시점에 임베더가 해소한다(wasmtime Linker 호스트 함수 등록 — hosts/otter-aot-e2e의 현행 스텁 배선과 동형 [실측]). host fn의 정본 소재(임베더 제공물)와 층위가 정확히 일치한다.
3. **컴포넌트 형상 정본**: 잔존 `otter_host.*`를 **WIT world의 import로 선언·리프트**한다 — world에 `import h-add7: func(a: s64) -> s64` 류(kebab 개명 규칙 §6.1)를 추가하고 embed가 코어 임포트를 canonical ABI 하강으로 결부. Host1/2/HostCtx1/2 시그니처는 전부 i64/i32 스칼라(O-14 §2.2)라 하강이 타입-청결할 것으로 본다 — 엔트리 ABI 하강 `(i32,i32,i64)->i64` 정확 일치는 [실측, SPK L5]이고, **host 시그니처로의 확장은 동형 추론 [추정]**(패널 N-1[gates]/N3[conflicts] 라벨 정정 — v1의 "[실측: 동형 논증]"은 유추에 실측 라벨을 붙인 표기 흐림). **함수-임포트 리프트의 wasm-tools 실물 성립도 미실측**(스파이크 E10은 export 리프트만, E11은 memory 임포트 거부만 실증) — [추정]이며 L-5 게이트가 실측화한다. **→ [실측] 승격(L-5)**: host-리프트 성공 — otter_host 잔존 임포트가 `$root`+kebab 규약(§6.1 착지)으로 컴포넌트-수준 함수 임포트로 리프트되고, wasmtime 48 컴포넌트 Linker가 h-add7/hc-xor를 네이티브 배선해 오라클 비트-동일(AOT48-E2E-7-COMPONENT-CALC-OK) — [추정 ②] 해소, 폴백 미발동(§9-4 소멸). 실패 시 폴백(지금 확정): 컴포넌트 게이트 산물을 host-프리 형상으로 한정하고 host-포함 형상은 코어-결합 배포(위 2)로 서빙. **단, host-프리 컴포넌트 1종만으로 오너 상향 목표 ① (iv)~(vii)이 충족되는지는 본 문서가 자체 인증할 수 없다 — 오너 확인 항목으로 등재(§9-4)**(패널 C-S6: v1의 "게이트 충족은 폴백에서도 성립(§1.1-3이 요건이므로)"은 자기 문서 인용의 순환이었다; 오너 결정 B는 "프로그램-AOT로 충분"이지 이 폴백 형상까지 확언한 바 없다 — 귀속 정직).

**기각 — 후보 (a) 별도 제공자 wasm 모듈**: host fn 바디를 wasm으로 재구현해 wasm-merge에 등록하는 형태는 (i) 정본 이중화(네이티브 임베더 구현과 wasm 재구현의 두 손 — record==live 규율이 봉쇄하려는 바로 그 클래스의 host판), (ii) 층위 착오(SPK §5-4 자체 지적: "host 함수의 정본 소재는 임베더 제공물") — 스파이크 L4의 잔존-0 그린이 대역-host 덕분이었다는 정직 기록을 제품 형상으로 오인하지 않는다.

**'동적 코드 격리' 규범(§5b-B)과의 정합 + 엔트리 계약 정합(3중 정합의 완결 — 패널 C-S5)**: host fn은 **정적으로 선언된**(매니페스트 등재, 빌드-타임 확정) 임베더 능력이지 동적 코드가 아니다 — 리프트는 그것을 컴포넌트 경계(shared-nothing·타입드 스칼라·canonical ABI) **뒤로** 보내므로 격리를 약화시키기는커녕 강화한다. 반면 기각안 (a)는 임베더 코드를 산물 신뢰 경계 **안으로** 끌어들여 규범의 취지와 역행한다. 셋째 정합: 컴포넌트는 shared-nothing이고 world는 함수만 export(memory 비리프트)라 **O-14 §12.4의 argc>0 args-블록 마셜링 경로가 컴포넌트 형상에는 구조적으로 없다** — 컴포넌트 형상의 엔트리 계약은 argc=0 v1 정본 호출 또는 게스트-측 부트 함수(§12.4 대안) 한정임을 §6.1에 명문화한다(코어-결합 배포 형상은 §12.4 전문 그대로 적용).

### §2-5. 실물 b′×b″ 결합 재실행 4항의 증분 배치 — **전부 L-4(결합 체인 증분)의 게이트로 일괄 편입**

SPK §5-5의 4항은 전부 "기존 커맨드 재사용으로 충분"(SPK 명기)하므로 쪼개지 않고 결합 체인이 처음 서는 증분(L-4)의 게이트 목록에 축자 편입한다:

| # | 항목 | L-4 게이트 형태 |
|---|---|---|
| ① | 결합물 EH-프리(§4-12의 [추정]→[실측] 승격) | `wasm-tools validate --features=all,-exceptions,-legacy-exceptions` + WAT EH-census grep=0 — 결합물·컴포넌트 양 산물(b′ 레시피 게이트의 축자 재사용) |
| ② | 실물 b′ 섀도-스택·힙 심볼 × scratch_region 겹침 | §2-2 검사 (iii)의 첫 실측(사이드카 heap_floor=0x100000/0x110000 대조) + 음성 프로브(base=0x10000 위반판 적색) |
| ③ | +atomics b′ ↔ `--enable-threads` 결합 정합(O-14 §12.1 shared_memory 행) | shared 형상 전-체인(E9 재현을 실물 b′로) + 결합물 실행 오라클-동일. **확장(패널 M3/N-2 — E9 대역 스텁은 start·passive·TLS 전무 [실측]라 셋 다 미실측 [추정])**: wasm-merge의 `(start $__wasm_init_memory)`·passive `$.rodata`·`__tls_base` 병합 거동 + shared memory 대상 active 마커의 병합-후 보존·wasmtime 수용 실측. **→ [실측] 승격(L-4 — [추정 ③] 해소)**: start(init-전용 30 op)·passive `$.rodata`·shared-대상 마커는 병합 통과 보존, 단 `__tls_base`는 **wasm-merge가 미참조 전역으로 소거**(§0 착지 부기 1 — 과일반화 방지 정정) + `--enable-bulk-memory` census 신규(착지 부기 4) |
| ④ | 실물 b′ 요구 wasm-merge 피처 플래그 전수 | 플래그-제거 이진 탐색 1회 실행·기록(대역의 `--enable-nontrapping-float-to-int`가 실물 Rust convert 경로에도 필요한지 재확인 — SPK §4-10 명기 잔여) |

**근거**: 4항의 공통 전제가 "실물 b′ 산물 + 결합 실행 하네스"이고 이는 L-4에서 처음 동시에 존재한다. 더 이른 배치는 대역 의존을 재생산하고, 더 늦은 배치(L-5 이후)는 §4-12 [추정] 위에 컴포넌트화를 쌓는 순서 역전이다.

### §2-6. 이름공간 서술 기준 — **b″ 정적 기준(wasm-merge 등록명 + 기계 개명) 채택**

**결정**: SPK §5-6의 권고 그대로, 본 문서와 도구·레시피·게이트의 이름공간 서술은 전부 **정적 wasm-merge 기준**이다: 임포트 모듈-문자열 == 등록 인스턴스-이름, 산물 쪽 3개 모듈-문자열(`otter`/`otter_helpers`/`otter_host` 중 개명 대상은 앞 2개 — §2-4에 따라 `otter_host`는 잔존·리프트)을 매니페스트 `imports` 정본으로 단일 등록명 `otter_lib`로 **import 섹션만 바이트 재작성**(code 섹션 불변 — SPK §4-3 규율, 스파이크의 print|sed 텍스트 왕복은 실증용이었고 프로덕션 개명기는 wasmparser/wasm-encoder 바이트 재작성으로 자체 구현). b′의 "런타임 인스턴스화-그래프 배선에서 매핑 자명" 서술(SPK §3-1)은 참고 각주로만 존치 — 최종 체인이 wasm-merge인 이상 개명은 실제 의무다(단일 파일 3중 등록 기각 실증, SPK L2 `Export name conflict: memory` Fatal). 이로써 **O-14 §11-1 종결**: 방출기 문자열(`otter_helpers`/`otter_host`) 현행 유지 + b″ 개명 단계 — 방출기 무변경.

**개명-후 불변 게이트(모드별 명세 — 패널 C-N1 보강)**: (a) **import-모드**(§2-6 본문 대상): 개명 산물의 code·type·function·table·element·export 섹션 바이트가 원 산물과 동일함을 도구가 자체 단언한다(변경 허용 섹션 = import뿐). (b) **export-모드**(§6.1 kebab 개명 — memory/global/data를 가진 **결합물** 대상): 변경 허용 섹션 = export뿐 — code·type·function·table·element·import·memory·global·data 등 여타 전 섹션 바이트 불변 단언.

---

## §3. 도구 소유권 — b″는 어디 사는가

**결정: (a) 신설 CLI 크레이트 `hosts/otter-aot-link`(워크스페이스 제외) + (b) justfile 레시피가 조합 계층** — 양자택일이 아니라 역할 분담이다: 크레이트가 **로직**(개명·마커 주입·게이트·export 축소·사이드카 스키마·대조)을, 레시피가 **체인 조합과 외부 도구 호출**을 소유한다. 셸 스크립트 단독안(순수 (b))은 기각 — 바이트-수준 재작성과 fail-closed 검사를 sed/grep으로 구현하는 것은 스파이크 실증용까지만 정당하다(SPK §4-3).

### 3.1 크레이트 형상

- **소재**: `hosts/otter-aot-link`. 배치는 **선택**이다 — v1의 "hosts/의 실질 규율 = 워크스페이스-제외 소비자-체인 실물" 일반화는 1개 데이터점(otter-glue-js) 유도라 철회한다(패널 T-N3 — demos/otter-wasm-demo도 워크스페이스-제외 소비자-체인 실물이면서 demos/에 있다). 선택 근거(A-제품 소비자-체인의 짝 otter-aot-e2e와 동거·레시피/CI 규율 재사용)는 A-5 선례대로 **신설 크레이트 Cargo.toml/README에 축자 기록**한다(L-1a 스코프 의무). 신규 최상위 디렉터리는 금도금(레시피·CI 규율 재사용 손실)으로 기각.
- **워크스페이스 제외 + empty `[workspace]`**: otter-aot-e2e 선례 축자 — 단 이 크레이트의 사유는 트레인 충돌이 아니라(wasmtime 의존 자체가 없다 — L-1 단위 검증은 wasmparser Validator, 인스턴스화 단언은 L-4 이관으로 이 성질이 유지된다, 패널 MF-4 해소) **락파일 위생과 게이트 소재의 명시성**이다. **의존(패널 M2[tooling] 정정 — v1의 "…뿐"은 JSON 층 소유자를 누락한 자기모순이었다)**: `otter-wasmemit`(path — 매니페스트 타입 소비) + `wasmparser`/`wasm-encoder`(버전은 워크스페이스 핀 0.258 [실측: otter-wasmemit/Cargo.toml:20,28]과 **수동 락스텝** — Cargo.lock 커밋, `--locked` 빌드) + **`serde`/`serde_json`(사이드카·매니페스트 JSON — 자체 락파일에 격리, 워크스페이스 무파급)**. fmt/clippy/test는 레시피가 게이트한다(wasm-aot-e2e 레시피의 "workspace-excluded ⇒ gate here" 규율 축자 [실측: justfile:257]).
- **스키마 단일 정본(패널 M2[tooling]≡N4[conflicts] 해소)**: JSON 사이드카·매니페스트 사이드카의 타입 정의·직렬화·역직렬화는 otter-aot-link의 **lib 타깃 `schema` 모듈이 단독 소유**하고, 생산 측(otter-aot-e2e 등 compile_module 호출자)은 이 크레이트를 path-dep로 소비해 동일 타입으로 직렬화한다 — 생산/소비가 스키마를 독립 수제 구현하면 record==live가 봉쇄하는 이중-정본 클래스가 스키마 층에서 재생산되므로 금지. 기각 대안: otter-wasmemit에 serde derive(feature 게이트) — 워크스페이스 멤버 의존 추가로 루트 Cargo.lock + demos/otter-wasm-demo 락파일 파급(O-14 v2.3-⑤가 기록한 바로 그 클래스) + 본 증분의 otter-wasmemit 변경 한계("AotOpts doc 1줄, additive") 상충; 수제 JSON — §3 서두의 sed/grep 기각과 동일 클래스(fail-closed 게이트 도구에서의 과잉 자체-구현).
- **서브커맨드**: `rename`(§2-6/§6.1 — 일반 매핑-테이블 사상, import/export 양 모드) · `inject-marker`(§2-1 — 폭-분기 3형상 + 입력 SHA 대조) · `check`(§5.2 정적 검사 전부 — 잔존-임포트 정확-일치·마커·커버리지·하한 대조·EH-census·start-검사·개명-후 불변) · `shrink-exports`(§5.4) · `lib-manifest`(b′ 사이드카 — `--extract`/`--finalize` 2단, §2-2). 전부 결정적·무네트워크.
- **매니페스트 입력 = JSON 사이드카**: compile_module 호출 측(e2e 등)이 `AotManifest`를 `schema` 모듈로 JSON 직렬화해 산물 곁에 둔다 — **O-14 §11-2 종결**: SPK §3-2 실측(소비 필드 전수 = imports/scratch_region/shared_memory/entry_slot/table_size)대로 JSON 한 층 채택. 세대-대조는 §4.3(콘텐츠 해시 정본).

### 3.2 외부 도구 조달 — binaryen/wasm-tools

- **자체 구현 우선 원칙**: 개명·마커·게이트·export 축소는 wasmparser/wasm-encoder로 자체 구현한다(위 서브커맨드). 외부 의존은 **wasm-merge(binaryen) 유일 필수** — **자체 결합기 기각의 논거(패널 T-N1 귀속 정정 — v1의 "rust 크레이트 대체물이 없다(SPK L1)"는 과잉 귀속: SPK L1 실측은 CLI 4종(wasm-ld·component link·compose/wac) 불성립이지 크레이트 공간 전수 조사가 아니다)**: 정적 결합은 함수/타입 인덱스 재번호로 code 섹션 전면 재작성을 요구하므로, 자체 결합기는 §2-6/§4-3의 code-불변 규율과 §5.3 검증 부담을 스스로 파괴한다. 컴포넌트화(`component embed`/`new`)는 wasm-tools **CLI**를 쓴다 — wit-component 크레이트 in-process 통합은 의존 그래프 비대 + 자체 구현 이득 0으로 v1 기각(재심 트리거: CLI 핀 조달이 CI에서 반복 마찰을 일으킬 때). 컴포넌트 **검사**(§6.2)만은 wasmparser `component-model` feature로 네이티브 수행(자체 락파일 격리, 동일 0.258 핀 — 패널 T-S4).
- **조달 형태**: **버전-핀 릴리스 바이너리 다운로드**(vendored 소스 기각 — binaryen은 C++ 빌드 세금; 시스템 패키지 기각 — 버전 부동). 신설 레시피 `just wasm-aot-tools`: binaryen `version_132`·wasm-tools `1.246.2` 릴리스 타르볼을 **SHA-256 핀**과 함께 스크래치-외 캐시 디렉터리에 멱등 다운로드·검증·전개. 오프라인 가드(캐시 히트 시 무네트워크) 포함 — `wasm-toolchain` 레시피 선례 [실측: justfile 154-160]. **출처 전환 부기(패널 T-N2)**: 스파이크의 1.246.2는 배포판 **시스템 바이너리**였고 [실측: commands.log "system"] 핀 대상은 GitHub 릴리스 빌드다 — 동일 버전이라 행동 동일이 기대되고 L-4 전-체인이 핀 빌드로 재검증되므로 실질 위험은 낮다. 시스템 wasm-tools가 PATH에 실존하는 환경의 침묵 대체를 막기 위해 레시피·체인은 도구를 **캐시 경로 명시 호출(비-PATH)**하고, 전개 직후 `--version` 단언을 포함한다.
- **초기 핀 확립 절차(TOFU — 패널 T-S1 신설)**: 최초 SHA-256 확립은 ① 독립 2-경로 다운로드의 해시 일치 + ② 업스트림 공표 체크섬 존재 시 대조 + ③ 확립 근거(출처 URL·확립 일시)의 본 절 기록으로 한다 — 첫 다운로드 오염 시 핀이 오염의 고정 장치가 되는 것의 차단(O-13 §7.1 선례는 rustup 채널이라 자체 검증 체계가 있었고, GitHub 릴리스 바이너리는 다르다). 교차 앵커로 스파이크 보존 타르볼 실물을 쓸 수 있다 [실측: o16c-bdblprime/binaryen.tar.gz sha256 = 195ddc94f9bc…]. **핀은 x86_64-linux 타르볼 한정(패널 T-N4)** — 타 플랫폼 기여자는 로컬 재확립(불일치는 fail-closed지만 에러 메시지에 원인·본 절 참조를 명기).

  **확립 기록 (TOFU ③의 이행 — L-3 착지, 2026-09-04)**:
  - `binaryen version_132` (x86_64-linux 타르볼, github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-linux.tar.gz):
    `195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572` — **3-앵커**(스파이크 보존 타르볼 ·
    신규 다운로드 · 업스트림 공표 `.sha256` 자산) 일치 + L-3 리뷰의 시점-분리 4번째 다운로드 재확인.
  - `wasm-tools 1.246.2` (x86_64-linux 타르볼, github.com/bytecodealliance/wasm-tools/releases/download/v1.246.2/):
    `f0df9428792225322e9b4344ca581f03b1740d509d6bdad33d14a810009be9ec` — **앵커 약함 정직 기록**: 업스트림은
    이 릴리스에 체크섬 자산을 공표하지 않고(자산 전수 확인), 스파이크는 시스템 바이너리를 썼으므로 타르볼
    교차 앵커가 없다; 2-엔드포인트(browser_download_url · API 자산 엔드포인트)는 동일 스토리지 배후라 완전
    독립이 아니다. 보완: 핀 빌드 `--version`(`ee323ec1d 2026-04-03`)의 버전 일치 + L-4 전-체인 재검증 의무.
- **핀 규율(O-13 §7.1 선례 축자 적용)**: 값의 정본은 justfile 변수(`binaryen-version`·`wasm-tools-version`·SHA 2종), 절차의 정본은 본 절. 범프 절차: ① 새 버전으로 `just wasm-aot-tools` ② 전-체인 재실행 — §5 게이트 전부 + **§5.6 뮤테이션 전건 재실행 포함**(패널 T-S4 명시화 — 범프가 검출기를 공허화하는 클래스의 차단; v1 문구는 1회 레저인지 범프마다인지 모호했다) ③ **wasm-merge 재인코딩 드리프트 검사** — 결합물의 의미-수준 검증 + 실행-차등 그린(바이트 대조는 원리적 불가, SPK §4-4) ④ 소재지 락스텝 갱신 + 본 절에 기록. 주기: O-13 §7.1과 동일하게 분기 1회 재평가에 편입. CI: `wasm-eh-pinned` 레인에 조달 스텝 추가(해시 키 캐시).

---

## §4. b′ 제품화 — 엔진-라이브러리 크레이트의 레포 편입

### 4.1 소재·형태

**결정**: 신설 워크스페이스-제외 크레이트 `hosts/otter-helpers-wasm` — 스파이크 래퍼(스크래치 `o16c-bprime/otter-helpers-wasm/` [실측])의 승격, 소스 실질 무변경(path-dep만 상대경로화).

- **crates/ 기각**: 워크스페이스 멤버는 루트 프로파일을 공유하는데 b′는 `[profile.release] panic="abort"` 전용 프로파일이 필수(SPK 레시피) — 프로파일은 루트 소유라 멤버로는 표현 불가. 또한 wasm32-wasip1 전용 산물이 `--workspace` 빌드에 편승할 이유가 없다(demos/otter-wasm-demo 제외 선례와 동일 논거 [실측: otter-aot-e2e Cargo.toml `[workspace]` 주석]).
- **hosts/ 선택 근거**: §3.1과 동일(선택 + Cargo.toml/README 축자 기록 의무 — L-2 스코프). b′는 A-제품 배포 체인의 게스트-측 절반이고, 그 짝(otter-aot-link·otter-aot-e2e)과 같은 디렉터리 규율 아래 두는 것이 트레인·레시피 규율의 재사용을 극대화한다. demos/ 기각: 데모가 아닌 제품 구성물.
- **크레이트 내용**(스파이크 축자 [실측]): `#![no_std]`, thin 위임 3종(`#[unsafe(no_mangle)] extern "C"` — O-14 §12.3 b′ export 의무의 이행), panic_handler→`wasm32::unreachable`, **trap-알로케이터**(SPK §4-5 결정 승계 — 무할당 증명 겸용; 실할당자 도입은 라이브러리 확장 시 재심으로 존치), crate-type `["cdylib"]`(staticlib은 제거 — b″ 입력 형상이 cdylib 완성 모듈로 확정됐으므로(SPK §4-11) .a 산출은 죽은 표면).

### 4.2 빌드 레시피 `just wasm-bprime`

```
cargo build --locked --release --target wasm32-wasip1      # 레포 핀 stable — rust-toolchain.toml 상속(평 cargo)
otter-aot-link lib-manifest --extract <unstripped>          # 사이드카 1단(§2-2): floor 성분·exports — strip 전
wasm-tools strip                                            # (비-PATH 핀 도구 — §3.2)
otter-aot-link lib-manifest --finalize <stripped>           # 사이드카 확정: 최종 원판 SHA-256 수록
wasm-tools validate --features=all,-exceptions,-legacy-exceptions   # EH-프리 게이트(축자)
```

- **락파일 규율(패널 M1[tooling] — must-fix)**: Cargo.lock 커밋 + 전 빌드 `--locked` + 워크스페이스 핀과의 수동 락스텝(§3.1 문구 축자). b′는 이 증분의 게이트 산출물 그 자체라 의존 비고정 해석은 산물 바이트의 침묵 변동 = 사이드카 SHA 결부·L-2 재측정 기록 전부 무효화다 — O-14 v2.3-⑤(제외-크레이트 락파일 표류의 `--locked` 잠복 적색)가 기록한 실패 클래스의 차단. 스파이크 크레이트에 Cargo.lock 실존 [실측] — 승격 비용 1줄.
- **평 `cargo`(패널 T-S2)**: 스테이블 핀의 정본은 rust-toolchain.toml 1곳 [실측: channel="1.96.0"] — v1의 `cargo +1.96.0` 하드코드는 제2 소재지 신설(O-13 §7.1 "값의 정본 1곳" 위반; O-13 W-4의 명시-핀 선례는 nightly 전용 논거)이라 기각하고 otter-aot-e2e 선례를 축자 상속한다 [실측: Cargo.toml 주석 "Builds on the repo's pinned stable"].
- **+atomics 프로파일 포함**: 포함한다 — 별도 target-dir·스파이크 축자 RUSTFLAGS(`+atomics,+bulk-memory` + `--shared-memory` + `--max-memory=4294967296`). 근거: O-14 §12.1 `shared_memory` 행의 정합 의무(sharedness 정확 일치)가 게이트 형상(AOT48-E2E-2/-4가 shared 임포트 산물 [실측])에 이미 물려 있어, shared 결합 체인은 +atomics b′ 없이는 성립하지 않는다.
- **O-W 워치리스트 등재(SPK §4-7의 이행)**: "+atomics stable에서 'not stably supported' **경고** — 보증 없는 통과" 건을 MODE §1.4 O-W 분기 검토 목록에 등재한다: 감시 지점 = 새 stable에서 경고→에러 전환 여부(분기 핀-범프 시 b′ 빌드 재실행이 자동 감지), 발화 시 효과 = shared 프로파일만 nightly/build-std 재논의(비공유 게이트 경로 무영향 — SPK 축자).
- **마커 주입의 산물별-단계화(패널 C-S7 정정 — v1의 "게이트 표준 region을 주입한 게이트판" 단수 서술은 성립 불능)**: region end는 산물별이고(calc 0x…08 vs deep 0x…18 [실측]) 검사 (i)이 마커의 정확 위치를 요구하므로, 주입된 게이트판 b′는 프로그램당 1개일 수밖에 없다. 따라서 본 레시피는 **미주입 원판 + 사이드카만** 산출하고, `inject-marker`는 링크 레시피(L-4)의 **산물별 단계**다(region은 각 산물의 매니페스트 유래). L-2의 형상 재측정(§2-1)은 대표 region을 주입한 **테스트 산물**로 수행한다(검증용 — 배포물 아님). 임베더 문서에 "원판 사용 시 inject-marker + check가 의무"를 명기(O-14 §12.2 (i)(ii)의 도구화된 이행).
- **B5 재서술(프로파일 분리 — §2-1)**: 비공유 = "자체 데이터 0·start 0(+주입 마커)", +atomics = "passive `$.rodata`·init-start·`__tls_base`(+주입 마커)" — L-2에서 프로파일별 재측정·기록.

### 4.3 사이드카 대조 — **정본 = 콘텐츠 해시** (패널 C-S1/T-S3 정정 — v1의 버전-대조 정본화는 공허했다)

- **정본 메커니즘**: 사이드카 `artifact_sha256`(strip-후 원판 바이트 — §2-2 생성 순서) — `inject-marker`가 주입 전 입력을 대조하고, `check`가 사이드카-산물 짝을 대조한다. 불일치는 실패(fail-closed; 우회 플래그 없음). v1이 §8-5 완화책으로 두었던 해시를 정본으로 승격한 것이다.
- **버전 대조의 무변별성 정직 명기**: `emitter_version` × 사이드카 `otter_version` 대조는 현행 전-레포 버전이 `0.0.0`이라 [실측: 루트 Cargo.toml:8, 스파이크 매니페스트 `emitter_version=0.0.0`] 상수끼리 비교 — 어떤 세대 표류(낡은 b′ × 새 방출기)도 잡지 못한다(버저닝은 otter-reusability-plan의 미해결 블로커). 기록·대조 코드는 유지하되, **버전 체계 도입 전까지 이 검사의 변별력은 0**이며 fail-closed 서사는 해시가 담당함을 명기한다. `rustc_version`은 **기록 전용**(산물 매니페스트에 대응 필드가 없어 대조 술어 자체가 미정의 — 패널 정정, 대조 목록에서 제외).
- 전체 빌드-해시 규율(방출물 포함)은 §9-3 유보 불변.

---

## §5. 게이트 설계

게이트는 전부 **b″ 자체 몫**이다 — wasm-merge는 침묵 부분-링크(exit 0)이므로 어떤 검사도 도구에 위임할 수 없다(SPK §4-1, 최중요 의무 조항 승계).

### 5.1 잔존-임포트 게이트 (`check --imports`)

§2-4 확정 문구: 잔존 코어 임포트 집합 **== manifest.imports의 `otter_host` 집합**(이름·타입 전건 일치 — ⊆ 아님, 패널 G-SF-3 강화), 그 외 0. host-프리 산물엔 `--require-self-contained`(공집합 일치의 강조 모드). 음성 대조는 스파이크 E7 재현(제공자 1개 누락 → 잔존 검출 적색) [실측 선례] + **임포트-소실 음성**(진부분집합 형상 → 적색 — 강화분의 비공허성).

### 5.2 scratch 정적 검사 3종 (`check --scratch`)

(i) **마커 대조(폭-분기 — §2-1)**: 메모리-소유 모듈(결합 전 b′ 게이트판 / 결합 후 산물)의 data 세그먼트에 대해 — W≥16: 매직-일치 마커가 정확히 base·end−8에 존재; W==8: 병합 마커 `OTTRSCRM`이 정확히 `[base,end)`에 존재; W==0: 마커 부재 + 빈-region 명시 분기(로그 기록). 전 분기 공통: **마커 외 세그먼트 ∩ region = ∅**. (ii) **커버리지**: memory min ≥ ⌈end/65536⌉(§2-3; W==0은 자명 통과). (iii) **라이브러리-하한 대조**: base ≥ 사이드카 heap_floor(§2-2 — 섀도-스택 겹침의 유일한 정적 방어선임을 주석 명기; 공식은 memory_min 항 포함 — 패널 M1). check 실패 시 공인 복구 경로 = **재방출**(새 base로 compile_module 재호출, O-14 §2.5-iv — SPK §3-3 서술과의 봉합, 패널 G-N-4).

### 5.3 링크-후 의미-수준 검증 + 실행-차등

바이트 게이트는 원리적 불가(재인코딩 — SPK §4-4)이므로 링크-후는 2층이다:
- **의미-수준(정적, `check --linked`)**: 단일 memory 정의(라이브러리 소유 형상), `otter_entry`/`otter_table` export 존재, 테이블 min=max=manifest.table_size, active element 슬롯 항등(element 항 수·offset 0), EH-census 0, (컴포넌트) component export 비공허(§6.2), **start 검사(패널 M3/MF-1 신설 — v1 목록엔 start 항목 자체가 없었다)**: start 부재이거나, **shared-프로파일 결합물에서 init-전용 start만 허용** — 바디 명령이 화이트리스트{제어(block/loop/br/br_if/end)·drop·i32.const·global.get/set·memory.init/data.drop·원자 연산(rmw/cmpxchg/store/load/wait/notify)} 내이고 call/call_indirect/return_call류 0(wasm-ld `__wasm_init_memory` 형상의 기계 판별 [실측: +atomics 실물 start = 0x100000 memory.init + data.drop + 원자 초기화 가드]). O-13 "인스턴스화는 게스트 코드 미실행" 규범과의 관계: 그 규범은 **pre-link 산물 전용**(O-14 §2.1 — 산물 start 없음, 불변)이며, 결합물의 init-start는 게스트 로직이 아닌 툴체인의 메모리-이미지 초기화다 — 화이트리스트가 "게스트 로직 아님"을 기계 강제한다.
- **실행-차등(오라클)**: 결합물·컴포넌트 각각 T0 오라클과 비트-동일 — 정본 실행 게이트. wasm-merge의 의미 보존을 실측으로 담보하는 유일한 층이므로 binaryen 범프 절차(§3.2)에 필수 편입. **코퍼스 보강(패널 G-N-3)**: L-2의 40,000케이스 헬퍼 차등을 **결합물 경유로 1회 재실행**(L-4 — 3형상+2M 대비 저비용 확장).

### 5.4 export 축소 (`shrink-exports`)

결합 산물 export 합집합(**대역-host 형상에서 8종 — SPK §4-8의 수치는 그 형상 귀속**(패널 자구 정정); 실물 체인의 잔존 집합은 산물·형상별이며 L-4에서 실측 기록)을 O-14 §2.4 표면(`otter_entry`+`otter_table`) + memory(임베더 마셜링 필요 — O-14 §12.4 args-블록 의무의 전제)로 축소. 헬퍼·호스트 export 제거가 기본. 컴포넌트 형상은 embed가 world 표면만 리프트하므로 축소가 사실상 자동이나, 코어-결합 배포 형상엔 명시 단계다. 축소 후 실행-차등 재실행(축소가 의미를 깨지 않음을 단언).

### 5.5 E2E — **기존 otter-aot-e2e 확장 (신설 기각)**

- **근거**: 동일 호스트-트레인(48)·동일 참조-호스트 Config·동일 마커-grep 규율·"산물은 런타임 compile_module 생성"이라는 단일-lowering 정본 규율까지 전부 재사용된다. 신설 크레이트는 같은 의존 집합의 복제만 낳는다. 필요한 델타: wasmtime `component-model` feature 추가(스파이크 aotgen과 동일 feature set [실측: SPK 도구 버전 행]) + 링크·컴포넌트 산물을 읽는 실행 모드 + otter-aot-link path-dep(`schema` 모듈 소비 — §3.1).
- **형상**: 신규 마커 `AOT48-E2E-6-LINKED-OK`(결합물: calc-host형·deep-selfcontained형·shared형 3형상 오라클-동일 + §2-5 ①~④) / `AOT48-E2E-7-COMPONENT-OK`(컴포넌트: 엔트리 리프트 실행 + host-리프트 실측 §2-4-3 + deep 2M 완주 재확인). 체인 조합(빌드→개명→merge→산물별 주입→check→shrink→embed/new)은 신설 레시피 `just wasm-aot-link-e2e`가 오케스트레이션(e2e 바이너리는 실행·차등만 — 외부 도구 호출을 크레이트에 넣지 않는다).
- **EH-프리 전-체인 게이트(SPK §4-12 해소)**: 위 레시피가 결합물·컴포넌트 양 산물에 §4.2 validate 게이트를 무조건 실행 — 그린 시 "§7 nightly 탈출"이 최종-산물 단위 [실측]으로 승격되고, O-14 §3.3의 관련 각주를 갱신한다(L-6). 이 게이트의 거부 방향(비공허성)은 §5.6-⑧이 실증한다(패널 MF-3).

### 5.6 뮤테이션 후보 (게이트가 실제로 무는지의 증거 — L-6에서 실행·레저 기록)

① 제공자 헬퍼 1종 export 제거 → 5.1 적색(E7 재현). ② 마커 세그먼트 제거/8바이트 이동 → 5.2-(i) 적색. ③ 메모리 min 하향(언더사이즈) → 5.2-(ii) 적색 + 실행 trap 음성 대조(E8 재현). ④ scratch_base=0x10000 위반판 → 5.2-(iii) 적색. ⑤ 개명기가 code 섹션 1바이트 변조 → §2-6 개명-후 불변 게이트 적색. ⑥ kebab 개명 생략 → embed 실패/빈-world 검출(§6.2) 적색(E10 함정 재현). ⑦ shrink가 `otter_table`까지 제거 → 5.3 의미-수준 적색. **⑧ EH 명령 주입 픽스처(try_table 1개 삽입한 결합물) → §4.2/§5.5 validate 적색 — EH-프리 게이트의 거부 방향 첫 실측(패널 MF-3: 통과 실측만 존재했다).** **⑨ 사이드카 위조(artifact_sha256 1바이트 변조 / 짝 잘못된 사이드카) → inject-marker·check 적색 — §4.3 정본 메커니즘의 음성 실증(패널 MF-3).** **⑩ 마커 외 data 세그먼트를 region 내로 주입 → 5.2-(i)의 ∩=∅ 부검사 적색(패널 G-SF-7 — ②로는 이 부검사가 물리지 않는다).**

---

## §6. 컴포넌트화 규범

### 6.1 WIT world 정본 + kebab-case 개명 규칙

- world 정본은 레포 파일 `hosts/otter-aot-link/wit/aot-artifact.wit`(스파이크 wit/world.wit 승격 [실측]): `package otter:aot; world aot-artifact { export otter-entry: func(ctx: u32, args: u32, argc: u64) -> s64; }` — canonical ABI 하강 = 정확히 `(i32,i32,i64)->i64`(SPK L5 [실측]). §2-4 채택 시 host import 선언이 가산된다.
- **컴포넌트-형상 엔트리 계약(패널 C-S5 명문화)**: world는 함수만 export하고 memory는 리프트되지 않으며 컴포넌트 경계는 shared-nothing — 임베더가 args-블록을 게스트 메모리에 마셜링할 경로가 구조적으로 없다. 따라서 컴포넌트 형상의 엔트리 계약은 **argc=0 v1 정본 호출(O-14 §12.4) 또는 게스트-측 부트 함수(§12.4 대안 — 인자 즉값 베이크) 한정**이다. argc>0 args-블록 의무(§12.4)는 코어-결합 배포 형상에만 적용된다.
- **개명 규칙(기계 규범 + 기계 강제 — 패널 G-SF-6/C-N2)**: 코어 export/import 이름의 snake_case → kebab-case 치환(`_`→`-`)만 허용 — `otter_entry`→`otter-entry`, `h_add7`→`h-add7`, `hc_xor`→`hc-xor`. 역변환이 유일하도록 이름에 `-` 원천 금지(방출기·리졸버 명명 규칙에 부기). **`rename`·`check`는 다음을 fail-closed로 거부한다**(부기가 아닌 도구 강제 — §2-1이 배격한 "규범 문서 신뢰 순환"의 재발 방지): 원천명에 `-` 포함, 치환-후 이름 충돌, 치환 결과가 유효 WIT 식별자 아님(대문자·비ASCII·선행 숫자 등). 개명은 `rename` 서브커맨드의 export-모드로 수행(import-개명과 동일 바이트-재작성 규율, 불변 게이트는 §2-6-(b)).
- **host-리프트의 코어 임포트 모듈-문자열 사상은 미규정·미실측 [추정]**(패널 G-SF-2 — E10은 export 리프트만 실증): wasm-tools의 world-수준 함수-임포트 결부가 특정 코어 모듈-문자열 규약을 요구할 개연이 높고, 그 경우 재작성은 **rename 도구의 요구사항**이 된다. 선제 흡수: `rename`은 특정 문자열 하드코드가 아니라 **일반 매핑-테이블 사상**(임의 모듈-문자열 쌍, import/export 양 모드)으로 명세한다(L-1a) — L-5에서 어떤 규약이 판명돼도 L-1 재작업 0. L-5 실측 항목으로 등재. **→ [실측] 승격(L-5 — [추정 ⑤] 해소)**: 규약 = **`$root` 모듈-문자열 + kebab 필드명**(`otter_host.h_add7` → `$root.h-add7`; 핀 wasm-tools 1.246.2 실측 — 미개명 이름공간은 component new가 시끄럽게 거부). 일반-사상 베팅 적중, rename 재작업 0.

### 6.2 '빈 world' 함정 방지

`component new` 직행 금지 — 코어 export는 자동 승격되지 않는다(SPK L5/§4-9 [실측]). 규범: 컴포넌트화는 항상 `embed --world` 경유 + `check`가 산출 컴포넌트의 **export 비공허 + world 일치**를 기계 단언. **구현(패널 T-S4 정정)**: wasmparser **`component-model` feature 네이티브 파싱**(otter-aot-link 자체 락파일 격리, 동일 0.258 핀)으로 component export 섹션을 직접 검사 — v1의 `wasm-tools component wit` 텍스트 덤프 대조는 사람-대상 CLI 출력이라 핀 범프에서 포맷이 움직이면 grep류 대조가 공허-통과로 약화되는, 레포가 뮤테이션으로 봉쇄해 온 클래스라 기각. 미링크 산물의 직접 컴포넌트화도 금지(E11 [실측]: bare 코어 임포트 해소 실패) — 체인 순서 자체를 레시피가 강제한다.

### 6.3 격리 규범 (오너 §5b-B 인용의 명문 반영)

오너 원문(MODE §5b-B): "동적 코드 실행은 어떤 식으로든 격리된 환경을 통해서만, 극히 제한적으로 지원." 본 증분의 이행: (1) O-16c′ 산물(결합물·컴포넌트)에는 동적 코드 경로가 원리적으로 없다 — `otter_jit.compile` 임포트 0·코드 전량 빌드-타임 확정(O-14 §7-1 승계). **start는 프로파일 분리(패널 M3/MF-1 정정 — v1의 "start 0" 단일 서술은 shared 결합물에서 거짓이었고, §5.3 목록에 start 검사도 없어 "기계 확인" 주장이 허위였다)**: 비공유 산물·결합물은 start 0, shared 결합물은 **init-전용 start만**(§5.3 화이트리스트 검사가 "게스트 로직 아님·코드 실행 경로 아님"을 기계 확인) — §5.1 잔존-임포트 게이트와 §5.3 의미-검증의 합으로 동적-코드-부재 논증이 양 프로파일에서 성립한다. (2) 컴포넌트/배포 맥락에서 동적 코드가 필요해지는 미래 지점은 격리 경계(컴포넌트 자체 샌드박스 안의 T0 인터프리터 실행, 또는 호스트 격리 사이드채널) 경유 + 극히 제한 스코프로만 — 그 설계는 O-16 소관이고 본 파이프라인에 그 훅을 **만들지 않는다**(자유형 런타임 코드젠을 CM 맥락에 심는 방향 금지 — §5b-B 운영화 문구 축자). (3) §2-4의 host-리프트 정합 논증이 이 규범의 적용 사례다.

---

## §7. 증분 분할 (각 1커밋 — 설계-패널-구현-리뷰 워크플로 단위; L-1은 패널 G-SF-5로 2분할)

| 증분 | 스코프 | 게이트 |
|---|---|---|
| **L-1a — DONE(5b2ea5e)** otter-aot-link 골격 + 개명·임포트 게이트 | `hosts/otter-aot-link` 신설(lib+bin): `rename`(일반 매핑-테이블 사상, import/export 양 모드 + kebab fail-closed 거부 3종 §6.1) · `check --imports`(정확 일치 §5.1) · 개명-후 불변(모드별 허용-섹션 §2-6) + 배치 근거 Cargo.toml/README 축자 기록(§3.1) | 손 wasm 픽스처 단위 테스트: 불변 게이트 양 모드 · 잔존-임포트 = (소실/초과/타입-불일치 음성 포함) · kebab 거부 3종 · 산물 wasmparser Validator 그린(인스턴스화는 L-4 이관 — §0-5) + fmt/clippy 레시피 게이트 |
| **L-1b — DONE(5b2ea5e)** 마커·사이드카·스크래치 검사 | `inject-marker`(폭-분기 3형상 + min 상향 + 입력 SHA 대조 §2-1) · `check --scratch`(§5.2 3종) · `lib-manifest`(--extract/--finalize §2-2) · `schema` 모듈(serde 격리 — ser/de 단독 소유 §3.1) | `check` **전 검사 항목별 양성·음성 각 ≥1**(패널 G-SF-7 — v1 "검사 3종 각 1"은 과소) + 겹침-세그먼트 주입 음성 + W∈{0,8,≥16} 3형상 픽스처 + shared-memory 대상 active 마커 픽스처(wasmparser 수준 — [추정] 1단 실측화, 엔진 수용은 L-4) + schema 왕복(ser→de 동등) |
| **L-2 — DONE(fbb1c2a)** b′ 제품화 | `hosts/otter-helpers-wasm` 신설 + `just wasm-bprime`(비공유·+atomics; `--locked`+락파일 커밋; 평 cargo; 사이드카 2단; EH-validate — §4.2) + 형상 재측정 **프로파일-분리** 기록(§2-1) + O-W 등재(§4.2) + 배치 근거 축자 기록 | 스파이크 B2–B5 재현(차등 40,000케이스 0 불일치 포함 — 러너 로직은 e2e 크레이트에 흡수) + EH-census 0 + **사이드카 heap_floor == 프로파일 기대 상수(0x100000/0x110000)**(§2-2 — 패널 M1 방어선) + 대표-region 주입 테스트 산물 재측정 |
| **L-3 — DONE(5b2ea5e)** 외부 도구 조달 | `just wasm-aot-tools`(binaryen 132·wasm-tools 1.246.2, SHA-256 핀 + **TOFU 확립 절차·기록** + 비-PATH 명시 호출·`--version` 단언 + x86_64-linux 한정 명기 — §3.2) + CI 조달 스텝 + §3.2 범프 절차 명문 | 조달 멱등성(2회 실행 무변경) + 해시 불일치 시 실패 실증 + 2-경로 해시 일치·확립 근거 기록 |
| **L-4 — DONE(ca2e06c)** 결합 체인 | `just wasm-aot-link-e2e` 전반부(빌드→개명→merge→**산물별 inject-marker**→check→shrink) + `shrink-exports`·`check --linked` 구현(**L-1에서 이동** — 소비자가 이때 처음 존재, 패널 G-SF-5) + §2-5 재실행 4항(③ 확장분 포함) + e2e SCRATCH_BASE 0x110000·**MEMORY_PAGES 18** 상향 + AotOpts doc 부기(§2-2) + **주입 산물 wasmtime 인스턴스화 단언(L-1 이관분 — §0-5)** | AOT48-E2E-6(3형상 오라클-동일) + EH-프리 결합물 + 검사 (iii)·start-검사 첫 실측 + 피처 플래그 전수 기록 + 40k 차등 결합물 경유 1회(§5.3) |
| **L-5 — DONE(6a22004)** 컴포넌트화 | wit/ 정본 + kebab 개명 + embed/new 체인 + host-리프트 실측(§2-4-3 [추정] 해소 — 실패 시 명기된 폴백 + §9-4 오너 확인 상신) + **host 모듈-문자열 규약 실측**(§6.1 [추정]) + §6.2 네이티브 검사(component-model) + e2e `component-model` feature·실행 모드 | AOT48-E2E-7(컴포넌트 오라클-동일 + deep 2M) + 빈-world 검출(§6.2) + 컴포넌트 EH-프리 |
| **L-6 — DONE(본 증분, §10)** 게이트 경화·마감 | 뮤테이션 ①~⑩ 실행·영구 레저(§5.6) + CI `wasm-eh-pinned` 레인 편입 + **ci.yml 캐시 목록 갱신**(5번째 제외-워크스페이스 — 패널 T-N3 [실측: ci.yml 캐시 주석 규율]) + O-14 §3.3/§11 상태 갱신 + SPK·본 문서 상태 갱신 + 적대적 리뷰(2렌즈) | 뮤테이션 전건 적색 실증 + 전 게이트 무회귀(기존 wasm-aot-e2e 포함) + 리뷰 통과 |

순서 제약: L-1a → L-1b → L-2(주입기·사이드카 의존) → L-4(양 실물 의존) → L-5 → L-6. **L-3 → L-2**(§4.2가 wasm-tools strip/validate를 호출 — v1의 "L-2 ∥ L-3"은 L-2를 미핀 시스템 도구로 돌리는 핀 규율 위반이라 철회, 패널 C-S3/G-SF-4). 병렬 여지: L-3 ∥ L-1a/L-1b.

---

## §8. 위험

1. **wasm-merge 재인코딩 드리프트**: binaryen 범프가 의미-보존을 침묵으로 깨면 바이트 게이트가 없어(SPK §4-4) 실행-차등만이 방어선 — 완화: 범프 절차에 전-체인+뮤테이션 재실행 의무(§3.2), 오라클 코퍼스를 e2e 3형상+2M+결합물-경유 40k(§5.3)로 유지.
2. **host-리프트 [추정] 실패**(§2-4-3) + **모듈-문자열 규약 미지**(§6.1): wasm-tools의 함수-임포트 하강 결부가 기대와 다를 수 있음 — 완화: 폴백 지금 확정(host-프리 컴포넌트 + 코어-결합 host 형상, 게이트-충족성은 §9-4 오너 확인), rename 일반-사상화로 규약 판명 시 도구 재작업 0, L-5 게이트가 조기 판별.
3. **+atomics 경고→에러 전환**(SPK §4-7): shared 게이트 경로 좌초 — 완화: O-W 등재(§4.2) + 비공유 경로 무영향 구조 유지.
4. **외부 도구 이중 핀 좌초**: binaryen·wasm-tools 릴리스 아카이브 소실 — 완화: SHA-핀 타르볼의 외부 미러 방침(O-13 §7.1 nightly 미러 방침 동형 — 필요 시점 절차화, §9-1) + 스파이크 보존 타르볼 교차 앵커(§3.2).
5. **사이드카-산물 표류**: 낡은 b′ 산물과 짝 잘못 맞으면 하한 대조가 오답 — 완화(정본 승격 완료 — §4.3): `artifact_sha256` 콘텐츠 결부 + inject-marker 입력 대조 + check 짝 대조, 음성은 §5.6-⑨.
6. **heap_floor 추출의 견고성**: strip-후 산물엔 name 섹션이 없어 스택-포인터 식별이 형상 휴리스틱화 — 완화: 추출을 strip 전 산물로 규정(§2-2 --extract 단계) + 휴리스틱 폴백(최대 초기값 mutable i32 전역) 시 경고-아닌-실패. memory_min 항(§2-2)이 스택-포인터 식별 실패의 하방도 부분 방어.
7. **트랩 진단 구분 불가 잔존**(SPK §4-6): 헬퍼 트랩 vs 가드 트랩 동일 `UnreachableCodeReached` — 본 증분 비목표로 존치(진단 표지 export는 b′ 표면 확장이라 별도 재심), 문서 명기만.
8. **e2e 의존 비대**: `component-model` feature + otter-aot-link path-dep가 48-트레인 락파일을 키움 — 완화: 호스트-트레인 락파일 위생 원칙(e2e Cargo.toml 주석 [실측])대로 e2e 소유 락파일에 국한, 참조 호스트 무영향.

---

## §9. 열린 결정 (4건 — §9-4는 패널 C-S6 신설)

1. **외부 도구 미러의 절차화 시점**: 방침만 고정(§8-4), 아카이브 소실 리스크 현실화 시 절차화 — O-13 §7.1 동형.
2. **export 축소의 최종 표면에서 memory 제거 여부**: v1은 memory 유지(§5.4 — args-블록 마셜링 전제; 단 컴포넌트 형상은 §6.1대로 argc=0/게스트-부트 한정이라 이 결정의 실질 대상은 코어-결합 형상). argc=0 전용 배포에서 memory 은닉이 유익한지는 소비자 실물 후 재심.
3. **매니페스트 빌드-해시 fail-closed 전체 규율**: b′ 콘텐츠-해시(§4.3 정본)까지 지금, 산물 전체(방출물 포함) 해시 체계는 배포 채널(패키징 포맷 — O-14 §7b-② universal-binary 선행 사례 재인용 예정) 확정 시. 버전-대조의 변별력 회복은 버저닝 도입(otter-reusability-plan) 종속.
4. **폴백 형상의 게이트-충족성(오너 확인 항목 — 패널 C-S6)**: host-리프트 실패 시 host-프리 컴포넌트 1종 + 코어-결합 host 형상의 조합이 상향 목표 ① (iv)~(vii)을 충족하는지 — 오너 결정 B("프로그램-AOT로 충분")가 이 폴백 형상까지 확언하지 않으므로 본 문서가 자체 확정할 수 없다. L-5 실측이 폴백을 실제 발동시키는 경우에만 상신(발동 없으면 소멸). **→ 소멸 (L-5, 자체 규정 축자 발동)**: host-리프트가 실측 성공([추정 ②]·⑤ 해소 — §2-4-3/§6.1 승격 부기)해 폴백은 발동하지 않았다 — 상신 대상 없음, 본 항목은 소멸한다. host-포함 컴포넌트(calc)가 게이트 산물 실물로 실존(AOT48-E2E-7-COMPONENT-CALC-OK).

---

## §10. O-16c′ 완료 대차 (L-6 신설 — 증분별 커밋·게이트·잔여)

### 10.1 증분별 착지

| 증분 | 커밋 | 실물 | 게이트 결과 |
|---|---|---|---|
| L-3 + L-1a + L-1b | `5b2ea5e` | `just wasm-aot-tools`(SHA-핀 조달, TOFU 기록 §3.2) + `hosts/otter-aot-link`(rename/check --imports/--rename-invariance/inject-marker/check --scratch/lib-manifest/schema) | `wasm-aot-link-check` 그린(단위 전 항목 ± — l1a 23 + l1b 30) |
| L-2 | `fbb1c2a` | `hosts/otter-helpers-wasm` + `just wasm-bprime`(양 프로파일, 사이드카 2단, EH-프리, heap_floor 상수 단언, B2–B5 재현 40k×0, 대표-region 재측정) + O-W 등재(MODE §1.4 행 7) | `wasm-bprime` 그린 + 이월 경화 4건 뮤테이션 실증 |
| L-4 | `ca2e06c` | `check --linked`/`shrink-exports` + `just wasm-aot-link-e2e`(3형상: rename→inject→instantiate→merge→check 3종→EH validate/census→shrink→재검→실행 차등) + §2-5 재실행 4항 전건 + 피처-플래그 census(5종 drop-one) + e2e 상수 상향(0x110000/18p) | AOT48-E2E-6 그린, [추정 ①③④] 해소, §7 nightly 탈출 최종-산물 단위 [실측] |
| L-5 | `6a22004` | wit/ 정본 2월드 + kebab/host-리프트 rename + `just wasm-aot-component`(embed/new + §6.2 네이티브 검사 + 실행 차등 + E10/E11 음성) | AOT48-E2E-7 그린, [추정 ②⑤] 해소($root+kebab), §9-4 소멸 |
| L-6 | (본 표를 담은 그 커밋 — 자기-참조; `git log --oneline`에서 "O-16c′ L-6" 커밋이 정본) | 뮤테이션 ①~⑩ 레저(`hosts/otter-aot-link/tests/l6_mutation_ledger.rs` — 실행 5건 + 영구-대체 5건 + 메타-테스트) + check --linked sharedness 교차 술어 + 세부 분기 음성 4종 + E11 실패-사유 grep + kind≠func 커버 + ci.yml 편입(스텝 2개 = 게이트 4종, 캐시 2행 추가, 카나리아 비편입 사유 기록) + 문서 마감 4종(본 문서·SPK·O-14·MODE) | 뮤테이션 전건 적색 실증 + 전 레시피 무회귀 + YAML 파스 |

### 10.2 게이트 산출물 대차 (§1.1의 4항 — 전건 충족)

1. **b′ 실물**: `hosts/otter-helpers-wasm` + `just wasm-bprime` — 충족(비공유·+atomics, EH-프리, stable 1.96, 사이드카 콘텐츠-해시 결부).
2. **b″ 실물**: `hosts/otter-aot-link` + `just wasm-aot-tools` — 충족(자체 구현 5 서브커맨드 + wasm-merge 유일 외부, 버전-핀 조달).
3. **결합물+컴포넌트 양 형상 E2E**: `just wasm-aot-link-e2e`(AOT48-E2E-6) + `just wasm-aot-component`(AOT48-E2E-7) — 충족(T0 오라클 비트-동일, 전-체인 EH-프리 기계 검증; SPK §4-12 [실측] 승격).
4. **SPK §5 상충 6건 + O-14 §11 유보 3건**: 전건 해소(§2 결정 → SPK §6 부기 / O-14 §11 상태 갱신).

### 10.3 잔여 (본 증분이 남기는 것 — 전부 기존 유보의 재확인, 신규 0)

- **§9-1** 외부 도구 미러 절차화: 방침만 고정 유지(발동 조건 미충족).
- **§9-2** export 축소 최종 표면의 memory 제거 여부: 소비자 실물 후 재심 유지.
- **§9-3** 산물 전체 빌드-해시 체계: 배포 채널 확정 시 유지. 버전-대조 변별력 회복은 버저닝(otter-reusability-plan) 종속 — §4.3의 "변별력 0" 정직 명기 불변.
- **§9-4**: 소멸(L-5 — §9 본문 부기).
- O-16f p3-브리지(해석 (i)~(iii))는 §1.2대로 본 문서 무관여 — 별도 증분.
- 트랩 진단 구분(§8-7)·실할당자 도입(SPK §4-5) 재심 트리거는 라이브러리 확장 시점.

---

*작성 규율 확인: 오너 결정 인용은 전부 MODE §5b(결정 A·B) — 그 외 결정(§0 처리·§2 해소 6건·§3 소재·§4 소재·§5 게이트 문구·§6 개명 규칙)은 본 문서의 설계 선택임을 명기. [실측]은 SPK·레포·스파이크 로그·패널 회차 직접 덤프 축자 귀속. [추정] 5건(① 주입-후 결합 보존 — L-4, ② host-리프트 — L-5, ③ wasm-merge×(start·passive·TLS) 병합 — L-4-③, ④ shared memory 대상 active 마커 주입·보존 — L-1b/L-4-③, ⑤ host-리프트 코어 모듈-문자열 규약 — L-5)은 **전건 [실측] 승격 완료** — 해소 증분·값은 각 절의 승격 부기(§2-1/§2-4-3/§2-5-③/§6.1)와 §0 착지 부기가 정본(③은 `__tls_base` 소거 정정 동반). v2 규율: 3렌즈 패널 must-fix 7건 전건 반영(§0), should-fix 18건 기각 0(구현-형태 선택 2건은 대안 기각 사유 명기), 상충 4건(§0-2 병합-매직/패딩, §0-3 허용/소거, §0-5 이관처, §0-7 스키마 소유) 기록 후 보수 선택.*
