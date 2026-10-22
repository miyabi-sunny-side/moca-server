// s16le/48k/mono PCM を Ogg/Opus へ逐次エンコードするストリーミングエンコーダ。
// コーデックは ropus (libopus 固定小数点版の純 Rust 移植、C リファレンスと
// ビット一致) + ogg コンテナで、C ツールチェーン依存なしに全 OS でビルドできる。
// 合格基準は「ffplay/Chrome で正しく再生され尺が合う」= 完全仕様準拠より実再生優先。

use ogg::{PacketWriteEndInfo, PacketWriter};
use ropus::{Application, Bitrate, Channels, Encoder};

const SAMPLE_RATE: u32 = 48000;
/// 20ms フレーム = 960 サンプル (48kHz mono)。
const FRAME_SAMPLES: usize = 960;
const BITRATE: u32 = 64000;
/// Ogg logical stream serial。単一ストリームなので固定値でよい ("MOCA")。
const SERIAL: u32 = 0x4d4f_4341;

/// `OpusHead` パケット (19 bytes)。RFC 7845 §5.1。
fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity(19);
    v.extend_from_slice(b"OpusHead");
    v.push(1); // version
    v.push(1); // channel count (mono)
    v.extend_from_slice(&pre_skip.to_le_bytes());
    v.extend_from_slice(&SAMPLE_RATE.to_le_bytes()); // input sample rate
    v.extend_from_slice(&0i16.to_le_bytes()); // output gain
    v.push(0); // channel mapping family 0 (mono/stereo)
    v
}

/// `OpusTags` パケット。vendor 文字列 + コメント 0 件。
fn opus_tags() -> Vec<u8> {
    let vendor: &[u8] = b"moca-server";
    let mut v = Vec::with_capacity(20 + vendor.len());
    v.extend_from_slice(b"OpusTags");
    v.extend_from_slice(
        &u32::try_from(vendor.len())
            .expect("vendor is 11 bytes")
            .to_le_bytes(),
    );
    v.extend_from_slice(vendor);
    v.extend_from_slice(&0u32.to_le_bytes()); // user comment list length
    v
}

/// Ogg/Opus のストリーミングエンコーダ。
/// PCM をバッファし 960 サンプル溜まるごとに encode → Ogg ページを内部バッファへ書く。
/// `take_bytes` で完成済みバイト列を回収し、セグメント完了ごとにチャンク送信する。
pub struct OpusStream {
    encoder: Encoder,
    writer: PacketWriter<'static, Vec<u8>>,
    pre_skip: u16,
    /// 960 未満の端数サンプル (次の PCM / flush まで持ち越す)。
    pcm_buf: Vec<i16>,
    /// エンコード済み 48k サンプル累積 (= frames * 960)。granulepos の基準。
    encoded_samples: u64,
    /// 実入力サンプル累積 (末尾パディングを granulepos で打ち切るため)。
    input_samples: u64,
}

impl OpusStream {
    /// エンコーダを初期化し `OpusHead` (BOS) / `OpusTags` ページを内部バッファへ書き込む。
    pub fn new() -> Result<Self, String> {
        let encoder = Encoder::builder(SAMPLE_RATE, Channels::Mono, Application::Audio)
            .bitrate(Bitrate::Bits(BITRATE))
            .build()
            .map_err(|e| format!("opus encoder init: {e}"))?;
        // pre-skip はエンコーダの lookahead (符号化遅延)。デコーダが先頭で読み飛ばす量。
        let pre_skip = u16::try_from(encoder.lookahead())
            .map_err(|_| "opus lookahead exceeds u16".to_string())?;

        let mut writer = PacketWriter::new(Vec::new());
        writer
            .write_packet(opus_head(pre_skip), SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| format!("write OpusHead: {e}"))?;
        writer
            .write_packet(opus_tags(), SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| format!("write OpusTags: {e}"))?;

        Ok(Self {
            encoder,
            writer,
            pre_skip,
            pcm_buf: Vec::new(),
            encoded_samples: 0,
            input_samples: 0,
        })
    }

    /// s16le/mono PCM を投入する。960 サンプル揃うごとに 1 フレームずつエンコードして
    /// Ogg ページ (`EndPage`) を出力する。端数は次回まで持ち越す。
    pub fn push_pcm(&mut self, pcm: &[u8]) -> Result<(), String> {
        for ch in pcm.as_chunks::<2>().0 {
            self.pcm_buf.push(i16::from_le_bytes([ch[0], ch[1]]));
        }
        self.input_samples += (pcm.len() / 2) as u64;
        while self.pcm_buf.len() >= FRAME_SAMPLES {
            let frame: Vec<i16> = self.pcm_buf.drain(..FRAME_SAMPLES).collect();
            self.write_frame(&frame, false)?;
        }
        Ok(())
    }

    /// 1 フレーム (960 サンプル固定) をエンコードして Ogg ページへ書く。
    fn write_frame(&mut self, frame: &[i16], end: bool) -> Result<(), String> {
        let mut packet = vec![0u8; 4000];
        let len = self
            .encoder
            .encode(frame, &mut packet)
            .map_err(|e| format!("opus encode: {e}"))?;
        packet.truncate(len);
        self.encoded_samples += FRAME_SAMPLES as u64;
        let granule = if end {
            // 末尾パディングを打ち切る: 出力尺 = granule - pre_skip = input_samples。
            self.input_samples + u64::from(self.pre_skip)
        } else {
            self.encoded_samples
        };
        let info = if end {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::EndPage
        };
        self.writer
            .write_packet(packet, SERIAL, info, granule)
            .map_err(|e| format!("write opus page: {e}"))?;
        Ok(())
    }

    /// 残りの端数フレームをパディングして flush し、EOS ページを出力する。
    pub fn finish(&mut self) -> Result<(), String> {
        let target = self.input_samples + u64::from(self.pre_skip);

        // 出力すべきフレーム列を先に確定させ、最後の 1 枚を EndStream にする。
        let mut pending: Vec<Vec<i16>> = Vec::new();
        if !self.pcm_buf.is_empty() {
            let mut frame = std::mem::take(&mut self.pcm_buf);
            frame.resize(FRAME_SAMPLES, 0);
            pending.push(frame);
        }
        // pre-skip 分だけ末尾が足りない場合は無音フレームで埋める (granulepos で後で打ち切る)。
        let mut projected = self.encoded_samples + (pending.len() * FRAME_SAMPLES) as u64;
        while projected < target {
            pending.push(vec![0i16; FRAME_SAMPLES]);
            projected += FRAME_SAMPLES as u64;
        }
        // 端数がぴったりの場合でも EOS を載せる搬送フレームが要る (granulepos=target で全トリム)。
        if pending.is_empty() {
            pending.push(vec![0i16; FRAME_SAMPLES]);
        }

        let last = pending.len() - 1;
        for (i, frame) in pending.iter().enumerate() {
            self.write_frame(frame, i == last)?;
        }
        Ok(())
    }

    /// 完成済みバイト列を回収する (回収後は内部バッファを空にする)。
    pub fn take_bytes(&mut self) -> Vec<u8> {
        std::mem::take(self.writer.inner_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_oggs_and_opushead() {
        let mut s = OpusStream::new().unwrap();
        let bytes = s.take_bytes();
        assert_eq!(&bytes[0..4], b"OggS", "first page must be an Ogg page");
        // 先頭ページ本体に OpusHead マジックが含まれる。
        assert!(
            bytes.windows(8).any(|w| w == b"OpusHead"),
            "header page must carry OpusHead"
        );
        assert!(bytes.windows(8).any(|w| w == b"OpusTags"));
    }

    #[test]
    fn short_pcm_still_produces_a_page_and_eos() {
        // フェイク PCM 相当: 100 サンプル (< 960) でも flush で 1 ページ以上出る。
        let mut s = OpusStream::new().unwrap();
        let _ = s.take_bytes();
        s.push_pcm(&[0u8; 200]).unwrap(); // 100 samples
        s.finish().unwrap();
        let bytes = s.take_bytes();
        assert!(!bytes.is_empty(), "flush must emit an audio page");
        assert_eq!(&bytes[0..4], b"OggS");
    }

    /// ラウンドトリップ検証: 440Hz サイン波 1 秒をエンコードし、Ogg パケットを
    /// 取り出して ropus のデコーダで復号。フレーム数と信号エネルギーが保たれる
    /// ことを確認する (無音化・破損エンコードへの回帰ガード)。
    #[test]
    fn sine_roundtrip_preserves_duration_and_energy() {
        // ヘッダページも含めた完全なストリームを丸ごと読ませる (BOS が無いと
        // PacketReader は InvalidData で拒否する)。
        let mut s = OpusStream::new().unwrap();
        let pcm: Vec<u8> = (0..SAMPLE_RATE)
            .flat_map(|i| {
                let t = f64::from(i) / f64::from(SAMPLE_RATE);
                let v = ((t * 440.0 * std::f64::consts::TAU).sin() * 12000.0) as i16;
                v.to_le_bytes()
            })
            .collect();
        s.push_pcm(&pcm).unwrap();
        s.finish().unwrap();
        let bytes = s.take_bytes();

        // Ogg からパケットを取り出して先頭 2 つ (OpusHead/OpusTags) を飛ばす。
        let mut reader = ogg::PacketReader::new(std::io::Cursor::new(bytes));
        let mut decoder = ropus::Decoder::new(SAMPLE_RATE, Channels::Mono).unwrap();
        let mut out = vec![0i16; FRAME_SAMPLES];
        let mut frames = 0u32;
        let mut energy = 0f64;
        let mut headers = 0;
        while let Some(packet) = reader.read_packet().unwrap() {
            if headers < 2 {
                headers += 1;
                continue;
            }
            let n = decoder
                .decode(&packet.data, &mut out, ropus::DecodeMode::Normal)
                .unwrap();
            assert_eq!(n, FRAME_SAMPLES);
            frames += 1;
            energy += out.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>();
        }
        // 48000 サンプル + pre-skip パディング = 51〜52 フレーム程度。
        assert!(frames >= 50, "expected ~50 frames, got {frames}");
        let rms = (energy / (f64::from(frames) * f64::from(u32::try_from(FRAME_SAMPLES).unwrap())))
            .sqrt();
        assert!(rms > 4000.0, "decoded signal too quiet (rms={rms:.0})");
    }
}
