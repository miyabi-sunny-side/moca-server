// 音声合成パイプ: 文分割・直列化・常駐 VOICEPEAK の再利用と配信。

use crate::opus::OpusStream;
use crate::voicepeak::Voicepeak;
use crate::wav::{wav_header, MOCA_FORMAT};
use axum::body::Bytes;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, MutexGuard};

// 48000Hz * 16bit * 1ch = 96000 bytes/s → 96 bytes/ms。
const BYTES_PER_MS: usize = 96;
// voicepeak は稀に起動直後に失敗するため、セグメントごとに最大 3 回リトライする。
const MAX_ATTEMPTS: usize = 3;

/// 合成のサーバー全体直列化キュー。
///
/// VOICEPEAK CLI は同時 1 プロセスに制限されている (ライセンス保護のハードチェック)。
/// 2 個目の起動は「up to 1 command line instance can be executed at same time」で
/// 失敗しリトライでは回避できないため、合成は必ず直列化する。
/// tokio の Mutex は FIFO 公平なので追加のキュー実装は不要。
pub struct SynthQueue {
    lock: Mutex<Voicepeak>,
    depth: AtomicUsize,
}

impl SynthQueue {
    pub fn new() -> Self {
        Self {
            lock: Mutex::new(Voicepeak::default()),
            depth: AtomicUsize::new(0),
        }
    }

    /// 待ち行列に加わり、ロックを取得したら SynthGuard を返す。
    /// Guard が drop されるまでロックを保持する。常駐プロセスは次の要求へ引き継ぐ。
    pub async fn enter(&self) -> SynthGuard<'_> {
        let waiting = self.depth.fetch_add(1, Ordering::SeqCst) + 1;
        tracing::debug!("synth queue depth: {waiting}");
        let permit = self.lock.lock().await;
        SynthGuard {
            _permit: permit,
            depth: &self.depth,
        }
    }
}

/// SynthQueue のロック保持証。Drop でロック解放と depth の減算を一元化する。
pub struct SynthGuard<'a> {
    _permit: MutexGuard<'a, Voicepeak>,
    depth: &'a AtomicUsize,
}

impl Drop for SynthGuard<'_> {
    fn drop(&mut self) {
        self.depth.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 合成に必要な voicepeak 設定 (Config から複製して合成タスクに渡す)。
#[derive(Clone)]
pub struct SynthConfig {
    pub voicepeak_path: String,
    pub narrator: String,
}

/// text/plain を文単位に分割する。規則:
/// 「。！？の直後」と「改行の直後」で区切り、区切り文字は前セグメント末尾に残す。
/// 空白 (半角 space / 全角　) のみの断片はスキップする。
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut cur = String::new();

    let flush = |cur: &mut String, out: &mut Vec<String>| {
        // 空白 (半角/全角) のみの断片は捨てる。どちらにせよ cur は空に戻す。
        let seg = std::mem::take(cur);
        if seg.chars().any(|c| c != ' ' && c != '　') {
            out.push(seg);
        }
    };

    for c in text.chars() {
        if c == '\n' {
            // 改行は区切りのみで、セグメントには含めない。
            flush(&mut cur, &mut segments);
        } else {
            cur.push(c);
            if c == '。' || c == '！' || c == '？' {
                flush(&mut cur, &mut segments);
            }
        }
    }
    flush(&mut cur, &mut segments);
    segments
}

/// pause (ms) 分の無音 PCM を生成する (48kHz/16bit/mono の 0 埋め)。
pub fn silence_pcm(pause_ms: usize) -> Vec<u8> {
    vec![0u8; pause_ms * BYTES_PER_MS]
}

/// 1 セグメントを voicepeak で合成し PCM (s16le/48k/mono) を返す。最大 3 回リトライ。
async fn synthesize_segment(
    engine: &mut Voicepeak,
    cfg: &SynthConfig,
    seg: &Value,
) -> Result<Vec<u8>, String> {
    let text = seg.get("text").and_then(Value::as_str).unwrap_or("");

    // emotion: {k1:v1,k2:v2} → "k1=v1,k2=v2" (serde_json の Map はキー昇順で安定)。
    let emotion = seg
        .get("emotion")
        .and_then(Value::as_object)
        .filter(|o| !o.is_empty())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        });
    let speed = seg.get("speed").and_then(Value::as_i64);
    let pitch = seg.get("pitch").and_then(Value::as_i64);

    let mut last_err = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        match run_voicepeak(engine, cfg, text, emotion.as_deref(), speed, pitch).await {
            Ok(pcm) => return Ok(pcm),
            Err(e) => {
                tracing::warn!("voicepeak failed (attempt {attempt}) for {text:?}: {e}");
                last_err = e;
            }
        }
    }
    Err(format!("voicepeak gave up on {text:?}: {last_err}"))
}

/// 1 件の生成を依頼し、tempfile 経由で出力 WAV を読んで PCM を取り出す。
async fn run_voicepeak(
    engine: &mut Voicepeak,
    cfg: &SynthConfig,
    text: &str,
    emotion: Option<&str>,
    speed: Option<i64>,
    pitch: Option<i64>,
) -> Result<Vec<u8>, String> {
    // 出力は必ずシーク可能なユニーク tempfile: -o /dev/stdout は空、FIFO は即失敗する
    // (RIFF chunk_size を後埋めするため)。NamedTempFile の RAII で使用後即削除される。
    let tmp = tempfile::NamedTempFile::new().map_err(|e| format!("tempfile: {e}"))?;
    let out_path = tmp.path();

    let mut args = vec![
        "voicepeak".into(),
        "-s".into(),
        text.into(),
        "-n".into(),
        cfg.narrator.clone(),
        "-o".into(),
        out_path.to_str().ok_or("output path is not UTF-8")?.into(),
    ];
    if let Some(e) = emotion {
        args.extend(["-e".into(), e.into()]);
    }
    if let Some(s) = speed {
        args.extend(["--speed".into(), s.to_string()]);
    }
    if let Some(p) = pitch {
        args.extend(["--pitch".into(), p.to_string()]);
    }
    engine
        .synthesize(&cfg.voicepeak_path, &args, out_path)
        .await
}

/// 配信フォーマット。既定は Opus (帯域 1/12)、Accept: audio/wav で Wav (動画素材用の可逆)。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Wav,
    Opus,
}

/// セグメント列を順に合成し、フォーマットに応じた音声フレームを mpsc へ送る。
/// タスク先頭で synth.enter() してサーバー全体の合成を直列化する
/// (ロックは本関数の終了まで保持)。
pub async fn stream_synthesis(
    synth: Arc<SynthQueue>,
    cfg: SynthConfig,
    segments: Vec<Value>,
    format: OutputFormat,
    tx: Sender<Result<Bytes, std::io::Error>>,
) {
    let mut guard = synth.enter().await;
    match format {
        OutputFormat::Wav => stream_wav(&mut guard._permit, &cfg, &segments, &tx).await,
        OutputFormat::Opus => stream_opus(&mut guard._permit, &cfg, &segments, &tx).await,
    }
}

/// WAV 経路 (動画素材用の可逆出力): ヘッダ + 各 PCM + pause 無音をそのまま送る。
async fn stream_wav(
    engine: &mut Voicepeak,
    cfg: &SynthConfig,
    segments: &[Value],
    tx: &Sender<Result<Bytes, std::io::Error>>,
) {
    // 最初にヘッダを送る → 1 文目完成時点で再生が始まる体感を保つ。
    if tx
        .send(Ok(Bytes::from(wav_header(&MOCA_FORMAT))))
        .await
        .is_err()
    {
        return; // 受信側 drop = クライアント切断。ロックを解放して終了。
    }

    for seg in segments {
        let pcm = match synth_or_abort(engine, cfg, seg, tx).await {
            Some(p) => p,
            None => return,
        };
        if tx.send(Ok(Bytes::from(pcm))).await.is_err() {
            return;
        }

        let pause = seg.get("pause").and_then(Value::as_u64).unwrap_or(0) as usize;
        if pause > 0 && tx.send(Ok(Bytes::from(silence_pcm(pause)))).await.is_err() {
            return;
        }
    }
}

/// Opus 経路: PCM を Ogg/Opus へ逐次エンコードし、セグメント完了ごとに
/// 出来上がった Ogg ページをチャンク送信する (セグメント間ストリーミング維持)。
async fn stream_opus(
    engine: &mut Voicepeak,
    cfg: &SynthConfig,
    segments: &[Value],
    tx: &Sender<Result<Bytes, std::io::Error>>,
) {
    let mut stream = match OpusStream::new() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("opus init failed: {e}");
            let _ = tx.send(Err(std::io::Error::other(e))).await;
            return;
        }
    };

    // OpusHead / OpusTags ページを先に送る。
    let header = stream.take_bytes();
    if tx.send(Ok(Bytes::from(header))).await.is_err() {
        return;
    }

    for seg in segments {
        let pcm = match synth_or_abort(engine, cfg, seg, tx).await {
            Some(p) => p,
            None => return,
        };
        // pause はエンコーダに 0 PCM として流す (Opus でも尺に反映)。
        let pause = seg.get("pause").and_then(Value::as_u64).unwrap_or(0) as usize;
        let encode = (|| {
            stream.push_pcm(&pcm)?;
            if pause > 0 {
                stream.push_pcm(&silence_pcm(pause))?;
            }
            Ok::<(), String>(())
        })();
        if let Err(e) = encode {
            tracing::error!("opus encode failed: {e}");
            let _ = tx.send(Err(std::io::Error::other(e))).await;
            return;
        }
        let page = stream.take_bytes();
        if !page.is_empty() && tx.send(Ok(Bytes::from(page))).await.is_err() {
            return;
        }
    }

    // 端数フレームを flush して EOS ページを送る。
    if let Err(e) = stream.finish() {
        tracing::error!("opus finish failed: {e}");
        let _ = tx.send(Err(std::io::Error::other(e))).await;
        return;
    }
    let tail = stream.take_bytes();
    if !tail.is_empty() {
        let _ = tx.send(Ok(Bytes::from(tail))).await;
    }
}

/// 1 セグメントを合成する。切断なら None を返し (タスク終了)、合成失敗ならエラーフレームを送って None。
async fn synth_or_abort(
    engine: &mut Voicepeak,
    cfg: &SynthConfig,
    seg: &Value,
    tx: &Sender<Result<Bytes, std::io::Error>>,
) -> Option<Vec<u8>> {
    tokio::select! {
        r = synthesize_segment(engine, cfg, seg) => match r {
            Ok(p) => Some(p),
            Err(e) => {
                // 3 リトライ後も失敗。ヘッダ送信済みでステータス変更不可なので
                // エラーフレームを送ってボディをエラー終了させる。
                tracing::error!("synth aborted: {e}");
                let _ = tx.send(Err(std::io::Error::other(e))).await;
                None
            }
        },
        _ = tx.closed() => None, // 受信側 drop = クライアント切断。合成タスクを中断し voicepeak を kill する。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_punctuation_and_newline() {
        assert_eq!(
            split_sentences("こんにちは。今日はいい天気。"),
            vec!["こんにちは。", "今日はいい天気。"]
        );
        assert_eq!(split_sentences("一行目\n二行目"), vec!["一行目", "二行目"]);
        assert_eq!(
            split_sentences("やあ！げんき？うん"),
            vec!["やあ！", "げんき？", "うん"]
        );
    }

    #[test]
    fn skips_whitespace_only_fragments() {
        // 空白のみ (半角/全角) の断片はスキップ、通常のトレーリング改行も落ちる。
        assert_eq!(split_sentences("A。  \n　\nB。\n"), vec!["A。", "B。"]);
    }

    #[test]
    fn keeps_trailing_text_without_delimiter() {
        assert_eq!(split_sentences("区切りなし"), vec!["区切りなし"]);
        assert_eq!(split_sentences(""), Vec::<String>::new());
    }

    #[test]
    fn silence_is_96_bytes_per_ms() {
        assert_eq!(silence_pcm(0).len(), 0);
        assert_eq!(silence_pcm(100).len(), 100 * 96);
        assert!(silence_pcm(10).iter().all(|&b| b == 0));
    }
}
