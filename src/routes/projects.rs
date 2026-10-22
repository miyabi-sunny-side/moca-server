// プロジェクト CRUD。作成/一覧/取得/リネーム/削除 (削除は行をカスケード)。

use crate::db::now_iso;
use crate::error::AppError;
use crate::serialize::{lines_by_project, serialize_line};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{parse_body, parse_id, JsonResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects", get(list).post(create))
        .route(
            "/api/projects/{id}",
            get(get_one).patch(rename).delete(remove),
        )
}

// project 行 (全列) を Value にする。
fn project_value(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let (id, name, created_at, updated_at): (i64, String, String, String) =
        (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
    Ok(json!({ "id": id, "name": name, "created_at": created_at, "updated_at": updated_at }))
}

// 一覧: リストペイン用に行数と updated_at を付ける
async fn list(State(state): State<AppState>) -> JsonResult {
    let conn = state.db.lock()?;
    let mut stmt = conn.prepare(
        "SELECT projects.id, projects.name, projects.created_at, projects.updated_at, \
         count(lines.id) AS lineCount \
         FROM projects LEFT JOIN lines ON lines.project_id = projects.id \
         GROUP BY projects.id ORDER BY projects.updated_at DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "name": r.get::<_, String>(1)?,
                "created_at": r.get::<_, String>(2)?,
                "updated_at": r.get::<_, String>(3)?,
                "lineCount": r.get::<_, i64>(4)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((StatusCode::OK, Json(Value::Array(rows))))
}

// 作成
async fn create(State(state): State<AppState>, body: Bytes) -> JsonResult {
    let body = parse_body(&body);
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .map_or("", str::trim);
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let now = now_iso();
    let row = {
        let conn = state.db.lock()?;
        conn.query_row(
            "INSERT INTO projects (name, created_at, updated_at) VALUES (?1, ?2, ?3) \
             RETURNING id, name, created_at, updated_at",
            (name, &now, &now),
            project_value,
        )?
    };
    Ok((StatusCode::CREATED, Json(row)))
}

// 単体 + その行を position 昇順で
async fn get_one(State(state): State<AppState>, Path(id): Path<String>) -> JsonResult {
    let id = parse_id(&id)?;
    let conn = state.db.lock()?;
    let project = conn
        .query_row(
            "SELECT id, name, created_at, updated_at FROM projects WHERE id = ?1",
            [id],
            project_value,
        )
        .optional()?;
    let Some(mut project) = project else {
        return Err(AppError::NotFound("not found".into()));
    };
    let lines: Vec<Value> = lines_by_project(&conn, id)?
        .iter()
        .map(serialize_line)
        .collect();
    if let Value::Object(map) = &mut project {
        map.insert("lines".into(), Value::Array(lines));
    }
    Ok((StatusCode::OK, Json(project)))
}

// 改名 (updated_at 更新)
async fn rename(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> JsonResult {
    let id = parse_id(&id)?;
    let body = parse_body(&body);
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .map_or("", str::trim);
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let now = now_iso();
    let row = {
        let conn = state.db.lock()?;
        conn.query_row(
            "UPDATE projects SET name = ?1, updated_at = ?2 WHERE id = ?3 \
             RETURNING id, name, created_at, updated_at",
            (name, &now, id),
            project_value,
        )
        .optional()?
    };
    match row {
        Some(row) => Ok((StatusCode::OK, Json(row))),
        None => Err(AppError::NotFound("not found".into())),
    }
}

// 削除 (lines も cascade 削除)
async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> JsonResult {
    let id = parse_id(&id)?;
    let mut conn = state.db.lock()?;
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM projects WHERE id = ?1", [id], |r| r.get(0))
        .optional()?;
    if exists.is_none() {
        return Err(AppError::NotFound("not found".into()));
    }
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM lines WHERE project_id = ?1", [id])?;
    tx.execute("DELETE FROM projects WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}
