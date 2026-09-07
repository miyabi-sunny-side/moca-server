// /say エンドポイント: テキスト/台本を合成し音声をチャンク配信する。
// 既定は Ogg/Opus、Accept: audio/wav で可逆 WAV を返す。

use crate::dictionary::{
    apply_dictionary, apply_dictionary_to_segments, load_dictionary, DictEntry,
};
use crate::error::AppError;
use crate::script::validate_script;
use crate::state::AppState;
use crate::synth::{split_sentences, stream_synthesis, OutputFormat, SynthConfig};
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::header::{ACCEPT, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

pub fn routes() -> Router<AppState> {
    Router::new().route("/say", get(say).post(say))
}

async fn say(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, AppError> {
    // POST + application/json → 台本合成、それ以外は文分割の感情なし合成。
    let is_json = method == Method::POST
        && headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("application/json"));

    let segments = if is_json {
        build_script_segments(&state, &body)?
    } else {
        build_plain_segments(&state, &method, &query, &body)?
    };

    // 既定は Ogg/Opus (帯域 1/12)。Accept: audio/wav は WAV 経路 (動画素材用の可逆出力)。
    let wants_wav = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("audio/wav"));
    let format = if wants_wav {
        OutputFormat::Wav
    } else {
        OutputFormat::Opus
    };

    Ok(stream_response(&state, segments, format))
}

// 辞書エントリを読む。DB ロックは .await をまたがせない (Send 制約) ため関数内で完結させる。
fn load_entries(state: &AppState) -> Result<Vec<DictEntry>, AppError> {
    let conn = state.db.lock().unwrap();
    Ok(load_dictionary(&conn)?)
}

// application/json 経路: validate_script を通し、各 segment.text に辞書を適用する。
fn build_script_segments(state: &AppState, body: &Bytes) -> Result<Vec<Value>, AppError> {
    let parsed: Value = serde_json::from_slice(body)
        .map_err(|_| AppError::BadRequest("invalid JSON body".into()))?;
    let script = validate_script(&parsed)?;
    let entries = load_entries(state)?;
    let array = script.as_array().expect("validate_script returns an array");
    // SQLite 辞書を先に消費し、その後段でフォールバック辞書を重ねる。
    Ok(apply_dictionary_to_segments(
        array,
        &entries,
        &state.fallback,
    ))
}

// text/plain 経路: 文分割してから 1 セグメント列にする。?raw=1 は辞書スキップ。
fn build_plain_segments(
    state: &AppState,
    method: &Method,
    query: &HashMap<String, String>,
    body: &Bytes,
) -> Result<Vec<Value>, AppError> {
    let text = if method == Method::POST {
        String::from_utf8_lossy(body).into_owned()
    } else {
        query.get("text").cloned().unwrap_or_default()
    };
    if text.trim().is_empty() {
        return Err(AppError::BadRequest("text required".into()));
    }

    // ?raw=1 は辞書をスキップする: 辞書プレビューは読み文字列をそのまま読み上げるので、
    // そこへ辞書を適用すると二重置換になりうるため。
    let raw = query.get("raw").is_some_and(|v| !v.is_empty());
    let processed = if raw {
        text
    } else {
        // SQLite 辞書 → フォールバック辞書の順で適用してから文分割する。
        state
            .fallback
            .apply(&apply_dictionary(&text, &load_entries(state)?))
    };

    let segments = split_sentences(&processed)
        .into_iter()
        .map(|s| json!({ "text": s }))
        .collect();
    Ok(segments)
}

// 合成タスクを spawn し、mpsc を Body::from_stream に繋いで即レスポンスを返す。
// audio.wav (可逆 WAV 固定) からも同じ経路を通すため兄弟モジュールへ公開する。
pub(super) fn stream_response(
    state: &AppState,
    segments: Vec<Value>,
    format: OutputFormat,
) -> Response {
    let synth = Arc::clone(&state.synth);
    let cfg = SynthConfig {
        voicepeak_path: state.config.voicepeak_path.clone(),
        narrator: state.config.narrator.clone(),
    };

    let content_type = match format {
        OutputFormat::Wav => "audio/wav",
        OutputFormat::Opus => "audio/ogg",
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    tokio::spawn(stream_synthesis(synth, cfg, segments, format, tx));

    let mut response = Response::new(Body::from_stream(ReceiverStream::new(rx)));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::synth::SynthQueue;
    use crate::wav::{wav_header, MOCA_FORMAT};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use rusqlite::Connection;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use tower::ServiceExt;

    // 固定小 WAV (44 byte ヘッダ + 200 byte PCM)。data サイズは正しく埋める。
    const PCM_LEN: usize = 200;

    fn fixture_wav() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + PCM_LEN) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // channels
        b.extend_from_slice(&48000u32.to_le_bytes());
        b.extend_from_slice(&96000u32.to_le_bytes()); // byte rate
        b.extend_from_slice(&2u16.to_le_bytes()); // block align
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(PCM_LEN as u32).to_le_bytes());
        b.extend_from_slice(&[7u8; PCM_LEN]);
        b
    }

    // 本物 voicepeak は起動しない (1 プロセス制限で実機開発サーバと衝突する)。
    // -o のパスへ固定 WAV を書き、-s と呼び出し回数を記録するフェイクを作る。
    struct Fake {
        _dir: TempDir,
        bin: PathBuf,
        record: PathBuf,
        count: PathBuf,
    }

    fn make_fake(sleep_secs: f64) -> Fake {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture.wav");
        std::fs::write(&fixture, fixture_wav()).unwrap();
        let record = dir.path().join("record.txt");
        let count = dir.path().join("count.txt");
        let bin = dir.path().join("fake-voicepeak");

        let sleep_line = if sleep_secs > 0.0 {
            format!("sleep {sleep_secs}\n")
        } else {
            String::new()
        };
        let script = format!(
            "#!/usr/bin/env bash\n\
             out=\"\"\n\
             text=\"\"\n\
             while [ $# -gt 0 ]; do\n\
             \x20 case \"$1\" in\n\
             \x20   -o) out=\"$2\"; shift 2;;\n\
             \x20   -s) text=\"$2\"; shift 2;;\n\
             \x20   *) shift;;\n\
             \x20 esac\n\
             done\n\
             printf '%s\\n' \"$text\" >> \"{record}\"\n\
             printf 'x' >> \"{count}\"\n\
             {sleep_line}\
             cp \"{fixture}\" \"$out\"\n",
            record = record.display(),
            count = count.display(),
            fixture = fixture.display(),
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        Fake {
            _dir: dir,
            bin,
            record,
            count,
        }
    }

    fn state_with(fake: &Fake) -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn);
        let (notify, _) = tokio::sync::broadcast::channel::<String>(16);
        AppState {
            db: Arc::new(Mutex::new(conn)),
            config: Config {
                port: 0,
                db_path: ":memory:".into(),
                voicepeak_path: fake.bin.display().to_string(),
                narrator: "Test".into(),
                bep_dict_path: "./bep-eng.dic".into(),
                bep_dict_url: String::new(),
                work_talk_timeout_secs: 60,
                assets_dir: "./target/test-missing-assets".into(),
                moca_illust_url: String::new(),
            },
            synth: Arc::new(SynthQueue::new()),
            analyzer: Arc::new(crate::analyze::Backend::None),
            news_analyzer: None,
            fallback: Arc::new(crate::fallback::FallbackDict::empty()),
            notify,
        }
    }

    // fallback 辞書を注入した state を作る (優先順位・後段適用の検証用)。
    fn state_with_fallback(fake: &Fake, dict: &str) -> AppState {
        let mut state = state_with(fake);
        state.fallback = Arc::new(crate::fallback::FallbackDict::parse(dict));
        state
    }

    fn app_with(fake: &Fake) -> Router {
        routes().with_state(state_with(fake))
    }

    fn insert_dict(state: &AppState, surface: &str, reading: &str) {
        let conn = state.db.lock().unwrap();
        conn.execute(
            "INSERT INTO dictionary (surface, reading, created_at) VALUES (?1, ?2, 'now')",
            [surface, reading],
        )
        .unwrap();
    }

    async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, bytes.to_vec())
    }

    fn post_text(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "text/plain")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    // Accept: audio/wav 版。WAV 経路を検証する (バイト長が決定的)。
    fn post_text_wav(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "text/plain")
            .header("accept", "audio/wav")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn post_json_wav(uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .header("accept", "audio/wav")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn post_json(uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn header_prefix() -> Vec<u8> {
        wav_header(&MOCA_FORMAT)
    }

    #[tokio::test]
    async fn post_text_default_is_ogg_opus() {
        let fake = make_fake(0.0);
        let app = app_with(&fake);
        let res = app
            .clone()
            .oneshot(post_text("/say", "こんにちは。"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get(CONTENT_TYPE).unwrap(), "audio/ogg");
        assert_eq!(res.headers().get(CACHE_CONTROL).unwrap(), "no-store");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        // Ogg マジックで始まる (OpusHead ページ)。
        assert_eq!(&body[0..4], b"OggS");
    }

    #[tokio::test]
    async fn accept_wav_returns_riff() {
        let fake = make_fake(0.0);
        let app = app_with(&fake);
        let res = app
            .clone()
            .oneshot(post_text_wav("/say", "こんにちは。"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get(CONTENT_TYPE).unwrap(), "audio/wav");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(body.starts_with(&header_prefix()));
        // ヘッダ + 1 セグメント分の PCM (WAV 経路はバイト長が決定的)。
        assert_eq!(body.len(), 44 + PCM_LEN);
    }

    #[tokio::test]
    async fn get_with_text_query_default_is_ogg_opus() {
        let fake = make_fake(0.0);
        let app = app_with(&fake);
        let (status, body) = send(&app, get("/say?text=%E3%81%82")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[0..4], b"OggS");
    }

    #[tokio::test]
    async fn empty_text_is_400() {
        let fake = make_fake(0.0);
        let app = app_with(&fake);
        let (status, _) = send(&app, post_text("/say", "   ")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = send(&app, get("/say")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn broken_json_is_400() {
        let fake = make_fake(0.0);
        let app = app_with(&fake);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/say")
            .header("content-type", "application/json")
            .body(Body::from("{not json"))
            .unwrap();
        let (status, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn empty_array_is_400() {
        let fake = make_fake(0.0);
        let app = app_with(&fake);
        let (status, _) = send(&app, post_json("/say", json!([]))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn dictionary_applies_to_plain_text() {
        let fake = make_fake(0.0);
        let state = state_with(&fake);
        insert_dict(&state, "GPU", "ジーピーユー");
        let app = routes().with_state(state);
        send(&app, post_text("/say", "このGPU")).await;
        let recorded = std::fs::read_to_string(&fake.record).unwrap();
        assert!(
            recorded.contains("このジーピーユー"),
            "recorded: {recorded}"
        );
    }

    #[tokio::test]
    async fn raw_query_skips_dictionary() {
        let fake = make_fake(0.0);
        let state = state_with(&fake);
        insert_dict(&state, "GPU", "ジーピーユー");
        let app = routes().with_state(state);
        send(&app, post_text("/say?raw=1", "このGPU")).await;
        let recorded = std::fs::read_to_string(&fake.record).unwrap();
        assert!(recorded.contains("このGPU"), "recorded: {recorded}");
        assert!(!recorded.contains("ジーピーユー"));
    }

    #[tokio::test]
    async fn fallback_applies_to_plain_text() {
        let fake = make_fake(0.0);
        let state = state_with_fallback(&fake, "COMIC ｺﾐｯｸ 0\n");
        let app = routes().with_state(state);
        send(&app, post_text("/say", "comic")).await;
        let recorded = std::fs::read_to_string(&fake.record).unwrap();
        assert!(recorded.contains("コミック"), "recorded: {recorded}");
    }

    #[tokio::test]
    async fn sqlite_dictionary_takes_priority_over_fallback() {
        // SQLite 辞書が先に COMIC を消費するので fallback は発火しない。
        let fake = make_fake(0.0);
        let state = state_with_fallback(&fake, "COMIC ｺﾐｯｸ 0\n");
        insert_dict(&state, "COMIC", "まんが");
        let app = routes().with_state(state);
        send(&app, post_text("/say", "comic")).await;
        let recorded = std::fs::read_to_string(&fake.record).unwrap();
        assert!(recorded.contains("まんが"), "recorded: {recorded}");
        assert!(!recorded.contains("コミック"));
    }

    #[tokio::test]
    async fn raw_query_skips_fallback() {
        let fake = make_fake(0.0);
        let state = state_with_fallback(&fake, "COMIC ｺﾐｯｸ 0\n");
        let app = routes().with_state(state);
        send(&app, post_text("/say?raw=1", "comic")).await;
        let recorded = std::fs::read_to_string(&fake.record).unwrap();
        assert!(recorded.contains("comic"), "recorded: {recorded}");
        assert!(!recorded.contains("コミック"));
    }

    #[tokio::test]
    async fn fallback_applies_to_script_segments() {
        let fake = make_fake(0.0);
        let state = state_with_fallback(&fake, "COMIC ｺﾐｯｸ 0\n");
        let app = routes().with_state(state);
        let script = json!([{ "text": "comic。" }]);
        send(&app, post_json_wav("/say", script)).await;
        let recorded = std::fs::read_to_string(&fake.record).unwrap();
        assert!(recorded.contains("コミック。"), "recorded: {recorded}");
    }

    #[tokio::test]
    async fn json_calls_voicepeak_per_segment() {
        // WAV 経路 (Accept: audio/wav) でバイト長を検証する (Opus はサイズ非決定的)。
        let fake = make_fake(0.0);
        let app = app_with(&fake);
        let script = json!([{ "text": "一。" }, { "text": "二。" }, { "text": "三。" }]);
        let (status, body) = send(&app, post_json_wav("/say", script)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with(&header_prefix()));
        let count = std::fs::read_to_string(&fake.count).unwrap();
        assert_eq!(count.len(), 3);
        assert_eq!(body.len(), 44 + PCM_LEN * 3);
    }

    #[tokio::test]
    async fn pause_adds_silence() {
        // WAV 経路でバイト長を検証する。
        let fake = make_fake(0.0);
        let app = app_with(&fake);
        let script = json!([{ "text": "一。", "pause": 100 }]);
        let (status, body) = send(&app, post_json_wav("/say", script)).await;
        assert_eq!(status, StatusCode::OK);
        // ヘッダ + PCM + pause(100ms * 96)。
        assert_eq!(body.len(), 44 + PCM_LEN + 100 * 96);
    }

    #[tokio::test]
    async fn concurrent_requests_are_serialized() {
        let fake = make_fake(0.2); // 各セグメント 200ms
        let app = app_with(&fake);
        let a = app.clone();
        let b = app.clone();
        let start = std::time::Instant::now();
        let (ra, rb) = tokio::join!(async { send(&a, post_text("/say", "A。")).await }, async {
            send(&b, post_text("/say", "B。")).await
        },);
        let elapsed = start.elapsed();
        assert_eq!(ra.0, StatusCode::OK);
        assert_eq!(rb.0, StatusCode::OK);
        // 直列化されるので 2 リクエストで合計 >= ~2 * 200ms。並列なら ~200ms で済む。
        assert!(
            elapsed.as_millis() >= 350,
            "expected serialized (>=350ms), got {elapsed:?}"
        );
    }
}
