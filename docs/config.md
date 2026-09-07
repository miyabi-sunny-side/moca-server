# 設定 (環境変数)

環境変数の名称・必須条件・既定値・不正値の扱いは
[README の環境変数一覧](../README.md#環境変数)を参照。
このページでは backend と外部 asset の設定方法を説明する。

`VOICEPEAK` はインストール先の絶対パスを推奨する。VOICEPEAK は `dic/`、`fonts/`、
`settings/` などの兄弟ディレクトリも参照するため、バイナリだけを移動せず配置を保つ。
`PATH` に通す場合はラッパーからインストール先を `exec` する方法も使える。

## 感情分析 backend の切替

`/analyze` と流し込み (演技モード) は `ANALYZE_BACKEND` で選んだ backend を経由する。安全側の既定は `none` (呼ぶと 502 を返す)。

- **`cli`** — `ANALYZE_CMD` に指定したコマンドを `sh -c` 経由で起動する。プロトコルは「stdin = プロンプト全文, stdout = 台本 JSON 配列」の 1 本だけ。`claude -p --model haiku` はもちろん、`ollama run llama3` や `curl … | jq …` のようなワンライナーも書ける。認証は各 CLI 任せ。**依存**: 指定したコマンド本体 (`claude` / `ollama` / `curl` など) を別途インストールしておく必要がある。既定の `claude` は [Claude Code CLI](https://claude.com/product/claude-code) が入っていれば動く。
- **`openai`** — `POST {OPENAI_API_BASE}/chat/completions` に `{model, messages:[{role:"user", content: プロンプト}]}` を投げ、`choices[0].message.content` を台本 JSON として解釈する。OpenAI Chat Completions 互換ならほぼそのまま通る。**依存**: 外部の HTTP エンドポイントのみ (追加インストール不要)。ローカル LLM (LM Studio / Ollama `/v1` / vLLM / LocalAI) に向ければ課金なしで動く。
- **`none`** — `/analyze` を封じる。台本を手書きするか、事前生成した JSON だけを `/say` に投げる運用向け。**依存**: なし。

## 作業タブの立ち絵素材

作業タブ (ポモドーロ+声かけ) の立ち絵は公式配布イラストを使う。**再配布禁止のため
リポジトリには同梱しない** — bep-eng.dic と同方式で、`MOCA_ASSETS_DIR` に素材が
無ければ起動時に `MOCA_ILLUST_URL` (既定: 公式サイトの moca_illust.zip、約150MB) を
バックグラウンドで DL して展開する。設定は不要で、初回起動後しばらくすると立ち絵が
出るようになる (DL 完了まで・失敗時は立ち絵なしのまま動く)。

- 展開後は `<MOCA_ASSETS_DIR>/moca_illust/001.png` 〜 `232.png` と利用ガイドライン txt が並ぶ
  (Thumbs.db は展開時に捨てる)
- zip の内訳は 18 目元セット × 13 口差分 (あいうえお等。2 セットだけ 12 枚で全 232 枚) で、
  クライアントが瞬きと口パクに使う
- 手動配置したい場合は `MOCA_ILLUST_URL=` (空文字) で自動 DL を無効化し、
  `MOCA_ASSETS_DIR` に自分で展開する
- 利用は AHS のキャラクター使用ガイドライン (非商用個人利用は申請不要) に従うこと

## 英単語→カタカナのフォールバック辞書

tmux セッション名などの英単語 (例: `comic`) がスペル読みされる問題への対策として、
SQLite の読み替え辞書の後段に英単語→カタカナのフォールバック辞書が入る。

- 適用は SQLite 辞書と同じ全経路 (say plain/script、audio.wav script/text)。`?raw=1` ではスキップ
- 完全一致した英字トークンのみ置換 (大文字小文字無視)。未収録語は素通し
- 辞書は GPLv2 系のためリポジトリに同梱せず、起動時に `BEP_DICT_URL` から取得して
  `BEP_DICT_PATH` にキャッシュする。bep-eng.dic 原本 (Shift_JIS・半角カナ) と
  alkana 派生 CSV (UTF-8) の両形式を透過的に読める
- 管理画面の辞書 CRUD には出てこない静的な下位ティア
