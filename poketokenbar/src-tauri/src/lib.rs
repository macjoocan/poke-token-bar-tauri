// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
pub mod core;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

// NOTE: 모듈명 `core` 는 std 의 `core` 크레이트와 충돌하므로 항상 `crate::core` 로 명시.
use crate::core::companion_model::AdventureLogEntry;
use crate::core::companion_store::{CompanionStore, SeededRng};
use crate::core::local_usage_reader as reader;
use crate::core::models::{self, UsageSnapshot};
use crate::core::oauth_limits::{self, ClaudeLimits};
use crate::core::poke_api::PokeApiClient;
use crate::core::token_formatter;
use crate::core::usage_store;

/// 앱 설정(영속 — `settings.json`). 자동시작은 OS(레지스트리)에서 직접 질의하므로 여기 저장 안 함.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppSettings {
    /// 한도 경고 토스트 알림 on/off.
    #[serde(default = "default_true")]
    limit_notifications: bool,
    /// 경고선(%) — 원본 기본 80.
    #[serde(default = "default_warn")]
    warn_threshold: f64,
    /// 위험선(%) — 원본 기본 95.
    #[serde(default = "default_crit")]
    crit_threshold: f64,
}
fn default_true() -> bool {
    true
}
fn default_warn() -> f64 {
    80.0
}
fn default_crit() -> f64 {
    95.0
}
impl Default for AppSettings {
    fn default() -> Self {
        AppSettings { limit_notifications: true, warn_threshold: 80.0, crit_threshold: 95.0 }
    }
}

fn load_settings(path: &Path) -> AppSettings {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn save_settings(path: &Path, s: &AppSettings) {
    if let Ok(json) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(path, json);
    }
}

/// 앱 전역 상태 — 컴패니언 스토어 + 설정 + 한도 알림 tier(엣지 트리거 영속 상태).
struct AppState {
    companion: Mutex<CompanionStore>,
    settings: Mutex<AppSettings>,
    settings_path: PathBuf,
    /// `usage_store::evaluate_limit_alerts` 의 창별 직전 tier(0/1/2). 세션 내 유지.
    notified_tier: Mutex<HashMap<String, i32>>,
}

/// 프런트 전달용 컴패니언 뷰.
#[derive(Debug, Clone, Serialize)]
struct CompanionView {
    display_state: String,
    is_egg: bool,
    egg_progress: f64,
    egg_tokens_to_hatch: i64,
    display_name: String,
    species_id: Option<i64>,
    is_shiny: bool,
    stage_index: i64,
    total_forms: i64,
    progress: f64,
    tokens_to_next: i64,
    rarity: Option<String>,
    dex_count: usize,
    // M3: 상점/아이템 상호작용용 파생값.
    nature: Option<String>,
    available_tokens: i64,
    rare_candy: i64,
    mint_count: i64,
    owns_shiny_charm: bool,
    can_use_candy: bool,
    can_use_mint: bool,
    /// 이번 refresh 에서 방금 발생한 모험 이벤트(말풍선용). 없으면 None.
    latest_event: Option<AdventureLogEntry>,
    /// 진행 중 야생 전투 존재 여부(홈 배너 노출용).
    has_pending_battle: bool,
    /// 몬스터볼 포획 성공 누계(통계).
    caught_count: i64,
    /// 볼 보유 수(가방 표시 — 전투 시 자동 사용).
    great_ball_count: i64,
    ultra_ball_count: i64,
    /// 장난감 보유 수(광장 놀아주기).
    toy_ball_count: i64,
    toy_balloon_count: i64,
    /// 보유한 광장 꾸미기 오브젝트(raw_value 목록).
    decorations: Vec<String>,
}

/// 진화 체인 한 단계(도감 상세 패널용).
#[derive(Debug, Clone, Serialize)]
struct ChainStepView {
    id: i64,
    name: String,
}

/// 도감 한 항목(프런트 전달).
#[derive(Debug, Clone, Serialize)]
struct DexView {
    final_id: i64,
    name: String,
    rarity: String,
    is_shiny: bool,
    nature: Option<String>,
    /// 획득일(YYYY-MM-DD). 구버전 저장 데이터엔 없을 수 있음.
    caught_at: Option<String>,
    /// 진화 체인(초기→최종) — 저장된 체인명을 현재 언어로 해석(없으면 #id).
    chain: Vec<ChainStepView>,
}

/// 상점 한 줄(프런트 전달).
#[derive(Debug, Clone, Serialize)]
struct ShopItemView {
    /// 구매 대상 식별자 — "rareCandy"/"mint"/"shinyCharm"/"freshEgg".
    id: String,
    name: String,
    price: i64,
    affordable: bool,
    /// 보유형(패시브) 이미 구매 완료 → 비활성.
    done: bool,
    emoji: String,
}

/// CompanionStore 로부터 현재 CompanionView 를 구성(잠금 보유 상태에서 호출).
fn build_companion_view(store: &CompanionStore) -> CompanionView {
    let active = store.state.active.clone();
    CompanionView {
        display_state: store.display_state_str().to_string(),
        is_egg: store.is_egg(),
        egg_progress: store.egg_progress(),
        egg_tokens_to_hatch: store.egg_tokens_to_hatch(),
        display_name: store.display_name(),
        species_id: store.current_species_id(),
        is_shiny: store.current_is_shiny(),
        stage_index: active.as_ref().map(|a| a.stage_index).unwrap_or(0),
        total_forms: active.as_ref().map(|a| a.total_forms).unwrap_or(0),
        progress: store.progress(),
        tokens_to_next: store.tokens_to_next(),
        rarity: active.as_ref().map(|a| format!("{:?}", a.rarity).to_lowercase()),
        dex_count: store.state.dex.len(),
        nature: store.current_nature().map(|n| format!("{:?}", n).to_lowercase()),
        available_tokens: store.available_tokens(),
        rare_candy: store.rare_candy_count(),
        mint_count: store.item_count(crate::core::companion_model::ItemKind::Mint),
        owns_shiny_charm: store.owns_shiny_charm(),
        can_use_candy: store.can_use_rare_candy(),
        can_use_mint: store.can_use_mint(),
        latest_event: None, // refresh_companion 이 트리거 시 채움
        has_pending_battle: store.battle().is_some(),
        caught_count: store.state.caught_count,
        great_ball_count: store.item_count(crate::core::companion_model::ItemKind::GreatBall),
        ultra_ball_count: store.item_count(crate::core::companion_model::ItemKind::UltraBall),
        toy_ball_count: store.item_count(crate::core::companion_model::ItemKind::ToyBall),
        toy_balloon_count: store.item_count(crate::core::companion_model::ItemKind::ToyBalloon),
        decorations: crate::core::companion_model::ItemKind::DECOS
            .iter()
            .filter(|k| store.item_count(**k) > 0)
            .map(|k| k.raw_value().to_string())
            .collect(),
    }
}

/// 사용량으로 컴패니언을 구동하고 현재 뷰를 반환. 부화 임계 도달 시 PokéAPI 로 부화(네트워크).
///
/// async 커맨드 + `spawn_blocking`: 디스크 스캔(all_entries)·부화 네트워크(hatch_if_needed)는
/// 블로킹 I/O 라 전용 블로킹 풀에서 돌린다. 이렇게 하면 메인/async 런타임 스레드를 막지 않아
/// 부화 중에도 UI(트레이·팝오버)가 응답한다. State 는 async 경계를 못 넘으므로 AppHandle 로 접근.
#[tauri::command]
async fn refresh_companion(app: tauri::AppHandle) -> CompanionView {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut store = state.companion.lock().unwrap();

        let since = SystemTime::now().checked_sub(Duration::from_secs(2 * 24 * 3600));
        let entries = reader::all_entries(since);
        let today_key = models::today_key();
        let daily = reader::daily(&entries, &today_key);
        let block = reader::active_block(&entries, chrono::Utc::now());
        let today_tokens = daily.as_ref().map(|d| d.total_tokens).unwrap_or(0);
        let tpm = block.as_ref().and_then(|b| b.tokens_per_minute).unwrap_or(0.0);
        let has_usage = !entries.is_empty();

        store.update(today_tokens, &today_key, 0, usage_store::burn_tier(tpm), false, has_usage);
        store.ensure_line_loaded();
        store.hatch_if_needed();
        // 토큰 사용에 연동된 모험 이벤트(간격 도달 시 1회). 발생하면 말풍선용으로 반환.
        let fired = store.maybe_trigger_adventure().is_some();

        let mut view = build_companion_view(&store);
        if fired {
            view.latest_event = store.adventure_log().last().cloned();
        }
        view
    })
    .await
    .expect("refresh_companion blocking task panicked")
}

/// 전투 화면용 뷰 — 야생 + 파트너 스냅샷.
#[derive(Debug, Clone, Serialize)]
struct BattleView {
    wild_id: i64,
    wild_level: i64,
    wild_hp: i64,
    wild_max_hp: i64,
    wild_shiny: bool,
    ally_species_id: Option<i64>,
    ally_shiny: bool,
    ally_name: String,
    ally_level: i64,
    ally_hp: i64,
    ally_max_hp: i64,
}

fn build_battle_view(store: &CompanionStore, b: &crate::core::battle::BattleState) -> BattleView {
    BattleView {
        wild_id: b.wild_id,
        wild_level: b.wild_level,
        wild_hp: b.wild_hp,
        wild_max_hp: b.wild_max_hp,
        wild_shiny: b.wild_shiny,
        ally_species_id: store.current_species_id(),
        ally_shiny: store.current_is_shiny(),
        ally_name: store.display_name(),
        ally_level: b.ally_level,
        ally_hp: b.ally_hp,
        ally_max_hp: b.ally_max_hp,
    }
}

/// 진행 중 전투 조회(전투 화면 진입 시). 없으면 None.
#[tauri::command]
async fn get_battle(app: tauri::AppHandle) -> Option<BattleView> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let store = state.companion.lock().unwrap();
        store.battle().map(|b| build_battle_view(&store, b))
    })
    .await
    .unwrap_or(None)
}

/// 전투 한 턴 결과(프런트 연출 시퀀스용).
#[derive(Debug, Clone, Serialize)]
struct BattleTurnView {
    outcome: crate::core::battle::TurnOutcome,
    /// 턴 반영 후 상태(종료된 전투도 최종 HP 를 담음).
    state: BattleView,
    /// "ongoing" | "win" | "lose"
    result: String,
    /// 승리 보상 사탕 수(승리 시만 > 0).
    reward_qty: i64,
    /// 승리 시 몬스터볼 포획 성공 여부(승리가 아니면 None).
    caught: Option<bool>,
    /// 포획에 사용한 볼("greatBall"/"ultraBall", None=기본 몬스터볼).
    ball_used: Option<String>,
    /// 포획 성공 시 그 종의 희귀도(lowercase) — 전설 특별 연출용.
    caught_rarity: Option<String>,
}

/// 전투: 싸우다. `wild_name` 은 프런트가 해석한 야생 이름(모험 로그 문구용).
#[tauri::command]
async fn battle_attack(app: tauri::AppHandle, wild_name: String) -> Option<BattleTurnView> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut store = state.companion.lock().unwrap();
        let r = store.battle_attack(&wild_name)?;
        let result = if r.outcome.wild_fainted {
            "win"
        } else if r.outcome.ally_fainted {
            "lose"
        } else {
            "ongoing"
        };
        Some(BattleTurnView {
            state: build_battle_view(&store, &r.state),
            outcome: r.outcome,
            result: result.to_string(),
            reward_qty: r.reward_qty,
            caught: r.caught,
            ball_used: r.ball_used.map(str::to_string),
            caught_rarity: r.caught_rarity,
        })
    })
    .await
    .unwrap_or(None)
}

/// 전투: 도망(항상 성공).
#[tauri::command]
async fn battle_flee(app: tauri::AppHandle) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut store = state.companion.lock().unwrap();
        store.battle_flee();
    })
    .await;
}

/// 모험 로그(최신 우선) — 프런트 로그 패널용.
#[tauri::command]
async fn get_adventure_log(app: tauri::AppHandle) -> Vec<AdventureLogEntry> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let store = state.companion.lock().unwrap();
        let mut log = store.adventure_log().to_vec();
        log.reverse(); // 최신이 위로
        log
    })
    .await
    .unwrap_or_default()
}

/// 도감 목록 — 졸업(최종체 수집) 순. 이름은 저장된 체인명을 현재 언어로 해석(없으면 #id).
#[tauri::command]
async fn get_dex(app: tauri::AppHandle) -> Vec<DexView> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let store = state.companion.lock().unwrap();
        store
            .state
            .dex
            .iter()
            .map(|e| {
                let names = store.dex_stored_chain_names(e);
                let resolve = |id: i64| {
                    names
                        .as_ref()
                        .and_then(|n| n.get(&id).cloned())
                        .unwrap_or_else(|| format!("#{}", id))
                };
                DexView {
                    final_id: e.final_id,
                    name: resolve(e.final_id),
                    rarity: format!("{:?}", e.rarity).to_lowercase(),
                    is_shiny: e.is_shiny,
                    nature: e.nature.map(|n| format!("{:?}", n).to_lowercase()),
                    caught_at: e.caught_at.map(|t| t.format("%Y-%m-%d").to_string()),
                    chain: e
                        .chain_order
                        .iter()
                        .map(|&id| ChainStepView { id, name: resolve(id) })
                        .collect(),
                }
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// 상점 목록 — 구매 가능 아이템 + (컴패니언 있으면) 새 알 리롤.
#[tauri::command]
async fn get_shop(app: tauri::AppHandle) -> Vec<ShopItemView> {
    use crate::core::companion_model::{ItemKind, ShopEntry};
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let store = state.companion.lock().unwrap();
        store
            .shop_entries()
            .into_iter()
            .map(|entry| match entry {
                ShopEntry::Item(kind) => {
                    let (id, name, emoji) = match kind {
                        ItemKind::RareCandy => ("rareCandy", "이상한 사탕", "🍬"),
                        ItemKind::Mint => ("mint", "민트", "🌿"),
                        ItemKind::ShinyCharm => ("shinyCharm", "빛나는 부적", "✨"),
                        ItemKind::GreatBall => ("greatBall", "수퍼볼", "🔵"),
                        ItemKind::UltraBall => ("ultraBall", "하이퍼볼", "🟡"),
                        ItemKind::ToyBall => ("toyBall", "장난감 공", "⚽"),
                        ItemKind::ToyBalloon => ("toyBalloon", "풍선", "🎈"),
                        ItemKind::DecoSign => ("decoSign", "표지판", "🪧"),
                        ItemKind::DecoFlowerbed => ("decoFlowerbed", "화단", "🌷"),
                        ItemKind::DecoBench => ("decoBench", "벤치", "🪑"),
                        ItemKind::DecoFountain => ("decoFountain", "분수", "⛲"),
                    };
                    let done = kind.is_passive() && store.item_count(kind) > 0;
                    ShopItemView {
                        id: id.to_string(),
                        name: name.to_string(),
                        price: kind.shop_price().unwrap_or(0),
                        affordable: store.can_buy(kind),
                        done,
                        emoji: emoji.to_string(),
                    }
                }
                ShopEntry::FreshEgg => ShopItemView {
                    id: "freshEgg".to_string(),
                    name: "새 알".to_string(),
                    price: entry.price(),
                    affordable: store.can_buy_fresh_egg(),
                    done: false,
                    emoji: "🥚".to_string(),
                },
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// 상점 구매 — id 로 대상 해석. 성공 시 갱신된 CompanionView 반환(실패 시 현재값).
#[tauri::command]
async fn buy_item(app: tauri::AppHandle, id: String) -> CompanionView {
    use crate::core::companion_model::ItemKind;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut store = state.companion.lock().unwrap();
        match id.as_str() {
            "rareCandy" => {
                store.buy(ItemKind::RareCandy);
            }
            "mint" => {
                store.buy(ItemKind::Mint);
            }
            "shinyCharm" => {
                store.buy(ItemKind::ShinyCharm);
            }
            "greatBall" => {
                store.buy(ItemKind::GreatBall);
            }
            "ultraBall" => {
                store.buy(ItemKind::UltraBall);
            }
            "toyBall" => {
                store.buy(ItemKind::ToyBall);
            }
            "toyBalloon" => {
                store.buy(ItemKind::ToyBalloon);
            }
            "decoSign" => {
                store.buy(ItemKind::DecoSign);
            }
            "decoFlowerbed" => {
                store.buy(ItemKind::DecoFlowerbed);
            }
            "decoBench" => {
                store.buy(ItemKind::DecoBench);
            }
            "decoFountain" => {
                store.buy(ItemKind::DecoFountain);
            }
            "freshEgg" => {
                store.buy_fresh_egg();
            }
            _ => {}
        }
        build_companion_view(&store)
    })
    .await
    .expect("buy_item blocking task panicked")
}

/// 장난감 사용(광장 놀아주기) — 보유 시 1개 소모 후 갱신 뷰 반환, 없으면 None.
#[tauri::command]
async fn use_toy(app: tauri::AppHandle, kind: String) -> Option<CompanionView> {
    use crate::core::companion_model::ItemKind;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut store = state.companion.lock().unwrap();
        let k = match kind.as_str() {
            "toyBall" => ItemKind::ToyBall,
            "toyBalloon" => ItemKind::ToyBalloon,
            _ => return None,
        };
        if store.consume_item(k) {
            Some(build_companion_view(&store))
        } else {
            None
        }
    })
    .await
    .unwrap_or(None)
}

/// 이상한 사탕 사용 — 진화/졸업 촉진. 갱신된 CompanionView 반환.
#[tauri::command]
async fn use_rare_candy(app: tauri::AppHandle) -> CompanionView {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut store = state.companion.lock().unwrap();
        let _ = store.use_rare_candy();
        store.consume_candy_feedback();
        build_companion_view(&store)
    })
    .await
    .expect("use_rare_candy blocking task panicked")
}

/// 민트 사용 — 성격 변경. 갱신된 CompanionView 반환.
#[tauri::command]
async fn use_mint(app: tauri::AppHandle) -> CompanionView {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut store = state.companion.lock().unwrap();
        let _ = store.use_mint();
        store.consume_mint_feedback();
        build_companion_view(&store)
    })
    .await
    .expect("use_mint blocking task panicked")
}

/// 창 높이를 콘텐츠에 맞춰 동적으로 설정(프런트가 측정한 px). 폭은 360 고정.
/// 모니터 논리 높이에서 여유(60)를 뺀 값으로 상한 → 화면 밖으로 안 나감. decorations:false 라
/// outer≈inner 여서 콘텐츠와 창이 정확히 맞는다.
#[tauri::command]
fn set_window_height(window: tauri::WebviewWindow, height: f64) {
    let mut h = height.max(240.0);
    match window.current_monitor() {
        Ok(Some(m)) => {
            let logical_h = m.size().height as f64 / m.scale_factor();
            h = h.min(logical_h - 60.0);
        }
        _ => h = h.min(1100.0),
    }
    let _ = window.set_size(tauri::LogicalSize::new(360.0_f64, h));
}

/// 저장 백업 파일 경로 — 문서 폴더의 `PokeTokenBar-save.json`(없으면 data_dir 폴백).
fn save_backup_path() -> Option<PathBuf> {
    dirs::document_dir()
        .or_else(dirs::data_dir)
        .map(|d| d.join("PokeTokenBar-save.json"))
}

/// 현재 진행도(도감·컴패니언·인벤토리)를 백업 파일로 내보내기. 성공 시 저장 경로 반환.
#[tauri::command]
fn export_save(state: tauri::State<AppState>) -> Option<String> {
    let dest = save_backup_path()?;
    let store = state.companion.lock().unwrap();
    if store.export_to(&dest) {
        Some(dest.to_string_lossy().to_string())
    } else {
        None
    }
}

/// 백업 파일에서 진행도 복원. 성공 여부 반환(파일 없음/손상 시 false, 현재 데이터 보존).
#[tauri::command]
fn import_save(state: tauri::State<AppState>) -> bool {
    let src = match save_backup_path() {
        Some(p) => p,
        None => return false,
    };
    let mut store = state.companion.lock().unwrap();
    store.import_from(&src)
}

/// 앱 언어 조회 — "ko"/"en"/"ja".
#[tauri::command]
fn get_language(state: tauri::State<AppState>) -> String {
    let store = state.companion.lock().unwrap();
    format!("{:?}", store.language()).to_lowercase()
}

/// 앱 언어 설정.
#[tauri::command]
fn set_language(state: tauri::State<AppState>, lang: String) {
    use crate::core::companion_model::AppLanguage;
    let parsed = match lang.as_str() {
        "ko" => AppLanguage::Ko,
        "ja" => AppLanguage::Ja,
        _ => AppLanguage::En,
    };
    let mut store = state.companion.lock().unwrap();
    store.set_language(parsed);
}

/// 설정 스냅샷(프런트 전달) — 자동시작은 OS 레지스트리에서 실시간 질의.
#[derive(Debug, Clone, Serialize)]
struct SettingsView {
    autostart: bool,
    limit_notifications: bool,
    warn_threshold: f64,
    crit_threshold: f64,
}

/// 현재 설정 조회.
#[tauri::command]
fn get_settings(app: tauri::AppHandle, state: tauri::State<AppState>) -> SettingsView {
    let s = state.settings.lock().unwrap();
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    SettingsView {
        autostart,
        limit_notifications: s.limit_notifications,
        warn_threshold: s.warn_threshold,
        crit_threshold: s.crit_threshold,
    }
}

/// 자동시작(레지스트리 Run) on/off. 성공 여부 반환.
#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> bool {
    let mgr = app.autolaunch();
    let r = if enabled { mgr.enable() } else { mgr.disable() };
    r.is_ok()
}

/// 알림/임계값 설정 저장. 임계값은 [0,100] 로 클램프하고 warn ≤ crit 보장.
#[tauri::command]
fn update_settings(state: tauri::State<AppState>, limit_notifications: bool, warn_threshold: f64, crit_threshold: f64) {
    let warn = warn_threshold.clamp(0.0, 100.0);
    let crit = crit_threshold.clamp(0.0, 100.0);
    let mut s = state.settings.lock().unwrap();
    s.limit_notifications = limit_notifications;
    s.warn_threshold = warn.min(crit);
    s.crit_threshold = crit.max(warn);
    save_settings(&state.settings_path, &s);
}

/// Claude 공식 한도를 조회해 **임계값을 새로 넘어선 창만** 토스트로 알린다(엣지 트리거).
/// task2 `usage_store::evaluate_limit_alerts` 순수판정 + AppState.notified_tier(창별 직전 tier).
/// 트레이 스레드에서 저빈도(120s)로 호출 — 네트워크·429 백오프 부담을 줄인다. 알림 꺼짐이면 no-op.
fn check_and_notify_limits(app: &tauri::AppHandle) {
    let state = match app.try_state::<AppState>() {
        Some(s) => s,
        None => return,
    };
    let (enabled, warn, crit) = {
        let s = state.settings.lock().unwrap();
        (s.limit_notifications, s.warn_threshold, s.crit_threshold)
    };
    if !enabled {
        return;
    }
    let limits = match oauth_limits::fetch_limits() {
        Ok(l) => l,
        Err(_) => return, // 자격증명 없음/네트워크 실패 → 조용히 무시(원본 정책)
    };
    let windows = usage_store::claude_alert_windows(&limits);
    let alerts = {
        let mut tiers = state.notified_tier.lock().unwrap();
        usage_store::evaluate_limit_alerts(&windows, warn, crit, &mut tiers)
    };
    for a in alerts {
        let title = if a.is_critical { "한도 위험 ⚠️" } else { "한도 경고" };
        let body = format!("{} {:.0}% 도달", a.window, a.utilization);
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

/// Claude 공식 한도(%) — 팀/Pro/Max 구독 계정의 5시간·주간 게이지.
/// 자격증명 없음/만료/네트워크 실패 시 None(팝오버는 한도 섹션만 숨김).
#[tauri::command]
fn get_claude_limits() -> Option<ClaudeLimits> {
    oauth_limits::fetch_limits().ok()
}

/// Codex 공식 한도(%) — 로컬 rollout 로그의 최신 `rate_limits` 이벤트 파싱(네트워크 불필요).
/// Codex 미사용/데이터 없음 시 None(팝오버는 Codex 한도 섹션만 숨김).
#[tauri::command]
fn get_codex_limits() -> Option<crate::core::codex_rate_limits::CodexLimits> {
    // 최근 8일 mtime 윈도우 — 주간(7일) 창을 담은 이벤트가 살아있도록 넉넉히.
    let since = SystemTime::now().checked_sub(Duration::from_secs(8 * 24 * 3600));
    crate::core::codex_rate_limits::read_codex_limits(since)
}

/// 프런트가 호출하는 "오늘 사용량" 커맨드.
/// 최근 2일 mtime 윈도우로 스캔(오늘 일자 + 5h 활성 블록에 충분) 후 전 프로바이더 통합 집계.
#[tauri::command]
fn get_today_usage() -> UsageSnapshot {
    let since = SystemTime::now().checked_sub(Duration::from_secs(2 * 24 * 3600));
    let entries = reader::all_entries(since);
    let today = models::today_key();
    let now = chrono::Utc::now();
    UsageSnapshot {
        today: reader::daily(&entries, &today),
        block: reader::active_block(&entries, now),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// M4 트레이 PoC — 동적 아이콘(타이머 갱신) + 툴팁 + 클릭 팝오버
//
// 검증 목표: ① 트레이 아이콘 이미지를 타이머로 교체해 스프라이트 애니메이션 가능한가
//            ② 사용량 숫자를 툴팁으로 노출 가능한가  ③ 클릭으로 팝오버 창 토글 가능한가
// (macOS 메뉴바의 "아이콘+텍스트"를 Windows 트레이에서 "동적 아이콘+툴팁"으로 대체하는 방식.)
// ─────────────────────────────────────────────────────────────────────────

const ICON_SIZE: u32 = 32;

/// 프레임별 트레이 아이콘 생성(raw RGBA) — PoC 애니메이션.
/// 배경 원 + 프레임에 따라 시계방향으로 차오르는 "게이지"(진행/활동 표현) + 프레임마다 도는 색상.
fn make_frame(frame: u32, fill: f32) -> Image<'static> {
    let size = ICON_SIZE as i32;
    let mut rgba = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    let cx = (size - 1) as f32 / 2.0;
    let cy = cx;
    let radius = cx - 1.0;

    // 프레임에 따라 도는 hue(0~1) → 간단한 HSV→RGB.
    let hue = (frame % 60) as f32 / 60.0;
    let (r, g, b) = hsv_to_rgb(hue, 0.65, 0.95);
    // 게이지 각도(위 12시부터 시계방향). fill 0~1.
    let fill_angle = fill.clamp(0.0, 1.0) * std::f32::consts::TAU;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = ((y * size + x) * 4) as usize;
            if dist > radius {
                continue; // 원 밖 → 투명
            }
            // 12시 기준 시계방향 각도 0~TAU
            let mut ang = (dx).atan2(-dy); // 위쪽이 0, 시계방향 증가
            if ang < 0.0 {
                ang += std::f32::consts::TAU;
            }
            let lit = ang <= fill_angle;
            let inner = dist < radius * 0.55;
            let (pr, pg, pb, pa) = if inner {
                (245, 245, 245, 255) // 중앙 코어(밝게)
            } else if lit {
                (r, g, b, 255) // 채워진 게이지
            } else {
                (60, 60, 70, 200) // 빈 게이지(어둡게)
            };
            rgba[idx] = pr;
            rgba[idx + 1] = pg;
            rgba[idx + 2] = pb;
            rgba[idx + 3] = pa;
        }
    }
    Image::new_owned(rgba, ICON_SIZE, ICON_SIZE)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32) % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// 오늘 사용량 → 툴팁 문자열. (트레이 hover 시 노출 — macOS 메뉴바 텍스트 대체)
fn tooltip_text(snap: &UsageSnapshot) -> String {
    match &snap.today {
        Some(d) => format!(
            "PokeTokenBar\n오늘 {} · {}",
            token_formatter::compact(d.total_tokens),
            token_formatter::cost(d.total_cost)
        ),
        None => "PokeTokenBar\n오늘 사용량 없음".to_string(),
    }
}

/// 트레이용 실제 펫 스프라이트(PokéAPI PNG) → Tauri `Image`. 디스크 캐시(`sprites/{id}[_shiny].png`).
/// 이로치 스프라이트가 없으면 일반으로 폴백. 네트워크/디코드 실패 시 None(호출부가 이전 아이콘 유지).
/// `Image::from_bytes` 는 `image-png` 피처로 PNG 를 디코드 → RGBA 를 소유 버퍼로 복사해 `'static` 보장.
fn fetch_sprite_image(data_dir: &std::path::Path, species_id: i64, shiny: bool) -> Option<Image<'static>> {
    let to_owned = |bytes: &[u8]| -> Option<Image<'static>> {
        let decoded = Image::from_bytes(bytes).ok()?;
        Some(Image::new_owned(decoded.rgba().to_vec(), decoded.width(), decoded.height()))
    };

    let dir = data_dir.join("sprites");
    let _ = std::fs::create_dir_all(&dir);
    let name = if shiny { format!("{}_shiny.png", species_id) } else { format!("{}.png", species_id) };
    let path = dir.join(&name);

    // 디스크 캐시 히트.
    if let Ok(bytes) = std::fs::read(&path) {
        if let Some(img) = to_owned(&bytes) {
            return Some(img);
        }
    }

    // 네트워크 fetch.
    let url = format!(
        "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/{}{}.png",
        if shiny { "shiny/" } else { "" },
        species_id
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client.get(&url).send().ok()?;
    if !resp.status().is_success() {
        // 이로치 스프라이트가 없는 종 → 일반 스프라이트로 폴백.
        return if shiny { fetch_sprite_image(data_dir, species_id, false) } else { None };
    }
    let bytes = resp.bytes().ok()?;
    let _ = std::fs::write(&path, &bytes);
    to_owned(&bytes)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(Vec::<&str>::new()),
        ))
        .invoke_handler(tauri::generate_handler![
            get_today_usage,
            get_claude_limits,
            get_codex_limits,
            refresh_companion,
            get_dex,
            get_shop,
            buy_item,
            use_rare_candy,
            use_mint,
            use_toy,
            get_adventure_log,
            get_battle,
            battle_attack,
            battle_flee,
            set_window_height,
            export_save,
            import_save,
            get_language,
            set_language,
            get_settings,
            set_autostart,
            update_settings
        ])
        .setup(|app| {
            // 컴패니언 스토어 — PokéAPI 프로바이더 + app-data 영속.
            let data_dir = dirs::data_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("PokeTokenBar");
            let _ = std::fs::create_dir_all(&data_dir);
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let store = CompanionStore::new(
                Box::new(PokeApiClient::new(data_dir.clone())),
                Box::new(chrono::Utc::now),
                data_dir.join("companion-state.json"),
                Box::new(SeededRng::new(seed)),
                true, // is_bundled_app — 메타몽 위장 롤 활성
            );
            let settings_path = data_dir.join("settings.json");
            let settings = load_settings(&settings_path);
            app.manage(AppState {
                companion: Mutex::new(store),
                settings: Mutex::new(settings),
                settings_path,
                notified_tier: Mutex::new(HashMap::new()),
            });

            // 시작 시 창은 숨김(트레이 앱) — 클릭 시 팝오버처럼 표시.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.hide();
            }

            // 트레이 우클릭 컨텍스트 메뉴 — 열기 / 종료.
            // (decorations:false 라 창에 닫기 버튼이 없으므로 종료 경로를 트레이 메뉴로 제공.)
            let open_i = MenuItem::with_id(app, "open", "열기", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&open_i, &quit_i])?;

            let _tray = TrayIconBuilder::with_id("main")
                .icon(make_frame(0, 0.0))
                .tooltip("PokeTokenBar")
                .menu(&tray_menu)
                // 좌클릭은 메뉴 대신 팝오버 토글에 쓴다(메뉴는 우클릭에서만).
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            let visible = win.is_visible().unwrap_or(false);
                            if visible {
                                let _ = win.hide();
                            } else {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            // 트레이 아이콘 스레드 (M4: PoC 애니메이션 → 실제 펫 스프라이트).
            //
            // - 알 상태: `make_frame` 게이지로 **부화 진행도**를 애니메이션 표시(1s/프레임 — 저부하).
            // - 부화 후: 실제 펫 스프라이트(PokéAPI PNG)를 아이콘으로. 정적이라 (species, shiny)
            //   변경 시에만 set_icon — 매 틱 재대입은 WindowServer/유휴 CPU 부하라 diff-gate
            //   (CLAUDE.md "메뉴바 상태아이템 = idle CPU 저격수" 규칙의 Windows 판).
            // - 툴팁(오늘 사용량)은 5s 마다 갱신.
            let handle = app.handle().clone();
            let sprite_dir = data_dir.clone();
            std::thread::spawn(move || {
                let mut frame: u32 = 0;
                let mut is_egg = true;
                let mut egg_fill = 0.0f32;
                let mut species: Option<i64> = None;
                let mut shiny = false;
                // diff-gate: 마지막으로 set 한 펫 아이콘 키. 변경 시에만 재대입.
                let mut last_pet_key: Option<(i64, bool)> = None;
                let mut sprite_cache: std::collections::HashMap<(i64, bool), Image<'static>> = std::collections::HashMap::new();

                loop {
                    // 5s(=5프레임)마다 사용량 툴팁 + 컴패니언 상태 재조회.
                    if frame % 5 == 0 {
                        let snap = get_today_usage();
                        if let Some(tray) = handle.tray_by_id("main") {
                            let _ = tray.set_tooltip(Some(&tooltip_text(&snap)));
                        }
                        if let Some(state) = handle.try_state::<AppState>() {
                            if let Ok(store) = state.companion.lock() {
                                is_egg = store.is_egg();
                                egg_fill = store.egg_progress() as f32;
                                species = store.current_species_id();
                                shiny = store.current_is_shiny();
                            }
                        }
                    }
                    // 120s 마다 Claude 한도 조회 → 임계값 엣지 트리거 토스트 알림(M4).
                    // (첫 틱 frame=0 에 즉시 1회 — 부팅 직후 이미 위험선이면 바로 알림.)
                    if frame % 120 == 0 {
                        check_and_notify_limits(&handle);
                    }

                    if is_egg || species.is_none() {
                        // 알: 부화 진행도 게이지를 애니메이션(색상은 프레임마다 회전).
                        last_pet_key = None;
                        if let Some(tray) = handle.tray_by_id("main") {
                            let _ = tray.set_icon(Some(make_frame(frame, egg_fill)));
                        }
                    } else if let Some(id) = species {
                        // 펫: 스프라이트가 바뀔 때만 아이콘 재대입(diff-gate).
                        let key = (id, shiny);
                        if last_pet_key != Some(key) {
                            let img = sprite_cache.get(&key).cloned().or_else(|| {
                                fetch_sprite_image(&sprite_dir, id, shiny).inspect(|img| {
                                    sprite_cache.insert(key, img.clone());
                                })
                            });
                            if let Some(img) = img {
                                if let Some(tray) = handle.tray_by_id("main") {
                                    let _ = tray.set_icon(Some(img));
                                }
                                last_pet_key = Some(key);
                            }
                        }
                    }

                    frame = frame.wrapping_add(1);
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
