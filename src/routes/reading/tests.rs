// 読みの確認を API の外側から確かめる。VOICEPEAK・生成 AI・聞き取りの道具は shell の偽物で、
// 偽 VOICEPEAK は渡された文をそのまま WAV の中身にし、偽の聞き取りがその文から音素を決める。

use crate::analyze::{Backend, CliAnalyzer};
use crate::config::Config;
use crate::listen::Listener;
use crate::state::AppState;
use crate::synth::SynthQueue;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tower::ServiceExt;

const ICHINICHI: &str = "それでは今日も一日頑張りましょう。";
const FIXED: &str = "それでは今日もいちにち頑張りましょう。";
const TSUITACHI: &str = "十一月一日に出発します。";

// 渡された文を PCM の位置に入れた WAV を -o へ書き、文を記録する。
const FAKE_VOICEPEAK: &str = r#"#!/usr/bin/env bash
out=""; text=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2;;
    -s) text="$2"; shift 2;;
    *) shift;;
  esac
done
printf '%s\n' "$text" >> "$RECORD"
le32() { printf "$(printf '\\x%02x\\x%02x\\x%02x\\x%02x' $(($1&255)) $(($1>>8&255)) $(($1>>16&255)) $(($1>>24&255)))"; }
n=$(printf '%s' "$text" | wc -c); pad=$((n%2)); size=$((n+pad))
{
  printf 'RIFF'; le32 $((36+size)); printf 'WAVEfmt '; le32 16
  printf '\x01\x00\x01\x00'; le32 48000; le32 96000; printf '\x02\x00\x10\x00'
  printf 'data'; le32 $size; printf '%s' "$text"
  if [ $pad = 1 ]; then printf '\0'; fi
} > "$out"
"#;

// WAV の中身 (= 読み上げた文) から、聞こえる音素を決める。「一日」は必ず「ついたち」と読む声。
const FAKE_LISTEN: &str = r#"#!/usr/bin/env bash
text=$(tail -c +45 | tr -d '\0')
case "$text" in
  *今日も一日*) echo "s o r e d e w a ky o o m o ts u i t a ch i g a N b a r i m a sh o o";;
  *今日もいちにち*) echo "s o r e d e w a ky o o m o i ch i n i ch i g a N b a r i m a sh o o";;
  *十一月一日*) echo "j u u i ch i g a ts U ts u i t a ch i n i sh u cl p a ts U sh i m a s U";;
  *) echo "listen: unknown text: $text" >&2; exit 1;;
esac
"#;

// 文脈を読める生成 AI。読みの問いには語ごとの読みを、2 択には「今日も一日」なら いちにち を返す。
const FAKE_ANALYZER: &str = r#"#!/usr/bin/env bash
prompt=$(cat)
case "$prompt" in
  *"A: いちにち"*) echo '{"answer":"A"}';;
  *"B: いちにち"*) echo '{"answer":"B"}';;
  *"文: それでは今日も一日"*) echo '[{"word":"それでは","yomi":"それでは"},{"word":"今日","yomi":"きょう"},{"word":"も","yomi":"も"},{"word":"一日","yomi":"いちにち"},{"word":"頑張りましょう","yomi":"がんばりましょう"},{"word":"。","yomi":""}]';;
  *"文: それでは今日もいちにち"*) echo '[{"word":"それでは","yomi":"それでは"},{"word":"今日","yomi":"きょう"},{"word":"も","yomi":"も"},{"word":"いちにち","yomi":"いちにち"},{"word":"頑張りましょう","yomi":"がんばりましょう"},{"word":"。","yomi":""}]';;
  *"文: 十一月一日"*) echo '[{"word":"十一月","yomi":"じゅういちがつ"},{"word":"一日","yomi":"ついたち"},{"word":"に","yomi":"に"},{"word":"出発します","yomi":"しゅっぱつします"},{"word":"。","yomi":""}]';;
  *) echo "analyzer: unknown prompt" >&2; exit 1;;
esac
"#;

struct Env {
    dir: TempDir,
    state: AppState,
    app: Router,
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

impl Env {
    fn new() -> Self {
        Self::with(true, true)
    }

    fn with(analyzer: bool, listener: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("record.txt");
        // 偽 VOICEPEAK は環境変数を受け取れない (引数だけで起動される) ので、記録先を書き込んでおく。
        let voicepeak = script(
            dir.path(),
            "voicepeak",
            &FAKE_VOICEPEAK.replace("$RECORD", &record.display().to_string()),
        );
        let listen = script(dir.path(), "listen", FAKE_LISTEN);
        let analyze = script(dir.path(), "analyze", FAKE_ANALYZER);

        let conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn);
        let (notify, _) = tokio::sync::broadcast::channel::<String>(16);
        let state = AppState {
            db: Arc::new(Mutex::new(conn)),
            config: Config {
                port: 0,
                db_path: ":memory:".into(),
                voicepeak_path: voicepeak.display().to_string(),
                narrator: "Test".into(),
                bep_dict_path: "./bep-eng.dic".into(),
                bep_dict_url: String::new(),
                work_talk_timeout_secs: 60,
                assets_dir: "./target/test-missing-assets".into(),
                moca_illust_url: String::new(),
            },
            synth: Arc::new(SynthQueue::new()),
            analyzer: Arc::new(if analyzer {
                Backend::Cli(CliAnalyzer::new(analyze.display().to_string()))
            } else {
                Backend::None
            }),
            news_analyzer: None,
            fallback: Arc::new(crate::fallback::FallbackDict::empty()),
            listener: listener.then(|| Arc::new(Listener::new(listen.display().to_string()))),
            notify,
        };
        let app = super::super::build_router(state.clone());
        Self { dir, state, app }
    }

    /// project を作り、行を入れる。返すのは (project の id, 行の id)。
    fn project(&self, lines: &[&str]) -> (i64, Vec<i64>) {
        let conn = self.state.db.lock().unwrap();
        conn.execute(
            "INSERT INTO projects (name, created_at, updated_at) VALUES ('P', 'now', 'now')",
            [],
        )
        .unwrap();
        let project = conn.last_insert_rowid();
        let ids = lines
            .iter()
            .enumerate()
            .map(|(position, text)| {
                conn.execute(
                    "INSERT INTO lines (project_id, position, mode, text) VALUES (?1, ?2, 'announcer', ?3)",
                    rusqlite::params![project, position, text],
                )
                .unwrap();
                conn.last_insert_rowid()
            })
            .collect();
        (project, ids)
    }

    /// VOICEPEAK へ渡された文 (合成した順)。
    fn spoken(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("record.txt"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    async fn send(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Vec<u8>) {
        let mut request = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(json) => {
                request = request.header("content-type", "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        let res = self
            .app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, bytes.to_vec())
    }

    async fn json(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, bytes) = self.send(method, uri, body).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// 読みの確認を最後まで流し、行ごとの進捗 (SSE の data) を返す。
    async fn check(&self, project: i64, force: bool) -> Vec<Value> {
        let uri = format!("/api/projects/{project}/reading/check");
        let (status, bytes) = self
            .send(Method::POST, &uri, Some(json!({ "force": force })))
            .await;
        assert_eq!(status, StatusCode::OK, "check failed");
        let events: Vec<Value> = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|data| serde_json::from_str(data.trim()).unwrap())
            .collect();
        let (last, lines) = events.split_last().expect("a complete event");
        assert_eq!(
            last["total"],
            lines.len(),
            "complete event counts the lines"
        );
        lines.to_vec()
    }

    async fn line(&self, project: i64, line: i64) -> Value {
        let (_, body) = self
            .json(Method::GET, &format!("/api/projects/{project}"), None)
            .await;
        let lines = body["lines"].as_array().expect("project lines");
        lines
            .iter()
            .find(|l| l["id"] == line)
            .cloned()
            .expect("the line")
    }
}

#[tokio::test]
async fn a_misread_line_is_fixed_and_a_correct_line_is_left_alone() {
    let env = Env::new();
    let (project, ids) = env.project(&[ICHINICHI, TSUITACHI]);
    let events = env.check(project, false).await;

    assert_eq!(events[0]["status"], "checked");
    assert_eq!(events[0]["index"], 1);
    assert_eq!(events[0]["total"], 2);
    let fixed = &events[0]["line"];
    assert_eq!(fixed["text"], ICHINICHI, "the shown text is never changed");
    assert_eq!(fixed["reading"]["status"], "fixed");
    let segment = &fixed["reading"]["segments"][0];
    assert_eq!(segment["text"], ICHINICHI);
    assert_eq!(
        segment["expected"],
        "それではきょうもいちにちがんばりましょう"
    );
    assert_eq!(segment["heard"], "それではきょうもついたちがんばりましょう");
    assert_eq!(segment["spoken"], FIXED);
    assert_eq!(
        segment["words"],
        json!([{ "surface": "一日", "expected": "いちにち", "heard": "ついたち", "action": "fixed" }])
    );

    let kept = &events[1]["line"];
    assert_eq!(kept["reading"]["status"], "ok");
    assert_eq!(kept["reading"]["segments"][0]["spoken"], Value::Null);
    assert_eq!(
        env.spoken(),
        [ICHINICHI, FIXED, TSUITACHI],
        "the fix is confirmed by ear; the correct line is synthesized once"
    );

    // 保存されていて、project を開き直しても見える。
    assert_eq!(
        env.line(project, ids[0]).await["reading"]["status"],
        "fixed"
    );
}

#[tokio::test]
async fn playback_keeps_using_the_fix() {
    let env = Env::new();
    let (project, ids) = env.project(&[ICHINICHI]);
    env.check(project, false).await;
    let before = env.spoken().len();

    // 動画用の WAV
    let (status, _) = env
        .send(
            Method::GET,
            &format!("/api/projects/{project}/lines/{}/audio.wav", ids[0]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // 画面の再生 (アナウンサー行は text、演技行は台本を送る)
    let line = ids[0];
    let (status, _) = env
        .send(
            Method::GET,
            &format!("/say?line={line}&text={ICHINICHI}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let script = json!([{ "text": ICHINICHI, "speed": 110 }]);
    let (status, _) = env
        .send(
            Method::POST,
            &format!("/say?line={line}"),
            Some(script.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(env.spoken()[before..], [FIXED, FIXED, FIXED]);

    // 行を指定しない読み上げと、?raw=1 は今までどおり。
    env.send(Method::POST, "/say", Some(script)).await;
    env.send(
        Method::GET,
        &format!("/say?line={line}&raw=1&text={ICHINICHI}"),
        None,
    )
    .await;
    assert_eq!(env.spoken()[before + 3..], [ICHINICHI, ICHINICHI]);
}

#[tokio::test]
async fn checked_lines_are_not_checked_again_unless_forced() {
    let env = Env::new();
    let (project, _) = env.project(&[ICHINICHI, TSUITACHI]);
    env.check(project, false).await;
    let before = env.spoken().len();

    let again = env.check(project, false).await;
    assert!(again.iter().all(|e| e["status"] == "skipped"), "{again:?}");
    assert_eq!(again[0]["line"]["reading"]["status"], "fixed");
    assert_eq!(env.spoken().len(), before, "nothing is synthesized");

    let forced = env.check(project, true).await;
    assert!(
        forced.iter().all(|e| e["status"] == "checked"),
        "{forced:?}"
    );
    assert_eq!(forced[0]["line"]["reading"]["status"], "fixed");
}

#[tokio::test]
async fn editing_the_text_drops_the_fix_and_the_line_is_checked_again() {
    let env = Env::new();
    let (project, ids) = env.project(&[ICHINICHI]);
    env.check(project, false).await;

    let uri = format!("/api/lines/{}", ids[0]);
    // 台本・モードだけの更新では直しを保つ。
    let (_, kept) = env
        .json(Method::PATCH, &uri, Some(json!({ "mode": "acting" })))
        .await;
    assert_eq!(kept["reading"]["status"], "fixed");
    let (_, edited) = env
        .json(Method::PATCH, &uri, Some(json!({ "text": TSUITACHI })))
        .await;
    assert_eq!(edited["reading"], Value::Null);

    let events = env.check(project, false).await;
    assert_eq!(events[0]["status"], "checked");
    assert_eq!(events[0]["line"]["reading"]["status"], "ok");
}

#[tokio::test]
async fn a_duplicated_line_keeps_the_fix() {
    let env = Env::new();
    let (project, ids) = env.project(&[ICHINICHI]);
    env.check(project, false).await;
    let (status, copy) = env
        .json(
            Method::POST,
            &format!("/api/lines/{}/duplicate", ids[0]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(copy["reading"]["segments"][0]["spoken"], FIXED);
}

#[tokio::test]
async fn a_dismissed_fix_is_no_longer_spoken_nor_fixed_again() {
    let env = Env::new();
    let (project, ids) = env.project(&[ICHINICHI]);
    env.check(project, false).await;

    let uri = format!("/api/lines/{}/reading/dismiss", ids[0]);
    let (status, line) = env.json(Method::POST, &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(line["reading"]["status"], "dismissed");
    assert_eq!(line["reading"]["segments"][0]["spoken"], Value::Null);
    assert_eq!(
        line["reading"]["segments"][0]["heard"], "それではきょうもついたちがんばりましょう",
        "the record stays visible"
    );

    let before = env.spoken().len();
    let events = env.check(project, false).await;
    assert_eq!(events[0]["status"], "skipped");
    // 確かめ直しても、外した直しは戻らない。
    let events = env.check(project, true).await;
    assert_eq!(events[0]["status"], "checked");
    assert_eq!(events[0]["line"]["reading"]["status"], "dismissed");
    let before = before + 2;
    assert_eq!(env.spoken().len(), before, "checked again by ear");
    env.send(
        Method::GET,
        &format!("/say?line={}&text={ICHINICHI}", ids[0]),
        None,
    )
    .await;
    assert_eq!(env.spoken()[before..], [ICHINICHI]);
}

#[tokio::test]
async fn dismissing_an_unchecked_or_missing_line_is_refused() {
    let env = Env::new();
    let (_, ids) = env.project(&[ICHINICHI]);
    let (status, _) = env
        .json(
            Method::POST,
            &format!("/api/lines/{}/reading/dismiss", ids[0]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = env
        .json(Method::POST, "/api/lines/9999/reading/dismiss", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn without_an_ear_or_an_ai_the_check_is_refused_and_speech_goes_on() {
    for (analyzer, listener) in [(true, false), (false, true)] {
        let env = Env::with(analyzer, listener);
        let (project, ids) = env.project(&[ICHINICHI]);
        let uri = format!("/api/projects/{project}/reading/check");
        let (status, body) = env.json(Method::POST, &uri, None).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(body["error"].is_string(), "the reason is told: {body}");
        assert_eq!(env.line(project, ids[0]).await["reading"], Value::Null);

        let (status, _) = env
            .send(
                Method::GET,
                &format!("/say?line={}&text={ICHINICHI}", ids[0]),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(env.spoken(), [ICHINICHI]);
    }
}

#[tokio::test]
async fn a_line_that_cannot_be_checked_keeps_its_earlier_fix() {
    let env = Env::new();
    let (project, ids) = env.project(&[ICHINICHI, "知らない文です。"]);
    let events = env.check(project, false).await;
    assert_eq!(events[0]["status"], "checked");
    // 偽の生成 AI はこの文に答えられない (= 生成 AI が動かない時と同じ)。
    assert_eq!(events[1]["status"], "failed");
    assert!(events[1]["error"].is_string(), "{}", events[1]);
    assert_eq!(
        events[1]["line"]["reading"],
        Value::Null,
        "nothing is saved"
    );

    // 聞き取りが壊れた後に確かめ直しても、前の直しは残る。
    std::fs::write(env.dir.path().join("listen"), "#!/bin/sh\nexit 1\n").unwrap();
    let events = env.check(project, true).await;
    assert_eq!(events[0]["status"], "failed");
    assert_eq!(
        env.line(project, ids[0]).await["reading"]["status"],
        "fixed"
    );
}

#[tokio::test]
async fn check_of_a_missing_project_is_404() {
    let env = Env::new();
    let (status, _) = env
        .json(Method::POST, "/api/projects/9999/reading/check", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_dictionary_change_makes_the_line_due_for_a_new_check() {
    let env = Env::new();
    let (project, ids) = env.project(&[ICHINICHI]);
    env.check(project, false).await;
    // 辞書で読み上げ文が変わると、古い直しは使われず、次の確認の対象になる。
    env.state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO dictionary (surface, reading, created_at) VALUES ('一日', 'いちにち', 'now')",
            [],
        )
        .unwrap();
    let before = env.spoken().len();
    env.send(
        Method::GET,
        &format!("/say?line={}&text={ICHINICHI}", ids[0]),
        None,
    )
    .await;
    assert_eq!(
        env.spoken()[before..],
        [FIXED],
        "the dictionary now does the job"
    );
    let events = env.check(project, false).await;
    assert_eq!(events[0]["status"], "checked");
    assert_eq!(events[0]["line"]["reading"]["status"], "ok");
}
