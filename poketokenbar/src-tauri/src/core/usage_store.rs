//! 사용량 파생값 — 원본 `UsageStore.swift` 의 **순수·테스트 가능 로직**만 이식.
//!
//! 원본 UsageStore 는 macOS 결합(UserDefaults·Timer·NSWorkspace·UNUserNotification)이 큰
//! 클래스지만, refresh 파이프라인/폴링/설정 저장은 Tauri 계층(`lib.rs`)의 관심사다. 여기서는
//! companion 을 구동하고 알림을 판정하는 **부수효과 없는 함수**만 옮긴다:
//!   1. `burn_tier` — 전 프로바이더 합산 burn(tokens/min) → 표시 티어(원본 임계 그대로).
//!   2. `evaluate_limit_alerts` — 한도 알림 엣지 트리거(임계 통과 순간만 1회, 휘발성 필드 미사용).
//!   3. `claude_alert_windows` / `is_limit_warning` — ClaudeLimits 게이지로부터 판정 입력 구성.
//!
//! **전 프로바이더 합산**은 Rust 판에선 `local_usage_reader::all_entries` 가 Claude/Codex/Gemini
//! 를 id 분기 없이 병합하므로(확장 규약 준수) 구조적으로 보장된다 — 별도 per-provider 스냅샷
//! 머신은 불필요. burn 은 병합 엔트리의 단일 `active_block.tokens_per_minute` 가 곧 합산값.

use std::collections::HashMap;

use crate::core::companion_model::BurnTier;
use crate::core::oauth_limits::ClaudeLimits;

/// 전 프로바이더 합산 burn(tokens/min) → `BurnTier`.
///
/// 원본 `UsageStore.burnTier` 임계 그대로 이식(현재 lib.rs 의 근사치 `burn_tier_from_tpm` 대체):
/// `burn > 1000` 이 아니면 idle, `< 100_000` normal, `< 400_000` fast, 그 이상 blazing.
pub fn burn_tier(combined_burn_per_minute: f64) -> BurnTier {
    let burn = combined_burn_per_minute;
    if burn <= 1_000.0 {
        return BurnTier::Idle; // 원본 guard `burn > 1000 else .idle`
    }
    if burn < 100_000.0 {
        return BurnTier::Normal;
    }
    if burn < 400_000.0 {
        return BurnTier::Fast;
    }
    BurnTier::Blazing
}

/// 한도 알림 1건의 발화 지시(순수 판정 결과). 부수효과(실 토스트 전송)와 분리해 테스트 가능하게.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitAlert {
    /// tier 추적·알림 identifier 용 유일 키(창마다 유일, 표시 안 함).
    pub key: String,
    /// 표시용 이름(알림 본문에 노출, 창끼리 중복 가능).
    pub window: String,
    pub is_critical: bool,
    pub utilization: f64,
}

/// 알림 판정(순수·엣지 트리거) — 창별 utilization·임계값·직전 tier 상태로부터
/// *임계값을 새로 넘어선 순간에만* 발화할 알림을 계산하고 tier 상태를 갱신한다.
///
/// - 경고선 통과 1회 + 위험선 통과 1회만. 같은 tier 유지 중엔 재알림 없음(80·81·84… 억제).
/// - utilization 이 경고선 아래로 내려가면(창 리셋 등) 맵에서 제거해 재무장.
/// - `resets_at` 등 매 fetch 변하는 휘발성 필드를 키에 쓰지 않는다(과거 반복-알림 회귀 원인).
/// - 창 식별은 표시명(중복 가능)이 아니라 `key`(창마다 유일)로 — 다른 두 창이 같은 표시명을
///   만들어도 서로의 tier 를 덮어써 억제/중복 발화하던 회귀(#61 계열) 차단.
///
/// 원본 `UsageStore.evaluateLimitAlerts` 이식(파리티 테스트 동반).
pub fn evaluate_limit_alerts(
    windows: &[(String, String, f64)], // (key, name, utilization)
    warn: f64,
    crit: f64,
    tiers: &mut HashMap<String, i32>,
) -> Vec<LimitAlert> {
    let mut alerts = Vec::new();
    for (key, name, utilization) in windows {
        let tier = if *utilization >= crit {
            2
        } else if *utilization >= warn {
            1
        } else {
            0
        };
        // 경고선 아래 → 맵에서 제거해 재무장. 0 을 저장하지 않고 제거하므로 맵은 "현재 상승
        // 중(tier≥1)인 창"만 보유 → 자연히 유한. removeAll 류 상한을 두지 않는다(임계 유지 중인
        // 창의 tier 까지 지워 스스로 재알림을 유발하는 역회귀 방지).
        if tier == 0 {
            tiers.remove(key);
            continue;
        }
        let previous = tiers.get(key).copied().unwrap_or(0);
        if tier <= previous {
            continue; // 같은/낮은 tier → 재알림 안 함
        }
        tiers.insert(key.clone(), tier);
        alerts.push(LimitAlert {
            key: key.clone(),
            window: name.clone(),
            is_critical: tier == 2,
            utilization: *utilization,
        });
    }
    alerts
}

/// ClaudeLimits 게이지 → 알림 판정 입력 `(key, name, utilization)`.
/// key 는 창 정체성(tier·identifier)으로 라벨+인덱스로 유일화(동일 라벨 중복 시 서로 안 덮어쓰게).
pub fn claude_alert_windows(limits: &ClaudeLimits) -> Vec<(String, String, f64)> {
    limits
        .gauges
        .iter()
        .enumerate()
        .map(|(i, g)| (format!("claude.{}.{}", g.label, i), g.label.clone(), g.used_percent))
        .collect()
}

/// 한도 경고 상태 — 어느 게이지든 위험선(crit) 이상이면 true. (원본 `isLimitWarning` 의
/// Claude 게이지 분기 — 5h·주간·모델별 주간 전부 검사. forecast 분기는 블록 토큰 정보가
/// ClaudeLimits 게이지에 없어 후속 이식.)
pub fn is_limit_warning(limits: Option<&ClaudeLimits>, crit: f64) -> bool {
    match limits {
        Some(l) => l.gauges.iter().any(|g| g.used_percent >= crit),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::oauth_limits::LimitGauge;

    // ── burn tier (원본 UsageStoreTests.testBurnTierThresholds 임계 파리티) ──

    #[test]
    fn burn_tier_thresholds() {
        assert_eq!(burn_tier(500.0), BurnTier::Idle); // <=1000 → idle
        assert_eq!(burn_tier(1_000.0), BurnTier::Idle); // 경계: >1000 아님 → idle
        assert_eq!(burn_tier(50_000.0), BurnTier::Normal); // <100k
        assert_eq!(burn_tier(200_000.0), BurnTier::Fast); // <400k
        assert_eq!(burn_tier(500_000.0), BurnTier::Blazing); // >=400k
    }

    #[test]
    fn burn_tier_combines_providers() {
        // 60k + 60k = 120k → fast (합산은 호출부에서 이미 병합된 tpm 을 넘김)
        assert_eq!(burn_tier(120_000.0), BurnTier::Fast);
    }

    // ── 한도 알림 엣지 트리거 (원본 UsageStoreTests 5종 파리티) ──

    fn w(key: &str, name: &str, util: f64) -> (String, String, f64) {
        (key.to_string(), name.to_string(), util)
    }

    /// 같은 tier 유지 중엔 재알림하지 않고, 위험선 통과 시에만 1회 추가 발화.
    /// (회귀: 80·81·84·90·94 매 갱신마다 반복 알림)
    #[test]
    fn fires_once_per_tier_not_every_refresh() {
        let mut tiers: HashMap<String, i32> = HashMap::new();
        let mut eval = |util: f64| evaluate_limit_alerts(&[w("주간", "주간", util)], 80.0, 95.0, &mut tiers);
        assert_eq!(
            eval(80.0),
            vec![LimitAlert { key: "주간".into(), window: "주간".into(), is_critical: false, utilization: 80.0 }]
        );
        assert!(eval(81.0).is_empty()); // 반복 억제
        assert!(eval(84.0).is_empty());
        assert!(eval(90.0).is_empty());
        assert!(eval(94.0).is_empty());
        assert_eq!(
            eval(95.0),
            vec![LimitAlert { key: "주간".into(), window: "주간".into(), is_critical: true, utilization: 95.0 }]
        );
        assert!(eval(96.0).is_empty()); // 위험 tier 유지 → 재알림 없음
        assert!(eval(99.0).is_empty());
    }

    /// 휘발성 resets_at 회귀 직접 재현: 같은 utilization 을 여러 번 평가해도 최초 1회만 발화.
    #[test]
    fn does_not_refire_on_repeated_same_utilization() {
        let mut tiers: HashMap<String, i32> = HashMap::new();
        assert_eq!(evaluate_limit_alerts(&[w("주간", "주간", 90.0)], 80.0, 95.0, &mut tiers).len(), 1);
        assert!(evaluate_limit_alerts(&[w("주간", "주간", 90.0)], 80.0, 95.0, &mut tiers).is_empty());
        assert!(evaluate_limit_alerts(&[w("주간", "주간", 90.0)], 80.0, 95.0, &mut tiers).is_empty());
    }

    /// 경고선 아래로 내려가면(창 리셋 등) 재무장 — 다음 상승 시 새 에피소드로 다시 1회 발화.
    #[test]
    fn rearms_after_dropping_below_warn() {
        let mut tiers: HashMap<String, i32> = HashMap::new();
        let _ = evaluate_limit_alerts(&[w("주간", "주간", 82.0)], 80.0, 95.0, &mut tiers);
        assert!(evaluate_limit_alerts(&[w("주간", "주간", 40.0)], 80.0, 95.0, &mut tiers).is_empty());
        assert_eq!(
            evaluate_limit_alerts(&[w("주간", "주간", 85.0)], 80.0, 95.0, &mut tiers),
            vec![LimitAlert { key: "주간".into(), window: "주간".into(), is_critical: false, utilization: 85.0 }]
        );
    }

    /// 여러 창은 독립 추적 — 5h 가 이미 위험 발화해도 주간은 자기 임계값에서 별도 1회.
    #[test]
    fn tracks_windows_independently() {
        let mut tiers: HashMap<String, i32> = HashMap::new();
        let first = evaluate_limit_alerts(
            &[w("5시간", "5시간", 96.0), w("주간", "주간", 50.0)],
            80.0,
            95.0,
            &mut tiers,
        );
        assert_eq!(
            first,
            vec![LimitAlert { key: "5시간".into(), window: "5시간".into(), is_critical: true, utilization: 96.0 }]
        );
        let second = evaluate_limit_alerts(
            &[w("5시간", "5시간", 97.0), w("주간", "주간", 82.0)],
            80.0,
            95.0,
            &mut tiers,
        );
        assert_eq!(
            second,
            vec![LimitAlert { key: "주간".into(), window: "주간".into(), is_critical: false, utilization: 82.0 }]
        );
    }

    /// 회귀(#61 계열): 서로 다른 창이 **같은 표시명**을 만들어도 tier 는 `key` 로 독립 추적.
    #[test]
    fn key_disambiguates_duplicate_display_names() {
        let mut tiers: HashMap<String, i32> = HashMap::new();
        let alerts = evaluate_limit_alerts(
            &[
                w("codex.codex.individual", "Codex 개인 한도", 90.0),
                w("codex.codex_other.individual", "Codex 개인 한도", 92.0),
            ],
            80.0,
            95.0,
            &mut tiers,
        );
        assert_eq!(alerts.len(), 2, "표시명이 같아도 key 가 다르면 각 창이 독립 발화");
        let keys: std::collections::HashSet<String> = alerts.iter().map(|a| a.key.clone()).collect();
        assert!(keys.contains("codex.codex.individual"));
        assert!(keys.contains("codex.codex_other.individual"));
        assert!(alerts.iter().all(|a| a.window == "Codex 개인 한도"));
        assert_eq!(tiers["codex.codex.individual"], 1);
        assert_eq!(tiers["codex.codex_other.individual"], 1);
        // 재평가 시 같은 tier → 둘 다 억제.
        assert!(evaluate_limit_alerts(
            &[
                w("codex.codex.individual", "Codex 개인 한도", 91.0),
                w("codex.codex_other.individual", "Codex 개인 한도", 93.0),
            ],
            80.0,
            95.0,
            &mut tiers,
        )
        .is_empty());
    }

    // ── 윈도우 구성 + 경고 판정 ──

    fn gauge(label: &str, pct: f64) -> LimitGauge {
        LimitGauge { label: label.into(), used_percent: pct, resets_at: None, is_active: true, as_of: None }
    }

    #[test]
    fn claude_windows_unique_keys_and_warning() {
        let limits = ClaudeLimits {
            plan: Some("Team 5x".into()),
            gauges: vec![gauge("5시간 세션", 42.0), gauge("주간", 96.0)],
        };
        let windows = claude_alert_windows(&limits);
        assert_eq!(windows.len(), 2);
        // 라벨+인덱스로 유일 — 동일 라벨이 와도 충돌 없음.
        let keys: std::collections::HashSet<&String> = windows.iter().map(|(k, _, _)| k).collect();
        assert_eq!(keys.len(), 2);
        assert!(is_limit_warning(Some(&limits), 95.0)); // 주간 96 ≥ 95
        assert!(!is_limit_warning(Some(&limits), 97.0));
        assert!(!is_limit_warning(None, 95.0));
    }
}
