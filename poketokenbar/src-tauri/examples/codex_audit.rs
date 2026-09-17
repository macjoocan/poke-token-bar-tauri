//! Codex 다중 버킷 한도 실측 — 고친 read_codex_limits 가 실제 로그에서 뭘 뽑는지 확인.
//! 실행: cargo run --example codex_audit
use std::time::{Duration, SystemTime};
use poketokenbar_lib::core::codex_rate_limits::read_codex_limits;

fn main() {
    let since = SystemTime::now().checked_sub(Duration::from_secs(8 * 24 * 3600));
    match read_codex_limits(since) {
        Some(l) => {
            println!("plan: {:?}", l.plan);
            println!("as_of(최신): {:?}", l.as_of);
            println!("게이지 {}개 (사용률 높은 순):", l.gauges.len());
            for g in &l.gauges {
                println!(
                    "  {:22} {:>6.1}%  active={}  as_of={}",
                    g.label, g.used_percent, g.is_active,
                    g.as_of.as_deref().unwrap_or("-")
                );
            }
        }
        None => println!("Codex 한도 데이터 없음"),
    }
}
