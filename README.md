# レーン主導権（LoL レーン主導権判定アプリ）

チャンピオンセレクト中に、全レーン（TOP / JG / MID / BOT）の **序盤の主導権をどちらが取りやすいか** を
マッチアップから自動で判定して表示する Windows 用デスクトップアプリです。仕様は [SPEC.md](SPEC.md)。

![画面イメージ](docs/screenshot.png)

## セットアップ

```powershell
cd C:\Users\iwary\lol-lane-priority
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
```

## 起動

```powershell
.\.venv\Scripts\python.exe -m app          # 本番（LoL クライアントに自動接続）
.\.venv\Scripts\python.exe -m app --mock   # クライアント無しでデモ用のチャンセレを流す
```

`run.bat` をダブルクリックしても起動できます（コンソールなし）。

LoL クライアントを先に起動しておく必要はありません。起動していなければ 5 秒ごとに探し直します。

## 画面の見方

| 表示 | 意味 |
|---|---|
| バーが左に伸びる（青） | 味方有利。`◀` の数が多いほど差が大きい（10以上 / 30以上 / 60以上） |
| バーが右に伸びる（赤） | 敵有利 |
| 灰色「互角」 | スコアの絶対値が 10 未満 |
| 札の「?」・点線の枠 | 敵ポジションの推定確信度が 60% 未満 |
| 札の「（選択中）」 | まだロックしていない（ホバー中） |

- **行をクリック**すると判定の根拠（各項目がスコアに何点効いているか）が開きます
- **敵の札をドラッグ**して別の札に落とすとポジションを入れ替えます（右クリックからも可）。
  入れ替えは次のチャンピオンセレクトが始まるまで保持され、「推定リセット」で元に戻ります
- **ランク帯**は右上で自由に選べます（実績DBを使うときに、どのランク帯のデータで判定するか）
- 下の黄色い欄は、主導権のあるサイドから見た JG の動き方の目安です

## 判定のしくみ

スコアは味方視点の -100〜+100。

| 要素 | 内容 | 重み |
|---|---|---|
| A. 実績データ | 同マッチアップの 10分ゴールド差（主指標）・10分XP差・10分までのソロキル | 0.6 |
| B. チャンピオン特性 | 射程・序盤の強さ・ウェーブクリア・Lv6スパイク・機動力（JG はクリア速度・ガンク力・インベード耐性） | 0.3 |
| C. 補正 | 熟練度など | 0.1（**未実装**） |

- A は試合数が「必要試合数」（既定 200）に届かない分だけ重みを B に回します。実績DBが無ければ B だけで判定します
- C が未実装なので、最終的には A と B の重みで正規化しています（データ十分なら A:B = 2:1）
- BOT は ADC 同士と SUP 同士を項目ごとの比率で合算した 2v2 評価です
- 敵ポジションは、ロール出現率の積が最大になる割り当て（120 通り総当たり）で推定します。
  スマイトが見えていれば JG として扱います

## 実績データの集め方（任意）

Riot API はマッチアップ統計を直接くれないので、Match-V5 の試合を集めて自分で集計します。
[Riot Developer Portal](https://developer.riotgames.com/) で API キー（開発用キーは 24 時間で失効）を取得してください。

```powershell
$env:RIOT_API_KEY = "RGAPI-xxxxxxxx"
.\.venv\Scripts\python.exe -m batch.collect_matches --platform jp1 --tiers GOLD,PLATINUM --players 30 --matches 20
```

- 保存先は既定で `data/matchups.sqlite`（アプリが自動で読みます）
- 取り込み済みの試合はスキップするので、Ctrl+C で止めても続きから再開できます
- 開発用キーのレート制限（20回/秒・100回/2分）を守るので、1試合あたり約 2.4 秒かかります
  （600 人×20 試合なら数時間〜）。実用的な精度には数万試合が必要です
- DB が溜まると、ロール出現率も同梱の概算値ではなく DB の値（50 試合以上あるチャンプ）を使います

| オプション | 既定 | 内容 |
|---|---|---|
| `--platform` | jp1 | jp1 / kr / na1 / euw1 など |
| `--tiers` | 全ランク帯 | `IRON,BRONZE,...,CHALLENGER` からカンマ区切り |
| `--players` | 20 | ランク帯ごとのプレイヤー数 |
| `--matches` | 20 | プレイヤーごとの試合数（最大 100） |
| `--db` | data/matchups.sqlite | 保存先 |

## 設定

右上の「設定」から変更できます（`%APPDATA%\lol-lane-priority\settings.json` に保存）。

| 項目 | 内容 |
|---|---|
| lockfile の場所 | 空欄なら自動検出（よくあるインストール先 → 起動中プロセスのコマンドライン） |
| 実績DB | SQLite のパス |
| 実績データの必要試合数 | これ未満のマッチアップは特性表に寄せる |
| 使う直近パッチ数 | 0 で全パッチ |

## データファイル

| ファイル | 内容 |
|---|---|
| `data/champions_ja.json` | Data Dragon のチャンピオン一覧（id・日本語名・射程・タグ）。起動時に最新版を取れたら `%APPDATA%` にキャッシュして差し替える |
| `data/champion_traits.json` | 特性表（1〜5 の手作業の概算値）。気になる値は直接編集してください |
| `data/role_rates.json` | ロール出現率（手作業の概算値） |

ロック・ザーヘン・ユナラは情報が少ないため値を推定しています（`_estimated`）。表に無い新チャンピオンは
Data Dragon のタグから既定値を作り、根拠欄に「特性値は推定」と出ます。

## 規約について

- 使うのはクライアントがローカルに公開している API（LCU・Live Client Data）と Riot 公式 API だけです
- ゲームのメモリ読み取り・入力の自動化はしません。敵の情報はチャンピオンセレクトで見えるものだけを使います
- 他人に配布・公開する場合は、Riot Developer Portal でのアプリ登録と Production キーの申請が必要です

## テスト

```powershell
.\.venv\Scripts\python.exe -m pytest -q
```

## 構成

```
app/
  main.py            起動口（python -m app）
  sources.py         取得元（クライアント / モック）
  lcu_client.py      LCU 接続・チャンセレ解析
  live_client.py     Live Client Data API（試合中）
  role_inference.py  ポジション推定
  scorer.py          主導権スコア計算・JG の動き方の目安
  analyzer.py        推定 → 計算 をまとめる
  matchup_db.py      実績DB（SQLite）
  champions.py       Data Dragon
  traits.py          特性表・ロール出現率
  ui/                画面（PySide6）
batch/
  collect_matches.py Match-V5 収集・集計
data/                同梱データ
tests/
```

## 未実装（SPEC.md の v1 以降）

- F-11 熟練度による補正（スコア要素 C）
- F-12 試合後の答え合わせ（予測と実際の 10分ゴールド差の比較）
- F-13 パッチごとのデータ自動更新（今は手動でバッチを実行）
- F-14 読み上げ
