mod analyze;
mod audio;
mod dictionary;
mod lines;
mod notify;
mod projects;
mod say;
mod work;

use crate::error::AppError;
use crate::spa;
use crate::state::AppState;
use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

/// 全ハンドラ共通の戻り値型。ステータスコードを明示して契約互換にする。
pub type JsonResult = Result<(StatusCode, Json<Value>), AppError>;

/// パスパラメータの id を手動 parse する (axum の i64 抽出は別レスポンスを返すため)。
/// TS の `Number(param)` → NaN で 400 と同じ挙動。
pub fn parse_id(s: &str) -> Result<i64, AppError> {
    s.parse::<i64>()
        .map_err(|_| AppError::BadRequest("invalid id".into()))
}

/// リクエストボディを serde_json::Value として読む。TS の
/// `await c.req.json().catch(() => ({}))` と同じく失敗時は空オブジェクトにフォールバック。
pub fn parse_body(bytes: &Bytes) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|_| json!({}))
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .merge(projects::routes())
        .merge(lines::routes())
        .merge(dictionary::routes())
        .merge(say::routes())
        .merge(audio::routes())
        .merge(analyze::routes())
        .merge(notify::routes())
        .merge(work::routes())
        .nest_service("/assets", ServeDir::new("client/build/assets"))
        // 作業タブの立ち絵素材 (再配布禁止のため非同梱、起動時に自動 DL — src/assets.rs)。
        // ディレクトリが未取得・DL 失敗でも ServeDir が 404 を返すだけで、SPA
        // フォールバックには流れない (index.html を 200 で返さない)。
        .nest_service("/moca-assets", ServeDir::new(&state.config.assets_dir))
        .fallback_service(get(spa_fallback))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// 非 API GET のフォールバック: SPA の index.html を返す。未ビルドなら 404 JSON。
async fn spa_fallback() -> Response {
    match spa::get_index_html() {
        Some(html) => Html(html).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Frontend not built. Run: cd client && bun install && bun run build" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use http_body_util::BodyExt;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    fn app() -> Router {
        app_with_analyzer(crate::analyze::Backend::None)
    }

    fn app_with_analyzer(analyzer: crate::analyze::Backend) -> Router {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn);
        let (notify, _) = tokio::sync::broadcast::channel::<String>(16);
        let state = AppState {
            db: Arc::new(Mutex::new(conn)),
            config: Config {
                port: 0,
                db_path: ":memory:".into(),
                voicepeak_path: "voicepeak".into(),
                narrator: "Miyamai Moca".into(),
                bep_dict_path: "./bep-eng.dic".into(),
                bep_dict_url: String::new(),
                work_talk_timeout_secs: 60,
                assets_dir: "./target/test-missing-assets".into(),
                moca_illust_url: String::new(),
            },
            synth: Arc::new(crate::synth::SynthQueue::new()),
            analyzer: Arc::new(analyzer),
            news_analyzer: None,
            fallback: Arc::new(crate::fallback::FallbackDict::empty()),
            notify,
        };
        build_router(state)
    }

    // (status, json body) を返すリクエストヘルパ。
    async fn req(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        let request = if let Some(b) = body {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(b.to_string())).unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let res = app.clone().oneshot(request).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    async fn create_project(app: &Router, name: &str) -> Value {
        let (status, body) = req(
            app,
            Method::POST,
            "/api/projects",
            Some(json!({ "name": name })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        body
    }

    async fn add_line(app: &Router, project_id: i64, body: Value) -> (StatusCode, Value) {
        req(
            app,
            Method::POST,
            &format!("/api/projects/{project_id}/lines"),
            Some(body),
        )
        .await
    }

    // --- projects ---

    #[tokio::test]
    async fn project_create_list_get() {
        let app = app();
        let created = create_project(&app, "物語A").await;
        assert!(created["id"].as_i64().unwrap() > 0);
        assert_eq!(created["name"], "物語A");
        assert!(created["created_at"].as_str().unwrap().len() >= 10);

        let (_, list) = req(&app, Method::GET, "/api/projects", None).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["lineCount"], 0);
        assert_eq!(list[0]["name"], "物語A");

        let id = created["id"].as_i64().unwrap();
        let (_, single) = req(&app, Method::GET, &format!("/api/projects/{id}"), None).await;
        assert_eq!(single["name"], "物語A");
        assert_eq!(single["lines"], json!([]));
    }

    #[tokio::test]
    async fn project_name_required() {
        let app = app();
        let (status, _) = req(
            &app,
            Method::POST,
            "/api/projects",
            Some(json!({ "name": "   " })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn project_rename() {
        let app = app();
        let created = create_project(&app, "旧名").await;
        let id = created["id"].as_i64().unwrap();
        let (status, updated) = req(
            &app,
            Method::PATCH,
            &format!("/api/projects/{id}"),
            Some(json!({ "name": "新名" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["name"], "新名");
    }

    #[tokio::test]
    async fn project_missing_is_404() {
        let app = app();
        let (status, _) = req(&app, Method::GET, "/api/projects/9999", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn project_invalid_id_is_400() {
        let app = app();
        let (status, body) = req(&app, Method::GET, "/api/projects/abc", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid id");
    }

    #[tokio::test]
    async fn project_delete_cascades_lines() {
        let app = app();
        let created = create_project(&app, "消す").await;
        let id = created["id"].as_i64().unwrap();
        add_line(&app, id, json!({ "mode": "announcer", "text": "行1" })).await;
        let (status, body) = req(&app, Method::DELETE, &format!("/api/projects/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        let (status, _) = req(&app, Method::GET, &format!("/api/projects/{id}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // --- lines ---

    #[tokio::test]
    async fn line_add_tail_and_script_structured() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (_, a) = add_line(&app, id, json!({ "mode": "announcer", "text": "いち" })).await;
        let (_, b) = add_line(&app, id, json!({ "mode": "announcer", "text": "に" })).await;
        assert_eq!(a["position"], 0);
        assert_eq!(b["position"], 1);
        assert_eq!(a["script"], Value::Null);

        let script = json!([{ "text": "こんにちは。", "emotion": { "honwaka": 60 } }]);
        let (status, c) = add_line(
            &app,
            id,
            json!({ "mode": "acting", "text": "さん", "script": script }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(c["mode"], "acting");
        assert_eq!(c["script"], script);
    }

    #[tokio::test]
    async fn line_text_required() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (status, _) = add_line(&app, id, json!({ "mode": "announcer", "text": "  " })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn line_announcer_ignores_script() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let script = json!([{ "text": "やあ。" }]);
        let (_, row) = add_line(
            &app,
            id,
            json!({ "mode": "announcer", "text": "x", "script": script }),
        )
        .await;
        assert_eq!(row["script"], Value::Null);
    }

    #[tokio::test]
    async fn line_patch_invalid_script_400() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (_, line) = add_line(&app, id, json!({ "mode": "announcer", "text": "x" })).await;
        let lid = line["id"].as_i64().unwrap();
        let (s1, _) = req(
            &app,
            Method::PATCH,
            &format!("/api/lines/{lid}"),
            Some(json!({ "script": [] })),
        )
        .await;
        assert_eq!(s1, StatusCode::BAD_REQUEST);
        let (s2, _) = req(
            &app,
            Method::PATCH,
            &format!("/api/lines/{lid}"),
            Some(json!({ "script": [{ "emotion": { "angry": 1 } }] })),
        )
        .await;
        assert_eq!(s2, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn line_patch_valid_script_saved() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (_, line) = add_line(&app, id, json!({ "mode": "announcer", "text": "x" })).await;
        let lid = line["id"].as_i64().unwrap();
        let script = json!([{ "text": "やあ。", "emotion": { "angry": 80 } }]);
        let (status, updated) = req(
            &app,
            Method::PATCH,
            &format!("/api/lines/{lid}"),
            Some(json!({ "mode": "acting", "script": script })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["mode"], "acting");
        assert_eq!(updated["script"], script);
    }

    #[tokio::test]
    async fn line_patch_null_script_clears() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let script = json!([{ "text": "やあ。" }]);
        let (_, line) = add_line(
            &app,
            id,
            json!({ "mode": "acting", "text": "x", "script": script }),
        )
        .await;
        let lid = line["id"].as_i64().unwrap();
        let (status, updated) = req(
            &app,
            Method::PATCH,
            &format!("/api/lines/{lid}"),
            Some(json!({ "script": Value::Null })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["script"], Value::Null);
    }

    #[tokio::test]
    async fn line_delete() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (_, line) = add_line(&app, id, json!({ "mode": "announcer", "text": "x" })).await;
        let lid = line["id"].as_i64().unwrap();
        let (status, _) = req(&app, Method::DELETE, &format!("/api/lines/{lid}"), None).await;
        assert_eq!(status, StatusCode::OK);
        let (_, single) = req(&app, Method::GET, &format!("/api/projects/{id}"), None).await;
        assert_eq!(single["lines"], json!([]));
    }

    #[tokio::test]
    async fn line_reorder() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (_, a) = add_line(&app, id, json!({ "mode": "announcer", "text": "A" })).await;
        let (_, b) = add_line(&app, id, json!({ "mode": "announcer", "text": "B" })).await;
        let (_, c) = add_line(&app, id, json!({ "mode": "announcer", "text": "C" })).await;
        let order = json!({ "order": [c["id"], a["id"], b["id"]] });
        let (status, reordered) = req(
            &app,
            Method::PUT,
            &format!("/api/projects/{id}/lines/order"),
            Some(order),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let texts: Vec<&str> = reordered
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["C", "A", "B"]);
        let positions: Vec<i64> = reordered
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["position"].as_i64().unwrap())
            .collect();
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn line_reorder_requires_each_id_once() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (_, a) = add_line(&app, id, json!({ "text": "A" })).await;
        let (_, b) = add_line(&app, id, json!({ "text": "B" })).await;
        let project_uri = format!("/api/projects/{id}");
        let order_uri = format!("{project_uri}/lines/order");
        let (_, original) = req(&app, Method::GET, &project_uri, None).await;

        for order in [
            json!([a["id"], a["id"]]),
            json!([a["id"], format!(" {}.0 ", a["id"])]),
            json!([a["id"]]),
            json!([a["id"], 99999]),
        ] {
            let (status, error) = req(
                &app,
                Method::PUT,
                &order_uri,
                Some(json!({ "order": order })),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "order: {order}, body: {error}"
            );
            assert_eq!(
                error,
                json!({ "error": "order must contain exactly the ids of this project" })
            );
            let (status, unchanged) = req(&app, Method::GET, &project_uri, None).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(unchanged, original, "order: {order}");
        }

        for (order, mut expected) in [
            (json!([b["id"], a["id"]]), json!([b, a])),
            (
                json!([format!(" {}.0 ", a["id"]), b["id"].to_string()]),
                json!([a, b]),
            ),
        ] {
            let (status, reordered) = req(
                &app,
                Method::PUT,
                &order_uri,
                Some(json!({ "order": order })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            expected[0]["position"] = json!(0);
            expected[1]["position"] = json!(1);
            assert_eq!(reordered, expected);
            let (status, saved) = req(&app, Method::GET, &project_uri, None).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(saved["lines"], expected);
        }
    }

    #[tokio::test]
    async fn line_reorder_foreign_id_400() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (_, a) = add_line(&app, id, json!({ "mode": "announcer", "text": "A" })).await;
        let order = json!({ "order": [a["id"], 99999] });
        let (status, _) = req(
            &app,
            Method::PUT,
            &format!("/api/projects/{id}/lines/order"),
            Some(order),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn line_reorder_non_array_400() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let (status, _) = req(
            &app,
            Method::PUT,
            &format!("/api/projects/{id}/lines/order"),
            Some(json!({ "order": "nope" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn line_duplicate_inserts_below() {
        let app = app();
        let p = create_project(&app, "テスト").await;
        let id = p["id"].as_i64().unwrap();
        let script = json!([{ "text": "やあ。", "emotion": { "angry": 80 } }]);
        let (_, a) = add_line(
            &app,
            id,
            json!({ "mode": "acting", "text": "A", "script": script }),
        )
        .await;
        add_line(&app, id, json!({ "mode": "announcer", "text": "B" })).await;
        let lid = a["id"].as_i64().unwrap();
        let (status, dup) = req(
            &app,
            Method::POST,
            &format!("/api/lines/{lid}/duplicate"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(dup["text"], "A");
        assert_eq!(dup["mode"], "acting");
        assert_eq!(dup["script"], script);
        assert_eq!(dup["position"], 1);

        let (_, single) = req(&app, Method::GET, &format!("/api/projects/{id}"), None).await;
        let texts: Vec<&str> = single["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["A", "A", "B"]);
        let positions: Vec<i64> = single["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["position"].as_i64().unwrap())
            .collect();
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn line_duplicate_missing_404() {
        let app = app();
        let (status, _) = req(&app, Method::POST, "/api/lines/99999/duplicate", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // --- import ---

    #[tokio::test]
    async fn import_announcer_multiline_skips_blanks() {
        let app = app();
        let p = create_project(&app, "import").await;
        let id = p["id"].as_i64().unwrap();
        let (status, body) = req(
            &app,
            Method::POST,
            &format!("/api/projects/{id}/import"),
            Some(json!({ "mode": "announcer", "text": "一行目\n\n  \n二行目\n三行目" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["mode"], "announcer");
        let texts: Vec<&str> = body["created"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["一行目", "二行目", "三行目"]);
        assert!(body["created"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["script"] == Value::Null));
        let positions: Vec<i64> = body["created"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["position"].as_i64().unwrap())
            .collect();
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn import_after_shifts_following() {
        let app = app();
        let p = create_project(&app, "import").await;
        let id = p["id"].as_i64().unwrap();
        let (_, base) = req(
            &app,
            Method::POST,
            &format!("/api/projects/{id}/import"),
            Some(json!({ "mode": "announcer", "text": "A\nB\nC" })),
        )
        .await;
        let after_id = base["created"][0]["id"].clone();
        let (_, inserted) = req(
            &app,
            Method::POST,
            &format!("/api/projects/{id}/import"),
            Some(json!({ "mode": "announcer", "text": "X\nY", "after": after_id })),
        )
        .await;
        let positions: Vec<i64> = inserted["created"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["position"].as_i64().unwrap())
            .collect();
        assert_eq!(positions, vec![1, 2]);

        let (_, single) = req(&app, Method::GET, &format!("/api/projects/{id}"), None).await;
        let texts: Vec<&str> = single["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["A", "X", "Y", "B", "C"]);
        let positions: Vec<i64> = single["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["position"].as_i64().unwrap())
            .collect();
        assert_eq!(positions, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn import_after_foreign_falls_back_to_tail() {
        let app = app();
        let p = create_project(&app, "import").await;
        let id = p["id"].as_i64().unwrap();
        req(
            &app,
            Method::POST,
            &format!("/api/projects/{id}/import"),
            Some(json!({ "mode": "announcer", "text": "A\nB" })),
        )
        .await;
        let (_, inserted) = req(
            &app,
            Method::POST,
            &format!("/api/projects/{id}/import"),
            Some(json!({ "mode": "announcer", "text": "Z", "after": 99999 })),
        )
        .await;
        assert_eq!(inserted["created"][0]["position"], 2);
    }

    #[tokio::test]
    async fn import_acting_none_backend_falls_back_to_announcer() {
        // 既定 app() の analyzer は Backend::None なので全行が分析失敗 → announcer 保存。
        let app = app();
        let p = create_project(&app, "import").await;
        let id = p["id"].as_i64().unwrap();
        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/projects/{id}/import"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "mode": "acting", "text": "文一\n文二" }).to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("text/event-stream"));
        let body = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec())
            .unwrap();
        assert!(body.contains("\"status\":\"failed\""), "body: {body}");

        // 失敗行も announcer として保存され、元テキストが残る。
        let (_, single) = req_get(&app, id).await;
        let lines = single["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .all(|l| l["mode"] == "announcer" && l["script"] == Value::Null));
        assert_eq!(lines[0]["text"], "文一");
    }

    async fn req_get(app: &Router, project_id: i64) -> (StatusCode, Value) {
        req(
            app,
            Method::GET,
            &format!("/api/projects/{project_id}"),
            None,
        )
        .await
    }

    #[tokio::test]
    async fn import_empty_returns_empty() {
        let app = app();
        let p = create_project(&app, "import").await;
        let id = p["id"].as_i64().unwrap();
        let (status, body) = req(
            &app,
            Method::POST,
            &format!("/api/projects/{id}/import"),
            Some(json!({ "mode": "announcer", "text": "  \n\n" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "mode": "announcer", "created": [] }));
    }

    // --- dictionary ---

    #[tokio::test]
    async fn dictionary_list_sorted_by_surface() {
        let app = app();
        req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "GPU", "reading": "ジーピーユー" })),
        )
        .await;
        req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "API", "reading": "エーピーアイ" })),
        )
        .await;
        let (_, rows) = req(&app, Method::GET, "/api/dictionary", None).await;
        let surfaces: Vec<&str> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["surface"].as_str().unwrap())
            .collect();
        assert_eq!(surfaces, vec!["API", "GPU"]);
    }

    #[tokio::test]
    async fn dictionary_empty_surface_reading_400() {
        let app = app();
        let (s1, _) = req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "  ", "reading": "よみ" })),
        )
        .await;
        assert_eq!(s1, StatusCode::BAD_REQUEST);
        let (s2, _) = req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "表記", "reading": "" })),
        )
        .await;
        assert_eq!(s2, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn dictionary_upsert_overwrites() {
        let app = app();
        let (_, first) = req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "ハード", "reading": "はーど" })),
        )
        .await;
        let (status, second) = req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "ハード", "reading": "かたい" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(second["id"], first["id"]);
        let (_, rows) = req(&app, Method::GET, "/api/dictionary", None).await;
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["reading"], "かたい");
    }

    #[tokio::test]
    async fn dictionary_patch_updates_fields_and_rejects_conflicts() {
        let app = app();
        let (_, first) = req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "API", "reading": "エーピーアイ" })),
        )
        .await;
        req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "GPU", "reading": "ジーピーユー" })),
        )
        .await;
        let id = first["id"].as_i64().unwrap();

        let (status, updated) = req(
            &app,
            Method::PATCH,
            &format!("/api/dictionary/{id}"),
            Some(json!({ "surface": "CPU", "reading": "シーピーユー" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["id"], id);
        assert_eq!(updated["surface"], "CPU");
        assert_eq!(updated["reading"], "シーピーユー");

        let (conflict, body) = req(
            &app,
            Method::PATCH,
            &format!("/api/dictionary/{id}"),
            Some(json!({ "surface": "GPU", "reading": "別の読み" })),
        )
        .await;
        assert_eq!(conflict, StatusCode::CONFLICT);
        assert_eq!(body, json!({ "error": "surface already exists" }));
    }

    #[tokio::test]
    async fn dictionary_patch_validates_body_and_id() {
        let app = app();
        let (_, row) = req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "API", "reading": "エーピーアイ" })),
        )
        .await;
        let id = row["id"].as_i64().unwrap();

        let (empty, _) = req(
            &app,
            Method::PATCH,
            &format!("/api/dictionary/{id}"),
            Some(json!({ "surface": " ", "reading": "よみ" })),
        )
        .await;
        assert_eq!(empty, StatusCode::BAD_REQUEST);

        let (missing, _) = req(
            &app,
            Method::PATCH,
            "/api/dictionary/999",
            Some(json!({ "surface": "X", "reading": "エックス" })),
        )
        .await;
        assert_eq!(missing, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn dictionary_delete_and_404() {
        let app = app();
        let (_, row) = req(
            &app,
            Method::POST,
            "/api/dictionary",
            Some(json!({ "surface": "x", "reading": "エックス" })),
        )
        .await;
        let did = row["id"].as_i64().unwrap();
        let (s1, _) = req(
            &app,
            Method::DELETE,
            &format!("/api/dictionary/{did}"),
            None,
        )
        .await;
        assert_eq!(s1, StatusCode::OK);
        let (s2, _) = req(
            &app,
            Method::DELETE,
            &format!("/api/dictionary/{did}"),
            None,
        )
        .await;
        assert_eq!(s2, StatusCode::NOT_FOUND);
    }

    // --- placeholders / spa ---

    #[tokio::test]
    async fn analyze_empty_text_is_400() {
        let app = app();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/analyze")
            .body(Body::from("   "))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn analyze_none_backend_is_502() {
        let app = app();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/analyze")
            .body(Body::from("なにか読み上げて"))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn analyze_carry_out_of_range_is_400() {
        let app = app();
        for carry in ["-0.1", "1.0", "abc"] {
            let req = Request::builder()
                .method(Method::POST)
                .uri(format!("/analyze?carry={carry}"))
                .body(Body::from("本文"))
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "carry={carry}");
        }
    }

    #[tokio::test]
    async fn notify_empty_is_400_and_broadcast_is_204() {
        let app = app();
        let empty = Request::builder()
            .method(Method::POST)
            .uri("/notify")
            .body(Body::from(""))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(empty).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );

        // 購読者ゼロでも 204 (fire-and-forget)。
        let post = Request::builder()
            .method(Method::POST)
            .uri("/notify")
            .body(Body::from("誰も聞いていない"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(post).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
    }

    // SSE ボディから最初の data 行を 1 つ読む (永続ストリームなので全読みしない)。
    async fn read_first_sse_data(body: Body) -> String {
        use tokio_stream::StreamExt;
        let mut stream = body.into_data_stream();
        loop {
            let chunk = stream.next().await.unwrap().unwrap();
            let text = String::from_utf8_lossy(&chunk);
            for line in text.lines() {
                if let Some(d) = line.strip_prefix("data:") {
                    return d.trim().to_string();
                }
            }
        }
    }

    #[tokio::test]
    async fn notify_broadcasts_to_all_subscribers() {
        let app = app();
        // 2 つ購読 (ハンドラ内で subscribe 済みの状態でレスポンスが返る)。
        let sub1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/notify/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let sub2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/notify/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(sub1
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("text/event-stream"));

        // 発火。
        let post = Request::builder()
            .method(Method::POST)
            .uri("/notify")
            .body(Body::from("テスト通知"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(post).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );

        assert_eq!(read_first_sse_data(sub1.into_body()).await, "テスト通知");
        assert_eq!(read_first_sse_data(sub2.into_body()).await, "テスト通知");
    }

    // --- import acting (SSE, フェイク cli analyzer) ---

    fn cli_analyzer(cmd: &str) -> crate::analyze::Backend {
        crate::analyze::Backend::Cli(crate::analyze::CliAnalyzer::new(cmd.to_string()))
    }

    async fn import_body(app: &Router, project_id: i64, body: Value) -> (StatusCode, String) {
        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/projects/{project_id}/import"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let text = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec())
            .unwrap();
        (status, text)
    }

    #[tokio::test]
    async fn import_acting_streams_progress_and_saves_acting() {
        let app = app_with_analyzer(cli_analyzer(
            "cat >/dev/null; echo '[{\"text\":\"こんにちは。\",\"emotion\":{\"honwaka\":60}}]'",
        ));
        let p = create_project(&app, "import").await;
        let id = p["id"].as_i64().unwrap();
        let (status, body) =
            import_body(&app, id, json!({ "mode": "acting", "text": "文一\n文二" })).await;
        assert_eq!(status, StatusCode::OK);

        // 2 件の done 進捗 + complete。
        assert_eq!(body.matches("\"status\":\"done\"").count(), 2);
        assert!(body.contains("\"index\":1"));
        assert!(body.contains("\"index\":2"));
        assert!(body.contains("event:complete") || body.contains("event: complete"));

        let (_, single) = req_get(&app, id).await;
        let lines = single["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .all(|l| l["mode"] == "acting" && l["script"] != Value::Null));
    }

    #[tokio::test]
    async fn import_acting_failed_line_saved_as_announcer() {
        // JSON を返さない analyzer → 分析失敗 → announcer フォールバック。
        let app = app_with_analyzer(cli_analyzer("cat >/dev/null; echo ごめんなさい"));
        let p = create_project(&app, "import").await;
        let id = p["id"].as_i64().unwrap();
        let (status, body) =
            import_body(&app, id, json!({ "mode": "acting", "text": "失敗する文" })).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"status\":\"failed\""), "body: {body}");

        let (_, single) = req_get(&app, id).await;
        let lines = single["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["mode"], "announcer");
        assert_eq!(lines[0]["text"], "失敗する文");
        assert_eq!(lines[0]["script"], Value::Null);
    }

    // --- /work/talk (作業タブの LLM 声かけ) ---

    #[tokio::test]
    async fn work_talk_generates_script_via_cli_backend() {
        let app = app_with_analyzer(cli_analyzer(
            "cat >/dev/null; echo '[{\"text\":\"がんばりましょう\"}]'",
        ));
        let (status, body) = req(
            &app,
            Method::POST,
            "/work/talk",
            Some(json!({ "kind": "milestone", "context": { "phase": "work", "setIndex": 1, "sets": 4 } })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[0]["text"], "がんばりましょう");
    }

    #[tokio::test]
    async fn work_talk_disabled_backend_is_502() {
        let app = app(); // Backend::None
        let (status, _) = req(
            &app,
            Method::POST,
            "/work/talk",
            Some(json!({ "kind": "chatter" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn work_talk_unknown_kind_is_rejected() {
        let app = app();
        let (status, _) = req(
            &app,
            Method::POST,
            "/work/talk",
            Some(json!({ "kind": "dance" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn moca_assets_without_dir_is_404_not_spa() {
        let app = app(); // assets_dir はテスト用の存在しないディレクトリ
        for path in [
            "/moca-assets/moca_illust/001.png",
            "/moca-assets",
            "/moca-assets/",
        ] {
            let (status, body) = req(&app, Method::GET, path, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "path: {path}");
            assert_eq!(body, Value::Null); // index.html にフォールバックしない
        }
    }
}
