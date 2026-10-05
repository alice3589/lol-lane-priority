"""マッチアップ実績DB（SQLite）。

batch/collect_matches.py が Match-V5 から集計して書き込み、アプリは読むだけ。
試合数・合計値で持っているので、ランク帯やパッチをまたいだ合算が正確にできる。
"""
from __future__ import annotations

import re
import sqlite3
from dataclasses import dataclass, field
from pathlib import Path

from .models import POSITIONS, TIERS

SCHEMA = """
CREATE TABLE IF NOT EXISTS matchups (
    patch TEXT NOT NULL,
    tier TEXT NOT NULL,
    lane TEXT NOT NULL,
    champ TEXT NOT NULL,
    opp TEXT NOT NULL,
    games INTEGER NOT NULL DEFAULT 0,
    sum_gd10 REAL NOT NULL DEFAULT 0,
    sum_xpd10 REAL NOT NULL DEFAULT 0,
    sum_csd10 REAL NOT NULL DEFAULT 0,
    solo_kills INTEGER NOT NULL DEFAULT 0,
    solo_deaths INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (patch, tier, lane, champ, opp)
);
CREATE TABLE IF NOT EXISTS role_counts (
    patch TEXT NOT NULL,
    tier TEXT NOT NULL,
    champ TEXT NOT NULL,
    position TEXT NOT NULL,
    games INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (patch, tier, champ, position)
);
CREATE TABLE IF NOT EXISTS processed_matches (
    match_id TEXT PRIMARY KEY,
    patch TEXT,
    tier TEXT
);
"""

# DB のロール出現率を信用するのに必要な試合数
ROLE_RATE_MIN_GAMES = 50


@dataclass
class MatchupStats:
    games: int
    gd10: float  # 10分ゴールド差の平均（champ 視点）
    xpd10: float
    csd10: float
    solo_kill_rate: float  # 1試合あたり、10分までに対面をソロキルした回数
    solo_death_rate: float
    tiers: list[str] = field(default_factory=list)


@dataclass
class LaneRow:
    """1試合・1レーン・片方向分の集計行（batch から渡される）。"""

    lane: str
    champ: str
    opp: str
    gd10: float
    xpd10: float
    csd10: float
    solo_kills: int
    solo_deaths: int


def _patch_key(patch: str) -> tuple[int, ...]:
    return tuple(int(x) for x in re.findall(r"\d+", patch))


def tier_search_order(tier: str) -> list[list[str]]:
    """ランク帯の探索順。サンプルが足りなければ隣のランク帯を1つずつ足していく。

    例: GOLD → [[GOLD], [GOLD, SILVER], [GOLD, SILVER, PLATINUM], ...]
    """
    if tier not in TIERS:
        return [list(TIERS)]
    idx = TIERS.index(tier)
    order = [tier]
    for d in range(1, len(TIERS)):
        for j in (idx - d, idx + d):
            if 0 <= j < len(TIERS):
                order.append(TIERS[j])
    return [order[: k + 1] for k in range(len(order))]


class MatchupDB:
    def __init__(self, path: str | Path):
        self.path = Path(path)
        self.conn = sqlite3.connect(str(self.path))
        self.conn.executescript(SCHEMA)
        self._cache: dict[tuple, MatchupStats | None] = {}
        self._role_cache: dict[tuple, tuple[dict[str, float], int]] = {}

    @classmethod
    def open_if_exists(cls, path: str | Path) -> "MatchupDB | None":
        return cls(path) if Path(path).exists() else None

    def close(self) -> None:
        self.conn.close()

    def clear_cache(self) -> None:
        self._cache.clear()
        self._role_cache.clear()

    # ---- 書き込み（batch 用） ----

    def has_match(self, match_id: str) -> bool:
        cur = self.conn.execute("SELECT 1 FROM processed_matches WHERE match_id = ?", (match_id,))
        return cur.fetchone() is not None

    def add_match(
        self,
        match_id: str,
        patch: str,
        tier: str,
        rows: list[LaneRow],
        roles: list[tuple[str, str]],
    ) -> bool:
        """1試合分を加算する。すでに取り込み済みなら何もせず False。"""
        with self.conn:
            cur = self.conn.execute(
                "INSERT OR IGNORE INTO processed_matches (match_id, patch, tier) VALUES (?, ?, ?)",
                (match_id, patch, tier),
            )
            if cur.rowcount == 0:
                return False
            for r in rows:
                self.conn.execute(
                    """
                    INSERT INTO matchups (patch, tier, lane, champ, opp, games, sum_gd10, sum_xpd10,
                                          sum_csd10, solo_kills, solo_deaths)
                    VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?)
                    ON CONFLICT (patch, tier, lane, champ, opp) DO UPDATE SET
                        games = games + 1,
                        sum_gd10 = sum_gd10 + excluded.sum_gd10,
                        sum_xpd10 = sum_xpd10 + excluded.sum_xpd10,
                        sum_csd10 = sum_csd10 + excluded.sum_csd10,
                        solo_kills = solo_kills + excluded.solo_kills,
                        solo_deaths = solo_deaths + excluded.solo_deaths
                    """,
                    (patch, tier, r.lane, r.champ, r.opp, r.gd10, r.xpd10, r.csd10, r.solo_kills, r.solo_deaths),
                )
            for champ, position in roles:
                self.conn.execute(
                    """
                    INSERT INTO role_counts (patch, tier, champ, position, games) VALUES (?, ?, ?, ?, 1)
                    ON CONFLICT (patch, tier, champ, position) DO UPDATE SET games = games + 1
                    """,
                    (patch, tier, champ, position),
                )
        self.clear_cache()
        return True

    # ---- 読み込み（アプリ用） ----

    def patches(self) -> list[str]:
        cur = self.conn.execute("SELECT DISTINCT patch FROM matchups")
        return sorted((row[0] for row in cur), key=_patch_key)

    def recent_patches(self, n: int) -> list[str] | None:
        """直近 n パッチ。n <= 0 なら None（＝全パッチ）。"""
        if n <= 0:
            return None
        return self.patches()[-n:]

    def _sum(self, lane: str, champ: str, opp: str, tiers: list[str] | None, patches: list[str] | None):
        sql = (
            "SELECT COALESCE(SUM(games),0), COALESCE(SUM(sum_gd10),0), COALESCE(SUM(sum_xpd10),0),"
            " COALESCE(SUM(sum_csd10),0), COALESCE(SUM(solo_kills),0), COALESCE(SUM(solo_deaths),0)"
            " FROM matchups WHERE lane = ? AND champ = ? AND opp = ?"
        )
        params: list = [lane, champ, opp]
        if tiers is not None:
            sql += f" AND tier IN ({','.join('?' * len(tiers))})"
            params += tiers
        if patches is not None:
            sql += f" AND patch IN ({','.join('?' * len(patches))})"
            params += patches
        return self.conn.execute(sql, params).fetchone()

    def lookup(
        self,
        lane: str,
        champ: str,
        opp: str,
        tier: str = "ALL",
        min_games: int = 200,
        patches: list[str] | None = None,
    ) -> MatchupStats | None:
        """champ 視点のマッチアップ実績。データが1試合もなければ None。"""
        key = (lane, champ, opp, tier, min_games, tuple(patches) if patches is not None else None)
        if key in self._cache:
            return self._cache[key]

        result = None
        candidates = [None] if tier == "ALL" else tier_search_order(tier)
        for tiers in candidates:
            games, gd, xpd, csd, sk, sd = self._sum(lane, champ, opp, tiers, patches)
            if games > 0:
                result = MatchupStats(
                    games=int(games),
                    gd10=gd / games,
                    xpd10=xpd / games,
                    csd10=csd / games,
                    solo_kill_rate=sk / games,
                    solo_death_rate=sd / games,
                    tiers=list(tiers) if tiers is not None else ["ALL"],
                )
            if result is not None and result.games >= min_games:
                break
        self._cache[key] = result
        return result

    def role_rates(self, champ: str, patches: list[str] | None = None) -> tuple[dict[str, float], int]:
        """DB に貯まったロール出現率（全ランク合算）と、その試合数。"""
        key = (champ, tuple(patches) if patches is not None else None)
        if key in self._role_cache:
            return self._role_cache[key]
        sql = "SELECT position, SUM(games) FROM role_counts WHERE champ = ?"
        params: list = [champ]
        if patches is not None:
            sql += f" AND patch IN ({','.join('?' * len(patches))})"
            params += patches
        sql += " GROUP BY position"
        counts = {pos: n for pos, n in self.conn.execute(sql, params) if pos in POSITIONS}
        total = sum(counts.values())
        rates = {pos: counts.get(pos, 0) / total for pos in POSITIONS} if total else {}
        self._role_cache[key] = (rates, total)
        return rates, total
