//! 旧 Python 版の計算結果（golden.json。Python 版を削除する前に書き出したもの）と同じ結果になるかの確認。
use lane_core::analyzer::{Analyzer, Settings};
use lane_core::champions::ChampionCatalog;
use lane_core::matchup_db::{LaneRow, MatchupDB};
use lane_core::models::{GameState, Player};
use lane_core::scorer::{LaneResult, Scorer};
use lane_core::traits::TraitTable;
use serde_json::Value;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load() -> (ChampionCatalog, TraitTable, Value) {
    let data = root().join("data");
    let catalog = ChampionCatalog::load(&data.join("champions_ja.json"), None).unwrap();
    let traits = TraitTable::load(&data).unwrap();
    let golden = serde_json::from_str(&std::fs::read_to_string(root().join("tests/golden.json")).unwrap()).unwrap();
    (catalog, traits, golden)
}

fn opts(v: &Value) -> Vec<Option<&str>> {
    v.as_array().unwrap().iter().map(|x| x.as_str()).collect()
}

fn close(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() < 1e-9, "{what}: rust={a} python={b}");
}

fn check_lane(r: &LaneResult, e: &Value, ctx: &str) {
    match e["score"].as_f64() {
        Some(s) => close(r.score.expect(ctx), s, &format!("{ctx} score")),
        None => assert!(r.score.is_none(), "{ctx}: score should be None"),
    }
    assert_eq!(r.verdict, e["verdict"].as_str().unwrap(), "{ctx} verdict");
    close(r.weight_a, e["weight_a"].as_f64().unwrap(), &format!("{ctx} weight_a"));
    close(r.weight_b, e["weight_b"].as_f64().unwrap(), &format!("{ctx} weight_b"));
    assert_eq!(r.games, e["games"].as_i64().unwrap(), "{ctx} games");
    let notes: Vec<&str> = e["notes"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    assert_eq!(r.notes, notes, "{ctx} notes");
    let reasons = e["reasons"].as_array().unwrap();
    assert_eq!(r.reasons.len(), reasons.len(), "{ctx} reasons len");
    for (x, y) in r.reasons.iter().zip(reasons) {
        assert_eq!(x.source, y[0].as_str().unwrap(), "{ctx} source");
        assert_eq!(x.label, y[1].as_str().unwrap(), "{ctx} label");
        assert_eq!(x.detail, y[2].as_str().unwrap(), "{ctx} detail");
        close(x.points, y[3].as_f64().unwrap(), &format!("{ctx} points {}", x.label));
    }
}

#[test]
fn lanes_without_db() {
    let (catalog, traits, golden) = load();
    let scorer = Scorer::new(&catalog, &traits);
    for c in golden["lanes"].as_array().unwrap() {
        let lane = c["lane"].as_str().unwrap();
        let r = scorer.score_lane(lane, &opts(&c["ally"]), &opts(&c["enemy"]));
        check_lane(&r, &c["expect"], &format!("{lane} {} vs {}", c["ally"], c["enemy"]));
    }
}

#[test]
fn lanes_with_db() {
    let (catalog, traits, golden) = load();
    let mut db = MatchupDB::open_in_memory().unwrap();
    let row = |lane: &str, c: &str, o: &str, gd, xp, cs, sk, sd| LaneRow {
        lane: lane.into(), champ: c.into(), opp: o.into(), gd10: gd, xpd10: xp, csd10: cs, solo_kills: sk, solo_deaths: sd,
    };
    for i in 0..300 {
        db.add_match(&format!("S{i}"), "16.19", "GOLD", &[row("middle", "Ahri", "Syndra", 600., 300., 10., 1, 0)], &[]).unwrap();
    }
    for i in 0..50 {
        db.add_match(&format!("G{i}"), "16.19", "GOLD", &[row("top", "Garen", "Darius", 400., 0., 0., 0, 2)], &[]).unwrap();
    }
    for i in 0..120 {
        db.add_match(
            &format!("B{i}"), "16.20", "SILVER",
            &[row("bottom", "Jinx", "Caitlyn", -200., -50., -3., 0, 1)],
            &[("Jinx".into(), "bottom".into())],
        ).unwrap();
    }
    let mut scorer = Scorer::new(&catalog, &traits);
    scorer.db = Some(&db);
    scorer.tier = "GOLD".into();
    for c in golden["db_lanes"].as_array().unwrap() {
        let lane = c["lane"].as_str().unwrap();
        let r = scorer.score_lane(lane, &opts(&c["ally"]), &opts(&c["enemy"]));
        check_lane(&r, &c["expect"], &format!("db {lane}"));
    }
}

fn players(v: &Value) -> Vec<Player> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| Player {
            champion: p[0].as_str().map(String::from),
            position: p[1].as_str().map(String::from),
            has_smite: p[2].as_bool().unwrap(),
            ..Default::default()
        })
        .collect()
}

#[test]
fn full_analysis() {
    let (catalog, traits, golden) = load();
    let analyzer = Analyzer::new(catalog, traits, Settings::default(), None);
    for c in golden["analysis"].as_array().unwrap() {
        let state = GameState { ally: players(&c["ally"]), enemy: players(&c["enemy"]), ..Default::default() };
        let an = analyzer.analyze(&state, None);
        let e = &c["expect"];
        for (side, got) in [("ally", &an.ally), ("enemy", &an.enemy)] {
            let exp = e[side].as_array().unwrap();
            assert_eq!(got.len(), exp.len());
            for (g, x) in got.iter().zip(exp) {
                assert_eq!(g.player.champion.as_deref(), x[0].as_str(), "{side} champion");
                assert_eq!(g.position, x[1].as_str().unwrap(), "{side} position of {:?}", g.player.champion);
                close(g.confidence, x[2].as_f64().unwrap(), "confidence");
                assert_eq!(g.fixed, x[3].as_bool().unwrap());
            }
        }
        for (r, x) in an.lanes.iter().zip(e["lanes"].as_array().unwrap()) {
            assert_eq!(r.lane, x["lane"].as_str().unwrap());
            check_lane(r, x, &format!("analysis {}", r.lane));
        }
        let advice: Vec<&str> = e["advice"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        assert_eq!(an.advice, advice);
    }
}
