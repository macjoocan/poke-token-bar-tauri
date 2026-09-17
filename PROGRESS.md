# PokeTokenBar → Windows(Tauri) 포팅 진행 현황

> 업데이트: 2026-07-22 · 기준 문서: [포팅계획서](PokeTokenBar_Windows_Tauri_포팅계획서.md)

## 착수 결정 (확정)
- 트레이 표시: **동적 아이콘 + 툴팁** (계획서 §4-1)
- 프런트엔드: **React + TypeScript**
- 배포 채널: 미정 (M5에서 결정)

## 마일스톤 상태

| 단계 | 내용 | 상태 |
|---|---|---|
| M0 | Windows CLI 로그 경로/포맷 실측 | ✅ 완료 ([검증서](M0_스파이크_로그경로_검증.md)) |
| — | 원본 소스 확보 (`upstream/`) | ✅ 완료 (7,195 LOC, 테스트 19) |
| — | 툴체인 설치 (Rust MSVC + VS BuildTools + SDK) | ✅ 완료 |
| — | Tauri 프로젝트 스캐폴딩 (`poketokenbar/`) | ✅ 완료 |
| M1 | 코어 파서 Rust 이식 + 테스트 | ✅ 완료 (검증) |
| M1 | 게임규칙(밸런스·희귀도·캔디·샤이니·메타몽) + 증분캐시 | ✅ 완료 (검증) |
| M1 | 스토어 동작(applyUsage 진화·졸업, useMint, buy) | ⬜ M2로 이관 (영속·네트워크·RNG 필요) |
| M2 | CompanionStore(applyUsage·상점·민트·새알·부화·표시상태·영속·RNG) | ✅ 검증 (22 테스트) |
| M2 | Claude 공식 한도(OAuth) — 팀 요금제 실측 검증 | ✅ (oauth_limits, 3 테스트) |
| M2 | PokéAPI 진화라인/base 인덱스 fetch | ✅ (poke_api, blocking, 3 테스트) |
| M2 | 컴패니언 앱 연결(Tauri state·refresh_companion·부화 구동) | ✅ (실기 — 알 인큐베이션 확인) |
| M2 | Codex 한도·usage_store 정밀 집계 | ⬜ (후속, Claude 우선) |
| M3 | UI(팝오버 4탭·연출) | 🟡 진행 중 (오늘 사용량 패널 슬라이스) |
| M4 | 트레이 PoC (동적 아이콘+툴팁+클릭 토글) | ✅ 검증 (실기 실행) |
| M4 | 트레이/OS 통합 나머지(자동시작·알림·자격증명) | ⬜ |
| M5 | 배포 | ⬜ |

## M1+M2 코어 — 검증 완료 (원본 패리티)

`poketokenbar/src-tauri/src/core/` 에 이식, `cargo test --lib` **66/66 통과**:

| 모듈 | 원본 | 검증 |
|---|---|---|
| `model_pricing.rs` | ModelPricing.swift | 단가 정확매칭·패밀리폴백·0 |
| `token_formatter.rs` | TokenFormatter.swift | compact/grouped/cost/percent |
| `models.rs` | Models.swift | ISO8601(밀리/마이크로초·오프셋 정규화), 집계 구조체 |
| `local_usage_reader.rs` | LocalUsageReader.swift | Claude dedup-keep-max·일자비용·Codex·기간·5h 활성블록·날짜유틸 |
| `local_usage_cache.rs` | LocalUsageCache.swift | 증분 재파싱·zlib 압축/평문폴백·40일 prune·60s throttle·디스크 라운드트립 |
| `companion_model.rs` | CompanionModel.swift(+Store 정적함수) | 밸런스(졸업총량·단계임계)·Rarity·EvoNode·이름폴백·캔디지급(엣지)·rollsShiny·dittoDisguiseHit·상태 serde |
| `companion_store.rs` | CompanionStore.swift | applyUsage(진화·졸업)·상점(잔액·정렬·구매)·민트·새알 리롤·부화(가중선택)·표시상태·영속·SeededRng(SplitMix64) |
| `oauth_limits.rs` | OAuthLimitsProvider.swift | Claude 공식 한도(%) — `.credentials.json` 토큰 → `/api/oauth/usage` → five_hour/seven_day/limits[] 파싱 + planDisplay |
| `poke_api.rs` | PokeAPIClient.swift | 진화라인(species→chain→트리·희귀도·다국어이름)·base 인덱스(GraphQL+30일 디스크캐시)·REST 폴백 (blocking reqwest, SSRF 가드) |

**실데이터 스모크** (`cargo run --example smoke`): 오늘 75.5M tokens / $64.83, Claude 3480·Codex 10 엔트리 파싱 확인.

### 이식한 원본 테스트 (파일 → Rust)
LocalUsageReaderTests · LocalUsageCacheTests · ModelLogicTests(모델부) · CompanionTests(밸런스부) · RareCandyTests(evaluateCandyGrants) · ShinyCharmTests(rollsShiny) · DittoTests(dittoDisguiseHit) · ShopTests(가격).

### M2로 이관한 테스트 (스토어·네트워크·영속 필요)
MintTests · FreshEggTests · ShopTests(구매 동작) · CompanionDisplayStateTests · UsageStoreTests · UpdateCheckerTests · BinaryLocatorTests · OAuth/Codex 한도 파생 · Performance.

## M4 트레이 PoC — 검증 완료 (실기 실행)

`lib.rs` 의 `setup` 에서 `TrayIconBuilder` 로 트레이 생성, 별도 스레드 타이머가 500ms마다
`tray.set_icon(make_frame(...))` 로 아이콘 교체(색상회전+게이지 애니메이션), 5s마다 실사용량으로
툴팁 갱신, 좌클릭 시 창 show/hide 토글. 실기 실행 확인(PID 살아있음, 패닉 없음).

- **결론**: macOS 메뉴바 "아이콘+텍스트" → Windows "동적 아이콘+툴팁" 대체 실현 가능. 최대 UX 리스크 해소.
- 아이콘은 파일 없이 코드로 raw RGBA 생성(`Image::new_owned`) → 스프라이트는 M3에서 PokéAPI PNG 로 교체.

### 실행 방법 (dev)
```
cd poketokenbar
npm install                       # 최초 1회 (Node 20.19+ 권장, 20.12 도 경고만 뜨고 동작)
# cargo 가 PATH 에 있어야 함:
PATH="$USERPROFILE/.cargo/bin:$PATH" npm run tauri dev
```
빌드만: `cargo build --manifest-path src-tauri/Cargo.toml` · 테스트: `cargo test --lib` · 스모크: `cargo run --example smoke`

### 이식 시 확정한 원본 동작 (주의점)
- Claude dedup: `(message.id|requestId)` 기준, **total 최대 항목 유지**(스트리밍 부분 output 과소집계 방지).
- Codex: `last_token_usage` 턴 델타 합산. input=total−cached, cacheRead=cached, cacheWrite=0.
- `cost_compact`: `usd<10_000` 이면 `$1200`(K 표기는 1만 이상부터) — 원본 주석 예시(`$1.2K`)에 오해 소지.
- 모듈명 `core` 는 std `core` 크레이트와 충돌 → 크레이트 루트에서 `crate::core` 로 명시.

## M1 완료 — 순수 로직/파서 코어 확보

파서·게임규칙·모델·캐시가 원본 패리티로 검증됨(41 테스트). 순수 함수로 뽑히지 않는 나머지
(스토어 상태머신·네트워크·영속·RNG)는 M2에서 스토어와 함께 이식한다.

## M2 착수 대상
- `usage_provider.rs`/`local_usage_provider.rs` trait 구체화 + `usage_store.rs`(전 프로바이더 합산·burn tier·한도 알림 엣지)
- `companion_store.rs`: `applyUsage`(진화·졸업 상태머신)·`useMint`·`buy`·부화·프리패칭 + Mint/FreshEgg/Shop/DisplayState 테스트
- `poke_api.rs`(reqwest + app-data 캐시), `oauth_limits.rs`(wincred keyring), `codex_rate_limits.rs`
- 상태 영속: `%APPDATA%\PokeTokenBar` (tauri-plugin-store 또는 직접 파일)
- Gemini: 파서 구현됨, CLI 설치 머신에서 라이브 검증

## M3 프런트 슬라이스 — 검증 완료 (실기 실행)

`src/App.tsx` 기본 템플릿을 **오늘 사용량 팝오버 패널**로 교체: `get_today_usage` invoke →
오늘 총 토큰(compact)·비용·4종 토큰 분해·최근 5시간 활동(번레이트) 표시, 10초 자동 갱신, 다크 테마.
창은 340×500 팝오버(visible:false·skipTaskbar), 트레이 클릭으로 토글. `npm run tauri dev` 실행 확인.

## Claude 공식 한도 — 팀 요금제 실측 검증 (2026-07-22)

`get_claude_limits` invoke → 팝오버에 게이지 표시. **팀 요금제(`subscriptionType=team`, `tier=default_claude_max_5x`)에서
`/api/oauth/usage` 가 정상 데이터 반환 확인**(HTTP 200):
- Windows 는 토큰이 `~/.claude/.credentials.json` 파일에 있어 Keychain 코드 불필요(macOS 대비 대폭 단순).
- 응답: `five_hour.utilization`(5시간 세션)·`seven_day.utilization`(주간)·`limits[]`(weekly_scoped 모델별). 원본 LimitStatus 구조와 동일.
- planDisplay: team+5x → "Team 5x". 파서는 실측 응답을 픽스처로 회귀 고정(`parses_team_usage_response`).
- 자동 갱신은 파일 읽기+HTTP(10초). 실패/무자격증명 시 게이지만 우아하게 숨김(사용량 표시엔 무영향).
- **주의**: 토큰 값은 코드 밖으로 노출 안 함(헤더에만). 프로브 `cargo run --example probe_limits` 로 원응답 확인 가능.

## 컴패니언 앱 연결 — 검증 완료 (실기)

`refresh_companion` 커맨드가 사용량으로 스토어를 구동: 오늘 토큰 → `update()`(baseline/delta) →
`ensure_line_loaded()` → `hatch_if_needed()`(PokéAPI 부화). `CompanionStore` 는 `Mutex<..>` Tauri
managed state. 프런트는 알 게이지/스프라이트(PokéAPI PNG)·진화 진행·도감 수 표시.
**실기 확인**: `%APPDATA%\PokeTokenBar\companion-state.json` 생성, install baseline 이 설치 이전 사용량(≈181M)을
제외하고 설치 후 사용분(≈1.5M)만 egg_usage 로 인큐베이션(≈31%). 5M 도달 시 실제 부화.

- **주의**: `refresh_companion` 은 sync 커맨드 — 부화/라인로드 시 network 가 메인스레드를 잠깐 블록(부화는 드묾).
  UI 매끄러움 필요 시 async + spawn_blocking 으로 전환 여지.

## 남은 작업 (후속)
- **비동기화**: `refresh_companion` 네트워크 구간 async 전환(부화 시 UI 블록 제거).
- **usage_store**: 전 프로바이더 정밀 집계·burn tier 원본 로직·한도 알림 엣지(현재 burn 은 근사치).
- **M3 확대**: 도감·상점·설정 탭 + 진화/샤이니/부화 연출(현재는 정적 스프라이트).
- **트레이 아이콘**: 현재 PoC 애니메이션 → 실제 펫 스프라이트로 교체.
- **M4 나머지**: 자동시작(레지스트리 Run)·토스트 알림.
- **M5 배포**: winget/MSI·인앱 업데이트·서명.
- **Codex/Gemini 한도**: Codex 팀 한도는 로컬 로그에 존재(파서만), Gemini 는 CLI 설치 머신 실측.
