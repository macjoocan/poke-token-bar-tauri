//! Codex 공식 한도(%) — **로컬 로그 파싱판**.
//!
//! 원본 `CodexRateLimitsProvider.swift` 는 codex 바이너리를 `app-server --stdio` JSON-RPC 로 띄워
//! `account/rateLimits/read` 를 호출하지만, Windows 판은 사용량과 마찬가지로 **`rate_limits` 가
//! 로컬 rollout `token_count` 이벤트에 이미 담겨 있어** 네트워크/프로세스 없이 최신 이벤트를 읽는다.
//!
//! ⚠️ Codex 는 한도가 **모델별 버킷으로 분리**된다(`limit_id`). 예: `codex`(메인) vs
//! `codex_bengalfox`(=GPT-5.3-Codex-Spark). 그래서 "전역 최신 이벤트 1개"만 보면, 방금 잠깐 쓴
//! 별도 모델 버킷(0%)이 실제 많이 쓴 메인 버킷(65%)을 가려 **0%로 오인**된다.
//! → **버킷별 최신 상태를 각각 게이지로** 보여주고(라벨로 구분), 사용률 높은 순으로 정렬한다.
//! 창(window)이 이미 리셋된 오래된 버킷은 `is_active=false`(stale)로 표시하고 게이지에 기준시각을 담는다.

use std::collections::HashMap;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use super::local_usage_reader as reader;
use super::models;
use super::oauth_limits::LimitGauge; // 게이지 표현 재사용(프런트 ClaudeLimits 와 동일 렌더)

/// 프런트 전달용 Codex 한도 스냅샷(Claude 와 동일 형태 — Gauge 컴포넌트 공유).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CodexLimits {
    /// "Prolite"/"Plus"/"Team" 등(plan_type title-case).
    pub plan: Option<String>,
    pub gauges: Vec<LimitGauge>,
    /// 채택한 이벤트 중 **가장 최신** 시각(ISO8601). 게이지별 기준시각은 각 LimitGauge.as_of 참조.
    #[serde(default)]
    pub as_of: Option<String>,
}

/// window_minutes → 사람이 읽는 라벨.
fn window_label(mins: Option<i64>, fallback: &str) -> String {
    match mins {
        Some(m) if m >= 10080 => "주간".into(),
        Some(m) if m >= 1440 => format!("{}일", m / 1440),
        Some(m) if m >= 60 => format!("{}시간", m / 60),
        Some(m) if m > 0 => format!("{}분", m),
        _ => fallback.into(),
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// 버킷 표시명 — limit_name 있으면 마지막 하이픈 토큰("GPT-5.3-Codex-Spark"→"Spark"),
/// 없으면 limit_id 매핑("codex"→"메인", 그 외 title-case).
fn bucket_short(limit_id: &str, limit_name: &str) -> String {
    let name = limit_name.trim();
    if !name.is_empty() {
        return name.rsplit('-').next().unwrap_or(name).trim().to_string();
    }
    match limit_id {
        "codex" => "메인".into(),
        other => title_case(other),
    }
}

/// 창(primary/secondary) 하나 → 게이지. used_percent 없으면 None.
fn window_gauge(w: &Value, fallback: &str, prefix: Option<&str>, as_of: Option<&str>, is_active: bool) -> Option<LimitGauge> {
    let pct = w.get("used_percent")?.as_f64()?;
    let mins = w.get("window_minutes").and_then(|v| v.as_i64());
    let win = window_label(mins, fallback);
    let label = match prefix {
        Some(p) => format!("{} · {}", p, win),
        None => win,
    };
    Some(LimitGauge {
        label,
        used_percent: pct,
        resets_at: None,
        is_active,
        as_of: as_of.map(String::from),
    })
}

/// 단일 `rate_limits` JSON → CodexLimits (버킷 프리픽스 없음, primary→secondary 순). 파서 단위 테스트용.
pub fn parse_codex_rate_limits(rl: &Value) -> CodexLimits {
    let mut gauges = Vec::new();
    for (key, fallback) in [("primary", "세션"), ("secondary", "주간")] {
        if let Some(w) = rl.get(key).filter(|v| v.is_object()) {
            if let Some(g) = window_gauge(w, fallback, None, None, true) {
                gauges.push(g);
            }
        }
    }
    let plan = rl
        .get("plan_type")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(title_case);
    CodexLimits { plan, gauges, as_of: None }
}

/// 버킷 하나의 최신 상태.
struct BucketState {
    limit_id: String,
    limit_name: String,
    ts: String,
    rl: Value,
}

fn window_has_used(rl: &Value, key: &str) -> bool {
    rl.get(key)
        .and_then(|w| w.get("used_percent"))
        .and_then(|v| v.as_f64())
        .is_some()
}

/// 버킷의 primary 창이 아직 리셋 안 됐는지(=값이 현재 유효한지). 최신 이벤트가 창 길이보다 오래됐으면 stale.
fn is_fresh(ts: &str, rl: &Value, now: DateTime<Utc>) -> bool {
    let window_min = rl
        .get("primary")
        .and_then(|p| p.get("window_minutes"))
        .and_then(|v| v.as_i64());
    let window_min = match window_min {
        Some(m) if m > 0 => m,
        _ => return true, // 창 길이 모르면 dim 하지 않음
    };
    match models::parse_iso8601(ts) {
        Some(t) => now.signed_duration_since(t).num_minutes() <= window_min,
        None => true,
    }
}

/// 여러 버킷의 최신 상태 → CodexLimits. 사용 가능한 창이 없는 버킷은 제외, 사용률 높은 순 정렬.
fn combine_buckets(mut buckets: Vec<BucketState>, now: DateTime<Utc>) -> Option<CodexLimits> {
    buckets.retain(|b| window_has_used(&b.rl, "primary") || window_has_used(&b.rl, "secondary"));
    if buckets.is_empty() {
        return None;
    }
    let multi = buckets.len() > 1;
    let newest = buckets.iter().map(|b| b.ts.clone()).max();
    let plan = buckets
        .iter()
        .find_map(|b| b.rl.get("plan_type").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .map(title_case);

    let mut gauges: Vec<LimitGauge> = Vec::new();
    for b in &buckets {
        let prefix = if multi {
            Some(bucket_short(&b.limit_id, &b.limit_name))
        } else {
            None
        };
        let fresh = is_fresh(&b.ts, &b.rl, now);
        for (key, fallback) in [("primary", "세션"), ("secondary", "주간")] {
            if let Some(w) = b.rl.get(key).filter(|v| v.is_object()) {
                if let Some(g) = window_gauge(w, fallback, prefix.as_deref(), Some(&b.ts), fresh) {
                    gauges.push(g);
                }
            }
        }
    }
    // 사용률 높은 순 — 한도에 가까운 버킷이 먼저(0% 버킷에 가려지지 않게). 동률이면 활성 우선.
    gauges.sort_by(|a, b| {
        b.used_percent
            .partial_cmp(&a.used_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.is_active.cmp(&a.is_active))
    });
    Some(CodexLimits { plan, gauges, as_of: newest })
}

/// 최근 Codex rollout 로그에서 **버킷(limit_id)별 최신** `rate_limits` 이벤트를 모아 파싱.
/// 데이터 없음(Codex 미사용/구버전) 시 None → 프런트는 Codex 한도 섹션만 숨김.
pub fn read_codex_limits(modified_since: Option<SystemTime>) -> Option<CodexLimits> {
    let dir = reader::codex_sessions_dir()?;
    let files = reader::jsonl_files(&dir, modified_since, false);
    let mut latest: HashMap<String, BucketState> = HashMap::new();
    for file in &files {
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for line in text.split('\n') {
            if !line.contains("rate_limits") {
                continue; // fast-path
            }
            let obj: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // rate_limits 는 payload 안에 있다(payload.type/info 의 형제).
            let rl = match obj.get("payload").and_then(|p| p.get("rate_limits")) {
                Some(v) if v.is_object() => v,
                _ => continue,
            };
            let ts = obj.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let limit_id = rl.get("limit_id").and_then(|v| v.as_str()).unwrap_or("codex").to_string();
            let limit_name = rl.get("limit_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            // 버킷별 최신(사전식 = 시간 비교) 채택.
            match latest.get(&limit_id) {
                Some(b) if b.ts.as_str() >= ts.as_str() => {}
                _ => {
                    latest.insert(
                        limit_id.clone(),
                        BucketState { limit_id, limit_name, ts, rl: rl.clone() },
                    );
                }
            }
        }
    }
    if latest.is_empty() {
        return None;
    }
    combine_buckets(latest.into_values().collect(), Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 실측 rate_limits(2026-09-16, prolite 플랜) — 단일 버킷 파서 회귀 고정.
    const SAMPLE: &str = r#"{"limit_id":"codex_bengalfox","limit_name":"GPT-5.3-Codex-Spark",
      "primary":{"used_percent":12.5,"window_minutes":300,"resets_at":1789573158},
      "secondary":{"used_percent":47.0,"window_minutes":10080,"resets_at":1790159958},
      "credits":{"has_credits":false,"unlimited":false,"balance":"0"},
      "individual_limit":null,"spend_control_reached":null,"plan_type":"prolite","rate_limit_reached_type":null}"#;

    #[test]
    fn parses_primary_secondary_and_plan() {
        let rl: Value = serde_json::from_str(SAMPLE).unwrap();
        let limits = parse_codex_rate_limits(&rl);
        assert_eq!(limits.plan.as_deref(), Some("Prolite"));
        assert_eq!(limits.gauges.len(), 2);
        assert_eq!(limits.gauges[0].label, "5시간"); // 300분, primary-first(정렬 안 함)
        assert_eq!(limits.gauges[0].used_percent, 12.5);
        assert_eq!(limits.gauges[1].label, "주간"); // 10080분
        assert_eq!(limits.gauges[1].used_percent, 47.0);
    }

    #[test]
    fn window_label_variants() {
        assert_eq!(window_label(Some(300), "x"), "5시간");
        assert_eq!(window_label(Some(10080), "x"), "주간");
        assert_eq!(window_label(Some(60), "x"), "1시간");
        assert_eq!(window_label(Some(2880), "x"), "2일");
        assert_eq!(window_label(None, "세션"), "세션");
    }

    #[test]
    fn missing_windows_yield_empty_gauges() {
        let rl: Value = serde_json::from_str(r#"{"plan_type":"plus"}"#).unwrap();
        let limits = parse_codex_rate_limits(&rl);
        assert_eq!(limits.plan.as_deref(), Some("Plus"));
        assert!(limits.gauges.is_empty());
    }

    #[test]
    fn bucket_short_names() {
        assert_eq!(bucket_short("codex_bengalfox", "GPT-5.3-Codex-Spark"), "Spark");
        assert_eq!(bucket_short("codex", ""), "메인");
        assert_eq!(bucket_short("premium", ""), "Premium");
    }

    // 핵심 — 메인(65%, 어제·stale) + Spark(0%, 오늘·fresh) 두 버킷을 모두 보여주고 65% 를 앞세운다.
    #[test]
    fn combine_multi_bucket_prefers_high_usage_and_marks_stale() {
        let now = models::parse_iso8601("2026-09-17T05:10:00Z").unwrap();
        let main = BucketState {
            limit_id: "codex".into(),
            limit_name: "".into(),
            ts: "2026-09-16T09:11:11Z".into(), // 어제 — 5시간 창 리셋됨 → stale
            rl: serde_json::json!({"limit_id":"codex","plan_type":"prolite",
                "primary":{"used_percent":65.0,"window_minutes":300}}),
        };
        let spark = BucketState {
            limit_id: "codex_bengalfox".into(),
            limit_name: "GPT-5.3-Codex-Spark".into(),
            ts: "2026-09-17T05:02:05Z".into(), // 오늘 — fresh
            rl: serde_json::json!({"limit_id":"codex_bengalfox","limit_name":"GPT-5.3-Codex-Spark","plan_type":"prolite",
                "primary":{"used_percent":0.0,"window_minutes":300},
                "secondary":{"used_percent":0.0,"window_minutes":10080}}),
        };
        let limits = combine_buckets(vec![spark, main], now).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Prolite"));
        assert_eq!(limits.gauges.len(), 3); // 메인 primary + Spark primary + Spark secondary
        // 사용률 높은 순 → 메인 65% 가 먼저(0% 에 안 가려짐)
        assert_eq!(limits.gauges[0].used_percent, 65.0);
        assert_eq!(limits.gauges[0].label, "메인 · 5시간");
        assert!(!limits.gauges[0].is_active, "어제 버킷은 stale");
        assert_eq!(limits.gauges[0].as_of.as_deref(), Some("2026-09-16T09:11:11Z"));
        // Spark 버킷은 오늘 fresh
        assert!(limits.gauges.iter().any(|g| g.label == "Spark · 5시간" && g.is_active));
        // 카드 기준시각 = 가장 최신
        assert_eq!(limits.as_of.as_deref(), Some("2026-09-17T05:02:05Z"));
    }

    // 단일 버킷이면 프리픽스 없이 창 라벨만.
    #[test]
    fn combine_single_bucket_no_prefix() {
        let now = models::parse_iso8601("2026-09-17T05:10:00Z").unwrap();
        let only = BucketState {
            limit_id: "codex".into(),
            limit_name: "".into(),
            ts: "2026-09-17T05:05:00Z".into(),
            rl: serde_json::json!({"limit_id":"codex","plan_type":"prolite",
                "primary":{"used_percent":30.0,"window_minutes":300}}),
        };
        let limits = combine_buckets(vec![only], now).unwrap();
        assert_eq!(limits.gauges.len(), 1);
        assert_eq!(limits.gauges[0].label, "5시간"); // 프리픽스 없음
        assert!(limits.gauges[0].is_active);
    }
}
