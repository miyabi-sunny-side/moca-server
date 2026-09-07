# テスト

- サーバー側 (Rust): `cargo test` で各モジュールのユニットテストが走る。
- 通知 listener: `bash tests/moca-listen.sh` で音量・プレイヤー選択・WAV 補正を検証する。
  `python3 tests/moca-listen-output.py` は一時 localhost SSE サーバーと実 curl、player stub を使う。
  通知なしの接続成立、再接続、日時と出力先、keepalive、再生順序・成功・失敗、シグナル終了を確認する。
  配布物は `python3 tests/moca-listen-output.py /path/to/moca-listen` で同じ検証を行える。
  Python は検証だけに使用し、listener の実行には不要。実機の音声出力はこの検証に含まない。
- クライアント側 (Svelte SPA): E2E テスト (Playwright + Chromium) を整備済み。
  `client/` で `bun run test` を実行すると、`vite build && vite preview` を自動起動し
  (バックエンド・VOICEPEAK は不要。全 API は `page.route` でモックする) 全テストが走る。
  ブラウザ実体は Chromium (`~/.cache/ms-playwright`)。

## E2E のカバレッジ

`client/tests/*.spec.js`、ファイル名の番号が下記項目に対応:

1. 初回フォーカス: プロジェクト1件でもプロジェクト列先頭にフォーカスリングが出て、`j`/`k` が境界で停止しリングが消えないこと (`01-initial-focus`)
2. `l` ガード: プロジェクト未選択・0行プロジェクトでフォーカスがプロジェクト列に残ること (`02-l-guard`)
3. `h`/`l` 列切替、`Enter` でプロジェクトを開く、`Enter` で演技行のアコーディオン開閉 (アナ行は no-op、`Space` では開閉しない) (`03-columns-accordion`)
4. `yy` → `p`/`P`: ヤンク後のペーストで addLine (POST) と reorder (PUT) が正しい位置で発行されること (`04-yank-paste`)
5. `dd`: 確認なしで DELETE、フォーカスが次行 (末尾なら前行) へ追従、ヤンクレジスタから `p` で復元 (`05-dd-delete`)
6. `o`/`O`: 新規作成モーダルから正しい位置に挿入 (`06-new-line`)
7. `a`/`i`: 編集モーダルのキャレット位置 (末尾/先頭)、演技行の編集確定で `/analyze` 再実行 (`07-text-edit`)
8. `m`: アナ→演技は `/analyze`、演技→アナは ConfirmModal (`08-mode-toggle`)
9. `n`: メガホンの ON/OFF、SSE の接続中/接続済み表示、スリープ・ネットワーク復旧時の再接続 (`09-notify-megaphone`)
10. ガード: input フォーカス中・モーダル表示中・辞書タブではショートカットが発火しないこと (`10-guards`)
11. `Space`: フォーカス中の対象を再生/停止 (プロジェクト列＝通し再生・台本列＝単発)、再度押下で停止 (`11-space-play`)
12. `?`: ショートカット一覧モーダルを ? キー / フッターボタンから開閉、表示中は他ショートカット無効 (`12-help-modal`)
13. プロジェクト列の `a`/`i`: 未選択プロジェクトも含めタイトル編集、キャレット位置・PATCH 内容・一覧反映 (`13-project-rename`)
14. 作業タブ: タブ切替とショートカット無効、`page.clock` によるフェーズ遷移 (作業→休憩→「どうする?」で停止→ユーザーがもう1セット)、終了時のおつかれさま、節目・チャッター (独り言) の声かけ (`/say` への script JSON POST)、一時停止/再開の期限再計算、スリープ復帰の resync 潰し、LLM 声かけ (`/work/talk`) の成功経路と 502 フォールバック、おしゃべり「なし」(`14-work-tab`)

## E2E では担保できないため、リリース前に手動確認する項目

- ページをリロードした直後に `n` でメガホンが ON になり、通知音声が実際に再生されること (Safari 等の autoplay 制限下でも)
