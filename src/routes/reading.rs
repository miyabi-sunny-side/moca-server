// 読みの確認 (判断は src/reading.rs)。
//   POST /api/projects/{id}/reading/check — project の行を順に確かめ、進捗を SSE で流す。
//     生成 AI が決めた読みと、今の設定のまま合成した音声の聞き取りを比べ、読み違えた語を
//     平仮名・カタカナにした表記を行に保存する。保存した表記は、以後の読み上げに使われる。
//     既に確かめた行は飛ばす (`{"force": true}` で確かめ直す)。
//   POST /api/lines/{id}/reading/dismiss — その行の直しを外す (確かめた記録は残す)。
//
// 生成 AI・聞き取りが動かない行は何も保存しない。読み上げは今のまま続く。

use super::audio::line_segments;
use super::{parse_body, parse_id, JsonResult};
use crate::analyze::{Backend, ResponseFormat};
use crate::db::now_iso;
use crate::dictionary::{load_dictionary, DictEntry};
use crate::error::AppError;
use crate::listen::Listener;
use crate::reading::{
    check_segment, dismiss, is_current, keep_dismissals, LineReading, Status, Tools,
};
use crate::serialize::{get_line, lines_by_project, serialize_line, LineRow};
use crate::state::AppState;
use crate::synth::{synthesize_pcm, SynthConfig};
use crate::wav::{wav_bytes, MOCA_FORMAT};
use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tokio_stream::wrappers::ReceiverStream;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/{id}/reading/check", post(check))
        .route("/api/lines/{id}/reading/dismiss", post(dismiss_line))
}

/// 実物の生成 AI と、VOICEPEAK・聞き取りの道具。
struct Live {
    state: AppState,
    listener: Arc<Listener>,
}

#[async_trait]
impl Tools for Live {
    async fn ask(&self, prompt: &str, format: &ResponseFormat) -> Result<String, String> {
        self.state.analyzer.ask(prompt, format).await
    }

    async fn hear(&self, segment: &Value) -> Result<String, String> {
        let cfg = SynthConfig {
            voicepeak_path: self.state.config.voicepeak_path.clone(),
            narrator: self.state.config.narrator.clone(),
        };
        let pcm = synthesize_pcm(&self.state.synth, &cfg, segment).await?;
        self.listener.hear(&wav_bytes(&MOCA_FORMAT, &pcm)).await
    }
}

async fn check(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, AppError> {
    let project_id = parse_id(&id)?;
    let force = parse_body(&body).get("force").and_then(Value::as_bool) == Some(true);

    let Some(listener) = state.listener.clone() else {
        return Err(AppError::BadGateway(
            "reading check is disabled (set READING_LISTEN_CMD in .env)".into(),
        ));
    };
    if matches!(*state.analyzer, Backend::None) {
        return Err(AppError::BadGateway(
            "reading check needs an analyze backend (set ANALYZE_BACKEND=cli or openai in .env)"
                .into(),
        ));
    }

    let (lines, entries) = {
        let conn = state.db.lock()?;
        let found: Option<i64> = conn
            .query_row("SELECT id FROM projects WHERE id = ?1", [project_id], |r| {
                r.get(0)
            })
            .ok();
        if found.is_none() {
            return Err(AppError::NotFound("not found".into()));
        }
        (
            lines_by_project(&conn, project_id)?,
            load_dictionary(&conn)?,
        )
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(4);
    tokio::spawn(async move {
        let tools = Live { state, listener };
        let total = lines.len();
        for (i, line) in lines.into_iter().enumerate() {
            if tx.is_closed() {
                break; // 画面を閉じた。確かめ終えた行は残す。
            }
            let payload = match check_line(&tools, line, &entries, force).await {
                Ok((status, line, error)) => json!({
                    "index": i + 1,
                    "total": total,
                    "status": status,
                    "error": error,
                    "line": line,
                }),
                Err(e) => {
                    tracing::error!("reading check failed: {e}");
                    break;
                }
            };
            let event = Event::default().data(payload.to_string());
            if tx.send(Ok(event)).await.is_err() {
                break;
            }
        }
        let done = Event::default()
            .event("complete")
            .data(json!({ "total": total }).to_string());
        let _ = tx.send(Ok(done)).await;
    });
    Ok(Sse::new(ReceiverStream::new(rx)).into_response())
}

/// 1 行を確かめて保存する。返すのは (進捗の状態, 画面へ返す行, 確かめられなかった理由)。
///   checked: 確かめて結果を保存した / skipped: 既に確かめてある / failed: 確かめられず、何も変えていない
async fn check_line(
    tools: &Live,
    line: LineRow,
    entries: &[DictEntry],
    force: bool,
) -> Result<(&'static str, Value, Option<String>), AppError> {
    let segments = line_segments(&line, entries, &tools.state.fallback)?;
    let stored = LineReading::parse(line.reading.as_deref());
    if !force
        && stored
            .as_ref()
            .is_some_and(|reading| is_current(reading, &segments))
    {
        return Ok(("skipped", serialize_line(&line), None));
    }

    let started = Instant::now();
    let mut checked = Vec::with_capacity(segments.len());
    for segment in &segments {
        let result = check_segment(tools, segment).await;
        if result.status == Status::Failed {
            // 生成 AI か聞き取りが動かない。前の結果も表記もそのままにする。
            return Ok(("failed", serialize_line(&line), result.error));
        }
        checked.push(result);
    }
    let reading = LineReading {
        checked_at: now_iso(),
        secs: (started.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        segments: checked,
    };
    // 利用者が外した直しは、確かめ直しても掛け直さない。
    let reading = keep_dismissals(reading, stored.as_ref());

    // 確かめている間に文が変わった行には保存しない (結果は古い文のもの)。
    let conn = tools.state.db.lock()?;
    conn.execute(
        "UPDATE lines SET reading = ?1 WHERE id = ?2 AND text = ?3",
        (serde_json::to_string(&reading)?, line.id, &line.text),
    )?;
    let saved = get_line(&conn, line.id)?.unwrap_or(line);
    Ok(("checked", serialize_line(&saved), None))
}

async fn dismiss_line(State(state): State<AppState>, Path(id): Path<String>) -> JsonResult {
    let id = parse_id(&id)?;
    let conn = state.db.lock()?;
    let Some(line) = get_line(&conn, id)? else {
        return Err(AppError::NotFound("not found".into()));
    };
    let Some(reading) = LineReading::parse(line.reading.as_deref()) else {
        return Err(AppError::Conflict("the line has no reading check".into()));
    };
    conn.execute(
        "UPDATE lines SET reading = ?1 WHERE id = ?2",
        (serde_json::to_string(&dismiss(reading))?, id),
    )?;
    let saved = get_line(&conn, id)?.unwrap_or(line);
    Ok((StatusCode::OK, Json(serialize_line(&saved))))
}

#[cfg(test)]
mod tests;
