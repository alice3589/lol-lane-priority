//! ポジション推定。5人 × 5ポジションの割り当て 120 通りを全探索し、出現率の積が最大のものを採用する。
//! 各人の確信度は周辺確率（全割り当てで重み付けしたときにそのポジションになる確率）。
use crate::models::{as_position, Placement, Player, POSITIONS};
use std::collections::{HashMap, HashSet};

/// 出現率 0 のロールでも完全には否定しない（オフロール対策）
const RATE_FLOOR: f64 = 0.01;
/// スマイト所持者が JG 以外にいる場合の係数
const SMITE_PENALTY: f64 = 0.01;

fn fixed_positions(players: &[Player], overrides: &HashMap<String, String>) -> Vec<Option<&'static str>> {
    let mut fixed: Vec<Option<&'static str>> = vec![None; players.len()];
    let mut taken: HashSet<&'static str> = HashSet::new();
    // 1. 手動指定
    for (i, p) in players.iter().enumerate() {
        let pos = p.champion.as_ref().and_then(|c| overrides.get(c)).and_then(|s| as_position(s));
        if let Some(pos) = pos {
            if !taken.contains(pos) {
                fixed[i] = Some(pos);
                taken.insert(pos);
            }
        }
    }
    // 2. クライアントが教えてくれたポジション
    for (i, p) in players.iter().enumerate() {
        if fixed[i].is_none() {
            if let Some(pos) = p.position.as_deref().and_then(as_position) {
                if !taken.contains(pos) {
                    fixed[i] = Some(pos);
                    taken.insert(pos);
                }
            }
        }
    }
    fixed
}

/// itertools.permutations と同じ辞書順で全順列を渡す。
fn for_each_permutation(n: usize, f: &mut impl FnMut(&[usize])) {
    fn rec(cur: &mut Vec<usize>, used: &mut [bool], n: usize, f: &mut impl FnMut(&[usize])) {
        if cur.len() == n {
            f(cur);
            return;
        }
        for i in 0..n {
            if !used[i] {
                used[i] = true;
                cur.push(i);
                rec(cur, used, n, f);
                cur.pop();
                used[i] = false;
            }
        }
    }
    rec(&mut Vec::with_capacity(n), &mut vec![false; n], n, f);
}

/// players（最大5人）にポジションを割り当てる。返り値は players と同じ順番。
pub fn infer_positions(
    players: &[Player],
    rates_for: &dyn Fn(&str) -> HashMap<String, f64>,
    overrides: Option<&HashMap<String, String>>,
) -> Vec<Placement> {
    let players: Vec<Player> = players.iter().take(5).cloned().collect();
    let n = players.len();
    if n == 0 {
        return vec![];
    }
    let mut slots = players.clone();
    slots.resize(5, Player::default());
    let empty = HashMap::new();
    let fixed = fixed_positions(&slots, overrides.unwrap_or(&empty));

    let weight_table: Vec<[f64; 5]> = slots
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut row = [0.0; 5];
            for (k, pos) in POSITIONS.iter().enumerate() {
                row[k] = if let Some(f) = fixed[i] {
                    if *pos == f { 1.0 } else { 0.0 }
                } else {
                    let mut w = 1.0;
                    if let Some(c) = &p.champion {
                        w = rates_for(c).get(*pos).copied().unwrap_or(0.0).max(RATE_FLOOR);
                    }
                    if p.has_smite && *pos != "jungle" {
                        w *= SMITE_PENALTY;
                    }
                    w
                };
            }
            row
        })
        .collect();

    let mut total = 0.0;
    let mut best_weight = -1.0;
    let mut best: Vec<usize> = (0..5).collect();
    let mut marginals = [[0.0f64; 5]; 5];
    for_each_permutation(5, &mut |perm| {
        let mut w = 1.0;
        for (i, &pos) in perm.iter().enumerate() {
            w *= weight_table[i][pos];
            if w == 0.0 {
                break;
            }
        }
        if w == 0.0 {
            return;
        }
        total += w;
        for (i, &pos) in perm.iter().enumerate() {
            marginals[i][pos] += w;
        }
        if w > best_weight {
            best_weight = w;
            best = perm.to_vec();
        }
    });

    (0..n)
        .map(|i| {
            let pos = best[i];
            let conf = if total > 0.0 { marginals[i][pos] / total } else { 0.0 };
            Placement { player: players[i].clone(), position: POSITIONS[pos], confidence: conf, fixed: fixed[i].is_some() }
        })
        .collect()
}
