// 行 (lines) CRUD + 並べ替え + 流し込み。
// import の acting モードは analyzer を使い、進捗を SSE でストリーミングしながら台本を保存する。

use crate::analyze::DEFAULT_CARRY;
use crate::db::now_iso;
use crate::error::AppError;
use crate::script::serialize_script;
use crate::serialize::{get_line, lines_by_project, serialize_line, LineRow};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{patch, post, put};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::convert::Infallible;
use tokio_stream::wrappers::ReceiverStream;

use super::{parse_body, parse_id, JsonResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{id}/lines", post(add))
        .route("/api/lines/{id}", patch(update).delete(remove))
        .route("/api/lines/{id}/duplicate", post(duplicate))
        .route("/api/projects/{id}/lines/order", put(reorder))
        .route("/api/projects/{id}/import", post(import))
}

fn touch_project(conn: &Connection, project_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE projects SET updated_at = ?1 WHERE id = ?2",
        (now_iso(), project_id),
    )?;
    Ok(())
}

fn next_position(conn: &Connection, project_id: i64) -> rusqlite::Result<i64> {
    let max: Option<i64> = conn.query_row(
        "SELECT max(position) FROM lines WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )?;
    Ok(max.unwrap_or(-1) + 1)
}

fn project_exists(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let found: Option<i64> = conn
        .query_row("SELECT id FROM projects WHERE id = ?1", [id], |r| r.get(0))
        .optional()?;
    Ok(found.is_some())
}

// 行追加 (position は末尾)
async fn add(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> JsonResult {
    let project_id = parse_id(&id)?;
    let body = parse_body(&body);

    let mode = if body.get("mode").and_then(Value::as_str) == Some("acting") {
        "acting"
    } else {
        "announcer"
    };
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");

    let conn = state.db.lock().unwrap();
    if !project_exists(&conn, project_id)? {
        return Err(AppError::NotFound("not found".into()));
    }
    if text.is_empty() {
        return Err(AppError::BadRequest("text required".into()));
    }

    // script は mode==acting かつ body.script が非 null の時のみ検証・保存する。
    let mut script: Option<String> = None;
    if mode == "acting" {
        if let Some(s) = body.get("script") {
            if !s.is_null() {
                script = Some(serialize_script(s)?);
            }
        }
    }

    let position = next_position(&conn, project_id)?;
    let row = conn.query_row(
        "INSERT INTO lines (project_id, position, mode, text, script) \
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id, project_id, position, mode, text, script",
        (project_id, position, mode, text, &script),
        LineRow::from_row,
    )?;
    touch_project(&conn, project_id)?;
    Ok((StatusCode::CREATED, Json(serialize_line(&row))))
}

// 行更新 (text / script / mode)。script は validateScript を通してから保存 (不正は 400)。
async fn update(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> JsonResult {
    let id = parse_id(&id)?;
    let body = parse_body(&body);

    let conn = state.db.lock().unwrap();
    let Some(line) = get_line(&conn, id)? else {
        return Err(AppError::NotFound("not found".into()));
    };

    let mut new_text = line.text.clone();
    if let Some(t) = body.get("text").and_then(Value::as_str) {
        let t = t.trim();
        if t.is_empty() {
            return Err(AppError::BadRequest("text must not be empty".into()));
        }
        new_text = t.to_string();
    }

    let mut new_mode = line.mode.clone();
    match body.get("mode").and_then(Value::as_str) {
        Some(m @ "announcer") | Some(m @ "acting") => new_mode = m.to_string(),
        _ => {}
    }

    // script は「キーが有る&null」「有る&値」「無い」の3状態を区別する。
    let mut new_script = line.script.clone();
    if let Some(s) = body.get("script") {
        if s.is_null() {
            new_script = None;
        } else {
            new_script = Some(serialize_script(s)?);
        }
    }

    let row = conn.query_row(
        "UPDATE lines SET text = ?1, mode = ?2, script = ?3 WHERE id = ?4 \
         RETURNING id, project_id, position, mode, text, script",
        (&new_text, &new_mode, &new_script, id),
        LineRow::from_row,
    )?;
    touch_project(&conn, line.project_id)?;
    Ok((StatusCode::OK, Json(serialize_line(&row))))
}

// 行複製。mode/text/script ごとコピーして対象行の直下 (position+1) に挿入する。
async fn duplicate(State(state): State<AppState>, Path(id): Path<String>) -> JsonResult {
    let id = parse_id(&id)?;
    let mut conn = state.db.lock().unwrap();
    let Some(line) = get_line(&conn, id)? else {
        return Err(AppError::NotFound("not found".into()));
    };

    let row = {
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE lines SET position = position + 1 WHERE project_id = ?1 AND position > ?2",
            (line.project_id, line.position),
        )?;
        let row = tx.query_row(
            "INSERT INTO lines (project_id, position, mode, text, script) \
             VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id, project_id, position, mode, text, script",
            (
                line.project_id,
                line.position + 1,
                &line.mode,
                &line.text,
                &line.script,
            ),
            LineRow::from_row,
        )?;
        tx.commit()?;
        row
    };
    touch_project(&conn, line.project_id)?;
    Ok((StatusCode::CREATED, Json(serialize_line(&row))))
}

// 行削除
async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> JsonResult {
    let id = parse_id(&id)?;
    let conn = state.db.lock().unwrap();
    let Some(line) = get_line(&conn, id)? else {
        return Err(AppError::NotFound("not found".into()));
    };
    conn.execute("DELETE FROM lines WHERE id = ?1", [id])?;
    touch_project(&conn, line.project_id)?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

// JS の Number(value) 相当。数値化できなければ None (NaN)。
fn to_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                Some(0.0)
            } else {
                t.parse::<f64>().ok()
            }
        }
        _ => None,
    }
}

// 並び順一括更新 (id 配列を受け position を 0..n で振り直す)
async fn reorder(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> JsonResult {
    let project_id = parse_id(&id)?;
    let body = parse_body(&body);

    let mut conn = state.db.lock().unwrap();
    if !project_exists(&conn, project_id)? {
        return Err(AppError::NotFound("not found".into()));
    }

    let Some(arr) = body.get("order").and_then(Value::as_array) else {
        return Err(AppError::BadRequest("order must be an array of ids".into()));
    };
    let mut order: Vec<i64> = Vec::with_capacity(arr.len());
    for v in arr {
        match to_number(v) {
            Some(n) => order.push(n as i64),
            None => return Err(AppError::BadRequest("order must be an array of ids".into())),
        }
    }

    let mut existing_set: std::collections::HashSet<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM lines WHERE project_id = ?1")?;
        let rows = stmt.query_map([project_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if order.len() != existing_set.len() || order.iter().any(|oid| !existing_set.remove(oid)) {
        return Err(AppError::BadRequest(
            "order must contain exactly the ids of this project".into(),
        ));
    }

    {
        let tx = conn.transaction()?;
        for (index, line_id) in order.iter().enumerate() {
            tx.execute(
                "UPDATE lines SET position = ?1 WHERE id = ?2",
                (index as i64, line_id),
            )?;
        }
        tx.commit()?;
    }
    touch_project(&conn, project_id)?;

    let rows: Vec<Value> = lines_by_project(&conn, project_id)?
        .iter()
        .map(serialize_line)
        .collect();
    Ok((StatusCode::OK, Json(Value::Array(rows))))
}

// テキスト流し込み。改行で分割し空行は無視。after 指定は両モード共通で前処理する。
//   announcer: 全行を一括 insert して JSON で即返す。
//   acting:    1 行ずつ analyze しながら SSE で進捗を push (失敗行は announcer として保存)。
async fn import(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, AppError> {
    let project_id = parse_id(&id)?;
    let body = parse_body(&body);
    let acting = body.get("mode").and_then(Value::as_str) == Some("acting");

    let raw = body.get("text").and_then(Value::as_str).unwrap_or("");
    // TS: split(/\r?\n/).map(trim).filter(Boolean)。trim() は末尾 \r も除く。
    let texts: Vec<String> = raw
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    // 位置計算と after シフトは両モード共通 (DB ロックは .await をまたがせない)。
    let start_pos = {
        let conn = state.db.lock().unwrap();
        if !project_exists(&conn, project_id)? {
            return Err(AppError::NotFound("not found".into()));
        }

        let mut start_pos = next_position(&conn, project_id)?;
        if !texts.is_empty() {
            if let Some(after) = body.get("after") {
                if !after.is_null() {
                    if let Some(after_id) = to_number(after).map(|n| n as i64) {
                        if let Some(after_row) = get_line(&conn, after_id)? {
                            if after_row.project_id == project_id {
                                conn.execute(
                                    "UPDATE lines SET position = position + ?1 \
                                     WHERE project_id = ?2 AND position > ?3",
                                    (texts.len() as i64, project_id, after_row.position),
                                )?;
                                start_pos = after_row.position + 1;
                            }
                        }
                    }
                }
            }
        }
        start_pos
    };

    if acting {
        return Ok(import_acting(state, project_id, texts, start_pos));
    }

    // announcer: 一括 insert して JSON で即返す。
    let conn = state.db.lock().unwrap();
    if texts.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(json!({ "mode": "announcer", "created": [] })),
        )
            .into_response());
    }
    let mut created: Vec<Value> = Vec::with_capacity(texts.len());
    for (i, text) in texts.iter().enumerate() {
        let row = conn.query_row(
            "INSERT INTO lines (project_id, position, mode, text, script) \
             VALUES (?1, ?2, 'announcer', ?3, NULL) \
             RETURNING id, project_id, position, mode, text, script",
            (project_id, start_pos + i as i64, text),
            LineRow::from_row,
        )?;
        created.push(serialize_line(&row));
    }
    touch_project(&conn, project_id)?;
    Ok((
        StatusCode::OK,
        Json(json!({ "mode": "announcer", "created": created })),
    )
        .into_response())
}

// acting 流し込み: SSE で 1 行ずつ進捗を流す。分析失敗行は announcer として保存し、
// クライアント切断 (受信側 drop) で中断する (保存済み行は残す)。
fn import_acting(state: AppState, project_id: i64, texts: Vec<String>, start_pos: i64) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(4);

    tokio::spawn(async move {
        let total = texts.len();
        for (i, text) in texts.iter().enumerate() {
            if tx.is_closed() {
                break; // クライアント切断。保存済み行は残す。
            }

            // 分析は DB ロック外で await する (Mutex ガードを .await にまたがせない)。
            let analyzed = state.analyzer.analyze(text, DEFAULT_CARRY).await;
            let (mode, script) = match &analyzed {
                Ok(script) => ("acting", Some(script.to_string())),
                Err(_) => ("announcer", None), // 失敗はデータを失わず announcer 保存。
            };

            let row = {
                let conn = state.db.lock().unwrap();
                conn.query_row(
                    "INSERT INTO lines (project_id, position, mode, text, script) \
                     VALUES (?1, ?2, ?3, ?4, ?5) \
                     RETURNING id, project_id, position, mode, text, script",
                    (project_id, start_pos + i as i64, mode, text, &script),
                    LineRow::from_row,
                )
            };
            let row = match row {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("import acting insert failed: {e}");
                    break;
                }
            };

            let status = if mode == "acting" { "done" } else { "failed" };
            let payload = json!({
                "index": i + 1,
                "total": total,
                "status": status,
                "line": serialize_line(&row),
            });
            if tx
                .send(Ok(Event::default().data(payload.to_string())))
                .await
                .is_err()
            {
                break; // 受信側 drop = クライアント切断。
            }
        }

        if total > 0 {
            let conn = state.db.lock().unwrap();
            let _ = touch_project(&conn, project_id);
        }
        let _ = tx
            .send(Ok(Event::default()
                .event("complete")
                .data(json!({ "total": total }).to_string())))
            .await;
    });

    Sse::new(ReceiverStream::new(rx)).into_response()
}
