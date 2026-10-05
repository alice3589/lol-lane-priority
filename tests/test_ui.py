"""画面のスモークテスト（画面は出さない）。"""
import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402

from app.analyzer import Analyzer  # noqa: E402
from app.config import Settings  # noqa: E402
from app.sources import MockSource  # noqa: E402
from app.ui.main_window import MainWindow  # noqa: E402


# QApplication はウィンドウより先に破棄されると終了時に落ちるので、プロセス終了まで保持する
_APP = QApplication.instance() or QApplication([])


@pytest.fixture
def window(catalog, traits):
    analyzer = Analyzer(catalog, traits, Settings())
    source = MockSource(catalog)
    w = MainWindow(analyzer, source, Settings(), save_settings=False, start_polling=False)
    yield w, source
    w.close()
    w.deleteLater()
    _APP.processEvents()


def _final_state(source):
    source.tick = 10 * MockSource.TICKS_PER_PICK  # 全員ピック済みの時点
    return source.poll()


def test_renders_mock_state(window):
    w, source = window
    w.on_state(_final_state(source))
    texts = {lane: (row.ally_chips[0].text(), row.enemy_chips[0].text()) for lane, row in w.rows.items()}
    assert texts["top"] == ("ダリウス", "ケイル")
    assert texts["bottom"][0].startswith("ADC  ジンクス")
    assert w.rows["top"].bar.verdict == "ally"
    assert w.advice_label.text()
    assert "青サイド" in w.side_label.text()
    w.grab()  # 描画で例外が出ないこと


def test_swap_enemy_positions(window):
    w, source = window
    w.on_state(_final_state(source))
    w.swap_positions("enemy", "top", "middle")
    assert w.rows["top"].enemy_chips[0].text().startswith("オリアナ")
    assert w.rows["middle"].enemy_chips[0].text().startswith("ケイル")
    # 次のポーリングでも入れ替えは保持される
    w.on_state(source.poll())
    assert w.rows["top"].enemy_chips[0].text().startswith("オリアナ")
    w.reset_overrides()
    assert w.rows["top"].enemy_chips[0].text().startswith("ケイル")


def test_tier_change(window):
    w, _ = window
    w.tier_combo.setCurrentIndex(w.tier_combo.findData("GOLD"))
    assert w.settings.tier == "GOLD"


def test_empty_state(window):
    w, _ = window
    assert w.rows["top"].bar.verdict == "unknown"
    assert w.rows["top"].ally_chips[0].text() == "未選択"
