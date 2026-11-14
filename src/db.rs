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
            script TEXT,
            reading TEXT
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
    add_column_if_missing(conn, "lines", "reading", "TEXT");
}

/// 後から足した列を、既にある DB にも足す (`CREATE TABLE IF NOT EXISTS` は列を足さない)。
fn add_column_if_missing(conn: &Connection, table: &str, column: &str, kind: &str) {
    let exists: bool = conn
        .query_row(
            "SELECT count(*) > 0 FROM pragma_table_info(?1) WHERE name = ?2",
            [table, column],
            |row| row.get(0),
        )
        .expect("Failed to read the table columns");
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"),
            [],
        )
        .expect("Failed to add a column");
    }
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

    // reading 列が無かった頃の DB を開いても、行を保ったまま列が足される。
    #[test]
    fn init_adds_the_reading_column_to_an_older_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE lines (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                mode TEXT NOT NULL,
                text TEXT NOT NULL,
                script TEXT
            );
            INSERT INTO lines (project_id, position, mode, text) VALUES (1, 0, 'announcer', '前からある行');",
        )
        .unwrap();
        init(&conn);
        init(&conn);
        let (text, reading): (String, Option<String>) = conn
            .query_row("SELECT text, reading FROM lines", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(text, "前からある行");
        assert_eq!(reading, None);
    }

    #[test]
    fn init_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn);
        init(&conn);
    }
}
