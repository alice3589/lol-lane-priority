"""同梱データの整合性チェック。"""
import json

from app.config import DATA_DIR


def _load(name):
    return json.loads((DATA_DIR / name).read_text(encoding="utf-8"))


def test_all_champions_have_traits_and_roles(catalog):
    traits = _load("champion_traits.json")["traits"]
    roles = _load("role_rates.json")["rates"]
    ids = set(catalog.ids())
    assert ids - set(traits) == set(), "特性表に無いチャンピオン"
    assert ids - set(roles) == set(), "ロール表に無いチャンピオン"
    assert set(traits) - ids == set(), "存在しない id が特性表にある"
    assert set(roles) - ids == set(), "存在しない id がロール表にある"


def test_value_ranges():
    for cid, values in _load("champion_traits.json")["traits"].items():
        assert len(values) == 7, cid
        assert all(1 <= v <= 5 for v in values), cid
    for cid, values in _load("role_rates.json")["rates"].items():
        assert len(values) == 5, cid
        assert sum(values) == 100, cid
