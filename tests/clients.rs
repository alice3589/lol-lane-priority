//! LCU / Live Client のレスポンス解析（Python 版 tests/test_clients.py の移植）。
use lane_core::champions::ChampionCatalog;
use lane_core::config::BUNDLED_CHAMPIONS;
use lane_core::sources::{parse_live, parse_session};
use serde_json::json;

fn catalog() -> ChampionCatalog {
    ChampionCatalog::from_json(BUNDLED_CHAMPIONS).unwrap()
}

fn champs(players: &[lane_core::models::Player]) -> Vec<Option<&str>> {
    players.iter().map(|p| p.champion.as_deref()).collect()
}

#[test]
fn parse_session_works() {
    let session = json!({
        "localPlayerCellId": 2,
        "myTeam": [
            {"cellId": 0, "championId": 122, "championPickIntent": 0, "assignedPosition": "top", "spell1Id": 4, "spell2Id": 12, "team": 2},
            {"cellId": 1, "championId": 64, "championPickIntent": 0, "assignedPosition": "jungle", "spell1Id": 11, "spell2Id": 4, "team": 2},
            {"cellId": 2, "championId": 0, "championPickIntent": 103, "assignedPosition": "middle", "spell1Id": 4, "spell2Id": 14, "team": 2},
            {"cellId": 3, "championId": 222, "championPickIntent": 0, "assignedPosition": "bottom", "spell1Id": 4, "spell2Id": 7, "team": 2},
            {"cellId": 4, "championId": 0, "championPickIntent": 0, "assignedPosition": "utility", "spell1Id": 0, "spell2Id": 0, "team": 2}
        ],
        "theirTeam": [
            {"cellId": 5, "championId": 10, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
            {"cellId": 6, "championId": 20, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
            {"cellId": 7, "championId": 0, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
            {"cellId": 8, "championId": 119, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
            {"cellId": 9, "championId": 0, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1}
        ],
        "actions": [
            [{"actorCellId": 0, "championId": 122, "completed": true, "type": "pick"}],
            [{"actorCellId": 3, "championId": 222, "completed": false, "type": "pick", "isInProgress": true}]
        ]
    });
    let s = parse_session(&session, &catalog());
    assert_eq!(s.phase, "champ_select");
    assert_eq!(s.side.as_deref(), Some("red"));
    assert_eq!(champs(&s.ally), [Some("Darius"), Some("LeeSin"), Some("Ahri"), Some("Jinx"), None]);
    let pos: Vec<_> = s.ally.iter().map(|p| p.position.as_deref().unwrap()).collect();
    assert_eq!(pos, ["top", "jungle", "middle", "bottom", "utility"]);
    assert!(s.ally[1].has_smite && !s.ally[0].has_smite);
    assert!(s.ally[2].hovering && s.ally[2].is_local); // pickIntent のみ
    assert!(s.ally[3].hovering); // アクション未完了
    assert!(!s.ally[0].hovering);
    assert_eq!(champs(&s.enemy), [Some("Kayle"), Some("Nunu"), None, Some("Draven"), None]);
    assert!(s.enemy.iter().all(|p| p.position.is_none()));
}

#[test]
fn parse_live_works() {
    let live = json!({
        "activePlayer": {"riotId": "Me#JP1"},
        "allPlayers": [
            {"riotId": "Me#JP1", "team": "CHAOS", "championName": "ダリウス", "rawChampionName": "game_character_displayname_Darius",
             "position": "TOP", "summonerSpells": {"summonerSpellOne": {"rawDisplayName": "GeneratedTip_SummonerSpell_SummonerFlash_DisplayName"}}},
            {"riotId": "Jg#JP1", "team": "CHAOS", "championName": "リー・シン", "rawChampionName": "game_character_displayname_LeeSin",
             "position": "", "summonerSpells": {"summonerSpellOne": {"rawDisplayName": "GeneratedTip_SummonerSpell_SummonerSmite_DisplayName"}}},
            {"riotId": "Enemy#JP1", "team": "ORDER", "championName": "ケイル", "rawChampionName": "game_character_displayname_Kayle",
             "position": "", "summonerSpells": {}}
        ]
    });
    let cat = catalog();
    let s = parse_live(&live, &cat).unwrap();
    assert_eq!((s.phase.as_str(), s.side.as_deref()), ("in_game", Some("red")));
    assert_eq!(champs(&s.ally), [Some("Darius"), Some("LeeSin")]);
    assert_eq!(s.ally[0].position.as_deref(), Some("top"));
    assert!(s.ally[0].is_local);
    assert!(s.ally[1].has_smite && s.ally[1].position.is_none());
    assert_eq!(champs(&s.enemy), [Some("Kayle")]);
    assert!(parse_live(&json!({"allPlayers": []}), &cat).is_none());
}

#[test]
fn catalog_resolve() {
    let c = catalog();
    assert_eq!(c.resolve(Some("FiddleSticks")), Some("Fiddlesticks"));
    assert_eq!(c.resolve(Some("game_character_displayname_MonkeyKing")), Some("MonkeyKing"));
    assert_eq!(c.resolve(Some("ウーコン")), Some("MonkeyKing"));
    assert_eq!(c.resolve(Some("???")), None);
    assert_eq!(c.id_by_key(Some(266)), Some("Aatrox"));
}
