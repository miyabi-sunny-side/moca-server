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

`/analyze` は、返った台本の `text` を繋げたものが入力と違う時 (文字化け・言い換え・文の抜け) も失敗として扱う。
比べるのは文字と数字だけで、空白・かぎ括弧・句読点の違いは許す。
失敗はどの backend でも 2 回まで試し、それでも駄目なら 502 を返す。

### OpenAI 互換 API への要求を調整する

`openai` backend の要求は、既定では `{model, messages}` だけである。相手に合わせて次の 3 つを足せる。

- `OPENAI_EXTRA_BODY` — 要求の本文へ足す JSON object。thinking を切る指定など、相手ごとの項目を書く。
- `OPENAI_JSON_SCHEMA=1` — 台本の JSON Schema を `response_format` (`json_schema`) として付ける。
  出力の形を縛れる相手 (llama.cpp の server など) で、壊れた JSON を防ぐ。配列を最上位に置く Schema なので、
  最上位に object を求める API では使えない。
- `OPENAI_TIMEOUT_SECS` — 1 回の要求を待つ秒数。未設定なら応答を待ち続ける。
  相手が接続を受けない時は、この設定に関わらず 5 秒で諦める。

llama.cpp の server で thinking のある model (Gemma 4 など) を使う例:

```sh
ANALYZE_BACKEND=openai
OPENAI_API_BASE=http://localhost:8080/v1
OPENAI_API_KEY=local
OPENAI_MODEL=gemma-4-12b
OPENAI_EXTRA_BODY='{"reasoning_effort":"none"}'
OPENAI_JSON_SCHEMA=1
OPENAI_TIMEOUT_SECS=60
```

thinking を切らないと、4 文のセリフに 1 分以上かかる (Gemma 4 12B・RTX 3060 での実測。切ると約 3 秒)。
`OPENAI_API_KEY` は認証の無い相手でも空にできないので、任意の文字列を入れる。

## 読みの確認

VOICEPEAK は「今日も一日」を「ついたち」と読むように、文脈に合わない読みをする事がある。
読みの確認は、project の行ごとに次を行い、読み違えた語だけを直す。

1. 行の読み上げ文 (辞書を適用した後の、VOICEPEAK へ渡す文) を生成 AI に渡し、語ごとの読みを得る。
2. 同じ文を今の設定のまま合成し、音声から実際の読みを聞き取る。
3. 読みが食い違った語を、平仮名にした表記で合成し直して聞く。合わなければカタカナで試し、
   それでも合わなければ元の表記へ戻す (合成し直すのは 1 文につき 3 回まで)。
4. 読みが合った表記を行に保存する。以後、その行の読み上げ (画面の再生・`audio.wav`) に使う。

画面に出る文 (`text`) と台本は書き換えない。1 行に 3〜12 秒かかるので、確かめるのは
画面の「読みを確認」を押した時だけである。確かめた行は飛ばすので、2 回目以降は増えた行だけになる。
文を編集した行と、辞書の変更で読み上げ文が変わった行は、古い直しを使わず、次の確認の対象になる。

必要なもの:

- **生成 AI** — `ANALYZE_BACKEND` (`cli` / `openai`) をそのまま使う。
- **聞き取りの道具** — `READING_LISTEN_CMD` に、stdin の WAV を聞いて stdout に OpenJTalk の音素
  (空白区切り) を返すコマンドを書く。同梱の [`moca-hear`](./cli.md#読みの聞き取り-binmoca-hear) がその形で返す。

```sh
READING_LISTEN_CMD=/home/you/.local/bin/moca-hear
```

どちらかが無い・動かない時、確認は 502 を返すか、その行を「確認できず」として何も保存しない。
読み上げは今までどおり続く。

### 直し過ぎを抑える決まり

生成 AI の読みは正しいとは限らない (同形異音語をよく誤る)。誤った読みへ「直す」事を避けるため、
次の全てを満たした語だけを直す。

- 漢字・数字を含み、英字を含まない語である (かな・英字の語は、辞書と VOICEPEAK の読みのままにする)。
- 生成 AI に同じ問いをもう 1 度尋ねても、同じ読みを返す。
- 生成 AI に「生成 AI の読み」と「聞き取った読み」を並べて見せ、並び順を入れ替えた 2 回とも、
  生成 AI の読みを選ぶ (「どちらも正しい」「どちらも違う」と答えた語は直さない)。
- 直した表記を合成し直して聞くと、その語の読みが合い、他の語の読みが変わらない。

このため、読み違いを見つけても直さない事がある (画面には「そのまま」「直せず」と出る)。
文脈に依らずいつも同じ読みにしたい語は、辞書へ入れる方が確実である。
イントネーションは確かめない。

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
