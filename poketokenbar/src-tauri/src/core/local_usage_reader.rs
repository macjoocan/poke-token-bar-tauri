//! Claude/Codex/Gemini 로컬 사용 로그를 직접 파싱해 토큰/비용을 집계한다.
//! 원본 `LocalUsageReader.swift` 이식 (ccusage CLI 대체).
//!
//! - Claude: `~/.claude/projects/**/*.jsonl` 의 `type:"assistant"` 라인
//!   (`message.usage` 4종 토큰, `message.model`, `message.id`+`requestId`, `timestamp`).
//!   세션 재개/sidechain 으로 같은 메시지가 여러 파일에 중복 → `(message.id, requestId)` 로 dedup.
//! - Codex: `~/.codex/sessions/**/rollout-*.jsonl` 의 `payload.type:"token_count"`
//!   (`info.last_token_usage` 턴 델타) 합산.
//! - Gemini: `~/.gemini/tmp/<hash>/chats/session-*.jsonl`(+레거시 .json) 의 message tokens.
//!
//! 성능: mtime 윈도우로 스캔 파일을 한정(범위 시작 이전에 수정된 파일은 범위 내 엔트리가 없음).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model_pricing;
use super::models::{self, BlockUsage, DailyUsage, PeriodUsage};

// MARK: 정규화 레코드

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub date: DateTime<Utc>,
    pub local_day: String,
    pub model: String,
    pub input: i64,
    pub output: i64,
    // 캐시 쓰기는 5분/1시간 단가가 달라 분리 저장(단가_보정_작업지시.md 결함 2).
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
    pub cache_read: i64,
}

impl Entry {
    /// 표시용 캐시 쓰기 합계(5m+1h) — 금액이 아니라 토큰 수.
    pub fn cache_write(&self) -> i64 {
        self.cache_write_5m + self.cache_write_1h
    }
    pub fn total(&self) -> i64 {
        self.input + self.output + self.cache_write() + self.cache_read
    }
}

#[derive(Debug, Default)]
struct Bucket {
    input: i64,
    output: i64,
    cache_write_5m: i64,
    cache_write_1h: i64,
    cache_read: i64,
    cost: f64,
}

impl Bucket {
    fn total(&self) -> i64 {
        self.input + self.output + self.cache_write_5m + self.cache_write_1h + self.cache_read
    }
    fn add(&mut self, e: &Entry) {
        self.input += e.input;
        self.output += e.output;
        self.cache_write_5m += e.cache_write_5m;
        self.cache_write_1h += e.cache_write_1h;
        self.cache_read += e.cache_read;
        self.cost += model_pricing::cost(
            &e.model,
            e.input,
            e.output,
            e.cache_write_5m,
            e.cache_write_1h,
            e.cache_read,
        );
    }
}

// MARK: 경로

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}
pub fn claude_projects_dir() -> Option<PathBuf> {
    home().map(|h| h.join(".claude").join("projects"))
}
pub fn codex_sessions_dir() -> Option<PathBuf> {
    home().map(|h| h.join(".codex").join("sessions"))
}
pub fn gemini_tmp_dir() -> Option<PathBuf> {
    home().map(|h| h.join(".gemini").join("tmp"))
}

// MARK: 스캔 (mtime 윈도우)

/// `root` 하위(재귀)의 `.jsonl` 파일 중 `modified_since` 이후 수정된 것.
/// `modified_since = None` 이면 전부 포함(원본 `.distantPast` 상당).
/// `.json` 은 Gemini 전용(`allow_json`) — Claude 루트 .meta.json 스캔 방지.
pub fn jsonl_files(root: &Path, modified_since: Option<SystemTime>, allow_json: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        // 숨김 파일 스킵(원본 .skipsHiddenFiles)
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !(ext == "jsonl" || (allow_json && ext == "json")) {
            continue;
        }
        if let Some(since) = modified_since {
            let m = entry.metadata().ok().and_then(|md| md.modified().ok());
            match m {
                Some(mtime) if mtime >= since => out.push(path.to_path_buf()),
                _ => {}
            }
        } else {
            out.push(path.to_path_buf());
        }
    }
    out
}

// MARK: 공통 유틸

fn int_value(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i
            } else {
                n.as_f64().map(|f| f as i64).unwrap_or(0)
            }
        }
        _ => 0,
    }
}

// MARK: Claude 파싱

/// 같은 id 가 스트리밍/재개로 여러 번 로깅될 때 output 은 증가하므로 **id 별 total 이 가장 큰(=완성된)
/// 항목**을 남긴다(전역 dedup). first-occurrence 를 남기면 부분 output 만 잡혀 비용이 과소집계됨.
pub fn dedup_keep_max(entries: Vec<Entry>) -> Vec<Entry> {
    let mut by_id: HashMap<String, Entry> = HashMap::new();
    for e in entries {
        match by_id.get(&e.id) {
            Some(ex) if e.total() <= ex.total() => {}
            _ => {
                by_id.insert(e.id.clone(), e);
            }
        }
    }
    by_id.into_values().collect()
}

fn parse_claude_line(line: &str) -> Option<Entry> {
    let obj: Value = serde_json::from_str(line).ok()?;
    if obj.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let msg = obj.get("message")?;
    let usage = msg.get("usage")?;
    let ts = obj.get("timestamp")?.as_str()?;
    let date = models::parse_iso8601(ts)?;
    let model = msg.get("model").and_then(|v| v.as_str()).unwrap_or("unknown");
    let id = format!(
        "{}|{}",
        msg.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        obj.get("requestId").and_then(|v| v.as_str()).unwrap_or("")
    );
    // 캐시 쓰기 5m/1h 분리 — usage.cache_creation.{ephemeral_1h,ephemeral_5m}_input_tokens.
    // 구버전 로그(cache_creation 객체 없음)는 잔여분을 5분으로 봐 기존 동작을 보존한다.
    let cw_total = int_value(usage.get("cache_creation_input_tokens"));
    let cc = usage.get("cache_creation");
    let w1h = cc.map(|c| int_value(c.get("ephemeral_1h_input_tokens"))).unwrap_or(0);
    let w5m = cc.map(|c| int_value(c.get("ephemeral_5m_input_tokens"))).unwrap_or(0);
    let rest = (cw_total - w1h - w5m).max(0);
    Some(Entry {
        id,
        date,
        local_day: models::local_day(&date),
        model: model.to_string(),
        input: int_value(usage.get("input_tokens")),
        output: int_value(usage.get("output_tokens")),
        cache_write_5m: w5m + rest,
        cache_write_1h: w1h,
        cache_read: int_value(usage.get("cache_read_input_tokens")),
    })
}

/// Claude 파일 하나를 파싱(파일 내 dedup).
pub fn parse_claude_file(path: &Path) -> Vec<Entry> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        // fast-path: 두 리터럴이 모두 있는 라인만 JSON 파싱
        if !(line.contains("\"usage\"") && line.contains("\"assistant\"")) {
            continue;
        }
        if let Some(e) = parse_claude_line(line) {
            out.push(e);
        }
    }
    dedup_keep_max(out)
}

pub fn claude_entries(modified_since: Option<SystemTime>, root: Option<&Path>) -> Vec<Entry> {
    let default_dir = claude_projects_dir();
    let root = match root.map(|r| r.to_path_buf()).or(default_dir) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut all = Vec::new();
    for file in jsonl_files(&root, modified_since, false) {
        all.extend(parse_claude_file(&file));
    }
    dedup_keep_max(all)
}

// MARK: Codex 파싱

/// info.last_token_usage(턴 델타)를 4종 토큰으로 매핑.
/// input(비캐시)=input−cached, cacheRead=cached, output=output_tokens(reasoning 포함), cacheWrite=0.
fn parse_codex_line(line: &str, file: &str, turn: usize, model: &str) -> Option<Entry> {
    let obj: Value = serde_json::from_str(line).ok()?;
    let payload = obj.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let last = payload.get("info")?.get("last_token_usage")?;
    let ts = obj.get("timestamp")?.as_str()?;
    let date = models::parse_iso8601(ts)?;
    let input_total = int_value(last.get("input_tokens"));
    let cached = int_value(last.get("cached_input_tokens"));
    let output = int_value(last.get("output_tokens"));
    let non_cached_input = (input_total - cached).max(0);
    Some(Entry {
        id: format!("codex|{}|{}", file, turn),
        date,
        local_day: models::local_day(&date),
        model: model.to_string(),
        input: non_cached_input,
        output,
        cache_write_5m: 0, // Codex 는 캐시 쓰기 개념 없음
        cache_write_1h: 0,
        cache_read: cached,
    })
}

fn codex_model(line: &str) -> Option<String> {
    let obj: Value = serde_json::from_str(line).ok()?;
    let payload = obj.get("payload")?;
    if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
        return Some(m.to_string());
    }
    payload
        .get("turn_context")
        .and_then(|tc| tc.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn parse_codex_file(path: &Path) -> Vec<Entry> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let file = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let mut entries = Vec::new();
    let mut turn = 0usize;
    let mut model = String::from("codex");
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        if line.contains("\"model\"") {
            if let Some(m) = codex_model(line) {
                model = m;
            }
        }
        if !line.contains("token_count") {
            continue;
        }
        if let Some(e) = parse_codex_line(line, &file, turn, &model) {
            turn += 1;
            entries.push(e);
        }
    }
    entries
}

pub fn codex_entries(modified_since: Option<SystemTime>, root: Option<&Path>) -> Vec<Entry> {
    let default_dir = codex_sessions_dir();
    let root = match root.map(|r| r.to_path_buf()).or(default_dir) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut entries = Vec::new();
    for file in jsonl_files(&root, modified_since, false) {
        entries.extend(parse_codex_file(&file));
    }
    entries
}

// MARK: Gemini 파싱

/// 토큰 매핑(usageMetadata 의미 보존, total == totalTokenCount):
///   input = (input − cached) + tool / cacheRead = cached
///   output = output + thoughts / cacheWrite = 0
pub fn parse_gemini_file(path: &Path) -> Vec<Entry> {
    let data = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let file = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let mut by_id: HashMap<String, Entry> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut synthetic_counter: usize = 0;

    let mut absorb = |obj: &Value, fallback_ts: Option<DateTime<Utc>>, synthetic: &mut usize| {
        let tokens = match obj.get("tokens") {
            Some(t) if t.is_object() => t,
            _ => return,
        };
        let id = match obj.get("id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                *synthetic += 1;
                format!("__syn{}", synthetic)
            }
        };
        let ts = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(models::parse_iso8601)
            .or(fallback_ts);
        let date = match ts {
            Some(d) => d,
            None => return,
        };
        let input = int_value(tokens.get("input"));
        let cached = int_value(tokens.get("cached"));
        let entry = Entry {
            id: format!("gemini|{}|{}", file, id),
            date,
            local_day: models::local_day(&date),
            model: obj.get("model").and_then(|v| v.as_str()).unwrap_or("gemini").to_string(),
            input: (input - cached).max(0) + int_value(tokens.get("tool")),
            output: int_value(tokens.get("output")) + int_value(tokens.get("thoughts")),
            cache_write_5m: 0, // Gemini 는 캐시 쓰기 미집계
            cache_write_1h: 0,
            cache_read: cached,
        };
        if !by_id.contains_key(&id) {
            order.push(id.clone());
        }
        by_id.insert(id, entry); // update 레코드가 최종값
    };

    if ext == "jsonl" {
        let mut last_ts: Option<DateTime<Utc>> = None;
        for line in data.split('\n') {
            if line.is_empty() {
                continue;
            }
            if !(line.contains("\"tokens\"") || line.contains("\"timestamp\"")) {
                continue;
            }
            let obj: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(ts) = obj.get("timestamp").and_then(|v| v.as_str()).and_then(models::parse_iso8601) {
                last_ts = Some(ts);
            }
            absorb(&obj, last_ts, &mut synthetic_counter);
        }
    } else {
        // 레거시 단일 JSON — messages 배열
        let obj: Value = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let session_start = obj
            .get("startTime")
            .and_then(|v| v.as_str())
            .and_then(models::parse_iso8601);
        if let Some(messages) = obj.get("messages").and_then(|v| v.as_array()) {
            for m in messages {
                absorb(m, session_start, &mut synthetic_counter);
            }
        }
    }

    order.into_iter().filter_map(|id| by_id.remove(&id)).collect()
}

pub fn gemini_entries(modified_since: Option<SystemTime>, root: Option<&Path>) -> Vec<Entry> {
    let default_dir = gemini_tmp_dir();
    let root = match root.map(|r| r.to_path_buf()).or(default_dir) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut entries = Vec::new();
    for file in jsonl_files(&root, modified_since, true) {
        entries.extend(parse_gemini_file(&file));
    }
    entries
}

// MARK: 전 프로바이더 통합

/// 세 프로바이더(Claude/Codex/Gemini) 엔트리를 모두 합친다.
/// 각 프로바이더 dir 이 없으면(미설치) 조용히 건너뜀 — 원본의 프로바이더별 우아한 숨김.
pub fn all_entries(modified_since: Option<SystemTime>) -> Vec<Entry> {
    let mut all = Vec::new();
    all.extend(claude_entries(modified_since, None));
    all.extend(codex_entries(modified_since, None));
    all.extend(gemini_entries(modified_since, None));
    all
}

// MARK: 집계

/// 특정 로컬 날짜의 합계 → DailyUsage. 해당 날짜 데이터 없으면 None.
pub fn daily(entries: &[Entry], local_day: &str) -> Option<DailyUsage> {
    let mut b = Bucket::default();
    for e in entries.iter().filter(|e| e.local_day == local_day) {
        b.add(e);
    }
    if b.total() <= 0 {
        return None;
    }
    Some(DailyUsage {
        date: local_day.to_string(),
        input_tokens: b.input,
        output_tokens: b.output,
        cache_creation_tokens: b.cache_write_5m + b.cache_write_1h,
        cache_read_tokens: b.cache_read,
        total_tokens: b.total(),
        total_cost: b.cost,
    })
}

/// 로컬 날짜 [from, to] (포함) 범위 합계 → PeriodUsage.
pub fn period(entries: &[Entry], period_key: &str, from_day: &str, to_day: &str) -> PeriodUsage {
    let mut b = Bucket::default();
    for e in entries
        .iter()
        .filter(|e| e.local_day.as_str() >= from_day && e.local_day.as_str() <= to_day)
    {
        b.add(e);
    }
    PeriodUsage {
        period: period_key.to_string(),
        total_tokens: b.total(),
        total_cost: b.cost,
    }
}

/// 최근 5시간 롤링 윈도우 기반 활성 블록(번 레이트 추정용).
pub fn active_block(entries: &[Entry], now: DateTime<Utc>) -> Option<BlockUsage> {
    let window_start = now - Duration::hours(5);
    let mut recent: Vec<&Entry> = entries.iter().filter(|e| e.date >= window_start).collect();
    recent.sort_by_key(|e| e.date);
    let first = recent.first()?;
    let mut b = Bucket::default();
    for e in &recent {
        b.add(e);
    }
    let minutes = (now.signed_duration_since(first.date).num_seconds() as f64 / 60.0).max(1.0);
    let tpm = b.total() as f64 / minutes;
    Some(BlockUsage {
        id: format!("block-{}", first.date.timestamp()),
        start_time: first.date.to_rfc3339(),
        end_time: (first.date + Duration::hours(5)).to_rfc3339(),
        is_active: true,
        total_tokens: b.total(),
        cost_usd: b.cost,
        tokens_per_minute: Some(tpm),
    })
}

// MARK: 날짜 유틸 (원본 LocalUsageReader startOfMonth/startOfWeek/monthKey — 로컬 타임존)

/// 로컬 기준 이달 1일 00:00.
pub fn start_of_month(d: DateTime<Local>) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(d.year(), d.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(d)
}

/// 로컬 기준 이번 주 시작(월요일 00:00). 원본은 로케일 첫 요일 의존이나, 집계 윈도우 용도라 월요일 고정.
pub fn start_of_week(d: DateTime<Local>) -> DateTime<Local> {
    let offset = d.weekday().num_days_from_monday() as i64;
    let day = d.date_naive() - Duration::days(offset);
    Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap_or(d)
}

/// 로컬 "yyyy-MM" 월 키.
pub fn month_key_local(d: DateTime<Local>) -> String {
    d.format("%Y-%m").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn temp_dir() -> PathBuf {
        // 스레드 이름 + 나노초로 충돌 없는 고유 디렉토리 (Date/random 미사용 환경 고려해 시스템 시각 사용).
        let base = std::env::temp_dir().join(format!(
            "ptb-local-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn write(lines: &[String], dir: &Path, name: &str, sub: Option<&str>) {
        let folder = match sub {
            Some(s) => dir.join(s),
            None => dir.to_path_buf(),
        };
        fs::create_dir_all(&folder).unwrap();
        let mut f = fs::File::create(folder.join(name)).unwrap();
        f.write_all(lines.join("\n").as_bytes()).unwrap();
    }

    fn claude_line(id: &str, req: &str, model: &str, ts: &str, i: i64, o: i64, cw: i64, cr: i64) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"{req}","timestamp":"{ts}","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{i},"output_tokens":{o},"cache_creation_input_tokens":{cw},"cache_read_input_tokens":{cr}}}}}}}"#
        )
    }

    // 원본 testClaudeDedupKeepsMaxOutput
    #[test]
    fn claude_dedup_keeps_max_output() {
        let dir = temp_dir();
        let ts = "2026-06-30T10:00:00.000Z";
        write(
            &[
                claude_line("A", "R1", "claude-opus-4-8", ts, 100, 5, 0, 1000),
                claude_line("A", "R1", "claude-opus-4-8", ts, 100, 200, 0, 1000),
                claude_line("B", "R2", "claude-sonnet-4-6", ts, 50, 10, 0, 0),
            ],
            &dir,
            "s.jsonl",
            Some("proj/sub"),
        );
        let entries = claude_entries(None, Some(&dir));
        assert_eq!(entries.len(), 2); // A(dedup), B
        let a = entries.iter().find(|e| e.id.starts_with("A|")).unwrap();
        assert_eq!(a.output, 200); // keep-max: 완성된 output
        assert_eq!(a.cache_read, 1000);
    }

    // 원본 testClaudeDailyAndCost
    #[test]
    fn claude_daily_and_cost() {
        let dir = temp_dir();
        let ts = "2026-06-30T10:00:00.000Z";
        let day = models::local_day(&models::parse_iso8601(ts).unwrap());
        write(
            &[claude_line("A", "R1", "claude-opus-4-8", ts, 1_000_000, 0, 0, 0)],
            &dir,
            "s.jsonl",
            Some("p"),
        );
        let entries = claude_entries(None, Some(&dir));
        let d = daily(&entries, &day).unwrap();
        assert_eq!(d.total_tokens, 1_000_000);
        assert!((d.total_cost - 5.0).abs() < 1e-6); // opus input 5/Mtok
        assert!(daily(&entries, "2000-01-01").is_none());
    }

    // 결함 2 — cache_creation 이 있으면 1h/5m 로 갈라진다
    fn claude_line_cc(id: &str, ts: &str, cw_total: i64, w1h: i64, w5m: i64) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"R-{id}","timestamp":"{ts}","message":{{"id":"m-{id}","model":"claude-opus-5","usage":{{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":{cw_total},"cache_read_input_tokens":0,"cache_creation":{{"ephemeral_1h_input_tokens":{w1h},"ephemeral_5m_input_tokens":{w5m}}}}}}}}}"#
        )
    }

    #[test]
    fn claude_cache_creation_splits_1h_5m() {
        let dir = temp_dir();
        let ts = "2026-06-30T10:00:00.000Z";
        write(&[claude_line_cc("A", ts, 12345, 12000, 345)], &dir, "s.jsonl", Some("p"));
        let e = &claude_entries(None, Some(&dir))[0];
        assert_eq!(e.cache_write_1h, 12000);
        assert_eq!(e.cache_write_5m, 345);
        assert_eq!(e.cache_write(), 12345);
    }

    #[test]
    fn claude_old_log_without_cache_creation_all_5m() {
        // cache_creation 객체 없는 옛 로그 → 전부 5분(기존 동작 보존)
        let dir = temp_dir();
        let ts = "2026-06-30T10:00:00.000Z";
        write(&[claude_line("A", "R1", "claude-opus-5", ts, 0, 0, 9999, 0)], &dir, "s.jsonl", Some("p"));
        let e = &claude_entries(None, Some(&dir))[0];
        assert_eq!(e.cache_write_1h, 0);
        assert_eq!(e.cache_write_5m, 9999);
    }

    #[test]
    fn claude_cache_creation_rest_goes_to_5m() {
        // ephemeral 합(1000)이 cache_creation_input_tokens(1500)보다 작으면 잔여 500 은 5분으로
        let dir = temp_dir();
        let ts = "2026-06-30T10:00:00.000Z";
        write(&[claude_line_cc("A", ts, 1500, 700, 300)], &dir, "s.jsonl", Some("p"));
        let e = &claude_entries(None, Some(&dir))[0];
        assert_eq!(e.cache_write_1h, 700);
        assert_eq!(e.cache_write_5m, 800); // 300 + 잔여 500
    }

    // 원본 testCodexParsing
    #[test]
    fn codex_parsing() {
        let dir = temp_dir();
        let line = r#"{"type":"event_msg","timestamp":"2026-06-30T11:00:00.000Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":1050}}}}"#;
        write(&[line.to_string()], &dir, "rollout-x.jsonl", Some("2026/06/30"));
        let entries = codex_entries(None, Some(&dir));
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.input, 800); // 1000 - 200
        assert_eq!(e.cache_read, 200);
        assert_eq!(e.output, 50);
        assert_eq!(e.cache_write(), 0);
    }

    // 원본 testPeriodAndActiveBlock (시각 고정 대신 고정 now 주입)
    #[test]
    fn period_and_active_block() {
        let now = models::parse_iso8601("2026-06-30T12:00:00.000Z").unwrap();
        let recent = now - Duration::minutes(30);
        let old = now - Duration::hours(10);
        let fmt = |d: &DateTime<Utc>| models::local_day(d);
        let entry = |date: DateTime<Utc>, tok: i64| Entry {
            id: format!("{}", date.timestamp_nanos_opt().unwrap_or(0)),
            date,
            local_day: fmt(&date),
            model: "claude-opus-4-8".to_string(),
            input: tok,
            output: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            cache_read: 0,
        };
        let entries = vec![entry(recent, 600_000), entry(old, 999)];
        let block = active_block(&entries, now).unwrap();
        assert_eq!(block.total_tokens, 600_000); // 5h 윈도우 내 항목만
        assert!(block.is_active);
        assert!(block.tokens_per_minute.unwrap() > 0.0);
        let today = fmt(&now);
        let p = period(&entries, "w", &today, &today);
        assert!(p.total_tokens >= 600_000);
    }
}
