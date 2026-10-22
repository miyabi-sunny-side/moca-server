// 台本 (script) JSON のスキーマ検証・平滑化・抽出。
// validate_script で範囲クランプ/未知軸除去、smooth_script で感情の指数移動平均、
// extract_json で LLM 出力から JSON 配列を切り出す。

use crate::error::AppError;
use serde_json::{Map, Value};

pub const EMOTIONS: [&str; 5] = ["bosoboso", "doyaru", "honwaka", "angry", "teary"];

/// 台本検証エラー。`AppError::BadRequest` にマップされる (TS の `ScriptError` 相当)。
#[derive(Debug)]
pub struct ScriptError(pub String);

impl From<ScriptError> for AppError {
    fn from(e: ScriptError) -> Self {
        AppError::BadRequest(e.0)
    }
}

fn clamp(n: f64, min: f64, max: f64) -> f64 {
    n.max(min).min(max)
}

// JS の Math.round (0.5 は +∞ 方向に丸める) を再現する。
fn js_round(n: f64) -> f64 {
    (n + 0.5).floor()
}

fn clamped_int(n: f64, min: f64, max: f64) -> Value {
    Value::from(clamp(js_round(n), min, max) as i64)
}

// 不正な構造は ScriptError を返し、範囲外の数値はクランプする。
// ({"segments":[...]} で包んできた場合も黙って剥がす。)
pub fn validate_script(input: &Value) -> Result<Value, ScriptError> {
    let raw = if let Some(arr) = input.as_array() {
        arr
    } else if let Some(arr) = input.get("segments").and_then(Value::as_array) {
        arr
    } else {
        return Err(ScriptError("script must be an array of segments".into()));
    };

    let mut segments: Vec<Value> = Vec::with_capacity(raw.len());
    for (i, seg) in raw.iter().enumerate() {
        let text = seg
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let Some(text) = text else {
            return Err(ScriptError(format!(
                "segments[{i}]: \"text\" must be a non-empty string"
            )));
        };

        let mut out = Map::new();
        out.insert("text".into(), Value::from(text));

        if let Some(emotion) = seg.get("emotion") {
            if !emotion.is_null() {
                let obj = emotion.as_object().ok_or_else(|| {
                    ScriptError(format!("segments[{i}]: \"emotion\" must be an object"))
                })?;
                let mut emo = Map::new();
                for (key, value) in obj {
                    if !EMOTIONS.contains(&key.as_str()) {
                        continue; // 未知の感情軸は黙って捨てる
                    }
                    if let Some(n) = value.as_f64() {
                        emo.insert(key.clone(), clamped_int(n, 0.0, 100.0));
                    }
                }
                if !emo.is_empty() {
                    out.insert("emotion".into(), Value::Object(emo));
                }
            }
        }

        if let Some(n) = seg.get("speed").and_then(Value::as_f64) {
            out.insert("speed".into(), clamped_int(n, 50.0, 200.0));
        }
        if let Some(n) = seg.get("pitch").and_then(Value::as_f64) {
            out.insert("pitch".into(), clamped_int(n, -300.0, 300.0));
        }
        if let Some(n) = seg.get("pause").and_then(Value::as_f64) {
            out.insert("pause".into(), clamped_int(n, 0.0, 10_000.0));
        }

        segments.push(Value::Object(out));
    }

    if segments.is_empty() {
        return Err(ScriptError("script has no segments".into()));
    }
    Ok(Value::Array(segments))
}

// script (配列) を検証して保存用文字列にする。不正なら ScriptError を返す。
pub fn serialize_script(input: &Value) -> Result<String, ScriptError> {
    let script = validate_script(input)?;
    Ok(script.to_string())
}

// JS の Math.round (0.5 は +∞ 方向) で丸めた i64。
fn round_i64(n: f64) -> i64 {
    js_round(n) as i64
}

/// 感情の急変をなじませる指数移動平均。
///   実効値[i] = (1 - w) × 自身の値 + w × 実効値[i-1]   (w = i==0 ? 0 : carry)
/// 感情軸は未指定=0、speed=100 / pitch=0 が基準値。carry<=0 で無変換。
/// pause は EMA 対象外でそのまま透過する。
pub fn smooth_script(script: &Value, carry: f64) -> Value {
    let Some(segments) = script.as_array() else {
        return script.clone();
    };
    if carry <= 0.0 {
        return script.clone();
    }

    let mut prev = [0.0f64; 5]; // EMOTIONS 固定順
    let mut prev_speed = 100.0f64;
    let mut prev_pitch = 0.0f64;

    let out: Vec<Value> = segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let w = if i == 0 { 0.0 } else { carry };
            let mut obj = Map::new();
            if let Some(text) = seg.get("text").and_then(Value::as_str) {
                obj.insert("text".into(), Value::from(text));
            }

            // 全 5 軸を EMOTIONS の固定順で毎回 EMA 更新 (未指定軸は 0 として prev も更新)。
            let mut emo = Map::new();
            for (idx, axis) in EMOTIONS.iter().enumerate() {
                let own = seg
                    .get("emotion")
                    .and_then(|e| e.get(*axis))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let v = (1.0 - w) * own + w * prev[idx];
                prev[idx] = v;
                let rounded = round_i64(v);
                if rounded > 0 {
                    emo.insert((*axis).to_string(), Value::from(rounded));
                }
            }
            if !emo.is_empty() {
                obj.insert("emotion".into(), Value::Object(emo));
            }

            let own_speed = seg.get("speed").and_then(Value::as_f64).unwrap_or(100.0);
            let own_pitch = seg.get("pitch").and_then(Value::as_f64).unwrap_or(0.0);
            prev_speed = (1.0 - w) * own_speed + w * prev_speed;
            prev_pitch = (1.0 - w) * own_pitch + w * prev_pitch;
            if round_i64(prev_speed) != 100 {
                obj.insert("speed".into(), Value::from(round_i64(prev_speed)));
            }
            if round_i64(prev_pitch) != 0 {
                obj.insert("pitch".into(), Value::from(round_i64(prev_pitch)));
            }

            if let Some(pause) = seg.get("pause") {
                obj.insert("pause".into(), pause.clone());
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(out)
}

/// 文字列を char 境界に丸めて先頭 n バイトまで切り出す (日本語途中切りパニック回避)。
fn truncate_chars(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// LLM 応答から JSON 部分 (配列 or オブジェクト) を取り出す。
pub fn extract_json(text: &str) -> Result<Value, ScriptError> {
    // JSON 構造文字は ASCII なのでバイトオフセットで char 境界問題は起きない。
    let starts: Vec<usize> = [text.find('['), text.find('{')]
        .into_iter()
        .flatten()
        .collect();
    let start = starts.iter().min().copied();
    let end = text.rfind(']').max(text.rfind('}'));

    let (start, end) = match (start, end) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => {
            return Err(ScriptError(format!(
                "no JSON found in: {}",
                truncate_chars(text, 200)
            )))
        }
    };

    serde_json::from_str(&text[start..=end]).map_err(|_| {
        ScriptError(format!(
            "invalid JSON in analyzer output: {}",
            truncate_chars(text, 200)
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_script_passes_through() {
        let out = validate_script(&json!([
            { "text": "こんにちは。", "emotion": { "honwaka": 60 }, "speed": 110, "pause": 300 }
        ]))
        .unwrap();
        assert_eq!(
            out,
            json!([{ "text": "こんにちは。", "emotion": { "honwaka": 60 }, "speed": 110, "pause": 300 }])
        );
    }

    #[test]
    fn unwraps_segments_wrapper() {
        let out = validate_script(&json!({ "segments": [{ "text": "a" }] })).unwrap();
        assert_eq!(out, json!([{ "text": "a" }]));
    }

    #[test]
    fn clamps_out_of_range_numbers() {
        let out = validate_script(&json!([
            { "text": "a", "emotion": { "angry": 150 }, "speed": 10, "pitch": 999, "pause": -5 }
        ]))
        .unwrap();
        assert_eq!(
            out,
            json!([{ "text": "a", "emotion": { "angry": 100 }, "speed": 50, "pitch": 300, "pause": 0 }])
        );
    }

    #[test]
    fn drops_unknown_emotion_axes() {
        let out =
            validate_script(&json!([{ "text": "a", "emotion": { "happy": 50, "doyaru": 30 } }]))
                .unwrap();
        assert_eq!(out[0]["emotion"], json!({ "doyaru": 30 }));
    }

    #[test]
    fn rejects_invalid_scripts() {
        assert!(validate_script(&json!({})).is_err());
        assert!(validate_script(&json!("text")).is_err());
        assert!(validate_script(&json!([])).is_err());
        assert!(validate_script(&json!([{ "emotion": { "angry": 1 } }])).is_err());
        assert!(validate_script(&json!([{ "text": "  " }])).is_err());
    }

    // --- smooth_script ---

    #[test]
    fn smooth_emotion_decays_by_carry() {
        let out = smooth_script(
            &json!([{ "text": "a", "emotion": { "angry": 90 } }, { "text": "b" }, { "text": "c" }]),
            1.0 / 3.0,
        );
        assert_eq!(out[0]["emotion"], json!({ "angry": 90 }));
        assert_eq!(out[1]["emotion"], json!({ "angry": 30 }));
        assert_eq!(out[2]["emotion"], json!({ "angry": 10 }));
    }

    #[test]
    fn smooth_blends_bosoboso_into_honwaka() {
        let out = smooth_script(
            &json!([
                { "text": "疲れた……。", "emotion": { "bosoboso": 70 }, "speed": 85 },
                { "text": "明日は休みます。", "emotion": { "honwaka": 75 } }
            ]),
            1.0 / 3.0,
        );
        assert_eq!(out[1]["emotion"], json!({ "honwaka": 50, "bosoboso": 23 }));
        assert_eq!(out[1]["speed"], json!(95));
    }

    #[test]
    fn smooth_carry_zero_is_identity() {
        let input = json!([{ "text": "a", "emotion": { "angry": 90 } }, { "text": "b" }]);
        assert_eq!(smooth_script(&input, 0.0), input);
    }

    #[test]
    fn smooth_pause_passes_through() {
        let out = smooth_script(
            &json!([{ "text": "a", "pause": 300 }, { "text": "b" }]),
            1.0 / 3.0,
        );
        assert_eq!(out[0]["pause"], json!(300));
        assert_eq!(out[1].get("pause"), None);
    }

    // --- extract_json ---

    #[test]
    fn extract_json_from_code_fence() {
        let out = extract_json("台本です:\n```json\n[{\"text\":\"a\"}]\n```").unwrap();
        assert_eq!(out, json!([{ "text": "a" }]));
    }

    #[test]
    fn extract_json_object_form() {
        let out = extract_json("{\"segments\":[{\"text\":\"a\"}]}").unwrap();
        assert_eq!(out, json!({ "segments": [{ "text": "a" }] }));
    }

    #[test]
    fn extract_json_missing_is_error() {
        assert!(extract_json("ごめんなさい、できません").is_err());
    }
}
