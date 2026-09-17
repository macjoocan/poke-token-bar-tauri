//! 모델별 토큰 단가(USD/토큰). 원본 `ModelPricing.swift` 이식.
//!
//! 기준: Anthropic 공식 가격표 <https://platform.claude.com/docs/en/about-claude/pricing> (2026-09-07 확인).
//! (과거엔 ccusage 오프라인 LiteLLM 스냅샷을 역산해 맞췄으나, 그 스냅샷이 낡아 Fable=$0·1h캐시 미분리로
//!  비용을 35.6% 과소집계했다 — 단가_보정_작업지시.md. 이제 공식 가격표를 단일 기준으로 삼는다.)
//!
//! 캐시 쓰기는 5분(1.25배)·1시간(2배)이 단가가 다르다 → `cache_write_5m`/`cache_write_1h` 분리.
//! 캐시 읽기는 기본 0.1배, 단 Fable 5.1·Mythos 5.1 만 0.025배(공식 각주).

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelRate {
    pub input: f64, // USD per token
    pub output: f64,
    pub cache_write_5m: f64, // 5분 캐시 쓰기 (기본 입력의 1.25배)
    pub cache_write_1h: f64, // 1시간 캐시 쓰기 (기본 입력의 2배)
    pub cache_read: f64,
}

impl ModelRate {
    pub const ZERO: ModelRate = ModelRate {
        input: 0.0,
        output: 0.0,
        cache_write_5m: 0.0,
        cache_write_1h: 0.0,
        cache_read: 0.0,
    };

    /// USD per **million** tokens 로 선언(가독성) → per-token 으로 변환.
    const fn per_million(input: f64, output: f64, cw5m: f64, cw1h: f64, cread: f64) -> ModelRate {
        ModelRate {
            input: input / 1_000_000.0,
            output: output / 1_000_000.0,
            cache_write_5m: cw5m / 1_000_000.0,
            cache_write_1h: cw1h / 1_000_000.0,
            cache_read: cread / 1_000_000.0,
        }
    }
}

/// 모델명 → 단가. 정확 매칭 → 접두 매칭 → 패밀리 폴백 → 0(미가격 모델 오표시 방지).
/// 값은 `단가_보정_실측.js` 의 `real()` 과 일치(공식 가격표 2026-09-07).
pub fn rate(model: &str) -> ModelRate {
    // per_million(input, output, cache_write_5m, cache_write_1h, cache_read)
    let exact = match model {
        // Opus 4.5~5 계열 — 전부 $5/$25 (5m 6.25 / 1h 10 / read 0.5)
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6"
        | "claude-opus-4-5" => Some(ModelRate::per_million(5.0, 25.0, 6.25, 10.0, 0.5)),
        // Opus 4 / 4.1 (은퇴) — $15/$75
        "claude-opus-4" | "claude-opus-4-1" => Some(ModelRate::per_million(15.0, 75.0, 18.75, 30.0, 1.5)),
        // Fable/Mythos 5.1 — 캐시읽기 0.025배($0.25) 주의(공식 각주)
        "claude-fable-5-1" | "claude-mythos-5-1" => Some(ModelRate::per_million(10.0, 50.0, 12.5, 20.0, 0.25)),
        // Fable/Mythos 5 — 캐시읽기 0.1배($1)
        "claude-fable-5" | "claude-mythos-5" => Some(ModelRate::per_million(10.0, 50.0, 12.5, 20.0, 1.0)),
        // Sonnet 5 — $2/$10 (정식 단가 확정, 폴백 $3/$15 과 다르다)
        "claude-sonnet-5" => Some(ModelRate::per_million(2.0, 10.0, 2.5, 4.0, 0.2)),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" => Some(ModelRate::per_million(3.0, 15.0, 3.75, 6.0, 0.3)),
        "claude-haiku-3-5" => Some(ModelRate::per_million(0.8, 4.0, 1.0, 1.6, 0.08)),
        // Codex/Gemini — 이번 보정 범위 밖(Anthropic 문서 외). 값 유지, 캐시쓰기는 5m/1h 동일 취급.
        "gpt-5.5" => Some(ModelRate::per_million(5.0, 30.0, 0.0, 0.0, 0.5)),
        "gemini-2.5-pro" => Some(ModelRate::per_million(1.25, 10.0, 0.0, 0.0, 0.3125)),
        "gemini-2.5-flash" => Some(ModelRate::per_million(0.30, 2.5, 0.0, 0.0, 0.075)),
        "gemini-2.0-flash" => Some(ModelRate::per_million(0.10, 0.4, 0.0, 0.0, 0.025)),
        _ => None,
    };
    if let Some(r) = exact {
        return r;
    }

    // 접두 매칭 — 날짜 접미사 드리프트 대비 (예: claude-haiku-4-5-20251001)
    if model.starts_with("claude-haiku-4-5") {
        return ModelRate::per_million(1.0, 5.0, 1.25, 2.0, 0.1);
    }

    // 정확 매칭 실패한 claude-* 모델은 폴백으로 "추정"하므로 1회 경고(단가 드리프트 조기 감지).
    if model.starts_with("claude") {
        warn_unpriced_once(model);
    }

    // 패밀리 폴백 (버전 드리프트 대비)
    let m = model.to_lowercase();
    if m.contains("opus") {
        return ModelRate::per_million(5.0, 25.0, 6.25, 10.0, 0.5);
    }
    if m.contains("sonnet") {
        // 4.6 기준($3/$15). Sonnet 5($2/$10)는 정확 매칭에서 잡는다.
        return ModelRate::per_million(3.0, 15.0, 3.75, 6.0, 0.3);
    }
    if m.contains("haiku") {
        return ModelRate::per_million(1.0, 5.0, 1.25, 2.0, 0.1);
    }
    if m.contains("fable") || m.contains("mythos") {
        // 5.1 계열의 캐시읽기 0.025배는 정확 매칭에서만. 폴백은 보수적으로 0.1배(1).
        return ModelRate::per_million(10.0, 50.0, 12.5, 20.0, 1.0);
    }
    if m.contains("gpt") || m.contains("codex") || m.contains("o4") || m.contains("o3") {
        return ModelRate::per_million(5.0, 30.0, 0.0, 0.0, 0.5);
    }
    // Gemini 패밀리 폴백 — pro/flash 만(버전 드리프트 대비). 그 외 gemini 변형은 0(오표시 방지).
    if m.starts_with("gemini") {
        if m.contains("pro") {
            return ModelRate::per_million(1.25, 10.0, 0.0, 0.0, 0.3125);
        }
        if m.contains("flash") {
            return ModelRate::per_million(0.30, 2.5, 0.0, 0.0, 0.075);
        }
    }
    ModelRate::ZERO
}

/// 캐시 쓰기가 5분·1시간으로 나뉜 비용 계산. (1h 는 5m 의 1.6배 단가)
pub fn cost(model: &str, input: i64, output: i64, cache_write_5m: i64, cache_write_1h: i64, cache_read: i64) -> f64 {
    let r = rate(model);
    input as f64 * r.input
        + output as f64 * r.output
        + cache_write_5m as f64 * r.cache_write_5m
        + cache_write_1h as f64 * r.cache_write_1h
        + cache_read as f64 * r.cache_read
}

/// 정확 매칭에 없는 claude 모델을 만나면 stderr 에 1회만 경고 — 폴백은 "추정"이라 조용하면
/// 새 모델의 단가 드리프트(예: 과거 Sonnet 5 가 폴백 $3/$15 로 50% 과대계상됐던 류)를 놓친다.
fn warn_unpriced_once(model: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut set) = seen.lock() {
        if set.insert(model.to_string()) {
            eprintln!(
                "[model_pricing] 경고: '{}' 는 정확 단가 매칭이 없어 패밀리 폴백으로 추정합니다. \
                 공식 가격표 확인 후 rate() 에 정확 매칭을 추가하세요.",
                model
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 인자: cost(model, input, output, cache_write_5m, cache_write_1h, cache_read)
    #[test]
    fn pricing_exact_and_fallback_and_zero() {
        // 기본 입력/출력
        assert!((cost("claude-opus-4-8", 1_000_000, 0, 0, 0, 0) - 5.0).abs() < 1e-6);
        assert!((cost("claude-opus-4-8", 0, 1_000_000, 0, 0, 0) - 25.0).abs() < 1e-6);
        assert!((cost("claude-haiku-4-5-20251001", 1_000_000, 0, 0, 0, 0) - 1.0).abs() < 1e-6);
        // 미지 모델은 여전히 $0 (오표시 방지)
        assert!(cost("totally-unknown", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000).abs() < 1e-9);
        // opus 패밀리 폴백
        assert!((cost("claude-opus-4-99", 1_000_000, 0, 0, 0, 0) - 5.0).abs() < 1e-6);
    }

    // 결함 1 — Fable 이 더 이상 $0 이 아니다
    #[test]
    fn fable_no_longer_zero() {
        assert!((cost("claude-fable-5", 1_000_000, 0, 0, 0, 0) - 10.0).abs() < 1e-6);
        assert!((cost("claude-fable-5", 0, 1_000_000, 0, 0, 0) - 50.0).abs() < 1e-6);
        assert!((cost("claude-fable-5", 0, 0, 0, 0, 1_000_000) - 1.0).abs() < 1e-6); // 캐시읽기 0.1배
    }

    // 결함 1 특수 — Fable 5.1 캐시읽기는 0.025배
    #[test]
    fn fable_5_1_cache_read_is_quarter() {
        assert!((cost("claude-fable-5-1", 0, 0, 0, 0, 1_000_000) - 0.25).abs() < 1e-6);
        assert!((cost("claude-fable-5-1", 1_000_000, 0, 0, 0, 0) - 10.0).abs() < 1e-6);
    }

    // 결함 2 — 1h 와 5m 이 다른 단가로 계산된다
    #[test]
    fn cache_write_1h_vs_5m_differ() {
        assert!((cost("claude-opus-5", 0, 0, 1_000_000, 0, 0) - 6.25).abs() < 1e-6); // 5m
        assert!((cost("claude-opus-5", 0, 0, 0, 1_000_000, 0) - 10.0).abs() < 1e-6); // 1h
        // opus 패밀리 폴백에도 1h 가 들어갔다
        assert!((cost("claude-opus-4-99", 0, 0, 0, 1_000_000, 0) - 10.0).abs() < 1e-6);
    }

    // 결함 3 — Sonnet 5 는 폴백($3/$15)이 아니라 정확 매칭($2/$10)
    #[test]
    fn sonnet_5_exact_not_fallback() {
        assert!((cost("claude-sonnet-5", 1_000_000, 0, 0, 0, 0) - 2.0).abs() < 1e-6);
        assert!((cost("claude-sonnet-5", 0, 1_000_000, 0, 0, 0) - 10.0).abs() < 1e-6);
        assert!((cost("claude-sonnet-4-6", 1_000_000, 0, 0, 0, 0) - 3.0).abs() < 1e-6); // 폴백 기준값
    }

    // opus-5 정확 매칭(폴백 의존 제거)
    #[test]
    fn opus_5_exact() {
        let r = rate("claude-opus-5");
        assert!((r.input * 1_000_000.0 - 5.0).abs() < 1e-6);
        assert!((r.output * 1_000_000.0 - 25.0).abs() < 1e-6);
    }
}
