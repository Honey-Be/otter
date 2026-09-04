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
| D8 | exnref-프리 게스트 변형 | Wasmi 임베디드 호스트 전제 | 구현 또는 폐기 결정 | 오너 결정 + tsavorite 수요 회신(문의 발신 상태) |
| D9 | 대안 런타임 후속 | WasmEdge 0.18 안정판 = 제2 호스트 재평가 트리거; `wasmi_ir` 벤더링은 W1 설계 시 재론; 차용 후보 6~20위 미채택분 | 3건 각각 처분 결정 | C-4 연동 / W1 착수 |
| D10 | perf 유보 4종 | float 네이티브 하강(NaN 위험), 범프 인자-스택, 구조적 제어흐름, JIT측 `return_call` | 4건 각각 구현 또는 폐기 결정 | D1의 NaN 결정(float 항목) |

## 운영

- 본 레지스터는 O-W 분기 검토 때마다 상태를 갱신한다(워치리스트 감시 결과 반영).
- 항목 완료 시 이 표의 상태를 갱신하고 완료 근거(커밋/문서)를 링크한다.
- 마지막 항목이 `done`이 되는 커밋에서 major bump를 **같은 PR/커밋 열차로** 집행한다(지연 금지 — "즉각").
