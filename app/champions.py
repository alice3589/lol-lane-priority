"""チャンピオン一覧（Data Dragon）の読み込み。

同梱の data/champions_ja.json を基本にし、起動時に Data Dragon から最新版を取れたら
%APPDATA% 側のキャッシュに保存して差し替える。
"""
from __future__ import annotations

import json
import re
from pathlib import Path

import requests

from .config import DATA_DIR, user_dir

BUNDLED_PATH = DATA_DIR / "champions_ja.json"
VERSIONS_URL = "https://ddragon.leagueoflegends.com/api/versions.json"
CHAMPIONS_URL = "https://ddragon.leagueoflegends.com/cdn/{version}/data/{locale}/champion.json"

DEFAULT_RANGE = 300


def cache_path() -> Path:
    return user_dir() / "champions_ja.json"


def _version_tuple(version: str) -> tuple[int, ...]:
    return tuple(int(x) for x in re.findall(r"\d+", version))


class ChampionCatalog:
    def __init__(self, data: dict):
        self.version: str = data.get("version", "")
        self._champs: dict[str, dict] = data["champions"]
        self._by_key = {int(c["key"]): cid for cid, c in self._champs.items()}
        self._by_lower = {cid.lower(): cid for cid in self._champs}
        self._by_name = {c["name"]: cid for cid, c in self._champs.items()}

    @classmethod
    def load(cls, cache: Path | None = None) -> "ChampionCatalog":
        """同梱データとキャッシュのうち、バージョンが新しい方を使う。"""
        bundled = json.loads(BUNDLED_PATH.read_text(encoding="utf-8"))
        cache = cache or cache_path()
        try:
            cached = json.loads(Path(cache).read_text(encoding="utf-8"))
            if _version_tuple(cached.get("version", "")) > _version_tuple(bundled.get("version", "")):
                return cls(cached)
        except (OSError, ValueError, KeyError):
            pass
        return cls(bundled)

    def ids(self) -> list[str]:
        return sorted(self._champs)

    def id_by_key(self, key: int | None) -> str | None:
        if not key:
            return None
        return self._by_key.get(int(key))

    def name(self, cid: str | None) -> str:
        if not cid:
            return ""
        c = self._champs.get(cid)
        return c["name"] if c else cid

    def attack_range(self, cid: str) -> int:
        c = self._champs.get(cid)
        return int(c["attackrange"]) if c else DEFAULT_RANGE

    def tags(self, cid: str) -> list[str]:
        c = self._champs.get(cid)
        return list(c["tags"]) if c else []

    def resolve(self, text: str | None) -> str | None:
        """いろいろな表記から Data Dragon の id を引く。

        例: "Darius" / "darius" / "game_character_displayname_Darius" / "ダリウス" / "FiddleSticks"
        """
        if not text:
            return None
        text = text.strip()
        if text.startswith("game_character_displayname_"):
            text = text[len("game_character_displayname_"):]
        if text in self._champs:
            return text
        if text.lower() in self._by_lower:
            return self._by_lower[text.lower()]
        return self._by_name.get(text)


def fetch_ddragon(locale: str = "ja_JP", timeout: float = 5.0) -> dict:
    """Data Dragon から最新のチャンピオン一覧を取って、アプリ用の軽い形に変換する。"""
    version = requests.get(VERSIONS_URL, timeout=timeout).json()[0]
    raw = requests.get(CHAMPIONS_URL.format(version=version, locale=locale), timeout=timeout).json()
    return {
        "version": version,
        "champions": {
            cid: {
                "key": int(c["key"]),
                "name": c["name"],
                "tags": c["tags"],
                "attackrange": c["stats"]["attackrange"],
            }
            for cid, c in raw["data"].items()
        },
    }


def refresh_cache(timeout: float = 5.0) -> ChampionCatalog | None:
    """最新版を取得してキャッシュに保存する。失敗したら None。"""
    try:
        data = fetch_ddragon(timeout=timeout)
    except (requests.RequestException, ValueError, KeyError, IndexError):
        return None
    path = cache_path()
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    except OSError:
        pass
    return ChampionCatalog(data)
