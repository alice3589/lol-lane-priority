"""LCU / Live Client のレスポンス解析。"""
from app.lcu_client import parse_commandline, parse_lockfile, parse_session
from app.live_client import parse_live


def test_parse_lockfile():
    c = parse_lockfile("LeagueClient:12345:54321:abcDEF_123:https\n")
    assert c.port == 54321 and c.password == "abcDEF_123" and c.protocol == "https"
    assert c.base_url == "https://127.0.0.1:54321"
    assert parse_lockfile("broken") is None


def test_parse_commandline():
    cmd = '"C:/Riot Games/League of Legends/LeagueClientUx.exe" "--remoting-auth-token=Xy-12_ab" "--app-port=61234"'
    c = parse_commandline(cmd)
    assert c.port == 61234 and c.password == "Xy-12_ab"
    assert parse_commandline("nothing") is None


SESSION = {
    "localPlayerCellId": 2,
    "myTeam": [
        {"cellId": 0, "championId": 122, "championPickIntent": 0, "assignedPosition": "top", "spell1Id": 4, "spell2Id": 12, "team": 2},
        {"cellId": 1, "championId": 64, "championPickIntent": 0, "assignedPosition": "jungle", "spell1Id": 11, "spell2Id": 4, "team": 2},
        {"cellId": 2, "championId": 0, "championPickIntent": 103, "assignedPosition": "middle", "spell1Id": 4, "spell2Id": 14, "team": 2},
        {"cellId": 3, "championId": 222, "championPickIntent": 0, "assignedPosition": "bottom", "spell1Id": 4, "spell2Id": 7, "team": 2},
        {"cellId": 4, "championId": 0, "championPickIntent": 0, "assignedPosition": "utility", "spell1Id": 0, "spell2Id": 0, "team": 2},
    ],
    "theirTeam": [
        {"cellId": 5, "championId": 10, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
        {"cellId": 6, "championId": 20, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
        {"cellId": 7, "championId": 0, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
        {"cellId": 8, "championId": 119, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
        {"cellId": 9, "championId": 0, "assignedPosition": "", "spell1Id": 0, "spell2Id": 0, "team": 1},
    ],
    "actions": [
        [{"actorCellId": 0, "championId": 122, "completed": True, "type": "pick"}],
        [{"actorCellId": 3, "championId": 222, "completed": False, "type": "pick", "isInProgress": True}],
    ],
}


def test_parse_session(catalog):
    s = parse_session(SESSION, catalog)
    assert s.phase == "champ_select"
    assert s.side == "red"
    assert [p.champion for p in s.ally] == ["Darius", "LeeSin", "Ahri", "Jinx", None]
    assert [p.position for p in s.ally] == ["top", "jungle", "middle", "bottom", "utility"]
    assert s.ally[1].has_smite and not s.ally[0].has_smite
    assert s.ally[2].hovering  # pickIntent のみ
    assert s.ally[2].is_local
    assert s.ally[3].hovering  # アクション未完了
    assert not s.ally[0].hovering
    assert [p.champion for p in s.enemy] == ["Kayle", "Nunu", None, "Draven", None]
    assert all(p.position is None for p in s.enemy)


LIVE = {
    "activePlayer": {"riotId": "Me#JP1"},
    "allPlayers": [
        {"riotId": "Me#JP1", "team": "CHAOS", "championName": "ダリウス", "rawChampionName": "game_character_displayname_Darius",
         "position": "TOP", "summonerSpells": {"summonerSpellOne": {"rawDisplayName": "GeneratedTip_SummonerSpell_SummonerFlash_DisplayName"}}},
        {"riotId": "Jg#JP1", "team": "CHAOS", "championName": "リー・シン", "rawChampionName": "game_character_displayname_LeeSin",
         "position": "", "summonerSpells": {"summonerSpellOne": {"rawDisplayName": "GeneratedTip_SummonerSpell_SummonerSmite_DisplayName"}}},
        {"riotId": "Enemy#JP1", "team": "ORDER", "championName": "ケイル", "rawChampionName": "game_character_displayname_Kayle",
         "position": "", "summonerSpells": {}},
    ],
}


def test_parse_live(catalog):
    s = parse_live(LIVE, catalog)
    assert s.phase == "in_game" and s.side == "red"
    assert [p.champion for p in s.ally] == ["Darius", "LeeSin"]
    assert s.ally[0].position == "top" and s.ally[0].is_local
    assert s.ally[1].has_smite and s.ally[1].position is None
    assert [p.champion for p in s.enemy] == ["Kayle"]
    assert parse_live({"allPlayers": []}, catalog) is None


def test_catalog_resolve(catalog):
    assert catalog.resolve("FiddleSticks") == "Fiddlesticks"
    assert catalog.resolve("game_character_displayname_MonkeyKing") == "MonkeyKing"
    assert catalog.resolve("ウーコン") == "MonkeyKing"
    assert catalog.resolve("???") is None
    assert catalog.id_by_key(266) == "Aatrox"


def test_lcu_search_is_throttled(monkeypatch):
    import app.lcu_client as lc

    calls = []
    monkeypatch.setattr(lc, "find_credentials", lambda path: calls.append(path))
    client = lc.LcuClient()
    assert not client.ensure_connected()
    assert not client.ensure_connected()
    assert len(calls) == 1  # 5秒以内は探し直さない


def test_client_source_flow(monkeypatch, catalog):
    import app.sources as src_mod

    monkeypatch.setattr(src_mod, "fetch_live", lambda session: None)
    source = src_mod.ClientSource(catalog)
    phase = {"value": "ChampSelect"}
    monkeypatch.setattr(source.lcu, "ensure_connected", lambda: True)
    monkeypatch.setattr(source.lcu, "gameflow_phase", lambda: phase["value"])
    monkeypatch.setattr(source.lcu, "champ_select_session", lambda: SESSION)

    assert source.poll().phase == "champ_select"
    phase["value"] = "InProgress"  # ロード画面: チャンセレの結果を出し続ける
    s = source.poll()
    assert s.phase == "loading" and s.ally[0].champion == "Darius"
    phase["value"] = "EndOfGame"
    assert source.poll().phase == "idle"
    phase["value"] = "InProgress"
    assert source.poll().phase == "idle"  # 前回のチャンセレは捨てている
