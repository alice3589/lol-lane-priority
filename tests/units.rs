//! 実績DB・ポジション推定・同梱データ・集計バッチのテスト（旧 Python 版テストの移植）。
use lane_core::champions::ChampionCatalog;
use lane_core::collect::{aggregate_match, patch_of, RateLimiter};
use lane_core::config::{BUNDLED_CHAMPIONS, BUNDLED_ROLES, BUNDLED_TRAITS};
use lane_core::matchup_db::{tier_search_order, LaneRow, MatchupDB};
use lane_core::models::Player;
use lane_core::role_inference::infer_positions;
use lane_core::traits::TraitTable;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

fn catalog() -> ChampionCatalog {
    ChampionCatalog::from_json(BUNDLED_CHAMPIONS).unwrap()
}

fn traits() -> TraitTable {
    TraitTable::from_json(BUNDLED_TRAITS, BUNDLED_ROLES).unwrap()
}

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

// ---- 実績DB ----

fn row(gd: f64) -> LaneRow {
    LaneRow { lane: "top".into(), champ: "Darius".into(), opp: "Garen".into(), gd10: gd, xpd10: gd / 2.0, csd10: 1.0, solo_kills: 1, solo_deaths: 0 }
}

#[test]
fn tier_order() {
    let o = tier_search_order("GOLD");
    assert_eq!(o[0], ["GOLD"]);
    assert_eq!(o[1], ["GOLD", "SILVER"]);
    assert_eq!(o[2], ["GOLD", "SILVER", "PLATINUM"]);
    assert_eq!(o.last().unwrap().last().unwrap(), "CHALLENGER");
    assert_eq!(tier_search_order("IRON")[1], ["IRON", "BRONZE"]);
}

#[test]
fn add_match_is_idempotent() {
    let mut db = MatchupDB::open_in_memory().unwrap();
    let roles = [("Darius".to_string(), "top".to_string())];
    assert!(db.add_match("JP1_1", "16.19", "GOLD", &[row(100.0)], &roles).unwrap());
    assert!(!db.add_match("JP1_1", "16.19", "GOLD", &[row(100.0)], &roles).unwrap());
    assert_eq!(db.lookup("top", "Darius", "Garen", "GOLD", 1, None).unwrap().games, 1);
}

#[test]
fn lookup_averages_and_tier_expansion() {
    let mut db = MatchupDB::open_in_memory().unwrap();
    for i in 0..3 {
        db.add_match(&format!("G{i}"), "16.19", "GOLD", &[row(300.0)], &[]).unwrap();
    }
    for i in 0..5 {
        db.add_match(&format!("S{i}"), "16.19", "SILVER", &[row(-100.0)], &[]).unwrap();
    }
    for i in 0..2 {
        db.add_match(&format!("D{i}"), "16.19", "DIAMOND", &[row(1000.0)], &[]).unwrap();
    }
    // GOLD だけで足りる
    let s = db.lookup("top", "Darius", "Garen", "GOLD", 3, None).unwrap();
    assert_eq!((s.games, s.tiers.clone()), (3, vec!["GOLD".to_string()]));
    approx(s.gd10, 300.0);
    approx(s.solo_kill_rate, 1.0);
    // 足りないので SILVER を足す
    let s = db.lookup("top", "Darius", "Garen", "GOLD", 5, None).unwrap();
    assert_eq!((s.games, s.tiers.clone()), (8, vec!["GOLD".to_string(), "SILVER".to_string()]));
    approx(s.gd10, (3.0 * 300.0 - 5.0 * 100.0) / 8.0);
    // 全ランク
    let s = db.lookup("top", "Darius", "Garen", "ALL", 1, None).unwrap();
    assert_eq!((s.games, s.tiers), (10, vec!["ALL".to_string()]));
    // 全ランク帯を足しても届かない → 取れた分を返す
    assert_eq!(db.lookup("top", "Darius", "Garen", "GOLD", 999, None).unwrap().games, 10);
    assert!(db.lookup("top", "Darius", "Nasus", "GOLD", 1, None).is_none());
}

#[test]
fn patch_filter() {
    let mut db = MatchupDB::open_in_memory().unwrap();
    for (id, patch, gd) in [("a", "16.9", 100.0), ("b", "16.10", 200.0), ("c", "16.11", 300.0)] {
        db.add_match(id, patch, "GOLD", &[row(gd)], &[]).unwrap();
    }
    assert_eq!(db.patches(), ["16.9", "16.10", "16.11"]); // 数値順
    assert_eq!(db.recent_patches(2).unwrap(), ["16.10", "16.11"]);
    assert!(db.recent_patches(0).is_none());
    let recent = db.recent_patches(2);
    let s = db.lookup("top", "Darius", "Garen", "GOLD", 1, recent.as_deref()).unwrap();
    assert_eq!(s.games, 2);
    approx(s.gd10, 250.0);
}

#[test]
fn db_role_rates() {
    let mut db = MatchupDB::open_in_memory().unwrap();
    for i in 0..3 {
        db.add_match(&format!("m{i}"), "16.19", "GOLD", &[], &[("Gragas".into(), "jungle".into())]).unwrap();
    }
    db.add_match("m9", "16.19", "GOLD", &[], &[("Gragas".into(), "top".into())]).unwrap();
    let (rates, games) = db.role_rates("Gragas", None);
    assert_eq!(games, 4);
    approx(rates["jungle"], 0.75);
    approx(rates["top"], 0.25);
    assert_eq!(db.role_rates("Nobody", None), (HashMap::new(), 0));
    assert!(MatchupDB::open_if_exists(std::env::temp_dir().join("lane-core-none.sqlite")).is_none());
}

// ---- ポジション推定 ----

fn p(c: &str) -> Player {
    Player { champion: Some(c.into()), ..Default::default() }
}

fn infer(players: &[Player], overrides: Option<&HashMap<String, String>>) -> Vec<lane_core::models::Placement> {
    let (c, t) = (catalog(), traits());
    infer_positions(players, &|cid| t.role_rates(cid, &c), overrides)
}

fn positions(players: &[Player], overrides: Option<&HashMap<String, String>>) -> HashMap<String, &'static str> {
    infer(players, overrides).into_iter().map(|x| (x.player.champion.unwrap_or_default(), x.position)).collect()
}

#[test]
fn standard_comp_in_random_order() {
    let r = positions(&["Thresh", "Ahri", "Darius", "Jinx", "LeeSin"].map(p), None);
    let expect: HashMap<String, &str> =
        [("Darius", "top"), ("LeeSin", "jungle"), ("Ahri", "middle"), ("Jinx", "bottom"), ("Thresh", "utility")]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b))
            .collect();
    assert_eq!(r, expect);
}

#[test]
fn flex_pick_and_smite() {
    let r = positions(&["Gragas", "Aatrox", "Syndra", "Caitlyn", "Lux"].map(p), None);
    assert_eq!((r["Gragas"], r["Aatrox"]), ("jungle", "top"));
    let mut players = ["Darius", "Garen", "Ahri", "Jinx", "Leona"].map(p);
    players[0].has_smite = true;
    let r = positions(&players, None);
    assert_eq!((r["Darius"], r["Garen"]), ("jungle", "top"));
}

#[test]
fn fixed_and_overrides() {
    let players = [
        Player { position: Some("middle".into()), ..p("Jinx") },
        Player { position: Some("bottom".into()), ..p("Ahri") },
    ];
    let pl = infer(&players, None);
    assert_eq!(pl.iter().map(|x| x.position).collect::<Vec<_>>(), ["middle", "bottom"]);
    assert!(pl.iter().all(|x| x.fixed && x.confidence == 1.0));

    let ov: HashMap<String, String> = [("Darius".into(), "middle".into()), ("Ahri".into(), "top".into())].into();
    let r = positions(&["Darius", "LeeSin", "Ahri", "Jinx", "Thresh"].map(p), Some(&ov));
    assert_eq!((r["Darius"], r["Ahri"]), ("middle", "top"));
}

#[test]
fn unknown_champions_and_confidence() {
    let players = [p("Ahri"), Player::default(), Player::default(), Player::default(), Player::default()];
    let pl = infer(&players, None);
    assert_eq!(pl[0].position, "middle");
    assert!(pl[0].confidence > 0.9);
    assert_eq!(pl.iter().map(|x| x.position).collect::<HashSet<_>>().len(), 5);
    // パンテオンは TOP/MID/SUP がほぼ同率 → 確信度は低い
    assert!(infer(&[p("Pantheon")], None)[0].confidence < 0.6);
    assert!(infer(&[], None).is_empty());
    let pl = infer(&[p("Jinx"), p("Thresh")], None);
    assert_eq!(pl.iter().map(|x| x.position).collect::<Vec<_>>(), ["bottom", "utility"]);
}

// ---- 同梱データ ----

#[test]
fn bundled_data_is_consistent() {
    let ids: HashSet<String> = catalog().ids().into_iter().map(String::from).collect();
    let traits: Value = serde_json::from_str(BUNDLED_TRAITS).unwrap();
    let roles: Value = serde_json::from_str(BUNDLED_ROLES).unwrap();
    for (name, table, len) in [("特性表", &traits["traits"], 7), ("ロール表", &roles["rates"], 5)] {
        let table = table.as_object().unwrap();
        let keys: HashSet<String> = table.keys().cloned().collect();
        assert!(ids.difference(&keys).next().is_none(), "{name}に無いチャンピオン: {:?}", ids.difference(&keys));
        assert!(keys.difference(&ids).next().is_none(), "存在しない id が{name}にある: {:?}", keys.difference(&ids));
        for (cid, values) in table {
            let v: Vec<f64> = values.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
            assert_eq!(v.len(), len, "{cid}");
            if len == 7 {
                assert!(v.iter().all(|x| (1.0..=5.0).contains(x)), "{cid}");
            } else {
                assert_eq!(v.iter().sum::<f64>(), 100.0, "{cid}");
            }
        }
    }
}

// ---- 集計バッチ ----

const PARTICIPANTS: [(i64, i64, &str, i64, &str); 10] = [
    (1, 100, "TOP", 122, "Darius"), (2, 100, "JUNGLE", 64, "LeeSin"), (3, 100, "MIDDLE", 103, "Ahri"),
    (4, 100, "BOTTOM", 222, "Jinx"), (5, 100, "UTILITY", 117, "Lulu"),
    (6, 200, "TOP", 10, "Kayle"), (7, 200, "JUNGLE", 20, "Nunu"), (8, 200, "MIDDLE", 61, "Orianna"),
    (9, 200, "BOTTOM", 119, "Draven"), (10, 200, "UTILITY", 89, "Leona"),
];

fn match_json() -> Value {
    let ps: Vec<Value> = PARTICIPANTS
        .iter()
        .map(|(pid, team, pos, cid, name)| json!({"participantId": pid, "teamId": team, "teamPosition": pos, "championId": cid, "championName": name}))
        .collect();
    json!({"info": {"gameVersion": "16.19.712.3456", "queueId": 420, "participants": ps}})
}

fn timeline_json(frames: usize, events: Value) -> Value {
    let mut pf = serde_json::Map::new();
    for (pid, team, ..) in PARTICIPANTS {
        let base = if team == 100 { 4000 } else { 3500 };
        pf.insert(
            pid.to_string(),
            json!({"totalGold": base + pid, "xp": if team == 100 { 5000 } else { 4800 }, "minionsKilled": 80, "jungleMinionsKilled": 0}),
        );
    }
    let mut out: Vec<Value> = (0..frames).map(|_| json!({"participantFrames": {}, "events": []})).collect();
    if frames > 10 {
        out[10]["participantFrames"] = Value::Object(pf);
    }
    out[3]["events"] = events;
    json!({"info": {"frames": out}})
}

#[test]
fn aggregate_basic() {
    let events = json!([
        {"type": "CHAMPION_KILL", "timestamp": 200000, "killerId": 1, "victimId": 6, "assistingParticipantIds": []},
        {"type": "CHAMPION_KILL", "timestamp": 250000, "killerId": 2, "victimId": 6, "assistingParticipantIds": [1]},
        {"type": "CHAMPION_KILL", "timestamp": 700000, "killerId": 1, "victimId": 6}
    ]);
    let c = catalog();
    let agg = aggregate_match(&match_json(), &timeline_json(11, events), Some(&c)).unwrap();
    assert_eq!(patch_of("16.19.712.3456"), "16.19");
    assert_eq!(agg.patch, "16.19");
    assert_eq!(agg.rows.len(), 10); // 5レーン × 両方向
    let top = agg.rows.iter().find(|r| r.lane == "top" && r.champ == "Darius").unwrap();
    assert_eq!(top.opp, "Kayle");
    approx(top.gd10, (4000.0 + 1.0) - (3500.0 + 6.0));
    approx(top.xpd10, 200.0);
    assert_eq!((top.solo_kills, top.solo_deaths), (1, 0));
    let rev = agg.rows.iter().find(|r| r.lane == "top" && r.champ == "Kayle").unwrap();
    approx(rev.gd10, -top.gd10);
    assert_eq!(rev.solo_deaths, 1);
    assert!(agg.roles.contains(&("Lulu".into(), "utility".into())));
}

#[test]
fn aggregate_skips_bad_games() {
    let c = catalog();
    assert!(aggregate_match(&match_json(), &timeline_json(8, json!([])), Some(&c)).is_none());
    let mut m = match_json();
    for p in m["info"]["participants"].as_array_mut().unwrap() {
        p["teamPosition"] = json!("");
    }
    assert!(aggregate_match(&m, &timeline_json(11, json!([])), Some(&c)).is_none());
}

#[test]
fn rate_limiter_waits() {
    let now = Rc::new(Cell::new(0.0));
    let slept = Rc::new(RefCell::new(vec![]));
    let (n1, n2, s2) = (now.clone(), now.clone(), slept.clone());
    let mut rl = RateLimiter::new(
        vec![(2, 1.0), (3, 10.0)],
        Box::new(move || n1.get()),
        Box::new(move |s| {
            s2.borrow_mut().push(s);
            n2.set(n2.get() + s);
        }),
    );
    rl.wait();
    rl.wait();
    assert!(slept.borrow().is_empty());
    rl.wait(); // 1秒2回の制限で待つ
    approx(now.get(), 1.01);
    rl.wait(); // 10秒3回の制限で待つ
    approx(now.get(), 10.01);
}

#[test]
fn counter_picks_are_role_filtered_and_sorted() {
    use lane_core::analyzer::{Analyzer, Settings};
    let analyzer = Analyzer::new(catalog(), traits(), Settings::default(), None);
    for (pos, enemy) in [("top", "Darius"), ("middle", "Zed"), ("bottom", "Jinx"), ("utility", "Lulu")] {
        let picks = analyzer.counter_picks(pos, enemy, 10);
        assert!(!picks.is_empty(), "{pos}");
        assert!(picks.len() <= 10);
        assert!(picks.windows(2).all(|w| w[0].score >= w[1].score), "{pos} is sorted");
        assert!(picks.iter().all(|c| c.champion != enemy));
        let rates = |c: &str| analyzer.rates_for(c).get(pos).copied().unwrap_or(0.0);
        assert!(picks.iter().all(|c| rates(&c.champion) >= 0.10), "{pos} role filter");
    }
}
