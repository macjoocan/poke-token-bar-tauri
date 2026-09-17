//! Claude 공식 한도(%) 조회 — 원본 `OAuthLimitsProvider.swift` 이식 (Windows 판).
//!
//! macOS 는 OAuth 토큰이 Keychain 에 있어 접근이 복잡했으나, **Windows 는 토큰이
//! `~/.claude/.credentials.json` 파일에 있어** 파일만 읽으면 된다(Keychain 코드 전부 불필요).
//! 비공식 endpoint 라 실패해도 토큰/사용량 표시엔 영향 없음 — 한도 섹션만 숨긴다.
//!
//! ⚠️ 이 모듈은 로컬 OAuth 토큰을 읽어 Anthropic 서버로 실제 요청을 보낸다(사용자 승인 하에).

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// 자격증명(토큰은 절대 로그/직렬화하지 않는다 — accessToken 은 이 구조 밖으로 안 나간다).
pub struct Credential {
    access_token: String,
    pub expires_at_unix: Option<i64>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
}

impl Credential {
    pub fn is_expired(&self) -> bool {
        match self.expires_at_unix {
            // 원본: 만료 60초 전이면 만료 취급.
            Some(exp) => exp <= chrono::Utc::now().timestamp() + 60,
            None => false,
        }
    }
    /// 표시용 요약(토큰 값 제외 — 안전).
    pub fn safe_summary(&self) -> String {
        format!(
            "subscription={:?}, tier={:?}, expires_at={:?}, expired={}, token_len={}",
            self.subscription_type,
            self.rate_limit_tier,
            self.expires_at_unix,
            self.is_expired(),
            self.access_token.len()
        )
    }
}

/// OAuth 자격증명을 읽는다.
/// - Windows/Linux(및 일부 Mac): `~/.claude/.credentials.json` 파일.
/// - macOS: 파일이 없으면 **키체인** 폴백(Claude Code 가 토큰을 키체인에 저장 — 원본 Swift 와 동일).
pub fn read_claude_credentials() -> Option<Credential> {
    if let Some(c) = read_credentials_from_file() {
        return Some(c);
    }
    #[cfg(target_os = "macos")]
    if let Some(c) = read_credentials_from_macos_keychain() {
        return Some(c);
    }
    None
}

fn read_credentials_from_file() -> Option<Credential> {
    let path = dirs::home_dir()?.join(".claude").join(".credentials.json");
    let data = std::fs::read(path).ok()?;
    parse_credential_json(&data)
}

/// 자격증명 JSON(`{"claudeAiOauth":{...}}`) 바이트 → Credential. 파일·키체인 공용.
fn parse_credential_json(data: &[u8]) -> Option<Credential> {
    let json: Value = serde_json::from_slice(data).ok()?;
    let oauth = json.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str()?;
    if token.is_empty() {
        return None;
    }
    Some(Credential {
        access_token: token.to_string(),
        expires_at_unix: parse_expires_at(oauth.get("expiresAt")),
        subscription_type: oauth.get("subscriptionType").and_then(|v| v.as_str()).map(String::from),
        rate_limit_tier: oauth.get("rateLimitTier").and_then(|v| v.as_str()).map(String::from),
    })
}

/// macOS 키체인에서 Claude Code 자격증명(generic password, service "Claude Code-credentials")을 읽는다.
/// `security` CLI 로 조회(추가 의존성 없음). 미승인/부재 시 None. (원본 OAuthLimitsProvider 의 키체인 경로 대응.)
#[cfg(target_os = "macos")]
fn read_credentials_from_macos_keychain() -> Option<Credential> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // `-w` 는 비밀번호(=자격증명 JSON)를 stdout 에 출력. 끝의 개행 제거 후 파싱.
    let blob = out.stdout;
    let trimmed = blob.strip_suffix(b"\n").unwrap_or(&blob);
    parse_credential_json(trimmed)
}

/// expiresAt — 초 또는 밀리초(>10^10 이면 ms) 휴리스틱. 원본 ISO/숫자 처리 동일.
fn parse_expires_at(raw: Option<&Value>) -> Option<i64> {
    let v = raw?;
    let n: f64 = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.parse().ok()?,
        _ => return None,
    };
    if n <= 0.0 {
        return None;
    }
    let secs = if n > 10_000_000_000.0 { n / 1000.0 } else { n };
    Some(secs as i64)
}

/// usage endpoint 호출 결과(프로브용 — status + 원문 바디).
pub struct UsageResponse {
    pub status: u16,
    pub body: String,
}

/// 공식 한도 endpoint 호출. 토큰은 헤더에만 실린다.
pub fn fetch_usage_raw(cred: &Credential) -> anyhow::Result<UsageResponse> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let resp = client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {}", cred.access_token))
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()?;
    let status = resp.status().as_u16();
    let body = resp.text()?;
    Ok(UsageResponse { status, body })
}

// ── 응답 타입 (팀 계정 실측 확인: five_hour/seven_day/limits[] — 원본 LimitStatus 와 동일) ──

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct LimitWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthLimitEntry {
    kind: Option<String>,
    percent: Option<f64>,
    resets_at: Option<String>,
    #[serde(default)]
    is_active: bool,
    scope: Option<Scope>,
}
#[derive(Debug, Clone, Deserialize)]
struct Scope {
    model: Option<ScopeModel>,
}
#[derive(Debug, Clone, Deserialize)]
struct ScopeModel {
    display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct UsageBody {
    five_hour: Option<LimitWindow>,
    seven_day: Option<LimitWindow>,
    #[serde(default)]
    limits: Vec<OAuthLimitEntry>,
}

/// 한도 게이지 1개(프런트 전달).
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct LimitGauge {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<String>,
    /// 현재 적용 중(활성) 창인지 — 표시 강조용. Codex 다중 버킷에선 stale(창 리셋됨) 버킷을 false 로.
    pub is_active: bool,
    /// 이 게이지 값이 기록된 시각(ISO8601). Codex 는 버킷별로 다를 수 있어 게이지마다 담는다. Claude=None(현재).
    #[serde(default)]
    pub as_of: Option<String>,
}

/// 프런트 전달용 Claude 한도 스냅샷.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaudeLimits {
    /// "Team 5x" 등 (없으면 None).
    pub plan: Option<String>,
    pub gauges: Vec<LimitGauge>,
}

/// rateLimitTier 끝의 배수 토큰("5x"/"20x") 추출. 없으면 None.
fn tier_multiplier(tier: &str) -> Option<String> {
    for part in tier.split('_') {
        if let Some(digits) = part.strip_suffix('x') {
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return Some(part.to_string());
            }
        }
    }
    None
}

/// subscriptionType + 배수 → "Team 5x" / "Pro" / None.
fn plan_display(subscription: Option<&str>, tier: Option<&str>) -> Option<String> {
    let sub = subscription?.trim();
    if sub.is_empty() {
        return None;
    }
    let base = {
        let mut c = sub.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    match tier.and_then(tier_multiplier) {
        Some(mult) => Some(format!("{} {}", base, mult)),
        None => Some(base),
    }
}

/// 응답 바디 + 자격증명 플랜정보 → 게이지 목록. (파서 — 실측 픽스처로 테스트)
pub fn parse_limits(body: &str, subscription: Option<&str>, tier: Option<&str>) -> anyhow::Result<ClaudeLimits> {
    let u: UsageBody = serde_json::from_str(body)?;
    let mut gauges = Vec::new();
    if let Some(w) = &u.five_hour {
        if let Some(util) = w.utilization {
            gauges.push(LimitGauge {
                label: "5시간 세션".into(),
                used_percent: util,
                resets_at: w.resets_at.clone(),
                is_active: true,
                as_of: None,
            });
        }
    }
    if let Some(w) = &u.seven_day {
        if let Some(util) = w.utilization {
            gauges.push(LimitGauge {
                label: "주간".into(),
                used_percent: util,
                resets_at: w.resets_at.clone(),
                is_active: true,
                as_of: None,
            });
        }
    }
    // limits[] 중 모델별 주간(weekly_scoped) 만 추가 — session/weekly_all 은 위에서 이미 표시.
    for e in &u.limits {
        let kind = e.kind.as_deref().unwrap_or("");
        if kind == "session" || kind == "weekly_all" {
            continue;
        }
        let model = e.scope.as_ref().and_then(|s| s.model.as_ref()).and_then(|m| m.display_name.clone());
        let label = match model {
            Some(m) => format!("{} 주간", m),
            None => kind.to_string(),
        };
        gauges.push(LimitGauge {
            label,
            used_percent: e.percent.unwrap_or(0.0),
            resets_at: e.resets_at.clone(),
            is_active: e.is_active,
            as_of: None,
        });
    }
    Ok(ClaudeLimits {
        plan: plan_display(subscription, tier),
        gauges,
    })
}

/// 자격증명 → HTTP 호출 → 파싱. 자격증명 없음/만료/네트워크 실패 시 Err(우아하게 숨김).
pub fn fetch_limits() -> anyhow::Result<ClaudeLimits> {
    let cred = read_claude_credentials().ok_or_else(|| anyhow::anyhow!("no claude credentials"))?;
    let resp = fetch_usage_raw(&cred)?;
    if resp.status != 200 {
        anyhow::bail!("usage endpoint HTTP {}", resp.status);
    }
    parse_limits(&resp.body, cred.subscription_type.as_deref(), cred.rate_limit_tier.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 팀 계정 실측 응답(2026-07-22) — 토큰 없음(사용량/한도만). 파서 회귀 고정.
    const TEAM_BODY: &str = r#"{
      "five_hour": {"resets_at":"2026-07-22T16:20:00.018363+00:00","utilization":9.0},
      "seven_day": {"resets_at":"2026-07-27T07:00:00.018388+00:00","utilization":25.0},
      "limits": [
        {"group":"session","is_active":false,"kind":"session","percent":9,"resets_at":"2026-07-22T16:20:00.018363+00:00","scope":null},
        {"group":"weekly","is_active":true,"kind":"weekly_all","percent":25,"resets_at":"2026-07-27T07:00:00.018388+00:00","scope":null},
        {"group":"weekly","is_active":false,"kind":"weekly_scoped","percent":0,"resets_at":null,"scope":{"model":{"display_name":"Fable","id":null}}}
      ]
    }"#;

    #[test]
    fn parses_team_usage_response() {
        let limits = parse_limits(TEAM_BODY, Some("team"), Some("default_claude_max_5x")).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Team 5x"));
        // five_hour + seven_day + weekly_scoped(Fable) = 3 (session/weekly_all 은 제외)
        assert_eq!(limits.gauges.len(), 3);
        assert_eq!(limits.gauges[0].label, "5시간 세션");
        assert_eq!(limits.gauges[0].used_percent, 9.0);
        assert_eq!(limits.gauges[1].label, "주간");
        assert_eq!(limits.gauges[1].used_percent, 25.0);
        assert_eq!(limits.gauges[2].label, "Fable 주간");
    }

    #[test]
    fn plan_display_variants() {
        assert_eq!(plan_display(Some("team"), Some("default_claude_max_5x")).as_deref(), Some("Team 5x"));
        assert_eq!(plan_display(Some("max"), Some("default_claude_max_20x")).as_deref(), Some("Max 20x"));
        assert_eq!(plan_display(Some("pro"), Some("default_claude_pro")).as_deref(), Some("Pro"));
        assert_eq!(plan_display(None, None), None);
    }

    #[test]
    fn expires_at_seconds_vs_millis() {
        // 초 형식
        assert_eq!(parse_expires_at(Some(&serde_json::json!(1_784_739_517i64))), Some(1_784_739_517));
        // 밀리초 형식(>10^10) → /1000
        assert_eq!(parse_expires_at(Some(&serde_json::json!(1_784_739_517_000i64))), Some(1_784_739_517));
        // 문자열
        assert_eq!(parse_expires_at(Some(&serde_json::json!("1784739517"))), Some(1_784_739_517));
        assert_eq!(parse_expires_at(Some(&serde_json::json!(0))), None);
    }
}
