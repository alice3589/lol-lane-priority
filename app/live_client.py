"""Live Client Data API（試合中・ポート 2999）から確定した10人を取る。"""
from __future__ import annotations

import requests
import urllib3

from .champions import ChampionCatalog
from .models import GameState, Player, normalize_position

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)

ALL_GAME_DATA_URL = "https://127.0.0.1:2999/liveclientdata/allgamedata"


def fetch_live(session: requests.Session, timeout: float = 1.0) -> dict | None:
    try:
        r = session.get(ALL_GAME_DATA_URL, timeout=timeout, verify=False)
    except requests.RequestException:
        return None
    if r.status_code != 200:
        return None
    try:
        return r.json()
    except ValueError:
        return None


def _has_smite(player: dict) -> bool:
    spells = player.get("summonerSpells") or {}
    for spell in spells.values():
        raw = (spell or {}).get("rawDisplayName", "") + (spell or {}).get("displayName", "")
        if "smite" in raw.lower() or "スマイト" in raw:
            return True
    return False


def _player_id(p: dict) -> str:
    return p.get("riotId") or p.get("summonerName") or ""


def parse_live(data: dict, catalog: ChampionCatalog) -> GameState | None:
    players = data.get("allPlayers") or []
    if not players:
        return None
    active = data.get("activePlayer") or {}
    me = active.get("riotId") or active.get("summonerName")
    my_team = next((p.get("team") for p in players if me and _player_id(p) == me), None)
    if my_team is None:
        my_team = "ORDER"  # 観戦などで自分が見つからないときは青側を味方として扱う

    ally, enemy = [], []
    for p in players:
        champion = catalog.resolve(p.get("rawChampionName")) or catalog.resolve(p.get("championName"))
        player = Player(
            champion=champion,
            position=normalize_position(p.get("position")),
            has_smite=_has_smite(p),
            is_local=bool(me) and _player_id(p) == me,
        )
        (ally if p.get("team") == my_team else enemy).append(player)

    side = {"ORDER": "blue", "CHAOS": "red"}.get(my_team)
    return GameState(phase="in_game", status="試合中（確定した10人で判定）", side=side, ally=ally, enemy=enemy)
