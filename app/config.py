"""設定の読み書き。設定ファイルは %APPDATA%\\lol-lane-priority\\settings.json に置く。"""
from __future__ import annotations

import json
import os
from dataclasses import asdict, dataclass, fields
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = ROOT / "data"
DEFAULT_DB_PATH = DATA_DIR / "matchups.sqlite"


def user_dir() -> Path:
    base = os.environ.get("APPDATA") or str(Path.home())
    return Path(base) / "lol-lane-priority"


def default_settings_path() -> Path:
    return user_dir() / "settings.json"


@dataclass
class Settings:
    tier: str = "ALL"  # 判定に使うランク帯（利用者が自由に選ぶ）
    min_games: int = 200  # 実績データを満額の重みで使うのに必要な試合数
    recent_patches: int = 2  # 実績データとして使う直近パッチ数（0 なら全パッチ）
    lockfile_path: str = ""  # 空なら自動検出
    db_path: str = str(DEFAULT_DB_PATH)
    poll_interval_ms: int = 500

    @classmethod
    def load(cls, path: Path | None = None) -> "Settings":
        path = path or default_settings_path()
        try:
            raw = json.loads(Path(path).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return cls()
        known = {f.name for f in fields(cls)}
        return cls(**{k: v for k, v in raw.items() if k in known})

    def save(self, path: Path | None = None) -> None:
        path = Path(path or default_settings_path())
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(asdict(self), ensure_ascii=False, indent=2), encoding="utf-8")
