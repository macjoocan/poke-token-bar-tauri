//! 토큰/비용 표시 포맷. 원본 `TokenFormatter.swift` 이식.

/// 987 → "987", 12_345 → "12.3K", 190_612_940 → "190.6M", 1_240_000_000 → "1.24B"
pub fn compact(value: i64) -> String {
    let v = (value.abs()) as f64;
    let sign = if value < 0 { "-" } else { "" };
    if v < 1_000.0 {
        format!("{}", value)
    } else if v < 1_000_000.0 {
        format!("{}{}K", sign, trim(v / 1_000.0, 1))
    } else if v < 1_000_000_000.0 {
        format!("{}{}M", sign, trim(v / 1_000_000.0, 1))
    } else {
        format!("{}{}B", sign, trim(v / 1_000_000_000.0, 2))
    }
}

/// 팝오버 상세용 천 단위 구분 (190,612,940)
pub fn grouped(value: i64) -> String {
    let s = value.abs().to_string();
    let bytes = s.as_bytes();
    let mut out = String::new();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    if value < 0 {
        format!("-{}", out)
    } else {
        out
    }
}

pub fn cost(usd: f64) -> String {
    format!("${:.2}", usd)
}

/// 메뉴바용 짧은 비용 표기: $9.5 / $311 / $1.2K
pub fn cost_compact(usd: f64) -> String {
    if usd < 100.0 {
        format!("${:.1}", usd)
    } else if usd < 10_000.0 {
        format!("${:.0}", usd)
    } else {
        format!("${:.1}K", usd / 1_000.0)
    }
}

pub fn percent(value: f64) -> String {
    if value == value.round() {
        format!("{:.0}%", value)
    } else {
        format!("{:.1}%", value)
    }
}

/// 소수 자리 뒤 불필요한 0 과 소수점 제거 ("12.30" → "12.3", "5.0" → "5").
fn trim(value: f64, decimals: usize) -> String {
    let mut s = format!("{:.*}", decimals, value);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // 원본 TokenFormatter.swift 독스트링 예시를 명세로 이식
    #[test]
    fn compact_matches_examples() {
        assert_eq!(compact(987), "987");
        assert_eq!(compact(12_345), "12.3K");
        assert_eq!(compact(190_612_940), "190.6M");
        assert_eq!(compact(1_240_000_000), "1.24B");
        assert_eq!(compact(0), "0");
        assert_eq!(compact(-12_345), "-12.3K");
    }

    #[test]
    fn grouped_thousands() {
        assert_eq!(grouped(190_612_940), "190,612,940");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
    }

    #[test]
    fn cost_and_percent() {
        assert_eq!(cost(9.5), "$9.50");
        assert_eq!(cost_compact(9.5), "$9.5");
        assert_eq!(cost_compact(311.0), "$311");
        // 원본: usd < 10_000 이면 "$%.0f" → 1200 은 "$1200". K 표기는 10_000 이상부터.
        assert_eq!(cost_compact(1_200.0), "$1200");
        assert_eq!(cost_compact(12_000.0), "$12.0K");
        assert_eq!(percent(80.0), "80%");
        assert_eq!(percent(80.5), "80.5%");
    }
}
