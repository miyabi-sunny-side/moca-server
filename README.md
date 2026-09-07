# moca-server

VOICEPEAK (宮舞モカ) の音声合成を HTTP でストリーミング配信する家庭内サーバー。
LLM による感情パラメータの自動生成に対応。文単位で逐次合成するため、
1文目ができた時点（約1.2秒）で再生が始まる。

![管理画面 (台本工房)](./docs/images/screenshot.png)

## セットアップ

必要なもの: VOICEPEAK と `moca-server` バイナリ。感情分析を使うなら LLM backend
(`claude` CLI または OpenAI 互換 API) も ([設定の詳細](./docs/config.md))。

**リリースバイナリを使う** — [GitHub Releases](https://github.com/miyabisun/moca-server/releases)
から `moca-server` バイナリと SPA 成果物 `client-build.tar.gz` を取得 (`.sha256` で整合性確認)。
SPA 成果物は実行ディレクトリの `client/build` に展開してから起動する。

**ソースからビルド** — Rust toolchain と [Bun](https://bun.sh) だけで足りる
(Opus は純 Rust 実装の [ropus](https://crates.io/crates/ropus) を使うため C ライブラリ不要):

```sh
git clone https://github.com/miyabisun/moca-server.git
cd moca-server
cp .env.example .env        # PORT / DATABASE_PATH / VOICEPEAK などを調整
(cd client && bun install --frozen-lockfile && bun run build)
cargo build --release --locked
./target/release/moca-server
```

リリース CI は Rust / Bun のバージョンと Rust の target を
[workflow](./.github/workflows/release.yml) で固定する。Rust は `opt-level=3`、
LTO 無効、`codegen-units=16`、strip 有効でビルド待ち時間を抑える。
SPA は OS 共通の外部ファイルとして一度だけビルド・梱包し、
Linux / Windows / macOS の native バイナリとは別に配布する。

## 環境変数

サーバーは起動時に `.env` を読み込む。既に渡されたプロセス環境が優先される。
相対パスは起動ディレクトリ基準。下表はアプリの既定値であり、systemd の
`EnvironmentFile` に必要な値を明示する ([常駐化](./docs/deploy.md))。

| 変数 | 必須 / 任意 | 未設定時 | 用途・空値 / 不正値の扱い |
|---|---|---|---|
| `PORT` | 任意 | `3000` | 待受ポート。空・整数でない値・`u16` 範囲外は既定値。`0` は OS による自動割当 |
| `DATABASE_PATH` | 任意 | `./moca.db` | SQLite ファイル。設定文字列をそのまま渡し、DB を開けなければ起動失敗 |
| `VOICEPEAK` | 任意 | `voicepeak` | 音声合成 CLI のパスまたはコマンド名。空・実行不能なら合成時に失敗。インストール先の絶対パス推奨 ([詳細](./docs/config.md)) |
| `MOCA_NARRATOR` | 任意 | `Miyamai Moca` | VOICEPEAK に渡す音源名。事前検証せず、空・未導入の音源などは合成時に判定 |
| `ANALYZE_BACKEND` | 任意 | `none` | `none` / `cli` / `openai`。空・未知値は起動失敗。`none` では感情分析無効 |
| `ANALYZE_CMD` | 任意（`cli` 時に使用） | `claude -p --model haiku` | `sh -c` で実行する LLM コマンド。前後空白を除いた空値は起動失敗。実行失敗は分析時のエラー |
| `OPENAI_API_BASE` | `openai` 時必須 | なし | OpenAI 互換 API ベース URL。末尾の `/chat/completions` を自動付与。未設定・空は起動失敗、不正 URL は分析時に失敗 |
| `OPENAI_API_KEY` | `openai` 時必須 | なし | API キー。未設定・空は起動失敗。不正な認証情報は API 呼び出し時に失敗。認証不要の API でも非空値が必要 |
| `OPENAI_MODEL` | `openai` 時必須 | なし | モデル ID。未設定・空は起動失敗。利用不可のモデルは API 呼び出し時に失敗 |
| `BEP_DICT_PATH` | 任意 | `./bep-eng.dic` | フォールバック辞書キャッシュ。読めなければ取得を試す。保存失敗は警告し、取得済み辞書はメモリ上で利用 |
| `BEP_DICT_URL` | 任意 | [alkana 派生 CSV](https://raw.githubusercontent.com/uesugi6111/alkana-rs/master/dictionary.csv) | キャッシュがない場合の取得元。空・不正 URL・取得失敗時は辞書無効で起動継続 |
| `MOCA_ASSETS_DIR` | 任意 | `./moca-assets` | 立ち絵の保存・配信先。空値もそのまま扱う。取得・保存失敗は警告し、立ち絵なしで継続 |
| `MOCA_ILLUST_URL` | 任意 | [公式イラスト ZIP](https://www.ah-soft.com/moca/moca_illust.zip) | 立ち絵取得元。空で自動取得無効。不正 URL・取得 / 展開失敗は警告して継続 |
| `WORK_NEWS_CMD` | 任意 | 通常の分析 backend を使用 | 作業タブの時事ネタ用 LLM コマンド。空白のみも未設定扱い。実行失敗は声かけ生成時に扱う |
| `WORK_TALK_TIMEOUT_SECS` | 任意 | `60` | 声かけ生成のタイムアウト秒。空・整数でない値・`u64` 範囲外は既定値。`0` も受理 |
| `NODE_ENV` | 任意 | HTML の更新日時を確認して再読込 | `production` のとき SPA HTML の初回キャッシュを固定。それ以外（空・未知値を含む）は更新確認 |
| `RUST_LOG` | 任意 | `info` | ログフィルター（例: `debug`、`moca_server=debug`）。現行の tracing-subscriber では空は `error`、構文不正は stderr に警告してログ無効。`LOG_LEVEL` は読まない |

### CLI クライアントと OS の実行環境

以下はサーバー設定ではない。`bin/moca*` は `.env` を読み込まず、クライアントの
プロセス環境を使う。すべて任意で、空値は未設定と同じ。

| 変数 | 未設定時 | 利用先・不正値の扱い |
|---|---|---|
| `MOCA_URL` | `http://localhost:3000` | `moca` / `moca-notify` / `moca-listen` の接続先。不正 URL・到達不能は curl のエラー |
| `MOCA_RETRY_DELAY` | `2` | `moca-listen` の再接続待機秒。事前検証せず `sleep` に渡すため、不正値では待機に失敗 |
| `MOCA_PLAYER` | 利用可能なプレイヤーを自動選択 | `moca-listen` の再生方法。未知値・指定プレイヤー未導入なら exit 1。選択肢は [CLI ドキュメント](./docs/cli.md) |
| `MOCA_VOLUME` | `100` | `moca-listen` の音量（1〜100 の整数、`--volume` 優先）。範囲外・非数値は exit 2 |
| `TMPDIR` | `/tmp` | `moca-listen` の WAV 一時ファイルを置く OS 用ディレクトリ。作成不能ならその再生に失敗 |

コマンド名で指定した VOICEPEAK、LLM CLI、クライアントのプレイヤーなどは OS の
`PATH` から探索する。CLI 自身の認証・音声デバイスの設定は各ツールの実行環境が所有する。

## 使い方

ブラウザで `http://localhost:3000/` を開くと管理画面 (台本工房)。
台本の作成・感情の微調整・再生ができ、[vim 風のキーボード操作](./docs/shortcuts.md)に対応する
(アプリ内の `?` キーでも一覧表示)。

CLI クライアント (`bash` / `curl` / `ffplay` があれば動く):

```sh
curl -o ~/bin/moca https://raw.githubusercontent.com/miyabisun/moca-server/main/bin/moca
chmod +x ~/bin/moca
export MOCA_URL=http://<server-host>:3000

# 感情分析つきで再生。台本JSONが stdout に表示される
moca "やった、ついに完成した！でも、ちょっと疲れたかも。"

# 感情分析なしでそのまま読み上げ (低レイテンシ。朗読・通知向け)
moca -r "ビルドが完了しました"
```

通知購読 (`bin/moca-notify`): 管理画面のメガホンを ON にしておくと、
`moca-notify "テキスト"` で送った通知をブラウザ側で宮舞モカが読み上げる。
tmux のベルと繋ぐと「ssh 先の完了通知を手元で聞く」ができる:

```tmux
set-hook -g alert-bell 'run-shell "moca-notify \"#{session_name} が待ってます\""'
```

ブラウザを開かず常駐購読する場合は `moca-listen` を使う。切断時は自動再接続し、
`ffplay` があれば Ogg/Opus、なければ macOS / Linux / WSL の標準系プレイヤーで WAV を再生する。

## ドキュメント

- [アーキテクチャと設計方針](./docs/architecture.md)
- [CLI クライアントの詳しい使い方](./docs/cli.md)
- [API リファレンス (台本JSONスキーマ・制限)](./docs/api.md)
- [設定 (環境変数・感情分析 backend・カタカナ辞書)](./docs/config.md)
- [キーボードショートカット](./docs/shortcuts.md)
- [常駐化 (systemd)・自動更新](./docs/deploy.md)
- [テスト (cargo test / Playwright E2E)](./docs/testing.md)

## 注意

認証なしの家庭内 LAN 専用。インターネットに公開しないこと。

## ライセンス

本リポジトリのコード (moca-server 本体) は [MIT ライセンス](./LICENSE) の下で配布する。

ただし本リポジトリは音声合成エンジン **VOICEPEAK 本体および音源データを一切含まない**。VOICEPEAK / 宮舞モカ (およびその他の音源) は AH-Software 株式会社の商用製品であり、その利用条件は同社のライセンスに従うこと。moca-server 側の MIT ライセンスは VOICEPEAK 本体には及ばない。
