"""チャンピオン特性表とロール出現率の読み込み。

表に載っていないチャンピオン（新チャンプなど）は Data Dragon のタグから既定値を作る。
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from .champions import ChampionCatalog
from .config import DATA_DIR
from .models import POSITIONS

TRAIT_FIELDS = ("early", "spike6", "waveclear", "mobility", "clear", "gank", "duel")

TRAIT_LABELS = {
    "early": "序盤(Lv1-5)の強さ",
    "spike6": "Lv6スパイク",
    "waveclear": "ウェーブクリア",
    "mobility": "機動力",
    "clear": "クリア速度",
    "gank": "ガンク力",
    "duel": "インベード耐性",
}

# タグ別の既定値（表にないチャンピオン用）
_TAG_TRAITS = {
    "Marksman": (3, 3, 3, 2, 1, 1, 1),
    "Mage": (3, 4, 4, 1, 1, 2, 1),
    "Assassin": (3, 4, 3, 4, 3, 4, 3),
    "Fighter": (3, 4, 3, 3, 3, 3, 4),
    "Tank": (3, 3, 3, 2, 3, 3, 3),
    "Support": (3, 3, 1, 2, 1, 2, 1),
}
_DEFAULT_TRAITS = (3, 3, 3, 3, 3, 3, 3)

_TAG_ROLES = {
    "Marksman": {"bottom": 0.9, "middle": 0.1},
    "Mage": {"middle": 0.7, "utility": 0.3},
    "Assassin": {"middle": 0.6, "jungle": 0.4},
    "Fighter": {"top": 0.7, "jungle": 0.3},
    "Tank": {"top": 0.5, "jungle": 0.25, "utility": 0.25},
    "Support": {"utility": 1.0},
}
_UNIFORM_ROLES = {p: 0.2 for p in POSITIONS}


@dataclass(frozen=True)
class Traits:
    early: float
    spike6: float
    waveclear: float
    mobility: float
    clear: float
    gank: float
    duel: float
    estimated: bool = False  # 手作業の表に無く、タグから作った値 / 情報が少なく推測した値


class TraitTable:
    def __init__(self, traits_json: dict, roles_json: dict, catalog: ChampionCatalog):
        self.catalog = catalog
        self._traits = traits_json["traits"]
        self._traits_estimated = set(traits_json.get("_estimated", []))
        fields_ = roles_json.get("_fields", list(POSITIONS))
        self._roles: dict[str, dict[str, float]] = {}
        for cid, values in roles_json["rates"].items():
            total = sum(values) or 1
            self._roles[cid] = {pos: v / total for pos, v in zip(fields_, values)}

    @classmethod
    def load(cls, catalog: ChampionCatalog, data_dir: Path = DATA_DIR) -> "TraitTable":
        traits = json.loads((data_dir / "champion_traits.json").read_text(encoding="utf-8"))
        roles = json.loads((data_dir / "role_rates.json").read_text(encoding="utf-8"))
        return cls(traits, roles, catalog)

    def traits(self, cid: str) -> Traits:
        values = self._traits.get(cid)
        if values is not None:
            return Traits(*values, estimated=cid in self._traits_estimated)
        tags = self.catalog.tags(cid)
        values = _TAG_TRAITS.get(tags[0], _DEFAULT_TRAITS) if tags else _DEFAULT_TRAITS
        return Traits(*values, estimated=True)

    def role_rates(self, cid: str) -> dict[str, float]:
        rates = self._roles.get(cid)
        if rates is not None:
            return rates
        tags = self.catalog.tags(cid)
        if tags and tags[0] in _TAG_ROLES:
            return dict(_TAG_ROLES[tags[0]])
        return dict(_UNIFORM_ROLES)
