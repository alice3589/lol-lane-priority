//! レーン主導権スコアの計算。味方視点で -100〜+100（正なら味方有利）。
//!   A. 実績データ  : 10分ゴールド差・XP差・ソロキル     初期重み 0.6
//!   B. チャンプ特性 : 射程・序盤・ウェーブクリア・Lv6・機動力 初期重み 0.3
//! A は試合数が min_games に届かない分だけ重みを B に回し、最後に A+B で正規化する。
use crate::champions::ChampionCatalog;
use crate::matchup_db::{MatchupDB, MatchupStats};
use crate::traits::{trait_label, TraitTable};

pub const EVEN_THRESHOLD: f64 = 10.0;
const WEIGHT_A: f64 = 0.6;
const WEIGHT_B: f64 = 0.3;

const GD_SCALE: f64 = 500.0;
const RANGE_SCALE: f64 = 400.0;
const SOLO_KILL_SCALE: f64 = 0.3;

const A_PARTS: [(&str, f64); 3] = [("gd10", 0.7), ("xpd10", 0.2), ("solo", 0.1)];
const LANE_TERMS: [(&str, f64); 5] =
    [("range", 0.25), ("early", 0.35), ("waveclear", 0.20), ("spike6", 0.12), ("mobility", 0.08)];
const JG_TERMS: [(&str, f64); 3] = [("clear", 0.40), ("gank", 0.35), ("duel", 0.25)];
const BOT_A_PAIRS: [(&str, f64); 2] = [("bottom", 0.6), ("utility", 0.4)];
const PAIR_LABELS: [&str; 2] = ["ADC", "SUP"];

fn bot_pair_weights(term: &str) -> (f64, f64) {
    match term {
        "range" => (0.85, 0.15),
        "early" => (0.5, 0.5),
        "waveclear" => (0.8, 0.2),
        "spike6" => (0.5, 0.5),
        "mobility" => (0.6, 0.4),
        _ => panic!("unknown term: {term}"),
    }
}

fn clamp1(x: f64) -> f64 {
    x.clamp(-1.0, 1.0)
}

#[derive(Debug, Clone)]
pub struct Reason {
    pub source: &'static str, // "A"（実績）/ "B"（特性）
    pub label: String,
    pub detail: String,
    pub points: f64,
}

#[derive(Debug, Clone)]
pub struct LaneResult {
    pub lane: String,
    pub ally: Vec<Option<String>>,
    pub enemy: Vec<Option<String>>,
    pub score: Option<f64>,
    pub verdict: &'static str, // ally / even / enemy / unknown
    pub reasons: Vec<Reason>,
    pub notes: Vec<String>,
    pub games: i64,
    pub tiers: Vec<String>,
    pub weight_a: f64,
    pub weight_b: f64,
}

pub fn verdict_of(score: Option<f64>) -> &'static str {
    match score {
        None => "unknown",
        Some(s) if s >= EVEN_THRESHOLD => "ally",
        Some(s) if s <= -EVEN_THRESHOLD => "enemy",
        Some(_) => "even",
    }
}

fn stats_score_parts(s: &MatchupStats) -> [(&'static str, f64); 3] {
    [
        ("gd10", (s.gd10 / GD_SCALE).tanh()),
        ("xpd10", (s.xpd10 / GD_SCALE).tanh()),
        ("solo", clamp1((s.solo_kill_rate - s.solo_death_rate) / SOLO_KILL_SCALE)),
    ]
}

type Part = (String, String, f64); // (label, detail, points)

struct StatsScore {
    value: f64,
    games: f64,
    tiers: Vec<String>,
    parts: Vec<Part>,
}

pub struct Scorer<'a> {
    pub catalog: &'a ChampionCatalog,
    pub traits: &'a TraitTable,
    pub db: Option<&'a MatchupDB>,
    pub tier: String,
    pub min_games: i64,
    pub patches: Option<Vec<String>>,
}

impl<'a> Scorer<'a> {
    pub fn new(catalog: &'a ChampionCatalog, traits: &'a TraitTable) -> Self {
        Scorer { catalog, traits, db: None, tier: "ALL".into(), min_games: 200, patches: None }
    }

    // ---- B: チャンピオン特性 ----

    fn value(&self, term: &str, cid: &str) -> f64 {
        if term == "range" {
            self.catalog.attack_range(cid) as f64
        } else {
            self.traits.traits(cid, self.catalog).get(term)
        }
    }

    fn diff(&self, term: &str, a: &str, b: &str) -> f64 {
        let d = self.value(term, a) - self.value(term, b);
        if term == "range" { clamp1(d / RANGE_SCALE) } else { clamp1(d / 4.0) }
    }

    fn detail(&self, term: &str, pairs: &[(usize, &str, &str)], multi: bool) -> String {
        pairs
            .iter()
            .map(|&(idx, a, b)| {
                let (va, vb) = (self.value(term, a), self.value(term, b));
                let text = if term == "range" {
                    format!("{va:.0} vs {vb:.0}（{:+.0}）", va - vb)
                } else {
                    format!("{va:.0} vs {vb:.0}")
                };
                if multi { format!("{} {text}", PAIR_LABELS[idx]) } else { text }
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }

    fn trait_score(&self, lane: &str, ally: &[Option<&str>], enemy: &[Option<&str>]) -> Option<(f64, Vec<Part>)> {
        let pairs: Vec<(usize, &str, &str)> = ally
            .iter()
            .zip(enemy)
            .enumerate()
            .filter_map(|(i, (a, b))| Some((i, (*a)?, (*b)?)))
            .collect();
        if pairs.is_empty() {
            return None;
        }
        let terms: &[(&str, f64)] = if lane == "jungle" { &JG_TERMS } else { &LANE_TERMS };
        let multi = lane == "bottom";
        let mut total = 0.0;
        let mut parts = vec![];
        for &(term, term_weight) in terms {
            let diff = if multi {
                let pw = bot_pair_weights(term);
                let w = |i: usize| if i == 0 { pw.0 } else { pw.1 };
                let wsum: f64 = pairs.iter().map(|&(i, _, _)| w(i)).sum();
                pairs.iter().map(|&(i, a, b)| w(i) * self.diff(term, a, b)).sum::<f64>() / wsum
            } else {
                let (_, a, b) = pairs[0];
                self.diff(term, a, b)
            };
            let points = 100.0 * term_weight * diff;
            total += points;
            let label = if term == "range" { "射程" } else { trait_label(term) };
            parts.push((label.to_string(), self.detail(term, &pairs, multi), points));
        }
        Some((total, parts))
    }

    // ---- A: 実績データ ----

    /// A の値・平均試合数・使ったランク帯・内訳。データが無ければ None。
    /// games は片方のペアしかデータが無いときの信頼度低下を織り込み済み。
    fn stats_score(&self, lane: &str, ally: &[Option<&str>], enemy: &[Option<&str>]) -> Option<StatsScore> {
        let db = self.db?;
        let db_pairs: Vec<(&str, f64)> =
            if lane == "bottom" { BOT_A_PAIRS.to_vec() } else { vec![(lane, 1.0)] };
        let mut found = vec![];
        let mut known_weight = 0.0;
        for (idx, ((db_lane, w), (a, b))) in db_pairs.iter().zip(ally.iter().zip(enemy)).enumerate() {
            let (Some(a), Some(b)) = (a, b) else { continue };
            known_weight += w;
            if let Some(stats) = db.lookup(db_lane, a, b, &self.tier, self.min_games, self.patches.as_deref()) {
                found.push((idx, *w, stats));
            }
        }
        if found.is_empty() {
            return None;
        }
        let wsum: f64 = found.iter().map(|(_, w, _)| w).sum();
        let multi = lane == "bottom";
        let (mut value, mut games) = (0.0, 0.0);
        let mut parts = vec![];
        let mut tiers: Vec<String> = vec![];
        for (idx, w, stats) in &found {
            let share = w / wsum;
            let vals = stats_score_parts(stats);
            let prefix = if multi { format!("{} ", PAIR_LABELS[*idx]) } else { String::new() };
            for (key, part_weight) in A_PARTS {
                let v = vals.iter().find(|(k, _)| *k == key).unwrap().1;
                let points = 100.0 * share * part_weight * v;
                value += points;
                let (label, detail) = match key {
                    "gd10" => ("10分ゴールド差", format!("平均 {:+.0}（{}試合）", stats.gd10, stats.games)),
                    "xpd10" => ("10分XP差", format!("平均 {:+.0}", stats.xpd10)),
                    _ => (
                        "ソロキル(〜10分)",
                        format!("{:.2} vs {:.2} 回/試合", stats.solo_kill_rate, stats.solo_death_rate),
                    ),
                };
                parts.push((format!("{prefix}{label}"), detail, points));
            }
            games += share * stats.games as f64;
            for t in &stats.tiers {
                if !tiers.contains(t) {
                    tiers.push(t.clone());
                }
            }
        }
        let coverage = if known_weight > 0.0 { wsum / known_weight } else { 0.0 };
        Some(StatsScore { value, games: games * coverage, tiers, parts })
    }

    // ---- 合算 ----

    pub fn score_lane(&self, lane: &str, ally: &[Option<&str>], enemy: &[Option<&str>]) -> LaneResult {
        let own = |v: &[Option<&str>]| v.iter().map(|c| c.map(String::from)).collect::<Vec<_>>();
        let mut result = LaneResult {
            lane: lane.into(),
            ally: own(ally),
            enemy: own(enemy),
            score: None,
            verdict: "unknown",
            reasons: vec![],
            notes: vec![],
            games: 0,
            tiers: vec![],
            weight_a: 0.0,
            weight_b: 0.0,
        };
        let Some((b_value, b_parts)) = self.trait_score(lane, ally, enemy) else {
            if ally.iter().all(|c| c.is_none()) {
                result.notes.push("味方のチャンピオンが未確定".into());
            }
            if enemy.iter().all(|c| c.is_none()) {
                result.notes.push("敵のチャンピオンが未確定".into());
            }
            return result;
        };

        let a = self.stats_score(lane, ally, enemy);
        let games = a.as_ref().map_or(0.0, |s| s.games);
        let min_games = self.min_games.max(1) as f64;
        let reliability = if a.is_some() { (games / min_games).min(1.0) } else { 0.0 };
        let w_a = WEIGHT_A * reliability;
        let w_b = WEIGHT_B + WEIGHT_A * (1.0 - reliability);
        let norm = w_a + w_b;
        result.weight_a = w_a / norm;
        result.weight_b = w_b / norm;

        let mut score = result.weight_b * b_value;
        if let Some(a) = &a {
            score += result.weight_a * a.value;
            for (l, d, p) in &a.parts {
                result.reasons.push(Reason { source: "A", label: l.clone(), detail: d.clone(), points: p * result.weight_a });
            }
        }
        for (l, d, p) in &b_parts {
            result.reasons.push(Reason { source: "B", label: l.clone(), detail: d.clone(), points: p * result.weight_b });
        }
        result.reasons.sort_by(|x, y| y.points.abs().partial_cmp(&x.points.abs()).unwrap());
        let score = score.clamp(-100.0, 100.0);
        result.score = Some(score);
        result.verdict = verdict_of(Some(score));
        result.games = games.round_ties_even() as i64;
        result.tiers = a.as_ref().map(|s| s.tiers.clone()).unwrap_or_default();

        if a.is_none() {
            result.notes.push("実績データなし（特性表のみで判定）".into());
        } else if reliability < 1.0 {
            result.notes.push(format!("実績データが少ない（{}試合）ため特性表の比重を上げています", result.games));
        }
        for (side, champs) in [("味方", ally), ("敵", enemy)] {
            for (idx, cid) in champs.iter().enumerate() {
                match cid {
                    None if lane == "bottom" => result.notes.push(format!("{side}{}が未確定", PAIR_LABELS[idx])),
                    Some(cid) if self.traits.traits(cid, self.catalog).estimated => {
                        result.notes.push(format!("{} の特性値は推定", self.catalog.name(cid)))
                    }
                    _ => {}
                }
            }
        }
        result
    }
}

/// 主導権のあるレーンから、JG の動き方の目安を作る。
pub fn advice(results: &[LaneResult]) -> Vec<String> {
    let s = |lane: &str| results.iter().find(|r| r.lane == lane).and_then(|r| r.score);
    let (top, mid, bot, jg) = (s("top"), s("middle"), s("bottom"), s("jungle"));
    let mut lines = vec![];
    if let (Some(top), Some(bot)) = (top, bot) {
        let t = EVEN_THRESHOLD;
        lines.push(if top >= t && bot >= t {
            "両サイドに主導権 → JG はどちら側でも動きやすい"
        } else if top <= -t && bot <= -t {
            "両サイド不利 → 序盤はファームと視界を優先、カウンタージャングルに注意"
        } else if top - bot >= 15.0 && top >= t {
            "TOP 側に主導権 → JG は TOP 側（ガンク・スカトル・ヴォイドグラブ）で動きやすい"
        } else if bot - top >= 15.0 && bot >= t {
            "BOT 側に主導権 → JG は BOT 側（ガンク・スカトル・ドラゴン）で動きやすい"
        } else {
            "サイドの主導権は拮抗"
        }.to_string());
    }
    if let Some(mid) = mid {
        if mid >= EVEN_THRESHOLD {
            lines.push("MID に主導権 → MID のロームや JG のカバーが通りやすい".into());
        } else if mid <= -EVEN_THRESHOLD {
            lines.push("MID が不利 → 敵 MID のロームに注意".into());
        }
    }
    if jg.is_some_and(|j| j <= -EVEN_THRESHOLD) {
        lines.push("敵 JG の方が序盤に強い → インベード・早いガンクに注意".into());
    }
    lines
}
