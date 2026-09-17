//! 실데이터 스모크 테스트 — 이 머신의 실제 ~/.claude·~/.codex·~/.gemini 로그를 파싱해
//! 오늘/블록 집계가 0이 아닌지 눈으로 확인한다 (M0 경로검증 + M1 파서의 end-to-end 확인).
//!
//! 실행: cargo run --example smoke

use std::time::{Duration, SystemTime};

use poketokenbar_lib::core::local_usage_reader as reader;
use poketokenbar_lib::core::models;

fn main() {
    let since = SystemTime::now().checked_sub(Duration::from_secs(3 * 24 * 3600));

    let claude = reader::claude_entries(since, None);
    let codex = reader::codex_entries(since, None);
    let gemini = reader::gemini_entries(since, None);

    println!("=== 최근 3일 mtime 윈도우 엔트리 수 ===");
    println!("Claude: {}", claude.len());
    println!("Codex : {}", codex.len());
    println!("Gemini: {} (미설치면 0)", gemini.len());

    let all = reader::all_entries(since);
    let today = models::today_key();
    println!("\n오늘({today}) 집계:");
    match reader::daily(&all, &today) {
        Some(d) => println!(
            "  total={} tokens (in={}, out={}, cacheW={}, cacheR={}), cost=${:.4}",
            d.total_tokens, d.input_tokens, d.output_tokens, d.cache_creation_tokens, d.cache_read_tokens, d.total_cost
        ),
        None => println!("  (오늘 데이터 없음)"),
    }

    let now = chrono::Utc::now();
    match reader::active_block(&all, now) {
        Some(b) => println!(
            "5h 활성 블록: total={} tokens, {:.1} tok/min, cost=${:.4}",
            b.total_tokens,
            b.tokens_per_minute.unwrap_or(0.0),
            b.cost_usd
        ),
        None => println!("5h 활성 블록: (없음)"),
    }
}
