"""レーン主導権スコアの計算（仕様書 4.1）。

スコアは味方視点で -100〜+100（正なら味方有利）。
  A. 実績データ  : 10分ゴールド差（主指標）・XP差・ソロキル     初期重み 0.6
  B. チャンプ特性 : 射程・序盤の強さ・ウェーブクリア・Lv6・機動力 初期重み 0.3
  C. 補正        : 熟練度など（未実装。v1 以降）                初期重み 0.1
A は試合数が min_games に届かない分だけ重みを B に回す。C が無いので最後に A+B で正規化する。
"""
from __future__ import annotations

import math
from dataclasses import dataclass, field

from .champions import ChampionCatalog
from .matchup_db import MatchupDB, MatchupStats
from .traits import TRAIT_LABELS, TraitTable

EVEN_THRESHOLD = 10.0
WEIGHT_A = 0.6
WEIGHT_B = 0.3

GD_SCALE = 500.0  # 10分ゴールド差 ±500 で tanh(1) ≒ 0.76
RANGE_SCALE = 400.0  # 射程差 400 で最大
SOLO_KILL_SCALE = 0.3  # 1試合あたりのソロキル差 0.3 で最大

# A の内訳
A_PARTS = (("gd10", 0.7), ("xpd10", 0.2), ("solo", 0.1))

# B の内訳（TOP / MID）
LANE_TERMS = (("range", 0.25), ("early", 0.35), ("waveclear", 0.20), ("spike6", 0.12), ("mobility", 0.08))
# BOT は ADC と SUP の重み付き合算（項目ごとに ADC / SUP の比率が違う）
BOT_PAIR_WEIGHTS = {
    "range": (0.85, 0.15),  # 近接のエンゲージサポートが射程だけで不利にならないよう SUP は軽め
    "early": (0.5, 0.5),
    "waveclear": (0.8, 0.2),
    "spike6": (0.5, 0.5),
    "mobility": (0.6, 0.4),
}
# JG は対面比較ではなく、ジャングラーとしての序盤の動きやすさで比べる
JG_TERMS = (("clear", 0.40), ("gank", 0.35), ("duel", 0.25))

# BOT の実績データ: ADC 同士 0.6 / SUP 同士 0.4
BOT_A_PAIRS = (("bottom", 0.6), ("utility", 0.4))

PAIR_LABELS = ("ADC", "SUP")


def _clamp(x: float, lo: float = -1.0, hi: float = 1.0) -> float:
    return max(lo, min(hi, x))


@dataclass
class Reason:
    source: str  # "A"（実績）/ "B"（特性）
    label: str
    detail: str
    points: float  # 最終スコアへの寄与（味方視点）。全 Reason の合計 = スコア


@dataclass
class LaneResult:
    lane: str
    ally: list[str | None]  # BOT は [ADC, SUP]、それ以外は1人
    enemy: list[str | None]
    score: float | None = None
    verdict: str = "unknown"  # ally / even / enemy / unknown
    reasons: list[Reason] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    games: int = 0
    tiers: list[str] = field(default_factory=list)
    weight_a: float = 0.0  # 正規化後の重み
    weight_b: float = 0.0


def verdict_of(score: float | None) -> str:
    if score is None:
        return "unknown"
    if score >= EVEN_THRESHOLD:
        return "ally"
    if score <= -EVEN_THRESHOLD:
        return "enemy"
    return "even"


def stats_score_parts(stats: MatchupStats) -> dict[str, float]:
    """実績データを -1〜+1 の値に直す。"""
    return {
        "gd10": math.tanh(stats.gd10 / GD_SCALE),
        "xpd10": math.tanh(stats.xpd10 / GD_SCALE),
        "solo": _clamp((stats.solo_kill_rate - stats.solo_death_rate) / SOLO_KILL_SCALE),
    }


class Scorer:
    def __init__(
        self,
        catalog: ChampionCatalog,
        traits: TraitTable,
        db: MatchupDB | None = None,
        tier: str = "ALL",
        min_games: int = 200,
        patches: list[str] | None = None,
    ):
        self.catalog = catalog
        self.traits = traits
        self.db = db
        self.tier = tier
        self.min_games = max(1, min_games)
        self.patches = patches

    # ---- B: チャンピオン特性 ----

    def _value(self, term: str, cid: str) -> float:
        if term == "range":
            return float(self.catalog.attack_range(cid))
        return float(getattr(self.traits.traits(cid), term))

    def _diff(self, term: str, a: str, b: str) -> float:
        va, vb = self._value(term, a), self._value(term, b)
        if term == "range":
            return _clamp((va - vb) / RANGE_SCALE)
        return _clamp((va - vb) / 4.0)

    def _detail(self, term: str, pairs: list[tuple[int, str, str]], multi: bool) -> str:
        parts = []
        for idx, a, b in pairs:
            va, vb = self._value(term, a), self._value(term, b)
            if term == "range":
                text = f"{va:.0f} vs {vb:.0f}（{va - vb:+.0f}）"
            else:
                text = f"{va:.0f} vs {vb:.0f}"
            parts.append(f"{PAIR_LABELS[idx]} {text}" if multi else text)
        return " / ".join(parts)

    def trait_score(self, lane: str, ally: list[str | None], enemy: list[str | None]):
        """B の値（-100〜+100）と項目ごとの内訳 [(label, detail, points)] を返す。"""
        pairs = [(i, a, b) for i, (a, b) in enumerate(zip(ally, enemy)) if a and b]
        if not pairs:
            return None, []
        if lane == "jungle":
            terms = JG_TERMS
        else:
            terms = LANE_TERMS
        multi = lane == "bottom"
        total = 0.0
        parts = []
        for term, term_weight in terms:
            if multi:
                pw = BOT_PAIR_WEIGHTS[term]
                wsum = sum(pw[i] for i, _, _ in pairs)
                diff = sum(pw[i] * self._diff(term, a, b) for i, a, b in pairs) / wsum
            else:
                _, a, b = pairs[0]
                diff = self._diff(term, a, b)
            points = 100.0 * term_weight * diff
            total += points
            label = "射程" if term == "range" else TRAIT_LABELS[term]
            parts.append((label, self._detail(term, pairs, multi), points))
        return total, parts

    # ---- A: 実績データ ----

    def stats_score(self, lane: str, ally: list[str | None], enemy: list[str | None]):
        """A の値・平均試合数・使ったランク帯・内訳を返す。データが無ければ値は None。"""
        if self.db is None:
            return None, 0.0, [], []
        db_pairs = BOT_A_PAIRS if lane == "bottom" else ((lane, 1.0),)
        found = []
        known_weight = 0.0
        for idx, ((db_lane, w), a, b) in enumerate(zip(db_pairs, ally, enemy)):
            if not (a and b):
                continue
            known_weight += w
            stats = self.db.lookup(db_lane, a, b, self.tier, self.min_games, self.patches)
            if stats is not None:
                found.append((idx, w, stats))
        if not found:
            return None, 0.0, [], []
        wsum = sum(w for _, w, _ in found)
        value = 0.0
        parts = []
        tiers: list[str] = []
        games = 0.0
        multi = lane == "bottom"
        for idx, w, stats in found:
            share = w / wsum
            vals = stats_score_parts(stats)
            prefix = f"{PAIR_LABELS[idx]} " if multi else ""
            details = {
                "gd10": ("10分ゴールド差", f"平均 {stats.gd10:+.0f}（{stats.games}試合）"),
                "xpd10": ("10分XP差", f"平均 {stats.xpd10:+.0f}"),
                "solo": ("ソロキル(〜10分)", f"{stats.solo_kill_rate:.2f} vs {stats.solo_death_rate:.2f} 回/試合"),
            }
            for key, part_weight in A_PARTS:
                points = 100.0 * share * part_weight * vals[key]
                value += points
                label, detail = details[key]
                parts.append((prefix + label, detail, points))
            games += share * stats.games
            for t in stats.tiers:
                if t not in tiers:
                    tiers.append(t)
        # 片方のペアしかデータが無いときは、その分だけ信頼度を下げる
        coverage = wsum / known_weight if known_weight else 0.0
        return value, games * coverage, tiers, parts

    # ---- 合算 ----

    def score_lane(self, lane: str, ally: list[str | None], enemy: list[str | None]) -> LaneResult:
        result = LaneResult(lane=lane, ally=list(ally), enemy=list(enemy))
        b_value, b_parts = self.trait_score(lane, ally, enemy)
        if b_value is None:
            if not any(ally):
                result.notes.append("味方のチャンピオンが未確定")
            if not any(enemy):
                result.notes.append("敵のチャンピオンが未確定")
            return result

        a_value, games, tiers, a_parts = self.stats_score(lane, ally, enemy)
        reliability = min(1.0, games / self.min_games) if a_value is not None else 0.0
        w_a = WEIGHT_A * reliability
        w_b = WEIGHT_B + WEIGHT_A * (1.0 - reliability)
        norm = w_a + w_b
        result.weight_a, result.weight_b = w_a / norm, w_b / norm

        score = result.weight_b * b_value
        if a_value is not None:
            score += result.weight_a * a_value
            result.reasons += [Reason("A", l, d, p * result.weight_a) for l, d, p in a_parts]
        result.reasons += [Reason("B", l, d, p * result.weight_b) for l, d, p in b_parts]
        result.reasons.sort(key=lambda r: -abs(r.points))
        result.score = max(-100.0, min(100.0, score))
        result.verdict = verdict_of(result.score)
        result.games = int(round(games))
        result.tiers = tiers

        if a_value is None:
            result.notes.append("実績データなし（特性表のみで判定）")
        elif reliability < 1.0:
            result.notes.append(f"実績データが少ない（{result.games}試合）ため特性表の比重を上げています")
        for side, champs in (("味方", ally), ("敵", enemy)):
            for idx, cid in enumerate(champs):
                if cid is None and lane == "bottom":
                    result.notes.append(f"{side}{PAIR_LABELS[idx]}が未確定")
                elif cid and self.traits.traits(cid).estimated:
                    result.notes.append(f"{self.catalog.name(cid)} の特性値は推定")
        return result


def advice(results: list[LaneResult]) -> list[str]:
    """主導権のあるレーンから、JG の動き方の目安を作る（仕様書 F-10 の簡易版）。"""
    s = {r.lane: r.score for r in results}
    top, mid, bot, jg = s.get("top"), s.get("middle"), s.get("bottom"), s.get("jungle")
    lines = []
    if top is not None and bot is not None:
        if top >= EVEN_THRESHOLD and bot >= EVEN_THRESHOLD:
            lines.append("両サイドに主導権 → JG はどちら側でも動きやすい")
        elif top <= -EVEN_THRESHOLD and bot <= -EVEN_THRESHOLD:
            lines.append("両サイド不利 → 序盤はファームと視界を優先、カウンタージャングルに注意")
        elif top - bot >= 15 and top >= EVEN_THRESHOLD:
            lines.append("TOP 側に主導権 → JG は TOP 側（ガンク・スカトル・ヴォイドグラブ）で動きやすい")
        elif bot - top >= 15 and bot >= EVEN_THRESHOLD:
            lines.append("BOT 側に主導権 → JG は BOT 側（ガンク・スカトル・ドラゴン）で動きやすい")
        else:
            lines.append("サイドの主導権は拮抗")
    if mid is not None:
        if mid >= EVEN_THRESHOLD:
            lines.append("MID に主導権 → MID のロームや JG のカバーが通りやすい")
        elif mid <= -EVEN_THRESHOLD:
            lines.append("MID が不利 → 敵 MID のロームに注意")
    if jg is not None and jg <= -EVEN_THRESHOLD:
        lines.append("敵 JG の方が序盤に強い → インベード・早いガンクに注意")
    return lines
