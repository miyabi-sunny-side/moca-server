// 管理画面 SPA の index.html を返す。ビルド成果物を verbatim で返す (書き換えなし)。

use std::fs;
use std::path::Path;
use std::sync::Mutex;

static CACHED_HTML: Mutex<Option<(String, u64)>> = Mutex::new(None);

const INDEX_PATH: &str = "client/build/index.html";

fn read_index_html(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn mtime_ms(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    u64::try_from(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis(),
    )
    .ok()
}

// ビルド前は None (fallback が 404 を返す)。dev では mtime が変わると再読込する。
// NODE_ENV=production は初回キャッシュを固定する。
pub fn get_index_html() -> Option<String> {
    let path = Path::new(INDEX_PATH);
    let is_prod = std::env::var("NODE_ENV").is_ok_and(|v| v == "production");

    // cache は都度上書きできるので、poison されても中身をそのまま使う。
    let mut cached = CACHED_HTML
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if is_prod {
        if let Some((html, _)) = cached.as_ref() {
            return Some(html.clone());
        }
    }

    let mtime = mtime_ms(path)?;
    if let Some((html, cached_mtime)) = cached.as_ref() {
        if *cached_mtime == mtime {
            return Some(html.clone());
        }
    }

    let html = read_index_html(path)?;
    *cached = Some((html.clone(), mtime));
    Some(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_for_missing_file() {
        assert!(read_index_html(Path::new("/no/such/index.html")).is_none());
    }

    #[test]
    fn reads_existing_file() {
        let dir = std::env::temp_dir().join("moca_spa_test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.html");
        fs::write(&path, "<!doctype html><title>宮舞モカ</title>").unwrap();
        let html = read_index_html(&path).unwrap();
        assert!(html.contains("宮舞モカ"));
    }
}
