//! 파일별 증분 캐시 — `(path, mtime, size)` 가 같으면 재파싱하지 않고, 스냅샷을 디스크에 zlib 로 영속화.
//! 원본 `LocalUsageCache.swift` 이식.
//!
//! 콜드 스타트(전체 파싱 ~수십초)를 최초 1회로 제한(배터리). 변경 파일만 재파싱(정상 상태 ~0.1s).
//! 원본은 actor 지만 Rust 판은 단일 소유 struct — 실앱에서는 Mutex/State 로 감싼다(M2).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::local_usage_reader::{self as reader, Entry};

const PRUNE_AGE_SECS: i64 = 40 * 86400;
const SAVE_THROTTLE_SECS: i64 = 60;

#[derive(Serialize, Deserialize, Clone)]
struct Blob {
    mtime: i64, // unix secs (정수 초 — 서브초 정밀도 불일치로 비교 깨지는 것 방지)
    size: u64,
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Default)]
struct Snapshot {
    #[serde(default)]
    claude: HashMap<String, Blob>,
    #[serde(default)]
    codex: HashMap<String, Blob>,
    #[serde(default)]
    gemini: HashMap<String, Blob>,
}

pub struct LocalUsageCache {
    claude_cache: HashMap<String, Blob>,
    codex_cache: HashMap<String, Blob>,
    gemini_cache: HashMap<String, Blob>,
    loaded: bool,
    dirty: bool,
    last_save: Option<i64>,
    claude_root: Option<PathBuf>,
    codex_root: Option<PathBuf>,
    gemini_root: Option<PathBuf>,
    file_url: PathBuf,
    now: Box<dyn Fn() -> i64>,
}

impl LocalUsageCache {
    /// 테스트 시임 — roots/fileURL/now 주입. 기본값(None/시스템 시각)은 실환경.
    pub fn new(
        claude_root: Option<PathBuf>,
        codex_root: Option<PathBuf>,
        gemini_root: Option<PathBuf>,
        file_url: PathBuf,
        now: Box<dyn Fn() -> i64>,
    ) -> Self {
        LocalUsageCache {
            claude_cache: HashMap::new(),
            codex_cache: HashMap::new(),
            gemini_cache: HashMap::new(),
            loaded: false,
            dirty: false,
            last_save: None,
            claude_root,
            codex_root,
            gemini_root,
            file_url,
            now,
        }
    }

    pub fn claude_entries(&mut self, since_secs: i64) -> Vec<Entry> {
        self.ensure_loaded();
        let root = self
            .claude_root
            .clone()
            .or_else(reader::claude_projects_dir)
            .unwrap_or_default();
        let mut cache = std::mem::take(&mut self.claude_cache);
        let all = self.collect(&root, since_secs, &mut cache, false, reader::parse_claude_file);
        self.claude_cache = cache;
        self.save_if_needed();
        reader::dedup_keep_max(all)
    }

    pub fn codex_entries(&mut self, since_secs: i64) -> Vec<Entry> {
        self.ensure_loaded();
        let root = self
            .codex_root
            .clone()
            .or_else(reader::codex_sessions_dir)
            .unwrap_or_default();
        let mut cache = std::mem::take(&mut self.codex_cache);
        let r = self.collect(&root, since_secs, &mut cache, false, reader::parse_codex_file);
        self.codex_cache = cache;
        self.save_if_needed();
        r
    }

    pub fn gemini_entries(&mut self, since_secs: i64) -> Vec<Entry> {
        self.ensure_loaded();
        let root = self
            .gemini_root
            .clone()
            .or_else(reader::gemini_tmp_dir)
            .unwrap_or_default();
        let mut cache = std::mem::take(&mut self.gemini_cache);
        let r = self.collect(&root, since_secs, &mut cache, true, reader::parse_gemini_file);
        self.gemini_cache = cache;
        self.save_if_needed();
        r
    }

    fn collect(
        &mut self,
        root: &Path,
        since_secs: i64,
        cache: &mut HashMap<String, Blob>,
        allow_json: bool,
        parse: fn(&Path) -> Vec<Entry>,
    ) -> Vec<Entry> {
        let mut result = Vec::new();
        for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !(ext == "jsonl" || (allow_json && ext == "json")) {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if mtime < since_secs {
                continue;
            }
            let size = meta.len();
            let key = path.to_string_lossy().to_string();
            match cache.get(&key) {
                Some(blob) if blob.mtime == mtime && blob.size == size => {
                    result.extend(blob.entries.iter().cloned()); // 변경 없음 → 재파싱 안 함
                }
                _ => {
                    let entries = parse(path);
                    cache.insert(key, Blob { mtime, size, entries: entries.clone() });
                    self.dirty = true;
                    result.extend(entries);
                }
            }
        }
        result
    }

    // MARK: 영속화

    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let raw = match std::fs::read(&self.file_url) {
            Ok(r) => r,
            Err(_) => return,
        };
        // zlib 압축 스냅샷(현행) → 실패 시 평문 JSON(구버전) 폴백
        let data = zlib_decompress(&raw).unwrap_or(raw);
        if let Ok(snap) = serde_json::from_slice::<Snapshot>(&data) {
            self.claude_cache = snap.claude;
            self.codex_cache = snap.codex;
            self.gemini_cache = snap.gemini;
        }
    }

    /// 40일보다 오래된 blob 제거(캐시 무한 증가 방지 + 삭제된 세션 파일 정리).
    fn prune(&mut self) {
        let cutoff = (self.now)() - PRUNE_AGE_SECS;
        self.claude_cache.retain(|_, b| b.mtime >= cutoff);
        self.codex_cache.retain(|_, b| b.mtime >= cutoff);
        self.gemini_cache.retain(|_, b| b.mtime >= cutoff);
    }

    /// 변경이 있으면 저장(최소 60초 간격 throttle).
    fn save_if_needed(&mut self) {
        if !self.dirty {
            return;
        }
        let now = (self.now)();
        if let Some(last) = self.last_save {
            if now - last < SAVE_THROTTLE_SECS {
                return;
            }
        }
        self.prune();
        let snap = Snapshot {
            claude: self.claude_cache.clone(),
            codex: self.codex_cache.clone(),
            gemini: self.gemini_cache.clone(),
        };
        if let Ok(json) = serde_json::to_vec(&snap) {
            // JSON 은 zlib 로 크게 압축됨. 실패 시 평문 저장(로드가 양쪽 처리).
            let out = zlib_compress(&json).unwrap_or(json);
            let tmp = self.file_url.with_extension("json.tmp");
            if std::fs::write(&tmp, &out).is_ok() && std::fs::rename(&tmp, &self.file_url).is_ok() {
                self.dirty = false;
                self.last_save = Some(now);
            }
        }
    }
}

fn zlib_compress(data: &[u8]) -> Option<Vec<u8>> {
    use flate2::{write::ZlibEncoder, Compression};
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).ok()?;
    e.finish().ok()
}

fn zlib_decompress(raw: &[u8]) -> Option<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    let mut d = ZlibDecoder::new(raw);
    let mut out = Vec::new();
    match d.read_to_end(&mut out) {
        Ok(_) => Some(out),
        Err(_) => None, // 평문(비zlib) → 폴백은 호출자가 raw 사용
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: PathBuf,
        cache_file: PathBuf,
        base: PathBuf,
    }

    fn fixture() -> Fixture {
        let uniq = format!(
            "ptb-cache-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        );
        let base = std::env::temp_dir().join(uniq);
        let root = base.join("projects");
        fs::create_dir_all(&root).unwrap();
        Fixture { root, cache_file: base.join("usage-cache.json"), base }
    }

    fn claude_line(id: &str, output: i64) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"r-{id}","timestamp":"2026-07-02T01:00:00.000Z","message":{{"id":"m-{id}","model":"claude-opus-4-8","usage":{{"input_tokens":10,"output_tokens":{output},"cache_creation_input_tokens":5,"cache_read_input_tokens":100}}}}}}"#
        )
    }

    fn write_file(root: &Path, name: &str, lines: &[String], mtime_secs: Option<i64>) {
        let url = root.join(name);
        fs::write(&url, lines.join("\n")).unwrap();
        if let Some(secs) = mtime_secs {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64);
            filetime_set(&url, t);
        }
    }

    // filetime 크레이트 없이 표준 라이브러리만으로 mtime 설정 — File::set_modified (Rust 1.75+).
    fn filetime_set(path: &Path, t: SystemTime) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }

    fn make_cache(fx: &Fixture) -> LocalUsageCache {
        LocalUsageCache::new(
            Some(fx.root.clone()),
            Some(fx.root.clone()),
            None,
            fx.cache_file.clone(),
            Box::new(|| SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64),
        )
    }

    fn now_secs() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
    }

    const SINCE: i64 = 0;

    #[test]
    fn unchanged_file_is_not_reparsed() {
        let fx = fixture();
        let t = now_secs() - 3600;
        write_file(&fx.root, "a.jsonl", &[claude_line("1", 111)], Some(t));
        let mut cache = make_cache(&fx);
        let first = cache.claude_entries(SINCE);
        assert_eq!(first.iter().map(|e| e.output).collect::<Vec<_>>(), vec![111]);

        // 같은 길이(111→222)·같은 mtime → 캐시 히트라면 여전히 111
        write_file(&fx.root, "a.jsonl", &[claude_line("1", 222)], Some(t));
        let second = cache.claude_entries(SINCE);
        assert_eq!(second.iter().map(|e| e.output).collect::<Vec<_>>(), vec![111]);
    }

    #[test]
    fn changed_file_is_reparsed() {
        let fx = fixture();
        let t = now_secs() - 3600;
        write_file(&fx.root, "a.jsonl", &[claude_line("1", 111)], Some(t));
        let mut cache = make_cache(&fx);
        let _ = cache.claude_entries(SINCE);
        write_file(&fx.root, "a.jsonl", &[claude_line("1", 999)], Some(t + 10));
        let second = cache.claude_entries(SINCE);
        assert_eq!(second.iter().map(|e| e.output).collect::<Vec<_>>(), vec![999]);
    }

    #[test]
    fn disk_round_trip_across_instances() {
        let fx = fixture();
        let t = now_secs() - 3600;
        write_file(&fx.root, "a.jsonl", &[claude_line("1", 42)], Some(t));
        let mut c1 = make_cache(&fx);
        let _ = c1.claude_entries(SINCE);
        assert!(fx.cache_file.exists(), "스냅샷이 저장돼야 한다");

        write_file(&fx.root, "a.jsonl", &[claude_line("1", 43)], Some(t));
        let mut c2 = make_cache(&fx);
        let entries = c2.claude_entries(SINCE);
        assert_eq!(entries.iter().map(|e| e.output).collect::<Vec<_>>(), vec![42]);
    }

    #[test]
    fn prune_drops_blobs_older_than_40_days() {
        let fx = fixture();
        let old = now_secs() - 45 * 86400;
        write_file(&fx.root, "old.jsonl", &[claude_line("o", 1)], Some(old));
        write_file(&fx.root, "new.jsonl", &[claude_line("n", 2)], None);
        let mut cache = make_cache(&fx);
        let entries = cache.claude_entries(SINCE);
        let outs: std::collections::HashSet<i64> = entries.iter().map(|e| e.output).collect();
        assert_eq!(outs, [1, 2].into_iter().collect());

        let raw = fs::read(&fx.cache_file).unwrap();
        let plain = zlib_decompress(&raw).unwrap_or(raw);
        let snap = String::from_utf8_lossy(&plain);
        assert!(!snap.contains("old.jsonl"), "45일 지난 blob 은 prune");
        assert!(snap.contains("new.jsonl"));
    }

    #[test]
    fn save_throttle_60s() {
        let fx = fixture();
        let clock = Rc::new(Cell::new(1_700_000_000i64));
        let c = clock.clone();
        let mut cache = LocalUsageCache::new(
            Some(fx.root.clone()),
            Some(fx.root.clone()),
            None,
            fx.cache_file.clone(),
            Box::new(move || c.get()),
        );
        write_file(&fx.root, "a.jsonl", &[claude_line("1", 1)], Some(clock.get()));
        let _ = cache.claude_entries(SINCE); // 첫 저장
        let first_snap = fs::read(&fx.cache_file).unwrap();

        // 30초 뒤 변경 → dirty 지만 throttle 로 저장 생략
        clock.set(clock.get() + 30);
        write_file(&fx.root, "a.jsonl", &[claude_line("1", 2)], Some(clock.get()));
        let _ = cache.claude_entries(SINCE);
        assert_eq!(fs::read(&fx.cache_file).unwrap(), first_snap, "60초 내 재저장 생략");

        // 61초 경과 → 저장됨
        clock.set(clock.get() + 61);
        let _ = cache.claude_entries(SINCE);
        assert_ne!(fs::read(&fx.cache_file).unwrap(), first_snap, "throttle 해제 후 저장");
    }

    #[test]
    fn modified_since_filters() {
        let fx = fixture();
        write_file(&fx.root, "old.jsonl", &[claude_line("o", 1)], Some(now_secs() - 10 * 86400));
        write_file(&fx.root, "new.jsonl", &[claude_line("n", 2)], None);
        let mut cache = make_cache(&fx);
        let entries = cache.claude_entries(now_secs() - 86400);
        assert_eq!(entries.iter().map(|e| e.output).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn legacy_plain_json_cache_loads() {
        let fx = fixture();
        let t = now_secs() - 3600;
        write_file(&fx.root, "a.jsonl", &[claude_line("1", 42)], Some(t));
        {
            let mut c = make_cache(&fx);
            let _ = c.claude_entries(SINCE); // 압축 스냅샷 저장
        }
        // 압축 해제해 평문으로 다운그레이드(구버전 캐시 시뮬레이션)
        let raw = fs::read(&fx.cache_file).unwrap();
        let plain = zlib_decompress(&raw).expect("현행은 zlib 이어야");
        fs::write(&fx.cache_file, &plain).unwrap();

        write_file(&fx.root, "a.jsonl", &[claude_line("1", 43)], Some(t));
        let mut c2 = make_cache(&fx);
        let entries = c2.claude_entries(SINCE);
        assert_eq!(entries.iter().map(|e| e.output).collect::<Vec<_>>(), vec![42], "평문 캐시 로드");
    }

    #[test]
    fn snapshot_is_compressed() {
        let fx = fixture();
        let t = now_secs() - 3600;
        let lines: Vec<String> = (1..=50).map(|i| claude_line(&i.to_string(), i)).collect();
        write_file(&fx.root, "a.jsonl", &lines, Some(t));
        let mut c = make_cache(&fx);
        let _ = c.claude_entries(SINCE);
        let raw = fs::read(&fx.cache_file).unwrap();
        let plain = zlib_decompress(&raw).expect("zlib 이어야 함");
        assert!(raw.len() < plain.len(), "압축본이 평문보다 작아야");
    }

    #[test]
    fn codex_entries_parse_model_and_cached_input() {
        let fx = fixture();
        let lines = [
            r#"{"timestamp":"2026-07-02T01:00:00.000Z","payload":{"type":"turn_context","model":"gpt-5.5-codex"}}"#.to_string(),
            r#"{"timestamp":"2026-07-02T01:01:00.000Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":7}}}}"#.to_string(),
        ];
        write_file(&fx.root, "rollout-1.jsonl", &lines, None);
        let mut cache = make_cache(&fx);
        let entries = cache.codex_entries(SINCE);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model, "gpt-5.5-codex");
        assert_eq!(entries[0].input, 20); // 100 - 80
        assert_eq!(entries[0].cache_read, 80);
        assert_eq!(entries[0].output, 7);
        assert_eq!(entries[0].cache_write(), 0);
    }

    #[test]
    fn date_helpers() {
        use chrono::{Local, TimeZone};
        let d = Local.with_ymd_and_hms(2026, 7, 2, 15, 0, 0).unwrap();
        let som = reader::start_of_month(d);
        assert_eq!(som.day(), 1);
        assert_eq!(som.month(), 7);
        let sow = reader::start_of_week(d);
        assert!(sow <= d);
        assert!((d - sow).num_seconds() < 7 * 86400);
        assert_eq!(reader::month_key_local(d), "2026-07");
    }

    // 이 파일에서 chrono Datelike(day/month) 사용 위해.
    use chrono::Datelike;

    // base 디렉토리 정리(누수 방지) — Drop 으로.
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
}
