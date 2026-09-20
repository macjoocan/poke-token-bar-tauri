# macOS 빌드 · 패키징 가이드

Windows/Tauri 포크를 맥에서 빌드·패키징한 실측 기록. **코드 수정 없이 빌드된다.**
Intel + Apple Silicon **유니버설 바이너리**로 만든다.

검증: 2026-09-20 · Apple Silicon(arm64) 호스트 · Xcode Command Line Tools

---

## 1. 사전 준비

| 도구 | 검증 버전 | 설치 |
|---|---|---|
| Rust | 1.98.1 (stable-aarch64-apple-darwin) | 아래 rustup |
| Node | v24.18.0 | `brew install node` 등 |
| npm | 11.16.0 | Node 동봉 |
| Xcode CLT | 27.0.0 (`/Library/Developer/CommandLineTools`) | `xcode-select --install` |

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
. "$HOME/.cargo/env"

# 유니버설 빌드에 필요 — 호스트가 Apple Silicon 이면 x86_64 타깃을 추가한다
rustup target add x86_64-apple-darwin
```

Windows 와 달리 VS BuildTools·Windows SDK 는 필요 없다. Xcode 전체 설치도 불필요하고 CLT 면 된다.

## 2. 빌드

```bash
cd poketokenbar
npm install
npm run tauri build -- --target universal-apple-darwin --bundles app,dmg
```

산출물 (`src-tauri/target/universal-apple-darwin/release/bundle/`):
- `macos/poketokenbar.app` — 29MB, Mach-O universal (x86_64 + arm64)
- `dmg/poketokenbar_0.1.0_universal.dmg`

변형:

| 목적 | 명령 |
|---|---|
| 개발 실행 | `npm run tauri dev` |
| 호스트 아키텍처 전용 | `npm run tauri build -- --bundles app,dmg` |
| 앱만(dmg 건너뛰기) | `... --bundles app` |
| 번들 없이 바이너리만 | `... --no-bundle` |

호스트 전용 빌드는 `target/release/bundle/` 로, 유니버설은 `target/universal-apple-darwin/release/bundle/` 로 나간다.
초회 release 컴파일 약 1분 23초(호스트 전용, 의존성 포함). 유니버설은 x86_64 의존성을 새로 컴파일하므로 초회에 그만큼 더 걸리고,
이후 증분은 20~50초.

아키텍처 확인:

```bash
lipo -info macos/poketokenbar.app/Contents/MacOS/poketokenbar
# → Architectures in the fat file: ... are: x86_64 arm64
```

## 3. 패키징

`tauri build` 가 붙이는 기본 서명은 **linker-signed adhoc** 이라 `Sealed Resources=none` 상태고,
`spctl -a -t exec` 평가에서 `code has no resources but signature indicates they must be present` 로 거부된다.
정식 ad-hoc 서명을 다시 입힌 뒤 dmg 를 만든다.

```bash
cd poketokenbar/src-tauri/target/universal-apple-darwin/release/bundle

# 1) ad-hoc 재서명
codesign --force --deep --sign - macos/poketokenbar.app
codesign --verify --deep --strict macos/poketokenbar.app   # → 무출력이면 통과

# 2) 드래그 설치용 레이아웃으로 dmg 생성
mkdir -p stage/PokeTokenBar
cp -R macos/poketokenbar.app stage/PokeTokenBar/
ln -sfn /Applications stage/PokeTokenBar/Applications
hdiutil create -volname "PokeTokenBar" -srcfolder stage/PokeTokenBar \
  -ov -format UDZO dmg/poketokenbar_0.1.0_universal.dmg

# 3) 검증
hdiutil verify dmg/poketokenbar_0.1.0_universal.dmg
```

재서명 후 `CodeDirectory flags=0x2(adhoc)`, `Sealed Resources version=2 rules=13` 로 바뀐다.
dmg 를 마운트해 내부 앱에 `codesign --verify --deep --strict` 를 돌려도 통과한다.

### Apple 정식 서명으로 바꾸려면

ad-hoc 서명은 Apple Developer ID 서명이 아니다. **받는 쪽 맥에서는 Gatekeeper 가 막는다.**
경고 없이 배포하려면 **유료 Apple Developer Program** 가입 후 Developer ID Application 인증서 + 공증이 필요하다.
인증서가 준비되면 위 3장 수동 절차는 전부 불필요해지고, 환경변수만 넣으면 `tauri build` 가 서명·공증·stapling 을 처리한다.

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: 이름 (TEAMID)"
export APPLE_ID="you@example.com"
export APPLE_PASSWORD="앱 전용 암호"      # Apple ID 계정 암호 아님
export APPLE_TEAM_ID="XXXXXXXXXX"
npm run tauri build -- --target universal-apple-darwin --bundles app,dmg
```

App Store Connect API 키 방식이면 `APPLE_API_ISSUER` / `APPLE_API_KEY` / `APPLE_API_KEY_PATH` 를 대신 쓴다.
`notarytool` 은 Xcode CLT 에 포함돼 있어 별도 설치가 필요 없다.

서명 없이 배포할 때 받는 쪽 대응: 우클릭 → 열기, 또는 `xattr -d com.apple.quarantine /Applications/poketokenbar.app`.

⚠️ 크로스컴파일 불가 — **맥에서만** 빌드·서명된다(Apple 정책).

## 4. 검증 결과

| 항목 | 결과 |
|---|---|
| `cargo test --lib` (arm64) | **113 passed, 0 failed** |
| `cargo test --target x86_64-apple-darwin --lib` (Rosetta) | **113 passed, 0 failed** |
| `cargo run --release --example smoke` | `~/.claude/projects` 57개 엔트리 파싱 → 당일 3,849,172 토큰 / $4.0161 집계 |
| `.app` 번들 | 29MB, Mach-O universal — x86_64 (15,302,880B) + arm64 (15,001,136B) |
| `.dmg` | 9.7MB, UDZO, checksum VALID |
| 실행 | `open poketokenbar.app` 으로 기동, 프로세스 유지 확인 |

`Info.plist`: `CFBundleIdentifier=com.macjoocan.poketokenbar`, `LSMinimumSystemVersion=10.15`, `CFBundleIconFile=icon.icns`.

맥 고유 코드 경로는 이미 반영돼 있어 손댈 것이 없었다:
- `oauth_limits.rs` — `#[cfg(target_os="macos")]` 키체인 폴백(`security find-generic-password`)
- `lib.rs:831` — `tauri_plugin_autostart::MacosLauncher::LaunchAgent`
- `tauri.conf.json` — `bundle.targets: "all"`, `bundle.macOS.minimumSystemVersion`, `icons/icon.icns`

전체 소스에서 Windows 전용 분기는 `main.rs:2` 의 `windows_subsystem`(맥에서 무시됨) 하나뿐이고,
`C:\`·`%APPDATA%` 하드코딩 경로는 0건이다(전부 `dirs::home_dir()` 기반).

## 5. 트러블슈팅

**`bundle_dmg.sh` 실패 — `failed to run .../bundle_dmg.sh`**
첫 빌드에서 발생했고 **동일 명령 재실행 시 통과**했다. 원인은 특정하지 못했다.
실패하면 `bundle/macos/rw.*.dmg` 임시 이미지와 `/Volumes/` 의 잔여 마운트를 정리하고 재시도한다.
계속 막히면 `--bundles app` 으로 앱만 만든 뒤 3장의 `hdiutil` 절차로 dmg 를 직접 만든다.

**`npm warn allow-scripts ... esbuild / fsevents`**
npm 11 이 postinstall 을 기본 차단해서 나온다. `@esbuild/darwin-arm64` 바이너리는 optional dependency 로
이미 설치되므로 빌드에 지장 없었다. 문제가 생기면 `npm approve-scripts esbuild`.

**`spctl` 이 앱을 거부**
3장의 ad-hoc 재서명을 건너뛴 경우다. `codesign --force --deep --sign -` 을 먼저 돌린다.

**유니버설 빌드가 x86_64 타깃을 못 찾음**
`rustup target add x86_64-apple-darwin` 누락이다.

## 6. 미검증 — 맥에서 확인 필요

빌드·실행은 되지만 **동작을 눈으로 확인하지 않은** 항목들. 코드에 반영된 것이 없으므로 그대로면 아래대로 동작할 가능성이 높다.

| 항목 | 예상 | 대응(필요 시) |
|---|---|---|
| 메뉴바 트레이 아이콘 표시·해상도 | Windows용 32x32 스프라이트라 Retina 에서 흐리거나 다크모드 대비가 안 맞을 수 있음 | template image 대응 또는 @2x 스프라이트 |
| 팝오버 위치 | 트레이 아이콘 기준 배치 코드가 **없음**(`win.show()` 만 호출) → 엉뚱한 위치에 뜰 수 있음 | 트레이 좌표 기준 `set_position` 추가 |
| Dock 아이콘 | `LSUIElement` 미설정이라 Dock 에 뜸 | `setup` 에 `#[cfg(target_os="macos")] app.set_activation_policy(tauri::ActivationPolicy::Accessory);` 또는 `Info.plist` 에 `LSUIElement=1` |
| 알림(한도 경고) | `tauri-plugin-notification` 맥 지원은 있으나 실기기 미확인 | — |
| 자동시작 | `MacosLauncher::LaunchAgent` 지정돼 있으나 실기기 미확인 | — |
| Claude 키체인 폴백 | 코드에 있으나 이번 검증에서 실행 경로를 타는지 확인 못 함 | `--example probe_limits` 로 확인 |
| **Intel 맥 실기기 동작** | x86_64 슬라이스는 Rosetta 로 테스트만 통과. 실제 Intel 맥에서 GUI 기동은 미확인 | Intel 맥에서 dmg 설치 후 확인 |

## 참고

원본 [chattymin/PokeTokenBar](https://github.com/chattymin/PokeTokenBar) 는 이미 네이티브 맥 앱이다(`brew install --cask poke-token-bar`).
이 포크에서 갈라진 기능(단가 보정 · Codex 다중버킷 · 모험/전투)이 목적이 아니라면 원본을 쓰는 편이 낫다.
