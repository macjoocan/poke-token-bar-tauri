//! 코어 로직 — 플랫폼 독립 (원본 `Sources/PokeTokenBar/Core/*` 재구현).
//!
//! 마일스톤별 채움 순서:
//! - M1: models, local_usage_reader, local_usage_cache, model_pricing, token_formatter, usage_provider
//! - M2: poke_api, oauth_limits, usage_store, companion_store
//! - 이후: companion_model(게임규칙), codex_rate_limits 등
//!
//! 원본 대조 기준: `upstream/Sources/PokeTokenBar/Core/`

// --- M1 스코프 (파서/모델/포맷/단가) ---
pub mod models; // 원본 Models.swift — 데이터 모델·enum (serde)
pub mod model_pricing; // 원본 ModelPricing.swift — 모델별 단가 테이블
pub mod token_formatter; // 원본 TokenFormatter.swift — "200.7M" 포맷
pub mod usage_provider; // 원본 UsageProvider.swift — 프로바이더 trait
pub mod local_usage_reader; // 원본 LocalUsageReader.swift — .claude/.codex/.gemini JSONL 파싱
pub mod local_usage_cache; // 원본 LocalUsageCache.swift — 증분 캐시
pub mod companion_model; // 원본 CompanionModel.swift — 게임 규칙·타입·밸런스 + 정적 결정함수

// --- M2 스코프 ---
pub mod companion_store; // 원본 CompanionStore.swift — 상태머신(진화·졸업)·상점·민트·부화·표시상태
pub mod oauth_limits; // 원본 OAuthLimitsProvider.swift — Claude 공식 한도(%) OAuth 조회
pub mod poke_api; // 원본 PokeAPIClient.swift — 진화라인/base 인덱스 fetch (blocking PokeProvider)
pub mod usage_store; // 원본 UsageStore.swift — 순수 파생값(burn tier·한도 알림 엣지 트리거)
pub mod adventure; // 신규(원본 없음) — 토큰 연동 모험 이벤트(룰 기반·결정적)
pub mod battle; // 신규(원본 없음) — 모험 조우 야생 전투(라이트 턴제·결정적)
pub mod codex_rate_limits; // 원본 CodexRateLimitsProvider.swift — Codex 한도(로컬 로그 파싱판)

// --- M2+ 스코프 (착수 시 주석 해제) ---
// pub mod local_usage_provider; // LocalUsageProvider.swift
