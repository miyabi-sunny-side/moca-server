// テキスト → 台本JSON の生成ロジック。/analyze とテキスト流し込み (演技モード) が共有する。
// backend は 3 種 (ANALYZE_BACKEND で切替):
//   - cli    … 任意の LLM CLI を sh -c で起動。stdin=プロンプト, stdout=台本JSON
//   - openai … OpenAI 互換の POST {base}/chat/completions を叩く
//   - none   … /analyze を無効化。呼ばれたら AnalyzeError

use crate::error::AppError;
use crate::script::{
    ensure_text_kept, extract_json, script_json_schema, smooth_script, validate_script, ScriptError,
};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// 感情分析が無効/失敗したことを表すエラー。/analyze で 502 にマップされる。
#[derive(Debug)]
pub struct AnalyzeError(pub String);

impl From<AnalyzeError> for AppError {
    fn from(e: AnalyzeError) -> Self {
        AppError::BadGateway(e.0)
    }
}

/// LLM に求める出力の形 (JSON Schema)。`OPENAI_JSON_SCHEMA=1` の `OpenAI` 互換 backend だけが要求に付ける。
pub struct ResponseFormat {
    pub name: &'static str,
    pub schema: Value,
}

/// 呼び出し側から見た共通インタフェース。carry は感情の引きずり量 (0-0.9, 既定 1/3)。
#[async_trait]
pub trait Analyzer: Send + Sync {
    /// LLM を 1 回叩いて生テキストを回収する。非ゼロ終了/通信失敗は Err (リトライ対象)。
    /// format は出力の形の希望で、渡せる backend だけが要求に付ける。
    async fn fetch_output(
        &self,
        prompt: &str,
        format: Option<&ResponseFormat>,
    ) -> Result<String, String>;
}

/// 既定 carry。TS の `carry = 1 / 3` に対応。
pub const DEFAULT_CARRY: f64 = 1.0 / 3.0;

/// 台本 JSON の共通ルール (感情軸・speed/pitch/pause・出力形式)。
/// /analyze のプロンプトと /work/talk の声かけ生成プロンプトが共有する。
pub const SCRIPT_RULES: &str = r#"- 感情軸は bosoboso(ぼそぼそ・陰気), doyaru(ドヤ顔・得意げ), honwaka(ほんわか・優しい), angry(怒り), teary(涙声・悲しい) の5種で、値は0〜100
- 使う軸だけを含める。平坦な文は emotion 自体を省略してよい
- 必要に応じて speed(50-200, 標準100), pitch(-300〜300, 標準0), pause(文の後の無音ms) も付与できる
- pitch は基本 0 付近を維持する。上げるのは「よほど焦っている(『可愛いね』などと
  口説かれて動揺する等)」「我を忘れて激昂している」といった、よほどの理由付けが
  ある時だけにすること
- speed は、長めの文は早口で言うために気持ち速め(100台前半)にする。標準より
  遅くするのは「それに。」「愛しの先輩に会えますもんね」のように、からかうため
  あえて強調して言う時だけ。悲しい文・沈んだ文でも speed は下げず、
  bosoboso/teary と pause で表現すること
- 出力はJSON配列のみ。コードフェンスや説明文は一切付けない。改行やインデントも入れず、1行で出力する

出力スキーマ (1要素 = 1文):
[{"text":"文","emotion":{"angry":80},"speed":110,"pitch":0,"pause":300}]"#;

/// 指示と変換対象テキストを結合して 1 つのプロンプトにする。
pub fn build_analyze_prompt(text: &str) -> String {
    let head = r"以下のテキストを、音声合成ソフト VOICEPEAK の「宮舞モカ」で読み上げるための台本JSONに変換してください。

ルール:
- まずテキスト全体を読み、話者の基調となるトーン(基調感情)を決める
- テキストを文単位に分割し、文ごとに感情パラメータを推定して付与する。
  ただし各文の感情は基調感情を土台にし、文単位で完全に切り替えない。
  本文が明確に要求する場合以外、隣り合う文でトーンを急変させないこと
  (例: 疲れた発言の直後の文は、内容が前向きでも明るさを抑えて余韻を残す)
";
    format!("{head}{SCRIPT_RULES}\n\n変換対象のテキスト:\n{text}")
}

/// 任意プロンプトで LLM を叩き、出力を台本 JSON として検証・スムージングする
/// 共通ループ。失敗したら最大 2 回まで再試行する。analyze と /work/talk が共有。
/// source は読み上げたい元のセリフ。渡すと、台本の text がそれと違う出力も失敗に数える
/// (LLM がセリフを作る /work/talk には元のセリフが無いので None)。
pub async fn run_script_prompt(
    analyzer: &dyn Analyzer,
    prompt: &str,
    source: Option<&str>,
    carry: f64,
) -> Result<Value, AnalyzeError> {
    let format = script_format();
    let mut last_error = String::new();
    for attempt in 1..=2 {
        let out = match analyzer.fetch_output(prompt, Some(&format)).await {
            Ok(o) => o,
            Err(e) => {
                last_error = e;
                tracing::error!("{last_error} (attempt {attempt})");
                continue;
            }
        };
        let checked = extract_json(&out)
            .and_then(|v| validate_script(&v))
            .and_then(|script| match source {
                Some(source) => ensure_text_kept(&script, source).map(|()| script),
                None => Ok(script),
            });
        match checked {
            Ok(script) => return Ok(smooth_script(&script, carry)),
            Err(ScriptError(msg)) => {
                last_error = format!("analyzer returned an invalid script: {msg}");
                tracing::error!("{last_error} (attempt {attempt})");
            }
        }
    }
    Err(AnalyzeError(last_error))
}

/// CLI backend。sh -c <cmd> を起動し stdin にプロンプト全文、stdout から回収する。
pub struct CliAnalyzer {
    cmd: String,
}

impl CliAnalyzer {
    pub fn new(cmd: String) -> Self {
        Self { cmd }
    }
}

#[async_trait]
impl Analyzer for CliAnalyzer {
    async fn fetch_output(
        &self,
        prompt: &str,
        _format: Option<&ResponseFormat>,
    ) -> Result<String, String> {
        // kill_on_drop: 呼び出し側の timeout やクライアント切断で future が落ちたとき、
        // CLI プロセス (LLM) を残存させない。
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&self.cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn analyze command: {e}"))?;

        // stdin にプロンプトを書いて閉じる (子が全読みできるよう drop で EOF)。
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.as_bytes())
                .await
                .map_err(|e| format!("write analyze stdin: {e}"))?;
            stdin
                .shutdown()
                .await
                .map_err(|e| format!("close analyze stdin: {e}"))?;
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| format!("wait analyze command: {e}"))?;
        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr.chars().take(500).collect();
            return Err(format!("analyze command failed ({code}): {tail}"));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// 相手が黙って接続を受けない時 (電源断など) に待つ上限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `OpenAI` 互換 backend。{base}/chat/completions を叩き choices[0].message.content を回収する。
pub struct OpenaiAnalyzer {
    url: String,
    api_key: String,
    model: String,
    extra_body: Map<String, Value>,
    /// 出力の形 (JSON Schema) を `response_format` として要求に付けるか (`OPENAI_JSON_SCHEMA`)。
    json_schema: bool,
    client: reqwest::Client,
}

impl OpenaiAnalyzer {
    /// `extra_body` は要求へ足す相手ごとの項目 (thinking を切る指定など)。
    /// timeout は 1 回の要求の上限で、None なら応答を待ち続ける。
    pub fn new(
        api_base: &str,
        api_key: String,
        model: String,
        extra_body: Map<String, Value>,
        json_schema: bool,
        timeout: Option<Duration>,
    ) -> Self {
        let base = api_base.trim_end_matches('/');
        let mut client = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);
        if let Some(timeout) = timeout {
            client = client.timeout(timeout);
        }
        Self {
            url: format!("{base}/chat/completions"),
            api_key,
            model,
            extra_body,
            json_schema,
            client: client.build().expect("build the HTTP client"),
        }
    }
}

#[async_trait]
impl Analyzer for OpenaiAnalyzer {
    async fn fetch_output(
        &self,
        prompt: &str,
        format: Option<&ResponseFormat>,
    ) -> Result<String, String> {
        let format = format.filter(|_| self.json_schema);
        let res = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&chat_body(&self.model, prompt, &self.extra_body, format))
            .send()
            .await
            .map_err(|e| format!("openai request failed: {}", error_chain(&e)))?;

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let body = res.text().await.unwrap_or_default();
            let tail: String = body.chars().take(500).collect();
            return Err(format!("openai backend failed ({status}): {tail}"));
        }

        let data: Value = res
            .json()
            .await
            .map_err(|e| format!("openai response parse failed: {e}"))?;
        Ok(data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }
}

const DISABLED: &str = "analyze backend is disabled (set ANALYZE_BACKEND=cli or openai in .env)";

/// backend の種別。none は analyze の前段で即 `AnalyzeError` にするため別扱いする
/// (`fetch_output` の Err はリトライ対象なので、無効化はループ外で確定させる)。
pub enum Backend {
    Cli(CliAnalyzer),
    Openai(OpenaiAnalyzer),
    None,
}

impl Backend {
    /// テキストを台本 JSON に変換する。none は TS と完全一致のメッセージで即エラー。
    /// 台本の text が入力と違う出力は受け取らない (文字化け・言い換えを読み上げない)。
    pub async fn analyze(&self, text: &str, carry: f64) -> Result<Value, AnalyzeError> {
        self.run(&build_analyze_prompt(text), Some(text), carry)
            .await
    }

    /// 任意プロンプトから台本 JSON を生成する (/work/talk の声かけが使う)。
    /// retry・検証・スムージングは analyze と同じ共通ループ。
    pub async fn generate(&self, prompt: &str, carry: f64) -> Result<Value, AnalyzeError> {
        self.run(prompt, None, carry).await
    }

    /// 任意プロンプトで LLM を 1 回叩き、出力をそのまま返す (読みの確認が使う)。
    /// 再試行と出力の検査は呼び出し側が持つ。
    pub async fn ask(&self, prompt: &str, format: &ResponseFormat) -> Result<String, String> {
        match self {
            Backend::Cli(a) => a.fetch_output(prompt, Some(format)).await,
            Backend::Openai(a) => a.fetch_output(prompt, Some(format)).await,
            Backend::None => Err(DISABLED.into()),
        }
    }

    async fn run(
        &self,
        prompt: &str,
        source: Option<&str>,
        carry: f64,
    ) -> Result<Value, AnalyzeError> {
        match self {
            Backend::Cli(a) => run_script_prompt(a, prompt, source, carry).await,
            Backend::Openai(a) => run_script_prompt(a, prompt, source, carry).await,
            Backend::None => Err(AnalyzeError(DISABLED.into())),
        }
    }
}

/// エラーを原因まで辿って 1 行にする。reqwest の表示は「送れなかった」までで、
/// 接続拒否か時間切れかは原因 (source) の側にある。
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        message = format!("{message}: {error}");
        cause = error.source();
    }
    message
}

/// chat/completions へ送る本文。追加項目 (`OPENAI_EXTRA_BODY`) を足すが、
/// model と messages は常に moca-server の値にする。format があれば `response_format` に付ける。
fn chat_body(
    model: &str,
    prompt: &str,
    extra: &Map<String, Value>,
    format: Option<&ResponseFormat>,
) -> Value {
    let mut body = extra.clone();
    if let Some(format) = format {
        body.insert(
            "response_format".into(),
            json!({
                "type": "json_schema",
                "json_schema": { "name": format.name, "strict": true, "schema": format.schema },
            }),
        );
    }
    body.insert("model".into(), Value::from(model));
    body.insert(
        "messages".into(),
        json!([{ "role": "user", "content": prompt }]),
    );
    Value::Object(body)
}

/// `OPENAI_JSON_SCHEMA` を読む。`1` で出力の JSON Schema を要求に付ける。未設定・空・`0` は付けない。
fn parse_json_schema_flag(raw: Option<&str>) -> Result<bool, String> {
    match raw.map(str::trim) {
        None | Some("" | "0") => Ok(false),
        Some("1") => Ok(true),
        Some(other) => Err(format!("OPENAI_JSON_SCHEMA must be 0 or 1: {other}")),
    }
}

/// 台本の出力の形。
fn script_format() -> ResponseFormat {
    ResponseFormat {
        name: "script",
        schema: script_json_schema(),
    }
}

/// `OPENAI_EXTRA_BODY` を読む。未設定・空は追加なし。JSON object 以外は Err。
fn parse_extra_body(raw: Option<&str>) -> Result<Map<String, Value>, String> {
    let raw = raw.map(str::trim).filter(|r| !r.is_empty());
    let Some(raw) = raw else {
        return Ok(Map::new());
    };
    serde_json::from_str(raw).map_err(|e| format!("OPENAI_EXTRA_BODY must be a JSON object: {e}"))
}

/// `OPENAI_TIMEOUT_SECS` を読む。未設定・空は上限なし。1 以上の整数以外は Err。
fn parse_timeout(raw: Option<&str>) -> Result<Option<Duration>, String> {
    let raw = raw.map(str::trim).filter(|r| !r.is_empty());
    let Some(raw) = raw else {
        return Ok(None);
    };
    match raw.parse::<u64>() {
        Ok(secs) if secs > 0 => Ok(Some(Duration::from_secs(secs))),
        _ => Err(format!(
            "OPENAI_TIMEOUT_SECS must be a whole number of seconds (1 or more): {raw}"
        )),
    }
}

/// 環境変数から backend を組み立てる。既定は none。
/// 欠落/未知値は起動時エラーとする。
pub fn create_analyzer_from_env() -> Backend {
    let backend = std::env::var("ANALYZE_BACKEND").unwrap_or_else(|_| "none".into());
    match backend.as_str() {
        "none" => Backend::None,
        "cli" => {
            let cmd = std::env::var("ANALYZE_CMD")
                .unwrap_or_else(|_| "claude -p --model haiku".into())
                .trim()
                .to_string();
            assert!(
                !cmd.is_empty(),
                "ANALYZE_CMD must not be empty when ANALYZE_BACKEND=cli"
            );
            Backend::Cli(CliAnalyzer::new(cmd))
        }
        "openai" => {
            let base = std::env::var("OPENAI_API_BASE").ok();
            let key = std::env::var("OPENAI_API_KEY").ok();
            let model = std::env::var("OPENAI_MODEL").ok();
            match (base, key, model) {
                (Some(base), Some(key), Some(model))
                    if !base.is_empty() && !key.is_empty() && !model.is_empty() =>
                {
                    let var = |name: &str| std::env::var(name).ok();
                    let fail = |e: String| -> ! { panic!("{e}") };
                    let extra = parse_extra_body(var("OPENAI_EXTRA_BODY").as_deref());
                    let schema = parse_json_schema_flag(var("OPENAI_JSON_SCHEMA").as_deref());
                    let timeout = parse_timeout(var("OPENAI_TIMEOUT_SECS").as_deref());
                    let extra = extra.unwrap_or_else(|e| fail(e));
                    let schema = schema.unwrap_or_else(|e| fail(e));
                    let timeout = timeout.unwrap_or_else(|e| fail(e));
                    Backend::Openai(OpenaiAnalyzer::new(
                        &base, key, model, extra, schema, timeout,
                    ))
                }
                _ => panic!(
                    "ANALYZE_BACKEND=openai requires OPENAI_API_BASE, OPENAI_API_KEY, OPENAI_MODEL"
                ),
            }
        }
        other => panic!("unknown ANALYZE_BACKEND: {other} (expected: cli, openai, none)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_ends_with_the_text() {
        let p = build_analyze_prompt("本文だよ");
        assert!(p.starts_with("以下のテキストを"));
        assert!(p.ends_with("変換対象のテキスト:\n本文だよ"));
    }

    #[tokio::test]
    async fn cli_backend_returns_script() {
        let cmd = "cat >/dev/null; echo '[{\"text\":\"ok\"}]'".to_string();
        let backend = Backend::Cli(CliAnalyzer::new(cmd));
        let out = backend.analyze("ok", DEFAULT_CARRY).await.unwrap();
        assert_eq!(out, json!([{ "text": "ok" }]));
    }

    #[tokio::test]
    async fn analyze_rejects_a_script_whose_text_differs() {
        let cmd = "cat >/dev/null; echo '[{\"text\":\"ok\"}]'".to_string();
        let backend = Backend::Cli(CliAnalyzer::new(cmd));
        let err = backend.analyze("なにか", DEFAULT_CARRY).await.unwrap_err();
        assert!(
            err.0.contains("differs from the input"),
            "message: {}",
            err.0
        );
    }

    /// 決めた出力を順に返す backend。呼ばれた回数も数える。
    struct Canned(std::sync::Mutex<Vec<Result<String, String>>>);

    impl Canned {
        fn new(outputs: &[Result<&str, &str>]) -> Self {
            let outputs = outputs
                .iter()
                .rev()
                .map(|o| o.map(str::to_string).map_err(str::to_string))
                .collect();
            Self(std::sync::Mutex::new(outputs))
        }

        fn remaining(&self) -> usize {
            self.0.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl Analyzer for Canned {
        async fn fetch_output(
            &self,
            _prompt: &str,
            _format: Option<&ResponseFormat>,
        ) -> Result<String, String> {
            self.0.lock().unwrap().pop().unwrap()
        }
    }

    #[tokio::test]
    async fn garbled_text_is_retried_then_accepted() {
        let llm = Canned::new(&[
            Ok(r#"[{"text":"おはよウございます。"}]"#),
            Ok(r#"[{"text":"おはようございます。"}]"#),
        ]);
        let out = run_script_prompt(&llm, "p", Some("おはようございます。"), 0.0)
            .await
            .unwrap();
        assert_eq!(out, json!([{ "text": "おはようございます。" }]));
    }

    #[tokio::test]
    async fn text_that_stays_different_is_an_error() {
        let llm = Canned::new(&[
            Ok(r#"[{"text":"おはよウございます。"}]"#),
            Ok(r#"[{"text":"おはよう。"}]"#),
            Ok(r#"[{"text":"おはようございます。"}]"#),
        ]);
        let err = run_script_prompt(&llm, "p", Some("おはようございます。"), 0.0)
            .await
            .unwrap_err();
        assert!(
            err.0.contains("differs from the input"),
            "message: {}",
            err.0
        );
        assert_eq!(llm.remaining(), 1, "must stop after two attempts");
    }

    #[tokio::test]
    async fn generated_lines_have_no_source_to_compare() {
        let llm = Canned::new(&[Ok(r#"[{"text":"がんばろっか"}]"#)]);
        let out = run_script_prompt(&llm, "声かけを作って", None, 0.0)
            .await
            .unwrap();
        assert_eq!(out, json!([{ "text": "がんばろっか" }]));
    }

    #[tokio::test]
    async fn unreachable_backend_fails_after_two_attempts() {
        let llm = Canned::new(&[
            Err("connection refused"),
            Err("connection refused"),
            Ok("[]"),
        ]);
        let err = run_script_prompt(&llm, "p", Some("x"), 0.0)
            .await
            .unwrap_err();
        assert_eq!(err.0, "connection refused");
        assert_eq!(llm.remaining(), 1, "must stop after two attempts");
    }

    #[test]
    fn error_chain_includes_every_cause() {
        #[derive(Debug)]
        struct Layer(&'static str, Option<Box<Layer>>);
        impl std::fmt::Display for Layer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for Layer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.1.as_deref().map(|e| e as _)
            }
        }
        let refused = Layer("connection refused", None);
        let connect = Layer("client error (Connect)", Some(Box::new(refused)));
        let send = Layer("error sending request", Some(Box::new(connect)));
        assert_eq!(
            error_chain(&send),
            "error sending request: client error (Connect): connection refused"
        );
    }

    #[test]
    fn chat_body_without_extra_is_model_and_messages() {
        assert_eq!(
            chat_body("m", "やあ", &Map::new(), None),
            json!({ "model": "m", "messages": [{ "role": "user", "content": "やあ" }] })
        );
    }

    #[test]
    fn chat_body_merges_extra_but_keeps_model_and_messages() {
        let extra = parse_extra_body(Some(
            r#"{"reasoning_effort":"none","model":"other","messages":[]}"#,
        ))
        .unwrap();
        assert_eq!(
            chat_body("m", "やあ", &extra, None),
            json!({
                "reasoning_effort": "none",
                "model": "m",
                "messages": [{ "role": "user", "content": "やあ" }],
            })
        );
    }

    #[test]
    fn script_schema_is_sent_as_response_format() {
        let extra = parse_extra_body(Some(r#"{"reasoning_effort":"none"}"#)).unwrap();
        let body = chat_body("m", "やあ", &extra, Some(&script_format()));
        assert_eq!(body["response_format"]["json_schema"]["name"], "script");
        assert_eq!(body["reasoning_effort"], "none");
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"],
            crate::script::script_json_schema()
        );
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["type"],
            "array"
        );
    }

    #[test]
    fn each_request_sends_its_own_schema() {
        let format = crate::reading::words_format();
        let body = chat_body("m", "やあ", &Map::new(), Some(&format));
        assert_eq!(body["response_format"]["json_schema"]["name"], "reading");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"],
            format.schema
        );
    }

    #[tokio::test]
    async fn ask_returns_the_raw_output_and_none_is_an_error() {
        let format = crate::reading::words_format();
        let backend = Backend::Cli(CliAnalyzer::new("cat".into()));
        assert_eq!(backend.ask("そのまま", &format).await.unwrap(), "そのまま");
        let err = Backend::None.ask("x", &format).await.unwrap_err();
        assert!(err.contains("disabled"), "message: {err}");
    }

    #[test]
    fn json_schema_flag_is_off_unless_one() {
        assert_eq!(parse_json_schema_flag(None), Ok(false));
        assert_eq!(parse_json_schema_flag(Some("")), Ok(false));
        assert_eq!(parse_json_schema_flag(Some("0")), Ok(false));
        assert_eq!(parse_json_schema_flag(Some("1")), Ok(true));
        assert!(parse_json_schema_flag(Some("yes")).is_err());
    }

    #[test]
    fn extra_body_unset_or_blank_adds_nothing() {
        assert!(parse_extra_body(None).unwrap().is_empty());
        assert!(parse_extra_body(Some("  ")).unwrap().is_empty());
    }

    #[test]
    fn extra_body_must_be_a_json_object() {
        assert!(parse_extra_body(Some("{reasoning_effort:none}")).is_err());
        assert!(parse_extra_body(Some(r#"["reasoning_effort"]"#)).is_err());
    }

    #[test]
    fn timeout_unset_or_blank_means_no_limit() {
        assert_eq!(parse_timeout(None), Ok(None));
        assert_eq!(parse_timeout(Some(" ")), Ok(None));
    }

    #[test]
    fn timeout_is_whole_seconds() {
        assert_eq!(parse_timeout(Some("30")), Ok(Some(Duration::from_secs(30))));
    }

    #[test]
    fn timeout_rejects_zero_and_non_numbers() {
        assert!(parse_timeout(Some("0")).is_err());
        assert!(parse_timeout(Some("30s")).is_err());
        assert!(parse_timeout(Some("-1")).is_err());
    }

    #[tokio::test]
    async fn cli_non_json_output_is_analyze_error() {
        let cmd = "cat >/dev/null; echo ごめんなさい".to_string();
        let backend = Backend::Cli(CliAnalyzer::new(cmd));
        assert!(backend.analyze("なにか", DEFAULT_CARRY).await.is_err());
    }

    #[tokio::test]
    async fn none_backend_message_matches_ts() {
        let err = Backend::None.analyze("x", DEFAULT_CARRY).await.unwrap_err();
        assert_eq!(
            err.0,
            "analyze backend is disabled (set ANALYZE_BACKEND=cli or openai in .env)"
        );
    }
}
