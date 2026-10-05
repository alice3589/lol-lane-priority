"""状態（GameState）→ ポジション推定 → スコア計算 をまとめて行う。UI からはここだけを呼ぶ。"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Mapping

from .champions import ChampionCatalog
from .config import Settings
from .matchup_db import ROLE_RATE_MIN_GAMES, MatchupDB
from .models import LANES, GameState, Placement
from .role_inference import infer_positions
from .scorer import LaneResult, Scorer, advice
from .traits import TraitTable

LANE_POSITIONS = {
    "top": ("top",),
    "jungle": ("jungle",),
    "middle": ("middle",),
    "bottom": ("bottom", "utility"),
}

# これ未満の確信度なら UI で「?」を付ける
UNCERTAIN_BELOW = 0.6


@dataclass
class Analysis:
    ally: list[Placement] = field(default_factory=list)
    enemy: list[Placement] = field(default_factory=list)
    lanes: list[LaneResult] = field(default_factory=list)
    advice: list[str] = field(default_factory=list)

    def placement_at(self, side: str, position: str) -> Placement | None:
        for p in self.ally if side == "ally" else self.enemy:
            if p.position == position:
                return p
        return None


class Analyzer:
    def __init__(
        self,
        catalog: ChampionCatalog,
        traits: TraitTable,
        settings: Settings,
        db: MatchupDB | None = None,
    ):
        self.catalog = catalog
        self.traits = traits
        self.db = db
        self.settings = settings

    def set_catalog(self, catalog: ChampionCatalog) -> None:
        self.catalog = catalog
        self.traits.catalog = catalog

    def rates_for(self, cid: str) -> Mapping[str, float]:
        if self.db is not None:
            rates, games = self.db.role_rates(cid, self._patches())
            if games >= ROLE_RATE_MIN_GAMES:
                return rates
        return self.traits.role_rates(cid)

    def _patches(self) -> list[str] | None:
        return self.db.recent_patches(self.settings.recent_patches) if self.db else None

    def scorer(self) -> Scorer:
        return Scorer(
            self.catalog,
            self.traits,
            self.db,
            tier=self.settings.tier,
            min_games=self.settings.min_games,
            patches=self._patches(),
        )

    def analyze(self, state: GameState, overrides: Mapping[str, Mapping[str, str]] | None = None) -> Analysis:
        overrides = overrides or {}
        ally = infer_positions(state.ally, self.rates_for, overrides.get("ally"))
        enemy = infer_positions(state.enemy, self.rates_for, overrides.get("enemy"))
        ally_by = {p.position: p.player.champion for p in ally}
        enemy_by = {p.position: p.player.champion for p in enemy}

        scorer = self.scorer()
        lanes = []
        for lane in LANES:
            positions = LANE_POSITIONS[lane]
            lanes.append(
                scorer.score_lane(
                    lane,
                    [ally_by.get(pos) for pos in positions],
                    [enemy_by.get(pos) for pos in positions],
                )
            )
        return Analysis(ally=ally, enemy=enemy, lanes=lanes, advice=advice(lanes))
