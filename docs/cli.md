# CLI クライアント (`bin/moca` / `bin/moca-notify` / `bin/moca-listen`)

## インストール

必要なもの: `bash`, `curl`, `ffmpeg` (再生に使う `ffplay` 同梱)。

```sh
# macOS: brew install ffmpeg
# Linux (Debian/Ubuntu): sudo apt install ffmpeg curl
# Windows: WSL または Git Bash + scoop/choco で ffmpeg / curl を入れる

curl -o ~/bin/moca https://raw.githubusercontent.com/miyabisun/moca-server/main/bin/moca
chmod +x ~/bin/moca
export MOCA_URL=http://<server-host>:3000
```

## `moca` の使い方

```sh
# 感情分析つきで再生。台本JSONが stdout に表示される
moca "やった、ついに完成した！でも、ちょっと疲れたかも。"

# 台本を保存 → 微調整 → 同じ演技で何度でも再収録
moca "..." > take1.json
vim take1.json
moca < take1.json

# 感情分析なしでそのまま読み上げ (低レイテンシ。朗読・通知向け)
moca -r "ビルドが完了しました"
moca -r < novel.txt

# 通知の見張り (行単位の連続読み上げ)
tail -f app.log | grep --line-buffered ERROR | while read -r line; do moca -r "$line"; done

# curl 直叩き (感情なし・低レイテンシ)
curl -sN --data-binary "テキスト" http://localhost:3000/say | ffplay -nodisp -autoexit -
```

`moca` は入力の (空白を除いた) 先頭が `[`・末尾が `]` なら台本JSONとみなして
`/analyze` をスキップし、直接 `/say` に投げる。

## 通知購読 (`bin/moca-notify`)

`moca` が「今すぐ聞く」ワンショット再生なのに対し、`moca-notify` は
**購読中のブラウザに通知を届ける** pub/sub 用のクライアント。管理画面ヘッダー右上の
メガホントグルを ON にしておくと、`POST /notify` に届いたテキストを宮舞モカが
その場で読み上げる (`/analyze` は経由せず素の朗読、低レイテンシ)。永続化されない
fire-and-forget なので、誰も購読していなければ捨てられる。`MOCA_URL` は `moca` と共有する。

```sh
# インストール (クライアント側)
curl -o ~/bin/moca-notify https://raw.githubusercontent.com/miyabisun/moca-server/main/bin/moca-notify
chmod +x ~/bin/moca-notify

moca-notify "ビルドが完了しました"     # 引数で本文を渡す
echo "5分経過しました" | moca-notify   # stdin からも可
```

用途は「ssh ログイン先の tmux でベルが鳴ったら手元のブラウザで読み上げる」等。
tmux 連携例 (1 行):

```tmux
set-hook -g alert-bell 'run-shell "moca-notify \"#{session_name} が待ってます\""'
```

dotfiles 側の hook 実装本体は moca-server のスコープ外 (サーバーは「テキストを受けて
購読中の SPA に音声で届ける」だけを担当する)。

## 通知の常駐購読 (`bin/moca-listen`)

`moca-listen` は `/notify/stream` に接続し、届いた通知をCLI側で読み上げる。
ブラウザのバックグラウンドタブ制限を受けず、切断時は自動的に再接続する。

```sh
curl -o ~/bin/moca-listen https://raw.githubusercontent.com/miyabisun/moca-server/main/bin/moca-listen
chmod +x ~/bin/moca-listen
export MOCA_URL=http://<server-host>:3000
moca-listen
```

再生方法は起動時に自動選択する。`ffplay` があれば Ogg/Opus をストリーミング再生し、
なければ WAV と次のOS標準系プレイヤーを使う。

- macOS: `afplay`
- WSL: Windows PowerShell の `System.Media.SoundPlayer`
- Linux: `pw-play`, `paplay`, `aplay` の順

どのプレイヤーも見つからない場合は、ffmpegの導入方法を表示して終了する。再接続間隔は
`MOCA_RETRY_DELAY` で変更できる (既定: 2秒)。通知はサーバー側で永続化されないため、
切断から再接続までに送られた通知は再生されない。自動選択を上書きする場合は
`MOCA_PLAYER` に `ffplay`, `afplay`, `pw-play`, `paplay`, `aplay`,
`windows-soundplayer` のいずれかを指定できる。

### 音量 (`--volume` / `MOCA_VOLUME`)

再生音量は `--volume N` (1〜100、既定 100) で下げられる。同じ意味の環境変数
`MOCA_VOLUME` もあり、引数が優先する。範囲外や数字でない値は usage を表示して
exit 2 で終了する。`--help` で usage を表示する。

```sh
moca-listen --volume 30
MOCA_VOLUME=30 moca-listen
```

プレイヤーごとの対応:

| プレイヤー | 対応 | 渡す引数 |
|---|---|---|
| `ffplay` | ○ | `-volume N` |
| `afplay` | ○ | `-v N/100` (0.0〜1.0。線形ではないが N/100 をそのまま渡す) |
| `pw-play` | ○ | `--volume N/100` (0.0〜1.0) |
| `paplay` | ○ | `--volume N*65536/100` (0〜65536) |
| `aplay` | × | 音量の option が無いので warning を 1 回出して 100 で再生する |
| Windows SoundPlayer | × | API に音量が無いので warning を 1 回出して 100 で再生する |
