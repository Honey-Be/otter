# O-13 W-0 계약 스파이크 — 결과 정본 (2026-09-02)

Status: **W-0 게이트 그린.** 스파이크 5개 마일스톤 전부 성공 + 3렌즈 적대적 패널(A: 계약 사양, B: 차용-수명, C: 빌드 구성) 전원 "W-0 통과 이의 없음". CRITICAL 결함 0건. 단 패널 요구 중 일부는 **후속 증분의 착수-선결 설계 반영 조건**으로 이월됨(§3 처분표) — W-0.5/W-1a 착수에는 지장 없음(§6).

관련 문서: 설계 정본 `otter-o13-wasm-emit-design.md`(본 결과 반영 수정 §4 참조), 스파이크 코드 `spikes/o13-w0-jit-contract/`(§5).

---

## §1. M1~M5 실측 판정표

| 항목 | 판정 | 핵심 증거 | 패널 독립 재현 |
|---|---|---|---|
| **M1** 계약 왕복 (§5 실물 검증) | **성공** | 손작성 방출 모듈(150B, §5.2 임포트 그대로) + wasmtime 45.0.3 호스트. 게스트 실행 도중 재진입 `Module::new`+`Instance::new`, `table.grow`→72·`set`, `transmute(idx)→CompiledFn` 호출, 방출 코드→메인 모듈 Rust fn `call_indirect` 되부름, 2회째 호출(인스턴스 잔존), compile 1회 — 전 구간 그린 (`f(3,4,cb)=77`) | A(재실행), C(재실행 77/1207) + **A가 EH 스토어에서 M1 전 구간 재실행 그린**(M1/M2 config 조합 공백 소거) |
| **M2** wasip1 EH build-std (§7 최대 불확실성) | **성공** | `nightly-2026-08-31 -Zbuild-std=std,panic_unwind` + `-Cpanic=unwind` → in-wasm `catch_unwind`가 패닉 포획 + 캐치 후 실행 계속. 산물에 표준 exnref(`try_table`/`throw_ref`) 방출. `wasm_exceptions(true)` 미설정 시 파싱 거부 실측(load-bearing 실증) | C(재실행 그린 + 파싱 거부 재현) |
| **M3** §2 프로브 wasip1 재실행 | **성공** | stable 1.96.0, wasip1 vs unknown-unknown — 3행(베이킹 상수/`call_indirect` 타입/CompiledFn `(i32,i32,i64)→i64`) 전부 동일 형태. 차이는 command-export 래퍼뿐(ABI 무관). **§11 위험 1의 W-0 항목 소거** | C(재빌드·재검사 3행 재현) |
| **M4** `Caller::get_export` 테이블 접근 | **성공** | wasmtime 45.0.3에서 `__indirect_function_table`/`memory` 직접 취득 성공 — Store-data 우회 불필요(참조용으로 잔존). **§11 위험 7 소거** | A(소스+실행 재확인) |
| **M5** 스크래치 live 창 분석 | **논증 성립** | "store→복사 완료" 창 내 호스트 임포트·게스트 코드·방출 재진입 경로 전무(코드 전수 독해; `maybe_tier_up`은 복사 후에만 도달; wasm 타깃에서 module_dispatch 호출자는 방출 코드뿐). 단서: 복사의 Vec 할당이 dlmalloc 실행 — 단 live 할당 불가침+`memory.grow` 보존으로 무해 | B(창 분석 재도출 — 옳음; **단 복사 삽입점은 B#1대로 정정 필요**, §3) |

**패널 자체 추가 실측 3건**: (A) `--export-table`만 걸고 `--growable-table` 누락 → 비-growable 테이블과도 **링크 성립**, 실패는 첫 티어업 `table.grow`에서 `-4` — 설계의 "-2로 발현" 서술 반증(설계 수정 반영됨). (A) EH 스토어에서 M1 전 구간 그린. (C) **통합 단일 아티팩트**(guest-main+catch_unwind, nightly build-std EH + 링크 플래그 동시) 빌드·실행 그린 — W-4/W-5 실제 형상의 실증 공백 소거.

---

## §2. §7 빌드 구성 최종 판정

**그린 — §12-1 (b)안(O-12 게이트 wasm 완화) 재상정 불필요.** nightly EH가 wasip1에서 실동작(M2 + 패널 C 재현 + 통합 아티팩트 실험)하므로 O-12 fail-closed 계약이 wasm에서 성립한다.

확정 구성(재현 검증됨):

```sh
# 전제 (누락 시 build-std가 EH와 무관해 보이는 링커 에러로 실패: crt1-command.o / -lc / -lunwind)
rustup toolchain install nightly-2026-08-31 --profile minimal \
  --component rust-src --target wasm32-wasip1

# 게스트 빌드 (배포 아티팩트)
RUSTFLAGS="-Cpanic=unwind -Cllvm-args=-wasm-use-legacy-eh=false \
  -C link-arg=--export-table -C link-arg=--growable-table" \
  cargo +nightly-2026-08-31 build --release --target wasm32-wasip1 \
  -Zbuild-std=std,panic_unwind
```

- `-Cllvm-args=-wasm-use-legacy-eh=false`는 현 핀(rustc 1.100.0-nightly / LLVM 23.1.0)에서 **no-op**(exnref가 이미 기본, 유/무 산물 비트 동일 — 패널 C 실측). 방어용 명시 핀으로 유지하되, LLVM에서 레거시 EH 옵션이 제거되면 하드 에러가 될 수 있으므로 핀 범프 절차(W-4)에서 재평가.
- 호스트: wasmtime 45는 exceptions **off-by-default**(기본 활성은 47+) — `Config::wasm_exceptions(true)`가 load-bearing, crate features는 `["runtime","cranelift","gc","gc-drc","std"]`(`gc`만으로는 collector 부재로 bail).

---

## §3. 패널 결함/요구 전수 처분표

처분 범례: **반영됨** = 본 세션에서 설계 문서 수정 완료(§4) 또는 본 문서에 기록 완료 / **이월(증분)** = 해당 증분에서 흡수, `[선결]` 표시는 그 증분 **착수 전 설계 문서 반영 필수** / **기각** = 근거와 함께 불채택. **기각 0건.**

### 렌즈 A (계약 사양) — 8건

| # | 심각도 | 요지 | 처분 |
|---|---|---|---|
| A#1 | MEDIUM(신규 실측) | `--growable-table`만 누락 시 `-2`가 아니라 첫 티어업의 `-4`로 발현(비-growable과도 링크 성립) — §5.1/§5.2/§5.3 오진단 | **반영됨** — 설계 §1/§5.1/§5.2/§5.3 수정(§4). §8(a)/(c) `-4` 회귀 케이스 추가는 **이월(W-1b 하네스 + W-5 E2E)** |
| A#2 | MEDIUM | 방출 모듈 start 섹션 무규범 — compile 중 게스트 코드 실행 창. "start 섹션 없음(백엔드 보장)" 규범 승격 + 호스트 존재 시 `-1` 거부(MUST, wasmtime shim) | **이월(W-1a[선결: §5.2 백엔드 규범 1문장] + W-5[§5.3 호스트 MUST])** — 저비용 규범 추가, 아키텍처 무변경 |
| A#3 | MEDIUM | §5.5 feature 목록 부정확 — `gc`만으로 부족, `gc-drc`(+shim `std`) 필요(collector 부재 bail, 벤더 소스 확인) | **반영됨** — 설계 §5.5 수정(§4). 스파이크 host의 미사용 `anyhow` 잔재 제거는 **이월(W-5)** |
| A#4 | MEDIUM-LOW | async Store 비호환 — async 구성에서 `Instance::new` bail → 전 compile `-2` → 침묵 전면 T0 강등 | **부분 반영** — §11 위험 7에 기록(§4). §5.5 "동기 Store 전용 + async는 `func_wrap_async`/`new_async`" 규범 문장은 **이월(W-5[선결])** |
| A#5 | LOW-MEDIUM | ptr/len 범위 밖 읽기의 에러 코드 미배정(실물 양쪽 `-1`) — `-1` 정의에 명문화 | **이월(W-6 문서 마감)** — sticky Refused 귀결은 현행대로 올바름 |
| A#6 | LOW | 에러 세분값의 규범 지위 불명(부호만 규범/세분은 진단 SHOULD) + `-5` 생산자 부재 + 글루 SHOULD 매핑 + `-5`=엔진 전역 disable MAY | **이월(W-5 글루/shim + W-6 문서)** |
| A#7 | LOW(개선) | grow+set 2단계 → grow-with-init 1단계(wasmtime/JS 모두 지원, 널 슬롯 창 소거) | **이월(W-5)** — 결함 아닌 사양 단순화; v1 단일 스레드에선 관측 불가 |
| A#8 | INFO | 스파이크 한계 2건: M1 wat이 콜백 인덱스를 게스트 데이터에서 로드(§11 위험 5의 상수-베이킹과 다른 형태 — M3가 별도 실증하므로 잔여 위험 ~0); 보고 #3/#4 재확인 정당 | **반영됨** — 본 문서에 한계로 기록. 추가 조치 불요 |

### 렌즈 B (차용-수명) — 7건

| # | 심각도 | 요지 | 처분 |
|---|---|---|---|
| B#1 | HIGH | 복사-즉시 변형은 **catch_unwind 내부**에서 수행해야 함(tail_signal 미러; 가드만 밖, Err 시 poison) — M5가 기술한 삽입점(catch_unwind 밖)대로면 복사의 패닉 표면이 nounwind abort→활성화 사망. alloc 실패는 별도 문서화(T0 argv와 동급 잔존) | **이월(W-2[선결: §2.5-2 규범 문장 + W-2 게이트 항목])** — M5 창 분석 자체는 유효, 삽입점만 정정 |
| B#2 | HIGH | "add-only → 주소 불변"(§2.5-1/§6)은 틀린 근거 — 주소 안정성은 **힙 간접화**(별도 힙 할당 + 소유 구조체 이동과 독립)에서 옴. 인라인 저장(SmallVec류) 금지 명문화 + CompiledWasmFn 필드 독(pushed_baseline 패턴) + W-3 레이아웃 실물 대조 | **이월(W-2/W-3[선결: §2.5-1/§6 규범 3건])** |
| B#3 | MEDIUM | `target_family="wasm"` cfg가 wasm64 미배제 — wasm64에서 타입 불일치 trap/주소 절단. `target_arch="wasm32"`로 조이거나 compile_error! | **이월(W-3[선결: §6 cfg 규범] + W-1a API cfg)** — arch-tiers 방침(wasm64=T3) 정합 |
| B#4 | MEDIUM | §5.3에 "compile은 호출 인스턴스의 어떤 export도 호출 금지(MUST NOT)" 부재 — M5 논증의 암묵 전제를 규범화 | **이월(W-5[선결: §5.3 1문장])** — 참조 구현은 자명 충족 |
| B#5 | MEDIUM-LOW | 베이킹 주소 provenance 미규정 — `as_mut_ptr()` 1회 채굴 + 이후 내부 참조 형성 금지 + int→ptr exposed provenance 명문화 | **이월(W-2 리뷰 항목 + §6 문서화)** |
| B#6 | LOW | §2.5-4 "심층 방어로 강등" 문구 과도 — 스크래치 외 버퍼(vec/tail_args/owned/엔진 상태)엔 여전히 load-bearing | **이월(W-2 설계 수정과 함께 또는 W-6)** |
| B#7 | LOW | 건전성 쌍대 불변식(방출 코드 유일 기입=자기 스크래치 / live 차용 버퍼와 할당-단위 서로소) 명문화 | **이월(W-2 설계 수정과 함께)** |

### 렌즈 C (빌드 구성) — 7건

| # | 심각도 | 요지 | 처분 |
|---|---|---|---|
| C#1 | MEDIUM | M1/M2가 별개 아티팩트 — 완전한 §7 구성의 단일 아티팩트 실증 공백 → **패널 자체 실험으로 해소(그린)** | **해소됨** + W-4 스모크를 통합 구성으로 명문화는 **이월(W-4)** |
| C#2 | MEDIUM | §7 "`-wasm-use-legacy-eh=false` 필요" 정정(현 핀 no-op) + 핀-범프 파손 벡터(옵션 삭제 시 하드 에러) + 범프 절차 3항(비트-diff/try_table 스모크/플래그 제거) | **반영됨** — 설계 §7 수정(§4). 핀-범프 절차는 **이월(W-4)** |
| C#3 | MEDIUM | `rust-src` 컴포넌트 + `rustup target add wasm32-wasip1`(self-contained wasi-libc)이 build-std 필수 전제 — 미기재 | **반영됨** — 설계 §7에 전제 명기(§4). W-4 절차 원라이너 상세는 **이월(W-4)** |
| C#4 | MEDIUM | §5.5 "최신 wasmtime 기본 활성 → 이중 안전장치"는 핀 45에서 거꾸로(off-by-default, 47+부터 기본) — `wasm_exceptions(true)`가 load-bearing | **반영됨** — 설계 §5.5 수정(§4). try_table 파싱 기동 스모크는 **이월(W-5)** |
| C#5 | LOW-MEDIUM | nightly 날짜 핀 CI 지속가능성 — 아카이브 보존 미문서화 + 컴포넌트 결락 실존 → dist 타르볼 미러 또는 핀-갱신 주기 + CI 2레인(핀 게이팅 + 최신-nightly 카나리아) | **이월(W-4)** — >7년 호라이즌 필수 보강 |
| C#6 | LOW | 보고 수치 모순(343 vs 217 — 서로 다른 grep 패턴; 산물은 비트 동일) + host `anyhow` 잔재 | **반영됨** — 본 문서에 해명 기록(결론 무영향). anyhow 제거는 **이월(W-5)** |
| C#7 | LOW | 스파이크 게스트는 opt-level=1/no-LTO — 배포 대표 프로파일×build-std×exnref 조합 미검증 | **이월(W-4)** — otter-wasm-demo를 배포 대표 프로파일로 빌드 |

### 스파이크 보고 자체의 설계-어긋남 4건

| # | 요지 | 처분 |
|---|---|---|
| R#1 | §7 legacy-eh 플래그 "필요" 서술 | **반영됨**(=C#2) |
| R#2 | nightly `rustup target add wasm32-wasip1` 전제 미기재 | **반영됨**(=C#3) |
| R#3 | wasmtime 45는 anyhow 불사용(`wasmtime::Error`는 `core::error::Error` 미구현, `func_wrap`은 `Result<T, wasmtime::Error>`) + `ValType`에 `PartialEq` 없음 → MUST 타입 검사는 패턴 매칭 | **이월(W-5 구현 노트)** — 설계 원문에 거짓 주장 없음(구현 지침으로 본 문서가 정본 기록) |
| R#4 | §5.5 get_export 우회 예고 불필요(주 경로 성립) | **반영됨** — 설계 §5.5/§11 위험 7 수정(§4) |

---

## §4. 설계 문서 수정 내역 (본 세션, 전부 "W-0 실측 반영" 표기)

`otter-o13-wasm-emit-design.md`에 가한 최소 수정 — 실측으로 사실관계가 달라진 절만:

1. **§1 공유 자원 확정 / §5.1 에러 코드 / §5.3 임베더 빌드 요건**: 링크 플래그 누락의 발현 정정 — 전누락(또는 `--export-table` 누락)→`-2`, `--growable-table`만 누락→링크 성립 후 **첫 티어업의 `-4`** (A#1 실측).
2. **§5.2**: min-0·max-부재 임포트는 비-growable(max=min) 테이블과도 링크 성립(실패는 grow로 지연) 정밀화.
3. **§5.5**: (i) get_export 우회 불필요 — M4 주 경로 성립, Store-data 우회는 참조용 강등; (ii) feature `["gc"]`→`["gc","gc-drc"]`(+shim `std`); (iii) "최신 wasmtime 기본 활성" → "핀 45는 off-by-default, `wasm_exceptions(true)`가 load-bearing(미설정 시 파싱 거부 실측), 기본 활성은 47+".
4. **§7**: (i) legacy-eh 플래그 "필요"→"현 핀 no-op(방어용 명시 핀; 핀 범프 시 재평가)"; (ii) nightly `rust-src` 컴포넌트 + `rustup target add wasm32-wasip1` 필수 전제 명기; (iii) 불확실 항목(wasip1 EH 성숙도) 해소 표기 — M2 그린, §12-1 (b) 재상정 불필요.
5. **§10 규모 추정 말미**: "wasip1 EH가 깨지면 재설계" 최대 불확실성 해소 표기.
6. **§11 위험 1/3/7**: W-0 몫 소거 표기(M3/M2/M4) + 위험 7에 async Store 비호환 신규 기록(A#4).

**미수정(의도적)**: 패널의 신규 규범 추가 요구(B#1/B#2/B#3/B#4/A#2/A#5/A#6 등)는 실측에 의한 사실 정정이 아니라 규범 신설이므로 §3 처분표의 해당 증분 착수 전 설계-수정으로 이월 — `[선결]` 표시가 그 의무를 게이트로 고정한다.

---

## §5. 스파이크 코드 위치와 재현 커맨드

전부 신규 파일, 미커밋, 루트/기존 파일 무변경. 각 하위 프로젝트는 빈 `[workspace]`로 루트에서 분리.

| 경로 (spikes/o13-w0-jit-contract/) | 내용 |
|---|---|
| `wat/emitted.wat` | M1a 손작성 방출 모듈(150B, §5.2 계약 축자) |
| `guest-main/` | M1b 게스트(stable 1.96, wasip1, 링크 플래그) + `src/emitted.wasm` |
| `guest-eh/` | M2 게스트(nightly build-std EH; `target-legacy/`는 플래그 유무 대조 산물) |
| `host/` | wasmtime 45.0.3 호스트(M1/M2/M4 겸용) — `o13-w0-host <guest.wasm> [--eh]` |
| `probe-wasip1/` | M3 프로브(§2 관례 미러) |

재현:

```sh
# M1: guest-main 빌드(stable) 후 호스트 실행
cd spikes/o13-w0-jit-contract/guest-main
RUSTFLAGS="-C link-arg=--export-table -C link-arg=--growable-table" \
  cargo build --release --target wasm32-wasip1
cd ../host && cargo run --release -- \
  ../guest-main/target/wasm32-wasip1/release/guest-main.wasm
# 기대: guest emitted f(...)=77, M1-GUEST-OK / HOST-OK, compile count = 1

# M2: guest-eh 빌드(nightly, §2의 확정 구성 커맨드) 후 --eh로 실행
cd ../guest-eh && RUSTFLAGS="-Cpanic=unwind -Cllvm-args=-wasm-use-legacy-eh=false" \
  cargo +nightly-2026-08-31 build --release --target wasm32-wasip1 \
  -Zbuild-std=std,panic_unwind
cd ../host && cargo run --release -- \
  ../guest-eh/target/wasm32-wasip1/release/guest-eh.wasm --eh
# 기대: catch_unwind 포획 + post-catch OK, M2-GUEST-OK

# M3: probe-wasip1을 양 타깃으로 빌드 후 wasm-tools print 대조
cd ../probe-wasip1
cargo build --release --target wasm32-wasip1
cargo build --release --target wasm32-unknown-unknown
```

패널 추가 실험 산물(스크래치패드, 세션 소멸성): growable-누락 대조 빌드, 통합 단일 아티팩트(guest-full) — 결과는 본 문서 §1이 정본 기록.

---

## §6. 후속 증분 착수 조건

| 증분 | 착수 | 선결 조건 |
|---|---|---|
| **W-0.5** (Neg F16 정본 수정) | **즉시 가능** | 없음 — 사용자 승인 기확정(§12-2 (a), 2026-09-02), W-0 결과와 독립(순수 네이티브 정본 수정). 자체 패널은 증분 내 게이트 |
| **W-1a** (lowering 부트스트랩) | **즉시 가능** | 착수 전 §5.2에 "방출 모듈은 start 섹션 없음(백엔드 보장)" 규범 1문장 반영(A#2 몫). **단 `Un(Neg)` lowering·차등은 W-0.5 완료 선행**(otter_unop 부재) — 직선 Effect/Expr·Bin/Convert·하네스 골격은 독립 진행 가능. wasm-emit API cfg에 wasm32 조임(B#3) 반영 |
| W-1b | W-1a 후 | §8(a)에 `-4` 회귀 케이스 추가(A#1 몫) |
| W-2 | W-1b 후 | **[선결] §2.5-1/2, §6 설계 반영: B#1(복사를 catch_unwind 내부로+poison), B#2(힙 간접화 규범), B#5/B#6/B#7 병행** |
| W-3 | W-2 후 | **[선결] §6: B#2 규범 3건 + B#3 cfg 조임 + 레이아웃 실물 대조 체크리스트** |
| W-4 | — | C#2(핀-범프 절차)/C#3(원라이너)/C#5(미러+2레인)/C#7(대표 프로파일)/C#1(통합 스모크) 흡수 |
| W-5 | — | **[선결] §5.3/§5.5: B#4(export 재진입 금지 MUST NOT), A#2(start 섹션 -1 거부 MUST), A#4(동기 Store 규범)** + R#3 구현 노트 + A#6/A#7 + anyhow 제거 + 기동 스모크 — **전부 이행됨, §7 참조 (2026-09-03)** |
| W-6 | — | A#5/A#6 문서 마감 + B#6(문구 정밀화) 잔여 — **전부 이행됨, §8 참조 (2026-09-03)** |

---

## §7. W-5 이월분 처리 내역 (2026-09-03 — W-5 착지와 함께 기록)

W-5 구현: `hosts/otter-host-wasmtime`(§5.5 참조 shim, 제품 품질 신규 작성 — 본 스파이크 `host/`는
계측 기록으로 무수정 보존) + `demos/otter-wasm-demo` §8(b)/(c) 실스위트 + `just wasm-e2e`. §3
처분표에서 W-5로 이월된 항목의 처리:

| 항목 | 처리 |
|---|---|
| **B#4** (compile의 호출 인스턴스 export 호출 금지 MUST NOT) | [선결] 설계 §5.3-7에 §13-4 문장 본문 승격 완료. shim 구현은 `otter_jit_compile` 함수 독스트링에 MUST NOT을 명기하고 실제로 §5.3-1 읽기와 §5.3-5 table.grow 외 게스트 상호작용 0 (코드 주석 명시) |
| **A#2** (start 섹션 거부 — shim MUST) | [선결] 설계 §5.3-2에 "wasmtime 참조 shim은 start 섹션 모듈을 항상 `-1` 거부(shim MUST)" 반영. shim은 `Module::new` 성공 **후** 원시 바이트 섹션 워크(id 8 스캔, 스캔 이상은 fail-closed 거부)로 이행 |
| **A#4** (동기 Store 규범) | [선결] 설계 §5.5에 규범 신설(async는 `func_wrap_async`+`new_async`, 위반 발현 = 전 compile `-2` 침묵 T0 강등). shim은 전 경로 동기 API + `async_support` 미설정 (wasmtime-wasi p1의 컴파일타임 async feature는 API 표면일 뿐임을 §5.5/README에 명기) |
| **R#3** (wasmtime 45 anyhow 불사용 / ValType PartialEq 부재) | 설계 §5.5에 구현 노트로 정본 반영. shim은 `wasmtime::Error`/`Context`/`Result` 사용, export "f" 타입 검사는 `matches!` 패턴 매칭 |
| **A#6** (에러 세분값 규범 지위) | shim측 몫 이행: 부호만 규범·세분값은 진단 SHOULD 노선으로 구현 — 전 실패 경로가 저층 에러를 stderr에 축자 출력, 링크 플래그 누락 2종의 발현(-2/-4)을 진단 메시지에 명기. 계약 문서 최종 문구화는 예정대로 W-6 잔여 |
| **A#7** (grow+set → grow-with-init) | 설계 §5.3-5에 1단계 형태 채택 반영, shim은 `Table::grow(store, 1, Ref::Func(Some(f)))` — 널-funcref 슬롯 창 소거 |
| **anyhow 제거** (A#3/C#6 잔재) | 제품 shim은 anyhow 의존 0 (Cargo.toml에 부재 사유 주석). 스파이크 `host/Cargo.toml`의 잔재 라인은 "spikes/ 무수정 보존" 원칙(계측 기록 동결)에 따라 그대로 두고, 제품 크레이트가 대체 — 스파이크는 W-5부로 superseded |
| **기동 스모크** (C#4 — try_table 파싱 수용) | `just wasm-e2e`가 겸함: §7 구성 게스트(산물에 exnref 실재)를 shim(`wasm_exceptions(true)`)이 파싱·기동하고 in-wasm catch_unwind가 E2E-4/E2E-6에서 실동작 |
| **A#1 몫** (§8(c) `-4` 회귀) | W-1b 하네스 몫은 기이행; E2E측은 정상 경로 그린으로 충족, 링크-플래그 누락 발현 2종은 shim 진단 메시지 + README로 문서화 |

E2E 실측 결과(수락 7항 마커·계수 전부)는 설계 문서 §10 W-5 행이 정본.

---

## §8. W-6 이월분 처리 내역 (2026-09-03 — W-6 착지와 함께 기록, 처분표 마감)

| 항목 | 처리 |
|---|---|
| **A#5** (ptr/len 범위-밖 읽기의 계약 지위) | 설계 §5.1 `-1` 정의에 명문화 완료 — "ptr/len이 게스트 linear memory 범위를 벗어나는 읽기 요청도 `-1` 클래스; 참조 shim은 할당 **전** 경계검사 후 `-1`, 게스트 부작용 0". shim 구현은 W-5 보안 수정에서 기이행(`hosts/otter-host-wasmtime` 선-할당 경계검사), 계약 문서 마감이 본 항목 |
| **A#6** (에러 세분값 규범 지위 — 계약 문서 최종 문구화) | 설계 §5.1 서두에 규범 문단 신설: **부호만 규범**(어떤 음수든 엔진 처리는 sticky Refused로 동일), **세분값 배정은 진단 SHOULD**. shim측 몫은 W-5 §7에서 기이행 — 이로써 A#6 완전 마감. 브라우저 글루(`hosts/otter-glue-js`)의 §5.1 인용도 이 문단이 실재 근거가 됨 |
| **B#6** (§2.5-4 문구 정밀화) | **W-2/W-3 시점 기이행 확인** — §2.5-4는 "스크래치 건전성"으로 기스코프 조정됐고 §13-6이 규범을 흡수. W-6 몫 없음(W-6 리뷰 렌즈 B 재확인) |

이로써 W-0 계약 스파이크 처분표의 전 항목이 마감됐다. O-13 잔여는 오너 최종 승인(설계 §14.4)뿐.
