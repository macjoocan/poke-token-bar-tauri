//! 게임 규칙·타입·밸런스 상수 + 순수 결정함수. 원본 `CompanionModel.swift` 이식
//! (+ `CompanionStore` 의 정적 순수함수 evaluate_candy_grants / rolls_shiny / ditto_disguise_hit).
//!
//! ⚠️ 스토어 인스턴스 동작(applyUsage 진화·졸업 상태머신, useMint, buy, 부화)은 영속·네트워크·RNG
//! 스택 전체가 필요해 **M2(companion_store)** 에서 이식한다. 여기엔 순수 규칙만 둔다.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────
// 희귀도
// ─────────────────────────────────────────────────────────────────────────

/// 표시 상태 — 사용량/burn 으로 결정(스프라이트 모션·상태 문구).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionStateKind {
    Egg,
    Idle,
    Working,
    Focus,
    Tired,
    Sleep,
    LevelUp,
}

/// 번 레이트 등급 (원본 UsageStore.BurnTier).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurnTier {
    Idle,
    Normal,
    Fast,
    Blazing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Legendary,
}

impl Rarity {
    /// 정렬 순위(높을수록 희귀). legendary→rare→uncommon→common.
    pub fn sort_rank(self) -> i32 {
        match self {
            Rarity::Common => 0,
            Rarity::Uncommon => 1,
            Rarity::Rare => 2,
            Rarity::Legendary => 3,
        }
    }

    pub fn from(capture_rate: i32, is_legendary: bool, is_mythical: bool) -> Rarity {
        if is_legendary || is_mythical {
            return Rarity::Legendary;
        }
        if capture_rate <= 45 {
            return Rarity::Rare;
        }
        if capture_rate <= 120 {
            return Rarity::Uncommon;
        }
        Rarity::Common
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 토큰 경제 밸런스
// ─────────────────────────────────────────────────────────────────────────

pub mod pokemon_balance {
    use super::Rarity;

    /// 알 부화 임계 — 이만큼 토큰을 써야 알이 깨진다. 초과분은 부화체 성장에 이월.
    pub const EGG_HATCH_THRESHOLD: i64 = 5_000_000;

    pub fn graduation_total(rarity: Rarity) -> i64 {
        match rarity {
            Rarity::Common => 750_000_000,
            Rarity::Uncommon => 1_875_000_000,
            Rarity::Rare => 3_000_000_000,
            Rarity::Legendary => 6_000_000_000,
        }
    }

    /// stage_index(0-based)에서 다음 단계/졸업까지 필요한 토큰.
    /// 형태 k개 라인에서 i번째(1-based) 형태 성장 비용 = T·i / (k(k+1)/2) → 합 = T.
    pub fn phase_threshold(rarity: Rarity, total_forms: i64, stage_index: i64) -> i64 {
        let kk = total_forms.max(1);
        let i = stage_index + 1; // 1-based
        let total = graduation_total(rarity) as f64;
        let denom = (kk * (kk + 1)) as f64 / 2.0;
        (total * i as f64 / denom).round() as i64
    }
}

// 아이템 밸런스 상수
pub mod rare_candy {
    /// 사용 시 현재 포켓몬에 주입하는 XP(토큰 환산).
    pub const XP: i64 = 100_000_000;
    /// 주간 한도 100% 도달 시 지급 개수.
    pub const WEEKLY_GRANT: i64 = 5;
    /// 상점 구매가.
    pub const PRICE: i64 = 500_000_000;
}
pub mod mint {
    pub const PRICE: i64 = 100_000_000;
}
pub mod poke_ball {
    /// 수퍼볼 구매가 — 전투 승리 후 포획에 자동 사용(1회용). 확률 보너스는 battle::tuning.
    pub const GREAT_PRICE: i64 = 150_000_000;
    /// 하이퍼볼 구매가 — 수퍼볼보다 우선 사용(1회용).
    pub const ULTRA_PRICE: i64 = 400_000_000;
}
pub mod plaza_goods {
    /// 장난감(소모품) — 광장에 던져 포켓몬과 놀아주기.
    pub const TOY_BALL_PRICE: i64 = 30_000_000;
    pub const TOY_BALLOON_PRICE: i64 = 50_000_000;
    /// 꾸미기(영구) — 광장 지정 자리에 배치되는 오브젝트. 토큰 싱크.
    pub const DECO_SIGN_PRICE: i64 = 200_000_000;
    pub const DECO_FLOWERBED_PRICE: i64 = 450_000_000;
    pub const DECO_BENCH_PRICE: i64 = 700_000_000;
    pub const DECO_FOUNTAIN_PRICE: i64 = 1_500_000_000;
}
pub mod shiny_charm {
    pub const PRICE: i64 = 3_000_000_000;
    /// 보유 시 이로치 부화 확률 분모 — 1/64 → 1/48 (+33%).
    pub const SHINY_DENOMINATOR: u64 = 48;
}
pub mod fresh_egg {
    pub const PRICE: i64 = 1_000_000_000;
}
pub mod pokemon_odds {
    /// 이로치 부화 확률 분모 — 1/64.
    pub const SHINY_DENOMINATOR: u64 = 64;
    /// 메타몽 위장 확률 분모 — common·≥2형태에 한해 1/128.
    pub const DITTO_DISGUISE_DENOMINATOR: u64 = 128;
    /// 메타몽 종 id.
    pub const DITTO_SPECIES_ID: i64 = 132;
}

// ─────────────────────────────────────────────────────────────────────────
// 인벤토리 아이템
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    RareCandy,
    Mint,
    ShinyCharm,
    GreatBall,
    UltraBall,
    ToyBall,
    ToyBalloon,
    DecoSign,
    DecoFlowerbed,
    DecoBench,
    DecoFountain,
}

impl ItemKind {
    /// 선언 순서(원본 CaseIterable) — 가방/상점 기본 정렬.
    pub const ALL: [ItemKind; 11] = [
        ItemKind::RareCandy,
        ItemKind::Mint,
        ItemKind::ShinyCharm,
        ItemKind::GreatBall,
        ItemKind::UltraBall,
        ItemKind::ToyBall,
        ItemKind::ToyBalloon,
        ItemKind::DecoSign,
        ItemKind::DecoFlowerbed,
        ItemKind::DecoBench,
        ItemKind::DecoFountain,
    ];

    /// 광장 꾸미기(영구 배치) 오브젝트들.
    pub const DECOS: [ItemKind; 4] = [
        ItemKind::DecoSign,
        ItemKind::DecoFlowerbed,
        ItemKind::DecoBench,
        ItemKind::DecoFountain,
    ];

    /// PokéAPI 아이템 스프라이트 파일명. None = 스프라이트 없음(이모지 폴백만).
    pub fn sprite_name(self) -> Option<&'static str> {
        match self {
            ItemKind::RareCandy => Some("rare-candy"),
            ItemKind::ShinyCharm => Some("shiny-charm"),
            ItemKind::GreatBall => Some("great-ball"),
            ItemKind::UltraBall => Some("ultra-ball"),
            _ => None,
        }
    }
    pub fn fallback_emoji(self) -> &'static str {
        match self {
            ItemKind::RareCandy => "🍬",
            ItemKind::Mint => "🌿",
            ItemKind::ShinyCharm => "✨",
            ItemKind::GreatBall => "🔵",
            ItemKind::UltraBall => "🟡",
            ItemKind::ToyBall => "⚽",
            ItemKind::ToyBalloon => "🎈",
            ItemKind::DecoSign => "🪧",
            ItemKind::DecoFlowerbed => "🌷",
            ItemKind::DecoBench => "🪑",
            ItemKind::DecoFountain => "⛲",
        }
    }
    /// 상점 판매가. None = 상점 미판매.
    pub fn shop_price(self) -> Option<i64> {
        match self {
            ItemKind::RareCandy => Some(rare_candy::PRICE),
            ItemKind::Mint => Some(mint::PRICE),
            ItemKind::ShinyCharm => Some(shiny_charm::PRICE),
            ItemKind::GreatBall => Some(poke_ball::GREAT_PRICE),
            ItemKind::UltraBall => Some(poke_ball::ULTRA_PRICE),
            ItemKind::ToyBall => Some(plaza_goods::TOY_BALL_PRICE),
            ItemKind::ToyBalloon => Some(plaza_goods::TOY_BALLOON_PRICE),
            ItemKind::DecoSign => Some(plaza_goods::DECO_SIGN_PRICE),
            ItemKind::DecoFlowerbed => Some(plaza_goods::DECO_FLOWERBED_PRICE),
            ItemKind::DecoBench => Some(plaza_goods::DECO_BENCH_PRICE),
            ItemKind::DecoFountain => Some(plaza_goods::DECO_FOUNTAIN_PRICE),
        }
    }
    /// 보유형(패시브) 아이템 — 1회 구매·상시 효과. 꾸미기 오브젝트 포함.
    pub fn is_passive(self) -> bool {
        matches!(
            self,
            ItemKind::ShinyCharm
                | ItemKind::DecoSign
                | ItemKind::DecoFlowerbed
                | ItemKind::DecoBench
                | ItemKind::DecoFountain
        )
    }
    pub fn raw_value(self) -> &'static str {
        match self {
            ItemKind::RareCandy => "rareCandy",
            ItemKind::Mint => "mint",
            ItemKind::ShinyCharm => "shinyCharm",
            ItemKind::GreatBall => "greatBall",
            ItemKind::UltraBall => "ultraBall",
            ItemKind::ToyBall => "toyBall",
            ItemKind::ToyBalloon => "toyBalloon",
            ItemKind::DecoSign => "decoSign",
            ItemKind::DecoFlowerbed => "decoFlowerbed",
            ItemKind::DecoBench => "decoBench",
            ItemKind::DecoFountain => "decoFountain",
        }
    }
}

/// 상점 표시 한 줄 — 판매 아이템 또는 새 알 리롤.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopEntry {
    Item(ItemKind),
    FreshEgg,
}

impl ShopEntry {
    pub fn price(self) -> i64 {
        match self {
            ShopEntry::Item(kind) => kind.shop_price().unwrap_or(0),
            ShopEntry::FreshEgg => fresh_egg::PRICE,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 앱 언어
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppLanguage {
    Ko,
    En,
    Ja,
}

impl AppLanguage {
    /// PokéAPI language.name 후보(첫 매칭 사용).
    pub fn api_codes(self) -> &'static [&'static str] {
        match self {
            AppLanguage::Ko => &["ko"],
            AppLanguage::En => &["en"],
            AppLanguage::Ja => &["ja-Hrkt", "ja"],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            AppLanguage::Ko => "한국어",
            AppLanguage::En => "English",
            AppLanguage::Ja => "日本語",
        }
    }
    /// byLang(langCode→name) 에서 이 언어의 이름을 고른다(apiCodes 첫 매칭 → 영어 폴백).
    pub fn resolve_name(self, by_lang: &HashMap<String, String>) -> Option<String> {
        for code in self.api_codes() {
            if let Some(n) = by_lang.get(*code) {
                return Some(n.clone());
            }
        }
        by_lang.get("en").cloned()
    }
    /// 신규 설치 기본 언어 — OS 로케일 감지(ko/ja 매칭, 그 외 en). 글로벌 출시라 한국어 강제 금지.
    pub fn system_default() -> AppLanguage {
        let loc = sys_locale::get_locale().unwrap_or_default().to_lowercase();
        if loc.starts_with("ko") {
            AppLanguage::Ko
        } else if loc.starts_with("ja") {
            AppLanguage::Ja
        } else {
            AppLanguage::En
        }
    }
}

impl Default for AppLanguage {
    fn default() -> Self {
        AppLanguage::system_default()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 성격 (부화 시 확정, 능력치 무관). TODO(M2): name(lang) 다국어 명칭.
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PokemonNature {
    Hardy, Lonely, Brave, Adamant, Naughty,
    Bold, Docile, Relaxed, Impish, Lax,
    Timid, Hasty, Serious, Jolly, Naive,
    Modest, Mild, Quiet, Bashful, Rash,
    Calm, Gentle, Sassy, Careful, Quirky,
}

impl PokemonNature {
    /// 본가 25종(선언 순서). 부화 롤·민트 풀에 사용.
    pub const ALL: [PokemonNature; 25] = [
        PokemonNature::Hardy, PokemonNature::Lonely, PokemonNature::Brave, PokemonNature::Adamant, PokemonNature::Naughty,
        PokemonNature::Bold, PokemonNature::Docile, PokemonNature::Relaxed, PokemonNature::Impish, PokemonNature::Lax,
        PokemonNature::Timid, PokemonNature::Hasty, PokemonNature::Serious, PokemonNature::Jolly, PokemonNature::Naive,
        PokemonNature::Modest, PokemonNature::Mild, PokemonNature::Quiet, PokemonNature::Bashful, PokemonNature::Rash,
        PokemonNature::Calm, PokemonNature::Gentle, PokemonNature::Sassy, PokemonNature::Careful, PokemonNature::Quirky,
    ];
}

// ─────────────────────────────────────────────────────────────────────────
// 진화 트리 / 라인
// ─────────────────────────────────────────────────────────────────────────

/// PokéAPI evolution-chain 을 파싱한 트리. 분기(evolves_to 다수)를 children 으로.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvoNode {
    #[serde(rename = "speciesID")]
    pub species_id: i64,
    pub children: Vec<EvoNode>,
}

impl EvoNode {
    pub fn new(species_id: i64, children: Vec<EvoNode>) -> EvoNode {
        EvoNode { species_id, children }
    }
    /// 최장 경로 길이(형태 수).
    pub fn depth(&self) -> usize {
        1 + self.children.iter().map(|c| c.depth()).max().unwrap_or(0)
    }
    pub fn node(&self, id: i64) -> Option<&EvoNode> {
        if self.species_id == id {
            return Some(self);
        }
        for c in &self.children {
            if let Some(f) = c.node(id) {
                return Some(f);
            }
        }
        None
    }
    /// 이 노드에서 도달 가능한 모든 최종체 id.
    pub fn final_ids(&self) -> Vec<i64> {
        if self.children.is_empty() {
            vec![self.species_id]
        } else {
            self.children.iter().flat_map(|c| c.final_ids()).collect()
        }
    }
    /// 루트→`id` 경로(양끝 포함). 트리에 없으면 None. (야생 포획 도감 등록용)
    pub fn path_to(&self, id: i64) -> Option<Vec<i64>> {
        if self.species_id == id {
            return Some(vec![id]);
        }
        for c in &self.children {
            if let Some(mut p) = c.path_to(id) {
                p.insert(0, self.species_id);
                return Some(p);
            }
        }
        None
    }
}

/// 부화 시 확정되는 라인 정보(트리 + 희귀도 + 다국어 이름).
#[derive(Debug, Clone)]
pub struct EvoLine {
    pub base_id: i64,
    pub tree: EvoNode,
    pub rarity: Rarity,
    /// speciesID → (langCode → name)
    pub names: HashMap<i64, HashMap<String, String>>,
}

impl EvoLine {
    pub fn total_forms(&self) -> usize {
        self.tree.depth()
    }
    pub fn localized_name(&self, id: i64, lang: AppLanguage) -> String {
        let empty = HashMap::new();
        let by_lang = self.names.get(&id).unwrap_or(&empty);
        lang.resolve_name(by_lang).unwrap_or_else(|| format!("#{}", id))
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 사탕 지급 판정 입력/결과
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowClass {
    Session,
    Weekly,
}

/// 사탕 지급 판정 입력 — 프로바이더 무관 한도 창 1개.
#[derive(Debug, Clone)]
pub struct CandyWindow {
    pub key: String,        // 안정 식별자(tier 추적) — resets_at 등 휘발 필드 금지
    pub name: String,       // 표시용(알림 "왜 받는지")
    pub kind: WindowClass,  // session=1개 · weekly=5개
    pub utilization: f64,   // 0~100+
}

/// 사탕 지급 1건(순수 판정 결과).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandyGrant {
    pub window_key: String,
    pub window_name: String,
    pub count: i64,
}

// ─────────────────────────────────────────────────────────────────────────
// 현재 포켓몬 / 도감 / 영속 상태
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MonState {
    pub base_id: i64,
    pub path_ids: Vec<i64>, // 실제 진화 경로(분기 선택 반영)
    pub stage_index: i64,   // path_ids 내 현재 위치
    pub used_at_stage: i64, // 현재 형태에서 누적 사용량
    pub rarity: Rarity,
    pub total_forms: i64,
    #[serde(default)]
    pub is_shiny: bool,
    #[serde(default)]
    pub nature: Option<PokemonNature>,
    #[serde(default)]
    pub ditto_disguise: Option<i64>,
    #[serde(default)]
    pub ditto_revealed: bool,
}

impl MonState {
    /// path_ids 가 비면 base_id 로 폴백; stage_index 는 경로 끝으로 클램프(out-of-bounds 방지).
    pub fn current_id(&self) -> i64 {
        if self.path_ids.is_empty() {
            self.base_id
        } else {
            let idx = (self.stage_index as usize).min(self.path_ids.len() - 1);
            self.path_ids[idx]
        }
    }
}

/// 도감 항목 — 라인 전체(초기→최종) 순서 보존.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DexEntry {
    #[serde(default)]
    pub id: String,
    pub base_id: i64,
    pub final_id: i64,
    pub chain_order: Vec<i64>, // 초기→최종 종 id
    pub rarity: Rarity,
    #[serde(default)]
    pub caught_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub is_shiny: bool,
    #[serde(default)]
    pub nature: Option<PokemonNature>,
    #[serde(default)]
    pub names: Option<HashMap<i64, HashMap<String, String>>>,
}

/// 영속 상태(app-data JSON).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompanionState {
    #[serde(default)]
    pub install_baseline_set: bool,
    #[serde(default)]
    pub used_since_install: i64,
    #[serde(default)]
    pub spent_tokens: i64,
    #[serde(default)]
    pub egg_usage: i64,
    #[serde(default)]
    pub pending_hatch_id: Option<i64>,
    #[serde(default)]
    pub claimed_today_tokens: i64,
    #[serde(default)]
    pub last_date: String,
    #[serde(default)]
    pub active: Option<MonState>,
    #[serde(default)]
    pub dex: Vec<DexEntry>,
    #[serde(default)]
    pub collected_finals: std::collections::HashSet<String>,
    #[serde(default)]
    pub language: AppLanguage,
    #[serde(default)]
    pub inventory: HashMap<String, i64>,
    #[serde(default)]
    pub candy_grant_tier: HashMap<String, i64>,
    #[serde(default)]
    pub candy_feature_seeded: bool,
    // ── 모험(CRPG) 이벤트 ──
    /// 마지막 이벤트 발생 시점의 `used_since_install`(토큰). 다음 이벤트 간격 판정 기준.
    #[serde(default)]
    pub last_event_tokens: i64,
    /// 이벤트 로그 정렬용 단조 증가 seq.
    #[serde(default)]
    pub event_seq: u64,
    /// 최근 모험 이벤트 로그(오래된→최신, 최대 `ADVENTURE_LOG_CAP` 개).
    #[serde(default)]
    pub adventure_log: Vec<AdventureLogEntry>,
    /// 진행 중 야생 전투(모험 조우). 없으면 None. 구버전 저장 데이터 호환(default).
    #[serde(default)]
    pub pending_battle: Option<super::battle::BattleState>,
    /// 몬스터볼 포획 성공 누계(통계 — 중복 종 재포획도 셈).
    #[serde(default)]
    pub caught_count: i64,
    /// 사용자가 설정에서 언어를 직접 골랐는가. false 면 로드 시 OS 로케일로 재유도한다
    /// (과거 기본값이 영어로 하드코딩돼 저장된 `language:"en"` 을 한국어 로케일에서 자동 교정).
    #[serde(default)]
    pub language_set_by_user: bool,
}

/// 영속 모험 로그 한 줄(app-data JSON).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdventureLogEntry {
    pub seq: u64,
    /// 이벤트 종류 식별자(아이콘/디버그).
    pub id: String,
    pub emoji: String,
    pub message: String,
    /// 획득 아이템(있으면 `ItemKind::raw_value`).
    #[serde(default)]
    pub reward: Option<String>,
    /// 획득 수량(잭팟 등 다수). 기본 1.
    #[serde(default = "one_i64")]
    pub reward_qty: i64,
}

fn one_i64() -> i64 {
    1
}

impl Default for CompanionState {
    fn default() -> Self {
        CompanionState {
            install_baseline_set: false,
            used_since_install: 0,
            spent_tokens: 0,
            egg_usage: 0,
            pending_hatch_id: None,
            claimed_today_tokens: 0,
            last_date: String::new(),
            active: None,
            dex: Vec::new(),
            collected_finals: std::collections::HashSet::new(),
            language: AppLanguage::default(),
            inventory: HashMap::new(),
            candy_grant_tier: HashMap::new(),
            candy_feature_seeded: false,
            last_event_tokens: 0,
            event_seq: 0,
            adventure_log: Vec::new(),
            pending_battle: None,
            caught_count: 0,
            language_set_by_user: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 순수 결정함수 (원본 CompanionStore 의 static 함수)
// ─────────────────────────────────────────────────────────────────────────

/// 사탕 지급 판정(순수·엣지 트리거) — 한도 창이 100% 를 새로 넘어선 순간에만 지급.
/// - 100% 미만 → 맵에서 제거(재무장). / 이미 지급(tier≥1)이면 재지급 안 함.
pub fn evaluate_candy_grants(
    windows: &[CandyWindow],
    grant_tier: &mut HashMap<String, i64>,
) -> Vec<CandyGrant> {
    let mut grants = Vec::new();
    for w in windows {
        if w.utilization < 100.0 {
            grant_tier.remove(&w.key);
            continue;
        }
        let previous = grant_tier.get(&w.key).copied().unwrap_or(0);
        if previous >= 1 {
            continue;
        }
        grant_tier.insert(w.key.clone(), 1);
        let count = if w.kind == WindowClass::Weekly {
            rare_candy::WEEKLY_GRANT
        } else {
            1
        };
        grants.push(CandyGrant {
            window_key: w.key.clone(),
            window_name: w.name.clone(),
            count,
        });
    }
    grants
}

/// 메타몽 위장 롤 판정(순수) — common·≥2형태만, roll % 128 == 0.
pub fn ditto_disguise_hit(rarity: Rarity, total_forms: i64, roll: u64) -> bool {
    rarity == Rarity::Common
        && total_forms >= 2
        && roll % pokemon_odds::DITTO_DISGUISE_DENOMINATOR == 0
}

/// 이로치 부화 판정(순수) — roll % 분모(부적 48, 없으면 64) == 0.
pub fn rolls_shiny(roll: u64, charm_owned: bool) -> bool {
    let denom = if charm_owned {
        shiny_charm::SHINY_DENOMINATOR
    } else {
        pokemon_odds::SHINY_DENOMINATOR
    };
    roll % denom == 0
}

#[cfg(test)]
mod tests {
    use super::pokemon_balance as pb;
    use super::*;

    fn node(id: i64, children: Vec<EvoNode>) -> EvoNode {
        EvoNode::new(id, children)
    }

    // ── 원본 CompanionTests.PokemonBalanceTests ──
    #[test]
    fn graduation_total_constant_per_rarity_regardless_of_stages() {
        for rarity in [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::Legendary] {
            let t = pb::graduation_total(rarity);
            for k in 1..=3i64 {
                let sum: i64 = (0..k).map(|i| pb::phase_threshold(rarity, k, i)).sum();
                // 반올림 오차 허용(±형태수) — 합이 T 에 수렴.
                assert!((sum - t).abs() <= k, "rarity={:?} k={} sum={} T={}", rarity, k, sum, t);
            }
        }
    }

    #[test]
    fn higher_stage_costs_more() {
        for k in 2..=3i64 {
            for i in 0..(k - 1) {
                assert!(
                    pb::phase_threshold(Rarity::Common, k, i)
                        < pb::phase_threshold(Rarity::Common, k, i + 1)
                );
            }
        }
    }

    #[test]
    fn rarer_costs_more() {
        assert!(pb::graduation_total(Rarity::Common) < pb::graduation_total(Rarity::Uncommon));
        assert!(pb::graduation_total(Rarity::Uncommon) < pb::graduation_total(Rarity::Rare));
        assert!(pb::graduation_total(Rarity::Rare) < pb::graduation_total(Rarity::Legendary));
    }

    // ── 원본 ModelLogicTests.RarityBoundaryTests ──
    #[test]
    fn rarity_capture_rate_boundaries() {
        assert_eq!(Rarity::from(45, false, false), Rarity::Rare);
        assert_eq!(Rarity::from(46, false, false), Rarity::Uncommon);
        assert_eq!(Rarity::from(120, false, false), Rarity::Uncommon);
        assert_eq!(Rarity::from(121, false, false), Rarity::Common);
        assert_eq!(Rarity::from(255, false, false), Rarity::Common);
        assert_eq!(Rarity::from(90, false, false), Rarity::Uncommon);
    }

    #[test]
    fn rarity_legendary_mythical_override() {
        assert_eq!(Rarity::from(255, true, false), Rarity::Legendary);
        assert_eq!(Rarity::from(255, false, true), Rarity::Legendary);
        assert_eq!(Rarity::from(3, true, false), Rarity::Legendary);
    }

    // ── 원본 ModelLogicTests.EvoNodeTests ──
    #[test]
    fn evo_node_depth_node_finals() {
        // 1 → {2 → 3, 4}
        let tree = node(1, vec![node(2, vec![node(3, vec![])]), node(4, vec![])]);
        assert_eq!(tree.depth(), 3);
        assert_eq!(node(20, vec![]).depth(), 1);
        assert_eq!(tree.node(3).map(|n| n.species_id), Some(3));
        assert_eq!(tree.node(4).map(|n| n.species_id), Some(4));
        assert!(tree.node(99).is_none());
        let mut finals = tree.final_ids();
        finals.sort();
        assert_eq!(finals, vec![3, 4]);
        assert_eq!(node(20, vec![]).final_ids(), vec![20]);
    }

    // ── 원본 ModelLogicTests.EvoLineNameTests ──
    fn line_with(names: HashMap<i64, HashMap<String, String>>) -> EvoLine {
        EvoLine { base_id: 1, tree: node(1, vec![]), rarity: Rarity::Common, names }
    }
    fn m(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn localized_name_fallback_chain() {
        let mut names = HashMap::new();
        names.insert(1, m(&[("ja-Hrkt", "ピカ"), ("ja", "ピカチュウ"), ("en", "Pika"), ("ko", "피카")]));
        names.insert(2, m(&[("en", "Eevee")]));
        names.insert(3, HashMap::new());
        let line = line_with(names);
        assert_eq!(line.localized_name(1, AppLanguage::Ja), "ピカ"); // ja-Hrkt 우선
        assert_eq!(line.localized_name(1, AppLanguage::Ko), "피카");
        assert_eq!(line.localized_name(1, AppLanguage::En), "Pika");
        assert_eq!(line.localized_name(2, AppLanguage::Ja), "Eevee"); // en 폴백
        assert_eq!(line.localized_name(2, AppLanguage::Ko), "Eevee");
        assert_eq!(line.localized_name(3, AppLanguage::Ko), "#3"); // 빈 맵 → #id
        assert_eq!(line.localized_name(99, AppLanguage::En), "#99"); // 없는 id
    }

    #[test]
    fn ja_falls_back_from_hrkt_to_plain_ja() {
        let mut names = HashMap::new();
        names.insert(1, m(&[("ja", "ピカチュウ"), ("en", "Pika")]));
        let line = line_with(names);
        assert_eq!(line.localized_name(1, AppLanguage::Ja), "ピカチュウ");
    }

    // ── 원본 RareCandyTests.CandyGrantEvaluationTests ──
    fn w(key: &str, kind: WindowClass, util: f64, name: &str) -> CandyWindow {
        CandyWindow { key: key.to_string(), name: name.to_string(), kind, utilization: util }
    }

    #[test]
    fn candy_session_grants_one() {
        let mut tier = HashMap::new();
        let g = evaluate_candy_grants(&[w("s", WindowClass::Session, 100.0, "T")], &mut tier);
        assert_eq!(g.iter().map(|x| x.count).collect::<Vec<_>>(), vec![1]);
        assert_eq!(tier.get("s"), Some(&1));
    }

    #[test]
    fn candy_weekly_grants_five() {
        let mut tier = HashMap::new();
        let g = evaluate_candy_grants(&[w("wk", WindowClass::Weekly, 100.0, "T")], &mut tier);
        assert_eq!(g.iter().map(|x| x.count).collect::<Vec<_>>(), vec![rare_candy::WEEKLY_GRANT]);
    }

    #[test]
    fn candy_below_100_no_grant() {
        let mut tier = HashMap::new();
        let g = evaluate_candy_grants(&[w("s", WindowClass::Session, 99.9, "T")], &mut tier);
        assert!(g.is_empty());
        assert_eq!(tier.get("s"), None);
    }

    #[test]
    fn candy_no_double_grant_while_at_100() {
        let mut tier = HashMap::new();
        let _ = evaluate_candy_grants(&[w("s", WindowClass::Session, 100.0, "T")], &mut tier);
        let again = evaluate_candy_grants(&[w("s", WindowClass::Session, 100.0, "T")], &mut tier);
        assert!(again.is_empty());
    }

    #[test]
    fn candy_rearm_after_drop_below_100() {
        let mut tier = HashMap::new();
        let _ = evaluate_candy_grants(&[w("s", WindowClass::Session, 100.0, "T")], &mut tier);
        let _ = evaluate_candy_grants(&[w("s", WindowClass::Session, 40.0, "T")], &mut tier);
        assert_eq!(tier.get("s"), None);
        let regrant = evaluate_candy_grants(&[w("s", WindowClass::Session, 100.0, "T")], &mut tier);
        assert_eq!(regrant.iter().map(|x| x.count).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn candy_mixed_windows() {
        let mut tier = HashMap::new();
        let g = evaluate_candy_grants(
            &[
                w("claude.fiveHour", WindowClass::Session, 100.0, "T"),
                w("claude.sevenDay", WindowClass::Weekly, 100.0, "T"),
                w("codex.primary", WindowClass::Session, 50.0, "T"),
            ],
            &mut tier,
        );
        let total: i64 = g.iter().map(|x| x.count).sum();
        assert_eq!(total, 1 + rare_candy::WEEKLY_GRANT);
    }

    #[test]
    fn candy_grant_carries_window_name() {
        let mut tier = HashMap::new();
        let g = evaluate_candy_grants(
            &[w("claude.fiveHour", WindowClass::Session, 100.0, "Claude 5시간 세션")],
            &mut tier,
        );
        assert_eq!(g.first().map(|x| x.window_name.as_str()), Some("Claude 5시간 세션"));
    }

    // ── 원본 ShinyCharmTests ──
    #[test]
    fn rolls_shiny_flips_with_charm() {
        assert!(rolls_shiny(48, true));
        assert!(!rolls_shiny(48, false));
        assert!(rolls_shiny(64, false));
        assert!(!rolls_shiny(64, true));
        assert!(rolls_shiny(96, true));
        assert!(!rolls_shiny(96, false));
        assert!(rolls_shiny(0, true));
        assert!(rolls_shiny(0, false));
        assert!(!rolls_shiny(1, true));
        assert!(!rolls_shiny(1, false));
    }

    #[test]
    fn shiny_charm_constants_and_passive() {
        assert_eq!(shiny_charm::PRICE, 3_000_000_000);
        assert_eq!(shiny_charm::SHINY_DENOMINATOR, 48);
        assert!(ItemKind::ShinyCharm.is_passive());
        assert!(!ItemKind::RareCandy.is_passive());
        assert!(!ItemKind::Mint.is_passive());
    }

    // ── 원본 DittoTests ──
    #[test]
    fn ditto_disguise_hit_cases() {
        assert!(ditto_disguise_hit(Rarity::Common, 2, 0));
        assert!(ditto_disguise_hit(Rarity::Common, 3, 128));
        assert!(ditto_disguise_hit(Rarity::Common, 3, 256));
        assert!(!ditto_disguise_hit(Rarity::Common, 3, 1));
        assert!(!ditto_disguise_hit(Rarity::Common, 3, 127));
        assert!(!ditto_disguise_hit(Rarity::Common, 3, 129));
        assert!(!ditto_disguise_hit(Rarity::Common, 1, 0)); // 단일 형태 제외
        assert!(!ditto_disguise_hit(Rarity::Uncommon, 3, 0)); // 비커먼 제외
        assert!(!ditto_disguise_hit(Rarity::Rare, 3, 0));
        assert!(!ditto_disguise_hit(Rarity::Legendary, 3, 0));
        assert_eq!(pokemon_odds::DITTO_DISGUISE_DENOMINATOR, 128);
    }

    // ── 원본 ShopTests.testShopEntriesInterleavesFreshEggByPrice (가격 접근자) ──
    #[test]
    fn shop_entry_prices() {
        assert_eq!(ShopEntry::Item(ItemKind::Mint).price(), 100_000_000);
        assert_eq!(ShopEntry::Item(ItemKind::RareCandy).price(), 500_000_000);
        assert_eq!(ShopEntry::FreshEgg.price(), 1_000_000_000);
        assert_eq!(ShopEntry::Item(ItemKind::ShinyCharm).price(), 3_000_000_000);
    }

    // ── 원본 ModelLogicTests.StatePersistenceLogicTests ──
    #[test]
    fn mon_state_current_id_clamps_to_path() {
        let m = MonState {
            base_id: 1, path_ids: vec![1, 2, 3], stage_index: 1, used_at_stage: 0,
            rarity: Rarity::Common, total_forms: 3, is_shiny: false, nature: None,
            ditto_disguise: None, ditto_revealed: false,
        };
        assert_eq!(m.current_id(), 2);
        let over = MonState {
            base_id: 1, path_ids: vec![1], stage_index: 5, used_at_stage: 0,
            rarity: Rarity::Common, total_forms: 1, is_shiny: false, nature: None,
            ditto_disguise: None, ditto_revealed: false,
        };
        assert_eq!(over.current_id(), 1);
    }

    #[test]
    fn companion_state_encode_decode_round_trip() {
        let mut st = CompanionState::default();
        st.install_baseline_set = true;
        st.used_since_install = 42;
        st.egg_usage = 1234;
        st.claimed_today_tokens = 7;
        st.last_date = "2026-06-27".to_string();
        st.collected_finals = ["1:3".to_string(), "10:12".to_string()].into_iter().collect();
        st.language = AppLanguage::Ja;
        st.dex = vec![DexEntry {
            id: String::new(), base_id: 1, final_id: 3, chain_order: vec![1, 2, 3],
            rarity: Rarity::Rare, caught_at: None, is_shiny: false, nature: None, names: None,
        }];

        let data = serde_json::to_string(&st).unwrap();
        let back: CompanionState = serde_json::from_str(&data).unwrap();

        assert_eq!(back.install_baseline_set, true);
        assert_eq!(back.used_since_install, 42);
        assert_eq!(back.egg_usage, 1234);
        assert_eq!(back.last_date, "2026-06-27");
        assert_eq!(back.collected_finals, st.collected_finals);
        assert_eq!(back.language, AppLanguage::Ja);
        assert_eq!(back.dex.len(), 1);
        assert_eq!(back.dex[0].chain_order, vec![1, 2, 3]);
    }
}
