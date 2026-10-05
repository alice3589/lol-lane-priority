import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from app.champions import BUNDLED_PATH, ChampionCatalog  # noqa: E402
from app.traits import TraitTable  # noqa: E402

import json  # noqa: E402


@pytest.fixture(scope="session")
def catalog():
    # キャッシュの影響を受けないよう同梱データだけを使う
    return ChampionCatalog(json.loads(BUNDLED_PATH.read_text(encoding="utf-8")))


@pytest.fixture(scope="session")
def traits(catalog):
    return TraitTable.load(catalog)
