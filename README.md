# PokeTokenBar — Windows/Tauri 포크

AI 코딩 CLI(**Claude Code / Codex / Gemini**)의 토큰 사용량을 로컬 로그에서 읽어
트레이에 보여주고, 사용량으로 **포켓몬을 키우는**(부화·진화·졸업) 데스크톱 펫 앱.

> [chattymin/PokeTokenBar](https://github.com/chattymin/PokeTokenBar)(macOS·SwiftUI, MIT)를
> **Rust + Tauri(웹 프런트)** 로 재구현한 Windows 판. 이후 단가 보정·Codex 다중버킷·모험 등으로 갈라짐.

## 기능
- **사용량 집계** — `~/.claude/projects`·`~/.codex/sessions`·`~/.gemini/tmp` JSONL 파싱(증분 캐시, 중복 제거).
- **비용 계산** — 공식 가격표 기준 단가(5분/1시간 캐시 분리, Fable 등 최신 모델 반영).
- **공식 한도 게이지** — Claude(OAuth `/api/oauth/usage`)·Codex(로컬 rate_limits, **모델별 버킷 분리 표시**).
- **컴패니언** — 설치 이후 사용량으로 알 부화 → 진화 → 도감. 상점/민트/새 알, 모험·전투(포크 확장).
- **트레이** — 동적 아이콘(펫 스프라이트) + 툴팁, 클릭 시 팝오버(홈/도감/상점/설정 4탭).
- OS 통합 — 자동시작·토스트 알림.

## 빌드
필요: **Rust(MSVC)**, **Node 20.19+**. (Windows 는 VS BuildTools C++ 워크로드 + Windows SDK)

```bash
cd poketokenbar
npm install
npm run tauri build -- --no-bundle    # 포터블 exe: src-tauri/target/release/poketokenbar.exe
# 또는 npm run tauri dev  (개발 실행)
```
- ⚠️ **포터블 exe 는 반드시 `tauri build`** 로. `cargo build` 로 만든 exe 는 프런트를 임베드하지 않아 "연결 거부"가 뜬다.
- **macOS**: 크로스플랫폼이라 대부분 그대로 빌드됨(맥에서 `npm run tauri build`). Claude 토큰 키체인 폴백 반영 완료.
  자세한 절차는 [`RESUME_새PC에서_이어하기.md`](RESUME_새PC에서_이어하기.md) 참고.

## 테스트
```bash
cargo test --manifest-path poketokenbar/src-tauri/Cargo.toml --lib
```
코어(파서·게임규칙·스토어·단가·한도)는 원본 테스트를 이식해 검증. 진단 예제: `cargo run --example {smoke,price_audit,codex_audit,probe_limits}`.

## 문서
- [`PROGRESS.md`](PROGRESS.md) — 진행 현황 · [`RESUME_새PC에서_이어하기.md`](RESUME_새PC에서_이어하기.md) — 재개/빌드 가이드
- [`PokeTokenBar_Windows_Tauri_포팅계획서.md`](PokeTokenBar_Windows_Tauri_포팅계획서.md) — 포팅 계획
- [`단가_보정_작업지시.md`](단가_보정_작업지시.md) — 단가 보정 작업 내역

## 라이선스
MIT. 원본 저작권 [chattymin](https://github.com/chattymin/PokeTokenBar) 유지 — [`LICENSE`](LICENSE) 참고.
