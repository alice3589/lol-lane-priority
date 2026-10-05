import pytest

from app.matchup_db import LaneRow, MatchupDB, tier_search_order


def _row(gd, lane="top", champ="Darius", opp="Garen"):
    return LaneRow(lane, champ, opp, gd, gd / 2, 1, 1, 0)


def test_tier_search_order():
    order = tier_search_order("GOLD")
    assert order[0] == ["GOLD"]
    assert order[1] == ["GOLD", "SILVER"]
    assert order[2] == ["GOLD", "SILVER", "PLATINUM"]
    assert order[-1][-1] == "CHALLENGER"
    assert tier_search_order("IRON")[1] == ["IRON", "BRONZE"]


def test_add_match_is_idempotent(tmp_path):
    db = MatchupDB(tmp_path / "m.sqlite")
    assert db.add_match("JP1_1", "16.19", "GOLD", [_row(100)], [("Darius", "top")])
    assert not db.add_match("JP1_1", "16.19", "GOLD", [_row(100)], [("Darius", "top")])
    s = db.lookup("top", "Darius", "Garen", "GOLD", 1)
    assert s.games == 1
    db.close()


def test_lookup_averages_and_tier_expansion(tmp_path):
    db = MatchupDB(tmp_path / "m.sqlite")
    for i in range(3):
        db.add_match(f"G{i}", "16.19", "GOLD", [_row(300)], [])
    for i in range(5):
        db.add_match(f"S{i}", "16.19", "SILVER", [_row(-100)], [])
    for i in range(2):
        db.add_match(f"D{i}", "16.19", "DIAMOND", [_row(1000)], [])

    # GOLD だけで足りる
    s = db.lookup("top", "Darius", "Garen", "GOLD", min_games=3)
    assert s.games == 3 and s.gd10 == pytest.approx(300) and s.tiers == ["GOLD"]
    assert s.solo_kill_rate == pytest.approx(1.0)

    # 足りないので SILVER を足す
    s = db.lookup("top", "Darius", "Garen", "GOLD", min_games=5)
    assert s.games == 8 and s.tiers == ["GOLD", "SILVER"]
    assert s.gd10 == pytest.approx((3 * 300 - 5 * 100) / 8)

    # 全ランク
    s = db.lookup("top", "Darius", "Garen", "ALL", min_games=1)
    assert s.games == 10 and s.tiers == ["ALL"]

    # 全ランク帯を足しても min_games に届かない → 取れた分を返す
    s = db.lookup("top", "Darius", "Garen", "GOLD", min_games=999)
    assert s.games == 10

    assert db.lookup("top", "Darius", "Nasus", "GOLD", 1) is None
    db.close()


def test_patch_filter(tmp_path):
    db = MatchupDB(tmp_path / "m.sqlite")
    db.add_match("a", "16.9", "GOLD", [_row(100)], [])
    db.add_match("b", "16.10", "GOLD", [_row(200)], [])
    db.add_match("c", "16.11", "GOLD", [_row(300)], [])
    assert db.patches() == ["16.9", "16.10", "16.11"]  # 数値順
    assert db.recent_patches(2) == ["16.10", "16.11"]
    assert db.recent_patches(0) is None
    s = db.lookup("top", "Darius", "Garen", "GOLD", 1, patches=db.recent_patches(2))
    assert s.games == 2 and s.gd10 == pytest.approx(250)
    db.close()


def test_role_rates(tmp_path):
    db = MatchupDB(tmp_path / "m.sqlite")
    for i in range(3):
        db.add_match(f"m{i}", "16.19", "GOLD", [], [("Gragas", "jungle")])
    db.add_match("m9", "16.19", "GOLD", [], [("Gragas", "top")])
    rates, games = db.role_rates("Gragas")
    assert games == 4
    assert rates["jungle"] == pytest.approx(0.75)
    assert rates["top"] == pytest.approx(0.25)
    assert db.role_rates("Nobody") == ({}, 0)
    db.close()


def test_open_if_exists(tmp_path):
    assert MatchupDB.open_if_exists(tmp_path / "none.sqlite") is None
