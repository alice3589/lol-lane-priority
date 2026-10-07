"""Python 版の計算結果を golden.json に書き出す。Rust 版との一致確認用。

使い方（リポジトリ直下で）: python rust/tests/gen_golden.py
"""
import json
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from app.analyzer import Analyzer  # noqa: E402
from app.champions import BUNDLED_PATH, ChampionCatalog  # noqa: E402
from app.config import Settings  # noqa: E402
from app.matchup_db import LaneRow, MatchupDB  # noqa: E402
from app.models import GameState, Player  # noqa: E402
from app.scorer import Scorer  # noqa: E402
from app.traits import TraitTable  # noqa: E402

catalog = ChampionCatalog(json.loads(BUNDLED_PATH.read_text(encoding="utf-8")))
traits = TraitTable.load(catalog)

LANE_CASES = [
    ("top", ["Darius"], ["Kayle"]),
    ("top", ["Renekton"], ["Vladimir"]),
    ("jungle", ["LeeSin"], ["Karthus"]),
    ("jungle", ["XinZhao"], ["Ivern"]),
    ("middle", ["Ahri"], ["Syndra"]),
    ("bottom", ["Caitlyn", "Lux"], ["Smolder", None]),
    ("bottom", ["Draven", "Leona"], ["Jinx", "Lulu"]),
    ("bottom", ["Caitlyn", None], ["Smolder", None]),
    ("top", ["Darius"], [None]),
    ("top", ["Locke"], ["Garen"]),
]


def lane_out(r):
    return {
        "score": r.score,
        "verdict": r.verdict,
        "weight_a": r.weight_a,
        "weight_b": r.weight_b,
        "games": r.games,
        "notes": r.notes,
        "reasons": [[x.source, x.label, x.detail, x.points] for x in r.reasons],
    }


out = {"lanes": [], "db_lanes": [], "analysis": []}

scorer = Scorer(catalog, traits)
for lane, a, e in LANE_CASES:
    out["lanes"].append({"lane": lane, "ally": a, "enemy": e, "expect": lane_out(scorer.score_lane(lane, a, e))})

# DB あり（試合数が足りるケースと足りないケース）
with tempfile.TemporaryDirectory() as d:
    db = MatchupDB(Path(d) / "m.sqlite")
    for i in range(300):
        db.add_match(f"S{i}", "16.19", "GOLD", [LaneRow("middle", "Ahri", "Syndra", 600, 300, 10, 1, 0)], [])
    for i in range(50):
        db.add_match(f"G{i}", "16.19", "GOLD", [LaneRow("top", "Garen", "Darius", 400, 0, 0, 0, 2)], [])
    for i in range(120):
        db.add_match(
            f"B{i}",
            "16.20",
            "SILVER",
            [LaneRow("bottom", "Jinx", "Caitlyn", -200, -50, -3, 0, 1)],
            [("Jinx", "bottom")],
        )
    sc = Scorer(catalog, traits, db, tier="GOLD", min_games=200)
    cases = [
        ("middle", ["Ahri"], ["Syndra"]),
        ("top", ["Garen"], ["Darius"]),
        ("bottom", ["Jinx", "Lulu"], ["Caitlyn", "Leona"]),
    ]
    for lane, a, e in cases:
        out["db_lanes"].append({"lane": lane, "ally": a, "enemy": e, "expect": lane_out(sc.score_lane(lane, a, e))})
    db.close()

# 全体解析（ポジション推定込み）
analyzer = Analyzer(catalog, traits, Settings())
STATES = [
    (
        [("Darius", None, False), ("LeeSin", None, True), ("Ahri", None, False), ("Jinx", None, False), ("Lulu", None, False)],
        [("Kayle", None, False), ("Karthus", None, True), ("Syndra", None, False), ("Caitlyn", None, False), ("Leona", None, False)],
    ),
    (
        [("Ahri", "middle", False), ("Garen", "top", False), ("Jinx", None, False)],
        [("Zed", None, False), ("Thresh", None, False), ("Ashe", None, False), (None, None, False)],
    ),
]
for ally, enemy in STATES:
    mk = lambda lst: [Player(champion=c, position=p, has_smite=s) for c, p, s in lst]
    an = analyzer.analyze(GameState(ally=mk(ally), enemy=mk(enemy)))
    out["analysis"].append(
        {
            "ally": ally,
            "enemy": enemy,
            "expect": {
                "ally": [[p.player.champion, p.position, p.confidence, p.fixed] for p in an.ally],
                "enemy": [[p.player.champion, p.position, p.confidence, p.fixed] for p in an.enemy],
                "lanes": [lane_out(r) | {"lane": r.lane} for r in an.lanes],
                "advice": an.advice,
            },
        }
    )

(ROOT / "rust" / "tests" / "golden.json").write_text(json.dumps(out, ensure_ascii=False, indent=1), encoding="utf-8")
print("wrote golden.json")
