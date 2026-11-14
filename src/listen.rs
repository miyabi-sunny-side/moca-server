// 聞き取りの道具: 合成した音声 (WAV) を外部コマンドへ渡し、聞こえた読みを受け取る。
// 道具は `READING_LISTEN_CMD` で差し替える。stdin に WAV、stdout に `OpenJTalk` の音素を
// 空白で区切って返すものなら何でもよい (同梱の bin/moca-hear がその形で返す)。

use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// 聞き取り 1 回を待つ上限。CPU の音素認識は 1 文 1 秒ほどで、初回は model の読み込みが加わる。
const TIMEOUT: Duration = Duration::from_secs(120);

pub struct Listener {
    cmd: String,
    timeout: Duration,
}

/// `READING_LISTEN_CMD` の値を読む。未設定・空は「聞き取りなし」。
fn parse_cmd(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|cmd| !cmd.is_empty())
        .map(str::to_string)
}

impl Listener {
    pub fn new(cmd: String) -> Self {
        Self {
            cmd,
            timeout: TIMEOUT,
        }
    }

    pub fn from_env() -> Option<Self> {
        parse_cmd(std::env::var("READING_LISTEN_CMD").ok().as_deref()).map(Self::new)
    }

    /// WAV を聞かせ、聞こえた音素を返す。失敗・時間切れは Err (呼び出し側は表記を変えない)。
    pub async fn hear(&self, wav: &[u8]) -> Result<String, String> {
        tokio::time::timeout(self.timeout, self.run(wav))
            .await
            .map_err(|_| {
                format!(
                    "listen command timed out after {} seconds",
                    self.timeout.as_secs()
                )
            })?
    }

    async fn run(&self, wav: &[u8]) -> Result<String, String> {
        // kill_on_drop: 時間切れや要求の中断で future が落ちた時に、道具を残さない。
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&self.cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn listen command: {e}"))?;

        if let Some(mut stdin) = child.stdin.take() {
            // 道具が読まずに終わると書き込みは失敗する。その時は終了コードと stderr の方を報告する。
            let _ = stdin.write_all(wav).await;
            let _ = stdin.shutdown().await;
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| format!("wait listen command: {e}"))?;
        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr.chars().take(500).collect();
            return Err(format!("listen command failed ({code}): {tail}"));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_blank_command_means_no_listener() {
        assert_eq!(parse_cmd(None), None);
        assert_eq!(parse_cmd(Some("  ")), None);
        assert_eq!(parse_cmd(Some(" moca-hear ")), Some("moca-hear".into()));
    }

    #[tokio::test]
    async fn the_command_receives_the_wav_and_its_output_is_returned() {
        let listener = Listener::new("wc -c | tr -d ' '; echo ' k a N j i'".into());
        let out = listener.hear(&[0u8; 1234]).await.unwrap();
        assert_eq!(out, "1234\n k a N j i");
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_exit_code_and_stderr() {
        let listener = Listener::new("echo 'model missing' >&2; exit 3".into());
        let err = listener.hear(b"RIFF").await.unwrap_err();
        assert!(err.contains("(3)"), "message: {err}");
        assert!(err.contains("model missing"), "message: {err}");
    }

    #[tokio::test]
    async fn a_command_that_hangs_is_given_up() {
        let listener = Listener {
            cmd: "sleep 30".into(),
            timeout: Duration::from_millis(100),
        };
        let err = listener.hear(b"RIFF").await.unwrap_err();
        assert!(err.contains("timed out"), "message: {err}");
    }
}
