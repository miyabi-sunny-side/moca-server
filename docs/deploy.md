# 常駐化 (systemd)

**Docker での運用は非対応** (実測: VOICEPEAK はコンテナ内でのみ合成を拒否する。
ライブラリ・デバイス・環境変数を揃えても再現し、コンテナ検知と推定)。
サーバーはネイティブバイナリを systemd (user scope) で常駐させる:

```ini
# ~/.config/systemd/user/moca-server.service
[Unit]
Description=moca-server
After=network.target

[Service]
Type=simple
ExecStart=%h/.local/bin/moca-server
WorkingDirectory=%h/.local/share/moca-server
EnvironmentFile=%h/.config/moca-server/.env
Restart=on-failure

[Install]
WantedBy=default.target
```

- バイナリは `~/.local/bin/moca-server`、SPA 成果物 (`client-build.tar.gz` を展開した
  `client/build`) は `WorkingDirectory` 配下に置く
- `.env` (EnvironmentFile) に PORT / DATABASE_PATH / VOICEPEAK 等を書く ([環境変数一覧](../README.md#環境変数))
- `systemctl --user enable --now moca-server` で常駐開始

home-server 配布を利用する場合の unit・設定保存先は
[home-server の systemd 手順](https://github.com/miyabisun/home-server/blob/main/systemd/README.md)
が所有する。既存の待受ポートを維持する場合はサービス専用の
`~/.config/moca-server/.env` に `PORT` を明示する。アプリと updater の未設定時は `3000`。

## 自動更新

GitHub Releases の `releases.atom` を systemd timer で監視して
新タグを検出 → SHA256 検証 → `install(1)` で差し替え → restart する構成が組める。
リリース CI との競合 (Release 作成直後で成果物が未アップロード) は「静かに次回リトライ」で
吸収し、差し替え後・restart 前に `POST /notify` で購読中のブラウザへ
「リロードしてください」と音声案内するとよい。
