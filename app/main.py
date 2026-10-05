"""起動口: python -m app  （クライアント無しで画面を見るなら python -m app --mock）"""
from __future__ import annotations

import argparse
import sys
import threading

from PySide6.QtWidgets import QApplication

from .analyzer import Analyzer
from .champions import ChampionCatalog, refresh_cache
from .config import Settings
from .matchup_db import MatchupDB
from .sources import ClientSource, MockSource
from .traits import TraitTable
from .ui.main_window import MainWindow


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="LoL レーン主導権判定アプリ")
    parser.add_argument("--mock", action="store_true", help="クライアント無しでデモ用のチャンセレを流す")
    parser.add_argument("--offline", action="store_true", help="Data Dragon の更新確認をしない")
    args = parser.parse_args(argv)

    app = QApplication(sys.argv[:1])
    settings = Settings.load()
    catalog = ChampionCatalog.load()
    traits = TraitTable.load(catalog)
    db = MatchupDB.open_if_exists(settings.db_path)
    analyzer = Analyzer(catalog, traits, settings, db)
    source = MockSource(catalog) if args.mock else ClientSource(catalog, settings.lockfile_path)

    window = MainWindow(analyzer, source, settings)
    window.show()

    if not args.offline:
        def update_catalog():
            new = refresh_cache()
            if new is not None and new.version != catalog.version:
                window.catalog_updated.emit(new)

        threading.Thread(target=update_catalog, daemon=True).start()

    code = app.exec()
    # QApplication より先にウィンドウを破棄しないと終了時に落ちることがある
    window.stop_polling()
    del window
    if db is not None:
        db.close()
    return code


if __name__ == "__main__":
    sys.exit(main())
