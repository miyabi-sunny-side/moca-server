// 台本 (script) JSON のスキーマ検証・平滑化・抽出。
// validate_script で範囲クランプ/未知軸除去、smooth_script で感情の指数移動平均、
// extract_json で LLM 出力から JSON 配列を切り出す。

use crate::error::AppError;
use serde_json::{json, Map, Value};

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

/// 台本の JSON Schema。`OpenAI` 互換 API の `response_format` に渡し、LLM の出力の形を縛る。
pub fn script_json_schema() -> Value {
    let integer = json!({ "type": "integer" });
    let axes: Map<String, Value> = EMOTIONS
        .iter()
        .map(|axis| ((*axis).to_string(), integer.clone()))
        .collect();
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "text": { "type": "string" },
                "emotion": { "type": "object", "properties": axes, "additionalProperties": false },
                "speed": integer,
                "pitch": integer,
                "pause": integer,
            },
            "required": ["text"],
            "additionalProperties": false,
        },
    })
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

/// 台本の text を順に繋げたものが、入力のセリフと同じ事を確かめる。
/// LLM が文字を化かしたり言い換えたりした台本を、そのまま読み上げないための検査。
/// 比べるのは読む文字 (文字と数字) だけで、空白・かぎ括弧・句読点の違いは許す。
pub fn ensure_text_kept(script: &Value, source: &str) -> Result<(), ScriptError> {
    let spoken: String = script
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|seg| seg.get("text").and_then(Value::as_str))
        .collect();
    if spoken_chars(&spoken) == spoken_chars(source) {
        Ok(())
    } else {
        Err(ScriptError(format!(
            "script text differs from the input: {}",
            truncate_chars(&spoken, 200)
        )))
    }
}

fn spoken_chars(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect()
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

    // --- script_json_schema ---

    // schema が許す項目を全て持つ台本が、検証でそのまま残る事 (schema と検証のずれを見つける)。
    #[test]
    fn schema_allows_exactly_what_validation_keeps() {
        let item = &script_json_schema()["items"];
        let fill = |props: &Value| -> Value {
            let props = props.as_object().expect("properties");
            props
                .iter()
                .map(|(key, prop)| {
                    let value = match prop["type"].as_str() {
                        Some("string") => json!("あ"),
                        Some("integer") => json!(50),
                        _ => Value::Null,
                    };
                    (key.clone(), value)
                })
                .collect::<Map<String, Value>>()
                .into()
        };
        let mut segment = fill(&item["properties"]);
        segment["emotion"] = fill(&item["properties"]["emotion"]["properties"]);

        let keys = |v: &Value| {
            v.as_object()
                .expect("object")
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut axes = keys(&segment["emotion"]);
        axes.sort();
        let mut known = EMOTIONS.map(str::to_string).to_vec();
        known.sort();
        assert_eq!(axes, known, "schema must offer every emotion axis");
        assert_eq!(
            keys(&segment).len(),
            5,
            "text, emotion, speed, pitch, pause"
        );
        assert_eq!(item["required"], json!(["text"]));

        let kept = validate_script(&json!([segment])).unwrap();
        assert_eq!(kept, json!([segment]));
    }

    // LLM は prompt の説明と同じ順 (text → emotion → speed → pitch → pause、感情軸は EMOTIONS の順) で
    // 書こうとする。schema の順が違うと、書けない項目で出力が崩れる (Gemma 4 12B で 40 回中 10 回)。
    #[test]
    fn schema_lists_properties_in_the_order_the_prompt_describes() {
        let sent = script_json_schema().to_string();
        let at = |key: &str| {
            sent.find(&format!("\"{key}\":"))
                .unwrap_or_else(|| panic!("{key} is missing: {sent}"))
        };
        let order = [
            "text", "emotion", "bosoboso", "doyaru", "honwaka", "angry", "teary",
        ];
        let order = order.iter().chain(&["speed", "pitch", "pause"]);
        let positions: Vec<usize> = order.map(|key| at(key)).collect();
        assert!(positions.is_sorted(), "order in {sent}");
    }

    // --- ensure_text_kept ---

    #[test]
    fn text_split_into_sentences_is_kept() {
        let script = json!([{ "text": "おはよう。" }, { "text": "今日は晴れ！" }]);
        assert!(ensure_text_kept(&script, "おはよう。今日は晴れ！").is_ok());
    }

    #[test]
    fn whitespace_differences_are_ignored() {
        let script = json!([{ "text": "Hello world." }, { "text": "二行目。" }]);
        assert!(ensure_text_kept(&script, " Hello  world.\n二行目。\n").is_ok());
    }

    #[test]
    fn bracket_and_punctuation_differences_are_ignored() {
        let script = json!([{ "text": "先輩、寝ましたか？" }, { "text": "寝たよ" }]);
        assert!(ensure_text_kept(&script, "「先輩、寝ましたか?」\n「寝たよ。」").is_ok());
    }

    #[test]
    fn garbled_character_is_rejected() {
        let script = json!([{ "text": "おはよウございます。" }]);
        let err = ensure_text_kept(&script, "おはようございます。").unwrap_err();
        assert!(err.0.contains("おはよウございます。"), "message: {}", err.0);
    }

    #[test]
    fn dropped_or_added_sentence_is_rejected() {
        let source = "一つ目。二つ目。";
        assert!(ensure_text_kept(&json!([{ "text": "一つ目。" }]), source).is_err());
        let added = json!([{ "text": "一つ目。" }, { "text": "二つ目。" }, { "text": "三つ目。" }]);
        assert!(ensure_text_kept(&added, source).is_err());
    }

    #[test]
    fn reordered_sentences_are_rejected() {
        let script = json!([{ "text": "二つ目。" }, { "text": "一つ目。" }]);
        assert!(ensure_text_kept(&script, "一つ目。二つ目。").is_err());
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
