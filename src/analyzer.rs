//! 状態（GameState）→ ポジション推定 → スコア計算 をまとめて行う。UI からはここだけを呼ぶ。
use crate::champions::ChampionCatalog;
use crate::matchup_db::{MatchupDB, ROLE_RATE_MIN_GAMES};
use crate::models::{GameState, Placement, LANES};
use crate::role_inference::infer_positions;
use crate::scorer::{advice, LaneResult, Scorer};
use crate::traits::TraitTable;
use std::collections::HashMap;

/// これ未満の確信度なら UI で「?」を付ける
pub const UNCERTAIN_BELOW: f64 = 0.6;

fn lane_positions(lane: &str) -> &'static [&'static str] {
    match lane {
        "top" => &["top"],
        "jungle" => &["jungle"],
        "middle" => &["middle"],
        _ => &["bottom", "utility"],
    }
}

pub use crate::config::Settings;

/// カウンター候補に挙げるロール出現率の下限
const COUNTER_MIN_ROLE_RATE: f64 = 0.10;

/// 「この相手に有利なチャンピオン」の1件
#[derive(Debug, Clone)]
pub struct CounterPick {
    pub champion: String,
    pub score: f64,
    /// 有利になっている主な理由（スコアへの寄与が最大の項目）
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Analysis {
    pub ally: Vec<Placement>,
    pub enemy: Vec<Placement>,
    pub lanes: Vec<LaneResult>,
    pub advice: Vec<String>,
}

impl Analysis {
    pub fn placement_at(&self, side: &str, position: &str) -> Option<&Placement> {
        let list = if side == "ally" { &self.ally } else { &self.enemy };
        list.iter().find(|p| p.position == position)
    }
}

pub struct Analyzer {
    pub catalog: ChampionCatalog,
    pub traits: TraitTable,
    pub settings: Settings,
    pub db: Option<MatchupDB>,
}

/// side ("ally" / "enemy") → チャンピオン id → ポジション
pub type Overrides = HashMap<String, HashMap<String, String>>;

impl Analyzer {
    pub fn new(catalog: ChampionCatalog, traits: TraitTable, settings: Settings, db: Option<MatchupDB>) -> Self {
        Analyzer { catalog, traits, settings, db }
    }

    fn patches(&self) -> Option<Vec<String>> {
        self.db.as_ref().and_then(|db| db.recent_patches(self.settings.recent_patches))
    }

    pub fn rates_for(&self, cid: &str) -> HashMap<String, f64> {
        if let Some(db) = &self.db {
            let (rates, games) = db.role_rates(cid, self.patches().as_deref());
            if games >= ROLE_RATE_MIN_GAMES {
                return rates;
            }
        }
        self.traits.role_rates(cid, &self.catalog)
    }

    pub fn scorer(&self) -> Scorer<'_> {
        Scorer {
            catalog: &self.catalog,
            traits: &self.traits,
            db: self.db.as_ref(),
            tier: self.settings.tier.clone(),
            min_games: self.settings.min_games,
            patches: self.patches(),
        }
    }

    /// position（5ポジション）で敵の enemy に対して有利なチャンピオンを、スコアの高い順に limit 件返す。
    /// 特性表（と実績DB）による判定で、そのポジションで一定以上使われるチャンピオンだけを候補にする。
    pub fn counter_picks(&self, position: &str, enemy: &str, limit: usize) -> Vec<CounterPick> {
        let (lane, slot) = match position {
            "utility" => ("bottom", 1),
            "bottom" => ("bottom", 0),
            other => (other, 0),
        };
        let scorer = self.scorer();
        let width = if lane == "bottom" { 2 } else { 1 };
        let mut enemy_side: Vec<Option<&str>> = vec![None; width];
        enemy_side[slot] = Some(enemy);
        let mut picks: Vec<CounterPick> = self
            .catalog
            .ids()
            .into_iter()
            .filter(|cid| *cid != enemy && self.rates_for(cid).get(position).copied().unwrap_or(0.0) >= COUNTER_MIN_ROLE_RATE)
            .filter_map(|cid| {
                let mut ally_side: Vec<Option<&str>> = vec![None; width];
                ally_side[slot] = Some(cid);
                let result = scorer.score_lane(lane, &ally_side, &enemy_side);
                let reason = result.reasons.iter().find(|r| r.points > 0.0).map(|r| r.label.clone()).unwrap_or_default();
                Some(CounterPick { champion: cid.to_string(), score: result.score?, reason })
            })
            .collect();
        picks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap().then_with(|| a.champion.cmp(&b.champion)));
        picks.truncate(limit);
        picks
    }

    pub fn analyze(&self, state: &GameState, overrides: Option<&Overrides>) -> Analysis {
        let rates = |cid: &str| self.rates_for(cid);
        let ally = infer_positions(&state.ally, &rates, overrides.and_then(|o| o.get("ally")));
        let enemy = infer_positions(&state.enemy, &rates, overrides.and_then(|o| o.get("enemy")));
        let by = |list: &[Placement]| -> HashMap<&'static str, Option<String>> {
            list.iter().map(|p| (p.position, p.player.champion.clone())).collect()
        };
        let (ally_by, enemy_by) = (by(&ally), by(&enemy));

        let scorer = self.scorer();
        let pick = |m: &HashMap<&'static str, Option<String>>, lane: &str| -> Vec<Option<String>> {
            lane_positions(lane).iter().map(|p| m.get(p).cloned().flatten()).collect()
        };
        let lanes: Vec<LaneResult> = LANES
            .iter()
            .map(|lane| {
                let a = pick(&ally_by, lane);
                let e = pick(&enemy_by, lane);
                let a_ref: Vec<Option<&str>> = a.iter().map(|c| c.as_deref()).collect();
                let e_ref: Vec<Option<&str>> = e.iter().map(|c| c.as_deref()).collect();
                scorer.score_lane(lane, &a_ref, &e_ref)
            })
            .collect();
        let advice = advice(&lanes);
        Analysis { ally, enemy, lanes, advice }
    }
}
