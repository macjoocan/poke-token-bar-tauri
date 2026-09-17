//! PokéAPI 클라이언트 — 종/진화체인 fetch + 파싱. 원본 `PokeAPIClient.swift` 이식 (blocking).
//!
//! `CompanionStore` 의 `PokeProvider`(sync) 구현체. reqwest blocking 사용.
//! - 진화라인: pokemon-species/{id} → evolution_chain → 트리 + 희귀도 + 다국어 이름
//! - base 인덱스: GraphQL 1쿼리(1~5세대, 메타몽 제외) + 30일 디스크 캐시, 실패 시 REST 폴백(store 가 per-hatch 처리)
//! 포켓몬 데이터는 레포에 번들하지 않는다(런타임 fetch).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::companion_model::{pokemon_odds, EvoLine, EvoNode, Rarity};
use super::companion_store::{BaseSpecies, PokeProvider};

const API_BASE: &str = "https://pokeapi.co/api/v2";
const GRAPHQL_URL: &str = "https://graphql.pokeapi.co/v1beta2";
const MAX_BASE_ID: i64 = 649; // Gen-V 애니메이션 스프라이트 상한
const LANG_CODES: [&str; 4] = ["ko", "en", "ja-Hrkt", "ja"];
const BASE_INDEX_TTL_SECS: i64 = 30 * 86400;

pub struct PokeApiClient {
    client: reqwest::blocking::Client,
    base_index_file: PathBuf,
    species_cache: Mutex<HashMap<i64, SpeciesDto>>,
    line_cache: Mutex<HashMap<i64, EvoLine>>,
    base_index_cache: Mutex<Option<Vec<BaseSpecies>>>,
}

impl PokeApiClient {
    pub fn new(app_data_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&app_data_dir);
        PokeApiClient {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("PokeTokenBar-win/0.1")
                .build()
                .expect("reqwest client"),
            base_index_file: app_data_dir.join("base-index.json"),
            species_cache: Mutex::new(HashMap::new()),
            line_cache: Mutex::new(HashMap::new()),
            base_index_cache: Mutex::new(None),
        }
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> anyhow::Result<T> {
        let resp = self.client.get(url).send()?;
        if !resp.status().is_success() {
            anyhow::bail!("GET {} → HTTP {}", url, resp.status());
        }
        Ok(resp.json::<T>()?)
    }

    fn species(&self, id: i64) -> anyhow::Result<SpeciesDto> {
        if let Some(c) = self.species_cache.lock().unwrap().get(&id) {
            return Ok(c.clone());
        }
        let dto: SpeciesDto = self.get_json(&format!("{}/pokemon-species/{}", API_BASE, id))?;
        self.species_cache.lock().unwrap().insert(id, dto.clone());
        Ok(dto)
    }

    fn fetch_base_index(&self) -> anyhow::Result<Vec<BaseSpecies>> {
        // evolves_from IS NULL(=base) + id ≤ 649 + 메타몽(#132) 제외.
        let query = format!(
            "{{ pokemonspecies(where: {{evolves_from_species_id: {{_is_null: true}}, id: {{_lte: {}, _neq: {}}}}}, order_by: {{id: asc}}) {{ id capture_rate }} }}",
            MAX_BASE_ID,
            pokemon_odds::DITTO_SPECIES_ID
        );
        let resp = self
            .client
            .post(GRAPHQL_URL)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(&serde_json::json!({ "query": query }))?)
            .send()?;
        if !resp.status().is_success() {
            anyhow::bail!("GraphQL base index → HTTP {}", resp.status());
        }
        let decoded: GraphQlBaseResponse = resp.json()?;
        let entries: Vec<BaseSpecies> = decoded
            .data
            .pokemonspecies
            .into_iter()
            .map(|r| BaseSpecies { id: r.id, capture_rate: r.capture_rate })
            .collect();
        if entries.is_empty() {
            anyhow::bail!("GraphQL base index empty");
        }
        Ok(entries)
    }

    fn read_disk_index(&self) -> Option<BaseIndexSnapshot> {
        let data = std::fs::read(&self.base_index_file).ok()?;
        serde_json::from_slice(&data).ok()
    }
    fn write_disk_index(&self, entries: &[BaseSpecies]) {
        let snap = BaseIndexSnapshot {
            fetched_at_unix: chrono::Utc::now().timestamp(),
            entries: entries.to_vec(),
        };
        if let Ok(data) = serde_json::to_vec(&snap) {
            let _ = std::fs::write(&self.base_index_file, data);
        }
    }
}

impl PokeProvider for PokeApiClient {
    fn line(&self, base_species_id: i64) -> anyhow::Result<EvoLine> {
        if let Some(cached) = self.line_cache.lock().unwrap().get(&base_species_id) {
            return Ok(cached.clone());
        }
        let base = self.species(base_species_id)?;
        let chain_url = validated_chain_url(&base.evolution_chain.url)
            .ok_or_else(|| anyhow::anyhow!("bad/foreign evolution_chain url"))?;
        let chain: ChainDto = self.get_json(&chain_url)?;
        let tree = node_from(&chain.chain);
        let rarity = Rarity::from(base.capture_rate as i32, base.is_legendary, base.is_mythical);
        let mut names: HashMap<i64, HashMap<String, String>> = HashMap::new();
        for id in all_ids(&tree) {
            let sp = self.species(id)?;
            let mut by_lang = HashMap::new();
            for n in &sp.names {
                if LANG_CODES.contains(&n.language.name.as_str()) {
                    by_lang.insert(n.language.name.clone(), n.name.clone());
                }
            }
            names.insert(id, by_lang);
        }
        let line = EvoLine { base_id: base_species_id, tree, rarity, names };
        self.line_cache.lock().unwrap().insert(base_species_id, line.clone());
        Ok(line)
    }

    fn base_species_index(&self) -> anyhow::Result<Vec<BaseSpecies>> {
        if let Some(c) = self.base_index_cache.lock().unwrap().as_ref() {
            return Ok(c.clone());
        }
        let disk = self.read_disk_index();
        let now = chrono::Utc::now().timestamp();
        if let Some(d) = &disk {
            if now - d.fetched_at_unix < BASE_INDEX_TTL_SECS && !d.entries.is_empty() {
                *self.base_index_cache.lock().unwrap() = Some(d.entries.clone());
                return Ok(d.entries.clone());
            }
        }
        match self.fetch_base_index() {
            Ok(entries) => {
                *self.base_index_cache.lock().unwrap() = Some(entries.clone());
                self.write_disk_index(&entries);
                Ok(entries)
            }
            Err(e) => {
                // 오프라인 — 오래된 디스크 인덱스라도 사용(store 의 per-hatch REST 폴백이 없을 때 대비).
                if let Some(d) = disk {
                    if !d.entries.is_empty() {
                        *self.base_index_cache.lock().unwrap() = Some(d.entries.clone());
                        return Ok(d.entries);
                    }
                }
                Err(e)
            }
        }
    }

    fn base_species(&self, id: i64) -> anyhow::Result<Option<BaseSpecies>> {
        if id == pokemon_odds::DITTO_SPECIES_ID {
            return Ok(None); // 메타몽은 위장 리빌 전용
        }
        let dto = self.species(id)?;
        if dto.evolves_from_species.is_some() {
            return Ok(None); // 진화 중간체 — 부화 후보 아님
        }
        Ok(Some(BaseSpecies { id, capture_rate: dto.capture_rate }))
    }
}

// ── URL/트리 유틸 ──

/// 진화체인 URL 검증(SSRF 가드) — https + pokeapi.co 고정.
fn validated_chain_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    if url.scheme() == "https" && url.host_str() == Some("pokeapi.co") {
        Some(url.to_string())
    } else {
        None
    }
}

fn node_from(link: &ChainLink) -> EvoNode {
    EvoNode::new(
        id_from_species_url(link.species.url.as_deref().unwrap_or("")),
        link.evolves_to.iter().map(node_from).collect(),
    )
}
fn all_ids(n: &EvoNode) -> Vec<i64> {
    let mut v = vec![n.species_id];
    for c in &n.children {
        v.extend(all_ids(c));
    }
    v
}
fn id_from_species_url(url: &str) -> i64 {
    url.split('/').filter(|s| !s.is_empty()).next_back().and_then(|s| s.parse().ok()).unwrap_or(0)
}

// ── DTO ──

#[derive(Debug, Clone, Deserialize)]
struct SpeciesDto {
    capture_rate: i64,
    #[serde(default)]
    is_legendary: bool,
    #[serde(default)]
    is_mythical: bool,
    names: Vec<NameDto>,
    evolution_chain: UrlRef,
    evolves_from_species: Option<NamedRef>,
}
#[derive(Debug, Clone, Deserialize)]
struct NameDto {
    name: String,
    language: NamedRef,
}
#[derive(Debug, Clone, Deserialize)]
struct NamedRef {
    name: String,
    #[serde(default)]
    url: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
struct UrlRef {
    url: String,
}
#[derive(Debug, Clone, Deserialize)]
struct ChainDto {
    chain: ChainLink,
}
#[derive(Debug, Clone, Deserialize)]
struct ChainLink {
    species: NamedRef,
    evolves_to: Vec<ChainLink>,
}

#[derive(Debug, Deserialize)]
struct GraphQlBaseResponse {
    data: GraphQlData,
}
#[derive(Debug, Deserialize)]
struct GraphQlData {
    pokemonspecies: Vec<GraphQlRow>,
}
#[derive(Debug, Deserialize)]
struct GraphQlRow {
    id: i64,
    capture_rate: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct BaseIndexSnapshot {
    fetched_at_unix: i64,
    entries: Vec<BaseSpecies>, // companion_store::BaseSpecies 는 serde derive 보유
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_from_url_parses_trailing_id() {
        assert_eq!(id_from_species_url("https://pokeapi.co/api/v2/pokemon-species/25/"), 25);
        assert_eq!(id_from_species_url("https://pokeapi.co/api/v2/pokemon-species/133"), 133);
        assert_eq!(id_from_species_url(""), 0);
    }

    #[test]
    fn chain_url_ssrf_guard() {
        assert!(validated_chain_url("https://pokeapi.co/api/v2/evolution-chain/10/").is_some());
        assert!(validated_chain_url("http://pokeapi.co/api/v2/evolution-chain/10/").is_none()); // http 거부
        assert!(validated_chain_url("https://evil.com/x").is_none()); // 타 호스트 거부
        assert!(validated_chain_url("not a url").is_none());
    }

    #[test]
    fn node_from_builds_tree() {
        // 1 → {2 → 3, 4}
        let chain = ChainLink {
            species: NamedRef { name: "a".into(), url: Some("/pokemon-species/1/".into()) },
            evolves_to: vec![
                ChainLink {
                    species: NamedRef { name: "b".into(), url: Some("/pokemon-species/2/".into()) },
                    evolves_to: vec![ChainLink {
                        species: NamedRef { name: "c".into(), url: Some("/pokemon-species/3/".into()) },
                        evolves_to: vec![],
                    }],
                },
                ChainLink {
                    species: NamedRef { name: "d".into(), url: Some("/pokemon-species/4/".into()) },
                    evolves_to: vec![],
                },
            ],
        };
        let tree = node_from(&chain);
        assert_eq!(tree.species_id, 1);
        assert_eq!(tree.depth(), 3);
        let mut finals = tree.final_ids();
        finals.sort();
        assert_eq!(finals, vec![3, 4]);
    }
}
