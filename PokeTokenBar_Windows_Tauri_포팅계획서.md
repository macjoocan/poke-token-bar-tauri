# PokeTokenBar → Windows (Tauri) 포팅 계획서

> 대상 저장소: `chattymin/PokeTokenBar` (Swift 6 + SwiftUI, macOS 14+ 메뉴바 앱)
> 목표 스택: **Tauri 2** — Rust 백엔드 + 웹(HTML/CSS/TS) 프런트엔드
> 작성일: 2026-07-22 · 기준 코드: main (소스 약 7,150 LOC — Core 4,751 / UI 2,036 + 테스트 19개 파일)

---

## 1. 한눈에 보는 결론

로직(게임 규칙·로그 파싱·API)은 플랫폼 독립적이라 Rust로 **재구현**하면 되고, UI는 SwiftUI라 웹으로 **전면 재작성**해야 합니다. 즉 "이식"이 아니라 "Windows판 리메이크"에 가깝습니다.

- **재사용 가능한 개념(≈45%)**: 게임 규칙, 로그 파싱 스펙, PokéAPI 연동, 비용/포맷 로직. 코드는 못 옮겨도 로직과 52개 테스트가 정확한 명세 역할을 합니다.
- **플랫폼 교체 필요(≈20%)**: Keychain, 로그인 항목, 자동 업데이트/배포, 알림.
- **전면 재작성(≈35%)**: 메뉴바(NSStatusItem)·팝오버, SwiftUI 화면 7종.

**핵심 난관은 코드가 아니라 UX 하나입니다** → macOS 메뉴바는 아이콘 옆에 `200.7M` 같은 **텍스트를 직접 표시**하지만, **Windows 시스템 트레이는 작은 아이콘만** 놓을 수 있습니다. 이 차이를 어떻게 풀지가 이 프로젝트에서 가장 먼저 결정할 사항입니다. ([4번 항목](#4-가장-큰-ux-난관--메뉴바--트레이) 참고)

---

## 2. 파일별 매핑 (로직 / 플랫폼 글루 / UI)

### 🟢 그대로 재구현 — Foundation만 쓰는 순수 로직

| 파일 | LOC | 역할 | Tauri에서 |
|---|---|---|---|
| `Core/Models.swift` | 408 | 데이터 모델·enum | Rust struct/enum (serde) |
| `Core/CompanionModel.swift` | 411 | **부화·진화·졸업 게임 규칙** | Rust 순수 함수 — 핵심 자산 |
| `Core/LocalUsageReader.swift` | 320 | `.claude/.codex/.gemini` JSONL 파싱 | Rust (경로만 Windows로) |
| `Core/LocalUsageCache.swift` | 159 | 증분 캐시 | Rust |
| `Core/LocalUsageProvider.swift` | 100 | 사용량 프로바이더 | Rust |
| `Core/UsageProvider.swift` | 23 | 프로바이더 프로토콜 | Rust trait |
| `Core/ModelPricing.swift` | 61 | 모델별 단가 테이블 | Rust 상수 테이블 |
| `Core/TokenFormatter.swift` | 48 | `200.7M` 포맷 | TS(프런트) 또는 Rust |
| `Core/Localization.swift` | 382 | KO/EN/JA 문자열 | **프런트 i18n**(JSON)으로 이동 |
| `Core/BinaryLocator.swift` | 140 | CLI 설치 경로 탐색 | Rust (Windows 경로로) |
| `Core/ProviderStatusChecker.swift` | 74 | 프로바이더 연결 확인 | Rust |
| `Core/CodexRateLimitsProvider.swift` | 76 | Codex 한도 | Rust |
| `Core/AppEnv.swift` | 11 | 환경값 | Rust |

### 🟡 로직은 살리되 플랫폼 API 교체

| 파일 | LOC | macOS 의존 | Windows/Tauri 대체 |
|---|---|---|---|
| `Core/UsageStore.swift` | 836 | AppKit·UserNotifications | 상태는 Rust, 알림은 `tauri-plugin-notification` |
| `Core/CompanionStore.swift` | 709 | `companion-state.json`·UserDefaults·알림 | Tauri app-data dir + `tauri-plugin-store` |
| `Core/OAuthLimitsProvider.swift` | 231 | **Keychain** OAuth 토큰 + HTTP | **Windows 자격 증명 관리자**(`keyring` crate) |
| `Core/PokeAPIClient.swift` | 220 | URLSession + Application Support 캐시 | `reqwest` + app-data 캐시 |
| `Core/UpdateChecker.swift` | 140 | HTTP·`NSWorkspace.open`·**Homebrew** | GitHub Releases + winget/MSI + `tauri-plugin-updater` |
| `Core/ProcessRunner.swift` | 145 | `codex app-server` 서브프로세스 | `std::process`(codex.exe 필요) |

### 🔴 macOS 전용 — Windows 방식으로 재작성

| 파일 | LOC | 무엇 | Windows/Tauri 대체 |
|---|---|---|---|
| `PokeTokenBarApp.swift` | 365 | **NSStatusItem 메뉴바 + NSPopover + 스프라이트 타이머** | **Tauri 트레이 아이콘 + 팝오버 창** (최대 재작성 지점) |
| `Core/KeychainAccess.swift` | 48 | Security/LocalAuthentication | `keyring`(wincred) |
| `Core/LoginItem.swift` | 41 | SMAppService/launchd KeepAlive | `tauri-plugin-autostart`(레지스트리 Run 키) |
| `Core/CrashReporter.swift` | 96 | NSApplication 알림·시그널 핸들러 | Rust `panic hook` + 로그 |
| `Core/SupportMail.swift` | 26 | `mailto:` | `tauri-plugin-shell` open |

### 🔵 UI — SwiftUI 전량, 웹으로 전면 재작성

| 파일 | LOC | 화면 |
|---|---|---|
| `UI/PopoverView.swift` | 596 | 메인 팝오버(홈/탭 컨테이너) |
| `UI/CompanionView.swift` | 517 | 컴패니언·진화 연출 |
| `UI/SettingsView.swift` | 383 | 설정 |
| `UI/ShopView.swift` | 198 | 상점 |
| `UI/BagView.swift` | 138 | 가방(레어 캔디 등) |
| `UI/SpriteLoader.swift` | 174 | 스프라이트 로딩 |
| `UI/SpriteAnimation.swift` | 30 | 스프라이트 애니메이션 |

→ 전부 React/Svelte + CSS로 재구현. 스프라이트 애니메이션·✨샤이니 연출은 CSS/Lottie로 오히려 더 매끄럽게 뽑을 수 있는 영역입니다.

### ⚪ 테스트 — 19개 파일

게임 규칙(부화·진화·민트·레어캔디·샤이니 확률·사용량 패리티)의 **정확한 명세**입니다. Rust `#[test]`로 포팅하면 로직 이식의 정답지가 됩니다. 버리지 말고 가장 먼저 옮기세요.

---

## 3. Tauri 아키텍처 제안

```
┌─────────────────────────────────────────┐
│  프런트엔드 (WebView: React/Svelte + CSS) │  ← UI 7종 재작성, i18n JSON
│   팝오버 창: 홈 / Pokédex / Bag / Shop    │
└───────────────▲───────────────────────────┘
                │  Tauri invoke / event
┌───────────────┴───────────────────────────┐
│  Rust 백엔드 (core 로직 재구현)             │
│  · 로그 파서(.claude/.codex/.gemini)        │
│  · 게임 규칙(부화·진화·졸업)                 │
│  · PokéAPI 클라이언트 + 캐시                 │
│  · 사용량/컴패니언 상태 스토어               │
│  · 갱신 타이머 → 트레이 아이콘/툴팁 업데이트 │
└───────────────┬───────────────────────────┘
                │  플러그인/OS API
    tray-icon · notification · autostart · store · updater · keyring(wincred)
```

경로 매핑 (macOS → Windows):

| 용도 | macOS | Windows |
|---|---|---|
| Claude 로그 | `~/.claude/projects` | `%USERPROFILE%\.claude\projects` |
| Codex 로그 | `~/.codex/sessions` | `%USERPROFILE%\.codex\sessions` |
| Gemini 로그 | `~/.gemini/tmp` | `%USERPROFILE%\.gemini\tmp` |
| 상태/캐시 | `~/Library/Application Support/PokeTokenBar` | `%APPDATA%\PokeTokenBar` |
| 자격 증명 | Keychain | Windows Credential Manager |

> ⚠️ **선행 검증 필요**: Windows용 Claude Code/Codex/Gemini CLI가 실제로 위 경로(`%USERPROFILE%\.xxx`)에 같은 포맷의 JSONL을 남기는지 실기에서 확인해야 합니다. CLI마다 Windows에서 `%APPDATA%`나 `%LOCALAPPDATA%`로 가는 경우가 있어, 여기가 어긋나면 앱 전체가 "사용량 0"이 됩니다. **1일차에 가장 먼저 확인할 항목.**

---

## 4. 가장 큰 UX 난관 — 메뉴바 → 트레이

macOS 원본은 메뉴바에 **애니메이션 스프라이트 + `200.7M` 텍스트 + `$`/`%`** 를 나란히 붙입니다(`NSStatusItem`이 임의 뷰/텍스트 허용). **Windows 시스템 트레이는 16–32px 아이콘 하나만** 놓을 수 있어 이 UX를 그대로 못 옮깁니다.

선택지:

1. **동적 아이콘 렌더링** — 타이머로 트레이 아이콘 이미지를 갱신해 스프라이트를 애니메이션. 숫자는 툴팁(마우스 오버)이나 팝오버에서만 노출. → 가장 Windows다운 방식, 권장.
2. **아이콘에 숫자 합성** — 작은 배지처럼 사용량을 아이콘에 그려 넣기. 32px에선 가독성 한계.
3. **작은 상시 위젯 창** — 트레이 대신(또는 병행) 데스크톱에 항상 떠 있는 미니 위젯으로 스프라이트+숫자 표시. → macOS 감성에 가장 근접하지만 사용자에 따라 거슬릴 수 있음.

**권장**: 1번(동적 아이콘 + 툴팁) 기본, 2번 배지를 옵션으로. 이 결정이 프런트/트레이 로직 설계를 좌우하므로 착수 전에 확정하세요.

---

## 5. 그 외 Windows 이슈 체크리스트

- **배포**: Homebrew cask → **winget** 등록 또는 MSI/NSIS 설치 파일 + GitHub Releases. 코드 서명(선택이지만 SmartScreen 경고 회피에 유효)·`tauri-plugin-updater`로 인앱 업데이트.
- **로그인 실행**: launchd KeepAlive(크래시 자동 재실행 포함) → Windows는 레지스트리 `Run` 키 자동시작이 표준. 크래시 워치독까지 원하면 별도 감시 프로세스나 작업 스케줄러 필요(원본만큼의 자동 재실행은 노력 대비 효용 낮음 — 후순위 권장).
- **Codex 공식 한도**: `codex app-server` 서브프로세스에 의존. Windows에 codex.exe가 있어야 동작하므로, 없으면 해당 섹션을 우아하게 숨기는 처리 필요(원본의 Keychain opt-out과 같은 패턴).
- **알림**: UserNotifications → `tauri-plugin-notification`(Windows 토스트).

---

## 6. 마일스톤 & 공수 (1인 기준, 러프 추정)

| 단계 | 내용 | 예상 |
|---|---|---|
| **M0. 스파이크** | Windows CLI 로그 경로/포맷 실측 검증 + Tauri 트레이 PoC | 2–3일 |
| **M1. 코어 로직** | 파서·게임규칙·모델 Rust 재구현 + 테스트 19개 이식 | 1.5–2주 |
| **M2. 데이터 연동** | PokéAPI 클라이언트·캐시·상태 스토어·설정 영속 | 1주 |
| **M3. UI** | 팝오버 4탭 + 설정, 스프라이트/진화/샤이니 연출 | 2–3주 |
| **M4. 트레이/OS 통합** | 트레이 아이콘 애니메이션·자동시작·알림·자격증명 | 1주 |
| **M5. 배포** | 설치 파일·인앱 업데이트·서명·winget | 3–5일 |
| **합계** | | **약 6–8주** |

가장 불확실한 변수는 M0의 경로 검증 결과와 M4의 트레이 UX 결정입니다.

---

## 7. 착수 전 결정할 3가지

1. **트레이 표시 방식** — 동적 아이콘+툴팁 / 아이콘 배지 / 상시 위젯 창 중 무엇으로?
2. **프런트 프레임워크** — React vs Svelte(둘 다 Tauri와 잘 맞음, Svelte가 더 가벼움).
3. **배포 채널** — winget / 자체 MSI / 둘 다? 코드 서명 여부.

---

*이 문서는 main 브랜치 소스 정적 분석 기준입니다. 실제 착수 시 M0 스파이크 결과에 따라 M1 이후 추정이 조정될 수 있습니다.*
