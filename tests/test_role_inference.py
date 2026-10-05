from app.models import Player
from app.role_inference import infer_positions


def _positions(placements):
    return {p.player.champion: p.position for p in placements}


def test_standard_comp_in_random_order(traits):
    players = [Player(c) for c in ["Thresh", "Ahri", "Darius", "Jinx", "LeeSin"]]
    result = _positions(infer_positions(players, traits.role_rates))
    assert result == {"Darius": "top", "LeeSin": "jungle", "Ahri": "middle", "Jinx": "bottom", "Thresh": "utility"}


def test_flex_pick_resolved_by_rest_of_team(traits):
    # グラガスは TOP/JG 両方あり得るが、他に JG がいなければ JG
    players = [Player(c) for c in ["Gragas", "Aatrox", "Syndra", "Caitlyn", "Lux"]]
    result = _positions(infer_positions(players, traits.role_rates))
    assert result["Gragas"] == "jungle"
    assert result["Aatrox"] == "top"


def test_smite_forces_jungle(traits):
    # ダリウスは普通 TOP だが、スマイト持ちなら JG
    players = [Player("Darius", has_smite=True), Player("Garen"), Player("Ahri"), Player("Jinx"), Player("Leona")]
    result = _positions(infer_positions(players, traits.role_rates))
    assert result["Darius"] == "jungle"
    assert result["Garen"] == "top"


def test_assigned_positions_are_fixed(traits):
    players = [Player("Jinx", position="middle"), Player("Ahri", position="bottom")]
    placements = infer_positions(players, traits.role_rates)
    assert [p.position for p in placements] == ["middle", "bottom"]
    assert all(p.fixed and p.confidence == 1.0 for p in placements)


def test_overrides_win(traits):
    players = [Player(c) for c in ["Darius", "LeeSin", "Ahri", "Jinx", "Thresh"]]
    result = _positions(infer_positions(players, traits.role_rates, {"Darius": "middle", "Ahri": "top"}))
    assert result["Darius"] == "middle"
    assert result["Ahri"] == "top"


def test_unknown_champions_and_confidence(traits):
    players = [Player("Ahri"), Player(), Player(), Player(), Player()]
    placements = infer_positions(players, traits.role_rates)
    assert placements[0].position == "middle"
    assert placements[0].confidence > 0.9
    # 未選択の4人には残りのポジションが重複なく割り当たる
    assert len({p.position for p in placements}) == 5


def test_low_confidence_for_ambiguous(traits):
    # パンテオンは TOP/MID/SUP がほぼ同率 → 確信度は低い
    placements = infer_positions([Player("Pantheon")], traits.role_rates)
    assert placements[0].confidence < 0.6


def test_fewer_than_five_players(traits):
    assert infer_positions([], traits.role_rates) == []
    placements = infer_positions([Player("Jinx"), Player("Thresh")], traits.role_rates)
    assert [p.position for p in placements] == ["bottom", "utility"]
