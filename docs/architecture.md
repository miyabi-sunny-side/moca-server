# アーキテクチャ

```
クライアント / ブラウザ                サーバー
┌──────────────┐   POST /analyze  ┌─────────────────────────────┐
│ moca "..."    │ ───────────────▶ │ LLM backend (cli / openai)   │
│   │           │ ◀─────────────── │   テキスト → 台本JSON         │
│   │ 台本JSON  │                  │                              │
│   ▼           │   POST /say      │ moca-server (Rust / axum)    │
│ (保存・編集可) │ ───────────────▶ │   segment毎に VOICEPEAK 起動  │
│   ▼           │ ◀─────────────── │   → 生PCM → Opus/WAV へ変換   │
│ ffplay 再生   │  audio/ogg (既定) └─────────────────────────────┘
└──────────────┘   (chunked)
```

文単位で逐次合成するため、1文目ができた時点（約1.2秒）で再生が始まる。

## 構成

- 単一の Rust バイナリ (`moca-server`) — Web サーバー (axum)。VOICEPEAK の起動・文分割・
  台本合成・Opus/WAV 変換・SPA 配信をすべて内蔵する
- `client/` — 管理画面 SPA (Svelte + Vite。`client/` 内で Bun でビルドし `client/build` へ出力)
- `bin/moca` — CLI クライアント (bash / curl / ffplay があれば動く)
- `bin/moca-notify` — 通知テキストを `POST /notify` に投げる薄いラッパー (bash / curl)
- `bin/moca-listen` — 通知 SSE の常駐購読・自動再接続・OS別音声再生 (bash / curl)

## 設計上の判断と経緯

- **合成はサーバー全体で直列化**する。VOICEPEAK CLI は同時 1 プロセスに制限されており、
  2 個目の起動は拒否されるため。
- **Docker での運用は非対応**。実測で VOICEPEAK はコンテナ内でのみ合成を拒否した。
  ライブラリ・デバイス・環境変数を揃えても再現し、コンテナ検知と推定。
  このためネイティブバイナリ + systemd (user scope) 常駐を正とする ([deploy.md](./deploy.md))。
- **/say の既定は Ogg/Opus 64kbps mono** (WAV の約 1/10)。動画素材など可逆が必要な
  場面だけ `Accept: audio/wav` で WAV を返す。
- **通知 (/notify) は fire-and-forget**。永続化・再送なし。誰も購読していなければ捨てる。
  サーバーは「テキストを受けて購読中の SPA に音声で届ける」だけを担当し、
  tmux hook などの送信側の実装はスコープ外。
- **読みの確認は、聞き取りを外部コマンドに出す** (`READING_LISTEN_CMD`)。合成した音声を音素で
  聞き取る model は Python の実行環境を要るので、サーバーのバイナリへ入れず、感情分析の `cli` backend と
  同じ「stdin に渡して stdout を読む」形で繋ぐ。生成 AI の読みは誤る事があるので、直すのは
  食い違いを聞き直しても同じ読みを返し、2 択でも選び切り、直した音声で確かめられた語だけにする
  ([config.md](./config.md#読みの確認))。
- **英単語→カタカナのフォールバック辞書** (bep-eng.dic 系) は GPLv2 のため同梱せず、
  起動時にダウンロードしてキャッシュする ([config.md](./config.md))。
