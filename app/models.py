"""アプリ全体で使うデータ型と定数。"""
from __future__ import annotations

from dataclasses import dataclass, field

# ポジション（LCU / Match-V5 の表記に合わせる）
POSITIONS = ("top", "jungle", "middle", "bottom", "utility")

# 画面に出すレーン（BOT は ADC+SUP をまとめて 2v2 で判定する）
LANES = ("top", "jungle", "middle", "bottom")

POSITION_LABELS = {
    "top": "TOP",
    "jungle": "JG",
    "middle": "MID",
    "bottom": "ADC",
    "utility": "SUP",
}

LANE_LABELS = {
    "top": "TOP",
    "jungle": "JG",
    "middle": "MID",
    "bottom": "BOT",
}

# いろいろな表記ゆれ → POSITIONS
POSITION_ALIASES = {
    "top": "top",
    "jungle": "jungle",
    "jg": "jungle",
    "middle": "middle",
    "mid": "middle",
    "bottom": "bottom",
    "bot": "bottom",
    "adc": "bottom",
    "utility": "utility",
    "support": "utility",
    "sup": "utility",
}

# ランク帯（低い順）。"ALL" は全ランク合算。
TIERS = (
    "IRON", "BRONZE", "SILVER", "GOLD", "PLATINUM",
    "EMERALD", "DIAMOND", "MASTER", "GRANDMASTER", "CHALLENGER",
)

TIER_LABELS = {
    "ALL": "全ランク",
    "IRON": "アイアン",
    "BRONZE": "ブロンズ",
    "SILVER": "シルバー",
    "GOLD": "ゴールド",
    "PLATINUM": "プラチナ",
    "EMERALD": "エメラルド",
    "DIAMOND": "ダイヤモンド",
    "MASTER": "マスター",
    "GRANDMASTER": "グランドマスター",
    "CHALLENGER": "チャレンジャー",
}


def normalize_position(value: str | None) -> str | None:
    """表記ゆれのあるポジション文字列を POSITIONS のどれかに直す。不明なら None。"""
    if not value:
        return None
    return POSITION_ALIASES.get(value.strip().lower())


@dataclass
class Player:
    """チャンピオンセレクト / 試合中の1人分の情報。"""

    champion: str | None = None  # Data Dragon の id（例: "Darius"）。未選択なら None
    position: str | None = None  # 確定しているポジション（味方の assignedPosition など）
    has_smite: bool = False
    hovering: bool = False  # まだロックしていない（ホバー中）
    is_local: bool = False  # 自分自身か


@dataclass
class GameState:
    """データ取得元から受け取る、ある時点の状態。"""

    phase: str = "disconnected"  # disconnected / idle / champ_select / loading / in_game / error
    status: str = ""
    side: str | None = None  # "blue" / "red"
    ally: list[Player] = field(default_factory=list)
    enemy: list[Player] = field(default_factory=list)


@dataclass
class Placement:
    """ポジション推定の結果（1人分）。"""

    player: Player
    position: str
    confidence: float  # 0〜1。確定済みなら 1
    fixed: bool  # 確定情報（assignedPosition / 手動指定）によるものか
