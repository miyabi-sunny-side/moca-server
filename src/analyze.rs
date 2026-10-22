// テキスト → 台本JSON の生成ロジック。/analyze とテキスト流し込み (演技モード) が共有する。
// backend は 3 種 (ANALYZE_BACKEND で切替):
//   - cli    … 任意の LLM CLI を sh -c で起動。stdin=プロンプト, stdout=台本JSON
//   - openai … OpenAI 互換の POST {base}/chat/completions を叩く
//   - none   … /analyze を無効化。呼ばれたら AnalyzeError

use crate::error::AppError;
use crate::script::{extract_json, smooth_script, validate_script, ScriptError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

/// 感情分析が無効/失敗したことを表すエラー。/analyze で 502 にマップされる。
#[derive(Debug)]
pub struct AnalyzeError(pub String);

impl From<AnalyzeError> for AppError {
    fn from(e: AnalyzeError) -> Self {
        AppError::BadGateway(e.0)
    }
}

/// 呼び出し側から見た共通インタフェース。carry は感情の引きずり量 (0-0.9, 既定 1/3)。
#[async_trait]
pub trait Analyzer: Send + Sync {
    /// LLM を 1 回叩いて生テキストを回収する。非ゼロ終了/通信失敗は Err (リトライ対象)。
    async fn fetch_output(&self, prompt: &str) -> Result<String, String>;
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
- 出力はJSON配列のみ。コードフェンスや説明文は一切付けない

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
pub async fn run_script_prompt(
    analyzer: &dyn Analyzer,
    prompt: &str,
    carry: f64,
) -> Result<Value, AnalyzeError> {
    let mut last_error = String::new();
    for attempt in 1..=2 {
        let out = match analyzer.fetch_output(prompt).await {
            Ok(o) => o,
            Err(e) => {
                last_error = e;
                tracing::error!("{last_error} (attempt {attempt})");
                continue;
            }
        };
        match extract_json(&out).and_then(|v| validate_script(&v)) {
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
    async fn fetch_output(&self, prompt: &str) -> Result<String, String> {
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

/// `OpenAI` 互換 backend。{base}/chat/completions を叩き choices[0].message.content を回収する。
pub struct OpenaiAnalyzer {
    url: String,
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenaiAnalyzer {
    pub fn new(api_base: &str, api_key: String, model: String) -> Self {
        let base = api_base.trim_end_matches('/');
        Self {
            url: format!("{base}/chat/completions"),
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Analyzer for OpenaiAnalyzer {
    async fn fetch_output(&self, prompt: &str) -> Result<String, String> {
        let res = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&json!({
                "model": self.model,
                "messages": [{ "role": "user", "content": prompt }],
            }))
            .send()
            .await
            .map_err(|e| format!("openai request failed: {e}"))?;

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

/// backend の種別。none は analyze の前段で即 `AnalyzeError` にするため別扱いする
/// (`fetch_output` の Err はリトライ対象なので、無効化はループ外で確定させる)。
pub enum Backend {
    Cli(CliAnalyzer),
    Openai(OpenaiAnalyzer),
    None,
}

impl Backend {
    /// テキストを台本 JSON に変換する。none は TS と完全一致のメッセージで即エラー。
    pub async fn analyze(&self, text: &str, carry: f64) -> Result<Value, AnalyzeError> {
        self.generate(&build_analyze_prompt(text), carry).await
    }

    /// 任意プロンプトから台本 JSON を生成する (/work/talk の声かけが使う)。
    /// retry・検証・スムージングは analyze と同じ共通ループ。
    pub async fn generate(&self, prompt: &str, carry: f64) -> Result<Value, AnalyzeError> {
        match self {
            Backend::Cli(a) => run_script_prompt(a, prompt, carry).await,
            Backend::Openai(a) => run_script_prompt(a, prompt, carry).await,
            Backend::None => Err(AnalyzeError(
                "analyze backend is disabled (set ANALYZE_BACKEND=cli or openai in .env)".into(),
            )),
        }
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
                    Backend::Openai(OpenaiAnalyzer::new(&base, key, model))
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
        let out = backend.analyze("なにか", DEFAULT_CARRY).await.unwrap();
        assert_eq!(out, json!([{ "text": "ok" }]));
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
