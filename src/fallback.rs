// 英単語→カタカナのフォールバック辞書 (Bilingual Emacspeak Project の bep-eng.dic 由来)。
// SQLite 読み替え辞書の後段として、未登録の英単語 ([A-Za-z]+) がスペル読みされる問題を緩和する。
// SQLite 辞書と同じく合成入力の前処理でのみ使い、マスターテキストは決して書き換えない。
// GPLv2 のため辞書ファイルはリポジトリに同梱せず、起動時にキャッシュへ DL する
// (失敗しても無効化して起動継続 = グレースフル劣化。パニックしない)。

use std::collections::HashMap;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

pub struct FallbackDict {
    // key = 小文字化した ASCII 英単語、value = 全角カタカナの読み。
    map: HashMap<String, String>,
}

impl FallbackDict {
    /// 空 (= 無効状態)。DL 失敗時やテストで使う。
    pub fn empty() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 辞書テキストを parse する。区切りはカンマ or 空白で、次の 2 形式を透過的に受ける。
    /// bep-eng.dic (Shift_JIS→デコード済み) の `WORD ｶﾅ 0` は末尾フィールドを無視し、
    /// alkana 派生 CSV (UTF-8) の `word,カナ` はそのまま扱う。
    /// 先頭 2 トークンを 語/読み とし、語が全 ASCII 英字の行だけ採用する。
    /// 読みは NFKC で半角カナ→全角カナへ正規化する (`ｺﾐｯｸ`→`コミック`, `ｶﾞ`→`ガ`)。
    /// 全角カナはそのまま通る。ヘッダ/空行/トークン不足の行はスキップし、重複キーは後勝ち。
    pub fn parse(text: &str) -> Self {
        let mut map = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            // Shift_JIS ヘッダコメント `-*- coding: shift_jis -*-` や空行を捨てる。
            if line.is_empty() || line.contains("-*- coding") {
                continue;
            }
            let mut tokens = line
                .split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter(|t| !t.is_empty());
            let (Some(word), Some(reading)) = (tokens.next(), tokens.next()) else {
                continue;
            };
            if !word.chars().all(|c| c.is_ascii_alphabetic()) {
                continue;
            }
            let reading: String = reading.nfkc().collect();
            map.insert(word.to_ascii_lowercase(), reading);
        }
        Self { map }
    }

    /// text を左→右に 1 パス走査し、`is_ascii_alphabetic` の極大ランをラン単位で
    /// 小文字ルックアップする。ヒットしたら読みを、無ければ元のランを出力。非英字は素通し。
    /// ラン単位・単一パスなので部分一致・カスケードは起きない (`comics` は非置換)。
    pub fn apply(&self, text: &str) -> String {
        if self.map.is_empty() {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut run = String::new();
        for c in text.chars() {
            if c.is_ascii_alphabetic() {
                run.push(c);
            } else {
                self.flush_run(&mut run, &mut out);
                out.push(c);
            }
        }
        self.flush_run(&mut run, &mut out);
        out
    }

    // 蓄積した英字ランをルックアップして out へ流し、run を空にする。
    fn flush_run(&self, run: &mut String, out: &mut String) {
        if run.is_empty() {
            return;
        }
        match self.map.get(&run.to_ascii_lowercase()) {
            Some(reading) => out.push_str(reading),
            None => out.push_str(run),
        }
        run.clear();
    }
}

/// キャッシュがあれば読み、無ければ url から DL してキャッシュへ保存し parse する。
/// ネットワーク/非200/IO/デコードのいずれの失敗も warn して empty() を返す (パニック禁止)。
pub async fn load_or_download(cache_path: &str, url: &str) -> FallbackDict {
    let path = Path::new(cache_path);

    // キャッシュ優先: 存在し非空なら再 DL しない。
    if let Ok(bytes) = std::fs::read(path) {
        if !bytes.is_empty() {
            return FallbackDict::parse(&decode(&bytes));
        }
    }

    let bytes = match download(url).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("fallback dict download failed ({url}): {e}; fallback disabled");
            return FallbackDict::empty();
        }
    };

    // temp へ書いて rename でアトミックにキャッシュへ保存する。
    // 保存に失敗してもメモリ上の辞書は使えるので継続する。
    if let Err(e) = atomic_write(path, &bytes) {
        tracing::warn!("fallback dict cache write failed ({cache_path}): {e}");
    }
    FallbackDict::parse(&decode(&bytes))
}

async fn download(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    Ok(bytes.to_vec())
}

// 同ディレクトリの temp へ書いて rename で置き換える (部分書き込みを見せない)。
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("bep-eng.dic");
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// UTF-8 を試し、不正バイトがあれば Shift_JIS でデコードする。
/// bep-eng.dic は Shift_JIS、alkana 派生 CSV は UTF-8 なので両方を透過的に扱える。
fn decode(bytes: &[u8]) -> String {
    let (text, _, had_errors) = encoding_rs::UTF_8.decode(bytes);
    if !had_errors {
        return text.into_owned();
    }
    let (text, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
    text.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bep_format_normalizes_halfwidth_to_fullwidth() {
        // bep-eng.dic 形式: 3 フィールド・半角カナ。末尾の 0 は無視、半角→全角へ正規化。
        let dict = FallbackDict::parse("COMIC ｺﾐｯｸ 0\n");
        assert_eq!(dict.apply("comic"), "コミック");
    }

    #[test]
    fn parse_skips_coding_header_and_short_lines() {
        let dict = FallbackDict::parse("\t-*- coding: shift_jis -*-\n\nGPU\nCOMIC ｺﾐｯｸ 0\n");
        // ヘッダ行・空行・1 トークンだけの行はスキップ。有効な 1 語のみ採る。
        assert!(!dict.is_empty());
        assert_eq!(dict.apply("gpu"), "gpu"); // 読み欠落行は不採用 → 素通し
        assert_eq!(dict.apply("comic"), "コミック");
    }

    #[test]
    fn parse_composes_dakuten() {
        // 半角カナ + 半角濁点は NFKC で 1 文字に合成される。
        let dict = FallbackDict::parse("GAME ｹﾞｰﾑ 0\n");
        assert_eq!(dict.apply("game"), "ゲーム");
    }

    #[test]
    fn parse_csv_format() {
        // alkana 派生 CSV: カンマ区切り・小文字・全角カナ済み。
        let dict = FallbackDict::parse("comic,コミック\n");
        assert_eq!(dict.apply("comic"), "コミック");
    }

    #[test]
    fn apply_is_case_insensitive() {
        let dict = FallbackDict::parse("COMIC ｺﾐｯｸ 0\n");
        assert_eq!(dict.apply("Comic"), "コミック");
        assert_eq!(dict.apply("COMIC"), "コミック");
        assert_eq!(dict.apply("comic"), "コミック");
    }

    #[test]
    fn apply_does_not_partial_match() {
        // ラン単位・完全一致のみ。複数形 comics は未収録なので素通し。
        let dict = FallbackDict::parse("COMIC ｺﾐｯｸ 0\n");
        assert_eq!(dict.apply("comics"), "comics");
    }

    #[test]
    fn apply_replaces_run_within_mixed_text() {
        let dict = FallbackDict::parse("COMIC ｺﾐｯｸ 0\n");
        assert_eq!(dict.apply("comicを読む"), "コミックを読む");
        // 日本語のみは無変化。
        assert_eq!(dict.apply("漫画を読む"), "漫画を読む");
        // 英字ランの直後に数字が続いてもラン単位で置換する。
        assert_eq!(dict.apply("comic123"), "コミック123");
    }

    #[test]
    fn empty_dict_passes_through() {
        let dict = FallbackDict::empty();
        assert!(dict.is_empty());
        assert_eq!(dict.apply("comic"), "comic");
    }
}
