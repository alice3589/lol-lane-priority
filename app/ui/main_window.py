"""メインウィンドウ（仕様書 5章）。"""
from __future__ import annotations

from PySide6.QtCore import QObject, QThread, QTimer, Signal, Slot
from PySide6.QtWidgets import (
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QMainWindow,
    QPushButton,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from ..analyzer import Analysis, Analyzer
from ..champions import ChampionCatalog
from ..config import Settings
from ..matchup_db import MatchupDB
from ..models import LANES, TIER_LABELS, TIERS, GameState
from .widgets import LaneRow

STYLE = """
QWidget { background: #14171d; color: #e6e8eb; font-family: "Yu Gothic UI", "Meiryo UI", sans-serif; font-size: 10.5pt; }
QFrame#laneRow { background: #1b1f27; border: 1px solid #262b35; border-radius: 8px; }
QFrame#laneRow:hover { border-color: #3a4150; }
QLabel#laneLabel { font-weight: bold; font-size: 13pt; color: #c9ccd1; background: transparent; }
QLabel#detail { background: #161a21; border-radius: 6px; padding: 8px; }
QLabel#chip { border-radius: 6px; padding: 6px 10px; font-weight: bold; }
QLabel#chip[chipState="ally"] { background: #1d3157; color: #cfe0ff; border: 1px solid #3a5fa8; }
QLabel#chip[chipState="enemy"] { background: #4a1f25; color: #ffd6d8; border: 1px solid #a83a44; }
QLabel#chip[chipState="ally-uncertain"] { background: #1d3157; color: #cfe0ff; border: 1px dashed #e0b84c; }
QLabel#chip[chipState="enemy-uncertain"] { background: #4a1f25; color: #ffd6d8; border: 1px dashed #e0b84c; }
QLabel#chip[chipState^="hover"] { background: #262b35; color: #c9ccd1; border: 1px dashed #5f6672; }
QLabel#chip[chipState="empty"] { background: #191c22; color: #5f6672; border: 1px dashed #2f3540; }
QLabel#status { color: #9aa0a6; }
QLabel#side { font-weight: bold; }
QLabel#advice { background: #1b1f27; border-radius: 8px; padding: 10px; color: #e0b84c; }
QLabel#header { color: #9aa0a6; background: transparent; }
QComboBox, QPushButton, QLineEdit, QSpinBox { background: #232833; border: 1px solid #333a47; border-radius: 5px; padding: 4px 8px; }
QPushButton:hover { background: #2c3341; }
QToolTip { background: #232833; color: #e6e8eb; border: 1px solid #333a47; }
QMenu { background: #232833; border: 1px solid #333a47; }
QMenu::item:selected { background: #2c3341; }
"""


class PollWorker(QObject):
    """別スレッドで取得元を定期的に問い合わせる（クライアント探索で UI を止めないため）。"""

    state_ready = Signal(object)

    def __init__(self, source, interval_ms: int):
        super().__init__()
        self.source = source
        self.interval_ms = interval_ms
        self.timer: QTimer | None = None

    @Slot()
    def start(self):
        self.timer = QTimer(self)
        self.timer.timeout.connect(self.tick)
        self.timer.start(self.interval_ms)
        self.tick()

    @Slot()
    def tick(self):
        try:
            state = self.source.poll()
        except Exception as e:  # 取得元の不具合でアプリごと落ちないようにする
            state = GameState(phase="error", status=f"取得エラー: {e}")
        self.state_ready.emit(state)


class SettingsDialog(QDialog):
    def __init__(self, settings: Settings, parent=None):
        super().__init__(parent)
        self.setWindowTitle("設定")
        self.setMinimumWidth(520)
        form = QFormLayout(self)

        self.lockfile = QLineEdit(settings.lockfile_path)
        self.lockfile.setPlaceholderText("空欄なら自動検出")
        browse = QPushButton("参照…")
        browse.clicked.connect(self._browse_lockfile)
        row = QHBoxLayout()
        row.addWidget(self.lockfile)
        row.addWidget(browse)
        form.addRow("lockfile の場所", row)

        self.db_path = QLineEdit(settings.db_path)
        form.addRow("実績DB（SQLite）", self.db_path)

        self.min_games = QSpinBox()
        self.min_games.setRange(1, 100000)
        self.min_games.setValue(settings.min_games)
        self.min_games.setToolTip("この試合数に届かないマッチアップは、実績データの重みを下げて特性表に寄せます")
        form.addRow("実績データの必要試合数", self.min_games)

        self.patches = QSpinBox()
        self.patches.setRange(0, 50)
        self.patches.setValue(settings.recent_patches)
        self.patches.setSpecialValueText("全パッチ")
        form.addRow("使う直近パッチ数", self.patches)

        buttons = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        form.addRow(buttons)

    def _browse_lockfile(self):
        path, _ = QFileDialog.getOpenFileName(self, "lockfile を選択", "C:/Riot Games/League of Legends")
        if path:
            self.lockfile.setText(path)

    def apply_to(self, settings: Settings) -> None:
        settings.lockfile_path = self.lockfile.text().strip()
        settings.db_path = self.db_path.text().strip()
        settings.min_games = self.min_games.value()
        settings.recent_patches = self.patches.value()


class MainWindow(QMainWindow):
    catalog_updated = Signal(object)

    def __init__(self, analyzer: Analyzer, source, settings: Settings, save_settings: bool = True,
                 start_polling: bool = True):
        super().__init__()
        self.analyzer = analyzer
        self.source = source
        self.settings = settings
        self.save_settings = save_settings
        self.overrides: dict[str, dict[str, str]] = {"ally": {}, "enemy": {}}
        self.state = GameState(phase="disconnected", status="起動中…")
        self.analysis = Analysis()
        self.poll_thread: QThread | None = None
        self.worker: PollWorker | None = None

        self.setWindowTitle("レーン主導権")
        self.resize(860, 560)
        self.setStyleSheet(STYLE)
        self._build_ui()
        self.catalog_updated.connect(self.set_catalog)
        self.refresh()
        if start_polling:
            self.start_polling()

    # ---- 画面の組み立て ----

    def _build_ui(self):
        root = QWidget()
        layout = QVBoxLayout(root)
        layout.setContentsMargins(14, 12, 14, 12)
        layout.setSpacing(8)

        top = QHBoxLayout()
        self.side_label = QLabel()
        self.side_label.setObjectName("side")
        top.addWidget(self.side_label)
        self.status_label = QLabel()
        self.status_label.setObjectName("status")
        top.addWidget(self.status_label, 1)

        top.addWidget(QLabel("ランク帯"))
        self.tier_combo = QComboBox()
        for tier in ("ALL",) + TIERS:
            self.tier_combo.addItem(TIER_LABELS[tier], tier)
        idx = self.tier_combo.findData(self.settings.tier)
        self.tier_combo.setCurrentIndex(max(0, idx))
        self.tier_combo.currentIndexChanged.connect(self.on_tier_changed)
        top.addWidget(self.tier_combo)

        reset = QPushButton("推定リセット")
        reset.setToolTip("ドラッグで入れ替えたポジションを元に戻します")
        reset.clicked.connect(self.reset_overrides)
        top.addWidget(reset)
        settings_btn = QPushButton("設定")
        settings_btn.clicked.connect(self.open_settings)
        top.addWidget(settings_btn)
        layout.addLayout(top)

        header = QHBoxLayout()
        header.setContentsMargins(12, 0, 12, 0)
        for text, stretch in (("", 0), ("味方", 2), ("判定（左に伸びるほど味方有利）", 3), ("敵", 2)):
            lbl = QLabel(text)
            lbl.setObjectName("header")
            if not text:
                lbl.setFixedWidth(48)
            header.addWidget(lbl, stretch)
        layout.addLayout(header)

        self.rows = {lane: LaneRow(self, lane) for lane in LANES}
        for lane in LANES:
            layout.addWidget(self.rows[lane])

        self.advice_label = QLabel()
        self.advice_label.setObjectName("advice")
        self.advice_label.setWordWrap(True)
        layout.addWidget(self.advice_label)

        hint = QLabel("行をクリックで根拠を表示 ／ 敵の札をドラッグ（右クリック）でレーンを入れ替え ／ 「?」は推定の確信度が低いもの")
        hint.setObjectName("status")
        hint.setWordWrap(True)
        layout.addWidget(hint)
        layout.addStretch(1)
        self.setCentralWidget(root)

    # ---- ポーリング ----

    def start_polling(self):
        self.poll_thread = QThread(self)
        self.worker = PollWorker(self.source, self.settings.poll_interval_ms)
        self.worker.moveToThread(self.poll_thread)
        self.poll_thread.started.connect(self.worker.start)
        self.worker.state_ready.connect(self.on_state)
        self.poll_thread.start()

    def stop_polling(self):
        if self.poll_thread is not None:
            self.poll_thread.quit()
            self.poll_thread.wait(3000)
            self.poll_thread = None

    def closeEvent(self, event):  # noqa: N802
        self.stop_polling()
        super().closeEvent(event)

    # ---- 状態の反映 ----

    @Slot(object)
    def on_state(self, state: GameState):
        # 新しいチャンピオンセレクトが始まったら手動の入れ替えは捨てる
        if state.phase == "champ_select" and self.state.phase not in ("champ_select",):
            self.overrides = {"ally": {}, "enemy": {}}
        self.state = state
        self.refresh()

    def refresh(self):
        self.analysis = self.analyzer.analyze(self.state, self.overrides)
        catalog = self.analyzer.catalog
        for result in self.analysis.lanes:
            self.rows[result.lane].update_row(result, self.analysis, catalog, self.settings.tier)

        side = {"blue": ("青サイド", "#4c8dff"), "red": ("赤サイド", "#ff5a5f")}.get(self.state.side or "")
        if side:
            self.side_label.setText(f"味方: {side[0]}")
            self.side_label.setStyleSheet(f"color: {side[1]};")
        else:
            self.side_label.setText("")
        db_note = "" if self.analyzer.db is not None else "　※実績DBなし（特性表のみで判定）"
        self.status_label.setText(self.state.status + db_note)

        if self.analysis.advice:
            self.advice_label.setText("　".join(f"▶ {a}" for a in self.analysis.advice))
            self.advice_label.setVisible(True)
        else:
            self.advice_label.setVisible(False)

    @Slot(object)
    def set_catalog(self, catalog: ChampionCatalog):
        self.analyzer.set_catalog(catalog)
        if hasattr(self.source, "catalog"):
            self.source.catalog = catalog
        self.refresh()

    # ---- 操作 ----

    def swap_positions(self, side: str, pos_from: str, pos_to: str):
        """side チームの pos_from と pos_to にいるチャンピオンを入れ替える（手動指定として記録）。"""
        if pos_from == pos_to:
            return
        a = self.analysis.placement_at(side, pos_from)
        b = self.analysis.placement_at(side, pos_to)
        ca = a.player.champion if a else None
        cb = b.player.champion if b else None
        ov = self.overrides[side]
        for champ in [c for c, p in ov.items() if p in (pos_from, pos_to) or c in (ca, cb)]:
            del ov[champ]
        if ca:
            ov[ca] = pos_to
        if cb:
            ov[cb] = pos_from
        self.refresh()

    def reset_overrides(self):
        self.overrides = {"ally": {}, "enemy": {}}
        self.refresh()

    def on_tier_changed(self, _index: int):
        self.settings.tier = self.tier_combo.currentData()
        self._save()
        self.refresh()

    def open_settings(self):
        dialog = SettingsDialog(self.settings, self)
        if dialog.exec() != QDialog.Accepted:
            return
        old_db = self.settings.db_path
        dialog.apply_to(self.settings)
        if self.settings.db_path != old_db:
            if self.analyzer.db is not None:
                self.analyzer.db.close()
            self.analyzer.db = MatchupDB.open_if_exists(self.settings.db_path)
        elif self.analyzer.db is not None:
            self.analyzer.db.clear_cache()
        lcu = getattr(self.source, "lcu", None)
        if lcu is not None:
            lcu.lockfile_path = self.settings.lockfile_path
            lcu.creds = None
            lcu._last_search = float("-inf")
        self._save()
        self.refresh()

    def _save(self):
        if self.save_settings:
            self.settings.save()
