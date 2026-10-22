// WAV ヘッダ生成と RIFF チャンクパーサ。合成出力 (48kHz/16bit/mono) の PCM 抽出に使う。

/// PCM フォーマット記述子。
pub struct WavFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

/// 宮舞モカの出力フォーマット (実測: `pcm_s16le` / 48kHz / mono)。
pub const MOCA_FORMAT: WavFormat = WavFormat {
    sample_rate: 48000,
    channels: 1,
    bits_per_sample: 16,
};

/// ストリーミング配信用の 44 バイト WAV ヘッダを生成する。
/// 合成完了まで全体長が分からないため、RIFF/data のサイズには
/// 0xFFFFFFFF (不定長) を入れる。ffplay やブラウザはこれで逐次再生できる。
pub fn wav_header(format: &WavFormat) -> Vec<u8> {
    let block_align = format.channels * (format.bits_per_sample / 8);
    let byte_rate = format.sample_rate * u32::from(block_align);

    let mut b = Vec::with_capacity(44);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // 全体サイズ: 不定
    b.extend_from_slice(b"WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&16u32.to_le_bytes()); // fmt チャンクサイズ
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&format.channels.to_le_bytes());
    b.extend_from_slice(&format.sample_rate.to_le_bytes());
    b.extend_from_slice(&byte_rate.to_le_bytes());
    b.extend_from_slice(&block_align.to_le_bytes());
    b.extend_from_slice(&format.bits_per_sample.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // データサイズ: 不定
    b
}

/// RIFF/WAVE を fmt/data チャンクとしてパースし data 部 (PCM) を取り出す。
/// 44 バイト決め打ちはしない — 実機 voicepeak の出力は LIST 等が挟まりうるため、
/// チャンクを正しく辿って data を探す。
pub fn extract_pcm(wav: &[u8]) -> Result<Vec<u8>, String> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut pos = 12;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let size =
            u32::from_le_bytes([wav[pos + 4], wav[pos + 5], wav[pos + 6], wav[pos + 7]]) as usize;
        let data_start = pos + 8;
        if id == b"data" {
            let end = data_start.saturating_add(size).min(wav.len());
            return Ok(wav[data_start..end].to_vec());
        }
        // RIFF チャンクはワード境界揃え: 奇数長は 1 バイトのパディングが入る。
        pos = data_start + size + (size & 1);
    }
    Err("no data chunk found".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_44_bytes_with_riff_wave() {
        let h = wav_header(&MOCA_FORMAT);
        assert_eq!(h.len(), 44);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..12], b"WAVE");
        assert_eq!(&h[12..16], b"fmt ");
        assert_eq!(&h[36..40], b"data");
        // channels / sampleRate / bitsPerSample を検証。
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 1);
        assert_eq!(u32::from_le_bytes([h[24], h[25], h[26], h[27]]), 48000);
        assert_eq!(u16::from_le_bytes([h[34], h[35]]), 16);
        // ストリーミング用の不定サイズ。
        assert_eq!(&h[4..8], &0xFFFF_FFFFu32.to_le_bytes());
        assert_eq!(&h[40..44], &0xFFFF_FFFFu32.to_le_bytes());
    }

    fn chunk(id: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&id);
        c.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0); // word alignment padding
        }
        c
    }

    #[test]
    fn extract_pcm_skips_leading_chunks() {
        let pcm: Vec<u8> = (0..200u8).collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&0u32.to_le_bytes()); // size (ignored)
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(&chunk(*b"fmt ", &[0u8; 16]));
        // LIST チャンク (奇数長でパディングが入る) を data の前に挟む。
        wav.extend_from_slice(&chunk(*b"LIST", b"INFOxyz"));
        wav.extend_from_slice(&chunk(*b"data", &pcm));
        assert_eq!(extract_pcm(&wav).unwrap(), pcm);
    }

    #[test]
    fn extract_pcm_rejects_non_wave() {
        assert!(extract_pcm(b"not a wav").is_err());
        assert!(extract_pcm(b"RIFF____JUNK").is_err());
    }
}
