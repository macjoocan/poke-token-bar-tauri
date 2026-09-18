//! 게임 상태 스토어 — 원본 `CompanionStore.swift` 이식 (동기 판).
//!
//! 설치 이후 토큰 사용량으로 알 부화 → 진화 → 졸업(도감). 진화 트리/희귀도/이름은 `PokeProvider` 로 주입.
//! 원본은 `@MainActor @Observable` + async(네트워크)지만, 로직 자체는 동기라 Rust 판은 **동기 struct**로
//! 이식하고 프로바이더도 동기 trait 로 둔다(실 네트워크 클라이언트는 blocking 어댑터로 감싼다 — M2b).
//! UI 연출(celebration/notification)은 표시 계층 관심사라 제외, 피드백 seq/값만 보존(테스트 대상).

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};

use super::adventure;
use super::battle;
use super::companion_model::pokemon_balance as pb;
use super::companion_model::{
    fresh_egg, rare_candy, rolls_shiny, AdventureLogEntry, AppLanguage, BurnTier, CompanionState,
    CompanionStateKind, DexEntry, EvoLine, ItemKind, MonState, PokemonNature, Rarity, ShopEntry,
};

// ─────────────────────────────────────────────────────────────────────────
// 프로바이더 / RNG
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct BaseSpecies {
    pub id: i64,
    pub capture_rate: i64,
}

/// `battle_attack` 한 턴의 결과 묶음(커맨드 계층 전달용).
#[derive(Debug, Clone)]
pub struct BattleTurnResult {
    pub outcome: battle::TurnOutcome,
    /// 턴 반영 후 상태(종료된 전투도 최종 HP 를 담음).
    pub state: battle::BattleState,
    /// 승리 보상 사탕 수(승리 시만 > 0).
    pub reward_qty: i64,
    /// 승리 시 몬스터볼 포획 결과(승리가 아니면 None).
    pub caught: Option<bool>,
    /// 포획에 사용한 볼(raw_value). None = 기본 몬스터볼(무한) 또는 승리 아님.
    pub ball_used: Option<&'static str>,
    /// 포획 성공 시 그 종의 희귀도(lowercase — 전설 특별 연출용).
    pub caught_rarity: Option<String>,
}

/// 포켓몬 라인 데이터 제공(주입 — 테스트는 스텁, 실앱은 PokéAPI blocking 어댑터).
/// Send+Sync — Tauri managed state(Mutex<CompanionStore>)로 관리하기 위함.
pub trait PokeProvider: Send + Sync {
    fn line(&self, base_species_id: i64) -> anyhow::Result<EvoLine>;
    fn base_species_index(&self) -> anyhow::Result<Vec<BaseSpecies>>;
    /// 단일 종이 base 면 Some, 아니면 None. 기본 구현은 인덱스에서 파생.
    fn base_species(&self, id: i64) -> anyhow::Result<Option<BaseSpecies>> {
        Ok(self.base_species_index()?.into_iter().find(|b| b.id == id))
    }
}

pub trait Rng: Send {
    fn next_u64(&mut self) -> u64;
}

/// SplitMix64 — 원본 테스트 SeededRNG 와 동일 알고리즘(결정적 재현).
pub struct SeededRng {
    state: u64,
}
impl SeededRng {
    pub fn new(seed: u64) -> Self {
        SeededRng { state: seed }
    }
}
impl Rng for SeededRng {
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 스토어
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandyUseResult {
    Evolved,
    Graduated,
    Progressed,
    Unavailable,
}

pub struct CompanionStore {
    pub state: CompanionState,
    pub current_line: Option<EvoLine>,
    pub display_state: CompanionStateKind,
    pub just_evolved_to: Option<String>,
    pub just_graduated: Option<String>,
    event_until: Option<DateTime<Utc>>,
    pub mint_feedback_nature: Option<PokemonNature>,
    pub mint_feedback_seq: u64,
    pub candy_feedback_amount: i64,
    pub candy_feedback_seq: u64,
    provider: Box<dyn PokeProvider>,
    clock: Box<dyn Fn() -> DateTime<Utc> + Send>,
    rng: Box<dyn Rng>,
    file_url: PathBuf,
    /// .app 번들 여부 — 메타몽 위장 롤을 이 게이트로 막는다(테스트=false → 롤 rng 미소비, 원본 && 단락과 동일).
    is_bundled_app: bool,
}

impl CompanionStore {
    pub fn new(
        provider: Box<dyn PokeProvider>,
        clock: Box<dyn Fn() -> DateTime<Utc> + Send>,
        file_url: PathBuf,
        rng: Box<dyn Rng>,
        is_bundled_app: bool,
    ) -> Self {
        let mut s = CompanionStore {
            state: CompanionState::default(),
            current_line: None,
            display_state: CompanionStateKind::Egg,
            just_evolved_to: None,
            just_graduated: None,
            event_until: None,
            mint_feedback_nature: None,
            mint_feedback_seq: 0,
            candy_feedback_amount: 0,
            candy_feedback_seq: 0,
            provider,
            clock,
            rng,
            file_url,
            is_bundled_app,
        };
        s.load();
        // 언어를 직접 고른 적 없으면 OS 로케일로 재유도(과거 en 하드코딩 기본값 자동 교정).
        if !s.state.language_set_by_user {
            s.state.language = AppLanguage::system_default();
        }
        if s.state.active.is_some() {
            s.display_state = CompanionStateKind::Idle;
        }
        s
    }

    // MARK: 영속
    fn load(&mut self) {
        let data = match std::fs::read(&self.file_url) {
            Ok(d) => d,
            Err(_) => return, // 파일 없음(첫 실행) → 기본값
        };
        match serde_json::from_slice::<CompanionState>(&data) {
            Ok(st) => self.state = st,
            Err(e) => {
                // 파싱 실패(스키마 불일치·손상) → 빈 상태로 시작하면 다음 save 가 파일을 덮어써
                // 데이터가 영구 손실된다. 원본을 백업으로 보존해 복구 가능하게 한다.
                let bak = self.file_url.with_extension("corrupt.json");
                let _ = std::fs::write(&bak, &data);
                eprintln!("companion-state load failed ({e}); original backed up to {bak:?}");
            }
        }
    }
    fn save(&self) {
        if let Ok(data) = serde_json::to_vec(&self.state) {
            let tmp = self.file_url.with_extension("json.tmp");
            if std::fs::write(&tmp, &data).is_ok() {
                let _ = std::fs::rename(&tmp, &self.file_url);
            }
        }
    }

    /// 현재 상태를 지정 파일로 내보내기(백업). 사람이 읽기 쉽게 pretty JSON.
    pub fn export_to(&self, dest: &std::path::Path) -> bool {
        match serde_json::to_vec_pretty(&self.state) {
            Ok(data) => {
                if let Some(dir) = dest.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                std::fs::write(dest, data).is_ok()
            }
            Err(_) => false,
        }
    }

    /// 백업 파일에서 상태 복원. 파싱 성공 시 교체·저장하고 라인 캐시를 비워 다음 틱에 재로드.
    /// 파일 없음/파싱 실패 시 현재 상태 보존(false 반환).
    pub fn import_from(&mut self, src: &std::path::Path) -> bool {
        let data = match std::fs::read(src) {
            Ok(d) => d,
            Err(_) => return false,
        };
        match serde_json::from_slice::<CompanionState>(&data) {
            Ok(st) => {
                self.state = st;
                self.current_line = None; // 새 상태의 라인 재로드 유도
                self.just_evolved_to = None;
                self.just_graduated = None;
                self.save();
                true
            }
            Err(_) => false,
        }
    }

    // MARK: 파생값 (UI)
    pub fn language(&self) -> AppLanguage {
        self.state.language
    }
    pub fn set_language(&mut self, lang: AppLanguage) {
        self.state.language = lang;
        self.state.language_set_by_user = true; // 이후 로드에서 OS 로케일로 덮지 않음(사용자 선택 존중)
        self.save();
    }
    pub fn has_active(&self) -> bool {
        self.state.active.is_some()
    }
    pub fn is_egg(&self) -> bool {
        self.state.active.is_none()
    }
    pub fn egg_started(&self) -> bool {
        self.state.egg_usage > 0
    }
    pub fn egg_progress(&self) -> f64 {
        (self.state.egg_usage as f64 / pb::EGG_HATCH_THRESHOLD as f64).clamp(0.0, 1.0)
    }
    pub fn egg_tokens_to_hatch(&self) -> i64 {
        (pb::EGG_HATCH_THRESHOLD - self.state.egg_usage).max(0)
    }
    pub fn item_count(&self, kind: ItemKind) -> i64 {
        self.state.inventory.get(kind.raw_value()).copied().unwrap_or(0)
    }
    /// 소모품 1개 사용(장난감 등) — 보유 시 차감·저장 후 true, 없으면 false.
    pub fn consume_item(&mut self, kind: ItemKind) -> bool {
        let c = self.item_count(kind);
        if c <= 0 {
            return false;
        }
        self.state.inventory.insert(kind.raw_value().to_string(), c - 1);
        self.save();
        true
    }
    pub fn rare_candy_count(&self) -> i64 {
        self.item_count(ItemKind::RareCandy)
    }
    pub fn owns_shiny_charm(&self) -> bool {
        self.item_count(ItemKind::ShinyCharm) > 0
    }
    pub fn current_species_id(&self) -> Option<i64> {
        self.state.active.as_ref().map(|a| a.current_id())
    }
    pub fn current_is_shiny(&self) -> bool {
        match &self.state.active {
            Some(a) => {
                if a.ditto_disguise.is_some() && !a.ditto_revealed {
                    return false; // 위장 중엔 이로치 숨김
                }
                a.is_shiny
            }
            None => false,
        }
    }
    pub fn current_nature(&self) -> Option<PokemonNature> {
        self.state.active.as_ref().and_then(|a| a.nature)
    }
    pub fn threshold(&self) -> i64 {
        match &self.state.active {
            Some(a) => pb::phase_threshold(a.rarity, a.total_forms, a.stage_index),
            None => 1,
        }
    }
    pub fn progress(&self) -> f64 {
        match &self.state.active {
            Some(a) if self.threshold() > 0 => {
                (a.used_at_stage as f64 / self.threshold() as f64).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
    pub fn tokens_to_next(&self) -> i64 {
        match &self.state.active {
            Some(a) => (self.threshold() - a.used_at_stage).max(0),
            None => 0,
        }
    }
    pub fn display_name(&self) -> String {
        match (&self.state.active, &self.current_line) {
            (Some(a), Some(line)) => line.localized_name(a.current_id(), self.state.language),
            _ => "Token Egg".to_string(),
        }
    }
    pub fn owned_items(&self) -> Vec<(ItemKind, i64)> {
        ItemKind::ALL
            .iter()
            .filter_map(|&k| {
                let c = self.item_count(k);
                if c > 0 {
                    Some((k, c))
                } else {
                    None
                }
            })
            .collect()
    }
    /// 저장된 체인 종별 이름을 현재 언어로 해석 — 없으면 None(뷰가 async 조회로 폴백).
    pub fn dex_stored_chain_names(&self, entry: &DexEntry) -> Option<std::collections::HashMap<i64, String>> {
        let names = entry.names.as_ref()?;
        if names.is_empty() {
            return None;
        }
        Some(
            names
                .iter()
                .filter_map(|(id, by_lang)| self.state.language.resolve_name(by_lang).map(|n| (*id, n)))
                .collect(),
        )
    }

    // MARK: 갱신 (AppDelegate 가 UsageStore 값으로 호출)
    pub fn update(
        &mut self,
        today_tokens: i64,
        today_date: &str,
        _month_total: i64,
        burn_tier: BurnTier,
        limit_warning: bool,
        has_usage_data: bool,
    ) {
        if !self.state.install_baseline_set {
            if !has_usage_data {
                self.display_state = CompanionStateKind::Egg;
                return;
            }
            self.state.install_baseline_set = true;
            self.state.claimed_today_tokens = today_tokens;
            self.state.last_date = today_date.to_string();
            self.save();
        } else {
            if today_date != self.state.last_date {
                self.state.last_date = today_date.to_string();
                self.state.claimed_today_tokens = 0;
            }
            if today_tokens > self.state.claimed_today_tokens {
                let delta = today_tokens - self.state.claimed_today_tokens;
                self.state.claimed_today_tokens = today_tokens;
                self.state.used_since_install += delta;
                if self.state.active.is_none() {
                    self.state.egg_usage += delta;
                } else {
                    self.apply_usage(delta);
                }
            }
        }
        // 이벤트 창 만료 — justEvolvedTo 는 여기(창 만료)에서만 정리(#4 회귀 방지).
        if let Some(until) = self.event_until {
            if (self.clock)() > until {
                self.just_graduated = None;
                self.just_evolved_to = None;
                self.event_until = None;
            }
        }
        self.display_state = self.compute_state(burn_tier, limit_warning, has_usage_data, today_tokens);
        self.save();
    }

    /// 토큰 증분을 현재 포켓몬에 적용 — 임계 도달 시 진화/졸업.
    pub fn apply_usage(&mut self, delta: i64) {
        if self.state.active.is_none() {
            return;
        }
        self.state.active.as_mut().unwrap().used_at_stage += delta;
        if self.current_line.is_none() {
            self.save();
            return;
        }
        let mut guard = 0;
        while self.state.active.is_some() && guard < 50 {
            guard += 1;
            let a = self.state.active.as_ref().unwrap();
            let (rarity, forms, stage, used, current, base) = (
                a.rarity,
                a.total_forms,
                a.stage_index,
                a.used_at_stage,
                a.current_id(),
                a.base_id,
            );
            let ditto_unrevealed = a.ditto_disguise.is_some() && !a.ditto_revealed;
            let thr = pb::phase_threshold(rarity, forms, stage);
            if used < thr {
                break;
            }
            // 라인에서 현재 노드의 자식 정보(species_id + finalIDs) 추출 후 라인 borrow 해제.
            let children_info: Vec<(i64, Vec<i64>)> = {
                let line = self.current_line.as_ref().unwrap();
                match line.tree.node(current) {
                    Some(n) => n.children.iter().map(|c| (c.species_id, c.final_ids())).collect(),
                    None => break,
                }
            };
            if children_info.is_empty() {
                self.graduate();
                break;
            }
            // 메타몽 위장: 진화 못 하는 메타몽 → 첫 진화 임계에서 진화 대신 정체 공개(원본 revealDitto).
            // 라인 fetch 실패(오프라인)면 break — 임계 이상 유지된 채 다음 틱 재시도.
            if ditto_unrevealed {
                if self.reveal_ditto() {
                    continue; // 메타몽으로 전환됨 — 새 임계(rare·단일 형태)로 재평가
                }
                break;
            }
            let next_id = self.pick_next_child(&children_info, base);
            {
                let a = self.state.active.as_mut().unwrap();
                let keep = ((stage as usize) + 1).min(a.path_ids.len());
                a.path_ids.truncate(keep);
                a.path_ids.push(next_id);
                a.stage_index += 1;
                a.used_at_stage = used - thr; // 초과분 이월
            }
            let new_name = self
                .current_line
                .as_ref()
                .unwrap()
                .localized_name(next_id, self.state.language);
            self.just_evolved_to = Some(new_name);
            self.event_until = Some((self.clock)() + Duration::seconds(4));
            self.register_species_in_dex(); // 진화한 새 형태도 도감에 수집
        }
        self.save();
    }

    /// 위장 → 리빌(원본 `revealDitto` 이식): 진화 못 하는 메타몽이 첫 진화 임계에서 진화 대신
    /// 정체를 드러낸다. 메타몽 라인 로드 후 상태 전환 — rarity/forms 는 메타몽 라인에서,
    /// is_shiny/nature/위장 마커는 유지, 첫 임계 초과분은 메타몽 성장으로 이월.
    /// 라인 fetch 실패(오프라인) 시 false — 호출부(apply_usage)가 다음 틱 재시도.
    fn reveal_ditto(&mut self) -> bool {
        let (rarity, forms, used, disguise_base) = match &self.state.active {
            Some(a) if a.ditto_disguise.is_some() && !a.ditto_revealed => {
                (a.rarity, a.total_forms, a.used_at_stage, a.base_id)
            }
            _ => return false,
        };
        let first_thr = pb::phase_threshold(rarity, forms, 0);
        if used < first_thr {
            return false; // 임계 미달 방어
        }
        let ditto_line = match self
            .provider
            .line(super::companion_model::pokemon_odds::DITTO_SPECIES_ID)
        {
            Ok(l) => l,
            Err(_) => return false,
        };
        let disguise_name = self
            .current_line
            .as_ref()
            .map(|l| l.localized_name(disguise_base, self.state.language))
            .unwrap_or_else(|| format!("#{disguise_base}"));
        let carry = (used - first_thr).max(0);
        let (d_base, d_rarity, d_forms) =
            (ditto_line.base_id, ditto_line.rarity, ditto_line.total_forms() as i64);
        {
            let a = self.state.active.as_mut().unwrap();
            a.base_id = d_base;
            a.path_ids = vec![d_base];
            a.stage_index = 0;
            a.rarity = d_rarity;
            a.total_forms = d_forms;
            a.used_at_stage = carry;
            a.ditto_revealed = true;
        }
        self.current_line = Some(ditto_line);
        let ditto_name = self.display_name();
        // 리빌 연출 — levelUp 창 + 모험 로그(사용자에게 "왜 안 진화했는지"가 남게).
        self.display_state = CompanionStateKind::LevelUp;
        self.event_until = Some((self.clock)() + Duration::seconds(5));
        self.push_adventure_log(
            "ditto_reveal",
            "🟣",
            format!("이럴 수가! {disguise_name}의 정체는 {ditto_name}이었다!"),
            None,
            0,
        );
        self.register_species_in_dex(); // 정체(메타몽)도 수집 형태로 등록(이 포트의 도감 정책)
        self.save();
        true
    }

    fn pick_next_child(&mut self, children: &[(i64, Vec<i64>)], base_id: i64) -> i64 {
        let fresh: Vec<&(i64, Vec<i64>)> = children
            .iter()
            .filter(|(_, finals)| {
                finals
                    .iter()
                    .any(|f| !self.state.collected_finals.contains(&format!("{}:{}", base_id, f)))
            })
            .collect();
        let pool: Vec<&(i64, Vec<i64>)> = if fresh.is_empty() {
            children.iter().collect()
        } else {
            fresh
        };
        let idx = (self.rng.next_u64() % pool.len() as u64) as usize;
        pool[idx].0
    }

    fn graduate(&mut self) {
        let a = match &self.state.active {
            Some(a) => a.clone(),
            None => return,
        };
        let final_id = a.current_id();
        self.state.collected_finals.insert(format!("{}:{}", a.base_id, final_id));
        let name = self
            .current_line
            .as_ref()
            .map(|line| line.localized_name(final_id, self.state.language))
            .unwrap_or_default();
        // 최종체를 도감에 등록(진화 시점에 이미 등록됐으면 dedupe 로 스킵).
        self.register_species_in_dex();
        self.just_graduated = Some(name);
        self.event_until = Some((self.clock)() + Duration::seconds(6));
        self.state.active = None;
        self.current_line = None;
        self.state.egg_usage = 0;
        self.state.pending_battle = None; // 파트너가 떠나면 진행 중 전투 무효
    }

    /// 현재 컴패니언(활성 종)을 도감에 등록 — 종 id 기준 중복 방지, 같은 종을 이로치로 다시
    /// 얻으면 승격. 부화·진화·졸업 시마다 호출해 **라인의 각 형태**를 수집한다.
    /// (`final_id` 필드는 "이 도감 항목이 대표하는 종 id" 의미로 재사용 — 부화형은 기본종, 진화형은
    /// 그 단계 종, 최종체는 최종종. `chain_order`/`names` 는 그 시점까지의 경로.)
    fn register_species_in_dex(&mut self) {
        let (base_id, path_ids, sid, rarity, is_shiny, nature) = match &self.state.active {
            Some(a) => (a.base_id, a.path_ids.clone(), a.current_id(), a.rarity, a.is_shiny, a.nature),
            None => return,
        };
        if let Some(e) = self.state.dex.iter_mut().find(|d| d.final_id == sid) {
            if is_shiny && !e.is_shiny {
                e.is_shiny = true; // 이로치 승격
            }
            return;
        }
        let names = self.current_line.as_ref().map(|line| {
            path_ids
                .iter()
                .filter_map(|id| line.names.get(id).map(|m| (*id, m.clone())))
                .collect::<std::collections::HashMap<_, _>>()
        });
        self.state.dex.push(DexEntry {
            id: String::new(),
            base_id,
            final_id: sid,
            chain_order: path_ids,
            rarity,
            caught_at: Some((self.clock)()),
            is_shiny,
            nature,
            names,
        });
    }

    // MARK: 이상한 사탕
    pub fn can_use_rare_candy(&self) -> bool {
        self.has_active() && self.current_line.is_some() && self.rare_candy_count() > 0
    }
    pub fn use_rare_candy(&mut self) -> CandyUseResult {
        if !self.can_use_rare_candy() {
            return CandyUseResult::Unavailable;
        }
        let c = self.rare_candy_count() - 1;
        self.state.inventory.insert(ItemKind::RareCandy.raw_value().to_string(), c);
        let before_stage = self.state.active.as_ref().map(|a| a.stage_index).unwrap_or(0);
        self.candy_feedback_amount = rare_candy::XP;
        self.candy_feedback_seq += 1;
        self.apply_usage(rare_candy::XP);
        if self.state.active.is_none() {
            return CandyUseResult::Graduated;
        }
        if self.state.active.as_ref().unwrap().stage_index > before_stage {
            CandyUseResult::Evolved
        } else {
            CandyUseResult::Progressed
        }
    }

    // MARK: 민트
    pub fn can_use_mint(&self) -> bool {
        self.has_active() && self.item_count(ItemKind::Mint) > 0
    }
    pub fn use_mint(&mut self) -> Option<PokemonNature> {
        if !self.can_use_mint() || self.state.active.is_none() {
            return None;
        }
        let cur = self.state.active.as_ref().unwrap().nature;
        let pool: Vec<PokemonNature> = PokemonNature::ALL.iter().copied().filter(|&n| Some(n) != cur).collect();
        let new = pool[(self.rng.next_u64() % pool.len() as u64) as usize];
        self.state.active.as_mut().unwrap().nature = Some(new);
        let c = self.item_count(ItemKind::Mint) - 1;
        self.state.inventory.insert(ItemKind::Mint.raw_value().to_string(), c);
        self.mint_feedback_nature = Some(new);
        self.mint_feedback_seq += 1;
        self.save();
        Some(new)
    }
    pub fn consume_mint_feedback(&mut self) {
        self.mint_feedback_nature = None;
    }
    pub fn consume_candy_feedback(&mut self) {
        self.candy_feedback_amount = 0;
    }

    // MARK: 상점
    pub fn available_tokens(&self) -> i64 {
        (self.state.used_since_install - self.state.spent_tokens).max(0)
    }
    pub fn purchasable_items(&self) -> Vec<ItemKind> {
        let mut items: Vec<ItemKind> = ItemKind::ALL.iter().copied().filter(|k| k.shop_price().is_some()).collect();
        items.sort_by(|&a, &b| {
            let a_done = a.is_passive() && self.item_count(a) > 0;
            let b_done = b.is_passive() && self.item_count(b) > 0;
            if a_done != b_done {
                // 구매 완료 보유형은 맨 아래
                return a_done.cmp(&b_done);
            }
            a.shop_price().unwrap_or(0).cmp(&b.shop_price().unwrap_or(0))
        });
        items
    }
    pub fn shop_entries(&self) -> Vec<ShopEntry> {
        let mut entries: Vec<ShopEntry> = self.purchasable_items().into_iter().map(ShopEntry::Item).collect();
        if self.has_active() {
            entries.push(ShopEntry::FreshEgg);
        }
        entries.sort_by(|&a, &b| {
            let a_done = self.is_purchased_passive(a);
            let b_done = self.is_purchased_passive(b);
            if a_done != b_done {
                return a_done.cmp(&b_done);
            }
            a.price().cmp(&b.price())
        });
        entries
    }
    fn is_purchased_passive(&self, entry: ShopEntry) -> bool {
        match entry {
            ShopEntry::Item(kind) => kind.is_passive() && self.item_count(kind) > 0,
            ShopEntry::FreshEgg => false,
        }
    }
    pub fn can_buy(&self, kind: ItemKind) -> bool {
        let price = match kind.shop_price() {
            Some(p) => p,
            None => return false,
        };
        if kind.is_passive() && self.item_count(kind) > 0 {
            return false;
        }
        self.available_tokens() >= price
    }
    pub fn buy(&mut self, kind: ItemKind) -> bool {
        let price = match kind.shop_price() {
            Some(p) => p,
            None => return false,
        };
        if self.available_tokens() < price {
            return false;
        }
        if kind.is_passive() && self.item_count(kind) > 0 {
            return false;
        }
        self.state.spent_tokens += price;
        *self.state.inventory.entry(kind.raw_value().to_string()).or_insert(0) += 1;
        self.save();
        true
    }
    pub fn can_buy_rare_candy(&self) -> bool {
        self.can_buy(ItemKind::RareCandy)
    }
    pub fn buy_rare_candy(&mut self) -> bool {
        self.buy(ItemKind::RareCandy)
    }

    // MARK: 새 알 (리롤)
    pub fn can_buy_fresh_egg(&self) -> bool {
        self.has_active() && self.available_tokens() >= fresh_egg::PRICE
    }
    pub fn buy_fresh_egg(&mut self) -> bool {
        if !self.can_buy_fresh_egg() {
            return false;
        }
        self.state.spent_tokens += fresh_egg::PRICE;
        self.state.active = None; // 폐기(졸업 아님 — dex/collectedFinals 미변경)
        self.current_line = None;
        self.state.egg_usage = 0;
        self.state.pending_hatch_id = None;
        self.state.pending_battle = None; // 파트너가 떠나면 진행 중 전투 무효
        self.just_graduated = None;
        self.just_evolved_to = None;
        self.event_until = None;
        self.save();
        true
    }

    // MARK: 부화
    pub fn hatch_if_needed(&mut self) {
        if self.state.active.is_some() || self.state.egg_usage < pb::EGG_HATCH_THRESHOLD {
            return;
        }
        let base = match self.state.pending_hatch_id {
            Some(p) => Some(p),
            None => self.choose_base(),
        };
        if let Some(base) = base {
            self.state.pending_hatch_id = None;
            self.hatch_core(base);
        }
    }
    pub fn hatch(&mut self, base_id: i64) {
        self.hatch_core(base_id);
    }

    /// 재시작 등으로 active 는 있으나 라인이 안 실린 경우 로드 후 밀린 진화 판정(apply_usage(0)).
    /// 원본 loadCurrentLine 상당. 네트워크 실패 시 조용히 무시(다음 틱 재시도).
    pub fn ensure_line_loaded(&mut self) {
        let base = match &self.state.active {
            Some(a) if self.current_line.is_none() => a.base_id,
            _ => return,
        };
        if let Ok(line) = self.provider.line(base) {
            self.current_line = Some(line);
            self.apply_usage(0);
            // 이미 부화해 있던 컴패니언도(이 기능 이전에 부화) 로드 시 도감에 등록.
            self.register_species_in_dex();
        }
    }

    // MARK: 모험(CRPG) 이벤트
    /// 토큰 사용이 `ADVENTURE_INTERVAL` 만큼 늘었으면 이벤트 1회 발생(펫 활동 중일 때만).
    /// 발생 시 보상 아이템을 인벤토리에 넣고 로그에 적재. 방금 발생한 이벤트 반환(없으면 None).
    pub fn maybe_trigger_adventure(&mut self) -> Option<adventure::AdventureEvent> {
        if !self.has_active() {
            return None; // 알 상태에선 모험 없음(부화 후부터)
        }
        let used = self.state.used_since_install;
        if self.state.last_event_tokens == 0 {
            // 첫 기준선 — 설치 직후 누적분으로 이벤트가 몰려 터지는 것 방지.
            self.state.last_event_tokens = used;
            self.save();
            return None;
        }
        if used - self.state.last_event_tokens < adventure::ADVENTURE_INTERVAL {
            return None;
        }
        self.state.last_event_tokens = used;
        let name = self.display_name();
        // 전투 조우 — 일부 이벤트를 야생 전투로 대체(진행 중 전투가 있으면 일반 이벤트).
        if self.state.pending_battle.is_none()
            && self.rng.next_u64() % 100 < battle::tuning::ENCOUNTER_PERCENT
        {
            let lvl = battle::ally_level(
                self.state.active.as_ref().map(|a| a.stage_index).unwrap_or(0),
                self.state.dex.len(),
            );
            let b = battle::new_battle(self.rng.as_mut(), lvl);
            self.state.pending_battle = Some(b);
            let ev = adventure::AdventureEvent {
                id: "battle",
                emoji: "⚔️",
                message: format!("야생 포켓몬이 나타났다! {name}이(가) 맞선다!"),
                reward: None,
                reward_qty: 0,
            };
            self.push_adventure_log("battle", "⚔️", ev.message.clone(), None, 0);
            self.save();
            return Some(ev);
        }
        let ev = adventure::roll_event(self.rng.as_mut(), &name);
        let qty = ev.reward_qty.max(1);
        if let Some(item) = ev.reward {
            *self.state.inventory.entry(item.raw_value().to_string()).or_insert(0) += qty;
        }
        self.push_adventure_log(ev.id, ev.emoji, ev.message.clone(), ev.reward, qty);
        self.save();
        Some(ev)
    }

    /// 모험 로그 한 줄 적재(seq 증가·cap 유지). save 는 호출부 책임.
    fn push_adventure_log(
        &mut self,
        id: &str,
        emoji: &str,
        message: String,
        reward: Option<ItemKind>,
        reward_qty: i64,
    ) {
        self.state.event_seq += 1;
        self.state.adventure_log.push(AdventureLogEntry {
            seq: self.state.event_seq,
            id: id.to_string(),
            emoji: emoji.to_string(),
            message,
            reward: reward.map(|i| i.raw_value().to_string()),
            reward_qty: reward_qty.max(1),
        });
        let cap = adventure::ADVENTURE_LOG_CAP;
        if self.state.adventure_log.len() > cap {
            let excess = self.state.adventure_log.len() - cap;
            self.state.adventure_log.drain(0..excess);
        }
    }

    // MARK: 야생 전투 (모험 조우)

    /// 진행 중 전투(있으면). 파트너가 없으면 None(전투 무효 — UI 미노출).
    pub fn battle(&self) -> Option<&battle::BattleState> {
        if !self.has_active() {
            return None;
        }
        self.state.pending_battle.as_ref()
    }

    /// 전투: 싸우다 — 한 턴 진행(선공 + 반격). 종료 시 보상 지급·로그·pending 해제.
    /// `wild_name` 은 프런트가 해석한 표시명(로그 문구용, 백엔드엔 이름 DB 없음).
    pub fn battle_attack(&mut self, wild_name: &str) -> Option<BattleTurnResult> {
        if !self.has_active() {
            return None;
        }
        let mut b = self.state.pending_battle.clone()?;
        let outcome = battle::player_attack(self.rng.as_mut(), &mut b);
        let mut reward = 0;
        let mut caught = None;
        let mut ball_used = None;
        let mut caught_rarity = None;
        if outcome.wild_fainted {
            reward = battle::reward_qty(&b);
            *self
                .state
                .inventory
                .entry(ItemKind::RareCandy.raw_value().to_string())
                .or_insert(0) += reward;
            let shiny_mark = if b.wild_shiny { "✨" } else { "" };
            self.push_adventure_log(
                "battle_win",
                "🏆",
                format!("야생 {wild_name}{shiny_mark}을(를) 쓰러뜨렸다!"),
                Some(ItemKind::RareCandy),
                reward,
            );
            // 승리 후 포획 시도 — 좋은 볼부터 자동 사용(하이퍼 > 수퍼 > 기본 몬스터볼(무한)).
            // 소모형이라 실패해도 볼은 소비된다.
            let (ball_kind, ball_bonus) = if self.item_count(ItemKind::UltraBall) > 0 {
                (Some(ItemKind::UltraBall), battle::tuning::CATCH_ULTRA_BONUS)
            } else if self.item_count(ItemKind::GreatBall) > 0 {
                (Some(ItemKind::GreatBall), battle::tuning::CATCH_GREAT_BONUS)
            } else {
                (None, 0)
            };
            let ball_name = match ball_kind {
                Some(ItemKind::UltraBall) => "하이퍼볼",
                Some(ItemKind::GreatBall) => "수퍼볼",
                _ => "몬스터볼",
            };
            if let Some(k) = ball_kind {
                let c = self.item_count(k) - 1;
                self.state.inventory.insert(k.raw_value().to_string(), c);
                ball_used = Some(k.raw_value());
            }
            let hit = battle::roll_catch(self.rng.as_mut(), &b, ball_bonus);
            caught = Some(hit);
            if hit {
                let rarity = self.register_wild_in_dex(&b);
                self.state.caught_count += 1;
                caught_rarity = Some(format!("{:?}", rarity).to_lowercase());
                let (id, emoji, prefix) = if rarity == Rarity::Legendary {
                    ("battle_catch_legend", "🌟", "전설의 ")
                } else {
                    ("battle_catch", "⚪", "야생 ")
                };
                self.push_adventure_log(
                    id,
                    emoji,
                    format!("{ball_name}을 던져 {prefix}{wild_name}{shiny_mark}을(를) 잡았다! 도감에 등록!"),
                    None,
                    0,
                );
            }
            self.state.pending_battle = None;
        } else if outcome.ally_fainted {
            let name = self.display_name();
            self.push_adventure_log(
                "battle_lose",
                "💫",
                format!("{name}이(가) 쓰러졌다… 다음에 설욕하자!"),
                None,
                0,
            );
            self.state.pending_battle = None;
        } else {
            self.state.pending_battle = Some(b.clone());
        }
        self.save();
        Some(BattleTurnResult {
            outcome,
            state: b,
            reward_qty: reward,
            caught,
            ball_used,
            caught_rarity,
        })
    }

    /// 포획한 야생 종을 도감에 등록하고 그 종의 희귀도를 반환. 라인 fetch(네트워크·30일 캐시)
    /// 성공 시 진화 체인·이름 포함, 실패(오프라인) 시 단일 종 최소 항목(이름은 프런트 #id 폴백).
    /// 종 id dedupe + 이로치 승격은 `register_species_in_dex` 와 동일 규칙.
    fn register_wild_in_dex(&mut self, b: &battle::BattleState) -> Rarity {
        let sid = b.wild_id;
        if let Some(e) = self.state.dex.iter_mut().find(|d| d.final_id == sid) {
            if b.wild_shiny && !e.is_shiny {
                e.is_shiny = true; // 이로치 승격
            }
            return e.rarity;
        }
        let line = self.provider.line(sid).ok();
        let (base_id, chain_order, rarity, names) = match &line {
            Some(l) => {
                // 트리 루트가 실제 기본종(요청 id 가 중간 진화체일 수 있음).
                let path = l.tree.path_to(sid).unwrap_or_else(|| vec![sid]);
                let names = path
                    .iter()
                    .filter_map(|id| l.names.get(id).map(|m| (*id, m.clone())))
                    .collect::<std::collections::HashMap<_, _>>();
                (l.tree.species_id, path, l.rarity, Some(names))
            }
            None => (sid, vec![sid], Rarity::Common, None),
        };
        self.state.dex.push(DexEntry {
            id: String::new(),
            base_id,
            final_id: sid,
            chain_order,
            rarity,
            caught_at: Some((self.clock)()),
            is_shiny: b.wild_shiny,
            nature: None,
            names,
        });
        rarity
    }

    /// 전투: 도망 — 항상 성공, 전투 해제(보상 없음·페널티 없음).
    pub fn battle_flee(&mut self) {
        if self.state.pending_battle.take().is_some() {
            let name = self.display_name();
            self.push_adventure_log("battle_flee", "🏃", format!("{name}이(가) 무사히 도망쳤다!"), None, 0);
            self.save();
        }
    }

    /// 모험 로그(오래된→최신). 프런트 로그 패널용.
    pub fn adventure_log(&self) -> &[AdventureLogEntry] {
        &self.state.adventure_log
    }

    /// 표시 상태 문자열(프런트 전달용).
    pub fn display_state_str(&self) -> &'static str {
        match self.display_state {
            CompanionStateKind::Egg => "egg",
            CompanionStateKind::Idle => "idle",
            CompanionStateKind::Working => "working",
            CompanionStateKind::Focus => "focus",
            CompanionStateKind::Tired => "tired",
            CompanionStateKind::Sleep => "sleep",
            CompanionStateKind::LevelUp => "levelUp",
        }
    }
    fn hatch_core(&mut self, base_id: i64) {
        let line = match self.provider.line(base_id) {
            Ok(l) => l,
            Err(_) => return, // 라인 fetch 실패 → 알 유지, 다음 틱 재시도
        };
        let (line_base, line_rarity, line_forms) = (line.base_id, line.rarity, line.total_forms() as i64);
        self.current_line = Some(line);
        let overflow = (self.state.egg_usage - pb::EGG_HATCH_THRESHOLD).max(0);
        self.state.egg_usage = 0;
        let is_shiny = rolls_shiny(self.rng.next_u64(), self.owns_shiny_charm());
        let nature = PokemonNature::ALL[(self.rng.next_u64() % PokemonNature::ALL.len() as u64) as usize];
        // 메타몽 위장 롤 — .app 게이트(&& 단락). 비앱(테스트)에선 rng 미소비.
        let ditto_disguise = if self.is_bundled_app
            && super::companion_model::ditto_disguise_hit(line_rarity, line_forms, self.rng.next_u64())
        {
            Some(line_base)
        } else {
            None
        };
        self.state.active = Some(MonState {
            base_id: line_base,
            path_ids: vec![line_base],
            stage_index: 0,
            used_at_stage: 0,
            rarity: line_rarity,
            total_forms: line_forms,
            is_shiny,
            nature: Some(nature),
            ditto_disguise,
            ditto_revealed: false,
        });
        self.just_evolved_to = None;
        self.display_state = CompanionStateKind::LevelUp;
        self.event_until = Some((self.clock)() + Duration::seconds(4));
        self.register_species_in_dex(); // 부화한 기본종을 도감에 즉시 등록
        if overflow > 0 {
            self.apply_usage(overflow);
        }
        self.save();
    }

    /// 부화 종 선정 — capture_rate 가중 rejection-free 1롤. 인덱스 실패 시 REST 폴백.
    fn choose_base(&mut self) -> Option<i64> {
        if let Ok(index) = self.provider.base_species_index() {
            if !index.is_empty() {
                let weights: Vec<i64> = index
                    .iter()
                    .map(|e| {
                        let collected = self
                            .state
                            .collected_finals
                            .iter()
                            .any(|s| s.starts_with(&format!("{}:", e.id)));
                        if collected {
                            (e.capture_rate / 2).max(1)
                        } else {
                            e.capture_rate.max(1)
                        }
                    })
                    .collect();
                let total: i64 = weights.iter().sum();
                let mut r = (self.rng.next_u64() % total as u64) as i64;
                for (i, w) in weights.iter().enumerate() {
                    r -= w;
                    if r < 0 {
                        return Some(index[i].id);
                    }
                }
                return index.last().map(|e| e.id);
            }
        }
        // REST 폴백 — 1~649 rejection sampling.
        for _ in 0..16 {
            let id = (self.rng.next_u64() % 649) as i64 + 1;
            match self.provider.base_species(id) {
                Ok(Some(_)) => return Some(id),
                Ok(None) => continue,
                Err(_) => return None,
            }
        }
        None
    }

    // MARK: 표시 상태
    fn compute_state(
        &self,
        burn_tier: BurnTier,
        limit_warning: bool,
        has_usage_data: bool,
        today: i64,
    ) -> CompanionStateKind {
        if self.state.active.is_none() {
            return CompanionStateKind::Egg;
        }
        let event_alive = self.event_until.map(|u| (self.clock)() < u).unwrap_or(false);
        if self.just_graduated.is_some() || event_alive {
            return CompanionStateKind::LevelUp;
        }
        if limit_warning {
            return CompanionStateKind::Tired;
        }
        if !has_usage_data || today == 0 {
            return CompanionStateKind::Sleep;
        }
        match burn_tier {
            BurnTier::Idle => CompanionStateKind::Idle,
            BurnTier::Normal => CompanionStateKind::Working,
            BurnTier::Fast | BurnTier::Blazing => CompanionStateKind::Focus,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::companion_model::{EvoNode, Rarity};
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    // 고정 시각(원본 fixedNow = 1_700_000_000).
    fn fixed_now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn temp_file() -> PathBuf {
        let uniq = format!(
            "poke-{}-{}.json",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        );
        std::env::temp_dir().join(uniq)
    }

    fn node(id: i64, children: Vec<EvoNode>) -> EvoNode {
        EvoNode::new(id, children)
    }
    fn all_ids(n: &EvoNode) -> Vec<i64> {
        let mut v = vec![n.species_id];
        for c in &n.children {
            v.extend(all_ids(c));
        }
        v
    }
    fn make_line(base: i64, tree: EvoNode, rarity: Rarity) -> EvoLine {
        let mut names = HashMap::new();
        for id in all_ids(&tree) {
            let mut m = HashMap::new();
            m.insert("en".to_string(), format!("P{}", id));
            m.insert("ko".to_string(), format!("포{}", id));
            m.insert("ja".to_string(), format!("ポ{}", id));
            names.insert(id, m);
        }
        EvoLine { base_id: base, tree, rarity, names }
    }
    fn linear3() -> EvoLine {
        make_line(1, node(1, vec![node(2, vec![node(3, vec![])])]), Rarity::Common)
    }

    // 프로바이더 스텁
    struct StubProvider {
        line: EvoLine,
    }
    impl PokeProvider for StubProvider {
        fn line(&self, _id: i64) -> anyhow::Result<EvoLine> {
            Ok(self.line.clone())
        }
        fn base_species_index(&self) -> anyhow::Result<Vec<BaseSpecies>> {
            Ok(vec![BaseSpecies { id: self.line.base_id, capture_rate: 255 }])
        }
    }
    struct NoProvider;
    impl PokeProvider for NoProvider {
        fn line(&self, _id: i64) -> anyhow::Result<EvoLine> {
            Err(anyhow::anyhow!("offline"))
        }
        fn base_species_index(&self) -> anyhow::Result<Vec<BaseSpecies>> {
            Ok(vec![])
        }
    }
    /// 위장 라인(3형태 common)과 메타몽 라인(#132·rare·단일)을 id 별로 돌려주는 스텁.
    struct DittoProvider;
    impl PokeProvider for DittoProvider {
        fn line(&self, id: i64) -> anyhow::Result<EvoLine> {
            if id == 132 {
                Ok(make_line(132, node(132, vec![]), Rarity::Rare))
            } else {
                Ok(dline())
            }
        }
        fn base_species_index(&self) -> anyhow::Result<Vec<BaseSpecies>> {
            Ok(vec![])
        }
    }
    /// 위장 라인은 주지만 메타몽 라인 fetch 는 실패하는 스텁(오프라인 리빌 재시도 검증).
    struct DittoOfflineProvider;
    impl PokeProvider for DittoOfflineProvider {
        fn line(&self, id: i64) -> anyhow::Result<EvoLine> {
            if id == 132 {
                Err(anyhow::anyhow!("offline"))
            } else {
                Ok(dline())
            }
        }
        fn base_species_index(&self) -> anyhow::Result<Vec<BaseSpecies>> {
            Ok(vec![])
        }
    }

    fn store_from(state: CompanionState, provider: Box<dyn PokeProvider>, seed: u64) -> CompanionStore {
        let path = temp_file();
        std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        CompanionStore::new(provider, Box::new(fixed_now), path, Box::new(SeededRng::new(seed)), false)
    }
    fn fresh_store(provider: Box<dyn PokeProvider>, seed: u64) -> CompanionStore {
        CompanionStore::new(provider, Box::new(fixed_now), temp_file(), Box::new(SeededRng::new(seed)), false)
    }

    fn active_common3(used_at_stage: i64, nature: Option<PokemonNature>, shiny: bool) -> MonState {
        MonState {
            base_id: 1, path_ids: vec![1], stage_index: 0, used_at_stage,
            rarity: Rarity::Common, total_forms: 3, is_shiny: shiny, nature,
            ditto_disguise: None, ditto_revealed: false,
        }
    }

    // ── 상점 (ShopTests) ──
    fn shop_state(used: i64, spent: i64, rare_candy: i64) -> CompanionState {
        let mut st = CompanionState::default();
        st.install_baseline_set = true;
        st.used_since_install = used;
        st.spent_tokens = spent;
        st.last_date = "d".into();
        if rare_candy > 0 {
            st.inventory.insert("rareCandy".into(), rare_candy);
        }
        st
    }

    #[test]
    fn available_equals_used_and_subtracts_spent_and_never_negative() {
        assert_eq!(store_from(shop_state(1_000_000_000, 0, 0), Box::new(NoProvider), 1).available_tokens(), 1_000_000_000);
        assert_eq!(store_from(shop_state(1_000_000_000, 300_000_000, 0), Box::new(NoProvider), 1).available_tokens(), 700_000_000);
        assert_eq!(store_from(shop_state(100_000_000, 500_000_000, 0), Box::new(NoProvider), 1).available_tokens(), 0);
    }

    #[test]
    fn decodes_without_spent_tokens_defaults_zero() {
        let json = r#"{"install_baseline_set":true,"used_since_install":900,"last_date":"d"}"#;
        let st: CompanionState = serde_json::from_str(json).unwrap();
        assert_eq!(st.spent_tokens, 0);
        assert_eq!(st.used_since_install, 900);
    }

    #[test]
    fn buy_debits_wallet_and_credits_inventory() {
        let mut s = store_from(shop_state(1_000_000_000, 0, 0), Box::new(NoProvider), 1);
        assert!(s.buy_rare_candy());
        assert_eq!(s.rare_candy_count(), 1);
        assert_eq!(s.state.spent_tokens, rare_candy::PRICE);
        assert_eq!(s.available_tokens(), 1_000_000_000 - rare_candy::PRICE);
        assert_eq!(s.state.used_since_install, 1_000_000_000); // 성장 미터 불변
    }

    #[test]
    fn buy_insufficient_is_noop_and_multiple_until_broke() {
        let mut s = store_from(shop_state(400_000_000, 0, 0), Box::new(NoProvider), 1);
        assert!(!s.buy_rare_candy());
        assert_eq!(s.rare_candy_count(), 0);
        assert_eq!(s.state.spent_tokens, 0);

        let mut s2 = store_from(shop_state(1_200_000_000, 0, 0), Box::new(NoProvider), 1);
        assert!(s2.buy_rare_candy());
        assert!(s2.buy_rare_candy());
        assert!(!s2.buy_rare_candy());
        assert_eq!(s2.rare_candy_count(), 2);
        assert_eq!(s2.available_tokens(), 200_000_000);
    }

    #[test]
    fn buy_persists_across_restart() {
        let path = temp_file();
        std::fs::write(&path, serde_json::to_vec(&shop_state(1_000_000_000, 0, 0)).unwrap()).unwrap();
        {
            let mut s1 = CompanionStore::new(Box::new(NoProvider), Box::new(fixed_now), path.clone(), Box::new(SeededRng::new(1)), false);
            assert!(s1.buy_rare_candy());
        }
        let s2 = CompanionStore::new(Box::new(NoProvider), Box::new(fixed_now), path.clone(), Box::new(SeededRng::new(1)), false);
        assert_eq!(s2.rare_candy_count(), 1);
        assert_eq!(s2.state.spent_tokens, rare_candy::PRICE);
    }

    #[test]
    fn shop_sorting_and_entries() {
        // 가격 오름차순 (toyBall 30M < toyBalloon 50M < mint 100M < greatBall 150M < sign 200M
        //  < ultraBall 400M < flowerbed 450M < rareCandy 500M < bench 700M < fountain 1.5B < shinyCharm 3B)
        let s = store_from(shop_state(0, 0, 0), Box::new(NoProvider), 1);
        assert_eq!(
            s.purchasable_items(),
            vec![
                ItemKind::ToyBall,
                ItemKind::ToyBalloon,
                ItemKind::Mint,
                ItemKind::GreatBall,
                ItemKind::DecoSign,
                ItemKind::UltraBall,
                ItemKind::DecoFlowerbed,
                ItemKind::RareCandy,
                ItemKind::DecoBench,
                ItemKind::DecoFountain,
                ItemKind::ShinyCharm
            ]
        );

        // 구매 완료 보유형(부적)은 맨 아래
        let mut st = shop_state(0, 0, 0);
        st.inventory.insert("shinyCharm".into(), 1);
        let s2 = store_from(st, Box::new(NoProvider), 1);
        assert_eq!(s2.purchasable_items().last(), Some(&ItemKind::ShinyCharm));

        // shopEntries: active 있으면 freshEgg(1B)가 bench(700M)와 fountain(1.5B) 사이
        let mut st3 = shop_state(5_000_000_000, 0, 0);
        st3.active = Some(MonState { base_id: 10, path_ids: vec![10], stage_index: 0, used_at_stage: 200_000_000, rarity: Rarity::Common, total_forms: 3, is_shiny: false, nature: None, ditto_disguise: None, ditto_revealed: false });
        let s3 = store_from(st3, Box::new(NoProvider), 1);
        assert_eq!(
            s3.shop_entries(),
            vec![
                ShopEntry::Item(ItemKind::ToyBall),
                ShopEntry::Item(ItemKind::ToyBalloon),
                ShopEntry::Item(ItemKind::Mint),
                ShopEntry::Item(ItemKind::GreatBall),
                ShopEntry::Item(ItemKind::DecoSign),
                ShopEntry::Item(ItemKind::UltraBall),
                ShopEntry::Item(ItemKind::DecoFlowerbed),
                ShopEntry::Item(ItemKind::RareCandy),
                ShopEntry::Item(ItemKind::DecoBench),
                ShopEntry::FreshEgg,
                ShopEntry::Item(ItemKind::DecoFountain),
                ShopEntry::Item(ItemKind::ShinyCharm)
            ]
        );
        // active 없으면 freshEgg 빠짐
        let s4 = store_from(shop_state(5_000_000_000, 0, 0), Box::new(NoProvider), 1);
        assert!(!s4.shop_entries().contains(&ShopEntry::FreshEgg));
    }

    // ── 민트 (MintTests) ──
    fn mint_state(nature: Option<PokemonNature>, mint: i64, shiny: bool) -> CompanionState {
        let mut st = CompanionState::default();
        st.install_baseline_set = true;
        st.used_since_install = 1_000_000_000;
        st.last_date = "d".into();
        st.active = Some(active_common3(50_000_000, nature, shiny));
        if mint > 0 {
            st.inventory.insert("mint".into(), mint);
        }
        st
    }

    #[test]
    fn use_mint_changes_nature_to_different() {
        let mut s = store_from(mint_state(Some(PokemonNature::Adamant), 1, false), Box::new(NoProvider), 7);
        let new = s.use_mint();
        assert!(new.is_some());
        assert_ne!(new, Some(PokemonNature::Adamant));
        assert_eq!(s.state.active.as_ref().unwrap().nature, new);
        assert_eq!(s.item_count(ItemKind::Mint), 0);
    }

    #[test]
    fn use_mint_never_repeats_current() {
        let mut s = store_from(mint_state(Some(PokemonNature::Adamant), 6, false), Box::new(NoProvider), 7);
        for _ in 0..6 {
            let before = s.state.active.as_ref().unwrap().nature;
            let new = s.use_mint();
            assert!(new.is_some());
            assert_ne!(new, before);
        }
    }

    #[test]
    fn use_mint_from_nil_sets_valid_and_no_growth_change() {
        let mut s = store_from(mint_state(None, 1, true), Box::new(NoProvider), 7);
        assert!(s.state.active.as_ref().unwrap().nature.is_none());
        let before_used = s.state.used_since_install;
        let new = s.use_mint();
        assert!(new.is_some());
        assert_eq!(s.state.active.as_ref().unwrap().nature, new);
        assert_eq!(s.state.active.as_ref().unwrap().used_at_stage, 50_000_000);
        assert!(s.current_is_shiny());
        assert_eq!(s.state.used_since_install, before_used);
    }

    #[test]
    fn cannot_use_mint_on_egg_or_without_stock() {
        // 알 상태
        let mut egg = CompanionState::default();
        egg.install_baseline_set = true;
        egg.inventory.insert("mint".into(), 2);
        let mut s = store_from(egg, Box::new(NoProvider), 1);
        assert!(s.is_egg());
        assert!(!s.can_use_mint());
        assert!(s.use_mint().is_none());
        assert_eq!(s.item_count(ItemKind::Mint), 2);
        // 재고 0
        let mut s2 = store_from(mint_state(Some(PokemonNature::Adamant), 0, false), Box::new(NoProvider), 1);
        assert!(!s2.can_use_mint());
        assert!(s2.use_mint().is_none());
    }

    #[test]
    fn mint_feedback_set_and_consumed() {
        let mut s = store_from(mint_state(Some(PokemonNature::Adamant), 1, false), Box::new(NoProvider), 7);
        let before = s.mint_feedback_seq;
        let new = s.use_mint();
        assert_eq!(s.mint_feedback_nature, new);
        assert_eq!(s.mint_feedback_seq, before + 1);
        s.consume_mint_feedback();
        assert!(s.mint_feedback_nature.is_none());
    }

    #[test]
    fn buy_mint_debits_and_gate() {
        let mut s = store_from(shop_state(300_000_000, 0, 0), Box::new(NoProvider), 1);
        assert!(s.can_buy(ItemKind::Mint));
        assert!(s.buy(ItemKind::Mint));
        assert_eq!(s.item_count(ItemKind::Mint), 1);
        assert_eq!(s.state.spent_tokens, 100_000_000);
        // 가격 미만
        let s2 = store_from(shop_state(100_000_000 - 1, 0, 0), Box::new(NoProvider), 1);
        assert!(!s2.can_buy(ItemKind::Mint));
    }

    // ── 새 알 (FreshEggTests) ──
    #[test]
    fn buy_fresh_egg_discards_without_dex_or_collected_impact() {
        let mut st = shop_state(2_000_000_000, 0, 0);
        st.active = Some(active_common3(300_000_000, None, false));
        st.dex = vec![DexEntry { id: String::new(), base_id: 99, final_id: 99, chain_order: vec![99], rarity: Rarity::Rare, caught_at: None, is_shiny: false, nature: None, names: None }];
        st.collected_finals = ["99:99".to_string()].into_iter().collect();
        let mut s = store_from(st, Box::new(NoProvider), 7);
        assert!(s.buy_fresh_egg());
        assert!(s.state.active.is_none()); // 폐기
        assert_eq!(s.state.egg_usage, 0);
        assert_eq!(s.state.spent_tokens, fresh_egg::PRICE);
        assert_eq!(s.state.dex.len(), 1); // dex 불변
        assert!(s.state.collected_finals.contains("99:99")); // 확률 가중 불변
        assert!(!s.state.collected_finals.contains("1:1")); // 폐기 개체는 미수집
    }

    #[test]
    fn cannot_reroll_when_egg_or_without_funds() {
        // 알 상태
        let mut egg = CompanionState::default();
        egg.install_baseline_set = true;
        egg.used_since_install = 5_000_000_000;
        let s = store_from(egg, Box::new(NoProvider), 7);
        assert!(!s.can_buy_fresh_egg());
        // 활성이나 자금 부족
        let mut st = shop_state(fresh_egg::PRICE - 1, 0, 0);
        st.active = Some(active_common3(0, None, false));
        let mut s2 = store_from(st, Box::new(NoProvider), 7);
        assert!(!s2.can_buy_fresh_egg());
        assert!(!s2.buy_fresh_egg());
    }

    // ── 부화·진화·졸업 (CompanionTests) ──
    fn base(s: &mut CompanionStore) {
        s.update(0, "d1", 0, BurnTier::Idle, false, true);
    }

    #[test]
    fn install_baseline_excludes_pre_install_usage() {
        let mut s = fresh_store(Box::new(StubProvider { line: linear3() }), 7);
        s.update(0, "d1", 0, BurnTier::Idle, false, false);
        assert!(!s.state.install_baseline_set);
        s.update(48_000_000, "d1", 0, BurnTier::Idle, false, true);
        assert!(s.state.install_baseline_set);
        assert_eq!(s.state.used_since_install, 0);
        s.update(148_000_000, "d1", 0, BurnTier::Idle, false, true);
        assert_eq!(s.state.used_since_install, 100_000_000);
    }

    #[test]
    fn egg_hatch_threshold_and_overflow() {
        // 임계 미만 → 미부화
        let mut s = fresh_store(Box::new(StubProvider { line: linear3() }), 7);
        base(&mut s);
        s.update(500_000, "d1", 0, BurnTier::Idle, false, true);
        assert_eq!(s.state.egg_usage, 500_000);
        s.hatch_if_needed();
        assert!(s.state.active.is_none());

        // 임계 도달 → 부화, egg_usage 0
        let mut s2 = fresh_store(Box::new(StubProvider { line: linear3() }), 7);
        base(&mut s2);
        s2.update(pb::EGG_HATCH_THRESHOLD, "d1", 0, BurnTier::Idle, false, true);
        s2.hatch_if_needed();
        assert!(s2.state.active.is_some());
        assert_eq!(s2.state.egg_usage, 0);

        // 초과분 이월
        let mut s3 = fresh_store(Box::new(StubProvider { line: linear3() }), 7);
        base(&mut s3);
        s3.update(pb::EGG_HATCH_THRESHOLD + 500_000, "d1", 0, BurnTier::Idle, false, true);
        s3.hatch_if_needed();
        assert_eq!(s3.state.active.as_ref().unwrap().used_at_stage, 500_000);
    }

    #[test]
    fn graduation_stores_chain_names() {
        let mut s = fresh_store(Box::new(StubProvider { line: linear3() }), 7);
        s.hatch(1);
        s.apply_usage(pb::phase_threshold(Rarity::Common, 3, 0)); // →2
        s.apply_usage(pb::phase_threshold(Rarity::Common, 3, 1)); // →3
        s.apply_usage(pb::phase_threshold(Rarity::Common, 3, 2)); // 졸업
        // 라인 전체 수집: 기본형(1)·중간(2)·최종(3) 각각 도감 등록.
        assert_eq!(s.state.dex.len(), 3);
        assert!(s.state.dex.iter().any(|d| d.final_id == 1)); // 부화형도 수집
        assert!(s.state.dex.iter().any(|d| d.final_id == 2)); // 중간 진화형도 수집
        // 최종체 항목은 전체 체인/이름 보존.
        let d = s.state.dex.iter().find(|d| d.final_id == 3).unwrap();
        assert_eq!(d.chain_order, vec![1, 2, 3]);
        assert_eq!(d.names.as_ref().unwrap().get(&1).unwrap().get("ko").unwrap(), "포1");
        assert_eq!(d.names.as_ref().unwrap().get(&3).unwrap().get("ja").unwrap(), "ポ3");
        assert!(s.state.active.is_none()); // 졸업 후 알
    }

    #[test]
    fn evolves_once_at_threshold() {
        let mut s = fresh_store(Box::new(StubProvider { line: linear3() }), 7);
        s.hatch(1);
        assert_eq!(s.state.active.as_ref().unwrap().stage_index, 0);
        s.apply_usage(pb::phase_threshold(Rarity::Common, 3, 0));
        assert_eq!(s.state.active.as_ref().unwrap().stage_index, 1);
        assert!(s.just_evolved_to.is_some());
    }

    // ── 표시 상태 (CompanionDisplayStateTests) ──
    fn dline() -> EvoLine {
        make_line(1, node(1, vec![node(2, vec![node(3, vec![])])]), Rarity::Common)
    }

    #[test]
    fn display_egg_when_no_usage_data() {
        let mut s = fresh_store(Box::new(StubProvider { line: dline() }), 1);
        s.update(0, "d", 0, BurnTier::Idle, false, false);
        assert_eq!(s.display_state, CompanionStateKind::Egg);
    }

    #[test]
    fn display_states_after_hatch() {
        // levelUp during event window (시계 미전진)
        let mut s = fresh_store(Box::new(StubProvider { line: dline() }), 5);
        s.hatch(1);
        s.update(100, "d", 0, BurnTier::Idle, false, true);
        assert_eq!(s.display_state, CompanionStateKind::LevelUp);
    }

    #[test]
    fn display_working_focus_tired_sleep_after_event_expires() {
        // 이벤트 창이 없는(event_until=None) 활성 상태를 직접 심어 compute_state 분기 검증
        // (부화가 여는 levelUp 창과 독립적으로 burn/limit/today 분기만 본다).
        let build = |burn, limit, today, has| {
            let mut s = fresh_store(Box::new(StubProvider { line: dline() }), 5);
            s.state.active = Some(active_common3(0, None, false));
            s.update(today, "d", 0, burn, limit, has);
            s
        };
        assert_eq!(build(BurnTier::Normal, false, 100, true).display_state, CompanionStateKind::Working);
        assert_eq!(build(BurnTier::Blazing, false, 100, true).display_state, CompanionStateKind::Focus);
        assert_eq!(build(BurnTier::Normal, true, 100, true).display_state, CompanionStateKind::Tired);
        assert_eq!(build(BurnTier::Idle, false, 0, true).display_state, CompanionStateKind::Sleep);
        assert_eq!(build(BurnTier::Idle, false, 100, true).display_state, CompanionStateKind::Idle);
    }

    #[test]
    fn egg_progress_and_tokens_to_hatch() {
        let mut s = fresh_store(Box::new(StubProvider { line: dline() }), 1);
        assert!(s.is_egg());
        assert_eq!(s.egg_progress(), 0.0);
        assert_eq!(s.egg_tokens_to_hatch(), pb::EGG_HATCH_THRESHOLD);
        base(&mut s);
        let part = pb::EGG_HATCH_THRESHOLD * 2 / 5;
        s.update(part, "d", 0, BurnTier::Idle, false, true);
        assert!((s.egg_progress() - 0.4).abs() < 0.001);
        assert_eq!(s.egg_tokens_to_hatch(), pb::EGG_HATCH_THRESHOLD - part);
        assert!(s.egg_started());
    }

    // ── 야생 전투 ──
    /// 승리 턴: 사탕 지급 + pending 해제 + 포획 결과와 도감 등록이 일치.
    #[test]
    fn battle_win_grants_candy_and_catch_registers_dex() {
        use crate::core::battle::BattleState;
        // 여러 시드로 포획 성공/실패 양쪽 경로를 모두 커버.
        let (mut saw_catch, mut saw_escape) = (false, false);
        for seed in 0..40 {
            let mut s = fresh_store(Box::new(StubProvider { line: dline() }), seed);
            s.state.active = Some(active_common3(0, None, false));
            // 픽스처 HP(임의값): wild_hp=1 → 첫 공격에 확정 승리.
            s.state.pending_battle = Some(BattleState {
                wild_id: 900 + seed as i64, // dline 스텁 체인과 안 겹치는 id
                wild_level: 10,
                wild_hp: 1,
                wild_max_hp: 40,
                wild_shiny: false,
                ally_level: 20,
                ally_hp: 60,
                ally_max_hp: 60,
                turn: 0,
            });
            let before = s.rare_candy_count();
            let r = s.battle_attack("테스트몬").expect("전투 진행");
            assert!(r.outcome.wild_fainted);
            assert!(s.state.pending_battle.is_none(), "승리 후 pending 해제");
            assert_eq!(s.rare_candy_count(), before + r.reward_qty);
            let caught = r.caught.expect("승리 턴은 포획 롤 수행");
            assert_eq!(
                s.state.dex.iter().any(|d| d.final_id == 900 + seed as i64),
                caught,
                "포획 결과와 도감 등록 불일치"
            );
            assert_eq!(s.state.caught_count, caught as i64, "포획 통계 불일치");
            assert!(r.ball_used.is_none(), "볼 미보유 시 기본 몬스터볼(소모 없음)");
            saw_catch |= caught;
            saw_escape |= !caught;
        }
        assert!(saw_catch && saw_escape, "40개 시드에서 성공/실패가 모두 나와야 함");
    }

    /// 볼 보유 시 좋은 볼부터 자동 소모(하이퍼 > 수퍼), 실패해도 소모.
    #[test]
    fn battle_catch_consumes_best_ball_first() {
        use crate::core::battle::BattleState;
        let mkbattle = || BattleState {
            wild_id: 500,
            wild_level: 10,
            wild_hp: 1, // 픽스처(임의값): 첫 공격 확정 승리
            wild_max_hp: 40,
            wild_shiny: false,
            ally_level: 20,
            ally_hp: 60,
            ally_max_hp: 60,
            turn: 0,
        };
        let mut s = fresh_store(Box::new(StubProvider { line: dline() }), 11);
        s.state.active = Some(active_common3(0, None, false));
        s.state.inventory.insert(ItemKind::UltraBall.raw_value().to_string(), 1);
        s.state.inventory.insert(ItemKind::GreatBall.raw_value().to_string(), 1);

        s.state.pending_battle = Some(mkbattle());
        let r = s.battle_attack("테스트몬").unwrap();
        assert_eq!(r.ball_used, Some("ultraBall"), "하이퍼볼 우선");
        assert_eq!(s.item_count(ItemKind::UltraBall), 0, "성공/실패 무관 소모");
        assert_eq!(s.item_count(ItemKind::GreatBall), 1);

        s.state.pending_battle = Some(mkbattle());
        let r2 = s.battle_attack("테스트몬").unwrap();
        assert_eq!(r2.ball_used, Some("greatBall"), "하이퍼볼 소진 후 수퍼볼");
        assert_eq!(s.item_count(ItemKind::GreatBall), 0);

        s.state.pending_battle = Some(mkbattle());
        let r3 = s.battle_attack("테스트몬").unwrap();
        assert_eq!(r3.ball_used, None, "볼 소진 후 기본 몬스터볼");
    }

    /// 장난감 소모 — 보유 시 차감·true, 0개면 false(차감 없음).
    #[test]
    fn consume_toy_decrements_and_fails_at_zero() {
        let mut s = fresh_store(Box::new(NoProvider), 1);
        s.state.inventory.insert(ItemKind::ToyBall.raw_value().to_string(), 2);
        assert!(s.consume_item(ItemKind::ToyBall));
        assert!(s.consume_item(ItemKind::ToyBall));
        assert_eq!(s.item_count(ItemKind::ToyBall), 0);
        assert!(!s.consume_item(ItemKind::ToyBall), "0개면 실패");
        assert!(!s.consume_item(ItemKind::ToyBalloon), "미보유 종류도 실패");
    }

    // ── 메타몽 위장 리빌 (원본 DittoTests의 리빌 파트 — 포팅 누락 회귀) ──
    /// 위장 개체가 첫 진화 임계에서 진화 대신 메타몽으로 리빌된다(사탕/사용량 공통 경로).
    /// 회귀: 리빌 미구현 시 위장 개체는 사탕을 먹여도 영원히 진화 불가였다.
    #[test]
    fn ditto_reveals_at_first_evolution_threshold_instead_of_evolving() {
        let mut s = fresh_store(Box::new(DittoProvider), 9);
        s.state.install_baseline_set = true;
        s.state.active = Some(MonState {
            base_id: 1, path_ids: vec![1], stage_index: 0, used_at_stage: 0,
            rarity: Rarity::Common, total_forms: 3, is_shiny: true, nature: None,
            ditto_disguise: Some(1), ditto_revealed: false,
        });
        s.ensure_line_loaded(); // 위장 종 라인 로드
        let thr = pb::phase_threshold(Rarity::Common, 3, 0);
        s.apply_usage(thr + 10);
        let a = s.state.active.as_ref().expect("리빌은 졸업이 아님 — 활성 유지");
        assert!(a.ditto_revealed, "첫 진화 임계에서 리빌돼야 한다");
        assert_eq!(a.base_id, 132);
        assert_eq!(a.path_ids, vec![132]);
        assert_eq!(a.stage_index, 0);
        assert_eq!(a.rarity, Rarity::Rare, "리빌 후 rarity 는 메타몽 라인 기준");
        assert_eq!(a.total_forms, 1);
        assert_eq!(a.used_at_stage, 10, "첫 임계 초과분 이월");
        assert!(a.is_shiny, "이로치 유지");
        assert!(a.ditto_disguise.is_some(), "위장 마커 보존");
        assert!(s.state.dex.iter().any(|d| d.final_id == 132), "메타몽 도감 등록");
        assert!(s.adventure_log().iter().any(|e| e.id == "ditto_reveal"), "리빌 로그");
        assert_eq!(s.display_state, CompanionStateKind::LevelUp, "리빌 연출 창");
    }

    /// 메타몽 라인 fetch 실패(오프라인) → 위장 유지 + XP 무손실, 이후 재시도로 리빌.
    #[test]
    fn ditto_reveal_offline_keeps_progress_and_retries() {
        let mut s = fresh_store(Box::new(DittoOfflineProvider), 9);
        s.state.install_baseline_set = true;
        s.state.active = Some(MonState {
            base_id: 1, path_ids: vec![1], stage_index: 0, used_at_stage: 0,
            rarity: Rarity::Common, total_forms: 3, is_shiny: false, nature: None,
            ditto_disguise: Some(1), ditto_revealed: false,
        });
        s.ensure_line_loaded();
        let thr = pb::phase_threshold(Rarity::Common, 3, 0);
        s.apply_usage(thr + 10);
        let a = s.state.active.as_ref().unwrap();
        assert!(!a.ditto_revealed, "fetch 실패 시 위장 유지");
        assert_eq!(a.base_id, 1);
        assert_eq!(a.used_at_stage, thr + 10, "임계 초과분 무손실(다음 틱 재시도용)");

        // 네트워크 복구 시뮬레이션 — 프로바이더 교체 후 apply_usage(0) 재평가로 리빌.
        s.provider = Box::new(DittoProvider);
        s.apply_usage(0);
        let a = s.state.active.as_ref().unwrap();
        assert!(a.ditto_revealed, "복구 후 재시도에서 리빌");
        assert_eq!(a.used_at_stage, 10);
    }

    /// 진행 턴엔 포획 롤 없음 + pending 유지.
    #[test]
    fn battle_ongoing_turn_has_no_catch() {
        use crate::core::battle::BattleState;
        let mut s = fresh_store(Box::new(StubProvider { line: dline() }), 3);
        s.state.active = Some(active_common3(0, None, false));
        s.state.pending_battle = Some(BattleState {
            wild_id: 25,
            wild_level: 50,
            wild_hp: 999, // 픽스처(임의값): 한 턴에 안 끝나게
            wild_max_hp: 999,
            wild_shiny: false,
            ally_level: 50,
            ally_hp: 999,
            ally_max_hp: 999,
            turn: 0,
        });
        let r = s.battle_attack("피카츄").expect("전투 진행");
        assert!(!r.outcome.wild_fainted && !r.outcome.ally_fainted);
        assert!(r.caught.is_none());
        assert!(s.state.pending_battle.is_some(), "진행 중이면 pending 유지");
        assert!(s.state.dex.is_empty());
    }
}
