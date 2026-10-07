# lane-core / lane-priority（Rust 版）

Python 版（`app/`）を Rust に移植したものです。UI は egui（eframe）で作りました。
データ JSON は exe に埋め込んであるので、`lane-priority.exe` 1つで動きます。

## 使い方

```sh
cargo run --release              # 本物のクライアントにつなぐ
cargo run --release -- --mock    # クライアント無しでデモのチャンセレを流す
cargo run --release -- --offline # Data Dragon の更新確認をしない
cargo test                       # テスト
```

ビルドした exe は `target/release/lane-priority.exe` に出ます（約 9MB。Python 版の dist は約 120MB）。

## 構成

| ファイル | 中身 | Python 版の対応 |
|---|---|---|
| `src/models.rs` | データ型・定数 | `app/models.py` |
| `src/config.rs` | 設定（Python 版と同じ `%APPDATA%\lol-lane-priority\settings.json` を共有）・同梱データ | `app/config.py` |
| `src/champions.rs` | チャンピオン一覧・Data Dragon 取得 | `app/champions.py` |
| `src/traits.rs` | 特性表・ロール出現率 | `app/traits.py` |
| `src/matchup_db.rs` | 実績DB（SQLite。スキーマは Python 版と同じ） | `app/matchup_db.py` |
| `src/role_inference.rs` | ポジション推定 | `app/role_inference.py` |
| `src/scorer.rs` | スコア計算 | `app/scorer.py` |
| `src/analyzer.rs` | 推定＋スコアのまとめ | `app/analyzer.py` |
| `src/sources.rs` | LCU / Live Client / モック | `app/lcu_client.py` `app/live_client.py` `app/sources.py` |
| `src/main.rs` | 画面（egui） | `app/ui/` `app/main.py` |

## Python 版との一致確認

`tests/gen_golden.py` が Python 版の計算結果を `tests/golden.json` に書き出し、
`tests/parity.rs` が Rust 版と比べます（スコア・内訳・ポジション推定の確信度などを 1e-9 以内で比較）。
Python 側の計算を変えたら、次のコマンドで golden.json を作り直してください。

```sh
python rust/tests/gen_golden.py
```

## 未移植・既知の差分

- Match-V5 の集計バッチ（`batch/collect_matches.py`）は Python のまま。DB のスキーマが同じなので、Python で集計した DB を Rust 版で読めます。
- 設定画面の「参照…」（ファイル選択ダイアログ）は無く、パスを直接入力します。
- 根拠の表示は、行全体ではなく、レーン名かスコアバーのクリックで開閉します。
- 手元の Rust 1.90 に合わせ、`.cargo/config.toml` で依存を Rust 1.90 対応版に固定しています（eframe 0.33）。Rust を更新すれば最新版も使えます。
