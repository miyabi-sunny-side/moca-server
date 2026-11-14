// DB 行 → API 形状。行 (lines) を JSON へ整形する。
// script は保存文字列を JSON parse して構造で返す (null は null)。
// reading は読みの確認の結果 (src/reading.rs)。確かめていない行は null。

use crate::reading::LineReading;
use rusqlite::{Connection, OptionalExtension, Row};
use serde_json::{json, Value};

pub struct LineRow {
    pub id: i64,
    pub project_id: i64,
    pub position: i64,
    pub mode: String,
    pub text: String,
    pub script: Option<String>,
    pub reading: Option<String>,
}

impl LineRow {
    pub fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(LineRow {
            id: row.get(0)?,
            project_id: row.get(1)?,
            position: row.get(2)?,
            mode: row.get(3)?,
            text: row.get(4)?,
            script: row.get(5)?,
            reading: row.get(6)?,
        })
    }
}

pub const LINE_COLS: &str = "id, project_id, position, mode, text, script, reading";

pub fn get_line(conn: &Connection, id: i64) -> rusqlite::Result<Option<LineRow>> {
    conn.query_row(
        &format!("SELECT {LINE_COLS} FROM lines WHERE id = ?1"),
        [id],
        LineRow::from_row,
    )
    .optional()
}

pub fn lines_by_project(conn: &Connection, project_id: i64) -> rusqlite::Result<Vec<LineRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {LINE_COLS} FROM lines WHERE project_id = ?1 ORDER BY position"
    ))?;
    let rows = stmt.query_map([project_id], LineRow::from_row)?;
    rows.collect()
}

pub fn serialize_line(row: &LineRow) -> Value {
    let script = match &row.script {
        Some(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        None => Value::Null,
    };
    json!({
        "id": row.id,
        "project_id": row.project_id,
        "position": row.position,
        "mode": row.mode,
        "text": row.text,
        "script": script,
        "reading": serialize_reading(row.reading.as_deref()),
    })
}

/// 読みの確認の結果に、行を代表する状態 (status) を添えて返す。
fn serialize_reading(stored: Option<&str>) -> Value {
    let Some(reading) = LineReading::parse(stored) else {
        return Value::Null;
    };
    let mut value = serde_json::to_value(&reading).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("status".into(), json!(reading.status()));
    }
    value
}
