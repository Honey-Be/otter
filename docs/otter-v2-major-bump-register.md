# otter v2 major-bump 레지스터

- 근거: 오너 결정 (2026-09-04) — **"C/D1~D4/D6~D10: 이것들을 전부 결정/구현 완료하면 즉각 major bump."**
- 규칙: 아래 모든 항목의 상태가 `done`(구현 완료 **또는** 근거가 기록된 최종 처분 결정)이 되는 즉시
  **v2.0.0으로 major bump**한다. 부분 완료는 minor/patch로 흡수한다.
- 제외(오너 지정): D5(AOT+JIT 조합 — J+A 완성 시 추가 조치 없이 암시적 완성, 작업 항목 없음),
  D11(소항목 위생 잔여 — 본 레지스터 비귀속).
- 전거: stable-승인 심사용 미결/한계 전수 목록(2026-09-04, 세션 정리)의 C·D절.
  각 항목의 기술 정본은 `.claude-research-library/`의 해당 설계 문서.

## C. 조건부 항목 (외부 이벤트 대기 — 워치리스트 제도화 완료)

완료 기준(공통): 외부 이벤트 **발화 시 구현 완료**, 또는 상류 제안 사망/방향 전환 등으로
**근거가 기록된 폐기 결정**. "대기 지속"은 완료가 아니다. 워치리스트 감시는 O-W 분기 검토에 귀속.

| # | 항목 | 감시 대상 | 발화 시 조치 | 상태 |
|---|---|---|---|---|
| C-1 | `func.new`(jit-interface) Phase 2 + 엔진 구현 | wasm 제안 단계 + 주요 엔진 셋업 | 컴포넌트-내 JIT 부활 경로 착수 | watching |
| C-2 | CM#401 dlopen WIT | component-model#401 | 동적 링크 경로 재설계 검토 | watching |
| C-3 | CM#423 런타임 인스턴스화 | component-model#423 | 컴포넌트 런타임 인스턴스화 채택 검토 | watching |
| C-4 | WasmEdge 컴포넌트 "실행" 노출 | WasmEdge 릴리즈 노트 | 제2 호스트 후보 재평가 | watching |
| C-5 | wasmtime 코어자원 API (#9466) | wasmtime#9466 | 리소스 기반 계약 표면 검토 | watching |
| C-6 | wasmer/Endive CM 성숙 | wasmer 릴리즈 | 제2 호스트 후보 재평가 | watching |
| C-7 | b′ +atomics stable 경고→에러 전환 | rustc 릴리즈 노트 | b′ 빌드 파이프라인 대응(§ L-2 정본) | watching |
| C-8 | wasip3 rustup 프리빌트 std 출현 | rust-lang 릴리즈 | wasip3 직접 게스트 빌드 재시도(현 3중 링크 벽) | watching |
| C-9 | wasmtime-wasi p3 안정화/semver 편입 | wasmtime 릴리즈 노트 | =48.0.1 핀 결박 해제 + p3 표면 재평가 | watching |
| C-10 | wasm64×WASI 외부 차단 | wasi-libc PR#444 | wasm64 게스트 std 경로 개통 검토 | watching |
| C-11 | wasm64×컴포넌트 외부 차단 | component-model#22 | A64 산물 컴포넌트화 개통 검토 | watching |
| C-12 | 스레드 간 공유 funcref 테이블 | shared-everything-threads Phase 1 | L2 멀티스레드 엔진 선결 해소(D3 연동) | watching |

## D. 의도적 이연 (수요/결정 대기 — stable 비차단 기결정)

| # | 항목 | 내용 | 완료 기준 | 선결 조건 |
|---|---|---|---|---|
| D1 | O-15 | M2b 엔진-리스 산술 자립(네이티브-op 하강 + f16 정적 링크 + Host* 임포트 역전) + NaN divergence 결정 | 구현 + NaN 정책 결정 기록, 또는 M2a 프로파일 영구 채택 결정 | 없음(병렬 perf 트랙) |
| D2 | O-18 | IR atomics | 구현 또는 폐기 결정 | L2 결정(D3) 선결 |
| D3 | L2 | 멀티스레드 엔진(Cell/RefCell 대공사) | 구현 또는 폐기 결정 | C-12(외부 Phase 1) |
| D4 | 엔진-on-wasm64 | 핀-실측 그린(unwind 포함)이나 업스트림 계약 아님 — 백로그 | 채택(카나리아 편입 포함) 또는 폐기 결정 | rustc wasm64 unwind의 계약화 관찰 |
| D6 | E4 | 전면-async 재배선 | 구현 또는 폐기 결정 | §5.3 async 변형 규범화 + tokio 평가 (async-WIT 수요 발생 시) |
| D7 | jco / H-component | jco 사이드채널(비게이트) + H-component 지정 폴백 | 각각 채택/폐기 처분 결정 | 수요 관찰 |
| D8 | exnref-프리 게스트 변형 | Wasmi 임베디드 호스트 전제 | **done — 폐기 결정**(오너, 2026-09-04) | tsavorite 수요 없음 확정(2026-09-04 회신); 재개 조건 2건은 아래 부기에 존치 — 발화 시 항목 재개 |
| D9 | 대안 런타임 후속 | WasmEdge 0.18 안정판 = 제2 호스트 재평가 트리거; `wasmi_ir` 벤더링은 W1 설계 시 재론; 차용 후보 6~20위 미채택분 | 3건 각각 처분 결정 | C-4 연동 / W1 착수 |
| D10 | perf 유보 4종 | float 네이티브 하강(NaN 위험), 범프 인자-스택, 구조적 제어흐름, JIT측 `return_call` | 4건 각각 구현 또는 폐기 결정 | D1의 NaN 결정(float 항목) |

### D-표 부기 (외부 입력 기록)

- **D8** — tsavorite 회신(2026-09-04, `re-1788506870-d8-exnref-and-b10-regions.md`): **수요 없음.**
  근거: 그들의 "코드 생성 금지 플랫폼" 자리는 자사 CPS 인터프리터 네이티브 정적 링크가 이미 덮고,
  Wasmi 경로는 이중 인터프리테이션이라 열세. **답을 바꿀 조건 2건(그들 명시, 축자 기록)**:
  ① 그들의 타깃 목록에 no_std 임베디드가 진입하는 경우(현 `docs/targets.md` 기준 전 타깃 std 보유,
  진짜 no_std wasm 타깃 `wasm32v1-none`은 목록 밖); ② 그들이 otter를 wasm 게스트로 돌리는 방향으로
  전환하는 경우(현 계획은 `wasm32-unknown-unknown` 인프로세스 T0 고정). 조건 발화 시 D8 재개.
- **D1 (O-15) 착지 시 H4 연동 의무** — O-15(D1) 착지 시 otter-wasmemit의 방출-opcode
  화이트리스트/코퍼스 확장(bin/un/convert 계열 네이티브-op 하강분)을 **같은 변경에** 포함할 것 —
  H4 인벤토리 게이트의 집합-동등(코퍼스 전체 합집합 == 화이트리스트) 유지가 조건이라, 화이트리스트만
  넓히고 코퍼스를 안 넓히면(또는 그 역이면) 게이트가 적색이 된다.
- **D1 (O-15) 연관 장래 요청 명명** — tsavorite가 cranelift-object 요청을 검토 후 **비요청**으로
  결정하며 남긴 명명 규약: 장래 요청하게 되면 "cranelift-object 채택"이 아니라 **"A-모드
  `HostImportName` 반전을 네이티브 오브젝트 타깃으로 확장"**으로 요청하겠다고 함(진짜 작업이
  `Expr::Host1/Host2` 함수 포인터의 재배치 전환이며 그것이 D1에 이연되어 있음을 정확히 파악).

## 운영

- 본 레지스터는 O-W 분기 검토 때마다 상태를 갱신한다(워치리스트 감시 결과 반영).
- 항목 완료 시 이 표의 상태를 갱신하고 완료 근거(커밋/문서)를 링크한다.
- 처분 이력: **D8 = 첫 done 항목**(폐기, 오너 2026-09-04 — 외부 입력: tsavorite 수요 없음
  `re-1788506870-d8-exnref-and-b10-regions.md`; 재개 조건 2건 부기 존치). 잔여: C-1~C-12 전건 +
  D1~D4·D6·D7·D9·D10.
- 마지막 항목이 `done`이 되는 커밋에서 major bump를 **같은 PR/커밋 열차로** 집행한다(지연 금지 — "즉각").
