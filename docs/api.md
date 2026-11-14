# API リファレンス

| エンドポイント | 説明 |
|---|---|
| `GET /` | ブラウザ用の管理画面 (SPA) |
| `POST /analyze` (本文=テキスト) | LLM backend で感情を推定し台本JSONを返す (約10秒)。`?carry=0〜0.9` で感情の引きずり量を調整 (デフォルト 1/3、0で無効)。backend 無効・失敗、台本として読めない出力、`text` が本文と違う出力は 502 |
| `POST /say` (text/plain) | そのまま合成した音声をチャンク配信。既定は `audio/ogg` (Opus 64kbps)。`Accept: audio/wav` で可逆 WAV |
| `POST /say` (application/json) | 台本JSONを感情パラメータ付きで合成 (既定 Opus / `Accept: audio/wav` で WAV) |
| `GET /say?text=...` | text/plain 版と同じ (ブラウザの `<audio>` 向け) |
| `/say?line=<行の id>` | 上の 3 つに付けられる。その行の[読みの確認](#読みの確認)で直した表記があれば、それで読む。`raw=1` と併用した時、行が無い時、文が確かめた時と違う時は、渡された文のまま読む。数値でない id は 400 |
| `GET /api/projects/:id/lines/:lineId/audio.wav` | 保存済みの行を合成して WAV で返す (動画素材は可逆が正なので Accept 不問で常に WAV)。acting かつ script があれば台本経路、それ以外は text 文分割経路。読みの確認で直した表記があれば、それで読む |
| `POST /api/projects/:id/reading/check` (application/json) | project の行を順に確かめ、読み違いを直す ([読みの確認](#読みの確認))。本文 `{"force": true}` で確かめ済みの行も確かめ直す (省略可)。text/event-stream で 1 行ごとに進捗を返す。`READING_LISTEN_CMD` 未設定・分析 backend 無効は 502、project が無ければ 404 |
| `POST /api/lines/:id/reading/dismiss` | その行の直しを外し、行を返す。確かめた記録は残るので、確かめ直しても直し直さない (行の文を変えると記録ごと消える)。確かめていない行は 409 |
| `POST /notify` (text/plain) | 本文を購読中の SPA に broadcast する。空文字は 400、それ以外は 204。永続化・再送なし (誰も購読していなければ捨てる) |
| `GET /notify/stream` | text/event-stream。通知購読の SSE ストリーム (管理画面ヘッダーのメガホン ON で接続) |
| `POST /work/talk` (application/json) | 作業タブの LLM 声かけ。`{"kind":"milestone"\|"chatter"\|"news","context":{"phase","hour"}}` (context は全て省略可、`phase` は `idle`/`work`/`break` のみ) を受け、台本JSONを返す。`news` は `WORK_NEWS_CMD` の backend を優先。backend 無効/失敗/タイムアウト (`WORK_TALK_TIMEOUT_SECS`) は 502 — 固定セリフへのフォールバックはクライアントの責務。未知の `kind` は 422 |
| `GET /moca-assets/*` | 作業タブの立ち絵素材 (`MOCA_ASSETS_DIR` を配信。無ければ起動時に自動 DL)。未取得の間は 404 (docs/config.md 参照) |

合成はサーバー全体で直列化される（VOICEPEAK CLI は同時 1 プロセスに制限されており、2 個目の起動は拒否されるため）。

## 台本JSONスキーマ

Segment の配列 (1要素 = 1文)。一息のボイスなら要素1つの配列になる:

```json
[
  { "text": "やった、ついに完成した！", "emotion": { "doyaru": 85 }, "speed": 110, "pitch": 20, "pause": 200 },
  { "text": "でも、ちょっと疲れたかも……。", "emotion": { "bosoboso": 43, "doyaru": 38 }, "speed": 95 }
]
```

- `emotion` — bosoboso / doyaru / honwaka / angry / teary (各 0-100)。省略可
- `speed` (50-200) / `pitch` (-300〜300) — 文単位で上書き。省略可
- `pause` — 文の後の無音 (ms)。省略可
- 範囲外の値はクランプ、未知の感情軸は無視される

`/analyze` は文単位の感情の急変を防ぐため、LLM の推定結果に指数移動平均をかけてから返す:
`実効値[i] = (1 - carry) × 自身の値 + carry × 実効値[i-1]`（感情・speed・pitch が対象、pause は対象外）。
1文目はそのまま。前の文の感情が carry (デフォルト 1/3) ずつ減衰しながら後続の文に乗る。

## 読みの確認

行 (`GET /api/projects/:id` の `lines[]` など) は、確かめた結果を `reading` に持つ。確かめていない行は `null`。

```json
{
  "status": "fixed",
  "checked_at": "2026-11-15T00:00:00.000Z",
  "secs": 6.1,
  "segments": [
    {
      "text": "それでは今日も一日頑張りましょう。",
      "status": "fixed",
      "expected": "それではきょうもいちにちがんばりましょう",
      "heard": "それではきょうもついたちがんばりましょう",
      "spoken": "それでは今日もいちにち頑張りましょう。",
      "words": [{ "surface": "一日", "expected": "いちにち", "heard": "ついたち", "action": "fixed" }],
      "error": null
    }
  ]
}
```

- `segments[]` は読み上げの 1 区切り (1 文) ごとの結果。`text` は確かめた時の読み上げ文 (辞書を適用した後)、
  `expected` は生成 AI の読み、`heard` は直す前に聞き取った読み、`spoken` は直した表記 (無ければ `null`)。
- `words[]` は読みが食い違った語。`action` は `fixed` (直した) / `kept` (生成 AI が選び切らず、変えていない) /
  `unresolved` (平仮名でもカタカナでも読みが合わず、変えていない)。
- `status` は `ok` (読み違いなし) / `fixed` / `kept` / `unresolved` / `dismissed` (直しを外した)。
  行の `status` は、その行で最も目立つ segment の状態。
- 行の `text` を更新すると `reading` は `null` に戻る。行の複製は `reading` も写す。

`POST …/reading/check` の進捗は、行ごとに `data: {"index":1,"total":9,"status":"checked","error":null,"line":{…}}`。
`status` は `checked` (確かめて保存した) / `skipped` (確かめ済み) / `failed` (生成 AI か聞き取りが動かず、
何も変えていない。理由は `error`)。最後に `event: complete` と `data: {"total":9}` が来る。
接続を切ると、そこで止まる (確かめ終えた行は残る)。

## 制限

- VOICEPEAK CLI は 1 回の `-s` に渡せる文字数に上限があるため、
  1 文が極端に長い（140字超）と失敗することがある。
- VOICEPEAK は稀に起動直後に失敗することがあるため、サーバーは
  segment ごとに最大3回リトライする。
- `/analyze` の消費（API 料金 / CLI サブスクリプション枠）は選択した backend の請求元にそのまま乗る。
