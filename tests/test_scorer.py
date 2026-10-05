import pytest

from app.matchup_db import LaneRow, MatchupDB
from app.scorer import Scorer, advice, verdict_of


@pytest.fixture
def scorer(catalog, traits):
    return Scorer(catalog, traits)


def test_trait_only_basic_matchups(scorer):
    # 射程・序盤で勝るケイトリン vs スモルダーは味方有利
    r = scorer.score_lane("bottom", ["Caitlyn", None], ["Smolder", None])
    assert r.score > 10 and r.verdict == "ally"
    # ダリウス vs ケイル（序盤最弱クラス）も味方有利
    r = scorer.score_lane("top", ["Darius"], ["Kayle"])
    assert r.verdict == "ally"
    # 同じチャンピオン同士は 0
    r = scorer.score_lane("middle", ["Ahri"], ["Ahri"])
    assert r.score == pytest.approx(0.0)
    assert r.verdict == "even"


def test_antisymmetric(scorer):
    for lane, a, b in [
        ("top", ["Renekton"], ["Vladimir"]),
        ("jungle", ["LeeSin"], ["Karthus"]),
        ("bottom", ["Draven", "Leona"], ["Jinx", "Lulu"]),
    ]:
        assert scorer.score_lane(lane, a, b).score == pytest.approx(-scorer.score_lane(lane, b, a).score)


def test_reasons_sum_to_score(scorer):
    r = scorer.score_lane("bottom", ["Draven", "Leona"], ["Jinx", "Lulu"])
    assert sum(x.points for x in r.reasons) == pytest.approx(r.score)
    assert r.weight_a == 0 and r.weight_b == 1


def test_jungle_uses_jungle_traits(scorer):
    r = scorer.score_lane("jungle", ["XinZhao"], ["Ivern"])
    labels = {x.label for x in r.reasons}
    assert labels == {"クリア速度", "ガンク力", "インベード耐性"}
    assert r.verdict == "ally"


def test_unknown_when_champion_missing(scorer):
    r = scorer.score_lane("top", ["Darius"], [None])
    assert r.score is None and r.verdict == "unknown"
    assert "敵のチャンピオンが未確定" in r.notes


def test_bot_with_one_support_missing(scorer):
    r = scorer.score_lane("bottom", ["Caitlyn", "Lux"], ["Smolder", None])
    assert r.score is not None
    assert any("SUPが未確定" in n for n in r.notes)


def test_stats_dominate_when_enough_games(tmp_path, catalog, traits):
    db = MatchupDB(tmp_path / "m.sqlite")
    rows = [LaneRow("middle", "Ahri", "Syndra", 600, 300, 10, 0, 0)]
    for i in range(300):
        db.add_match(f"S{i}", "16.19", "GOLD", rows, [])
    scorer = Scorer(catalog, traits, db, tier="GOLD", min_games=200)
    r = scorer.score_lane("middle", ["Ahri"], ["Syndra"])
    assert r.games == 300
    assert r.weight_a == pytest.approx(2 / 3)
    assert r.verdict == "ally"
    assert sum(x.points for x in r.reasons) == pytest.approx(r.score)
    assert any(x.source == "A" and "10分ゴールド差" in x.label for x in r.reasons)
    db.close()


def test_stats_weight_scales_with_sample_size(tmp_path, catalog, traits):
    db = MatchupDB(tmp_path / "m.sqlite")
    rows = [LaneRow("top", "Garen", "Darius", 400, 0, 0, 0, 0)]
    for i in range(50):
        db.add_match(f"G{i}", "16.19", "GOLD", rows, [])
    scorer = Scorer(catalog, traits, db, tier="GOLD", min_games=200)
    r = scorer.score_lane("top", ["Garen"], ["Darius"])
    # 50/200 = 0.25 → A の生重み 0.15、B 0.3+0.45
    assert r.weight_a == pytest.approx(0.15 / 0.9)
    assert any("少ない" in n for n in r.notes)
    db.close()


def test_verdict_thresholds():
    assert verdict_of(None) == "unknown"
    assert verdict_of(10) == "ally"
    assert verdict_of(9.9) == "even"
    assert verdict_of(-10) == "enemy"


def test_advice(scorer):
    lanes = [
        scorer.score_lane("top", ["Darius"], ["Kayle"]),
        scorer.score_lane("jungle", ["LeeSin"], ["LeeSin"]),
        scorer.score_lane("middle", ["Ahri"], ["Ahri"]),
        scorer.score_lane("bottom", ["Smolder", "Soraka"], ["Caitlyn", "Leona"]),
    ]
    lines = advice(lanes)
    assert any("TOP 側に主導権" in line for line in lines)
