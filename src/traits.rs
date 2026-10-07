//! チャンピオン特性表とロール出現率の読み込み。表に無いチャンピオンは Data Dragon のタグから既定値を作る。
use crate::champions::ChampionCatalog;
use crate::models::POSITIONS;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub fn trait_label(term: &str) -> &'static str {
    match term {
        "early" => "序盤(Lv1-5)の強さ",
        "spike6" => "Lv6スパイク",
        "waveclear" => "ウェーブクリア",
        "mobility" => "機動力",
        "clear" => "クリア速度",
        "gank" => "ガンク力",
        "duel" => "インベード耐性",
        _ => "?",
    }
}

type Row = [f64; 7];
const DEFAULT_TRAITS: Row = [3.0; 7];

fn tag_traits(tag: &str) -> Row {
    match tag {
        "Marksman" => [3., 3., 3., 2., 1., 1., 1.],
        "Mage" => [3., 4., 4., 1., 1., 2., 1.],
        "Assassin" => [3., 4., 3., 4., 3., 4., 3.],
        "Fighter" => [3., 4., 3., 3., 3., 3., 4.],
        "Tank" => [3., 3., 3., 2., 3., 3., 3.],
        "Support" => [3., 3., 1., 2., 1., 2., 1.],
        _ => DEFAULT_TRAITS,
    }
}

fn tag_roles(tag: &str) -> Option<Vec<(&'static str, f64)>> {
    Some(match tag {
        "Marksman" => vec![("bottom", 0.9), ("middle", 0.1)],
        "Mage" => vec![("middle", 0.7), ("utility", 0.3)],
        "Assassin" => vec![("middle", 0.6), ("jungle", 0.4)],
        "Fighter" => vec![("top", 0.7), ("jungle", 0.3)],
        "Tank" => vec![("top", 0.5), ("jungle", 0.25), ("utility", 0.25)],
        "Support" => vec![("utility", 1.0)],
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Traits {
    pub early: f64,
    pub spike6: f64,
    pub waveclear: f64,
    pub mobility: f64,
    pub clear: f64,
    pub gank: f64,
    pub duel: f64,
    pub estimated: bool,
}

impl Traits {
    fn new(v: Row, estimated: bool) -> Self {
        Traits { early: v[0], spike6: v[1], waveclear: v[2], mobility: v[3], clear: v[4], gank: v[5], duel: v[6], estimated }
    }

    pub fn get(&self, term: &str) -> f64 {
        match term {
            "early" => self.early,
            "spike6" => self.spike6,
            "waveclear" => self.waveclear,
            "mobility" => self.mobility,
            "clear" => self.clear,
            "gank" => self.gank,
            "duel" => self.duel,
            _ => panic!("unknown trait: {term}"),
        }
    }
}

#[derive(Deserialize)]
struct TraitsJson {
    traits: HashMap<String, Row>,
    #[serde(default, rename = "_estimated")]
    estimated: Vec<String>,
}

#[derive(Deserialize)]
struct RolesJson {
    #[serde(default, rename = "_fields")]
    fields: Option<Vec<String>>,
    rates: HashMap<String, Vec<f64>>,
}

pub struct TraitTable {
    traits: HashMap<String, Row>,
    estimated: HashSet<String>,
    roles: HashMap<String, HashMap<String, f64>>,
}

impl TraitTable {
    pub fn from_json(traits_json: &str, roles_json: &str) -> Result<Self, serde_json::Error> {
        let t: TraitsJson = serde_json::from_str(traits_json)?;
        let r: RolesJson = serde_json::from_str(roles_json)?;
        let fields = r.fields.unwrap_or_else(|| POSITIONS.iter().map(|s| s.to_string()).collect());
        let mut roles = HashMap::new();
        for (cid, values) in r.rates {
            let sum: f64 = values.iter().sum();
            let total = if sum == 0.0 { 1.0 } else { sum };
            roles.insert(cid, fields.iter().cloned().zip(values.iter().map(|v| v / total)).collect());
        }
        Ok(TraitTable { traits: t.traits, estimated: t.estimated.into_iter().collect(), roles })
    }

    pub fn load(data_dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let t = std::fs::read_to_string(data_dir.join("champion_traits.json"))?;
        let r = std::fs::read_to_string(data_dir.join("role_rates.json"))?;
        Ok(Self::from_json(&t, &r)?)
    }

    pub fn traits(&self, cid: &str, catalog: &ChampionCatalog) -> Traits {
        if let Some(v) = self.traits.get(cid) {
            return Traits::new(*v, self.estimated.contains(cid));
        }
        let row = catalog.tags(cid).first().map_or(DEFAULT_TRAITS, |t| tag_traits(t));
        Traits::new(row, true)
    }

    pub fn role_rates(&self, cid: &str, catalog: &ChampionCatalog) -> HashMap<String, f64> {
        if let Some(r) = self.roles.get(cid) {
            return r.clone();
        }
        if let Some(r) = catalog.tags(cid).first().and_then(|t| tag_roles(t)) {
            return r.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        }
        POSITIONS.iter().map(|p| (p.to_string(), 0.2)).collect()
    }
}
