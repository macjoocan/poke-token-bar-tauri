//! 모험(CRPG) 이벤트 — 토큰 사용량에 연동된 룰 기반·결정적·오프라인 이벤트.
//!
//! 설계: **토큰 사용이 곧 모험 진행**이다. `used_since_install` 이 `ADVENTURE_INTERVAL` 만큼
//! 늘 때마다 컴패니언이 이벤트를 하나 겪는다(펫이 부화해 활동 중일 때만). 결과는 아이템 획득
//! 또는 플레이버 텍스트. 대사는 실제 LLM 이 아니라 **가중 템플릿 테이블**에서 뽑는다 —
//! `SeededRng`(companion_store) 로 결정적이라 테스트 가능하고, 네트워크·비용이 없다.
//!
//! 트리거·인벤토리 반영·로그 적재는 `CompanionStore::maybe_trigger_adventure` 가 담당하고,
//! 이 모듈은 **부수효과 없는 순수 롤(`roll_event`)** 만 제공한다.

use super::companion_model::ItemKind;
use super::companion_store::Rng;

/// 이벤트 발생 간격(토큰). `used_since_install` 이 이만큼 늘 때마다 이벤트 1회.
pub const ADVENTURE_INTERVAL: i64 = 300_000;
/// 모험 로그 최대 보관 개수(오래된 것부터 버림).
pub const ADVENTURE_LOG_CAP: usize = 20;

/// 롤 결과(순수). 부수효과(인벤토리·로그)는 호출부가 적용.
#[derive(Debug, Clone, PartialEq)]
pub struct AdventureEvent {
    pub id: &'static str,
    pub emoji: &'static str,
    pub message: String,
    /// 획득 아이템(있으면 인벤토리에 `reward_qty` 만큼). 상점 판매품이지만 모험 보상은 무상(코딩 보너스).
    pub reward: Option<ItemKind>,
    /// 보상 수량(reward 가 있을 때만 유효, 기본 1; 잭팟 이벤트는 여러 개).
    pub reward_qty: i64,
}

/// 이벤트 정의(가중 테이블 1행). `{name}` 은 펫 이름으로 치환.
struct Spec {
    weight: u32,
    id: &'static str,
    emoji: &'static str,
    reward: Option<ItemKind>,
    qty: i64,
    templates: &'static [&'static str],
}

/// 가중 이벤트 테이블. 보상 이벤트 ~23%(사탕/민트/잭팟), 나머지는 플레이버.
/// (ShinyCharm 은 패시브·고가라 드롭 제외 — 경제 균형.)
const SPECS: &[Spec] = &[
    // ── 보상: 이상한 사탕 ──
    Spec {
        weight: 9,
        id: "berry",
        emoji: "🍬",
        reward: Some(ItemKind::RareCandy),
        qty: 1,
        templates: &[
            "{name}이(가) 덤불에서 이상한 사탕을 주워 왔어요!",
            "길가에서 이상한 사탕을 발견했다!",
        ],
    },
    Spec {
        weight: 4,
        id: "gift",
        emoji: "🎁",
        reward: Some(ItemKind::RareCandy),
        qty: 1,
        templates: &["{name}이(가) 수상한 선물 상자를 열었어요 — 이상한 사탕!", "누군가 두고 간 선물에서 사탕이 나왔다!"],
    },
    // ── 보상: 민트 ──
    Spec {
        weight: 9,
        id: "herb",
        emoji: "🌿",
        reward: Some(ItemKind::Mint),
        qty: 1,
        templates: &["{name}이(가) 향긋한 민트를 캐 왔어요!", "들판에서 싱싱한 민트를 발견했다!"],
    },
    // ── 보상: 잭팟(희귀) ──
    Spec {
        weight: 2,
        id: "jackpot",
        emoji: "💰",
        reward: Some(ItemKind::RareCandy),
        qty: 3,
        templates: &["대박! {name}이(가) 이상한 사탕을 잔뜩 주워 왔어요!", "보물 더미를 발견 — 사탕을 한아름 안고 왔다!"],
    },
    // ── 플레이버(보상 없음) ──
    Spec { weight: 9, id: "wild", emoji: "👀", reward: None, qty: 0,
        templates: &["야생 포켓몬이 나타났다가 도망갔다…", "{name}이(가) 야생 포켓몬과 눈이 마주쳤어요!"] },
    Spec { weight: 8, id: "rest", emoji: "😴", reward: None, qty: 0,
        templates: &["{name}이(가) 그늘에서 잠깐 낮잠을 잤어요.", "잠시 앉아 한숨 돌렸다."] },
    Spec { weight: 7, id: "scenery", emoji: "🌄", reward: None, qty: 0,
        templates: &["{name}이(가) 노을을 바라보고 있어요.", "언덕 위에서 멋진 경치를 발견했다!"] },
    Spec { weight: 7, id: "friend", emoji: "🤝", reward: None, qty: 0,
        templates: &["{name}이(가) 친구 포켓몬을 만났어요!", "지나가던 트레이너와 인사를 나눴다."] },
    Spec { weight: 8, id: "train", emoji: "💪", reward: None, qty: 0,
        templates: &["{name}이(가) 열심히 몸을 단련했어요!", "특훈으로 땀을 흘렸다!"] },
    Spec { weight: 6, id: "fish", emoji: "🎣", reward: None, qty: 0,
        templates: &["{name}이(가) 낚시를 했지만 놓쳤어요…", "물가에서 한가로이 낚시를 즐겼다!"] },
    Spec { weight: 6, id: "swim", emoji: "💦", reward: None, qty: 0,
        templates: &["{name}이(가) 웅덩이에서 첨벙첨벙 물놀이!", "시원한 물가에서 몸을 식혔다."] },
    Spec { weight: 6, id: "star", emoji: "🌟", reward: None, qty: 0,
        templates: &["{name}이(가) 밤하늘 별을 세다 잠들었어요.", "쏟아지는 별똥별을 구경했다!"] },
    Spec { weight: 6, id: "flower", emoji: "🌸", reward: None, qty: 0,
        templates: &["{name}이(가) 꽃밭에서 나비를 쫓아다녔어요!", "들꽃 향기에 기분이 좋아졌다."] },
    Spec { weight: 5, id: "mushroom", emoji: "🍄", reward: None, qty: 0,
        templates: &["{name}이(가) 수상한 버섯을 발견… 안 먹길 잘했다.", "숲에서 알록달록한 버섯을 구경했다."] },
    Spec { weight: 4, id: "rainbow", emoji: "🌈", reward: None, qty: 0,
        templates: &["{name}이(가) 무지개를 봤어요 — 왠지 좋은 일이 생길 것 같아요!", "비 갠 하늘에 무지개가 걸렸다."] },
    Spec { weight: 4, id: "song", emoji: "🎵", reward: None, qty: 0,
        templates: &["{name}이(가) 기분 좋게 노래를 흥얼거렸어요.", "숲속 새들과 합창을 했다!"] },
    Spec { weight: 4, id: "treasure", emoji: "🗺️", reward: None, qty: 0,
        templates: &["{name}이(가) 낡은 보물지도를 주웠어요… 어디로 가는 걸까?", "땅을 파다 오래된 동전을 발견했다."] },
];

/// 이벤트 1개를 결정적으로 롤(가중 선택 + 템플릿 선택). rng 를 2회 소비.
pub fn roll_event(rng: &mut dyn Rng, pet_name: &str) -> AdventureEvent {
    let total: u32 = SPECS.iter().map(|s| s.weight).sum();
    let pick = (rng.next_u64() % total as u64) as u32;
    let mut acc = 0u32;
    let mut chosen = &SPECS[0];
    for s in SPECS {
        acc += s.weight;
        if pick < acc {
            chosen = s;
            break;
        }
    }
    let t = chosen.templates[(rng.next_u64() as usize) % chosen.templates.len()];
    AdventureEvent {
        id: chosen.id,
        emoji: chosen.emoji,
        message: t.replace("{name}", pet_name),
        reward: chosen.reward,
        reward_qty: chosen.qty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::companion_store::SeededRng;

    /// 테이블상 고유 이벤트 종 수(all-reachable 테스트 기대값).
    const EVENT_KINDS: usize = 17;

    #[test]
    fn roll_is_deterministic_for_same_seed() {
        let mut a = SeededRng::new(42);
        let mut b = SeededRng::new(42);
        for _ in 0..50 {
            assert_eq!(roll_event(&mut a, "이브이"), roll_event(&mut b, "이브이"));
        }
    }

    #[test]
    fn name_is_substituted_and_message_nonempty() {
        let mut rng = SeededRng::new(7);
        for _ in 0..200 {
            let ev = roll_event(&mut rng, "리자몽");
            assert!(!ev.message.is_empty());
            assert!(!ev.message.contains("{name}"), "치환되지 않은 플레이스홀더");
        }
    }

    #[test]
    fn reward_ratio_in_reasonable_bounds() {
        // 테이블상 보상 확률 ~23%. 4000회 롤에서 대략 그 근처여야(느슨한 경계).
        let mut rng = SeededRng::new(12345);
        let mut rewards = 0;
        let n = 4000;
        for _ in 0..n {
            if roll_event(&mut rng, "꼬부기").reward.is_some() {
                rewards += 1;
            }
        }
        let ratio = rewards as f64 / n as f64;
        assert!(ratio > 0.15 && ratio < 0.33, "보상 비율 이상: {ratio}");
    }

    #[test]
    fn reward_events_have_positive_qty_flavor_zero() {
        let mut rng = SeededRng::new(555);
        for _ in 0..500 {
            let ev = roll_event(&mut rng, "이상해씨");
            if ev.reward.is_some() {
                assert!(ev.reward_qty >= 1, "보상 이벤트는 수량 ≥ 1");
            } else {
                assert_eq!(ev.reward_qty, 0, "플레이버 이벤트는 수량 0");
            }
        }
    }

    #[test]
    fn all_ids_reachable_over_many_rolls() {
        let mut rng = SeededRng::new(999);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..6000 {
            seen.insert(roll_event(&mut rng, "피카츄").id);
        }
        assert_eq!(seen.len(), EVENT_KINDS, "일부 이벤트가 롤에서 안 나옴: {:?}", seen);
    }
}
