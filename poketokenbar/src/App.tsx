import { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

// Rust core::models 직렬화(snake_case)와 일치.
type DailyUsage = {
  date: string;
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
  total_tokens: number;
  total_cost: number;
};
type BlockUsage = {
  id: string;
  start_time: string;
  end_time: string;
  is_active: boolean;
  total_tokens: number;
  cost_usd: number;
  tokens_per_minute: number | null;
};
type UsageSnapshot = { today: DailyUsage | null; block: BlockUsage | null };

type LimitGauge = {
  label: string;
  used_percent: number;
  resets_at: string | null;
  is_active: boolean;
  as_of?: string | null;
};

// 짧은 상대시각: "어제", "3시간 전". stale 게이지(어제 기록된 값 등) 옆에 붙인다.
function relAgo(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const t = new Date(iso);
  if (isNaN(t.getTime())) return null;
  const mins = Math.floor((Date.now() - t.getTime()) / 60000);
  if (mins >= 2880) return `${Math.floor(mins / 1440)}일 전`;
  if (mins >= 1440) return "어제";
  if (mins >= 60) return `${Math.floor(mins / 60)}시간 전`;
  if (mins >= 1) return `${mins}분 전`;
  return "방금";
}
type ClaudeLimits = { plan: string | null; gauges: LimitGauge[]; as_of?: string | null };

// 기준시각 라벨: "기준 오전 11:56 (5시간 전)" 형태. 로컬 로그 기반이라 stale 가능.
function asOfLabel(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const t = new Date(iso);
  if (isNaN(t.getTime())) return null;
  const hhmm = t.toLocaleTimeString("ko-KR", { hour: "2-digit", minute: "2-digit" });
  const mins = Math.floor((Date.now() - t.getTime()) / 60000);
  let ago = "";
  if (mins >= 1440) ago = ` (${Math.floor(mins / 1440)}일 전)`;
  else if (mins >= 60) ago = ` (${Math.floor(mins / 60)}시간 전)`;
  else if (mins >= 1) ago = ` (${mins}분 전)`;
  return `기준 ${hhmm}${ago}`;
}

type AdventureEntry = {
  seq: number;
  id: string;
  emoji: string;
  message: string;
  reward: string | null;
  reward_qty: number;
};

type CompanionView = {
  latest_event: AdventureEntry | null;
  display_state: string;
  is_egg: boolean;
  egg_progress: number;
  egg_tokens_to_hatch: number;
  display_name: string;
  species_id: number | null;
  is_shiny: boolean;
  stage_index: number;
  total_forms: number;
  progress: number;
  tokens_to_next: number;
  rarity: string | null;
  dex_count: number;
  nature: string | null;
  available_tokens: number;
  rare_candy: number;
  mint_count: number;
  owns_shiny_charm: boolean;
  can_use_candy: boolean;
  can_use_mint: boolean;
  has_pending_battle: boolean;
  caught_count: number;
  great_ball_count: number;
  ultra_ball_count: number;
  toy_ball_count: number;
  toy_balloon_count: number;
  decorations: string[];
};

type BattleView = {
  wild_id: number;
  wild_level: number;
  wild_hp: number;
  wild_max_hp: number;
  wild_shiny: boolean;
  ally_species_id: number | null;
  ally_shiny: boolean;
  ally_name: string;
  ally_level: number;
  ally_hp: number;
  ally_max_hp: number;
};

type TurnOutcome = {
  ally_dmg: number;
  ally_crit: boolean;
  wild_fainted: boolean;
  wild_dmg: number;
  wild_crit: boolean;
  ally_fainted: boolean;
};

type BattleTurnView = {
  outcome: TurnOutcome;
  state: BattleView;
  result: "ongoing" | "win" | "lose";
  reward_qty: number;
  /// 승리 시 몬스터볼 포획 성공 여부(승리가 아니면 null).
  caught: boolean | null;
  /// 포획에 사용한 볼("greatBall"/"ultraBall", null=기본 몬스터볼).
  ball_used: string | null;
  /// 포획 성공 시 그 종의 희귀도(lowercase) — 전설 특별 연출용.
  caught_rarity: string | null;
};

type ChainStep = { id: number; name: string };

type DexView = {
  final_id: number;
  name: string;
  rarity: string;
  is_shiny: boolean;
  nature: string | null;
  caught_at: string | null;
  chain: ChainStep[];
};

/// 홈 히어로에 전시할 포켓몬(도감에서 선택, localStorage 영속).
type Showcase = {
  final_id: number;
  name: string;
  is_shiny: boolean;
  rarity: string;
  nature: string | null;
};
type HeroMode = "partner" | "showcase";
const SHOWCASE_KEY = "ptb-showcase";
const HERO_MODE_KEY = "ptb-hero-mode";

type ShopItemView = {
  id: string;
  name: string;
  price: number;
  affordable: boolean;
  done: boolean;
  emoji: string;
};

type SettingsView = {
  autostart: boolean;
  limit_notifications: boolean;
  warn_threshold: number;
  crit_threshold: number;
};

type Tab = "home" | "plaza" | "dex" | "shop" | "settings";
type Lang = "ko" | "en" | "ja";
type Fx = "hatch" | "evolve" | "shiny" | null;

const spriteUrl = (id: number, shiny = false) =>
  `https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/${
    shiny ? "shiny/" : ""
  }${id}.png`;

// 히어로용 스프라이트 후보(우선순위대로 시도, onError 로 다음 후보로 폴백).
// 1~5세대(id ≤ 649)는 블랙/화이트 **애니메이션 GIF** 가 있어 움직인다. 그 외/누락 시 정적 PNG.
// 이로치는 색 보존을 우선(shiny-ani → shiny-png → normal-ani → normal-png).
function heroCandidates(id: number, shiny: boolean): string[] {
  const b = "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon";
  const ani = (s: boolean) => `${b}/versions/generation-v/black-white/animated/${s ? "shiny/" : ""}${id}.gif`;
  const png = (s: boolean) => `${b}/${s ? "shiny/" : ""}${id}.png`;
  const hasAnimated = id <= 649; // Gen 1~5 만 B/W 애니 존재
  const list: string[] = [];
  if (shiny) {
    if (hasAnimated) list.push(ani(true));
    list.push(png(true));
  }
  if (hasAnimated) list.push(ani(false));
  list.push(png(false));
  return list;
}
const rarityKo: Record<string, string> = {
  common: "일반",
  uncommon: "고급",
  rare: "희귀",
  legendary: "전설",
};
// 상점/가방 아이템 기능 설명.
const shopDesc: Record<string, string> = {
  rareCandy: "사용하면 경험치를 얻어 진화·성장이 빨라져요",
  mint: "사용하면 포켓몬의 성격이 바뀌어요",
  shinyCharm: "보유 중이면 부화 시 이로치(색違) 확률이 올라가요",
  greatBall: "전투 승리 후 포획에 자동 사용 — 확률 +15%p (1회용)",
  ultraBall: "전투 승리 후 포획에 자동 사용 — 확률 +30%p (1회용, 수퍼볼보다 우선)",
  toyBall: "광장에 던져주면 포켓몬들이 공놀이를 해요 (1회용)",
  toyBalloon: "광장에 띄우면 포켓몬들이 풍선을 쫓아다녀요 (1회용)",
  decoSign: "광장에 나무 표지판을 세워요 (영구)",
  decoFlowerbed: "광장에 화단을 가꿔요 (영구)",
  decoBench: "광장에 벤치를 놓아요 (영구)",
  decoFountain: "광장 한가운데 분수를 세워요 — 우리 광장의 명물! (영구)",
  freshEgg: "지금 컴패니언을 보내고 새 알을 받아 다시 시작해요",
};
const natureKo: Record<string, string> = {
  hardy: "노력", lonely: "외로움", brave: "용감", adamant: "고집", naughty: "개구쟁이",
  bold: "대담", docile: "온순", relaxed: "무사태평", impish: "장난꾸러기", lax: "촐랑",
  timid: "겁쟁이", hasty: "성급", serious: "성실", jolly: "명랑", naive: "천진난만",
  modest: "조심", mild: "의젓", quiet: "냉정", bashful: "수줍음", rash: "덜렁",
  calm: "차분", gentle: "얌전", sassy: "건방", careful: "신중", quirky: "변덕",
};
// 포켓몬 타입(속성) — 한글명 + 공식 계열 색상.
const typeKo: Record<string, string> = {
  normal: "노말", fire: "불꽃", water: "물", electric: "전기", grass: "풀", ice: "얼음",
  fighting: "격투", poison: "독", ground: "땅", flying: "비행", psychic: "에스퍼",
  bug: "벌레", rock: "바위", ghost: "고스트", dragon: "드래곤", dark: "악",
  steel: "강철", fairy: "페어리",
};
const typeColors: Record<string, string> = {
  normal: "#A8A77A", fire: "#EE8130", water: "#6390F0", electric: "#F7D02C",
  grass: "#7AC74C", ice: "#96D9D6", fighting: "#C22E28", poison: "#A33EA1",
  ground: "#E2BF65", flying: "#A98FF3", psychic: "#F95587", bug: "#A6B91A",
  rock: "#B6A136", ghost: "#735797", dragon: "#6F35FC", dark: "#705746",
  steel: "#B7B7CE", fairy: "#D685AD",
};

// 타입은 저장 데이터에 없어 PokéAPI 에서 조회(종별 1회, 메모리 캐시). 오프라인/실패 시 빈 배열.
const typeCache = new Map<number, string[]>();
function useTypes(id: number | null): string[] | null {
  const [types, setTypes] = useState<string[] | null>(
    id != null ? typeCache.get(id) ?? null : null
  );
  useEffect(() => {
    if (id == null) {
      setTypes(null);
      return;
    }
    const cached = typeCache.get(id);
    if (cached) {
      setTypes(cached);
      return;
    }
    setTypes(null);
    let alive = true;
    fetch(`https://pokeapi.co/api/v2/pokemon/${id}`)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((j) => {
        const t = (j.types as { type: { name: string } }[]).map((x) => x.type.name);
        typeCache.set(id, t);
        if (alive) setTypes(t);
      })
      .catch(() => {
        if (alive) setTypes([]);
      });
    return () => {
      alive = false;
    };
  }, [id]);
  return types;
}

function TypeChips({ id }: { id: number | null }) {
  const types = useTypes(id);
  if (!types || types.length === 0) return null;
  return (
    <div className="type-chips">
      {types.map((t) => (
        <span key={t} className="type-chip" style={{ background: typeColors[t] ?? "#666" }}>
          {typeKo[t] ?? t}
        </span>
      ))}
    </div>
  );
}

// 전투 아군용 뒷모습 스프라이트 후보(애니 GIF → 정적 PNG → 앞모습 폴백).
function backCandidates(id: number, shiny: boolean): string[] {
  const b = "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon";
  const ani = (s: boolean) =>
    `${b}/versions/generation-v/black-white/animated/back/${s ? "shiny/" : ""}${id}.gif`;
  const png = (s: boolean) => `${b}/back/${s ? "shiny/" : ""}${id}.png`;
  const hasAnimated = id <= 649;
  const list: string[] = [];
  if (shiny) {
    if (hasAnimated) list.push(ani(true));
    list.push(png(true));
  }
  if (hasAnimated) list.push(ani(false));
  list.push(png(false));
  list.push(...heroCandidates(id, shiny)); // 뒷모습 자체가 없으면 앞모습으로
  return list;
}

// 야생 포켓몬 한글 이름 — 백엔드엔 이름 DB 가 없어(도감 라인만 저장) PokéAPI 종 조회로 해석.
const koNameCache = new Map<number, string>();
async function fetchKoName(id: number): Promise<string> {
  const cached = koNameCache.get(id);
  if (cached) return cached;
  try {
    const r = await fetch(`https://pokeapi.co/api/v2/pokemon-species/${id}`);
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const j = await r.json();
    const names = j.names as { name: string; language: { name: string } }[];
    const name = names.find((n) => n.language.name === "ko")?.name ?? j.name ?? `#${id}`;
    koNameCache.set(id, name);
    return name;
  } catch {
    return `#${id}`;
  }
}

/// 후보 URL 을 순서대로 시도하는 스프라이트 이미지(onError 폴백). key 로 종 변경 시 리셋.
function FallbackImg({ cands, alt, flip }: { cands: string[]; alt: string; flip?: boolean }) {
  return (
    <img
      key={cands[0]}
      src={cands[0]}
      alt={alt}
      style={flip ? { transform: "scaleX(-1)" } : undefined}
      data-i="0"
      onError={(e) => {
        const el = e.currentTarget as HTMLImageElement;
        const i = Number(el.dataset.i ?? "0") + 1;
        if (i < cands.length) {
          el.dataset.i = String(i);
          el.src = cands[i];
        }
      }}
    />
  );
}

// TokenFormatter.compact 포팅(표시용).
function compact(v: number): string {
  const n = Math.abs(v);
  const sign = v < 0 ? "-" : "";
  const trim = (x: number, d: number) => {
    let s = x.toFixed(d);
    s = s.replace(/0+$/, "").replace(/\.$/, "");
    return s;
  };
  if (n < 1_000) return `${v}`;
  if (n < 1_000_000) return sign + trim(n / 1_000, 1) + "K";
  if (n < 1_000_000_000) return sign + trim(n / 1_000_000, 1) + "M";
  return sign + trim(n / 1_000_000_000, 2) + "B";
}
const cost = (v: number) => `$${v.toFixed(2)}`;

function App() {
  const [tab, setTab] = useState<Tab>("home");
  const [snap, setSnap] = useState<UsageSnapshot | null>(null);
  const [limits, setLimits] = useState<ClaudeLimits | null>(null);
  const [codexLimits, setCodexLimits] = useState<ClaudeLimits | null>(null);
  const [pet, setPet] = useState<CompanionView | null>(null);
  const [dex, setDex] = useState<DexView[]>([]);
  const [shop, setShop] = useState<ShopItemView[]>([]);
  const [lang, setLang] = useState<Lang>("ko");
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [log, setLog] = useState<AdventureEntry[]>([]);
  const [speech, setSpeech] = useState<AdventureEntry | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [fx, setFx] = useState<Fx>(null);
  // 홈 히어로 전환(파트너/전시) — localStorage 영속.
  const [heroMode, setHeroMode] = useState<HeroMode>(() =>
    localStorage.getItem(HERO_MODE_KEY) === "showcase" ? "showcase" : "partner"
  );
  const [showcase, setShowcase] = useState<Showcase | null>(() => {
    try {
      return JSON.parse(localStorage.getItem(SHOWCASE_KEY) ?? "null");
    } catch {
      return null;
    }
  });
  const prevPet = useRef<CompanionView | null>(null);
  const tabRef = useRef(tab);
  tabRef.current = tab; // 리사이즈 콜백이 최신 탭을 읽도록
  const lastH = useRef(0);

  const loadTabData = useCallback(async () => {
    try {
      setDex(await invoke<DexView[]>("get_dex"));
    } catch {
      /* keep */
    }
    try {
      setShop(await invoke<ShopItemView[]>("get_shop"));
    } catch {
      /* keep */
    }
    try {
      setLog(await invoke<AdventureEntry[]>("get_adventure_log"));
    } catch {
      /* keep */
    }
  }, []);

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<UsageSnapshot>("get_today_usage");
      setSnap(s);
      setErr(null);
    } catch (e) {
      setErr(String(e));
    } finally {
      setLoading(false);
    }
    // 컴패니언 구동(부화/진화는 네트워크가 붙을 수 있음 — 실패해도 사용량 표시엔 무영향).
    try {
      const p = await invoke<CompanionView>("refresh_companion");
      setPet(p);
      if (p.latest_event) {
        setSpeech(p.latest_event); // 말풍선
        setLog(await invoke<AdventureEntry[]>("get_adventure_log").catch(() => []));
      }
    } catch {
      /* keep last */
    }
    loadTabData();
  }, [loadTabData]);

  // 한도는 네트워크(비공식 endpoint)라 usage 갱신(10초)과 분리해 저빈도로 조회한다.
  // 실패/없음이면 **이전 값 유지**(keep-previous) — 원본 정책. 매 폴마다 null 로 덮으면
  // "공식 한도" 섹션이 깜빡이며 사라졌다 나타난다(잦은 폴링이 레이트리밋도 유발). 유효한
  // 게이지를 받은 경우에만 교체.
  const refreshLimits = useCallback(async () => {
    try {
      const l = await invoke<ClaudeLimits | null>("get_claude_limits");
      if (l && l.gauges.length > 0) setLimits(l);
    } catch {
      /* keep previous */
    }
    // Codex 한도 — 로컬 로그 파싱(네트워크 무관). 유효할 때만 교체(keep-previous).
    try {
      const c = await invoke<ClaudeLimits | null>("get_codex_limits");
      if (c && c.gauges.length > 0) setCodexLimits(c);
    } catch {
      /* keep previous */
    }
  }, []);

  useEffect(() => {
    invoke<string>("get_language")
      .then((l) => setLang((l as Lang) ?? "ko"))
      .catch(() => {});
    invoke<SettingsView>("get_settings")
      .then(setSettings)
      .catch(() => {});
    refresh();
    refreshLimits();
    const t = setInterval(refresh, 10_000); // 사용량·컴패니언: 10초(로컬, 저렴)
    const tl = setInterval(refreshLimits, 120_000); // 한도: 120초(네트워크, 원본과 동일)
    return () => {
      clearInterval(t);
      clearInterval(tl);
    };
  }, [refresh, refreshLimits]);

  // 상태 전이 연출 감지(부화/진화/이로치).
  useEffect(() => {
    const prev = prevPet.current;
    if (pet && prev) {
      if (prev.is_egg && !pet.is_egg) setFx("hatch");
      else if (!pet.is_egg && !prev.is_egg && pet.stage_index > prev.stage_index) setFx("evolve");
      else if (pet.is_shiny && !prev.is_shiny) setFx("shiny");
    }
    prevPet.current = pet;
  }, [pet]);
  useEffect(() => {
    if (!fx) return;
    const t = setTimeout(() => setFx(null), 1600);
    return () => clearTimeout(t);
  }, [fx]);
  // 말풍선 자동 소멸(6초).
  useEffect(() => {
    if (!speech) return;
    const t = setTimeout(() => setSpeech(null), 6000);
    return () => clearTimeout(t);
  }, [speech]);

  // 창 높이 = **홈 탭 콘텐츠 기준으로 유동**. 홈에서만 body 높이를 측정해 창을 맞추고,
  // 도감/상점/설정 탭에선 창을 다시 조절하지 않아 홈 크기를 유지한다(탭 전환 시 창이 튀지 않음).
  // 비-홈 탭은 `.popover.fixed-size`(min-height:100vh)로 홈 크기를 채우고 넘치면 내부 스크롤.
  const applyHomeHeight = useCallback(() => {
    if (tabRef.current !== "home") return;
    const h = Math.ceil(document.body.scrollHeight);
    if (h > 0 && Math.abs(h - lastH.current) > 1) {
      lastH.current = h;
      invoke("set_window_height", { height: h }).catch(() => {});
    }
  }, []);
  useEffect(() => {
    let timer: number | undefined;
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = window.setTimeout(applyHomeHeight, 50);
    };
    schedule();
    const ro = new ResizeObserver(schedule);
    ro.observe(document.body);
    return () => {
      ro.disconnect();
      if (timer) clearTimeout(timer);
    };
  }, [applyHomeHeight]);
  // 홈으로 돌아오면 재측정(다른 탭에서 홈 콘텐츠가 바뀌었을 수 있음).
  useEffect(() => {
    if (tab !== "home") return;
    const t = window.setTimeout(applyHomeHeight, 60);
    return () => clearTimeout(t);
  }, [tab, applyHomeHeight]);

  const buy = useCallback(
    async (id: string) => {
      try {
        const p = await invoke<CompanionView>("buy_item", { id });
        setPet(p);
        loadTabData();
      } catch {
        /* ignore */
      }
    },
    [loadTabData]
  );
  const useItem = useCallback(
    async (cmd: "use_rare_candy" | "use_mint") => {
      try {
        const p = await invoke<CompanionView>(cmd);
        setPet(p);
        loadTabData();
      } catch {
        /* ignore */
      }
    },
    [loadTabData]
  );
  const changeLang = useCallback((l: Lang) => {
    setLang(l);
    invoke("set_language", { lang: l }).catch(() => {});
  }, []);

  // 전투 카드 표시 래치 — pending 감지로 켜고, 연출까지 끝난 onDone 에서만 끈다.
  // (backend 는 마지막 턴에서 pending 을 지우므로, 10s refresh 가 연출 중에
  //  has_pending_battle=false 를 받아와도 카드가 중간에 사라지지 않게 분리.)
  const [battleActive, setBattleActive] = useState(false);
  useEffect(() => {
    // 창이 보일 때만 시작 — 팝오버가 숨겨진 채 전투가 몰래 끝나버리지 않게.
    const latch = () => {
      if (document.visibilityState !== "hidden" && pet?.has_pending_battle) {
        setBattleActive(true);
      }
    };
    latch();
    document.addEventListener("visibilitychange", latch);
    return () => document.removeEventListener("visibilitychange", latch);
  }, [pet?.has_pending_battle]);
  const battleDone = useCallback(() => {
    setBattleActive(false);
    refresh(); // 모험 로그·사탕 수·pet 상태 갱신
  }, [refresh]);

  const changeHeroMode = useCallback((m: HeroMode) => {
    setHeroMode(m);
    localStorage.setItem(HERO_MODE_KEY, m);
  }, []);
  // 도감 상세에서 "메인에 전시" — 저장 후 전시 모드로 전환.
  const showcaseFromDex = useCallback(
    (d: DexView) => {
      const s: Showcase = {
        final_id: d.final_id,
        name: d.name,
        is_shiny: d.is_shiny,
        rarity: d.rarity,
        nature: d.nature,
      };
      setShowcase(s);
      localStorage.setItem(SHOWCASE_KEY, JSON.stringify(s));
      changeHeroMode("showcase");
    },
    [changeHeroMode]
  );

  const [backupMsg, setBackupMsg] = useState<string | null>(null);
  const exportSave = useCallback(async () => {
    try {
      const p = await invoke<string | null>("export_save");
      setBackupMsg(p ? `백업 저장 완료:\n${p}` : "백업 실패");
    } catch {
      setBackupMsg("백업 실패");
    }
  }, []);
  const importSave = useCallback(async () => {
    try {
      const ok = await invoke<boolean>("import_save");
      if (ok) {
        setBackupMsg("백업을 불러왔어요! 🎉");
        refresh();
        loadTabData();
      } else {
        setBackupMsg("불러올 백업 파일이 없어요");
      }
    } catch {
      setBackupMsg("복원 실패");
    }
  }, [refresh, loadTabData]);

  const toggleAutostart = useCallback(
    async (enabled: boolean) => {
      setSettings((s) => (s ? { ...s, autostart: enabled } : s));
      try {
        await invoke<boolean>("set_autostart", { enabled });
      } catch {
        // 실패 시 실제 상태 재조회로 복원.
        invoke<SettingsView>("get_settings").then(setSettings).catch(() => {});
      }
    },
    []
  );
  const saveSettings = useCallback((next: SettingsView) => {
    setSettings(next);
    invoke("update_settings", {
      limitNotifications: next.limit_notifications,
      warnThreshold: next.warn_threshold,
      critThreshold: next.crit_threshold,
    }).catch(() => {});
  }, []);

  const today = snap?.today ?? null;
  const block = snap?.block ?? null;

  return (
    <main className={`popover ${tab !== "home" ? "fixed-size" : ""}`}>
      <header className="pop-header" data-tauri-drag-region>
        <span className="pop-title" data-tauri-drag-region>PokeTokenBar</span>
        <button
          className="pop-refresh"
          onClick={() => {
            refresh();
            refreshLimits();
          }}
          title="새로고침"
        >
          ↻
        </button>
      </header>

      {loading ? (
        <div className="pop-empty">불러오는 중…</div>
      ) : err && tab === "home" ? (
        <div className="pop-empty pop-error">오류: {err}</div>
      ) : (
        <div className="tab-body">
          {tab === "home" && (
            <>
              {/* 전투 중엔 히어로 자리를 전투 장면이 차지(자동 진행). */}
              {battleActive ? (
                <BattleCard onDone={battleDone} />
              ) : (
                <>
                  <div className="hero-toggle">
                    <button
                      className={heroMode === "partner" ? "active" : ""}
                      onClick={() => changeHeroMode("partner")}
                    >
                      🐾 파트너
                    </button>
                    <button
                      className={heroMode === "showcase" ? "active" : ""}
                      onClick={() => changeHeroMode("showcase")}
                    >
                      🖼️ 전시
                    </button>
                  </div>
                  {heroMode === "showcase" ? (
                    showcase ? (
                      <ShowcaseCard s={showcase} />
                    ) : (
                      <section className="pet-hero">
                        <div className="showcase-empty">
                          아직 전시할 포켓몬이 없어요.
                          <br />
                          도감 탭에서 포켓몬을 클릭한 뒤
                          <br />
                          「메인에 전시하기」를 눌러보세요!
                        </div>
                      </section>
                    )
                  ) : (
                    pet && <CompanionCard pet={pet} fx={fx} speech={speech} />
                  )}
                </>
              )}
              {log.length > 0 && <AdventureLog log={log} />}
              <section className="today-card">
                <div className="today-label">오늘 사용량</div>
                <div className="today-tokens">{today ? compact(today.total_tokens) : "0"}</div>
                <div className="today-cost">{today ? cost(today.total_cost) : "$0.00"}</div>
              </section>
              {today && (
                <section className="breakdown">
                  <Row label="입력" value={compact(today.input_tokens)} />
                  <Row label="출력" value={compact(today.output_tokens)} />
                  <Row label="캐시 쓰기" value={compact(today.cache_creation_tokens)} />
                  <Row label="캐시 읽기" value={compact(today.cache_read_tokens)} />
                </section>
              )}
              <section className="block-card">
                <div className="block-label">최근 5시간 활동</div>
                {block ? (
                  <div className="block-row">
                    <span>{compact(block.total_tokens)} tokens</span>
                    <span className="block-rate">
                      {Math.round(block.tokens_per_minute ?? 0).toLocaleString()} tok/min
                    </span>
                  </div>
                ) : (
                  <div className="block-idle">활동 없음</div>
                )}
              </section>
              {limits && limits.gauges.length > 0 && (
                <section className="limits-card">
                  <div className="limits-head">
                    <span className="block-label">Claude 한도</span>
                    {limits.plan && <span className="plan-badge">{limits.plan}</span>}
                  </div>
                  {limits.gauges.map((g) => (
                    <Gauge key={g.label} g={g} />
                  ))}
                </section>
              )}
              {codexLimits && codexLimits.gauges.length > 0 && (
                <section className="limits-card">
                  <div className="limits-head">
                    <span className="block-label">Codex 한도</span>
                    {codexLimits.plan && <span className="plan-badge codex">{codexLimits.plan}</span>}
                  </div>
                  {codexLimits.gauges.map((g) => (
                    <Gauge key={`codex-${g.label}`} g={g} />
                  ))}
                  {asOfLabel(codexLimits.as_of) && (
                    <div className="limits-asof">{asOfLabel(codexLimits.as_of)}</div>
                  )}
                </section>
              )}
            </>
          )}

          {tab === "plaza" && <PlazaTab dex={dex} pet={pet} onPet={setPet} />}

          {tab === "dex" && (
            <PokedexTab
              dex={dex}
              total={pet?.dex_count ?? dex.length}
              caught={pet?.caught_count ?? 0}
              showcase={showcase}
              onShowcase={showcaseFromDex}
            />
          )}

          {tab === "shop" && (
            <ShopTab pet={pet} shop={shop} onBuy={buy} onUse={useItem} />
          )}

          {tab === "settings" && (
            <SettingsTab
              lang={lang}
              onLang={changeLang}
              pet={pet}
              settings={settings}
              onAutostart={toggleAutostart}
              onSave={saveSettings}
              onExport={exportSave}
              onImport={importSave}
              backupMsg={backupMsg}
            />
          )}
        </div>
      )}

      <nav className="tab-bar">
        <TabBtn cur={tab} id="home" label="홈" icon="🏠" onClick={setTab} />
        <TabBtn cur={tab} id="plaza" label="광장" icon="🏞️" onClick={setTab} />
        <TabBtn cur={tab} id="dex" label="도감" icon="📖" onClick={setTab} />
        <TabBtn cur={tab} id="shop" label="상점" icon="🛍️" onClick={setTab} />
        <TabBtn cur={tab} id="settings" label="설정" icon="⚙️" onClick={setTab} />
      </nav>
    </main>
  );
}

function TabBtn({
  cur,
  id,
  label,
  icon,
  onClick,
}: {
  cur: Tab;
  id: Tab;
  label: string;
  icon: string;
  onClick: (t: Tab) => void;
}) {
  return (
    <button className={`tab-btn ${cur === id ? "active" : ""}`} onClick={() => onClick(id)}>
      <span className="tab-ico">{icon}</span>
      <span className="tab-lbl">{label}</span>
    </button>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="brk-row">
      <span className="brk-label">{label}</span>
      <span className="brk-value">{value}</span>
    </div>
  );
}

function SpeechBubble({ speech }: { speech: AdventureEntry | null }) {
  if (!speech) return null;
  return (
    <div className="speech" key={speech.seq}>
      <span className="speech-emoji">{speech.emoji}</span>
      <span className="speech-text">{speech.message}</span>
    </div>
  );
}

function CompanionCard({ pet, fx, speech }: { pet: CompanionView; fx: Fx; speech: AdventureEntry | null }) {
  const fxClass = fx ? `fx-${fx}` : "";
  if (pet.is_egg) {
    const pct = Math.round(pet.egg_progress * 100);
    return (
      <section className={`pet-hero ${fxClass}`}>
        <div className="hero-sprite egg">🥚</div>
        <div className="hero-name">토큰 알</div>
        <div className="hero-sub">부화까지 {compact(pet.egg_tokens_to_hatch)}</div>
        <div className="gauge-track">
          <div className="gauge-fill ok" style={{ width: `${pct}%` }} />
        </div>
      </section>
    );
  }
  const isFinal = pet.stage_index + 1 >= pet.total_forms;
  const pct = Math.round(pet.progress * 100);
  const heroCands = pet.species_id != null ? heroCandidates(pet.species_id, pet.is_shiny) : [];
  return (
    <section className={`pet-hero ${fxClass}`}>
      <SpeechBubble speech={speech} />
      <div className="hero-sprite">
        {fx && <span className="fx-overlay" />}
        {pet.species_id != null ? (
          <FallbackImg cands={heroCands} alt={pet.display_name} />
        ) : (
          "❔"
        )}
      </div>
      <div className="hero-name">
        {pet.display_name}
        {pet.is_shiny && (
          <span className="shiny" title="이로치">
            ✨
          </span>
        )}
        {pet.rarity && <span className="rarity-badge">{rarityKo[pet.rarity] ?? pet.rarity}</span>}
      </div>
      <div className="hero-sub">
        {isFinal ? "최종 단계" : `${pet.stage_index + 1} / ${pet.total_forms} 단계`}
        {" · "}도감 {pet.dex_count}
        {pet.nature && ` · ${natureKo[pet.nature] ?? pet.nature}`}
      </div>
      {!isFinal && (
        <>
          <div className="gauge-track">
            <div className="gauge-fill ok" style={{ width: `${pct}%` }} />
          </div>
          <div className="hero-next">다음까지 {compact(pet.tokens_to_next)}</div>
        </>
      )}
    </section>
  );
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function BattleHpBar({ hp, max, showNum }: { hp: number; max: number; showNum?: boolean }) {
  const pct = max > 0 ? Math.max(0, Math.min(100, (hp / max) * 100)) : 0;
  const hue = pct > 50 ? "ok" : pct > 20 ? "warn" : "danger";
  return (
    <div className="bhp">
      <span className="bhp-label">HP</span>
      <div className="bhp-track">
        <div className={`bhp-fill ${hue}`} style={{ width: `${pct}%` }} />
      </div>
      {showNum && (
        <span className="bhp-num">
          {hp}/{max}
        </span>
      )}
    </div>
  );
}

/// 야생 전투 카드 — 홈 히어로 자리에서 **자동 진행**되는 HGSS 스타일 미니 전투.
/// 마운트 시 pending 전투를 읽어 자동 턴 루프를 돌리고, 끝나면 onDone(→refresh)으로
/// 카드를 닫는다. 모든 대기 지점에서 alive 를 확인해 언마운트(StrictMode 이중 마운트
/// 포함) 시 즉시 중단 — 첫 공격 전에 체크포인트가 있어 중복 턴 소비가 없다.
function BattleCard({ onDone }: { onDone: () => void }) {
  const [bs, setBs] = useState<BattleView | null>(null);
  const [msg, setMsg] = useState("…");
  const [wildName, setWildName] = useState("");
  const [wildHit, setWildHit] = useState(false);
  const [allyHit, setAllyHit] = useState(false);
  const [wildFaint, setWildFaint] = useState(false);
  const [allyFaint, setAllyFaint] = useState(false);
  // 몬스터볼 연출 단계(승리 후): null=없음 → shake(흔들림) → caught(포획 확정).
  const [ball, setBall] = useState<null | "shake" | "caught">(null);
  // 볼 등급(색상): "" = 기본 몬스터볼, great = 수퍼볼(파랑), ultra = 하이퍼볼(노랑).
  const [ballKind, setBallKind] = useState("");
  // 전설 포획 특별 연출(황금 플래시).
  const [legendFx, setLegendFx] = useState(false);

  useEffect(() => {
    let alive = true;
    // 대기 + 생존 확인(취소 체크포인트).
    const wait = async (ms: number) => {
      await sleep(ms);
      return alive;
    };
    (async () => {
      const b = await invoke<BattleView | null>("get_battle").catch(() => null);
      if (!alive) return;
      if (!b) {
        onDone(); // 전투 무효(파트너 교체 등)
        return;
      }
      setBs(b);
      const name = await fetchKoName(b.wild_id);
      if (!alive) return;
      setWildName(name);
      setMsg(`야생 ${name}이(가) 나타났다!`);
      if (!(await wait(1400))) return;

      // ── 자동 턴 루프 ──
      for (;;) {
        const t = await invoke<BattleTurnView | null>("battle_attack", {
          wildName: name,
        }).catch(() => null);
        if (!alive) return;
        if (!t) {
          onDone();
          return;
        }
        setMsg(`${b.ally_name}의 공격!`);
        setWildHit(true);
        if (!(await wait(450))) return;
        setWildHit(false);
        setBs((s) => (s ? { ...s, wild_hp: t.state.wild_hp } : t.state));
        if (t.outcome.ally_crit) {
          setMsg("급소에 맞았다!");
          if (!(await wait(700))) return;
        }
        if (!(await wait(500))) return;
        if (t.result === "win") {
          setMsg(`야생 ${name}은(는) 힘이 다했다!`);
          if (!(await wait(1000))) return;
          // ── 포획 시도(결과·사용 볼은 백엔드가 이미 판정) ──
          const ballName =
            t.ball_used === "ultraBall" ? "하이퍼볼" : t.ball_used === "greatBall" ? "수퍼볼" : "몬스터볼";
          setBallKind(t.ball_used === "ultraBall" ? "ultra" : t.ball_used === "greatBall" ? "great" : "");
          setMsg(`${ballName}을 던졌다!`);
          setBall("shake");
          if (!(await wait(2100))) return;
          if (t.caught) {
            setBall("caught");
            if (t.caught_rarity === "legendary") {
              // ── 전설 특별 연출: 황금 플래시 + 스파클 ──
              setLegendFx(true);
              setMsg(`이럴 수가…!! 전설의 ${name}을(를) 잡았다!!`);
              if (!(await wait(2800))) return;
              setMsg(`🌟 전설의 포켓몬이 도감에 등록됐다!`);
              if (!(await wait(1600))) return;
            } else {
              setMsg(`신난다! ${name}을(를) 잡았다! 도감에 등록됐다!`);
              if (!(await wait(1900))) return;
            }
          } else {
            setBall(null);
            setMsg(`앗! ${name}은(는) 볼에서 빠져나왔다!`);
            if (!(await wait(900))) return;
            setWildFaint(true);
            setMsg(`야생 ${name}은(는) 도망쳐 버렸다…`);
            if (!(await wait(1300))) return;
          }
          setMsg(`🍬 전리품으로 이상한 사탕 ×${t.reward_qty} 획득!`);
          if (!(await wait(1600))) return;
          onDone();
          return;
        }
        setMsg(`야생 ${name}의 반격!`);
        setAllyHit(true);
        if (!(await wait(450))) return;
        setAllyHit(false);
        setBs(t.state);
        if (t.outcome.wild_crit) {
          setMsg("급소에 맞았다!");
          if (!(await wait(700))) return;
        }
        if (!(await wait(500))) return;
        if (t.result === "lose") {
          setAllyFaint(true);
          setMsg(`${b.ally_name}은(는) 쓰러졌다…`);
          if (!(await wait(1700))) return;
          onDone();
          return;
        }
        if (!(await wait(550))) return;
      }
    })();
    return () => {
      alive = false;
    };
  }, [onDone]);

  return (
    <section className="battle-card">
      <div className={`battle-scene ${legendFx ? "legendary" : ""}`}>
        {bs && (
          <>
            <div className="binfo binfo-wild">
              <div className="binfo-name">
                {wildName || `#${bs.wild_id}`}
                {bs.wild_shiny && <span className="shiny">✨</span>}
                <span className="binfo-lv">Lv.{bs.wild_level}</span>
              </div>
              <BattleHpBar hp={bs.wild_hp} max={bs.wild_max_hp} />
            </div>
            <div className={`bmon bmon-wild ${wildHit ? "hit" : ""} ${wildFaint ? "faint" : ""}`}>
              <div className="bplat" />
              {ball ? (
                <div className={`pokeball ${ball} ${ballKind}`} />
              ) : (
                <FallbackImg cands={heroCandidates(bs.wild_id, bs.wild_shiny)} alt={wildName} />
              )}
            </div>
            <div className={`bmon bmon-ally ${allyHit ? "hit" : ""} ${allyFaint ? "faint" : ""}`}>
              <div className="bplat" />
              {bs.ally_species_id != null && (
                <FallbackImg
                  cands={backCandidates(bs.ally_species_id, bs.ally_shiny)}
                  alt={bs.ally_name}
                />
              )}
            </div>
            <div className="binfo binfo-ally">
              <div className="binfo-name">
                {bs.ally_name}
                {bs.ally_shiny && <span className="shiny">✨</span>}
                <span className="binfo-lv">Lv.{bs.ally_level}</span>
              </div>
              <BattleHpBar hp={bs.ally_hp} max={bs.ally_max_hp} showNum />
            </div>
          </>
        )}
      </div>
      <div className="battle-msg">{msg}</div>
    </section>
  );
}

/// 전시 모드 히어로 — 도감에서 고른 포켓몬을 파트너 대신 크게 표시.
function ShowcaseCard({ s }: { s: Showcase }) {
  const cands = heroCandidates(s.final_id, s.is_shiny);
  return (
    <section className="pet-hero">
      <div className="hero-sprite">
        <FallbackImg cands={cands} alt={s.name} />
      </div>
      <div className="hero-name">
        {s.name}
        {s.is_shiny && (
          <span className="shiny" title="이로치">
            ✨
          </span>
        )}
        <span className="rarity-badge">{rarityKo[s.rarity] ?? s.rarity}</span>
      </div>
      <div className="hero-sub">
        전시 중{s.nature ? ` · ${natureKo[s.nature] ?? s.nature} 성격` : ""}
      </div>
      <TypeChips id={s.final_id} />
    </section>
  );
}

const rewardKo: Record<string, string> = {
  rareCandy: "이상한 사탕",
  mint: "민트",
  shinyCharm: "빛나는 부적",
};

function AdventureLog({ log }: { log: AdventureEntry[] }) {
  return (
    <section className="adv-card">
      <div className="adv-head">
        <span className="block-label">모험 로그</span>
        <span className="adv-hint">토큰을 쓸수록 모험이 진행돼요</span>
      </div>
      <div className="adv-list">
        {log.slice(0, 5).map((e) => (
          <div className="adv-row" key={e.seq}>
            <span className="adv-emoji">{e.emoji}</span>
            <span className="adv-msg">{e.message}</span>
            {e.reward && (
              <span className="adv-reward">
                +{rewardKo[e.reward] ?? e.reward}
                {e.reward_qty > 1 ? ` ×${e.reward_qty}` : ""}
              </span>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}

type DexSort = "number" | "rarity" | "recent";
type DexFilter = "all" | "shiny";
const rarityRank: Record<string, number> = { legendary: 3, rare: 2, uncommon: 1, common: 0 };

function PokedexTab({
  dex,
  total,
  caught,
  showcase,
  onShowcase,
}: {
  dex: DexView[];
  total: number;
  caught: number;
  showcase: Showcase | null;
  onShowcase: (d: DexView) => void;
}) {
  const [sort, setSort] = useState<DexSort>("number");
  const [filter, setFilter] = useState<DexFilter>("all");
  const [selected, setSelected] = useState<DexView | null>(null);
  const shinyCount = dex.filter((d) => d.is_shiny).length;

  const select = (d: DexView) => {
    setSelected(d);
    // 상세 패널은 목록 위에 붙으므로 스크롤을 맨 위로 올려 바로 보이게 한다.
    document.querySelector(".tab-body")?.scrollTo({ top: 0, behavior: "smooth" });
  };

  if (dex.length === 0) {
    return (
      <div className="pop-empty">
        아직 도감에 등록된 포켓몬이 없어요.
        <br />
        알이 부화하면 그 포켓몬이 도감에 등록됩니다.
      </div>
    );
  }

  // dex 는 획득순(오래된→최신). recent 는 역순, number 는 도감번호, rarity 는 희귀도 높은 순.
  const view = dex
    .map((d, i) => ({ d, i }))
    .filter(({ d }) => (filter === "shiny" ? d.is_shiny : true))
    .sort((a, b) => {
      if (sort === "number") return a.d.final_id - b.d.final_id;
      if (sort === "recent") return b.i - a.i;
      // rarity: 높은 순, 동률이면 번호
      const r = (rarityRank[b.d.rarity] ?? 0) - (rarityRank[a.d.rarity] ?? 0);
      return r !== 0 ? r : a.d.final_id - b.d.final_id;
    })
    .map(({ d }) => d);

  const sorts: { id: DexSort; label: string }[] = [
    { id: "number", label: "번호" },
    { id: "rarity", label: "희귀도" },
    { id: "recent", label: "최근" },
  ];

  const isShowcased =
    selected != null &&
    showcase != null &&
    showcase.final_id === selected.final_id &&
    showcase.is_shiny === selected.is_shiny;

  return (
    <section className="dex-wrap">
      {selected && (
        <DexDetail
          d={selected}
          showcased={isShowcased}
          onShowcase={onShowcase}
          onClose={() => setSelected(null)}
        />
      )}
      <div className="dex-count">
        <span>
          도감 {total}종 수집
          {caught > 0 && <span className="dex-caught"> · 포획 {caught}</span>}
        </span>
        {shinyCount > 0 && <span className="dex-shiny-count">✨ {shinyCount}</span>}
      </div>
      <div className="dex-controls">
        <div className="dex-seg">
          {sorts.map((s) => (
            <button
              key={s.id}
              className={`dex-seg-btn ${sort === s.id ? "active" : ""}`}
              onClick={() => setSort(s.id)}
            >
              {s.label}
            </button>
          ))}
        </div>
        <button
          className={`dex-shiny-toggle ${filter === "shiny" ? "active" : ""}`}
          onClick={() => setFilter(filter === "shiny" ? "all" : "shiny")}
          title="이로치만 보기"
        >
          ✨
        </button>
      </div>
      {view.length === 0 ? (
        <div className="pop-empty">이로치로 수집한 포켓몬이 아직 없어요.</div>
      ) : (
        <div className="dex-grid">
          {view.map((d, i) => (
            <button
              className={`dex-cell ${d.is_shiny ? "shiny" : ""} ${
                // dex 는 10초마다 새 배열로 갱신되므로 참조가 아닌 id 로 비교.
                selected?.final_id === d.final_id && selected?.is_shiny === d.is_shiny
                  ? "selected"
                  : ""
              }`}
              key={`${d.final_id}-${i}`}
              title={d.name}
              onClick={() => select(d)}
            >
              <img src={spriteUrl(d.final_id, d.is_shiny)} alt={d.name} width={64} height={64} />
              <div className="dex-name">
                {d.name}
                {d.is_shiny && <span className="shiny">✨</span>}
              </div>
              <div className="dex-rarity">{rarityKo[d.rarity] ?? d.rarity}</div>
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

/// 도감 상세 패널 — 클릭한 포켓몬을 상단에 크게 + 속성/성격/획득일/진화 체인 텍스트.
function DexDetail({
  d,
  showcased,
  onShowcase,
  onClose,
}: {
  d: DexView;
  showcased: boolean;
  onShowcase: (d: DexView) => void;
  onClose: () => void;
}) {
  const cands = heroCandidates(d.final_id, d.is_shiny);
  return (
    <section className="pet-hero dex-detail">
      <button className="dex-detail-close" onClick={onClose} title="닫기">
        ✕
      </button>
      <div className="hero-sprite">
        <FallbackImg cands={cands} alt={d.name} />
      </div>
      <div className="hero-name">
        No.{d.final_id} {d.name}
        {d.is_shiny && (
          <span className="shiny" title="이로치">
            ✨
          </span>
        )}
      </div>
      <TypeChips id={d.final_id} />
      <div className="dex-detail-rows">
        <div className="dex-info-row">
          <span>희귀도</span>
          <span>{rarityKo[d.rarity] ?? d.rarity}</span>
        </div>
        {d.nature && (
          <div className="dex-info-row">
            <span>성격</span>
            <span>{natureKo[d.nature] ?? d.nature}</span>
          </div>
        )}
        {d.caught_at && (
          <div className="dex-info-row">
            <span>획득일</span>
            <span>{d.caught_at}</span>
          </div>
        )}
      </div>
      {d.chain.length > 1 && (
        <div className="dex-chain">
          {d.chain.map((c, i) => (
            <div className="dex-chain-item" key={c.id}>
              {i > 0 && <span className="chain-arrow">→</span>}
              <div className={`chain-node ${c.id === d.final_id ? "cur" : ""}`}>
                <img src={spriteUrl(c.id, d.is_shiny)} alt={c.name} width={40} height={40} />
                <span>{c.name}</span>
              </div>
            </div>
          ))}
        </div>
      )}
      <button
        className="dex-showcase-btn"
        disabled={showcased}
        onClick={() => onShowcase(d)}
      >
        {showcased ? "✓ 메인에 전시 중" : "🖼️ 메인에 전시하기"}
      </button>
    </section>
  );
}

// ── 광장(플라자) — 모은 포켓몬들이 도트 필드에서 자율 행동(산책·잠·식사·놀이·투닥) ──

type PlazaAction = "idle" | "walk" | "sleep" | "eat" | "play" | "fight";
/// 걸음걸이 — walk 뒤뚱 / hop 폴짝 이동 / run 전력질주(빠름).
type PlazaGait = "walk" | "hop" | "run";
type Resident = {
  key: string;
  id: number;
  shiny: boolean;
  name: string;
  partner: boolean; // 현재 키우는 파트너 표시
  x: number;
  y: number;
  dur: number; // 현재 이동 transition 시간(s)
  flip: boolean;
  /// 위쪽(화면 안쪽)으로 갈 땐 뒷모습 스프라이트.
  facing: "front" | "back";
  gait: PlazaGait;
  action: PlazaAction;
  bubble: string | null;
  busyUntil: number; // ms epoch — 이 시각까지 현재 행동 유지
};
/// 걸음걸이별 이동 속도(px/s).
const GAIT_SPEED: Record<PlazaGait, number> = { walk: 42, hop: 48, run: 92 };

const PLAZA_MAX = 12; // 동시 출연 상한(GIF 성능·밀도)
const PLAZA_SPRITE = 56;
const plazaFoods = ["🍎", "🍙", "🍇", "🍪"];
/// 던진 장난감이 필드에 머무는 시간(ms).
const TOY_LIFETIME = 30_000;

/// 필드에 던져진 장난감(한 번에 하나).
type Toy = {
  kind: "ball" | "balloon";
  x: number;
  y: number;
  dur: number;
  moveUntil: number; // 현재 이동(굴러가기/드리프트) 종료 시각
  until: number; // 소멸 시각
};
// 필드 가장자리 관목 울타리 두께(css px) — 이동/스폰 경계.
const PLAZA_HEDGE = 32;

// 결정적 PRNG — 배경 타일맵이 리사이즈/재마운트에도 같은 그림이 나오게.
function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/// 도트 오버월드 타일맵 — 저해상 캔버스에 직접 픽셀을 찍고 ×2 확대(pixelated).
/// GBA 루트맵 문법: 체커 잔디 + 풀잎 자국, 흙길, 연못, 키큰풀 패치, 관목 울타리, 꽃/바위.
function drawPlazaMap(cv: HTMLCanvasElement, cssW: number, cssH: number, decos: string[]) {
  const S = 2; // 논리 픽셀 1 = css 2px (스프라이트 픽셀 밀도와 유사)
  const W = Math.ceil(cssW / S);
  const H = Math.ceil(cssH / S);
  cv.width = W;
  cv.height = H;
  const g = cv.getContext("2d");
  if (!g) return;
  const rnd = mulberry32(20260813);
  const T = 16;

  // 잔디 — 두 톤 체커 + 듬성한 풀잎 자국
  for (let ty = 0; ty < Math.ceil(H / T); ty++) {
    for (let tx = 0; tx < Math.ceil(W / T); tx++) {
      g.fillStyle = (tx + ty) % 2 === 0 ? "#7cb94e" : "#74b046";
      g.fillRect(tx * T, ty * T, T, T);
      if (rnd() < 0.55) {
        g.fillStyle = "#639c3f";
        const px = tx * T + 2 + Math.floor(rnd() * (T - 7));
        const py = ty * T + 2 + Math.floor(rnd() * (T - 6));
        g.fillRect(px, py, 2, 1);
        g.fillRect(px + 3, py + 2, 2, 1);
      }
    }
  }

  // 흙길(가로 밴드) — 가장자리 요철 + 밝은 점
  const pathY = Math.floor(H * 0.6);
  const pathH = 14;
  g.fillStyle = "#d9c08b";
  g.fillRect(0, pathY, W, pathH);
  g.fillStyle = "#c2a267";
  for (let x = 0; x < W; x += 4) {
    if (rnd() < 0.5) g.fillRect(x, pathY, 2, 2);
    if (rnd() < 0.5) g.fillRect(x, pathY + pathH - 2, 2, 2);
  }
  g.fillStyle = "#e8d8a8";
  for (let i = 0; i < W / 6; i++) {
    g.fillRect(Math.floor(rnd() * W), pathY + 3 + Math.floor(rnd() * (pathH - 5)), 2, 1);
  }

  // 연못(우하단) — 진한 외곽 + 물 + 하이라이트·물결
  const pw = 46;
  const ph = 26;
  const pxx = W - pw - 14;
  const pyy = H - ph - 16;
  const pond = (inset: number, color: string) => {
    g.fillStyle = color;
    g.fillRect(pxx + inset + 2, pyy + inset, pw - inset * 2 - 4, ph - inset * 2);
    g.fillRect(pxx + inset, pyy + inset + 2, pw - inset * 2, ph - inset * 2 - 4);
  };
  pond(0, "#2e5e86");
  pond(2, "#4e9fd8");
  g.fillStyle = "#7cc0ea";
  g.fillRect(pxx + 6, pyy + 4, 10, 2);
  g.fillRect(pxx + 4, pyy + 6, 4, 2);
  g.fillStyle = "#a8d8f0";
  g.fillRect(pxx + 16, pyy + 12, 6, 1);
  g.fillRect(pxx + 30, pyy + 8, 5, 1);
  g.fillRect(pxx + 10, pyy + 18, 6, 1);

  // 키큰풀 패치 — 8px 셀 지그재그(수풀 타일)
  const tall = (bx: number, by: number, tw: number, th: number) => {
    for (let y = 0; y < th; y++) {
      for (let x = 0; x < tw; x++) {
        const gx = bx + x * 8;
        const gy = by + y * 8;
        g.fillStyle = "#4e9a3e";
        g.fillRect(gx, gy, 8, 8);
        g.fillStyle = "#3e7c34";
        g.fillRect(gx, gy + 5, 2, 3);
        g.fillRect(gx + 3, gy + 3, 2, 5);
        g.fillRect(gx + 6, gy + 5, 2, 3);
        g.fillStyle = "#66b452";
        g.fillRect(gx + 1, gy + 1, 1, 2);
        g.fillRect(gx + 4, gy, 1, 2);
        g.fillRect(gx + 7, gy + 1, 1, 2);
      }
    }
  };
  tall(20, Math.floor(H * 0.3), 4, 2);
  tall(Math.floor(W * 0.56), Math.floor(H * 0.18), 3, 2);

  // 꽃 — 노랑 심 + 흰/분홍 꽃잎(길·연못 회피)
  const flower = (fx: number, fy: number, petal: string) => {
    g.fillStyle = petal;
    g.fillRect(fx - 2, fy, 2, 2);
    g.fillRect(fx + 2, fy, 2, 2);
    g.fillRect(fx, fy - 2, 2, 2);
    g.fillRect(fx, fy + 2, 2, 2);
    g.fillStyle = "#ffd94a";
    g.fillRect(fx, fy, 2, 2);
  };
  for (let i = 0; i < 9; i++) {
    const fx = 14 + Math.floor(rnd() * (W - 28));
    const fy = 14 + Math.floor(rnd() * (H - 34));
    if (fy > pathY - 6 && fy < pathY + pathH + 5) continue;
    if (fx > pxx - 6 && fy > pyy - 6) continue;
    flower(fx, fy, rnd() < 0.5 ? "#ffffff" : "#ff8ab8");
  }

  // 바위 — 외곽선 + 하이라이트
  const rock = (rx: number, ry: number) => {
    g.fillStyle = "#6d7680";
    g.fillRect(rx, ry + 2, 12, 6);
    g.fillRect(rx + 2, ry, 8, 10);
    g.fillStyle = "#9aa3ab";
    g.fillRect(rx + 2, ry + 1, 8, 6);
    g.fillStyle = "#c2c9d0";
    g.fillRect(rx + 3, ry + 2, 3, 2);
  };
  rock(Math.floor(W * 0.16), H - 26);
  rock(Math.floor(W * 0.72), Math.floor(H * 0.34));

  // ── 꾸미기 오브젝트(상점 구매 시 지정 자리에 등장) — 꽃/바위 위에 덮어 그린다 ──
  if (decos.includes("decoSign")) {
    // 나무 표지판 — 흙길 왼쪽 위
    const sx = 20;
    const sy = pathY - 22;
    g.fillStyle = "#6b4222";
    g.fillRect(sx + 9, sy + 12, 4, 9); // 기둥
    g.fillRect(sx - 1, sy - 1, 24, 14); // 보드 외곽
    g.fillStyle = "#c89858";
    g.fillRect(sx, sy, 22, 12); // 보드
    g.fillStyle = "#8a6238";
    g.fillRect(sx + 3, sy + 3, 16, 2); // 글줄 1
    g.fillRect(sx + 3, sy + 7, 12, 2); // 글줄 2
  }
  if (decos.includes("decoFlowerbed")) {
    // 화단 — 좌하단, 흙 테두리 + 알록달록 꽃
    const bx = 18;
    const by = H - 46;
    g.fillStyle = "#6b4a2f";
    g.fillRect(bx - 2, by - 2, 48, 24); // 테두리
    g.fillStyle = "#8a6238";
    g.fillRect(bx, by, 44, 20); // 경작 흙
    g.fillStyle = "#7d5636";
    for (let x = 0; x < 44; x += 4) g.fillRect(bx + x, by + 9, 2, 2); // 고랑
    const petals = ["#ff6b6b", "#ffd94a", "#ffffff", "#ff8ab8", "#7cc0ea", "#c58aff"];
    for (let i = 0; i < 6; i++) {
      const fx = bx + 5 + (i % 3) * 14;
      const fy = by + 4 + Math.floor(i / 3) * 10;
      g.fillStyle = petals[i];
      g.fillRect(fx - 2, fy, 2, 2);
      g.fillRect(fx + 2, fy, 2, 2);
      g.fillRect(fx, fy - 2, 2, 2);
      g.fillRect(fx, fy + 2, 2, 2);
      g.fillStyle = "#ffd94a";
      g.fillRect(fx, fy, 2, 2);
    }
  }
  if (decos.includes("decoBench")) {
    // 나무 벤치 — 좌상단(울타리 바로 아래)
    const bx = 26;
    const by = 26;
    g.fillStyle = "#6b4222";
    g.fillRect(bx - 1, by - 1, 32, 5); // 등받이 외곽
    g.fillStyle = "#b07f48";
    g.fillRect(bx, by, 30, 3); // 등받이
    g.fillStyle = "#6b4222";
    g.fillRect(bx - 1, by + 6, 32, 8); // 좌판 외곽
    g.fillStyle = "#b07f48";
    g.fillRect(bx, by + 7, 30, 6); // 좌판
    g.fillStyle = "#8a5a30";
    g.fillRect(bx + 9, by + 7, 1, 6); // 판자 이음새
    g.fillRect(bx + 19, by + 7, 1, 6);
    g.fillStyle = "#4e3018";
    g.fillRect(bx + 2, by + 14, 3, 4); // 다리
    g.fillRect(bx + 25, by + 14, 3, 4);
  }
  if (decos.includes("decoFountain")) {
    // 분수 — 광장 중앙 명물: 돌 수반 + 물 + 기둥 + 물보라
    const fx = Math.floor(W / 2) - 21;
    const fy = Math.floor(H * 0.26);
    const basin = (inset: number, color: string) => {
      g.fillStyle = color;
      g.fillRect(fx + inset + 3, fy + inset, 42 - inset * 2 - 6, 30 - inset * 2);
      g.fillRect(fx + inset, fy + inset + 3, 42 - inset * 2, 30 - inset * 2 - 6);
    };
    basin(0, "#5f6a75"); // 외곽선
    basin(1, "#8d99a4"); // 돌 수반
    basin(4, "#4e9fd8"); // 물
    g.fillStyle = "#b8c2cc"; // 수반 상단 하이라이트
    g.fillRect(fx + 4, fy + 1, 34, 2);
    g.fillStyle = "#7cc0ea"; // 물 하이라이트
    g.fillRect(fx + 7, fy + 6, 8, 2);
    g.fillStyle = "#5f6a75"; // 중앙 기둥
    g.fillRect(fx + 17, fy + 8, 8, 12);
    g.fillStyle = "#8d99a4";
    g.fillRect(fx + 18, fy + 9, 6, 10);
    g.fillStyle = "#b8c2cc"; // 기둥 캡
    g.fillRect(fx + 15, fy + 7, 12, 3);
    g.fillStyle = "#a8d8f0"; // 물보라 기둥
    g.fillRect(fx + 19, fy + 1, 2, 6);
    g.fillRect(fx + 15, fy + 3, 2, 4);
    g.fillRect(fx + 25, fy + 3, 2, 4);
    g.fillStyle = "#e8f6ff"; // 물방울
    g.fillRect(fx + 13, fy + 1, 2, 2);
    g.fillRect(fx + 27, fy + 2, 2, 2);
    g.fillRect(fx + 20, fy - 2, 2, 2);
    g.fillStyle = "#a8d8f0"; // 수면 물결
    g.fillRect(fx + 8, fy + 22, 6, 1);
    g.fillRect(fx + 28, fy + 20, 6, 1);
  }

  // 관목 울타리(테두리 16px 타일) — 루트맵 가장자리 문법. 마지막에 그려 위를 덮는다.
  const bush = (bx: number, by: number) => {
    g.fillStyle = "#2f6b34";
    g.fillRect(bx, by, 16, 16);
    g.fillStyle = "#3f8a42";
    g.fillRect(bx + 1, by + 1, 14, 7);
    g.fillRect(bx + 2, by + 9, 5, 5);
    g.fillRect(bx + 9, by + 9, 5, 5);
    g.fillStyle = "#58a85c";
    g.fillRect(bx + 2, by + 2, 4, 2);
    g.fillRect(bx + 8, by + 3, 4, 2);
    g.fillRect(bx + 3, by + 10, 2, 2);
    g.fillRect(bx + 10, by + 10, 2, 2);
  };
  for (let x = 0; x < W; x += 16) {
    bush(x, 0);
    bush(x, H - 16);
  }
  for (let y = 16; y < H - 16; y += 16) {
    bush(0, y);
    bush(W - 16, y);
  }
}

/// 광장 배경 캔버스 — 부모 크기에 맞춰 그리고, 리사이즈/꾸미기 변경 시 다시 그린다(결정적이라 같은 맵).
function PlazaBackdrop({ decos }: { decos: string[] }) {
  const ref = useRef<HTMLCanvasElement | null>(null);
  const decosKey = decos.join(",");
  useEffect(() => {
    const cv = ref.current;
    const parent = cv?.parentElement;
    if (!cv || !parent) return;
    const list = decosKey ? decosKey.split(",") : [];
    const draw = () => {
      const r = parent.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) drawPlazaMap(cv, r.width, r.height, list);
    };
    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(parent);
    return () => ro.disconnect();
  }, [decosKey]);
  return <canvas className="plaza-canvas" ref={ref} />;
}

function PlazaTab({
  dex,
  pet,
  onPet,
}: {
  dex: DexView[];
  pet: CompanionView | null;
  onPet: (p: CompanionView) => void;
}) {
  const [shuffleKey, setShuffleKey] = useState(0);
  const fieldRef = useRef<HTMLDivElement | null>(null);
  const [mons, setMons] = useState<Resident[]>([]);
  // 장난감 — 틱 로직(공 차기)이 mons 업데이트와 얽혀 ref 로 들고, bump 로 렌더 동기화.
  const toyRef = useRef<Toy | null>(null);
  const [, bumpToy] = useState(0);
  const setToy = useCallback((t: Toy | null) => {
    toyRef.current = t;
    bumpToy((k) => k + 1);
  }, []);

  const throwToy = useCallback(
    async (kind: "toyBall" | "toyBalloon") => {
      if (toyRef.current) return; // 한 번에 하나
      try {
        const p = await invoke<CompanionView | null>("use_toy", { kind });
        if (!p) return; // 미보유
        onPet(p); // 보유 수 즉시 갱신
        const w = fieldRef.current?.clientWidth ?? 330;
        const h = fieldRef.current?.clientHeight ?? 380;
        const now = Date.now();
        if (kind === "toyBall") {
          setToy({
            kind: "ball",
            x: PLAZA_HEDGE + 10 + Math.random() * Math.max(40, w - PLAZA_HEDGE * 2 - 36),
            y: PLAZA_HEDGE + 30 + Math.random() * Math.max(40, h - PLAZA_HEDGE * 2 - 60),
            dur: 0.6,
            moveUntil: now + 800,
            until: now + TOY_LIFETIME,
          });
        } else {
          setToy({
            kind: "balloon",
            x: PLAZA_HEDGE + 20 + Math.random() * Math.max(40, w - PLAZA_HEDGE * 2 - 60),
            y: PLAZA_HEDGE + 4 + Math.random() * 36,
            dur: 0,
            moveUntil: now,
            until: now + TOY_LIFETIME,
          });
        }
      } catch {
        /* ignore */
      }
    },
    [onPet, setToy]
  );

  // dex 는 10초 폴링마다 새 배열이므로 내용 시그니처로 고정 — 내용이 실제로 바뀔 때만 재추첨.
  const dexSig = dex.map((d) => `${d.final_id}:${d.is_shiny ? 1 : 0}`).join(",");

  // 출연진 — 파트너(부화 후) + 도감에서 랜덤 표본. 파트너와 같은 종은 중복 제외.
  const roster = useMemo(() => {
    const list: { id: number; shiny: boolean; name: string; partner: boolean }[] = [];
    if (pet && !pet.is_egg && pet.species_id != null) {
      list.push({ id: pet.species_id, shiny: pet.is_shiny, name: pet.display_name, partner: true });
    }
    const pool = dex.filter((d) => !(list[0] && d.final_id === list[0].id && d.is_shiny === list[0].shiny));
    // Fisher–Yates 셔플(표본용) — 광장은 연출이라 비결정 랜덤이어도 무방.
    const arr = [...pool];
    for (let i = arr.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [arr[i], arr[j]] = [arr[j], arr[i]];
    }
    for (const d of arr.slice(0, PLAZA_MAX - list.length)) {
      list.push({ id: d.final_id, shiny: d.is_shiny, name: d.name, partner: false });
    }
    return list;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dexSig, pet?.species_id, pet?.is_shiny, pet?.is_egg, shuffleKey]);

  // 스폰 — 출연진이 바뀌면 랜덤 위치로 재배치.
  useEffect(() => {
    const w = fieldRef.current?.clientWidth ?? 330;
    const h = fieldRef.current?.clientHeight ?? 380;
    const now = Date.now();
    setMons(
      roster.map((r, i) => ({
        key: `${r.id}-${r.shiny}-${i}`,
        id: r.id,
        shiny: r.shiny,
        name: r.name,
        partner: r.partner,
        x: PLAZA_HEDGE - 8 + Math.random() * Math.max(40, w - PLAZA_SPRITE - PLAZA_HEDGE * 2 + 16),
        y: PLAZA_HEDGE - 6 + Math.random() * Math.max(40, h - PLAZA_SPRITE - PLAZA_HEDGE * 2 + 4),
        dur: 0,
        flip: Math.random() < 0.5,
        facing: "front" as const,
        gait: "walk" as const,
        action: "idle",
        bubble: null,
        busyUntil: now + Math.random() * 1800,
      }))
    );
  }, [roster]);

  // 행동 틱 — 한가해진 개체마다 다음 행동을 굴린다. 장난감이 있으면 그쪽으로 쏠린다.
  useEffect(() => {
    const t = setInterval(() => {
      const w = fieldRef.current?.clientWidth ?? 330;
      const h = fieldRef.current?.clientHeight ?? 380;
      const now0 = Date.now();

      // 장난감 수명·풍선 드리프트
      const toy0 = toyRef.current;
      if (toy0 && now0 > toy0.until) {
        setToy(null);
      } else if (toy0 && toy0.kind === "balloon" && now0 > toy0.moveUntil) {
        const nx = Math.min(Math.max(PLAZA_HEDGE + 8, toy0.x + (Math.random() * 120 - 60)), w - PLAZA_HEDGE - 24);
        const ny = PLAZA_HEDGE + 4 + Math.random() * 50;
        const dur = Math.max(1.6, Math.hypot(nx - toy0.x, ny - toy0.y) / 16);
        setToy({ ...toy0, x: nx, y: ny, dur, moveUntil: now0 + dur * 1000 + 600 });
      }

      setMons((prev) => {
        const now = Date.now();
        const next = prev.map((m) => ({ ...m }));
        const toy = toyRef.current;
        let kicked = false; // 한 틱에 공은 한 번만 차인다

        const walkTo = (m: Resident, nx0: number, ny0: number, gait0?: PlazaGait) => {
          const nx = Math.min(Math.max(PLAZA_HEDGE - 8, nx0), w - PLAZA_SPRITE - PLAZA_HEDGE + 8);
          const ny = Math.min(Math.max(PLAZA_HEDGE - 6, ny0), h - PLAZA_SPRITE - PLAZA_HEDGE - 2);
          const dx = nx - m.x;
          const dy = ny - m.y;
          // 걸음걸이 — 지정 없으면 뒤뚱 60% / 폴짝 25% / 질주 15%.
          const r = Math.random();
          const gait: PlazaGait = gait0 ?? (r < 0.6 ? "walk" : r < 0.85 ? "hop" : "run");
          const dur = Math.max(gait === "run" ? 0.5 : 0.8, Math.hypot(dx, dy) / GAIT_SPEED[gait]);
          m.flip = nx < m.x;
          // 위쪽(화면 안쪽)으로 가는 세로 위주 이동이면 뒷모습으로.
          m.facing = dy < 0 && Math.abs(dy) > Math.abs(dx) * 0.7 ? "back" : "front";
          m.gait = gait;
          m.x = nx;
          m.y = ny;
          m.dur = dur;
          m.action = "walk";
          m.bubble = null;
          m.busyUntil = now + dur * 1000 + 400;
        };

        for (const m of next) {
          if (now < m.busyUntil) continue;

          // 공 근처에서 한가해졌다 → 뻥 차기! (공이 새 자리로 굴러간다)
          if (
            toy &&
            toy.kind === "ball" &&
            !kicked &&
            now > toy.moveUntil &&
            Math.hypot(m.x + 28 - (toy.x + 8), m.y + 46 - (toy.y + 8)) < 46
          ) {
            const nx = Math.min(Math.max(PLAZA_HEDGE + 6, toy.x + (Math.random() * 180 - 90)), w - PLAZA_HEDGE - 22);
            const ny = Math.min(Math.max(PLAZA_HEDGE + 24, toy.y + (Math.random() * 120 - 60)), h - PLAZA_HEDGE - 20);
            // updater 안에서 ref 직접 갱신 — 이번 setMons 렌더에 함께 반영된다(StrictMode 이중 호출 시 시각적 이중 킥 정도로 무해).
            toyRef.current = { ...toy, x: nx, y: ny, dur: 0.7, moveUntil: now + 1000 };
            kicked = true;
            m.action = "play";
            m.bubble = "⚽";
            m.flip = nx < m.x;
            m.facing = "front";
            m.busyUntil = now + 1400;
            continue;
          }
          // 풍선 아래 도착 → 폴짝폴짝
          if (toy && toy.kind === "balloon" && Math.abs(m.x + 18 - toy.x) < 34) {
            m.action = "play";
            m.bubble = "🎈";
            m.dur = 0;
            m.facing = "front";
            m.busyUntil = now + 2200;
            continue;
          }

          const roll = Math.random();
          // 장난감이 있으면 우선 그쪽으로 몰려간다(신나서 주로 질주/폴짝).
          if (toy && roll < 0.55) {
            const excited: PlazaGait = Math.random() < 0.6 ? "run" : "hop";
            if (toy.kind === "ball") {
              walkTo(m, toy.x - 20 + (Math.random() * 16 - 8), toy.y - 38 + (Math.random() * 12 - 6), excited);
            } else {
              walkTo(m, toy.x - 18 + (Math.random() * 48 - 24), toy.y + 46 + Math.random() * 40, excited);
            }
            continue;
          }
          if (roll < 0.44) {
            // 산책 — 울타리 안쪽으로만
            walkTo(
              m,
              PLAZA_HEDGE - 8 + Math.random() * Math.max(40, w - PLAZA_SPRITE - PLAZA_HEDGE * 2 + 16),
              PLAZA_HEDGE - 6 + Math.random() * Math.max(40, h - PLAZA_SPRITE - PLAZA_HEDGE * 2 + 4)
            );
          } else if (roll < 0.58) {
            m.action = "sleep";
            m.bubble = "💤";
            m.dur = 0;
            m.facing = "front";
            m.busyUntil = now + 5000 + Math.random() * 5000;
          } else if (roll < 0.7) {
            m.action = "eat";
            m.bubble = plazaFoods[Math.floor(Math.random() * plazaFoods.length)];
            m.dur = 0;
            m.facing = "front";
            m.busyUntil = now + 3800;
          } else if (roll < 0.84) {
            m.action = "play";
            m.bubble = "🎵";
            m.dur = 0;
            m.facing = "front";
            m.busyUntil = now + 3000;
          } else {
            // 투닥투닥 — 가까이(130px)에 한가한 상대가 있으면 같이 싸운다.
            const other = next.find(
              (o) =>
                o.key !== m.key &&
                now >= o.busyUntil &&
                Math.hypot(o.x - m.x, o.y - m.y) < 130
            );
            if (other) {
              m.action = "fight";
              other.action = "fight";
              m.bubble = "💢";
              other.bubble = "⚔️";
              m.flip = other.x < m.x;
              other.flip = m.x < other.x;
              m.facing = "front";
              other.facing = "front";
              m.dur = 0;
              other.dur = 0;
              m.busyUntil = now + 3200;
              other.busyUntil = now + 3200;
            } else {
              m.action = "idle";
              m.bubble = null;
              m.facing = "front";
              m.busyUntil = now + 1500 + Math.random() * 2000;
            }
          }
        }
        return next;
      });
    }, 700);
    return () => clearInterval(t);
  }, [setToy]);

  // 클릭 — 이름표(다음 행동 전까지 잠깐).
  const poke = (key: string) => {
    setMons((prev) =>
      prev.map((m) =>
        m.key === key
          ? {
              ...m,
              bubble: `${m.name}${m.shiny ? "✨" : ""}`,
              busyUntil: Math.max(m.busyUntil, Date.now() + 1800),
            }
          : m
      )
    );
  };

  if (roster.length === 0) {
    return (
      <div className="pop-empty">
        광장에 나올 포켓몬이 아직 없어요.
        <br />
        알을 부화시키거나 야생 포켓몬을 잡아보세요!
      </div>
    );
  }

  return (
    <section className="plaza-wrap">
      <div className="plaza-head">
        <span>
          광장에 {roster.length}마리 놀러 나왔어요
          {dex.length > PLAZA_MAX && ` (도감 ${dex.length}마리)`}
        </span>
        <div className="plaza-actions">
          <button
            className="plaza-toy-btn"
            disabled={!!toyRef.current || (pet?.toy_ball_count ?? 0) <= 0}
            onClick={() => throwToy("toyBall")}
            title={
              (pet?.toy_ball_count ?? 0) > 0
                ? "공 던져주기"
                : "상점에서 장난감 공을 구매하세요"
            }
          >
            ⚽ ×{pet?.toy_ball_count ?? 0}
          </button>
          <button
            className="plaza-toy-btn"
            disabled={!!toyRef.current || (pet?.toy_balloon_count ?? 0) <= 0}
            onClick={() => throwToy("toyBalloon")}
            title={
              (pet?.toy_balloon_count ?? 0) > 0
                ? "풍선 띄우기"
                : "상점에서 풍선을 구매하세요"
            }
          >
            🎈 ×{pet?.toy_balloon_count ?? 0}
          </button>
          {dex.length + 1 > PLAZA_MAX && (
            <button className="plaza-shuffle" onClick={() => setShuffleKey((k) => k + 1)}>
              🔀
            </button>
          )}
        </div>
      </div>
      <div className="plaza" ref={fieldRef}>
        <PlazaBackdrop decos={pet?.decorations ?? []} />
        {toyRef.current && (
          <div
            className={`plaza-toy ${toyRef.current.kind}`}
            style={{
              left: toyRef.current.x,
              top: toyRef.current.y,
              zIndex: Math.round(toyRef.current.y) + 8,
              transition:
                toyRef.current.dur > 0
                  ? `left ${toyRef.current.dur}s ease-out, top ${toyRef.current.dur}s ease-out`
                  : "none",
            }}
          />
        )}
        {mons.map((m) => (
          <div
            className={`plaza-mon ${m.action} g-${m.gait}`}
            key={m.key}
            style={{
              left: m.x,
              top: m.y,
              zIndex: Math.round(m.y),
              transition: m.dur > 0 ? `left ${m.dur}s linear, top ${m.dur}s linear` : "none",
            }}
            onClick={() => poke(m.key)}
            title={m.name}
          >
            {m.bubble && <div className="plaza-bubble">{m.bubble}</div>}
            {m.partner && <div className="plaza-partner-mark">🐾</div>}
            {/* 걸음걸이 애니는 .gait 스팬(transform)에 — img 의 좌우반전(inline transform)과 분리 */}
            <span className="gait">
              <FallbackImg
                cands={m.facing === "back" ? backCandidates(m.id, m.shiny) : heroCandidates(m.id, m.shiny)}
                alt={m.name}
                flip={m.flip}
              />
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}

function ShopTab({
  pet,
  shop,
  onBuy,
  onUse,
}: {
  pet: CompanionView | null;
  shop: ShopItemView[];
  onBuy: (id: string) => void;
  onUse: (cmd: "use_rare_candy" | "use_mint") => void;
}) {
  return (
    <section className="shop-wrap">
      <div className="shop-balance">
        <span className="shop-balance-label">보유 토큰</span>
        <span className="shop-balance-value">{pet ? compact(pet.available_tokens) : "0"}</span>
      </div>

      {pet &&
        (pet.rare_candy > 0 ||
          pet.mint_count > 0 ||
          pet.great_ball_count > 0 ||
          pet.ultra_ball_count > 0) && (
        <div className="bag">
          <div className="bag-title">가방</div>
          <div className="bag-items">
            {pet.rare_candy > 0 && (
              <button
                className="bag-item"
                disabled={!pet.can_use_candy}
                onClick={() => onUse("use_rare_candy")}
                title={shopDesc.rareCandy}
              >
                🍬 이상한 사탕 ×{pet.rare_candy}
                <span className="bag-use">사용</span>
              </button>
            )}
            {pet.mint_count > 0 && (
              <button
                className="bag-item"
                disabled={!pet.can_use_mint}
                onClick={() => onUse("use_mint")}
                title={shopDesc.mint}
              >
                🌿 민트 ×{pet.mint_count}
                <span className="bag-use">사용</span>
              </button>
            )}
            {pet.great_ball_count > 0 && (
              <div className="bag-item static" title={shopDesc.greatBall}>
                🔵 수퍼볼 ×{pet.great_ball_count}
                <span className="bag-use muted">전투 시 자동 사용</span>
              </div>
            )}
            {pet.ultra_ball_count > 0 && (
              <div className="bag-item static" title={shopDesc.ultraBall}>
                🟡 하이퍼볼 ×{pet.ultra_ball_count}
                <span className="bag-use muted">전투 시 자동 사용</span>
              </div>
            )}
          </div>
        </div>
      )}

      <div className="shop-list">
        {shop.map((s) => (
          <div className={`shop-row ${s.done ? "done" : ""}`} key={s.id}>
            <span className="shop-emoji">{s.emoji}</span>
            <div className="shop-info">
              <span className="shop-name">{s.name}</span>
              <span className="shop-desc">{shopDesc[s.id] ?? ""}</span>
            </div>
            <div className="shop-right">
              <span className="shop-price">{s.done ? "보유 중" : `${compact(s.price)} 토큰`}</span>
              <button
                className="shop-buy"
                disabled={s.done || !s.affordable}
                onClick={() => onBuy(s.id)}
              >
                {s.done ? "✓ 보유" : "구매"}
              </button>
            </div>
          </div>
        ))}
        {shop.length === 0 && <div className="pop-empty">상점 준비 중…</div>}
      </div>
    </section>
  );
}

function Toggle({
  on,
  onChange,
}: {
  on: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button className={`toggle ${on ? "on" : ""}`} onClick={() => onChange(!on)} role="switch" aria-checked={on}>
      <span className="toggle-knob" />
    </button>
  );
}

function SettingsTab({
  lang,
  onLang,
  pet,
  settings,
  onAutostart,
  onSave,
  onExport,
  onImport,
  backupMsg,
}: {
  lang: Lang;
  onLang: (l: Lang) => void;
  pet: CompanionView | null;
  settings: SettingsView | null;
  onAutostart: (v: boolean) => void;
  onSave: (s: SettingsView) => void;
  onExport: () => void;
  onImport: () => void;
  backupMsg: string | null;
}) {
  const langs: { id: Lang; label: string }[] = [
    { id: "ko", label: "한국어" },
    { id: "en", label: "English" },
    { id: "ja", label: "日本語" },
  ];
  return (
    <section className="settings-wrap">
      <div className="set-group">
        <div className="set-label">언어</div>
        <div className="set-seg">
          {langs.map((l) => (
            <button
              key={l.id}
              className={`seg-btn ${lang === l.id ? "active" : ""}`}
              onClick={() => onLang(l.id)}
            >
              {l.label}
            </button>
          ))}
        </div>
      </div>

      <div className="set-group">
        <div className="set-label">시작·알림</div>
        <div className="set-toggle-row">
          <span>Windows 시작 시 자동 실행</span>
          <Toggle on={settings?.autostart ?? false} onChange={onAutostart} />
        </div>
        <div className="set-toggle-row">
          <span>한도 경고 알림</span>
          <Toggle
            on={settings?.limit_notifications ?? true}
            onChange={(v) => settings && onSave({ ...settings, limit_notifications: v })}
          />
        </div>
        {settings && settings.limit_notifications && (
          <>
            <div className="set-slider-row">
              <span>경고선</span>
              <input
                type="range"
                min={50}
                max={100}
                value={settings.warn_threshold}
                onChange={(e) => onSave({ ...settings, warn_threshold: Number(e.target.value) })}
              />
              <span className="set-slider-val">{settings.warn_threshold}%</span>
            </div>
            <div className="set-slider-row">
              <span>위험선</span>
              <input
                type="range"
                min={50}
                max={100}
                value={settings.crit_threshold}
                onChange={(e) => onSave({ ...settings, crit_threshold: Number(e.target.value) })}
              />
              <span className="set-slider-val">{settings.crit_threshold}%</span>
            </div>
          </>
        )}
      </div>

      <div className="set-group">
        <div className="set-label">컴패니언</div>
        <div className="set-info-row">
          <span>도감 수집</span>
          <span>{pet?.dex_count ?? 0}마리</span>
        </div>
        <div className="set-info-row">
          <span>전투 포획</span>
          <span>{pet?.caught_count ?? 0}마리</span>
        </div>
        <div className="set-info-row">
          <span>빛나는 부적</span>
          <span>{pet?.owns_shiny_charm ? "보유 ✨" : "미보유"}</span>
        </div>
      </div>

      <div className="set-group">
        <div className="set-label">저장 데이터 (백업/복원)</div>
        <div className="backup-btns">
          <button className="backup-btn" onClick={onExport}>
            ⬇️ 백업 내보내기
          </button>
          <button className="backup-btn" onClick={onImport}>
            ⬆️ 백업 복원
          </button>
        </div>
        {backupMsg && <div className="backup-msg">{backupMsg}</div>}
        <div className="set-note">
          진행도는 자동 저장돼요(앱 데이터 폴더). 다른 PC로 옮기거나 공유하려면 백업 파일(문서 폴더의
          PokeTokenBar-save.json)을 복사한 뒤 복원하세요.
        </div>
      </div>

      <div className="set-group">
        <div className="set-label">정보</div>
        <div className="set-info-row">
          <span>PokeTokenBar</span>
          <span className="muted">Windows · Tauri</span>
        </div>
      </div>
    </section>
  );
}

function Gauge({ g }: { g: LimitGauge }) {
  const pct = Math.max(0, Math.min(100, g.used_percent));
  const hue = pct < 60 ? "ok" : pct < 85 ? "warn" : "danger";
  // as_of 가 있고 비활성이면 stale(창 리셋됨/오래된 값) — 흐리게 + 기록시각 표시.
  const stale = !!g.as_of && !g.is_active;
  return (
    <div className={`gauge${stale ? " stale" : ""}`}>
      <div className="gauge-top">
        <span className="gauge-label">
          {g.label}
          {g.is_active && <span className="gauge-dot" title="적용 중" />}
          {stale && <span className="gauge-stale" title="이 값이 기록된 시각(창이 이미 리셋됐을 수 있음)">{relAgo(g.as_of)}</span>}
        </span>
        <span className="gauge-pct">{pct % 1 === 0 ? pct : pct.toFixed(1)}%</span>
      </div>
      <div className="gauge-track">
        <div className={`gauge-fill ${hue}`} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export default App;
