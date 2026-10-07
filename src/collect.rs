//! Match-V5 からマッチアップ実績を収集・集計して SQLite に保存する（仕様書 4.3）。
//!
//! ・ランク帯ごとに League-V4 からプレイヤーを集め、各プレイヤーのランク戦の試合を取得する
//! ・timeline の 10分時点の gold / xp / CS から、レーンごとの差を両方向で記録する
//! ・取り込み済みの試合はスキップするので、途中で止めても続きから再開できる
//! ・開発用キーのレート制限（20回/秒・100回/2分）を守るため、時間はかかる
use crate::champions::ChampionCatalog;
use crate::matchup_db::{LaneRow, MatchupDB};
use crate::models::{normalize_position, POSITIONS};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

pub const RANKED_SOLO_QUEUE: i64 = 420;
const QUEUE_NAME: &str = "RANKED_SOLO_5x5";
const TEN_MINUTES_MS: i64 = 600_000;
const DIVISIONS: [&str; 4] = ["I", "II", "III", "IV"];

pub fn region_of(platform: &str) -> Option<&'static str> {
    Some(match platform {
        "jp1" | "kr" => "asia",
        "na1" | "br1" | "la1" | "la2" => "americas",
        "euw1" | "eun1" | "tr1" | "ru" | "me1" => "europe",
        "oc1" | "ph2" | "sg2" | "th2" | "tw2" | "vn2" => "sea",
        _ => return None,
    })
}

fn apex_endpoint(tier: &str) -> Option<&'static str> {
    Some(match tier {
        "MASTER" => "masterleagues",
        "GRANDMASTER" => "grandmasterleagues",
        "CHALLENGER" => "challengerleagues",
        _ => return None,
    })
}

/// 複数の「n回 / t秒」制限をまとめて守る。時計と sleep は差し替えられる（テスト用）。
pub struct RateLimiter {
    limits: Vec<(usize, f64)>,
    calls: VecDeque<f64>,
    clock: Box<dyn FnMut() -> f64>,
    sleep: Box<dyn FnMut(f64)>,
}

impl RateLimiter {
    pub fn new(limits: Vec<(usize, f64)>, clock: Box<dyn FnMut() -> f64>, sleep: Box<dyn FnMut(f64)>) -> Self {
        RateLimiter { limits, calls: VecDeque::new(), clock, sleep }
    }

    /// 開発用キーの制限（20回/秒・100回/2分）
    pub fn riot_default() -> Self {
        let start = Instant::now();
        Self::new(
            vec![(20, 1.0), (100, 120.0)],
            Box::new(move || start.elapsed().as_secs_f64()),
            Box::new(|s| std::thread::sleep(Duration::from_secs_f64(s))),
        )
    }

    pub fn wait(&mut self) {
        let longest = self.limits.iter().map(|&(_, w)| w).fold(0.0, f64::max);
        loop {
            let now = (self.clock)();
            while self.calls.front().is_some_and(|&t| now - t >= longest) {
                self.calls.pop_front();
            }
            let mut delay: f64 = 0.0;
            for &(count, window) in &self.limits {
                let recent: Vec<f64> = self.calls.iter().copied().filter(|&t| now - t < window).collect();
                if recent.len() >= count {
                    delay = delay.max(window - (now - recent[recent.len() - count]));
                }
            }
            if delay <= 0.0 {
                self.calls.push_back(now);
                return;
            }
            (self.sleep)(delay + 0.01);
        }
    }
}

pub struct RiotApi {
    api_key: String,
    platform: String,
    region: &'static str,
    limiter: RateLimiter,
    agent: ureq::Agent,
}

impl RiotApi {
    pub fn new(api_key: &str, platform: &str) -> Result<Self, String> {
        let region = region_of(platform).ok_or_else(|| format!("未対応のプラットフォーム: {platform}"))?;
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        Ok(RiotApi { api_key: api_key.into(), platform: platform.into(), region, limiter: RateLimiter::riot_default(), agent })
    }

    fn get(&mut self, url: &str, query: &[(&str, String)]) -> Result<Option<Value>, String> {
        for attempt in 0..5u32 {
            self.limiter.wait();
            let mut req = self.agent.get(url).header("X-Riot-Token", &self.api_key);
            for (k, v) in query {
                req = req.query(*k, v);
            }
            let mut r = match req.call() {
                Ok(r) => r,
                Err(_) => {
                    std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
                    continue;
                }
            };
            match r.status().as_u16() {
                200 => return r.body_mut().with_config().limit(64 * 1024 * 1024).read_json().map(Some).map_err(|e| e.to_string()),
                404 => return Ok(None),
                429 => {
                    let wait = r
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(10.0);
                    std::thread::sleep(Duration::from_secs_f64(wait));
                }
                s if s >= 500 => std::thread::sleep(Duration::from_secs(2u64.pow(attempt))),
                s => return Err(format!("HTTP {s}: {url}")),
            }
        }
        Err(format!("リトライ上限: {url}"))
    }

    fn platform_url(&self, path: &str) -> String {
        format!("https://{}.api.riotgames.com{path}", self.platform)
    }

    fn region_url(&self, path: &str) -> String {
        format!("https://{}.api.riotgames.com{path}", self.region)
    }

    fn puuid_of_entry(&mut self, entry: &Value) -> Result<Option<String>, String> {
        if let Some(p) = entry["puuid"].as_str().filter(|s| !s.is_empty()) {
            return Ok(Some(p.into()));
        }
        if let Some(id) = entry["summonerId"].as_str().filter(|s| !s.is_empty()) {
            // 古い形式のレスポンス向け
            let url = self.platform_url(&format!("/lol/summoner/v4/summoners/{id}"));
            return Ok(self.get(&url, &[])?.and_then(|s| s["puuid"].as_str().map(String::from)));
        }
        Ok(None)
    }

    /// ランク帯 tier のプレイヤーを最大 limit 人。
    pub fn tier_puuids(&mut self, tier: &str, limit: usize) -> Result<Vec<String>, String> {
        let mut puuids = vec![];
        if let Some(ep) = apex_endpoint(tier) {
            let url = self.platform_url(&format!("/lol/league/v4/{ep}/by-queue/{QUEUE_NAME}"));
            let data = self.get(&url, &[])?.unwrap_or(Value::Null);
            for entry in data["entries"].as_array().into_iter().flatten() {
                if puuids.len() >= limit {
                    break;
                }
                if let Some(p) = self.puuid_of_entry(entry)? {
                    puuids.push(p);
                }
            }
            return Ok(puuids);
        }
        // 各ディビジョンから均等に集める
        let per_division = limit.div_ceil(DIVISIONS.len()).max(1);
        for division in DIVISIONS {
            let (mut got, mut page) = (0, 1);
            while got < per_division && puuids.len() < limit {
                let url = self.platform_url(&format!("/lol/league/v4/entries/{QUEUE_NAME}/{tier}/{division}"));
                let entries = match self.get(&url, &[("page", page.to_string())])? {
                    Some(Value::Array(a)) if !a.is_empty() => a,
                    _ => break,
                };
                for entry in &entries {
                    if got >= per_division || puuids.len() >= limit {
                        break;
                    }
                    if let Some(p) = self.puuid_of_entry(entry)? {
                        puuids.push(p);
                        got += 1;
                    }
                }
                page += 1;
            }
        }
        Ok(puuids)
    }

    pub fn match_ids(&mut self, puuid: &str, count: usize) -> Result<Vec<String>, String> {
        let url = self.region_url(&format!("/lol/match/v5/matches/by-puuid/{puuid}/ids"));
        let q = [("queue", RANKED_SOLO_QUEUE.to_string()), ("type", "ranked".into()), ("count", count.to_string())];
        Ok(self
            .get(&url, &q)?
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()))
            .unwrap_or_default())
    }

    pub fn match_info(&mut self, match_id: &str) -> Result<Option<Value>, String> {
        let url = self.region_url(&format!("/lol/match/v5/matches/{match_id}"));
        self.get(&url, &[])
    }

    pub fn timeline(&mut self, match_id: &str) -> Result<Option<Value>, String> {
        let url = self.region_url(&format!("/lol/match/v5/matches/{match_id}/timeline"));
        self.get(&url, &[])
    }
}

#[derive(Debug)]
pub struct MatchAggregate {
    pub patch: String,
    pub rows: Vec<LaneRow>,
    pub roles: Vec<(String, String)>,
}

/// "16.19.712.3456" → "16.19"
pub fn patch_of(game_version: &str) -> String {
    game_version.split('.').take(2).collect::<Vec<_>>().join(".")
}

fn champion_id(p: &Value, catalog: Option<&ChampionCatalog>) -> Option<String> {
    if let Some(c) = catalog {
        if let Some(cid) = c.id_by_key(p["championId"].as_i64()).or_else(|| c.resolve(p["championName"].as_str())) {
            return Some(cid.into());
        }
    }
    p["championName"].as_str().filter(|s| !s.is_empty()).map(String::from)
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// 1試合分の集計行を作る。10分に届かない試合・ポジションが崩れている試合は None。
pub fn aggregate_match(m: &Value, timeline: &Value, catalog: Option<&ChampionCatalog>) -> Option<MatchAggregate> {
    let info = &m["info"];
    let frames = timeline["info"]["frames"].as_array()?;
    if frames.len() <= 10 {
        return None;
    }
    let pf10 = &frames[10]["participantFrames"];

    // (チーム, ポジション) → (参加者, チャンピオン)
    let mut by_slot: HashMap<(i64, &str), (&Value, String)> = HashMap::new();
    let mut roles = vec![];
    for p in info["participants"].as_array().into_iter().flatten() {
        let (Some(pos), Some(champ)) = (normalize_position(p["teamPosition"].as_str()), champion_id(p, catalog)) else {
            continue;
        };
        roles.push((champ.clone(), pos.to_string()));
        by_slot.insert((p["teamId"].as_i64().unwrap_or(0), pos), (p, champ));
    }

    // 10分までのソロキル（アシスト無しで、キラーがプレイヤーのもの）
    let mut solo: HashMap<(i64, i64), i64> = HashMap::new();
    for frame in frames.iter().take(11) {
        for ev in frame["events"].as_array().into_iter().flatten() {
            if ev["type"] != "CHAMPION_KILL" || ev["timestamp"].as_i64().unwrap_or(0) >= TEN_MINUTES_MS {
                continue;
            }
            if ev["assistingParticipantIds"].as_array().is_some_and(|a| !a.is_empty()) {
                continue;
            }
            let (killer, victim) = (ev["killerId"].as_i64().unwrap_or(0), ev["victimId"].as_i64().unwrap_or(0));
            if killer != 0 && victim != 0 {
                *solo.entry((killer, victim)).or_default() += 1;
            }
        }
    }

    let snapshot = |p: &Value| -> Option<(f64, f64, f64)> {
        let f = pf10.get(p["participantId"].as_i64()?.to_string())?;
        Some((num(&f["totalGold"]), num(&f["xp"]), num(&f["minionsKilled"]) + num(&f["jungleMinionsKilled"])))
    };

    let mut rows = vec![];
    for pos in POSITIONS {
        let (Some((blue, bc)), Some((red, rc))) = (by_slot.get(&(100, pos)), by_slot.get(&(200, pos))) else { continue };
        let (Some(sb), Some(sr)) = (snapshot(blue), snapshot(red)) else { continue };
        let (bid, rid) = (blue["participantId"].as_i64().unwrap_or(0), red["participantId"].as_i64().unwrap_or(0));
        let (bk, rk) = (*solo.get(&(bid, rid)).unwrap_or(&0), *solo.get(&(rid, bid)).unwrap_or(&0));
        let (gd, xpd, csd) = (sb.0 - sr.0, sb.1 - sr.1, sb.2 - sr.2);
        let row = |lane: &str, champ: &str, opp: &str, s: f64, k, d| LaneRow {
            lane: lane.into(),
            champ: champ.into(),
            opp: opp.into(),
            gd10: s * gd,
            xpd10: s * xpd,
            csd10: s * csd,
            solo_kills: k,
            solo_deaths: d,
        };
        rows.push(row(pos, bc, rc, 1.0, bk, rk));
        rows.push(row(pos, rc, bc, -1.0, rk, bk));
    }
    if rows.is_empty() {
        return None;
    }
    Some(MatchAggregate { patch: patch_of(info["gameVersion"].as_str().unwrap_or("")), rows, roles })
}

pub fn collect(
    api: &mut RiotApi,
    db: &mut MatchupDB,
    tiers: &[String],
    players: usize,
    matches: usize,
    catalog: Option<&ChampionCatalog>,
    log: &mut dyn FnMut(String),
) -> Result<usize, String> {
    let mut added = 0;
    for tier in tiers {
        log(format!("[{tier}] プレイヤー取得中…"));
        let puuids = api.tier_puuids(tier, players)?;
        log(format!("[{tier}] {} 人", puuids.len()));
        for (i, puuid) in puuids.iter().enumerate() {
            for match_id in api.match_ids(puuid, matches)? {
                if db.has_match(&match_id).map_err(|e| e.to_string())? {
                    continue;
                }
                let Some(m) = api.match_info(&match_id)? else { continue };
                if m["info"]["queueId"].as_i64() != Some(RANKED_SOLO_QUEUE) {
                    continue;
                }
                let Some(tl) = api.timeline(&match_id)? else { continue };
                let Some(agg) = aggregate_match(&m, &tl, catalog) else { continue };
                if db.add_match(&match_id, &agg.patch, tier, &agg.rows, &agg.roles).map_err(|e| e.to_string())? {
                    added += 1;
                }
            }
            log(format!("[{tier}] {}/{} 人処理 / 追加 {added} 試合", i + 1, puuids.len()));
        }
    }
    Ok(added)
}
