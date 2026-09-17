//! 야생 전투 — 모험 조우에서 시작되는 라이트 턴제 전투(원본에 없는 Windows 판 확장).
//!
//! 설계: 모험 이벤트 롤의 일부(`tuning::ENCOUNTER_PERCENT`)가 "야생 포켓몬 조우"로 대체된다.
//! 이 모듈은 **부수효과 없는 순수 로직**(생성·데미지·턴 진행·보상 판정)만 제공하고,
//! 영속(`pending_battle`)·보상 지급·로그 적재는 `CompanionStore` 가 담당한다.
//! `SeededRng` 로 결정적이라 테스트 가능하고, 네트워크가 필요 없다(스프라이트·이름
//! 해석은 프런트 몫).

use serde::{Deserialize, Serialize};

use super::companion_store::Rng;

/// 밸런스 테이블 — 전투 수치는 로직에 박지 않고 전부 여기에 모은다(시뮬 스윕용).
pub mod tuning {
    /// 야생 종 상한 — Gen V(히어로/트레이 애니 스프라이트 상한과 동일).
    pub const WILD_MAX_ID: i64 = 649;
    /// 모험 이벤트 중 전투 조우로 대체되는 비율(%). 진행 중 전투가 있으면 일반 이벤트.
    pub const ENCOUNTER_PERCENT: u64 = 25;
    /// 야생 이로치 확률(1/N) — 시각·로그 플레이버(도감 등록 아님).
    pub const WILD_SHINY_ODDS: u64 = 64;

    /// 최대 HP = HP_BASE + level × HP_PER_LEVEL. 데미지 공식과 맞물려 평균 4~6턴.
    pub const HP_BASE: i64 = 20;
    pub const HP_PER_LEVEL: i64 = 2;

    /// 데미지 = level/DMG_LEVEL_DIV + DMG_BASE + 변동(0..=level/DMG_VAR_DIV).
    pub const DMG_BASE: i64 = 4;
    pub const DMG_LEVEL_DIV: i64 = 2;
    pub const DMG_VAR_DIV: i64 = 4;
    /// 급소 확률(1/N)·배율(NUM/DEN) — 본가 1세대 근사.
    pub const CRIT_ODDS: u64 = 16;
    pub const CRIT_NUM: i64 = 3;
    pub const CRIT_DEN: i64 = 2;

    /// 파트너 레벨 = LEVEL_BASE + stage×LEVEL_PER_STAGE + dex×LEVEL_PER_DEX (MIN..=MAX).
    pub const LEVEL_BASE: i64 = 8;
    pub const LEVEL_PER_STAGE: i64 = 12;
    pub const LEVEL_PER_DEX: i64 = 2;
    pub const LEVEL_MIN: i64 = 5;
    pub const LEVEL_MAX: i64 = 100;
    /// 야생 레벨 = 파트너 ± SPREAD (WILD_LEVEL_MIN 하한).
    pub const WILD_LEVEL_SPREAD: i64 = 4;
    pub const WILD_LEVEL_MIN: i64 = 2;

    /// 승리 보상(이상한 사탕): 기본 + 레벨 열세 승리 보너스 + 이로치 보너스.
    pub const REWARD_BASE: i64 = 1;
    pub const REWARD_UNDERDOG_BONUS: i64 = 1;
    pub const REWARD_SHINY_BONUS: i64 = 1;

    /// 승리 후 몬스터볼 포획 확률(%). 야생이 레벨 우위였으면 페널티.
    pub const CATCH_BASE_PERCENT: u64 = 50;
    pub const CATCH_UNDERDOG_PENALTY: u64 = 15;
    /// 볼 등급 보너스(%p) — 수퍼볼/하이퍼볼(1회용, 좋은 볼부터 자동 사용).
    pub const CATCH_GREAT_BONUS: u64 = 15;
    pub const CATCH_ULTRA_BONUS: u64 = 30;
    /// 포획 확률 상한(%) — 확정 포획은 없다.
    pub const CATCH_MAX_PERCENT: u64 = 90;
}

/// 진행 중 전투 상태(영속 — companion-state.json 의 `pending_battle`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BattleState {
    pub wild_id: i64,
    pub wild_level: i64,
    pub wild_hp: i64,
    pub wild_max_hp: i64,
    #[serde(default)]
    pub wild_shiny: bool,
    pub ally_level: i64,
    pub ally_hp: i64,
    pub ally_max_hp: i64,
    #[serde(default)]
    pub turn: i64,
}

/// 한 턴 결과(프런트 연출용 — HP 반영은 `BattleState` 쪽).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnOutcome {
    pub ally_dmg: i64,
    pub ally_crit: bool,
    pub wild_fainted: bool,
    /// 반격 데미지(야생이 살아있을 때만 > 0).
    pub wild_dmg: i64,
    pub wild_crit: bool,
    pub ally_fainted: bool,
}

/// 레벨 → 최대 HP.
pub fn max_hp(level: i64) -> i64 {
    tuning::HP_BASE + level * tuning::HP_PER_LEVEL
}

/// 파트너 레벨 — 별도 경험치 없이 진화 단계·도감 수집 수에서 파생.
pub fn ally_level(stage_index: i64, dex_count: usize) -> i64 {
    (tuning::LEVEL_BASE + stage_index * tuning::LEVEL_PER_STAGE + dex_count as i64 * tuning::LEVEL_PER_DEX)
        .clamp(tuning::LEVEL_MIN, tuning::LEVEL_MAX)
}

/// 새 전투 생성 — 야생 종(1..=WILD_MAX_ID)·레벨(파트너 ±SPREAD)·이로치 롤. rng 3회 소비.
pub fn new_battle(rng: &mut dyn Rng, ally_level: i64) -> BattleState {
    let spread_span = (tuning::WILD_LEVEL_SPREAD * 2 + 1) as u64;
    let wild_id = (rng.next_u64() % tuning::WILD_MAX_ID as u64) as i64 + 1;
    let spread = (rng.next_u64() % spread_span) as i64 - tuning::WILD_LEVEL_SPREAD;
    let wild_level = (ally_level + spread).clamp(tuning::WILD_LEVEL_MIN, tuning::LEVEL_MAX);
    let wild_shiny = rng.next_u64() % tuning::WILD_SHINY_ODDS == 0;
    BattleState {
        wild_id,
        wild_level,
        wild_hp: max_hp(wild_level),
        wild_max_hp: max_hp(wild_level),
        wild_shiny,
        ally_level,
        ally_hp: max_hp(ally_level),
        ally_max_hp: max_hp(ally_level),
        turn: 0,
    }
}

/// 데미지 롤 — 레벨 비례 기본치 + 변동 + 급소. rng 2회 소비.
fn damage(rng: &mut dyn Rng, level: i64) -> (i64, bool) {
    let base = level / tuning::DMG_LEVEL_DIV + tuning::DMG_BASE;
    let var = (rng.next_u64() % (level / tuning::DMG_VAR_DIV + 1) as u64) as i64;
    let crit = rng.next_u64() % tuning::CRIT_ODDS == 0;
    let mut dmg = base + var;
    if crit {
        dmg = dmg * tuning::CRIT_NUM / tuning::CRIT_DEN;
    }
    (dmg.max(1), crit)
}

/// 플레이어 공격 턴 — 선공(플레이어 유리). 야생이 살아남으면 반격.
pub fn player_attack(rng: &mut dyn Rng, b: &mut BattleState) -> TurnOutcome {
    let (ally_dmg, ally_crit) = damage(rng, b.ally_level);
    b.wild_hp = (b.wild_hp - ally_dmg).max(0);
    let wild_fainted = b.wild_hp == 0;

    let (mut wild_dmg, mut wild_crit, mut ally_fainted) = (0, false, false);
    if !wild_fainted {
        let (d, c) = damage(rng, b.wild_level);
        wild_dmg = d;
        wild_crit = c;
        b.ally_hp = (b.ally_hp - d).max(0);
        ally_fainted = b.ally_hp == 0;
    }
    b.turn += 1;
    TurnOutcome { ally_dmg, ally_crit, wild_fainted, wild_dmg, wild_crit, ally_fainted }
}

/// 승리 보상(이상한 사탕 수).
pub fn reward_qty(b: &BattleState) -> i64 {
    tuning::REWARD_BASE
        + if b.wild_level > b.ally_level { tuning::REWARD_UNDERDOG_BONUS } else { 0 }
        + if b.wild_shiny { tuning::REWARD_SHINY_BONUS } else { 0 }
}

/// 포획 확률(%) — 승리한 전투 기준 + 볼 보너스(%p), 상한 캡.
pub fn catch_chance(b: &BattleState, ball_bonus: u64) -> u64 {
    let base = if b.wild_level > b.ally_level {
        tuning::CATCH_BASE_PERCENT.saturating_sub(tuning::CATCH_UNDERDOG_PENALTY)
    } else {
        tuning::CATCH_BASE_PERCENT
    };
    (base + ball_bonus).min(tuning::CATCH_MAX_PERCENT)
}

/// 포획 롤(결정적). rng 1회 소비.
pub fn roll_catch(rng: &mut dyn Rng, b: &BattleState, ball_bonus: u64) -> bool {
    rng.next_u64() % 100 < catch_chance(b, ball_bonus)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::companion_store::SeededRng;

    #[test]
    fn new_battle_is_deterministic_and_in_bounds() {
        let mut a = SeededRng::new(42);
        let mut b = SeededRng::new(42);
        for _ in 0..200 {
            let x = new_battle(&mut a, 30);
            let y = new_battle(&mut b, 30);
            assert_eq!(x, y);
            assert!((1..=tuning::WILD_MAX_ID).contains(&x.wild_id));
            assert!(
                (30 - tuning::WILD_LEVEL_SPREAD..=30 + tuning::WILD_LEVEL_SPREAD)
                    .contains(&x.wild_level),
                "레벨 스프레드 범위 밖: {}",
                x.wild_level
            );
            assert_eq!(x.wild_hp, x.wild_max_hp);
            assert_eq!(x.ally_hp, max_hp(30));
            assert_eq!(x.turn, 0);
        }
    }

    #[test]
    fn wild_level_clamped_at_low_ally_level() {
        let mut rng = SeededRng::new(7);
        for _ in 0..200 {
            let b = new_battle(&mut rng, tuning::LEVEL_MIN);
            assert!(b.wild_level >= tuning::WILD_LEVEL_MIN, "야생 레벨 하한 위반: {}", b.wild_level);
        }
    }

    #[test]
    fn ally_level_derivation_and_clamp() {
        assert_eq!(ally_level(0, 0), tuning::LEVEL_BASE);
        assert_eq!(
            ally_level(1, 3),
            tuning::LEVEL_BASE + tuning::LEVEL_PER_STAGE + 3 * tuning::LEVEL_PER_DEX
        );
        assert_eq!(ally_level(100, 100), tuning::LEVEL_MAX); // 상한
    }

    #[test]
    fn damage_positive_and_crit_bounded() {
        let mut rng = SeededRng::new(99);
        let mut crits = 0;
        let n = 4000;
        // L=20: 비급소 최대 = 10+4+5 = 19, 급소 최대 = 19*3/2 = 28
        let max_noncrit = 20 / tuning::DMG_LEVEL_DIV + tuning::DMG_BASE + 20 / tuning::DMG_VAR_DIV;
        let max_crit = max_noncrit * tuning::CRIT_NUM / tuning::CRIT_DEN;
        for _ in 0..n {
            let (d, crit) = damage(&mut rng, 20);
            assert!(d >= 1);
            assert!(d <= max_crit, "데미지 상한 초과: {d}");
            if crit {
                crits += 1;
            }
        }
        let ratio = crits as f64 / n as f64;
        assert!(ratio > 0.03 && ratio < 0.10, "급소 비율 이상(기대 ~6.25%): {ratio}");
    }

    #[test]
    fn battle_always_terminates_and_hp_never_negative() {
        let mut rng = SeededRng::new(123);
        for i in 0..100 {
            let mut b = new_battle(&mut rng, 10 + i % 60);
            let mut turns = 0;
            loop {
                let o = player_attack(&mut rng, &mut b);
                assert!(b.wild_hp >= 0 && b.ally_hp >= 0);
                turns += 1;
                if o.wild_fainted || o.ally_fainted {
                    // 동시 기절 없음: 야생이 먼저 기절하면 반격 안 함
                    assert!(!(o.wild_fainted && o.ally_fainted));
                    break;
                }
                assert!(turns < 50, "전투가 끝나지 않음");
            }
        }
    }

    #[test]
    fn player_wins_more_than_half_at_equal_footing() {
        // 선공 이점으로 승률이 50% 를 넘어야 한다(레벨 스프레드 ±4 포함).
        let mut rng = SeededRng::new(2026);
        let mut wins = 0;
        let n = 500;
        for _ in 0..n {
            let mut b = new_battle(&mut rng, 30);
            loop {
                let o = player_attack(&mut rng, &mut b);
                if o.wild_fainted {
                    wins += 1;
                    break;
                }
                if o.ally_fainted {
                    break;
                }
            }
        }
        let ratio = wins as f64 / n as f64;
        assert!(ratio > 0.5, "승률이 절반 이하: {ratio}");
        assert!(ratio < 0.95, "승률이 비현실적으로 높음: {ratio}");
    }

    #[test]
    fn reward_scales_with_underdog_and_shiny() {
        // 픽스처 수치는 임의(밸런스 아님) — reward_qty 는 레벨 비교·이로치만 본다.
        let base = BattleState {
            wild_id: 25, wild_level: 30, wild_hp: 0, wild_max_hp: 80, wild_shiny: false,
            ally_level: 30, ally_hp: 10, ally_max_hp: 80, turn: 5,
        };
        assert_eq!(reward_qty(&base), tuning::REWARD_BASE);
        assert_eq!(
            reward_qty(&BattleState { wild_level: 34, ..base.clone() }),
            tuning::REWARD_BASE + tuning::REWARD_UNDERDOG_BONUS
        );
        assert_eq!(
            reward_qty(&BattleState { wild_shiny: true, ..base.clone() }),
            tuning::REWARD_BASE + tuning::REWARD_SHINY_BONUS
        );
        assert_eq!(
            reward_qty(&BattleState { wild_level: 34, wild_shiny: true, ..base }),
            tuning::REWARD_BASE + tuning::REWARD_UNDERDOG_BONUS + tuning::REWARD_SHINY_BONUS
        );
    }

    #[test]
    fn catch_chance_penalized_when_wild_stronger() {
        let base = BattleState {
            wild_id: 25, wild_level: 30, wild_hp: 0, wild_max_hp: 80, wild_shiny: false,
            ally_level: 30, ally_hp: 10, ally_max_hp: 80, turn: 5,
        };
        assert_eq!(catch_chance(&base, 0), tuning::CATCH_BASE_PERCENT);
        assert_eq!(
            catch_chance(&BattleState { wild_level: 34, ..base }, 0),
            tuning::CATCH_BASE_PERCENT - tuning::CATCH_UNDERDOG_PENALTY
        );
    }

    #[test]
    fn catch_chance_ball_bonus_and_cap() {
        let base = BattleState {
            wild_id: 25, wild_level: 30, wild_hp: 0, wild_max_hp: 80, wild_shiny: false,
            ally_level: 30, ally_hp: 10, ally_max_hp: 80, turn: 5,
        };
        assert_eq!(
            catch_chance(&base, tuning::CATCH_GREAT_BONUS),
            tuning::CATCH_BASE_PERCENT + tuning::CATCH_GREAT_BONUS
        );
        assert_eq!(
            catch_chance(&base, tuning::CATCH_ULTRA_BONUS),
            tuning::CATCH_BASE_PERCENT + tuning::CATCH_ULTRA_BONUS
        );
        // 상한 캡 — 과도한 보너스여도 CATCH_MAX_PERCENT 를 못 넘는다.
        assert_eq!(catch_chance(&base, 100), tuning::CATCH_MAX_PERCENT);
    }

    #[test]
    fn roll_catch_ratio_near_chance() {
        let mut rng = SeededRng::new(777);
        let b = BattleState {
            wild_id: 1, wild_level: 20, wild_hp: 0, wild_max_hp: 60, wild_shiny: false,
            ally_level: 20, ally_hp: 30, ally_max_hp: 60, turn: 4,
        };
        let n = 4000;
        let hits = (0..n).filter(|_| roll_catch(&mut rng, &b, 0)).count();
        let ratio = hits as f64 / n as f64;
        let expect = tuning::CATCH_BASE_PERCENT as f64 / 100.0;
        assert!((ratio - expect).abs() < 0.05, "포획률 이상(기대 {expect}): {ratio}");
    }

    #[test]
    fn counterattack_only_when_wild_alive() {
        let mut rng = SeededRng::new(5);
        // 야생 HP 1 → 첫 공격에 기절 → 반격 0
        let mut b = new_battle(&mut rng, 50);
        b.wild_hp = 1;
        let o = player_attack(&mut rng, &mut b);
        assert!(o.wild_fainted);
        assert_eq!(o.wild_dmg, 0);
        assert!(!o.ally_fainted);
    }
}
