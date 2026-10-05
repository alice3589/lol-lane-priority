"""LoL クライアント（LCU API）への接続とチャンピオンセレクト情報の取得（F-01〜F-03）。

接続情報は次の順で探す。
  1. 設定で指定された lockfile
  2. よくあるインストール先の lockfile
  3. 起動中の LeagueClientUx.exe のコマンドライン（--app-port / --remoting-auth-token）
"""
from __future__ import annotations

import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

import requests
import urllib3

from .champions import ChampionCatalog
from .models import GameState, Player, normalize_position

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)

LOCKFILE_CANDIDATES = [
    r"C:\Riot Games\League of Legends\lockfile",
    r"D:\Riot Games\League of Legends\lockfile",
    r"E:\Riot Games\League of Legends\lockfile",
    r"C:\Program Files\Riot Games\League of Legends\lockfile",
    r"C:\Program Files (x86)\Riot Games\League of Legends\lockfile",
]

SMITE_SPELL_ID = 11


@dataclass(frozen=True)
class LcuCredentials:
    port: int
    password: str
    protocol: str = "https"

    @property
    def base_url(self) -> str:
        return f"{self.protocol}://127.0.0.1:{self.port}"


def parse_lockfile(text: str) -> LcuCredentials | None:
    """lockfile の中身（例: LeagueClient:1234:54321:abcdef:https）を読む。"""
    parts = text.strip().split(":")
    if len(parts) < 5:
        return None
    try:
        return LcuCredentials(port=int(parts[2]), password=parts[3], protocol=parts[4])
    except ValueError:
        return None


def parse_commandline(cmdline: str) -> LcuCredentials | None:
    port = re.search(r"--app-port=(\d+)", cmdline)
    token = re.search(r"--remoting-auth-token=([\w-]+)", cmdline)
    if not (port and token):
        return None
    return LcuCredentials(port=int(port.group(1)), password=token.group(1))


def _process_commandline() -> str | None:
    if sys.platform != "win32":
        return None
    cmd = [
        "powershell", "-NoProfile", "-NonInteractive", "-Command",
        "(Get-CimInstance Win32_Process -Filter \"Name='LeagueClientUx.exe'\").CommandLine",
    ]
    try:
        out = subprocess.run(
            cmd, capture_output=True, text=True, timeout=8,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return out.stdout.strip() or None


def find_credentials(lockfile_path: str = "") -> LcuCredentials | None:
    paths = ([lockfile_path] if lockfile_path else []) + LOCKFILE_CANDIDATES
    for p in paths:
        try:
            creds = parse_lockfile(Path(p).read_text(encoding="utf-8"))
        except OSError:
            continue
        if creds:
            return creds
    cmdline = _process_commandline()
    return parse_commandline(cmdline) if cmdline else None


class LcuClient:
    # 未接続のとき、クライアントを探し直す間隔（プロセス検索は重いので毎回はやらない）
    SEARCH_INTERVAL = 5.0

    def __init__(self, lockfile_path: str = ""):
        self.lockfile_path = lockfile_path
        self.creds: LcuCredentials | None = None
        self.session = requests.Session()
        self.session.verify = False
        self._last_search = float("-inf")

    def ensure_connected(self) -> bool:
        if self.creds is None:
            now = time.monotonic()
            if now - self._last_search < self.SEARCH_INTERVAL:
                return False
            self._last_search = now
            self.creds = find_credentials(self.lockfile_path)
            if self.creds:
                self.session.auth = ("riot", self.creds.password)
        return self.creds is not None

    def get(self, path: str, timeout: float = 2.0):
        """GET して JSON を返す。404 などは None。接続できなければ接続情報を捨てて None。"""
        if not self.ensure_connected():
            return None
        try:
            r = self.session.get(self.creds.base_url + path, timeout=timeout)
        except requests.RequestException:
            # クライアントが終了した / lockfile が古い → 次回探し直す
            self.creds = None
            return None
        if r.status_code != 200:
            return None
        try:
            return r.json()
        except ValueError:
            return None

    def gameflow_phase(self) -> str | None:
        return self.get("/lol-gameflow/v1/gameflow-phase")

    def champ_select_session(self) -> dict | None:
        return self.get("/lol-champ-select/v1/session")


def _pick_actions(session: dict) -> dict[int, dict]:
    """cellId → そのセルの pick アクション（最後のもの）。"""
    picks: dict[int, dict] = {}
    for group in session.get("actions") or []:
        for action in group:
            if action.get("type") == "pick":
                picks[action.get("actorCellId")] = action
    return picks


def _member_to_player(member: dict, picks: dict[int, dict], catalog: ChampionCatalog, local_cell) -> Player:
    cell = member.get("cellId")
    champ_id = member.get("championId") or 0
    hovering = False
    if champ_id:
        action = picks.get(cell)
        hovering = action is not None and not action.get("completed", False)
    else:
        champ_id = member.get("championPickIntent") or 0
        hovering = bool(champ_id)
    spells = (member.get("spell1Id"), member.get("spell2Id"))
    return Player(
        champion=catalog.id_by_key(champ_id),
        position=normalize_position(member.get("assignedPosition")),
        has_smite=SMITE_SPELL_ID in spells,
        hovering=hovering,
        is_local=cell is not None and cell == local_cell,
    )


def parse_session(session: dict, catalog: ChampionCatalog) -> GameState:
    """/lol-champ-select/v1/session の JSON を GameState に変換する。"""
    picks = _pick_actions(session)
    local_cell = session.get("localPlayerCellId")
    my_team = session.get("myTeam") or []
    their_team = session.get("theirTeam") or []
    ally = [_member_to_player(m, picks, catalog, local_cell) for m in my_team]
    enemy = [_member_to_player(m, picks, catalog, None) for m in their_team]
    # 敵の assignedPosition は通常空。入っていても信用しすぎないよう推定に任せる
    for p in enemy:
        p.position = None
    team = my_team[0].get("team") if my_team else None
    side = {1: "blue", 2: "red"}.get(team)
    return GameState(phase="champ_select", status="チャンピオンセレクト中", side=side, ally=ally, enemy=enemy)
