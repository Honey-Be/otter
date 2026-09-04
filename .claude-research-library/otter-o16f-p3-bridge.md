Status: SHIPPED (2026-09-04 — 구현·2렌즈 리뷰 완료; 본 노트는 리뷰가 차단 지적한 "정본 부재"의 해소분으로,
구현 커밋에 동반된다)

# O-16f — WASIp3 브리지 (J-모드, 경로 12의 제품화)

정본 관계: 모드 문서(`otter-wasm-mode-architecture-alternatives.md`) §1.1 경로 12 + §4.1 J-모드 "선택
확장"의 실물. 이 노트가 8곳(호스트 main.rs/Cargo.toml/README, 데모 Cargo.toml/README, justfile, ci.yml)
에서 인용되는 규범의 실체다. 표기: **[실측]** = 스파이크 S1/S2·구현 게이트·리뷰 독립 재실행으로 확인,
**[추론]** = 미실측 추론(명시).

---

## §1. 스파이크 요지 (S1/S2, 2026-09-04 — 전문은 워크플로 기록)

### S1 — wasm32-wasip3 직접 게스트 빌드: **불성립 확정** [실측]

- 타깃은 rustc 목록에 존재하나 tier-3(rustup 프리빌트 std 없음). §7 레시피(build-std)로 **컴파일은 전부
  통과, 링크가 3중 벽**: ① p3 전용 self-contained wasi-libc 부재(어디서도 배포 안 됨) ② wasip2 libc
  차용 시 lld가 `--cooperative-threading`(타깃 스펙이 강제) 위반으로 구조적 거부 ③ 그 플래그를 제거한
  비지원 커스텀 스펙조차 `__wasm_get/set_stack_pointer` 미정의(SP-via-function 코드젠이 트리플에 무조건
  내장) — **cooperative-threading 모드로 빌드된 p3 전용 wasi-libc 없이는 어떤 wasip3 링크도 불가**.
- 링크가 성공했더라도: 타깃 스펙이 `wasm-component-ld`를 내장해 산출물은 **컴포넌트** — otter_jit 계약의
  전제(코어 memory/table을 호스트와 직접 공유)가 컴포넌트 경계에서 표면화 불가(모드 문서 §1.3 불가능
  판정의 게스트-측 실측 뒷받침). 부수 실측: wasip3 타깃 스펙은 `-wasm-use-legacy-eh=false`를 내장(§7
  exnref 핀이 태생적 기본값).
- **판정**: wasip3-게스트 직접 빌드 경로 폐기. **브리지(경로 12)가 (i)~(iii)의 유일 경로**임이 실측으로
  확정. **재평가 트리거 = "wasip3 rustup 프리빌트 std 출현"**(모드 문서 §1.4 워치리스트 등재).

### S2 — 혼합 호스팅: **성립** [실측] + 대발견

- **대발견 — §5.5 "동기-Store 긴장"의 전제가 48에서 소멸**: `Config::async_support`는 48에서
  **deprecated + no-op**(config.rs:427-431). 실제 기전은 Store별 단방향 래치 `async_required` — async-향
  요소(async 호스트 함수의 **실링크**, async limiter/call-hook/fuel-yield)가 Store에 들어오는 순간
  래치가 걸리고 이후 모든 동기 진입점이 `validate_sync_call`로 거부된다(store.rs:2249; asyncness는
  실링크된 임포트들의 OR — 등록만으로는 무해).
- 같은 동기 `Store<Host>`에 코어 게스트(p1 sync + otter_jit) + p3 컴포넌트 공존 [실측: E1-E3]. 하나의
  `WasiP1Ctx`가 p1 코어 배선과 p3 컴포넌트 배선을 동시에 받침(`WasiP1Ctx: WasiView` 자체 구현 —
  p1-on-p2 선례의 타입 증거, p1.rs:232).
- 브리지 왕복 [실측: E3]: 코어 게스트 임포트 → 호스트 클로저(`Caller`→`AsContextMut`) → **같은 Store에서
  컴포넌트 export 재진입 호출** → 결과를 게스트 선형 메모리에 기입. 동일 실행 안에서 otter_jit 티어업과
  공존 [실측: E3 — jit 1/1, bridge 2회].
- **경계 실측 [E5]**: async-WIT 임포트(`wait-for`류)는 ① 동기 인스턴스화가 거부되며 **실패 시도 자체가
  Store를 래치**시키고(latch-then-validate) ② async 인스턴스화 후에도 동기-lift에서 블로킹하면 "cannot
  block a synchronous task" trap — **브리지 표면을 동기-WIT로 한정하는 한 동기 규범 무손상, 넘는 순간
  Store 단위 전염**.
- 예비 경로 [실측: E4]: 전면 async 재배선(p1 `add_to_linker_async` + `func_wrap_async` +
  `Instance::new_async`)은 게스트 무변경으로 성립 — 단 침습적(§5 참조).

## §2. 제품 아키텍처 (구현 실물 — 리뷰 2렌즈 독립 재실행 검증)

- **cargo feature `p3-bridge` + CLI `--p3-bridge`** (hosts/otter-host-wasmtime). 스톡 빌드는 플래그를
  **시끄럽게 거부**(침묵 무배선 실행 차단). feature 격리 [실측: cargo tree — 스톡 그래프에 semver-면제
  p3 모듈 미컴파일; p3-bridge에서만 `wasmtime-wasi/p3` + `component-model-async/-bytes/wat` 추가].
- 같은 동기 Store에 p3 데모 컴포넌트(임포트: `wasi:clocks/monotonic-clock@0.3.0` +
  `wasi:random/random@0.3.0`, `wasmtime_wasi::p3::add_to_linker`로 배선) + 코어 게스트 브리지 임포트
  `otter_p3.fetch_stamp(dst: u32) -> i32(status)` — 호스트 클로저가 컴포넌트 `stamp: func() -> u64`
  export를 재진입 호출하고 값을 게스트 메모리 dst에 LE 기입.
- **방향성 정직 기록**: 실측된 프록시 방향은 **코어→컴포넌트 1방향**이다. 역방향(컴포넌트 임포트를
  호스트가 받아 코어 인스턴스로 되돌리는 형상)은 동일 메커니즘으로 성립할 것으로 보이나 [추론 — 모드
  문서 경로 12의 원 서술은 API-표면 근거 추론이었음] **미제품화·미실측**. 필요 시 별도 스파이크가 게이트.

## §3. 계약

1. **J-모드 §5.1/§5.3 무변경** [실측: 리뷰가 게스트 3플레이버 임포트 표면 wasm-tools 대조 —
   emit/t0 불변, p3 플레이버만 `otter_p3` 추가; otter_jit 배선 diff 무접촉]. tsavorite 영향 0.
2. **브리지 표면 화이트리스트(호스트 계약)**: 브리지가 노출하는 p3 표면은 **동기-WIT 함수 한정**(MUST).
   근거는 §1-S2-E5의 래치 전염 실측. wasmtime-wasi 48.0.1 p3의 동기 표면 = cli/clocks/filesystem-types/
   random/sockets-types 대부분(bindgen상 동기 호스트 함수); sockets 일부와 스트림 I/O·wait-*는
   async-WIT라 **금지 대상** [실측: M4].
3. **오류 부호-클래스**: 브리지 임포트의 반환 status는 §5.1 준용 — 0 = 성공, 음수 = 실패(부호만 규범,
   세분값은 진단 SHOULD). 게스트 메모리 기입은 호출자가 넘긴 dst 구간(고정 폭)만.
4. 공유-메모리(O-17) 게스트는 v1 브리지 비대상(-4류 명시 거부) — 데모 표면 한정 스코프.

## §4. §5.5 규범 갱신 (o13 설계 문서 부기와 쌍)

o13 설계 §5.5의 "async 구성 Store(`Config::async_support(true)`)" 서술은 48에서 **낡음**(no-op).
갱신 규범: **"이 Store가 async 래치(`async_required`)를 밟지 않는 한 동기 계약이 유지된다 — async-향
요소(async 호스트 함수의 실링크, async limiter/call-hook/fuel-yield)를 게스트 Store에 절대 들이지
않는다. 위반 발현 = 이후 모든 동기 진입점이 validate_sync_call로 거부(§5.3-3 동기 Instance::new 포함 →
전 compile -2 → 침묵 전면 T0 강등)."** 매 실행 기계-고정: `HOST-P3-STORE-SYNC-OK` 프로브(게스트 실행
후 동기 인스턴스화 1회 시도)가 래치 미접촉을 단언한다.

## §5. E4 예비 경로 규범 (async-WIT가 필요해지는 날)

브리지가 async-WIT p3 기능(스트림 I/O, sockets 비동기, wait-*)을 요구하게 되면: 전면 async
재배선(E4 — 성립 실측됨)으로 이행하되, **선행 게이트 2건**: ① §5.3-3 동기 `Instance::new` 규범의
async 변형(`Instance::new_async`) 별도 규범화(§5.5의 async 임베더 조항 구체화), ② tokio 류 런타임
도입의 호스트-트레인 영향 평가. 그 전까지 sync-WIT 한정이 정본.

## §6. 릴리즈 보안 게이트 등재

`wasmtime-wasi` 48.0.1 p3 모듈의 자기선언 [실측: p3/mod.rs 축자]: "Experimental, unstable and
incomplete… not compliant with semver… Bug and security fixes limited to wasip3 will not be given
patch releases". **등재 사항**([[release-rowhammer-gate]] 메모리에도 기록): p3-bridge feature가 컴파일된
호스트 배포는 semver-면제·보안패치-비보장 표면을 포함하므로 매 testing/stable 릴리즈에서 (i) 이 표면의
업스트림 이슈 점검, (ii) =48.0.1 락 핀 유지·범프 시 S2 경계 실측(E5) 재실행이 의무다. 완화: 스톡
빌드에는 이 표면이 컴파일되지 않음(feature 격리 [실측]).

## §7. 게이트 정의

`just wasm-e2e-p3` [전건 그린, 리뷰 2렌즈 독립 재실행]: `E2E-9-P3BRIDGE-OK`(스탬프 3종 신선·쌍별
상이 — clocks/random 실호출 증거) · 이중 회계(HOST-COMPILE 2==2==2 + BRIDGE-CALLS 3==3 — 티어업과
브리지의 **인터리빙** 공존; 승격 함수의 wasm-emit 도달은 게스트-내 tier_of 단언 + 호스트 컴파일 계수
2채널) · `HOST-P3-STORE-SYNC-OK`(래치 프로브) · `NEGPROBE-P3-UNWIRED-OK`(무배선 음성 — unresolved
import를 정확 명명) · 호스트 단위 8종(제품 Config 하 p3 라운드트립+래치). CI: wasm-eh-pinned 레인
BLOCKING 스텝, 카나리아 제외(브리지는 stable 호스트 측 — nightly 드리프트 면 없음; 게스트 플레이버
빌드는 §7 파이프라인 공유).

**정직 한정**: "인터리빙"이다 — 브리지 호출 pre/between/post + 코어 프레임 활성 중 컴포넌트 재진입은
실측이지만, *승격된 otter 함수 내부에서* 브리지를 호출하는 형상은 아니다(Host* IR 빌트인 경유 브리지는
데모 스코프 밖).

## §8. 목표 ① (i)~(iii) 충족 지위 — 정의 복원과 판정

**정의**(모드 연구 reframe 렌즈 §④ — 여기 정본 복원): 목표 ①(CM+WASIp3)의 **임베디드 해석** —
(i) p3 서비스 사용, (ii) 서드파티 컴포넌트 소비, (iii) 프로세스-내 배선. [배포-아티팩트 해석
(iv) WIT 월드 구현체 (v) WAC compose (vi) 레지스트리 (vii) 범용 CM 호스트 — **O-16c′가 충족**, 모드
문서 §4.3 기갱신.]

**판정: 아키텍처-수준 충족 — 3중 단서 동반** [리뷰 2렌즈 합치]:
- (i) p3 서비스 사용: 충족 — wasi:clocks/random 0.3.0이 브리지 경유로 게스트에 실공급 [실측].
- (ii) 서드파티 컴포넌트 소비: 충족 — 데모 컴포넌트가 그 실물(p3 월드 임포트를 가진 표준 컴포넌트를
  호스트가 적재·배선·호출) [실측].
- (iii) 프로세스-내 배선: 충족 — 같은 Store, 재진입 호출, JIT 생존 [실측].
- **단서** ① S1로 직접-게스트 경로가 폐기돼 브리지가 유일 경로(대안이 아니라 전부) ② 표면은 sync-WIT
  한정(§3-2) ③ p3 호스트 표면은 =48.0.1 핀에 결박(§6). 실행된 표면은 5패키지 중 2패키지의 동기 함수
  2개 — **아키텍처-수준 충족이지 폭(breadth)-수준 충족이 아니다**. 폭 확장은 수요 발생 시 §3-2
  화이트리스트 안에서 증분.

## §9. 워치·잔여

1. **wasip3 rustup 프리빌트 std 출현**(S1 재평가 트리거) — 모드 문서 §1.4 등재.
2. **p3 semver-면제 표면**(§6) — 모드 문서 §1.4 등재 + 릴리즈 게이트.
3. E4 이행 조건(§5) — async-WIT 수요 발생 시.
4. 역방향 프록시(§2 방향성) — 수요 발생 시 스파이크 게이트.
