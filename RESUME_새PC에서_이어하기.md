# 새 PC에서 이어하기 — PokeTokenBar Windows 포팅

> 이 파일 하나로 새 PC에서 재개 가능하도록 정리. 상세 진행은 [PROGRESS.md](PROGRESS.md), 원본 계획은
> [포팅계획서](PokeTokenBar_Windows_Tauri_포팅계획서.md).
> ⚠️ **Claude Code 프로젝트 메모리는 옛 PC 로컬(`~/.claude/...`)이라 이전 안 됨** → 이 문서가 새 PC의 유일한 재개 근거.

## 0. 폴더 백업 시 주의 (용량 큰 재생성물은 빼도 됨)
새 PC에서 재생성되므로 백업/이전에서 제외해도 무방(오히려 권장):
- `poketokenbar/node_modules/` (npm install 로 재생성)
- `poketokenbar/src-tauri/target/` (cargo build 로 재생성, 수 GB)
소스(`poketokenbar/src`, `src-tauri/src`, 설정)와 `upstream/`, 문서(.md)는 반드시 포함.

## 1. 새 PC 사전 설치 (툴체인)
Windows 11 기준. winget 사용.
```powershell
# Rust (MSVC 툴체인)
winget install --id Rustlang.Rustup -e --accept-package-agreements --accept-source-agreements
rustup default stable-x86_64-pc-windows-msvc
# MSVC C++ 빌드툴 + Windows SDK (Rust MSVC 링크 필수, 수 GB)
winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
  --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
# Node 20.19+ (또는 22.12+) 권장. WebView2 런타임은 Win11 기본 탑재.
```
검증: `rustc --version`, `cargo --version`, `node --version`, `link.exe`(x64) 존재.

## ⚠️ 포터블 exe 는 반드시 `tauri build` 로 (cargo build 금지)
`cargo build --release` 로 만든 exe 를 단독 실행하면 **"연결 거부"**가 뜬다 — 그 빌드는 프런트를
dist 로 만들어 exe 에 심지 않아서 앱이 개발 서버(`localhost:1420`)를 찾기 때문. **반드시 아래로 빌드:**
```powershell
cd poketokenbar
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
npm run tauri build -- --no-bundle    # npm run build(→dist) 후 exe 에 임베드
# 산출물: src-tauri\target\release\poketokenbar.exe → 루트 PokeTokenBar.exe 로 복사
```

## 2. 빌드 · 실행 · 테스트
```powershell
cd poketokenbar
npm install                                   # 최초 1회
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"   # cargo 를 PATH 에 (tauri dev 가 cargo metadata 호출)
npm run tauri dev                             # 앱 실행(트레이 앱 — 클릭 시 팝오버)
```
- 코어 테스트: `cargo test --manifest-path src-tauri/Cargo.toml --lib`  → **현재 69/69 통과**
- 실데이터 스모크: `cargo run --manifest-path src-tauri/Cargo.toml --example smoke`
- Claude 한도 프로브: `cargo run --manifest-path src-tauri/Cargo.toml --example probe_limits`

## macOS 빌드 (2026-09-20 실빌드 검증 완료 — 코드 수정 0줄)
> 실측 절차·패키징·트러블슈팅·미검증 항목은 [`macOS_빌드_가이드.md`](macOS_빌드_가이드.md) 로 분리했다. 아래는 배경 설명.
Tauri 크로스플랫폼 + 경로를 `dirs`(home/data_dir)로 짜서 **로그 파싱·컴패니언·Codex 한도·단가 등 대부분 그대로** 돈다.
맥 고유 처리는 코드에 반영해 둠:
- **Claude OAuth 토큰**: 맥은 키체인에 저장 → `oauth_limits.rs` 에 `#[cfg(target_os="macos")]` 키체인 폴백 추가
  (`security find-generic-password -s "Claude Code-credentials" -w`). 파일이 있으면 파일 우선.
- `tauri.conf.json` 에 `bundle.macOS.minimumSystemVersion` 추가. 아이콘 `icons/icon.icns` 이미 있음.

**빌드(맥에서)**:
```bash
cd poketokenbar
npm install
npm run tauri build          # → src-tauri/target/release/bundle/macos/PokeTokenBar.app (+ dmg)
```
- ⚠️ **맥에서만 빌드/서명 가능**(Apple 정책 — Windows 크로스컴파일 불가).
- 배포 시 Gatekeeper 경고 없이 하려면 Apple 개발자 인증서 + 공증. 개인용은 우클릭→열기.
- (선택) **메뉴바 전용**(Dock 아이콘 숨김): `setup` 에 `#[cfg(target_os="macos")] app.set_activation_policy(tauri::ActivationPolicy::Accessory);`
  또는 `Info.plist` 에 `LSUIElement=1`. 미검증(맥에서 확인 필요)이라 코드엔 미반영.
- Windows 빌드는 영향 없음(위 키체인 코드는 macOS 에서만 컴파일).

**참고**: 원본 PokeTokenBar 는 이미 네이티브 맥 앱(`brew install --cask poke-token-bar`). 우리 포크는 단가 보정·Codex 다중버킷·모험/전투가 추가돼 갈라짐.

## 3. 확정된 결정 (재논의 불필요)
- 트레이 표시: **동적 아이콘 + 툴팁** / 프런트: **React + TypeScript**
- **Claude 기반 우선** (Codex/Gemini 한도는 후순위)
- 상태/캐시 경로: `%APPDATA%\PokeTokenBar\` (companion-state.json, base-index.json, usage-cache.json)

## 4. 환경 특이사항 (실측)
- Claude/Codex 로그는 `%USERPROFILE%\.claude\projects`·`.codex\sessions` 에 계획서대로 존재.
- **Gemini CLI 미설치**(이 PC엔 Antigravity IDE만) → `~/.gemini/tmp` 부재. 파서는 구현돼 있으니 Gemini CLI 있는 PC에서 라이브 검증만 남음.
- **OAuth 토큰**은 `%USERPROFILE%\.claude\.credentials.json` 파일에 있음(macOS Keychain 불필요). 팀 요금제(`team`/`default_claude_max_5x`)에서 `/api/oauth/usage` 정상 응답(five_hour/seven_day/limits[]) 실측 확인.
- 코드 함정: 모듈명 `core` 가 std `core` 와 충돌 → 크레이트 루트에서 `crate::core` 로 참조.

## 5. 완료 상태 (What works)
- 파서(Claude/Codex/Gemini)·증분캐시·게임규칙·스토어(부화/진화/졸업/상점/민트/새알)·SeededRng — 원본 패리티, 69 테스트.
- Claude 공식 한도 게이지(팀 요금제) — 실기 검증.
- PokéAPI 진화라인 fetch + 컴패니언 앱 연결 — 실기 검증(알 인큐베이션 동작).
- 트레이 PoC(동적 아이콘+툴팁), 프런트 팝오버(오늘 사용량·한도·컴패니언).
- **단가 보정(2026-09-07)** — `model_pricing.rs`/`local_usage_reader.rs`. 공식 가격표 기준으로 전면 개정: Fable 계열 $0→실단가($10/$50), 캐시쓰기 5분/1시간 분리(`cache_write_5m`/`cache_write_1h` + `ephemeral_1h/5m` 파싱, 옛 로그는 5분 폴백), opus-5·sonnet-5·은퇴모델 정확매칭, 미가격 claude 모델 1회 경고. 실측 대비 **35.6% 과소집계 해소**. 검증: `node 단가_보정_실측.js`(공식 단가 열) ≡ `cargo run --example price_audit`(고친 앱) **센트 단위 일치**. gpt/gemini 단가는 미확인이라 미변경(별도 작업).
- **Codex 한도 다중 버킷(2026-09-17)** — `codex_rate_limits.rs`. Codex 는 한도가 모델별 버킷(`limit_id`)으로 분리됨(예: `codex` 메인 vs `codex_bengalfox`=Spark). 기존엔 "전역 최신 이벤트 1개"만 써서, 방금 쓴 Spark 버킷(0%)이 메인 버킷(주간 65%)을 가려 **0%로 오인**됐다. → **버킷별 최신 상태를 각각 게이지로**(라벨 `메인·주간`/`Spark·5시간`), 사용률 높은 순 정렬, 창 리셋된 버킷은 `is_active=false`(stale, 프런트 흐림+기록시각). `LimitGauge` 에 `as_of` 필드 추가. 실측: `cargo run --example codex_audit` → `메인·주간 65%` 가 맨 앞(더는 0% 아님). 6 신규 테스트, **113/113**.

## 6. 재개 지점 (다음 할 일 — 우선순위)
1. ~~`refresh_companion` async 전환~~ ✅ **완료(2026-07-23)** — `async fn refresh_companion(app: AppHandle)` + `tauri::async_runtime::spawn_blocking`(디스크스캔·부화네트워크 블로킹풀로), State→AppHandle 접근. dev 재빌드 검증.
2. ~~usage_store 정밀 집계~~ ✅ **완료(2026-07-23)** — `core/usage_store.rs`: `burn_tier`(원본 임계 1k/100k/400k, lib.rs 근사 대체)·`evaluate_limit_alerts`(엣지 트리거)+`LimitAlert`·`claude_alert_windows`·`is_limit_warning`. `refresh_companion` 이 `usage_store::burn_tier` 사용. 8 신규 테스트, **77/77**. (알림 토스트 실발화는 M4에서 이 순수판정에 배선.)
3. ~~M3 UI 확대~~ ✅ **완료(2026-07-23)** — 하단 4탭(홈/도감/상점/설정). 백엔드 커맨드 신규: `get_dex`·`get_shop`·`buy_item`·`use_rare_candy`·`use_mint`·`get_language`·`set_language`, `CompanionView` 확장(available_tokens·rare_candy·mint_count·owns_shiny_charm·can_use_*·nature). 프런트: 도감 그리드·상점(구매/가방 사용)·설정(언어 3종). 연출: 부화/진화/이로치 CSS 키프레임(App.tsx 에서 pet 상태 diff → `fx-*` 클래스).
4. ~~트레이 아이콘 실펫 스프라이트~~ ✅ **완료(2026-07-23)** — `fetch_sprite_image`(PokéAPI PNG→`Image::from_bytes`, 디스크캐시 `sprites/`, 이로치 없으면 일반 폴백). 트레이 스레드: 알=부화진행 게이지 애니, 부화후=실펫 스프라이트(정적, (species,shiny) 변경 시에만 set_icon = idle-CPU diff-gate). 1s 틱, 툴팁 5s.
5. ~~M4 자동시작·알림·설정영속~~ ✅ **완료(2026-07-23)** — `tauri-plugin-autostart`(레지스트리 Run) + `tauri-plugin-notification`(Windows 토스트). `AppSettings`(limit_notifications·warn/crit) → `%APPDATA%\PokeTokenBar\settings.json` 영속. 커맨드 `get_settings`·`set_autostart`·`update_settings`. 트레이 스레드 120s마다 `check_and_notify_limits`: `fetch_limits`→`claude_alert_windows`→`evaluate_limit_alerts`(AppState.notified_tier)→토스트. 프런트 설정탭: 자동시작·알림 토글 + 경고/위험선 슬라이더. **플러그인은 Rust 에서만 호출 → capabilities 변경 불필요.** ⚠️ 토스트는 **번들 앱(설치본)에서 확실**; dev 실행 시 AppUserModelID 미설정으로 안 뜰 수 있음(M5 번들 후 실검증).
6. ~~CRPG MVP(모험 이벤트+아이템 드롭+말풍선)~~ ✅ **완료(2026-07-23)** — `core/adventure.rs`(원본 없음): 가중 이벤트 테이블 7종(보상 24%=사탕·민트, 플레이버 76%)·`roll_event`(SeededRng 결정적)·`ADVENTURE_INTERVAL`(30만 토큰). `CompanionStore::maybe_trigger_adventure`가 `used_since_install` 간격마다 발생→인벤토리 +1·`adventure_log`(cap 20) 적재. `CompanionState`에 `last_event_tokens`·`event_seq`·`adventure_log` 필드. lib: `CompanionView.latest_event`(말풍선)·`get_adventure_log` 커맨드. 프런트: 펫 말풍선(6s)·홈 "모험 로그" 패널. 4 신규 테스트, **81/81**. 룰 기반·오프라인. 확장 여지: 선택형 이벤트/전투/퀘스트.
7. **M5 배포** ✅ **포터블 exe 완료(2026-07-23)** — `tauri build --no-bundle`(release 45s, deps 캐시) → **단독 실행 exe** `target/release/poketokenbar.exe`(13MB) → 루트 `C:\00.SVN\Token_Poketmon\PokeTokenBar.exe` 복사, 더블클릭 실행 검증. **설치 불필요·포터블**(WebView2 Win11 기본). 트레이 **우클릭 컨텍스트 메뉴(열기/종료)** 추가 — `Menu`+`MenuItem`, `show_menu_on_left_click(false)`(좌클릭=팝오버 토글 유지), `on_menu_event`(open=show, quit=`app.exit(0)`). NSIS 설치본은 이전 `--bundles nsis` 빌드에 생성돼 있음. **남은 것(선택·외부작업)**: 코드서명(인증서→SmartScreen 경고 제거), 자동업데이트(`tauri-plugin-updater`+서버), winget 공개배포, 토스트 설치본 실검증. **공유 시 주의**: 앱이 `~/.claude/.credentials.json` 토큰을 읽어 `/api/oauth/usage` 호출 → 받는 사람이 unsigned exe 를 신뢰해야 함(각 PC 로컬 데이터만 처리, 데이터 상호 유출 없음). 소스 동봉 권장.
8. (후순위) Codex 팀 한도 파서(`codex_rate_limits`, 로컬 token_count 이벤트에 rate_limits 존재), Gemini 라이브 검증.

## 7. 코어 모듈 지도 (`poketokenbar/src-tauri/src/core/`)
| 모듈 | 원본 | 상태 |
|---|---|---|
| models·model_pricing·token_formatter | 동명 | ✅ |
| local_usage_reader·local_usage_cache | LocalUsageReader/Cache | ✅ |
| companion_model | CompanionModel(+Store 정적함수) | ✅ |
| companion_store | CompanionStore | ✅ (네트워크 부분 sync) |
| oauth_limits | OAuthLimitsProvider | ✅ (Claude) |
| poke_api | PokeAPIClient | ✅ (blocking) |
| usage_store | UsageStore(순수판정만) | ✅ (burn_tier·한도알림 엣지트리거) |
| adventure | (원본 없음, 신규) | ✅ (CRPG 모험 이벤트·룰 기반·결정적) |
| codex_rate_limits·local_usage_provider | — | ⬜ 미착수 |

프런트: `poketokenbar/src/App.tsx`(+App.css) — 팝오버 단일 화면(컴패니언/사용량/한도). `lib.rs` 에 트레이·커맨드(get_today_usage·get_claude_limits·refresh_companion).
