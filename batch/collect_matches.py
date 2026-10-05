"""Match-V5 からマッチアップ実績を収集・集計して SQLite に保存する（仕様書 4.3）。

使い方（PowerShell）:
    $env:RIOT_API_KEY = "RGAPI-..."
    python -m batch.collect_matches --platform jp1 --tiers GOLD,PLATINUM --players 30 --matches 20

・ランク帯ごとに League-V4 からプレイヤーを集め、各プレイヤーのランク戦の試合を取得する
・timeline の 10分時点の gold / xp / CS から、レーンごとの差を両方向で記録する
・取り込み済みの試合はスキップするので、途中で止めても続きから再開できる
・開発用キーのレート制限（20回/秒・100回/2分）を守るため、時間はかかる
"""
from __future__ import annotations

import argparse
import os
import sys
import time
from collections import deque
from dataclasses import dataclass
from typing import Callable

import requests

from app.champions import ChampionCatalog
from app.config import DEFAULT_DB_PATH
from app.matchup_db import LaneRow, MatchupDB
from app.models import POSITIONS, TIERS, normalize_position

RANKED_SOLO_QUEUE = 420
QUEUE_NAME = "RANKED_SOLO_5x5"
TEN_MINUTES_MS = 600_000

PLATFORM_TO_REGION = {
    "jp1": "asia", "kr": "asia",
    "na1": "americas", "br1": "americas", "la1": "americas", "la2": "americas",
    "euw1": "europe", "eun1": "europe", "tr1": "europe", "ru": "europe", "me1": "europe",
    "oc1": "sea", "ph2": "sea", "sg2": "sea", "th2": "sea", "tw2": "sea", "vn2": "sea",
}

APEX_ENDPOINTS = {
    "MASTER": "masterleagues",
    "GRANDMASTER": "grandmasterleagues",
    "CHALLENGER": "challengerleagues",
}
DIVISIONS = ("I", "II", "III", "IV")


class RateLimiter:
    """複数の「n回 / t秒」制限をまとめて守る。"""

    def __init__(
        self,
        limits=((20, 1.0), (100, 120.0)),
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
    ):
        self.limits = limits
        self.clock = clock
        self.sleep = sleep
        self.calls: deque[float] = deque()

    def wait(self) -> None:
        longest = max(window for _, window in self.limits)
        while True:
            now = self.clock()
            while self.calls and now - self.calls[0] >= longest:
                self.calls.popleft()
            delay = 0.0
            for count, window in self.limits:
                recent = [t for t in self.calls if now - t < window]
                if len(recent) >= count:
                    delay = max(delay, window - (now - recent[-count]))
            if delay <= 0:
                self.calls.append(now)
                return
            self.sleep(delay + 0.01)


class RiotApi:
    def __init__(self, api_key: str, platform: str, limiter: RateLimiter | None = None,
                 session: requests.Session | None = None):
        if platform not in PLATFORM_TO_REGION:
            raise ValueError(f"未対応のプラットフォーム: {platform}")
        self.platform = platform
        self.region = PLATFORM_TO_REGION[platform]
        self.limiter = limiter or RateLimiter()
        self.session = session or requests.Session()
        self.session.headers["X-Riot-Token"] = api_key

    def _get(self, url: str, params: dict | None = None, retries: int = 5):
        for attempt in range(retries):
            self.limiter.wait()
            try:
                r = self.session.get(url, params=params, timeout=15)
            except requests.RequestException:
                time.sleep(2 ** attempt)
                continue
            if r.status_code == 200:
                return r.json()
            if r.status_code == 404:
                return None
            if r.status_code == 429:
                time.sleep(float(r.headers.get("Retry-After", 10)))
                continue
            if r.status_code >= 500:
                time.sleep(2 ** attempt)
                continue
            r.raise_for_status()
        raise RuntimeError(f"リトライ上限: {url}")

    def _platform_url(self, path: str) -> str:
        return f"https://{self.platform}.api.riotgames.com{path}"

    def _region_url(self, path: str) -> str:
        return f"https://{self.region}.api.riotgames.com{path}"

    def _puuid_of_entry(self, entry: dict) -> str | None:
        if entry.get("puuid"):
            return entry["puuid"]
        if entry.get("summonerId"):  # 古い形式のレスポンス向け
            s = self._get(self._platform_url(f"/lol/summoner/v4/summoners/{entry['summonerId']}"))
            return s.get("puuid") if s else None
        return None

    def tier_puuids(self, tier: str, limit: int) -> list[str]:
        """ランク帯 tier のプレイヤーを最大 limit 人。"""
        puuids: list[str] = []
        if tier in APEX_ENDPOINTS:
            data = self._get(self._platform_url(f"/lol/league/v4/{APEX_ENDPOINTS[tier]}/by-queue/{QUEUE_NAME}"))
            for entry in (data or {}).get("entries", []):
                if len(puuids) >= limit:
                    break
                p = self._puuid_of_entry(entry)
                if p:
                    puuids.append(p)
            return puuids
        # 各ディビジョンから均等に集める
        per_division = max(1, -(-limit // len(DIVISIONS)))
        for division in DIVISIONS:
            got = 0
            page = 1
            while got < per_division and len(puuids) < limit:
                entries = self._get(
                    self._platform_url(f"/lol/league/v4/entries/{QUEUE_NAME}/{tier}/{division}"),
                    params={"page": page},
                )
                if not entries:
                    break
                for entry in entries:
                    if got >= per_division or len(puuids) >= limit:
                        break
                    p = self._puuid_of_entry(entry)
                    if p:
                        puuids.append(p)
                        got += 1
                page += 1
        return puuids

    def match_ids(self, puuid: str, count: int) -> list[str]:
        return self._get(
            self._region_url(f"/lol/match/v5/matches/by-puuid/{puuid}/ids"),
            params={"queue": RANKED_SOLO_QUEUE, "type": "ranked", "count": count},
        ) or []

    def match(self, match_id: str) -> dict | None:
        return self._get(self._region_url(f"/lol/match/v5/matches/{match_id}"))

    def timeline(self, match_id: str) -> dict | None:
        return self._get(self._region_url(f"/lol/match/v5/matches/{match_id}/timeline"))


@dataclass
class MatchAggregate:
    patch: str
    rows: list[LaneRow]
    roles: list[tuple[str, str]]


def patch_of(game_version: str) -> str:
    """"16.19.712.3456" → "16.19"。"""
    return ".".join(game_version.split(".")[:2])


def _champion_id(p: dict, catalog: ChampionCatalog | None) -> str | None:
    if catalog is not None:
        cid = catalog.id_by_key(p.get("championId")) or catalog.resolve(p.get("championName"))
        if cid:
            return cid
    return p.get("championName") or None


def aggregate_match(match: dict, timeline: dict, catalog: ChampionCatalog | None = None) -> MatchAggregate | None:
    """1試合分の集計行を作る。10分に届かない試合・ポジションが崩れている試合は None。"""
    info = match.get("info") or {}
    frames = (timeline.get("info") or {}).get("frames") or []
    if len(frames) <= 10:
        return None
    pf10 = frames[10].get("participantFrames") or {}

    # (チーム, ポジション) → 参加者
    by_slot: dict[tuple[int, str], dict] = {}
    roles: list[tuple[str, str]] = []
    for p in info.get("participants") or []:
        pos = normalize_position(p.get("teamPosition"))
        champ = _champion_id(p, catalog)
        if pos is None or champ is None:
            continue
        roles.append((champ, pos))
        by_slot[(p.get("teamId"), pos)] = {**p, "_champ": champ}

    # 10分までのソロキル（アシスト無しで、キラーがプレイヤーのもの）
    solo: dict[tuple[int, int], int] = {}
    for frame in frames[:11]:
        for ev in frame.get("events") or []:
            if ev.get("type") != "CHAMPION_KILL" or ev.get("timestamp", 0) >= TEN_MINUTES_MS:
                continue
            if ev.get("assistingParticipantIds"):
                continue
            killer, victim = ev.get("killerId", 0), ev.get("victimId", 0)
            if killer and victim:
                solo[(killer, victim)] = solo.get((killer, victim), 0) + 1

    def snapshot(p: dict):
        f = pf10.get(str(p.get("participantId")))
        if f is None:
            return None
        cs = f.get("minionsKilled", 0) + f.get("jungleMinionsKilled", 0)
        return f.get("totalGold", 0), f.get("xp", 0), cs

    rows: list[LaneRow] = []
    for pos in POSITIONS:
        blue, red = by_slot.get((100, pos)), by_slot.get((200, pos))
        if not (blue and red):
            continue
        sb, sr = snapshot(blue), snapshot(red)
        if sb is None or sr is None:
            continue
        bid, rid = blue.get("participantId"), red.get("participantId")
        b_kills, r_kills = solo.get((bid, rid), 0), solo.get((rid, bid), 0)
        gd, xpd, csd = sb[0] - sr[0], sb[1] - sr[1], sb[2] - sr[2]
        rows.append(LaneRow(pos, blue["_champ"], red["_champ"], gd, xpd, csd, b_kills, r_kills))
        rows.append(LaneRow(pos, red["_champ"], blue["_champ"], -gd, -xpd, -csd, r_kills, b_kills))

    if not rows:
        return None
    return MatchAggregate(patch=patch_of(info.get("gameVersion", "")), rows=rows, roles=roles)


def collect(api: RiotApi, db: MatchupDB, tiers: list[str], players: int, matches: int,
            catalog: ChampionCatalog | None = None, log=print) -> int:
    added = 0
    for tier in tiers:
        log(f"[{tier}] プレイヤー取得中…")
        puuids = api.tier_puuids(tier, players)
        log(f"[{tier}] {len(puuids)} 人")
        for i, puuid in enumerate(puuids, 1):
            for match_id in api.match_ids(puuid, matches):
                if db.has_match(match_id):
                    continue
                match = api.match(match_id)
                if not match or (match.get("info") or {}).get("queueId") != RANKED_SOLO_QUEUE:
                    continue
                timeline = api.timeline(match_id)
                if not timeline:
                    continue
                agg = aggregate_match(match, timeline, catalog)
                if agg is None:
                    continue
                if db.add_match(match_id, agg.patch, tier, agg.rows, agg.roles):
                    added += 1
            log(f"[{tier}] {i}/{len(puuids)} 人処理 / 追加 {added} 試合")
    return added


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Match-V5 からマッチアップ実績を集計する")
    parser.add_argument("--platform", default="jp1", help="jp1 / kr / na1 / euw1 など（既定: jp1）")
    parser.add_argument("--tiers", default=",".join(TIERS), help="カンマ区切りのランク帯（既定: 全ランク帯）")
    parser.add_argument("--players", type=int, default=20, help="ランク帯ごとのプレイヤー数")
    parser.add_argument("--matches", type=int, default=20, help="プレイヤーごとの試合数（最大100）")
    parser.add_argument("--db", default=str(DEFAULT_DB_PATH), help="保存先 SQLite")
    parser.add_argument("--api-key", default=os.environ.get("RIOT_API_KEY", ""), help="省略時は環境変数 RIOT_API_KEY")
    args = parser.parse_args(argv)

    if not args.api_key:
        print("Riot API キーがありません。--api-key か環境変数 RIOT_API_KEY を設定してください。", file=sys.stderr)
        return 2
    tiers = [t.strip().upper() for t in args.tiers.split(",") if t.strip()]
    unknown = [t for t in tiers if t not in TIERS]
    if unknown:
        print(f"不明なランク帯: {unknown}（使えるもの: {', '.join(TIERS)}）", file=sys.stderr)
        return 2

    api = RiotApi(args.api_key, args.platform)
    db = MatchupDB(args.db)
    try:
        added = collect(api, db, tiers, args.players, min(100, args.matches), ChampionCatalog.load())
    except KeyboardInterrupt:
        print("\n中断しました（取り込み済みの分は保存されています）")
        return 130
    finally:
        db.close()
    print(f"完了: {added} 試合を追加しました → {args.db}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
