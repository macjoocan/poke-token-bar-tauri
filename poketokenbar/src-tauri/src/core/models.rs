//! 데이터 모델 + 날짜 유틸. 원본 `Models.swift` 이식 (집계 결과 구조체 + ISO8601Parser).
//!
//! 원본은 ccusage JSON 을 디코드하기 위한 유연한 CodingKeys 를 갖지만, Rust 판은 ccusage 를
//! 호출하지 않고 로컬 로그를 직접 파싱해 이 구조체를 **생성**하므로 필드만 보존한다.
//! (프런트로 invoke 전달 위해 Serialize.)

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

/// 일자별 집계 (원본 DailyUsage).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DailyUsage {
    pub date: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
}

/// 주/월 기간 집계 (원본 PeriodUsage).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeriodUsage {
    /// 주 시작일("2026-05-31") 또는 월("2026-06")
    pub period: String,
    pub total_tokens: i64,
    pub total_cost: f64,
}

/// 활성 블록 (원본 BlockUsage) — 5시간 롤링 윈도우 기반 번 레이트 추정.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockUsage {
    pub id: String,
    pub start_time: String,
    pub end_time: String,
    pub is_active: bool,
    pub total_tokens: i64,
    pub cost_usd: f64,
    /// burnRate.tokensPerMinute — 한도 소진 예측·companion 표시 상태에 사용.
    pub tokens_per_minute: Option<f64>,
}

/// 프런트로 전달하는 "오늘" 스냅샷 (today 일자 집계 + 5h 활성 블록).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageSnapshot {
    pub today: Option<DailyUsage>,
    pub block: Option<BlockUsage>,
}

/// ISO8601 타임스탬프 파싱. 원본 `ISO8601Parser` 이식.
///
/// 원본은 마이크로초("...034464+00:00")/밀리초("....303Z") 를 수동 처리하지만, chrono 의
/// `parse_from_rfc3339` 는 임의 자릿수 소수초 + 오프셋('Z'/±hh:mm)을 모두 처리하므로 그대로 위임한다.
pub fn parse_iso8601(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// 로컬 타임존 기준 "yyyy-MM-dd" (원본 localDayFormatter — TimeZone .current).
pub fn local_day(dt: &DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%Y-%m-%d").to_string()
}

/// 오늘(로컬) 날짜 키.
pub fn today_key() -> String {
    local_day(&Utc::now())
}

/// 로컬 "yyyy-MM" 월 키 (원본 monthKey).
pub fn month_key(dt: &DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%Y-%m").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_parses_millis_and_micros_and_z() {
        assert!(parse_iso8601("2026-06-30T10:00:00.000Z").is_some());
        assert!(parse_iso8601("2026-06-30T10:00:00.034464+00:00").is_some());
        assert!(parse_iso8601("2026-06-30T10:00:00Z").is_some());
        assert!(parse_iso8601("not-a-date").is_none());
    }

    #[test]
    fn iso8601_instant_is_stable() {
        // 같은 순간을 다른 오프셋 표기로 줘도 동일 UTC 로 정규화.
        let a = parse_iso8601("2026-06-30T10:00:00.000Z").unwrap();
        let b = parse_iso8601("2026-06-30T19:00:00.000+09:00").unwrap();
        assert_eq!(a, b);
    }
}
