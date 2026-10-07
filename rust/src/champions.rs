//! チャンピオン一覧（Data Dragon）の読み込み。
//!
//! exe に埋め込んだ同梱データを基本にし、起動時に Data Dragon から最新版を取れたら
//! %APPDATA% 側のキャッシュに保存して差し替える。
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub const DEFAULT_RANGE: i64 = 300;

#[derive(Debug, Clone, Deserialize)]
pub struct Champion {
    pub key: i64,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub attackrange: i64,
}

#[derive(Debug, Deserialize)]
struct Raw {
    #[serde(default)]
    version: String,
    champions: HashMap<String, Champion>,
}

#[derive(Debug, Clone)]
pub struct ChampionCatalog {
    pub version: String,
    champs: HashMap<String, Champion>,
    by_key: HashMap<i64, String>,
    by_lower: HashMap<String, String>,
    by_name: HashMap<String, String>,
}

fn version_tuple(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

impl ChampionCatalog {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let raw: Raw = serde_json::from_str(text)?;
        let by_key = raw.champions.iter().map(|(id, c)| (c.key, id.clone())).collect();
        let by_lower = raw.champions.keys().map(|id| (id.to_lowercase(), id.clone())).collect();
        let by_name = raw.champions.iter().map(|(id, c)| (c.name.clone(), id.clone())).collect();
        Ok(ChampionCatalog { version: raw.version, champs: raw.champions, by_key, by_lower, by_name })
    }

    /// 同梱データとキャッシュのうち、バージョンが新しい方を使う。
    pub fn load(bundled: &Path, cache: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_from_str(&std::fs::read_to_string(bundled)?, cache)
    }

    pub fn load_from_str(bundled: &str, cache: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let bundled = Self::from_json(bundled)?;
        if let Some(cache) = cache {
            if let Ok(text) = std::fs::read_to_string(cache) {
                if let Ok(cached) = Self::from_json(&text) {
                    if version_tuple(&cached.version) > version_tuple(&bundled.version) {
                        return Ok(cached);
                    }
                }
            }
        }
        Ok(bundled)
    }

    pub fn ids(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.champs.keys().map(|s| s.as_str()).collect();
        v.sort_unstable();
        v
    }

    pub fn id_by_key(&self, key: Option<i64>) -> Option<&str> {
        let k = key.filter(|k| *k != 0)?;
        self.by_key.get(&k).map(|s| s.as_str())
    }

    pub fn name<'a>(&'a self, cid: &'a str) -> &'a str {
        match self.champs.get(cid) {
            Some(c) => &c.name,
            None => cid,
        }
    }

    pub fn attack_range(&self, cid: &str) -> i64 {
        self.champs.get(cid).map_or(DEFAULT_RANGE, |c| c.attackrange)
    }

    pub fn tags(&self, cid: &str) -> &[String] {
        self.champs.get(cid).map_or(&[], |c| c.tags.as_slice())
    }

    /// "Darius" / "darius" / "game_character_displayname_Darius" / "ダリウス" など → id
    pub fn resolve(&self, text: Option<&str>) -> Option<&str> {
        let mut t = text?.trim();
        if t.is_empty() {
            return None;
        }
        if let Some(rest) = t.strip_prefix("game_character_displayname_") {
            t = rest;
        }
        if let Some((id, _)) = self.champs.get_key_value(t) {
            return Some(id);
        }
        if let Some(id) = self.by_lower.get(&t.to_lowercase()) {
            return Some(id);
        }
        self.by_name.get(t).map(|s| s.as_str())
    }
}

const VERSIONS_URL: &str = "https://ddragon.leagueoflegends.com/api/versions.json";

pub fn cache_path() -> std::path::PathBuf {
    crate::config::user_dir().join("champions_ja.json")
}

/// Data Dragon から最新のチャンピオン一覧を取って、アプリ用の軽い形（同梱データと同じ形）に変換する。
pub fn fetch_ddragon(locale: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build()
        .into();
    let versions: Value = agent.get(VERSIONS_URL).call()?.body_mut().read_json()?;
    let version = versions[0].as_str().ok_or("no version")?.to_string();
    let url = format!("https://ddragon.leagueoflegends.com/cdn/{version}/data/{locale}/champion.json");
    let raw: Value = agent.get(&url).call()?.body_mut().with_config().limit(50 * 1024 * 1024).read_json()?;
    let mut champs = serde_json::Map::new();
    for (cid, c) in raw["data"].as_object().ok_or("no data")? {
        champs.insert(
            cid.clone(),
            json!({
                "key": c["key"].as_str().and_then(|k| k.parse::<i64>().ok()).ok_or("bad key")?,
                "name": c["name"],
                "tags": c["tags"],
                "attackrange": c["stats"]["attackrange"].as_f64().ok_or("bad range")? as i64,
            }),
        );
    }
    Ok(json!({ "version": version, "champions": champs }))
}

/// 最新版を取得してキャッシュに保存する。失敗したら None。
pub fn refresh_cache() -> Option<ChampionCatalog> {
    let data = fetch_ddragon("ja_JP").ok()?;
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, data.to_string());
    ChampionCatalog::from_json(&data.to_string()).ok()
}
