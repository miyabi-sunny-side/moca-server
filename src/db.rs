use chrono::{SecondsFormat, Utc};
use rusqlite::Connection;

/// 現在時刻を ISO8601 ミリ秒精度 + Z (UTC) で返す。
pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// 起動時に raw SQL でテーブルを作る。日時は ISO8601 文字列で統一。
pub fn open(path: &str) -> Connection {
    let conn = Connection::open(path).expect("Failed to open database");
    // WAL + NORMAL 同期 + 外部キー有効。in-memory では WAL は実質 no-op。
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
    )
    .expect("Failed to set pragmas");
    init(&conn);
    conn
}

pub fn init(conn: &Connection) {
    conn.execute_batch(
        r"
        CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS lines (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id INTEGER NOT NULL REFERENCES projects(id),
            position INTEGER NOT NULL,
            mode TEXT NOT NULL,
            text TEXT NOT NULL,
            script TEXT
        );
        CREATE INDEX IF NOT EXISTS lines_project_position_idx ON lines(project_id, position);
        CREATE TABLE IF NOT EXISTS dictionary (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            surface TEXT NOT NULL,
            reading TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS dictionary_surface_idx ON dictionary(surface);
        ",
    )
    .expect("Failed to initialize database schema");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_tables() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn);
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('projects', 'lines', 'dictionary')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn init_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn);
        init(&conn);
    }
}
