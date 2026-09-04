# neosurf 엔진 스택 결정 기록 (v1)

> 결정일: 2026-06-15 · 근거 분석: [`web-engine-comparison.md`](./web-engine-comparison.md)
> 비교 리포트의 **옵션 B**(Rust 코어 + 검증된 크레이트 재사용 + 독자 통합)를 기반으로, 사용자 지시에 따라 일부를 재정의한 **옵션 B′**.

## 1. 확정된 스택

| 영역 | 결정 | 비고 |
|---|---|---|
| **코어 언어** | Rust | 옵션 B 유지 |
| **JS/TS + WASM 런타임** | **자체 개발** (SpiderMonkey/V8 임베드 안 함) | 옵션 B에서 **변경** — 최대 변경점 |
| **레이아웃** | **Servo Layout 2020** 재사용/fork + 업스트림 추적·contribute-back, **레이아웃 seam 뒤** | **S4 결정(2026-06-16)**: Blitz/Taffy 기각·폴백 (근거 spikes/s4-layout) |
| **CSS/스타일** | **Stylo** 업스트림 그대로 | 옵션 B 유지 |
| **렌더러** | **렌더러-불가지 추상화 표준 API 우선(day-1)**; default 렌더러 미정(WebRender 유력 후보) | §9 #5 (API 먼저, default 후논의) |
| **프로세스 모델** | **3계층 하이브리드** — OS 프로세스(보안 경계) + in-VM 그린 프로세스·슈퍼비전(동시성) + capability 권한층 | 옵션 B에서 **변경**, 격리 스코프 유지 (상세 §4.2) |
| **DOM·GC** | **통합 힙**(DOM=GC 객체, 사이클 컬렉터 불필요) + **컴파일러 강제 루팅**(메커니즘 미정) + **실시간 GC**(저사양 양호) | 상세 §4.4 |
| **정형검증** | **계층형** — 안전성 정형 + translation validation(SMT) + 차분/퍼즈 | 상세 §4.5 |
| **표준 베이스라인** | WPT ≥ 80%, ES2025 완벽 지원 | 방침 |
| **스코프** | minimal embeddable-by-default 코어 (완성 브라우저 아님) | 방침 |
| **라이선스** | **BSD-2-Clause-Patent** (프로젝트 자체; SPDX, LICENSE=newsniper-org) — 재사용 Stylo/WebRender는 파일단위 MPL-2.0(BSD에 결합 무방, 해당 파일만 MPL 유지) | 2026-06-15 확정 |

## 2. 자체 JS/TS/WASM 런타임 — 상세

- **개발 순서**: 먼저 **메타-VM 생성 프레임워크(L0)** 를 Rust로 개발(의미론 저작 언어=제한된 TS 서브셋) → 그 위에 런타임(L1) 구현·빌드(formal verification 병용).
- **JIT 프레이밍 (합의 2026-06-15, spike #3 검증)**: "meta-hybrid"를 **단일 주석 소스 → 다티어·다전략 VM 자동생성**으로 재정의. Deegen·Truffle·RPython이 모두 "주석 소스 1벌 → VM"임에 착안해 *장점 통합*:
  - **단일 진리 소스** = `{바이트코드 의미론 + 최소 JIT 힌트}`; 개발자는 인터프리터+표식만, 프레임워크가 메타-JIT 수행(RPython/Truffle식 DX·범용성).
  - **티어**: T0 특화 인터프리터(Deegen식 생성=시맨틱 오라클) → T1 copy-and-patch 베이스라인(Deegen식 생성, 마이크로초·빠른 warm-up) → T2 최적화(힌트구동 메서드/PE=Truffle식; 선택 LBBV; 코드젠 **Cranelift/MIR 재사용** 검토). **메타-트레이싱=장기실행·루프용 선택적 적응 전략.**
  - **범용성**: L0는 독립 프레임워크 — 아무 언어나 `{의미론+힌트}`로 티어드 VM; neosurf 런타임(L1)은 한 클라이언트.
  - **신규=연구기여/리스크**: **전략-불가지 단일 힌트 어휘**(promote/elidable/unroll/virtualizable/IC-spec)가 copy-and-patch IC + PE 특화 + (선택)트레이싱을 *동시에* 구동(1소스→다백엔드, 미통합 영역).
  - **저작 언어**: substrate=Rust(no_std/alloc,stable); 의미론 저작=제한된 TS 서브셋(RPython→Python 유추) = "Rust+TS"의 실체.
  - **표현(정직)**: "단일전략 능가" 폐기 → falsifiable "method-JIT 베이스라인 대비 경쟁력 + warm-up 격차 축소"; "표식만 넣으면 동작"=범용성·낮은 노력의 *동작* JIT엔 참, 피크는 힌트 튜닝 필요; 통합 힌트·1소스→다백엔드 미입증 → spike #3 kill-criteria. (메모리: jit-approaches-research, meta-hybrid-jit-prior-art)
- **목표**:
  - JIT · AOT · PGO 지원
  - ECMAScript: **최소 ES2025 완벽 지원**
  - TypeScript 지원
  - WASM ABI: **wasm32 + wasip2 우선(committed)** + wasm64·wasip3은 목표(experimental, §9 #4)
  - **단독으로도 easily embeddable** (분리 가능한 서브프로젝트)
- **개발 방법론** (참고): **PyPy/RPython 방법론** 차용(도구는 Py2.7이라 버리되 *how*는 차용) — 인터프리터 우선·JIT은 메타레벨에서 적용, 언어무관 툴체인을 toy로 먼저 검증, "미번역" 빠른 인터프리터 모드를 시맨틱 오라클(=translation validation)로 유지, 정확성 먼저·성능 나중(test262 통과 후 JIT 적층), 레퍼런스 대비 차분 테스트, 벤치 주도, 인터프리터 힌트 어휘 설계(`@elidable`/`promote`류 → hybrid의 trace↔method 선택까지). Py2.7 락인·느린 전체번역·워밍업은 회피(AOT/PGO로 완화). (메모리: runtime-dev-methodology-pypy)
- **Rust 설정**(런타임 트랙): **stable ≥ 1.96 + 2024 edition + 가급적 `no_std` + `alloc`**. stable=재현성·기여 접근성·임베드성(Servo nightly 핀 회피), no_std+alloc=최대 이식성 + capability 모델 정합(앰비언트 OS 권한 없음=no_std) + 검증 표면 축소. 적용: 코어 크레이트 `#![no_std]`+`alloc`, 가장자리에 얇은 std/플랫폼 shim(W^X 실행메모리·스케줄러·시계·호스트 I/O는 capability/WASI 뒤로). **JIT 최종 코드방출·실행메모리는 플랫폼 의존이라 no_std 불가 → shim 격리**(AOT 출력은 무관). **툴체인은 독립 산출물**: 이 설정은 메타-하이브리드 JIT 툴체인을 *단독으로* 쓰려는 경우(툴체인"만" 가져다 다른 언어 런타임을 만드는 RPython식 활용)를 위해 stable 위에 홀로 서야 하기 때문. **L0 툴체인은 독립적으로 stable**(단독 재사용). 레이어링: **L0 툴체인 → L1 런타임 → L2 브라우저 엔진 — 전부 stable**. (정정 2026-06-16: Stylo/WebRender/Servo는 이제 **stable 빌드** — servo/stylo CI=stable, Servo 0.1.0 crates.io 2026-04, Blitz가 Stylo를 stable로 사용 → 검증 리포트의 "코어 트랙 nightly" 우려·두 toolchain 화해 문제 **해소**.) (메모리: runtime-rust-toolchain-config)
- **벤치 하네스**(런타임): 별도 환경설정 없이 **커맨드 한 줄로 즉시 벤치**. 실행모드 비교(인터프리터/JIT/AOT/PGO) + 전략 비교(trace/method/**hybrid**) + throughput·latency·**warmup**·footprint·**GC pause** 동시 측정 + 회귀 추적·CI 게이트. BacCaml/Marr-Ducasse 방향성 재현의 *측정 도구*이자 실시간 GC 트레이드오프 게이지(차분 정확성도 곁들이면 Tier2 FV 직결). (메모리: runtime-benchmark-harness)
- **선행 연구 좌표**(설계 참고):
  - **BacCaml** (정전급 meta-hybrid PoC, prg-titech) — OCaml/MinCaml 기반, RPython 아키텍처, meta-tracing에 method-based를 더해 **실행 컨텍스트별로 두 방식을 amalgamate**. DLS'20 "Amalgamating Different JIT Compilations..."(arXiv 2011.03516), ICOOOLPS'21(arXiv 2106.12496). **단 obsolete/unmaintained → 후속은 Adaptive RPython.**
  - **Adaptive RPython / Adaptive Meta-JIT** — BacCaml의 살아있는 후속(RPython=PyPy 기반). **단 ⚠ Python 2.7 전용**(RPython 툴체인은 Py3 미이식, Py2.7은 2020년 EOL) → 라이브 하네스가 아니라 *읽기용 레퍼런스*로. (메타메서드 측 라이브 프로덕션 사례는 GraalVM Truffle=Java.)
  - RPython/PyPy(메타트레이싱 substrate), GraalVM Truffle(부분평가=메타메서드/Futamura), Marr & Ducasse OOPSLA'15(두 방식 비교), CakeML(검증 *AOT* — 검증 메타-JIT 프로덕션 선례 없음).
  - **함의**: 기존 meta-hybrid 선행은 PoC급이며 OCaml/Python. **Rust meta-hybrid 프레임워크는 존재하지 않음** → 본 Rust+TS 툴체인은 greenfield/frontier. (참고 메모리: meta-hybrid-jit-prior-art)

## 3. 핵심 함의

1. **DOM↔스크립트 바인딩·GC 루팅이 완전 자체 구현**이 됨. Servo의 mozjs 기반 바인딩(`crown`/Reflector/JSTraceable)은 **전이 불가** — 비교 리포트가 "가장 위험한 서브시스템"으로 지목한 부분을, SpiderMonkey의 검증된 기계장치 없이 새로 풀어야 함.
2. **Servo Layout 2020 fork는 유효** — 레이아웃은 JS 엔진과 대체로 독립적이라 fork가 성립. 단 업스트림 호환 추적 규율 필요.
3. **런타임은 크리티컬 패스에서 제외**(§4.1 확정) — 분리 서브프로젝트로 병행. 단 통합 전까지 브라우저는 비스크립트 문서 웹에 한정(SpiderMonkey 임시 사용 금지).
4. **공수·리스크 재보정**: 런타임은 RPython/GraalVM급 연구 프로젝트. 검증 메타-JIT은 선례가 없어 일정 리스크 최상위.

## 4. 설계 분기 결정 (Q1–Q3 확정)

### 4.1 런타임 순서 (Q1) — 확정: 문서웹 코어 먼저 + 런타임 병행
브라우저 코어(Servo-layout/Stylo/WebRender)로 **비스크립트 문서 웹 MVP를 먼저** 세워 조기 WPT 진전을 내고, 자체 JS/TS/WASM 런타임은 **분리 서브프로젝트로 병행 개발 → 준비되면 통합**. 금지 의존(SpiderMonkey) 없이 가장 빠른 가시적 성과. 런타임은 크리티컬 패스에서 제외.

### 4.2 프로세스/보안 모델 (Q2) — 확정: 3계층 하이브리드
| 계층 | 기술 | 담당 |
|---|---|---|
| ① 보안 경계 | **OS 프로세스**(사이트/origin별, OOPIF식) | T1 메모리 익스플로잇 · T2 Spectre · T4 샌드박스 탈출 봉쇄 (비협상) |
| ② 동시성·내결함성 | **in-VM 그린 프로세스 + 슈퍼비전**(도메인 내부) | 값싼 병렬 · let-it-crash |
| ③ 권한 | **capability 기반**(ambient authority 제거, IPC=capability 전달) | T3 최소권한 · 임베드성 · **WASI(wasip2/p3) 정합** |

근거: capability·in-VM 격리는 T3(권한)엔 강력하나 **T2(Spectre)를 못 막음** — WASM 단일 주소공간 Spectre 우회가 확인됐고(소프트웨어 펜스 1.8–19.6× 비용), CHERI/Morello조차 Spectre 미해결. 런타임 JIT의 런타임 기계어 생성이 in-process 공격면을 키우므로, 상호 불신 주체 간 경계는 **OS 프로세스가 비협상**. CHERI/HFI는 미래 선택적 강화. 정형검증 친화(seL4=검증된 capability 마이크로커널).

**보안 불변식 (확정 2026-06-15)**:
- **(1) 스케줄링 (strong, 비협상)**: 한 OS프로세스는 **단일 신뢰도메인**만 호스팅 — 서로 다른 신뢰도메인의 녹색프로세스 혼재 **불허(자원 압박 시에도; consolidation 기각)**. 같은 도메인의 N 녹색프로세스(프레임/워커/탭)는 한 프로세스에 허용 → 교차도메인 Spectre 노출 원천 차단.
- **(2) 힙-파티션**: 통합힙을 per-OS-프로세스; 녹색프로세스별 GC region; 교차-agent는 capability/불변 메시지만(브랜딩이 비탈출 컴파일강제).
- **(3) 표현**: 녹색프로세스 = 기밀/무결 경계 0; 상호불신 코드는 전부 OS프로세스 분리; 녹색프로세스+supervision = 도메인 내 동시성·크래시 복구만.
- **저자원 양립 (병합 대신)**: RAM은 *동시 라이브 도메인 수*에 비례(프레임 수 아님). ⓐ 신뢰도메인 입도 = **site(eTLD+1)**(프로세스 수↓·강한 불변식 유지) ⓑ **동시 라이브 도메인 제한 + LRU eviction/discarding**(병합 아닌 종료·재생성) ⓒ CoW 공유 읽기전용 페이지(코드/폰트/ICU) ⓓ 값싼 녹색프로세스로 도메인당 OS프로세스 최소화.

### 4.3 TypeScript 실행 (Q3) — 확정: 타입 소거 기본 + 선택적 타입 지향 최적화
기본은 **타입 스트립 후 JS 의미론 실행**(표준 정합, Node/Deno/Bun식). 추가로 TS 타입을 최적화 힌트로 쓰는 **타입 지향 최적화는 선택적 경로**(비건전 타입은 가드/런타임 체크) — 메타-하이브리드 JIT의 최적화 목표와 정합.

### 4.4 DOM↔스크립트 바인딩·GC 모델 (①) — 합의 방향 (2026-06-15; 구체는 spike #1 검증)
- **통합 힙**: DOM=GC 객체(스크립트와 동일 컬렉터), 사이클 컬렉터 불필요. 자체 런타임이라 GC·DOM 양쪽 통제 → 처음부터 가능.
- **컬렉터 = 자체 세대별-이동 GC**: 증분·세이프포인트 기반 우선(minor=너서리 evacuate, major=증분 마킹+바운드 compaction); 동시 evacuation(읽기 배리어, ZGC/Shenandoah/LXR급)은 **tunable 상위 티어**. **MMTk를 되돌림 옵션**으로 보유(루팅은 ~직교).
- **루팅 = 하이브리드**: durable `Rooted<T>`/핸들(루트 집합 등록, 이동 시 컬렉터 갱신 — 네이티브/JIT/임베더/장수) + transient 브랜디드 `Gc<'gc>`(영비용, 컴파일러 강제 비탈출). **브랜드 스코프 = 컴파일러 강제 no-move/no-GC 스코프**. (= V8/SpiderMonkey 모델 + Rust borrow-checker 업그레이드.) gc-arena는 *브랜딩 기법 참고만*(비이동·단일스레드, 컬렉터 아님).
- **GC 호출 최소화 (co-equal 축)**: ⓐ *GC 힙에 안 넣기*(구조적·footprint 무상): 스크립트 비관측 엔진 내부는 Rust 소유권+phase arena(일괄 해제), 태그드 immediate/NaN-boxing, shape/atom; ⓑ *덜 할당/재사용*: 풀/free-list, rope/slice, **메타-JIT escape analysis→스칼라 치환·스택 할당**; ⓒ *덜·좋은 때 수집*: 세대별(저렴 minor·드문 major), **idle/턴 사이 스케줄링**(vsync slack), 프레임 임계 구간은 브랜드 no-GC 스코프로 배제. 너서리 크기=티어별 tunable.
- **트레이드오프**: 빈도↔pause↔footprint 삼각. *구조적 레버*(ⓐ+escape+풀링)는 셋 다 무상 개선; *사이징 레버*(큰 너서리)는 footprint 교환 → 구조적 1순위, 사이징은 튜닝 노브.
- **WebIDL→Rust 바인딩 생성기**: Trace/Rooted 글루 자동 생성(루팅 ergonomics 흡수). 1급 워크스트림(VM이 아니라 바인딩 유지가 진짜 병목).
- **spike #1 증명 변수**: ⑴ 브랜드 스코프 ↔ 이벤트루프 턴/agent 경계 매핑, ⑵ 실제 DOM 핸들 볼륨·비용, ⑶ JIT 루트 전략(정밀 stack map vs 핸들), ⑷ 저사양 sub-frame MMU·배리어 비용, ⑸ **GC-힙 residency 비율·할당률**(최소화 레버 효과).

### 4.5 Formal verification 범위 (②) — 확정: 계층형/리스크 기반
- **Tier1** 안전성: 코드젠 안전·GC 루팅 컴파일러 강제·capability 봉쇄 — 정형 보장.
- **Tier2** translation validation(SMT, TurboTV/Alive2식)으로 JIT 출력 의미보존 검증 + 차분·퍼즈 테스트(test262) — miscompilation 차단.
- **Tier3** 최소 시맨틱 코어·투기/deopt 기계증명(POPL'21 "verified speculation & deopt"식) — 북극성. TCB 명시. 전체 기능 증명은 근시일 범위 밖.

## 5. 프로세스 규율

- **verify-before-implement (위험 서브시스템 엄격 적용)**: 위험 서브시스템 — 자체 meta-hybrid JIT/런타임 코드젠, DOM↔스크립트 바인딩+GC 루팅, unsafe/FFI 경계, 프로세스 격리·보안 경계 — 의 변경은 **구현 전에 검증 산물**(정형 명세·증명 → 모델검사 → 속성/차분 테스트+레퍼런스 오라클 → 검증된 프로토타입)을 먼저 확립하고 **그 통과를 게이트**로 삼는다. "구현 먼저, 테스트 나중"은 이들 영역에서 불허. 위험 아이디어는 기존 substrate에서 먼저 검증하되 — 메타-하이브리드 JIT의 경우 BacCaml(OCaml, 死)·Adaptive RPython(**Python 2.7 EOL**)·GraalVM Truffle(Java, 메타메서드 측만)은 *읽기용 레퍼런스*로 두고, 검증 산물은 **toy 바이트코드 + 트레이싱/메서드 두 전략 + 선택기를 구현한 소형 Rust 프로토타입**(BacCaml DLS'20·Marr&Ducasse OOPSLA'15의 방향성 결과 재현)으로 삼는다 — Py2.7 하네스에 검증을 묶지 않는다. (메모리: verify-before-implement-dangerous-subsystems)

## 6. 웹표준 외 추가 지원 목표 (멀티미디어 · 최소)

웹 표준(WPT≥80% 베이스라인) 외에 추가 지원할 *최소* 목표 (Advice 4):
- **오디오 코덱**: FLAC(Ogg 컨테이너 경유), WavPack v5
- **비디오 코덱**: Motion JPEG XL
- **이미지 포맷**: JPEG XL
- **색 심도**(이미지/비디오 제외 — UI/CSS/canvas/컴포지팅): 최소 **10bpc**까지 완벽 지원
- **그 외 미기재**: FFmpeg 또는 GStreamer로 끌어오는 정도면 충분

함의:
- 철학: 로열티-프리·고충실·개방 포맷(JXL 중심 이미지+모션, 무손실 오디오, 깊은 색심도). JXL은 웹표준 외(Chrome 2023 제거, Safari 추가).
- 후보 pure-Rust 디코더: **jxl-oxide**(JPEG XL), **Symphonia**(FLAC+Ogg) 또는 `claxon`+`ogg`. **WavPack v5는 성숙한 pure-Rust 부재** → C 바인딩 또는 FFmpeg 경로. "Motion JPEG XL"=JXL 애니메이션/멀티프레임(디코더 애니 성숙도 확인 필요).
- **10bpc는 렌더링 파이프라인 제약**(WebRender 서피스 포맷, CSS Color 4 광색역/HDR, 캔버스, 컬러 매니지먼트) — L2 설계 사안.
- **보안(비협상)**: FFmpeg/GStreamer·C 코덱 라이브러리는 큰 C/C++ 공격면 → 3계층 모델대로 **반드시 샌드박스**(아웃오브프로세스 미디어 디코더 / RLBox식 wasm 격리).
(메모리: multimedia-support-targets)

## 7. 프로토콜 지원 목표 (Advice 5)

최신 프로토콜 지원에 유의 — 특히 **HTTP/3, SRT, TLS 1.3**:
- **TLS 1.3**: ✅ 선택한 **rustls가 TLS 1.3-first** → 추가 의존 없음.
- **HTTP/3**: TARGET(검증 리포트의 열린 "QUIC: neqo vs quinn" 결정). all-Rust는 **quinn + h3(hyperium)**(rustls 기반), Firefox 검증은 **neqo**. UDP/QUIC는 네트워크 프로세스 + 능력 모델 안.
- **SRT(Secure Reliable Transport)**: 웹 페이지용 아님 — 저지연 **미디어 전송**(고충실 멀티미디어 노선과 일관, §6). **srt-tokio/srt-rs**(pure-Rust, 성숙도↓) 또는 libsrt(C→샌드박스).
- 보안: QUIC·SRT은 UDP 기반 큰 파싱 면 → C 전송 라이브러리는 반드시 샌드박스.
(메모리: protocol-support-targets)

## 8. 검증으로 확인된 사실 (팩트 정정 — 2026-06-15)

`validation-report.md`의 적대적 검증에서 확인된 **객관적 사실**(결정/타깃 변경 아님; 이에 대한 재프레임은 논의 후 §5.3대로 반영):
- **WebRender**: crates.io에 **published**(0.69.0, 2026-06-12, 형제 크레이트 포함) — 재사용 가능. 단 **안정 semver 계약 없음**, **OpenGL/GLES 내부**(네이티브 Vulkan/Metal/wgpu 백엔드 부재) → 저자원 GPU 가정과 긴장.
- **wasip3 / wasm64**: **experimental**. wasip3=no-semver(Wasmtime), WASI 1.0 ~2027 예상; **wasm64/memory64 스펙 repo 2025-01 archived**, Rust 패리티 미존재.
- **gc-arena**: **비이동(non-moving) + 단일스레드**(멀티스레드 수집·객체 이동 미지원). "generativity"는 컬렉터가 아니라 단일 `mutate()` 스코프 rooting 규율.
- **WPT**: **~99.5% 서브테스트가 JS 게이트**(독립 측정) → "문서 코어가 조기 WPT 신호" 전제와 충돌.
- **메타-tracing**: 브라우저가 출하 후 폐기(TraceMonkey→Jaeger/Ion); Marr & Ducasse(OOPSLA'15)=피크 성능 우위 없음(실제 발견은 *구현 노력 절감*). 러스트 메타-하이브리드 선례 0.
- **공수**: 결정된 워크스트림 ~604–1216 PM(50–101 PY); 누락 서브시스템 포함 시 **>1000 PM(>80 PY)**.

→ 위 사실들에 기반한 **결정 재프레임**(메타-하이브리드 프레이밍, GC per-agent화, 레이아웃 Blitz/Taffy 대안, WPT 지표 이원화, 3계층 불변식, wasm64 강등 등)은 **논의 후** 본 문서에 반영한다. (근거: validation-report.md §2·§5)

## 9. validation 재프레임 반영 (2026-06-15)

저논쟁 재프레임 처리 (validation §5.3):
- **#1 WPT 지표 이원화 (수용)**: pre-runtime = 스크립트-독립 서브셋 %(reftest/css/parser/큐레이션), full WPT≥80% = 런타임 통합 후(+test262≥95%); 호라이즌 7–10+년; 문서-웹-우선·no-stand-in 유지(베이스라인이 JS 경로); "조기 WPT 신호" 정당화 삭제.
- **#3 C14 보안 표현 (수용)**: "프로세스 경계=robust/유일" → "필요 최소 + SMT 규율(형제 SMT에 두 principal 비공존)+context-switch flush/IBPB+microcode 최신"; 상용 HW Spectre 완전차단 불가; JIT은 코드젠 하드닝 면허 아님.
- **#4 wasm64/wasip3 강등 (수용)**: wasip2 즉시 채택; wasip3=flag 추적(WASI 1.0~2027); wasm64=experimental(memory64 archived) — 목표 유지·committed ABI 아님; "WASI=Spectre 경계" 삭제.
- **#6 런타임 3 프런티어 디커플 (수용)**: 메타-JIT·실시간 GC·Tier2/3 검증을 한 마일스톤 적층 금지; 단순(non-meta) 티어에서 GC·translation validation 먼저.
- **#7 Stylo 재사용 비용 정밀화 (수용)**: 두 trait 레이어(Stylo `dom.rs` TElement/TNode/TDocument ~73 contract points (+Servo-fork 시 `wrapper_traits` 둘째 표면)) + ElementData(`servo_arc`+`SharedRwLock`)가 GC 밖 → 통합힙 충돌 = verify-before-implement; 상류 핀·연속 통합세.
- **#5 렌더러 = API 우선 (수정 수용)**: WebRender를 default로 선커밋하지 않음. **렌더러-불가지 추상화 표준 API를 day-1 제정** → 그 후 default 렌더러(WebRender 유력 vs 기타) 재논의. SWGL식 SW 폴백·no-semver·GL-내부 caveat는 후보 평가 입력.
- **#2 3계층 보안 불변식 (수용·확정)**: (1)스케줄링=**strong**(교차도메인 혼재 불허, consolidation 기각), (2)힙-파티션, (3)표현 — §4.2 불변식에 통합. 저자원은 eviction+site입도+CoW로(병합 아님).

### 9.1 보안 불변식 (1) — 확정: strong (2026-06-15)
서로 다른 신뢰도메인의 녹색프로세스를 한 OS프로세스에 혼재 **불허**(자원 압박 시에도). **consolidation 타협 기각** — 더 안전한 형태(검증 리포트 원래 권고). 저자원은 병합이 아니라 **LRU eviction/discarding + site(eTLD+1) 입도 + CoW 공유 읽기전용 페이지**로 대응. → §4.2 불변식에 통합 완료.

*(이 문서는 결정이 진전되면 갱신한다.)*
