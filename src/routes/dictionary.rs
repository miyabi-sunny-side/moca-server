// 読み替え辞書 CRUD。surface (表記) をキーに upsert し、reading (読み) を返す。

use crate::db::now_iso;
use crate::error::AppError;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{parse_body, parse_id, JsonResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/dictionary", get(list).post(upsert))
        .route("/api/dictionary/{id}", patch(update).delete(remove))
}

fn required_fields(body: &Value) -> Result<(&str, &str), AppError> {
    let surface = body
        .get("surface")
        .and_then(Value::as_str)
        .map_or("", str::trim);
    let reading = body
        .get("reading")
        .and_then(Value::as_str)
        .map_or("", str::trim);
    if surface.is_empty() {
        return Err(AppError::BadRequest("surface required".into()));
    }
    if reading.is_empty() {
        return Err(AppError::BadRequest("reading required".into()));
    }
    Ok((surface, reading))
}

fn entry_value(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let (id, surface, reading, created_at): (i64, String, String, String) =
        (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
    Ok(json!({ "id": id, "surface": surface, "reading": reading, "created_at": created_at }))
}

// 全件 (表記昇順の安定順)
async fn list(State(state): State<AppState>) -> JsonResult {
    let conn = state.db.lock()?;
    let mut stmt =
        conn.prepare("SELECT id, surface, reading, created_at FROM dictionary ORDER BY surface")?;
    let rows = stmt
        .query_map([], entry_value)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((StatusCode::OK, Json(Value::Array(rows))))
}

// 追加 / 更新。surface が既存なら reading を上書き (upsert)。insert/update どちらも 201。
async fn upsert(State(state): State<AppState>, body: Bytes) -> JsonResult {
    let body = parse_body(&body);
    let (surface, reading) = required_fields(&body)?;

    let conn = state.db.lock()?;
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM dictionary WHERE surface = ?1",
            [surface],
            |r| r.get(0),
        )
        .optional()?;

    let row = match existing {
        Some(existing_id) => conn.query_row(
            "UPDATE dictionary SET reading = ?1 WHERE id = ?2 \
             RETURNING id, surface, reading, created_at",
            (reading, existing_id),
            entry_value,
        )?,
        None => conn.query_row(
            "INSERT INTO dictionary (surface, reading, created_at) VALUES (?1, ?2, ?3) \
             RETURNING id, surface, reading, created_at",
            (surface, reading, now_iso()),
            entry_value,
        )?,
    };
    Ok((StatusCode::CREATED, Json(row)))
}

// ID を維持したまま表記・読みを更新する。別 ID の表記との衝突は明示的に 409。
async fn update(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> JsonResult {
    let id = parse_id(&id)?;
    let body = parse_body(&body);
    let (surface, reading) = required_fields(&body)?;
    let conn = state.db.lock()?;
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM dictionary WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    if exists.is_none() {
        return Err(AppError::NotFound("not found".into()));
    }
    let conflict: Option<i64> = conn
        .query_row(
            "SELECT id FROM dictionary WHERE surface = ?1 AND id != ?2",
            (surface, id),
            |r| r.get(0),
        )
        .optional()?;
    if conflict.is_some() {
        return Err(AppError::Conflict("surface already exists".into()));
    }
    let row = conn.query_row(
        "UPDATE dictionary SET surface = ?1, reading = ?2 WHERE id = ?3 \
         RETURNING id, surface, reading, created_at",
        (surface, reading, id),
        entry_value,
    )?;
    Ok((StatusCode::OK, Json(row)))
}

// 削除
async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> JsonResult {
    let id = parse_id(&id)?;
    let conn = state.db.lock()?;
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM dictionary WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    if exists.is_none() {
        return Err(AppError::NotFound("not found".into()));
    }
    conn.execute("DELETE FROM dictionary WHERE id = ?1", [id])?;
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}
