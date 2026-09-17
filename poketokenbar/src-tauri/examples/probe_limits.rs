//! Claude 공식 한도 endpoint 실측 프로브 — 팀 요금제가 무엇을 돌려주는지 확인.
//! 실행: cargo run --example probe_limits
//!
//! ⚠️ 로컬 OAuth 토큰을 읽어 Anthropic 서버로 요청을 보낸다(사용자 승인 하에).
//!    토큰 값 자체는 절대 출력하지 않는다 — 요약(길이/만료)과 응답 바디만.

use poketokenbar_lib::core::oauth_limits;

fn main() {
    let cred = match oauth_limits::read_claude_credentials() {
        Some(c) => c,
        None => {
            println!("자격증명 없음(~/.claude/.credentials.json 부재 또는 형식 불일치)");
            return;
        }
    };
    println!("=== 자격증명 요약(토큰 값 제외) ===");
    println!("{}", cred.safe_summary());
    println!();

    if cred.is_expired() {
        println!("⚠️ 토큰 만료됨 — 응답이 401 일 수 있음(그래도 요청은 시도).");
    }

    println!("=== GET /api/oauth/usage 응답 ===");
    match oauth_limits::fetch_usage_raw(&cred) {
        Ok(r) => {
            println!("HTTP {}", r.status);
            // 응답 바디에는 토큰이 없다(사용량/한도 데이터만) — 그대로 출력해 팀 응답 구조 확인.
            match serde_json::from_str::<serde_json::Value>(&r.body) {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or(r.body)),
                Err(_) => println!("{}", r.body),
            }
        }
        Err(e) => println!("요청 실패: {}", e),
    }
}
