// 作業タブの立ち絵素材 (公式配布 moca_illust.zip) の自動取得。fallback.rs の
// bep-eng.dic と同じ思想: 再配布禁止のためリポジトリに同梱せず、無ければ起動時に
// 公式サイトから DL してキャッシュする (失敗しても無効化して起動継続 =
// グレースフル劣化。パニックしない)。
//
// bep-eng.dic との違い:
// - zip (約150MB) なので起動をブロックせず tokio::spawn で走らせ、DL は一時
//   ファイルへストリーム、展開は spawn_blocking で行う
// - 配信ルート (MOCA_ASSETS_DIR) へ直接書かない。兄弟のステージングディレクトリ
//   (<dir>.download-<pid>) で DL・展開・検証まで済ませ、最後に rename で
//   アトミックに設置する。途中失敗が「完了済み」に化けたり、展開途中のファイルが
//   /moca-assets から見えたりしない。クラッシュ残骸は次回起動時に掃除する

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 展開済みかの目印。portrait マニフェストの先頭画像と同じパス。
/// ステージングで全件検証してから rename するので、これが在る = 完全に在る。
fn marker(dir: &str) -> PathBuf {
    Path::new(dir).join("moca_illust").join("001.png")
}

/// ステージングディレクトリ。配信ルートの外 (兄弟) + pid 付きで、配信されず
/// 同時起動プロセスとも衝突しない。
fn work_dir(dir: &str) -> PathBuf {
    PathBuf::from(format!("{dir}.download-{}", std::process::id()))
}

// 公式 zip は約150MB / 233ファイル / 展開後約160MB。上限はその数倍を取り、
// URL 差し替えや万一の zip bomb でディスク・CPU を食い潰さないようにする。
const MAX_ZIP_BYTES: u64 = 500 * 1024 * 1024;
const MAX_ENTRIES: usize = 2_000;
const MAX_TOTAL_UNCOMPRESSED: u64 = 1024 * 1024 * 1024;

/// 素材が無ければ DL して展開する。冪等 (既にあれば何もしない)。
/// URL が空なら自動取得は無効 (手動配置した `MOCA_ASSETS_DIR` をそのまま使う)。
pub async fn ensure_illust(dir: String, url: String) {
    cleanup_stale(&dir); // 前回クラッシュのステージング残骸 (150MB) を掃除
    if url.is_empty() {
        tracing::info!("moca illust auto-download disabled (MOCA_ILLUST_URL is empty)");
        return;
    }
    if marker(&dir).exists() {
        tracing::info!("moca illust assets present ({dir})");
        return;
    }
    let result = download_and_extract(&dir, &url).await;
    let _ = std::fs::remove_dir_all(work_dir(&dir)); // 成否によらず自分の作業場を片付ける
    match result {
        Ok(count) => tracing::info!("moca illust downloaded and extracted ({count} files)"),
        Err(e) => tracing::warn!("moca illust download failed (portrait disabled): {e}"),
    }
}

/// `<dir>.download-*` (自分以外の pid 含む) を削除する。ステージングは使い捨てで、
/// 完成物は rename 済みなので、起動時に残っているものは全てゴミ。
fn cleanup_stale(dir: &str) {
    let path = Path::new(dir);
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!("{name}.download-");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(&prefix))
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

async fn download_and_extract(dir: &str, url: &str) -> Result<usize, String> {
    let work = work_dir(dir);
    std::fs::create_dir_all(&work).map_err(|e| format!("create staging dir: {e}"))?;

    // ステージングへストリーム DL (150MB をメモリに乗せない)。上限超過で中断。
    let zip_path = work.join("illust.zip");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let mut res = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request {url}: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("request {url}: HTTP {}", res.status()));
    }
    let mut file = std::fs::File::create(&zip_path).map_err(|e| format!("create tmp: {e}"))?;
    let mut downloaded: u64 = 0;
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| format!("download body: {e}"))?
    {
        downloaded += chunk.len() as u64;
        if downloaded > MAX_ZIP_BYTES {
            return Err(format!("zip exceeds {MAX_ZIP_BYTES} bytes"));
        }
        file.write_all(&chunk)
            .map_err(|e| format!("write tmp: {e}"))?;
    }
    drop(file);

    // 展開もステージング内 (自分が作った新規ディレクトリなので symlink 混入の余地なし)。
    let out = work.join("out");
    let zip_for_task = zip_path.clone();
    let out_for_task = out.clone();
    let count = tokio::task::spawn_blocking(move || extract_zip(&zip_for_task, &out_for_task))
        .await
        .map_err(|e| format!("extract task: {e}"))??;

    // 全件展開できたことを検証してから、配信ルートへアトミックに設置する。
    if !out.join("moca_illust").join("001.png").exists() {
        return Err("extracted zip is missing moca_illust/001.png".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("create assets dir: {e}"))?;
    std::fs::rename(out.join("moca_illust"), Path::new(dir).join("moca_illust"))
        .map_err(|e| format!("move into place: {e}"))?;
    Ok(count)
}

/// zip を out へ展開する。エントリ名は `enclosed_name` でサニタイズし (パス
/// トラバーサル対策)、Thumbs.db だけ捨てる。利用ガイドライン txt は残す。
/// エントリ数と展開後の総バイト数に上限を敷く (zip bomb 対策 — ヘッダの申告
/// サイズではなく実際に書いたバイト数で数える)。
fn extract_zip(zip_path: &Path, out: &Path) -> Result<usize, String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("open zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("read zip: {e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!("zip has too many entries ({})", archive.len()));
    }
    let mut count = 0;
    let mut budget = MAX_TOTAL_UNCOMPRESSED;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip entry {i}: {e}"))?;
        let Some(rel) = entry.enclosed_name() else {
            continue; // 危険なパスはスキップ
        };
        if entry.is_dir() || rel.file_name().is_some_and(|n| n == "Thumbs.db") {
            continue;
        }
        let dest = out.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        }
        let mut file = std::fs::File::create(&dest).map_err(|e| format!("create file: {e}"))?;
        let written = std::io::copy(&mut (&mut entry).take(budget + 1), &mut file)
            .map_err(|e| format!("extract file: {e}"))?;
        if written > budget {
            return Err(format!("zip expands beyond {MAX_TOTAL_UNCOMPRESSED} bytes"));
        }
        budget -= written;
        count += 1;
    }
    if count == 0 {
        return Err("zip contained no files".into());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 最小の zip を組み立てて展開の正常系・サニタイズ・Thumbs.db 除外・上限を確認する。
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, data) in entries {
            w.start_file(*name, zip::write::FileOptions::<()>::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn extract_skips_thumbs_and_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("a.zip");
        std::fs::write(
            &zip_path,
            build_zip(&[
                ("moca_illust/001.png", b"png"),
                ("moca_illust/Thumbs.db", b"junk"),
                ("../evil.txt", b"evil"),
            ]),
        )
        .unwrap();
        let out = tmp.path().join("out");
        let count = extract_zip(&zip_path, &out).unwrap();
        assert_eq!(count, 1);
        assert!(out.join("moca_illust/001.png").exists());
        assert!(!out.join("moca_illust/Thumbs.db").exists());
        assert!(!tmp.path().join("evil.txt").exists());
    }

    #[test]
    fn empty_zip_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("empty.zip");
        std::fs::write(&zip_path, build_zip(&[])).unwrap();
        assert!(extract_zip(&zip_path, &tmp.path().join("out")).is_err());
    }

    #[test]
    fn too_many_entries_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}")).collect();
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
        let zip_path = tmp.path().join("many.zip");
        std::fs::write(&zip_path, build_zip(&entries)).unwrap();
        let err = extract_zip(&zip_path, &tmp.path().join("out")).unwrap_err();
        assert!(err.contains("too many entries"), "{err}");
    }

    #[test]
    fn stale_staging_dirs_are_cleaned() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("assets");
        let stale = tmp.path().join("assets.download-99999");
        std::fs::create_dir_all(&stale).unwrap();
        cleanup_stale(dir.to_str().unwrap());
        assert!(!stale.exists());
    }
}
