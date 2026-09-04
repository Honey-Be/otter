Status: SPIKE RESULTS (2026-09-04 — O-19 설계 입력의 영속화; 설계 §7 C0의 "휘발성 증거 정본화" 이행)

# O-19 스파이크 결과 — S1(otter T0 wasm64 실물) + S2(memory64 방출 프로브)

작업 실물: 스크래치 `o19-s1/`(게스트 3형상 + 호스트 + run-final.log 30줄) · `o19-s2/`(main probe +
wasmtime 45 sub-probe). 도구: nightly-2026-08-31(§7 핀) / wasmtime 48.0.1(+45.0.3 교차) /
wasm-encoder·wasmparser 0.258. 본 문서는 두 스파이크 보고의 정본 요약이며, 설계
`otter-o19-wasm64-design.md` v2의 [실측] 귀속이 여기 걸린다.

## §1. S1 — otter T0의 wasm64 실물 (M1–M4 전부 그린 + 예상 뒤집기 2건)

| 항목 | 결과 |
|---|---|
| M1 빌드 3형상 | otter 라이브러리(no_std+alloc+half): `wasm64-unknown-unknown -Zbuild-std=core,alloc` **소스 무수정 그린**. no_std 게스트: bump 할로케이터+panic_handler(단 `core::arch::wasm64` 전체 unstable — rust#90599, wasm32와 비대칭). std 게스트: `-Zbuild-std=std,panic_abort` 그린(I/O 무동작). 문서화 패닉 → `UnreachableCodeReached` trap |
| **M1-보너스 — 전제 ⑦ 소멸** | **rustc 문서 "wasm64 panic=unwind 미지원"은 §7 핀에서 스테일**: `-Cpanic=unwind -Zbuild-std=std,panic_unwind`+exnref 빌드·링크 성공, try_table/throw_ref 154개, validate 통과, **런타임 catch_unwind 작동**. exnref tag `(param i64)`(포인터 폭 추종). **otter-engine 자체가 wasm64 탑승 — O-12 fail-closed(try_dispatch=Err(Trap)+clear_trap 재사용)가 게스트 안에서 실증**. 단 핀-실측이지 업스트림 계약 아님(rustc 문서는 여전히 미지원 선언) — 엔진-on-wasm64는 '미착수 백로그'로 정직 분류, 채택 시 W-4 2레인 카나리아 규율 편입 |
| M2 wasmtime L1 | 48.0.1 인스턴스화·실행 그린. 검증 2경로: typed 반환 == 네이티브 오라클(1001007) / 게스트 반환 주소로 memory 8바이트 LE 판독 == 오라클. **48은 memory64 기본 ON**(연구 §1.4 "옵트인 기본 false"는 스테일 — 명시 `wasm_memory64(false)`만 거부: "memory64 must be enabled for 64-bit tables") |
| M3 전제 ⑤ | **fn-ptr==테이블-인덱스 wasm64 성립** — `(table i64 25 25 funcref)`(table64), elem 베이스 (i64.const 1), fnptr 값 1/2(wasm32 동일 레이아웃), 호스트 table.get(fnptr)==게스트 간접호출==네이티브 미러. **정체성 이월, 폭만 파라미터화 대상**. 소비자 판정: 엔진의 기계적 배제가 소멸했으므로 '조건부 이관' 철회 — 실질 소비자(J-mode-on-wasm64)는 전망상 존재하나 그 작업은 오너 문안대로 게이트 밖('미착수'로 분류) |
| M4 성능 | T0 디스패치 2천만 인스트: native 256ms / wasm32 356ms / wasm64 523ms — **w64/w32 = 1.45–1.47×**(문서 밴드 1.2–1.8× 내). 크기 +8.5%. 전환 동기 음수 유지 — 게이트 근거는 성능이 아니라 오너 스코프 결정 |

이탈 기록: 사본 path-dep 절대경로화(레포 무수정), rustup wasip2 타깃 추가(존치·무해)와 sysroot 심링크
심(측정 후 제거·원상복구), Franken-스펙(-cooperative-threading 제거)은 형상 측정 전용 — 결론 무의존.
tmpfs 고갈로 본 세션의 완료된 과거 스파이크 target만 삭제(소스 보존).

## §2. S2 — memory64 방출 프로브 (M1–M4 전부 [실측])

| 항목 | 결과 |
|---|---|
| M1-1/2 | `(memory i64 0)` 임포트+i64 주소 store/load 유효·실행. **주소 상수 I32Const는 validator가 거부**("expected i64, found i32") — I64Const 강제(§2.2 #2/#3의 근거) |
| M1-3/4 | **혼합형(memory64 + table64:false) 유효·실행** — addrtype은 메모리별/테이블별 독립, **테이블·call_indirect 기계 무변경**. table64는 별개 옵트인이 아니라 MEMORY64 피처가 겸장; table64=true면 elem offset i64.const 강제 |
| M1-5 | memarg offset 2^32: mem64 유효 / mem32 거부. `MemArg.offset`은 이미 u64 — API 변경 불요 |
| M1-6 | **wasmtime 게이팅 정정**: 45.0.3·48.0.1 **양 트레인 default Config로 memory64/table64 수용** — §1.4 "옵트인" 서술 이중 스테일. L1 실행에 Config 변경 0(단 구현은 명시 `wasm_memory64(true)`+음성 대조 권고 — 기본값 재반전 방어) |
| M2 | otter-wasmemit 주소-폭 지점 전수 11건(파일:행 목록은 원 보고 — COMPILED_TY 폭/marshal·table_call·tailcall 스크래치 상수/push_memory_import/Ctx·Refusal·AotOpts·AotScratchEntry·scratch_region u64화/커서·경계/SHARED_MEMORY_IMPORT_MAX_PAGES는 wasm32 전용 → **shared×memory64 v1 fail-closed 거부 권고**). 불변 지점: call_indirect 슬롯·I32WrapI64·선언 테이블·elem offset·BINOP/UNARY/HOST* 타입·br_table·MemArg 구조·JIT 표면(DISPATCH_TY/memory_and_table_imports — EmitOpts 무확장, **A64는 AotOpts 전용 노출**) |
| M3 소비자 | JIT 계약 v1 비대상(엔진 wasm64 탑승은 백로그 — S1과 정합). **AOT×b′ 정적 링크 차단**(b′=wasip1/32-bit; wasi-libc wasm64 부재 = L2 외부 차단과 동일 뿌리) ⇒ v1 승인 형상 = pre-link 산물 + 호스트 직접 헬퍼 임포트. **L-2 memory64 거부와 충돌 없음** — 거부 지점(owned_memory)은 메모리-소유 모듈 전용, O-19 형상은 임포트라 미도달; manifest memory64 필드+invariance 검사 신설 권고(→ 설계 C5) |
| M4 | 실행 왕복 전건 PASS(i64 주소 store→load, baked i64.const 주소, 혼합형, table64). 호스트 수신측은 `MemoryType::new64` 필수(32-bit 메모리는 링크 거부 — 전제 ⑥) |

## §3. 32-bit 전제 7건 처분 (설계 §5의 근거)

① 계약 ptr/len u32 — JIT v1 비대상(엔진 백로그 종속) · ②③④⑥ — addrtype 파라미터화로 해소(C1/C2) ·
⑤ — 성립 확인, 폭만 이월(무변경) · ⑦ — 핀-실측 소멸(업스트림 계약 아님 — 백로그 채택 시 카나리아).

## §4. 정본 문서 정정 대상 (→ 설계 C6 스윕)

`otter-wasm-scope-escalation-research.md` §1.4: (i) "wasm64 panic=unwind 미지원 [치명 캐비앗]" →
핀-실측 그린(문서 스테일, 업스트림 보증 없음) (ii) "Config::wasm_memory64 기본 false — 옵트인" →
45/48 양 트레인 기본 ON 실측. L2(×WASI)·L3(웹 Safari) 차단 판정은 무변경.
