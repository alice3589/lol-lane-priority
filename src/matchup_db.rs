//! マッチアップ実績DB（SQLite）。旧 Python 版と同じスキーマなので、そちらで集計した .sqlite もそのまま読める。
use crate::models::{POSITIONS, TIERS};
use rusqlite::{params, params_from_iter, Connection};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS matchups (
    patch TEXT NOT NULL,
    tier TEXT NOT NULL,
    lane TEXT NOT NULL,
    champ TEXT NOT NULL,
    opp TEXT NOT NULL,
    games INTEGER NOT NULL DEFAULT 0,
    sum_gd10 REAL NOT NULL DEFAULT 0,
    sum_xpd10 REAL NOT NULL DEFAULT 0,
    sum_csd10 REAL NOT NULL DEFAULT 0,
    solo_kills INTEGER NOT NULL DEFAULT 0,
    solo_deaths INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (patch, tier, lane, champ, opp)
);
CREATE TABLE IF NOT EXISTS role_counts (
    patch TEXT NOT NULL,
    tier TEXT NOT NULL,
    champ TEXT NOT NULL,
    position TEXT NOT NULL,
    games INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (patch, tier, champ, position)
);
CREATE TABLE IF NOT EXISTS processed_matches (
    match_id TEXT PRIMARY KEY,
    patch TEXT,
    tier TEXT
);
";

/// DB のロール出現率を信用するのに必要な試合数
pub const ROLE_RATE_MIN_GAMES: i64 = 50;

#[derive(Debug, Clone)]
pub struct MatchupStats {
    pub games: i64,
    pub gd10: f64,
    pub xpd10: f64,
    pub csd10: f64,
    pub solo_kill_rate: f64,
    pub solo_death_rate: f64,
    pub tiers: Vec<String>,
}

/// 1試合・1レーン・片方向分の集計行（batch から渡される）。
#[derive(Debug, Clone)]
pub struct LaneRow {
    pub lane: String,
    pub champ: String,
    pub opp: String,
    pub gd10: f64,
    pub xpd10: f64,
    pub csd10: f64,
    pub solo_kills: i64,
    pub solo_deaths: i64,
}

fn patch_key(patch: &str) -> Vec<u64> {
    patch.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

/// ランク帯の探索順。サンプルが足りなければ隣のランク帯を1つずつ足していく。
pub fn tier_search_order(tier: &str) -> Vec<Vec<String>> {
    let Some(idx) = TIERS.iter().position(|t| *t == tier) else {
        return vec![TIERS.iter().map(|s| s.to_string()).collect()];
    };
    let mut order = vec![TIERS[idx].to_string()];
    for d in 1..TIERS.len() {
        for j in [idx as i64 - d as i64, (idx + d) as i64] {
            if j >= 0 && (j as usize) < TIERS.len() {
                order.push(TIERS[j as usize].to_string());
            }
        }
    }
    (0..order.len()).map(|k| order[..=k].to_vec()).collect()
}

type LookupKey = (String, String, String, String, i64, Option<Vec<String>>);

pub struct MatchupDB {
    conn: Connection,
    cache: RefCell<HashMap<LookupKey, Option<MatchupStats>>>,
    role_cache: RefCell<HashMap<(String, Option<Vec<String>>), (HashMap<String, f64>, i64)>>,
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

impl MatchupDB {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(MatchupDB { conn, cache: Default::default(), role_cache: Default::default() })
    }

    pub fn open_if_exists(path: impl AsRef<Path>) -> Option<Self> {
        path.as_ref().exists().then(|| Self::open(path).ok()).flatten()
    }

    pub fn clear_cache(&self) {
        self.cache.borrow_mut().clear();
        self.role_cache.borrow_mut().clear();
    }

    // ---- 書き込み（batch 用） ----

    pub fn has_match(&self, match_id: &str) -> rusqlite::Result<bool> {
        let mut st = self.conn.prepare("SELECT 1 FROM processed_matches WHERE match_id = ?")?;
        st.exists(params![match_id])
    }

    /// 1試合分を加算する。すでに取り込み済みなら何もせず false。
    pub fn add_match(
        &mut self,
        match_id: &str,
        patch: &str,
        tier: &str,
        rows: &[LaneRow],
        roles: &[(String, String)],
    ) -> rusqlite::Result<bool> {
        let tx = self.conn.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO processed_matches (match_id, patch, tier) VALUES (?, ?, ?)",
            params![match_id, patch, tier],
        )?;
        if inserted == 0 {
            return Ok(false);
        }
        for r in rows {
            tx.execute(
                "INSERT INTO matchups (patch, tier, lane, champ, opp, games, sum_gd10, sum_xpd10,
                                       sum_csd10, solo_kills, solo_deaths)
                 VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?)
                 ON CONFLICT (patch, tier, lane, champ, opp) DO UPDATE SET
                    games = games + 1,
                    sum_gd10 = sum_gd10 + excluded.sum_gd10,
                    sum_xpd10 = sum_xpd10 + excluded.sum_xpd10,
                    sum_csd10 = sum_csd10 + excluded.sum_csd10,
                    solo_kills = solo_kills + excluded.solo_kills,
                    solo_deaths = solo_deaths + excluded.solo_deaths",
                params![patch, tier, r.lane, r.champ, r.opp, r.gd10, r.xpd10, r.csd10, r.solo_kills, r.solo_deaths],
            )?;
        }
        for (champ, position) in roles {
            tx.execute(
                "INSERT INTO role_counts (patch, tier, champ, position, games) VALUES (?, ?, ?, ?, 1)
                 ON CONFLICT (patch, tier, champ, position) DO UPDATE SET games = games + 1",
                params![patch, tier, champ, position],
            )?;
        }
        tx.commit()?;
        self.clear_cache();
        Ok(true)
    }

    // ---- 読み込み（アプリ用） ----

    pub fn patches(&self) -> Vec<String> {
        let mut out: Vec<String> = (|| -> rusqlite::Result<Vec<String>> {
            let mut st = self.conn.prepare("SELECT DISTINCT patch FROM matchups")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect()
        })()
        .unwrap_or_default();
        out.sort_by_key(|p| patch_key(p));
        out
    }

    /// 直近 n パッチ。n <= 0 なら None（＝全パッチ）。
    pub fn recent_patches(&self, n: i64) -> Option<Vec<String>> {
        if n <= 0 {
            return None;
        }
        let all = self.patches();
        let start = all.len().saturating_sub(n as usize);
        Some(all[start..].to_vec())
    }

    fn sum(&self, lane: &str, champ: &str, opp: &str, tiers: Option<&[String]>, patches: Option<&[String]>)
        -> (i64, f64, f64, f64, i64, i64)
    {
        let mut sql = String::from(
            "SELECT COALESCE(SUM(games),0), COALESCE(SUM(sum_gd10),0), COALESCE(SUM(sum_xpd10),0),
             COALESCE(SUM(sum_csd10),0), COALESCE(SUM(solo_kills),0), COALESCE(SUM(solo_deaths),0)
             FROM matchups WHERE lane = ? AND champ = ? AND opp = ?",
        );
        let mut params: Vec<&str> = vec![lane, champ, opp];
        if let Some(t) = tiers {
            sql += &format!(" AND tier IN ({})", placeholders(t.len()));
            params.extend(t.iter().map(|s| s.as_str()));
        }
        if let Some(p) = patches {
            sql += &format!(" AND patch IN ({})", placeholders(p.len()));
            params.extend(p.iter().map(|s| s.as_str()));
        }
        self.conn
            .query_row(&sql, params_from_iter(params), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })
            .unwrap_or((0, 0.0, 0.0, 0.0, 0, 0))
    }

    /// champ 視点のマッチアップ実績。データが1試合もなければ None。
    pub fn lookup(
        &self,
        lane: &str,
        champ: &str,
        opp: &str,
        tier: &str,
        min_games: i64,
        patches: Option<&[String]>,
    ) -> Option<MatchupStats> {
        let key: LookupKey =
            (lane.into(), champ.into(), opp.into(), tier.into(), min_games, patches.map(|p| p.to_vec()));
        if let Some(hit) = self.cache.borrow().get(&key) {
            return hit.clone();
        }
        let mut result: Option<MatchupStats> = None;
        let candidates: Vec<Option<Vec<String>>> =
            if tier == "ALL" { vec![None] } else { tier_search_order(tier).into_iter().map(Some).collect() };
        for tiers in candidates {
            let (games, gd, xpd, csd, sk, sd) = self.sum(lane, champ, opp, tiers.as_deref(), patches);
            if games > 0 {
                let g = games as f64;
                result = Some(MatchupStats {
                    games,
                    gd10: gd / g,
                    xpd10: xpd / g,
                    csd10: csd / g,
                    solo_kill_rate: sk as f64 / g,
                    solo_death_rate: sd as f64 / g,
                    tiers: tiers.clone().unwrap_or_else(|| vec!["ALL".to_string()]),
                });
            }
            if result.as_ref().is_some_and(|r| r.games >= min_games) {
                break;
            }
        }
        self.cache.borrow_mut().insert(key, result.clone());
        result
    }

    /// DB に貯まったロール出現率（全ランク合算）と、その試合数。
    pub fn role_rates(&self, champ: &str, patches: Option<&[String]>) -> (HashMap<String, f64>, i64) {
        let key = (champ.to_string(), patches.map(|p| p.to_vec()));
        if let Some(hit) = self.role_cache.borrow().get(&key) {
            return hit.clone();
        }
        let mut sql = String::from("SELECT position, SUM(games) FROM role_counts WHERE champ = ?");
        let mut params: Vec<&str> = vec![champ];
        if let Some(p) = patches {
            sql += &format!(" AND patch IN ({})", placeholders(p.len()));
            params.extend(p.iter().map(|s| s.as_str()));
        }
        sql += " GROUP BY position";
        let mut counts: HashMap<String, i64> = HashMap::new();
        if let Ok(mut st) = self.conn.prepare(&sql) {
            if let Ok(rows) = st.query_map(params_from_iter(params), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                for (pos, n) in rows.flatten() {
                    if POSITIONS.contains(&pos.as_str()) {
                        counts.insert(pos, n);
                    }
                }
            }
        }
        let total: i64 = counts.values().sum();
        let rates = if total > 0 {
            POSITIONS.iter().map(|p| (p.to_string(), *counts.get(*p).unwrap_or(&0) as f64 / total as f64)).collect()
        } else {
            HashMap::new()
        };
        let out = (rates, total);
        self.role_cache.borrow_mut().insert(key, out.clone());
        out
    }
}
