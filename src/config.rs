use std::env;

#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub db_path: String,
    /// voicepeak CLI の実行パス (env: VOICEPEAK)。合成時に直接起動する。
    pub voicepeak_path: String,
    /// 合成に使うナレーター名 (env: `MOCA_NARRATOR`)。
    pub narrator: String,
    /// フォールバック辞書 (bep-eng.dic 由来) のローカルキャッシュパス (env: `BEP_DICT_PATH`)。
    pub bep_dict_path: String,
    /// フォールバック辞書の DL 元 URL (env: `BEP_DICT_URL`)。ops が差し替え可能。
    pub bep_dict_url: String,
    /// 作業タブの立ち絵などローカル素材のディレクトリ (env: `MOCA_ASSETS_DIR`)。
    /// /moca-assets で配信する。公式イラストは再配布禁止のためリポジトリに同梱せず、
    /// 無ければ起動時に `MOCA_ILLUST_URL` からバックグラウンド DL して展開する
    /// (bep-eng.dic と同方式。失敗しても立ち絵なしで動く)。
    pub assets_dir: String,
    /// 立ち絵 zip の取得元 (env: `MOCA_ILLUST_URL`)。空文字で自動取得を無効化。
    pub moca_illust_url: String,
    /// /work/talk のサーバ側タイムアウト秒 (env: `WORK_TALK_TIMEOUT_SECS`, 既定 60)。
    /// クライアントは 20 秒で固定セリフに切り替えるが、CLI プロセスの後始末は
    /// サーバ側の timeout + `kill_on_drop` が受け持つ。
    pub work_talk_timeout_secs: u64,
}

// GPLv2 のため辞書そのものは同梱せず起動時に DL する。既定は単体ファイルで配られる
// alkana 派生辞書 (UTF-8 CSV, bep-eng.dic 由来)。ops は Shift_JIS の本家 bep-eng.dic URL に
// 差し替えてもよい (parser がカンマ/空白・UTF-8/Shift_JIS を両対応する)。
const DEFAULT_BEP_DICT_URL: &str =
    "https://raw.githubusercontent.com/uesugi6111/alkana-rs/master/dictionary.csv";

// 公式サイト (https://www.ah-soft.com/moca/) の配布 zip。立ち絵差分 232 枚入り。
const DEFAULT_MOCA_ILLUST_URL: &str = "https://www.ah-soft.com/moca/moca_illust.zip";

impl Config {
    pub fn from_env() -> Self {
        let port = env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3000);

        // 既定: リポジトリ直下の moca.db (相対パスは起動ディレクトリ基準)
        let db_path = env::var("DATABASE_PATH").unwrap_or_else(|_| "./moca.db".to_string());

        let voicepeak_path = env::var("VOICEPEAK").unwrap_or_else(|_| "voicepeak".to_string());
        let narrator = env::var("MOCA_NARRATOR").unwrap_or_else(|_| "Miyamai Moca".to_string());

        // 既定: リポジトリ直下 (db_path と同じ相対既定スタイル)。
        let bep_dict_path =
            env::var("BEP_DICT_PATH").unwrap_or_else(|_| "./bep-eng.dic".to_string());
        let bep_dict_url =
            env::var("BEP_DICT_URL").unwrap_or_else(|_| DEFAULT_BEP_DICT_URL.to_string());

        // 既定: リポジトリ直下 (db_path と同じ相対既定スタイル)。
        let assets_dir =
            env::var("MOCA_ASSETS_DIR").unwrap_or_else(|_| "./moca-assets".to_string());
        let moca_illust_url =
            env::var("MOCA_ILLUST_URL").unwrap_or_else(|_| DEFAULT_MOCA_ILLUST_URL.to_string());

        let work_talk_timeout_secs = env::var("WORK_TALK_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        Self {
            port,
            db_path,
            voicepeak_path,
            narrator,
            bep_dict_path,
            bep_dict_url,
            assets_dir,
            moca_illust_url,
            work_talk_timeout_secs,
        }
    }
}
