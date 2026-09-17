//! 단가 보정 검증 — 고친 앱 코드로 실제 Claude 로그의 모델별 비용을 계산해
//! `단가_보정_실측.js` 의 "공식 단가" 열과 대조한다. (두 독립 구현이 같은 값을 내야 통과.)
//! 실행: cargo run --example price_audit

use std::collections::BTreeMap;

use poketokenbar_lib::core::local_usage_reader as reader;
use poketokenbar_lib::core::model_pricing;

fn main() {
    let entries = reader::claude_entries(None, None); // 전체 로그, 전역 dedup
    let mut by_model: BTreeMap<String, (f64, i64)> = BTreeMap::new();
    let mut total = 0.0;
    for e in &entries {
        let c = model_pricing::cost(
            &e.model,
            e.input,
            e.output,
            e.cache_write_5m,
            e.cache_write_1h,
            e.cache_read,
        );
        let ent = by_model.entry(e.model.clone()).or_insert((0.0, 0));
        ent.0 += c;
        ent.1 += 1;
        total += c;
    }
    println!("중복 제거 후 {} 건 (claude)\n", entries.len());
    println!("{:34}{:>14}{:>8}", "모델", "공식 단가(앱)", "응답");
    println!("{}", "-".repeat(58));
    for (m, (c, n)) in &by_model {
        println!("{:34}{:>14}{:>8}", m, format!("${:.2}", c), n);
    }
    println!("{}", "-".repeat(58));
    println!("{:34}{:>14}", "합계", format!("${:.2}", total));
}
