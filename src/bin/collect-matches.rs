//! Match-V5 からマッチアップ実績を集計して SQLite に保存する。
//!
//! 使い方（PowerShell）:
//!     $env:RIOT_API_KEY = "RGAPI-..."
//!     collect-matches --platform jp1 --tiers GOLD,PLATINUM --players 30 --matches 20
//!
//! 1試合ごとに保存するので、Ctrl+C で止めても取り込み済みの分は残り、次回は続きから再開する。
use lane_core::champions::{cache_path, ChampionCatalog};
use lane_core::collect::{collect, RiotApi};
use lane_core::config::{default_db_path, BUNDLED_CHAMPIONS};
use lane_core::matchup_db::MatchupDB;
use lane_core::models::TIERS;
use std::process::ExitCode;

const HELP: &str = "Match-V5 からマッチアップ実績を集計する

オプション:
  --platform <名前>   jp1 / kr / na1 / euw1 など（既定: jp1）
  --tiers <一覧>      カンマ区切りのランク帯（既定: 全ランク帯）
  --players <数>      ランク帯ごとのプレイヤー数（既定: 20）
  --matches <数>      プレイヤーごとの試合数（最大100、既定: 20）
  --db <パス>         保存先 SQLite（既定: exe と同じ場所の data\\matchups.sqlite）
  --api-key <キー>    省略時は環境変数 RIOT_API_KEY";

fn main() -> ExitCode {
    let mut platform = "jp1".to_string();
    let mut tiers = TIERS.join(",");
    let mut players = 20usize;
    let mut matches = 20usize;
    let mut db_path = default_db_path().to_string_lossy().into_owned();
    let mut api_key = std::env::var("RIOT_API_KEY").unwrap_or_default();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "-h" || a == "--help" {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        let Some(v) = args.next() else {
            eprintln!("{a} の値がありません\n\n{HELP}");
            return ExitCode::from(2);
        };
        let parse = |v: &str| v.parse::<usize>().map_err(|_| format!("{a} には数を指定してください"));
        let r = match a.as_str() {
            "--platform" => Ok(platform = v),
            "--tiers" => Ok(tiers = v),
            "--players" => parse(&v).map(|n| players = n),
            "--matches" => parse(&v).map(|n| matches = n),
            "--db" => Ok(db_path = v),
            "--api-key" => Ok(api_key = v),
            _ => Err(format!("不明なオプション: {a}\n\n{HELP}")),
        };
        if let Err(e) = r {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    }

    if api_key.is_empty() {
        eprintln!("Riot API キーがありません。--api-key か環境変数 RIOT_API_KEY を設定してください。");
        return ExitCode::from(2);
    }
    let tiers: Vec<String> = tiers.split(',').map(|t| t.trim().to_uppercase()).filter(|t| !t.is_empty()).collect();
    let unknown: Vec<&String> = tiers.iter().filter(|t| !TIERS.contains(&t.as_str())).collect();
    if !unknown.is_empty() {
        eprintln!("不明なランク帯: {unknown:?}（使えるもの: {}）", TIERS.join(", "));
        return ExitCode::from(2);
    }

    let mut api = match RiotApi::new(&api_key, &platform) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if let Some(dir) = std::path::Path::new(&db_path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut db = match MatchupDB::open(&db_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("DB を開けません: {e}");
            return ExitCode::FAILURE;
        }
    };
    let catalog = ChampionCatalog::load_from_str(BUNDLED_CHAMPIONS, Some(&cache_path())).ok();
    match collect(&mut api, &mut db, &tiers, players, matches.min(100), catalog.as_ref(), &mut |m| println!("{m}")) {
        Ok(added) => {
            println!("完了: {added} 試合を追加しました → {db_path}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("エラー: {e}（取り込み済みの分は保存されています）");
            ExitCode::FAILURE
        }
    }
}
