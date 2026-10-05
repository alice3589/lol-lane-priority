"""画面の部品: スコアバー・チャンピオン札・レーン行。"""
from __future__ import annotations

from html import escape
from typing import TYPE_CHECKING

from PySide6.QtCore import QMimeData, QRectF, Qt
from PySide6.QtGui import QColor, QDrag, QFont, QPainter
from PySide6.QtWidgets import QApplication, QFrame, QGridLayout, QLabel, QMenu, QVBoxLayout, QWidget

from ..analyzer import LANE_POSITIONS, UNCERTAIN_BELOW, Analysis
from ..champions import ChampionCatalog
from ..models import LANE_LABELS, POSITION_LABELS, POSITIONS, TIER_LABELS, Placement
from ..scorer import LaneResult

if TYPE_CHECKING:
    from .main_window import MainWindow

ALLY_COLOR = "#4c8dff"
ENEMY_COLOR = "#ff5a5f"
EVEN_COLOR = "#9aa0a6"
TRACK_COLOR = "#2a2f3a"
MIME_TYPE = "application/x-lane-priority-slot"

VERDICT_TEXT = {"ally": "味方有利", "even": "互角", "enemy": "敵有利", "unknown": "判定待ち"}
VERDICT_COLOR = {"ally": ALLY_COLOR, "even": EVEN_COLOR, "enemy": ENEMY_COLOR, "unknown": "#5f6672"}


def arrows(score: float | None) -> str:
    if score is None or abs(score) < 10:
        return "―"
    n = 1 if abs(score) < 30 else 2 if abs(score) < 60 else 3
    return "◀" * n if score > 0 else "▶" * n


class ScoreBar(QWidget):
    """中央から左（味方有利・青）/ 右（敵有利・赤）に伸びるバー。"""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.score: float | None = None
        self.verdict = "unknown"
        self.setMinimumSize(240, 46)

    def set_score(self, score: float | None, verdict: str) -> None:
        self.score, self.verdict = score, verdict
        self.update()

    def text(self) -> str:
        if self.score is None:
            return VERDICT_TEXT["unknown"]
        return f"{arrows(self.score)}  {self.score:+.0f}  {VERDICT_TEXT[self.verdict]}"

    def paintEvent(self, event):  # noqa: N802 (Qt の命名)
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        w = self.width()
        margin, bar_h, y = 8.0, 10.0, 6.0
        half = w / 2 - margin
        cx = w / 2

        p.setPen(Qt.NoPen)
        p.setBrush(QColor(TRACK_COLOR))
        p.drawRoundedRect(QRectF(margin, y, w - 2 * margin, bar_h), 5, 5)

        if self.score is not None:
            length = half * min(1.0, abs(self.score) / 100.0)
            color = QColor(VERDICT_COLOR[self.verdict])
            p.setBrush(color)
            if self.score > 0:
                p.drawRoundedRect(QRectF(cx - length, y, length, bar_h), 5, 5)
            elif self.score < 0:
                p.drawRoundedRect(QRectF(cx, y, length, bar_h), 5, 5)

        p.setBrush(QColor("#d0d4da"))
        p.drawRect(QRectF(cx - 1, y - 3, 2, bar_h + 6))

        font = QFont(self.font())
        font.setBold(True)
        font.setPointSizeF(font.pointSizeF() + 1)
        p.setFont(font)
        p.setPen(QColor(VERDICT_COLOR[self.verdict]))
        p.drawText(QRectF(0, y + bar_h + 2, w, self.height() - y - bar_h - 2), Qt.AlignCenter, self.text())
        p.end()


class ChampChip(QLabel):
    """チャンピオン1人分の札。同じチーム内でドラッグ＆ドロップするとポジションを入れ替える。"""

    def __init__(self, window: "MainWindow", side: str, position: str, show_position: bool):
        super().__init__()
        self.main = window
        self.side = side
        self.position = position
        self.show_position = show_position
        self.champion: str | None = None
        self._press_pos = None
        self.setAcceptDrops(True)
        self.setAlignment(Qt.AlignCenter)
        self.setMinimumWidth(150)
        self.setObjectName("chip")

    def set_placement(self, placement: Placement | None, catalog: ChampionCatalog) -> None:
        prefix = f"{POSITION_LABELS[self.position]}  " if self.show_position else ""
        if placement is None or placement.player.champion is None:
            self.champion = None
            self.setText(prefix + "未選択")
            self.setToolTip("")
            state = "empty"
        else:
            player = placement.player
            self.champion = player.champion
            uncertain = not placement.fixed and placement.confidence < UNCERTAIN_BELOW
            text = prefix + catalog.name(player.champion)
            if uncertain:
                text += " ?"
            if player.hovering:
                text += "（選択中）"
            self.setText(text)
            if placement.fixed:
                origin = "確定（クライアント情報 / 手動指定）"
            else:
                origin = f"推定（確信度 {placement.confidence:.0%}）"
            self.setToolTip(f"{origin}\nドラッグで入れ替え / 右クリックでレーン変更")
            state = "hover" if player.hovering else self.side
            if uncertain:
                state += "-uncertain"
        self.setProperty("chipState", state)
        self.style().unpolish(self)
        self.style().polish(self)

    # ---- ドラッグ＆ドロップ ----

    def mousePressEvent(self, event):  # noqa: N802
        if event.button() == Qt.LeftButton:
            self._press_pos = event.position().toPoint()
        event.accept()

    def mouseMoveEvent(self, event):  # noqa: N802
        if not (event.buttons() & Qt.LeftButton) or self._press_pos is None or not self.champion:
            return
        if (event.position().toPoint() - self._press_pos).manhattanLength() < QApplication.startDragDistance():
            return
        mime = QMimeData()
        mime.setData(MIME_TYPE, f"{self.side}|{self.position}".encode())
        drag = QDrag(self)
        drag.setMimeData(mime)
        drag.setPixmap(self.grab())
        self._press_pos = None
        drag.exec(Qt.MoveAction)

    def _decode(self, mime) -> tuple[str, str] | None:
        if not mime.hasFormat(MIME_TYPE):
            return None
        side, pos = bytes(mime.data(MIME_TYPE)).decode().split("|")
        return side, pos

    def dragEnterEvent(self, event):  # noqa: N802
        src = self._decode(event.mimeData())
        if src and src[0] == self.side and src[1] != self.position:
            event.acceptProposedAction()
        else:
            event.ignore()

    def dropEvent(self, event):  # noqa: N802
        src = self._decode(event.mimeData())
        if src and src[0] == self.side:
            event.acceptProposedAction()
            self.main.swap_positions(self.side, src[1], self.position)

    def contextMenuEvent(self, event):  # noqa: N802
        menu = QMenu(self)
        if self.champion:
            for pos in POSITIONS:
                act = menu.addAction(f"{POSITION_LABELS[pos]} にする")
                act.setEnabled(pos != self.position)
                act.triggered.connect(lambda _=False, p=pos: self.main.swap_positions(self.side, self.position, p))
            menu.addSeparator()
        menu.addAction("推定をリセット").triggered.connect(self.main.reset_overrides)
        menu.exec(event.globalPos())


class LaneRow(QFrame):
    """1レーン分の行。クリックで判定の根拠を開閉する。"""

    def __init__(self, window: "MainWindow", lane: str):
        super().__init__()
        self.main = window
        self.lane = lane
        self.setObjectName("laneRow")
        self.setCursor(Qt.PointingHandCursor)

        grid = QGridLayout(self)
        grid.setContentsMargins(12, 8, 12, 8)
        grid.setHorizontalSpacing(12)

        label = QLabel(LANE_LABELS[lane])
        label.setObjectName("laneLabel")
        label.setFixedWidth(48)
        grid.addWidget(label, 0, 0, Qt.AlignVCenter)

        positions = LANE_POSITIONS[lane]
        multi = len(positions) > 1
        self.ally_chips = [ChampChip(window, "ally", pos, multi) for pos in positions]
        self.enemy_chips = [ChampChip(window, "enemy", pos, multi) for pos in positions]
        grid.addLayout(self._stack(self.ally_chips), 0, 1)
        self.bar = ScoreBar()
        grid.addWidget(self.bar, 0, 2)
        grid.addLayout(self._stack(self.enemy_chips), 0, 3)
        grid.setColumnStretch(1, 2)
        grid.setColumnStretch(2, 3)
        grid.setColumnStretch(3, 2)

        self.detail = QLabel()
        self.detail.setObjectName("detail")
        self.detail.setWordWrap(True)
        self.detail.setTextFormat(Qt.RichText)
        self.detail.setVisible(False)
        grid.addWidget(self.detail, 1, 0, 1, 4)

    @staticmethod
    def _stack(chips):
        box = QVBoxLayout()
        box.setSpacing(4)
        for c in chips:
            box.addWidget(c)
        return box

    def mousePressEvent(self, event):  # noqa: N802
        if event.button() == Qt.LeftButton:
            self.detail.setVisible(not self.detail.isVisible())

    def update_row(self, result: LaneResult, analysis: Analysis, catalog: ChampionCatalog, tier: str) -> None:
        for chip in self.ally_chips:
            chip.set_placement(analysis.placement_at("ally", chip.position), catalog)
        for chip in self.enemy_chips:
            chip.set_placement(analysis.placement_at("enemy", chip.position), catalog)
        self.bar.set_score(result.score, result.verdict)
        self.detail.setText(detail_html(result, tier))


def detail_html(result: LaneResult, tier: str) -> str:
    if result.score is None:
        notes = "<br>".join(escape(n) for n in result.notes) or "チャンピオンが揃うと判定します"
        return f"<span style='color:#9aa0a6'>{notes}</span>"
    head = (
        f"<b>判定の内訳</b>　<span style='color:#9aa0a6'>実績 A {result.weight_a:.0%} / "
        f"特性 B {result.weight_b:.0%}（スコア {result.score:+.1f}）</span>"
    )
    rows = []
    for r in result.reasons:
        color = ALLY_COLOR if r.points > 0.05 else ENEMY_COLOR if r.points < -0.05 else EVEN_COLOR
        tag = "実績" if r.source == "A" else "特性"
        rows.append(
            f"<tr><td style='color:#9aa0a6'>[{tag}]</td><td style='padding-right:14px'>{escape(r.label)}</td>"
            f"<td style='padding-right:14px'>{escape(r.detail)}</td>"
            f"<td align='right' style='color:{color}'>{r.points:+.1f}</td></tr>"
        )
    table = "<table cellspacing='0' cellpadding='2'>" + "".join(rows) + "</table>"
    extra = []
    if result.games:
        tiers = "+".join(TIER_LABELS.get(t, t) for t in result.tiers)
        extra.append(f"実績データ: {tiers}（約{result.games}試合）／ 選択ランク帯: {TIER_LABELS.get(tier, tier)}")
    extra += result.notes
    notes = "".join(f"<br>・{escape(n)}" for n in extra)
    return f"{head}{table}<span style='color:#9aa0a6'>{notes}</span>"
