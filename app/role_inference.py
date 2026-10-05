"""ポジション推定（仕様書 3.3）。

5人 × 5ポジションの割り当て 120 通りをすべて調べ、ロール出現率の積が最大のものを採用する。
各人の確信度は「全割り当てで重み付けしたときに、そのポジションになる確率（周辺確率）」。
"""
from __future__ import annotations

import itertools
from typing import Callable, Mapping

from .models import POSITIONS, Placement, Player

# 出現率 0 のロールでも完全には否定しない（オフロール対策）
RATE_FLOOR = 0.01
# スマイト所持者が JG 以外にいる場合の係数
SMITE_PENALTY = 0.01

RatesFn = Callable[[str], Mapping[str, float]]


def _fixed_positions(players: list[Player], overrides: Mapping[str, str]) -> list[str | None]:
    """確定しているポジションを決める。手動指定 > assignedPosition の順で、重複は後勝ちさせない。"""
    fixed: list[str | None] = [None] * len(players)
    taken: set[str] = set()
    # 1. 手動指定（ドラッグでの入れ替え）
    for i, p in enumerate(players):
        pos = overrides.get(p.champion) if p.champion else None
        if pos in POSITIONS and pos not in taken:
            fixed[i] = pos
            taken.add(pos)
    # 2. クライアントが教えてくれたポジション
    for i, p in enumerate(players):
        if fixed[i] is None and p.position in POSITIONS and p.position not in taken:
            fixed[i] = p.position
            taken.add(p.position)
    return fixed


def infer_positions(
    players: list[Player],
    rates_for: RatesFn,
    overrides: Mapping[str, str] | None = None,
) -> list[Placement]:
    """players（最大5人）にポジションを割り当てる。返り値は players と同じ順番。"""
    players = list(players[:5])
    n = len(players)
    if n == 0:
        return []
    slots = players + [Player() for _ in range(5 - n)]
    fixed = _fixed_positions(slots, overrides or {})

    # 各人・各ポジションの重みを先に計算しておく
    weight_table: list[dict[str, float]] = []
    for i, p in enumerate(slots):
        row = {}
        for pos in POSITIONS:
            if fixed[i] is not None:
                w = 1.0 if pos == fixed[i] else 0.0
            else:
                w = 1.0
                if p.champion:
                    w = max(rates_for(p.champion).get(pos, 0.0), RATE_FLOOR)
                if p.has_smite and pos != "jungle":
                    w *= SMITE_PENALTY
            row[pos] = w
        weight_table.append(row)

    total = 0.0
    best_weight = -1.0
    best: tuple[str, ...] = POSITIONS
    marginals = [dict.fromkeys(POSITIONS, 0.0) for _ in slots]
    for perm in itertools.permutations(POSITIONS):
        w = 1.0
        for i, pos in enumerate(perm):
            w *= weight_table[i][pos]
            if w == 0.0:
                break
        if w == 0.0:
            continue
        total += w
        for i, pos in enumerate(perm):
            marginals[i][pos] += w
        if w > best_weight:
            best_weight = w
            best = perm

    result = []
    for i in range(n):
        pos = best[i]
        conf = marginals[i][pos] / total if total > 0 else 0.0
        result.append(Placement(player=players[i], position=pos, confidence=conf, fixed=fixed[i] is not None))
    return result
