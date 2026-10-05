"""収集バッチの集計ロジック（ネットワークは使わない）。"""
import pytest

from batch.collect_matches import RateLimiter, aggregate_match, patch_of

PARTICIPANTS = [
    (1, 100, "TOP", 122, "Darius"), (2, 100, "JUNGLE", 64, "LeeSin"), (3, 100, "MIDDLE", 103, "Ahri"),
    (4, 100, "BOTTOM", 222, "Jinx"), (5, 100, "UTILITY", 117, "Lulu"),
    (6, 200, "TOP", 10, "Kayle"), (7, 200, "JUNGLE", 20, "Nunu"), (8, 200, "MIDDLE", 61, "Orianna"),
    (9, 200, "BOTTOM", 119, "Draven"), (10, 200, "UTILITY", 89, "Leona"),
]


def _match(version="16.19.712.3456"):
    return {
        "info": {
            "gameVersion": version,
            "queueId": 420,
            "participants": [
                {"participantId": pid, "teamId": team, "teamPosition": pos, "championId": cid, "championName": name}
                for pid, team, pos, cid, name in PARTICIPANTS
            ],
        }
    }


def _timeline(frames=11, events=None):
    pf = {}
    for pid, team, *_ in PARTICIPANTS:
        base = 4000 if team == 100 else 3500
        pf[str(pid)] = {"totalGold": base + pid, "xp": 5000 if team == 100 else 4800,
                        "minionsKilled": 80, "jungleMinionsKilled": 0}
    out = [{"participantFrames": {}, "events": []} for _ in range(frames)]
    if frames > 10:
        out[10]["participantFrames"] = pf
    out[3]["events"] = events or []
    return {"info": {"frames": out}}


def test_patch_of():
    assert patch_of("16.19.712.3456") == "16.19"


def test_aggregate_basic(catalog):
    events = [
        {"type": "CHAMPION_KILL", "timestamp": 200000, "killerId": 1, "victimId": 6, "assistingParticipantIds": []},
        {"type": "CHAMPION_KILL", "timestamp": 250000, "killerId": 2, "victimId": 6, "assistingParticipantIds": [1]},
        {"type": "CHAMPION_KILL", "timestamp": 700000, "killerId": 1, "victimId": 6},  # 10分以降
    ]
    agg = aggregate_match(_match(), _timeline(events=events), catalog)
    assert agg.patch == "16.19"
    assert len(agg.rows) == 10  # 5レーン × 両方向
    top = [r for r in agg.rows if r.lane == "top" and r.champ == "Darius"][0]
    assert top.opp == "Kayle"
    assert top.gd10 == (4000 + 1) - (3500 + 6)
    assert top.xpd10 == 200
    assert top.solo_kills == 1 and top.solo_deaths == 0
    rev = [r for r in agg.rows if r.lane == "top" and r.champ == "Kayle"][0]
    assert rev.gd10 == -top.gd10 and rev.solo_deaths == 1
    assert ("Lulu", "utility") in agg.roles


def test_aggregate_skips_short_games(catalog):
    assert aggregate_match(_match(), _timeline(frames=8), catalog) is None


def test_aggregate_skips_missing_positions(catalog):
    m = _match()
    for p in m["info"]["participants"]:
        p["teamPosition"] = ""
    assert aggregate_match(m, _timeline(), catalog) is None


def test_rate_limiter_waits():
    now = [0.0]
    slept = []

    def sleep(s):
        slept.append(s)
        now[0] += s

    rl = RateLimiter(limits=((2, 1.0), (3, 10.0)), clock=lambda: now[0], sleep=sleep)
    rl.wait()
    rl.wait()
    assert slept == []
    rl.wait()  # 1秒2回の制限で待つ
    assert now[0] == pytest.approx(1.01)
    rl.wait()  # 10秒3回の制限で待つ
    assert now[0] == pytest.approx(10.01)
