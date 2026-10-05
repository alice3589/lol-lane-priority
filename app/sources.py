"""状態の取得元。本物のクライアント用（ClientSource）と動作確認用（MockSource）。"""
from __future__ import annotations

import copy

import requests

from .champions import ChampionCatalog
from .lcu_client import LcuClient, parse_session
from .live_client import fetch_live, parse_live
from .models import GameState, Player


class ClientSource:
    def __init__(self, catalog: ChampionCatalog, lockfile_path: str = ""):
        self.catalog = catalog
        self.lcu = LcuClient(lockfile_path)
        self.live = requests.Session()
        self._last_select: GameState | None = None

    def poll(self) -> GameState:
        # 1. 試合中なら Live Client Data API が一番確実
        data = fetch_live(self.live)
        if data:
            state = parse_live(data, self.catalog)
            if state:
                return state

        # 2. クライアント（LCU）
        if not self.lcu.ensure_connected():
            return GameState(phase="disconnected", status="LoL クライアントが見つかりません（起動を待っています）")
        phase = self.lcu.gameflow_phase()
        if phase is None:
            return GameState(phase="disconnected", status="LoL クライアントに接続できません（再接続を待っています）")

        if phase == "ChampSelect":
            session = self.lcu.champ_select_session()
            if session:
                state = parse_session(session, self.catalog)
                self._last_select = state
                return state
            return GameState(phase="idle", status="チャンピオンセレクトの情報を取得中…")

        if phase in ("GameStart", "InProgress", "Reconnect") and self._last_select is not None:
            # ロード画面中：チャンセレの最終状態を出し続ける
            state = copy.deepcopy(self._last_select)
            state.phase = "loading"
            state.status = "ロード中（チャンピオンセレクトの結果で判定）"
            return state

        if phase not in ("GameStart", "InProgress", "Reconnect"):
            self._last_select = None
        return GameState(phase="idle", status=f"待機中（{phase}）")


# モック: 青サイドのランク戦を想定したチャンセレの進行
_MOCK_ALLY = [
    ("Darius", "top", False),
    ("LeeSin", "jungle", True),
    ("Ahri", "middle", False),
    ("Jinx", "bottom", False),
    ("Lulu", "utility", False),
]
_MOCK_ENEMY = ["Kayle", "Nunu", "Orianna", "Draven", "Leona"]
# (味方 or 敵, インデックス) の順でピックが進む（ランクのドラフト順 B1 R1 R2 B2 B3 R3 R4 B4 B5 R5）
_MOCK_ORDER = [("a", 0), ("e", 3), ("e", 4), ("a", 1), ("a", 2), ("e", 1), ("e", 0), ("a", 3), ("a", 4), ("e", 2)]


class MockSource:
    """クライアントが無くても画面を確認できるよう、ピックが進んでいく様子を再現する。"""

    TICKS_PER_PICK = 3
    HOLD_TICKS = 40

    def __init__(self, catalog: ChampionCatalog):
        self.catalog = catalog
        self.tick = 0

    def poll(self) -> GameState:
        total = len(_MOCK_ORDER) * self.TICKS_PER_PICK + self.HOLD_TICKS
        t = self.tick % total
        self.tick += 1
        picked = min(len(_MOCK_ORDER), t // self.TICKS_PER_PICK)
        hovering_next = picked < len(_MOCK_ORDER)

        ally = [Player(position=pos, has_smite=smite, is_local=(i == 2)) for i, (_, pos, smite) in enumerate(_MOCK_ALLY)]
        enemy = [Player() for _ in _MOCK_ENEMY]
        for k, (side, i) in enumerate(_MOCK_ORDER[: picked + (1 if hovering_next else 0)]):
            hovering = k == picked
            if side == "a":
                ally[i].champion = _MOCK_ALLY[i][0]
                ally[i].hovering = hovering
            else:
                enemy[i].champion = _MOCK_ENEMY[i]
                enemy[i].hovering = hovering

        status = "モック: チャンピオンセレクト中" if hovering_next else "モック: ピック完了"
        return GameState(phase="champ_select", status=status, side="blue", ally=ally, enemy=enemy)
