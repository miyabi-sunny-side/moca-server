use super::*;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Mutex;

fn moras(kana: &str) -> Vec<String> {
    kana_to_moras(kana)
}

fn words(text: &str, pairs: &[(&str, &str)]) -> Vec<Word> {
    let output: Vec<Value> = pairs
        .iter()
        .map(|(word, yomi)| json!({ "word": word, "yomi": yomi }))
        .collect();
    parse_words(&Value::Array(output), text).expect("valid words")
}

const ICHINICHI: &str = "それでは今日も一日頑張りましょう。";

fn ichinichi_words() -> Vec<Word> {
    words(
        ICHINICHI,
        &[
            ("それでは", "それでは"),
            ("今日", "きょう"),
            ("も", "も"),
            ("一日", "いちにち"),
            ("頑張りましょう", "がんばりましょう"),
            ("。", ""),
        ],
    )
}

// --- phonemes_to_moras ---

#[test]
fn phonemes_become_kana_moras() {
    assert_eq!(
        phonemes_to_moras("s o r e d e w a ky o o m o ts u i t a ch i").concat(),
        "それでわきょおもついたち"
    );
    assert_eq!(
        phonemes_to_moras("sh u cl p a ts U sh i m a s U").concat(),
        "しゅっぱつします"
    );
    assert_eq!(phonemes_to_moras("k a N j i").concat(), "かんじ");
}

#[test]
fn a_consonant_without_a_vowel_still_becomes_a_mora() {
    assert_eq!(phonemes_to_moras("m a i ch n o").concat(), "まいちの");
    assert_eq!(phonemes_to_moras("d e s").concat(), "です");
}

#[test]
fn silence_and_unknown_tokens_are_skipped() {
    assert_eq!(phonemes_to_moras("sil a pau i ??? \n").concat(), "あい");
    assert!(phonemes_to_moras("").is_empty());
}

// --- kana_to_moras ---

#[test]
fn kana_splits_into_moras_and_katakana_becomes_hiragana() {
    assert_eq!(moras("きょう"), ["きょ", "う"]);
    assert_eq!(moras("カレー"), ["か", "れ", "ー"]);
    assert_eq!(moras("しゅっぱつ"), ["しゅ", "っ", "ぱ", "つ"]);
    assert_eq!(moras("読み、です。"), ["み", "で", "す"]);
}

// --- parse_words ---

#[test]
fn words_are_located_in_the_text() {
    let ws = ichinichi_words();
    let found: Vec<(&str, usize)> = ws.iter().map(|w| (w.surface.as_str(), w.start)).collect();
    assert_eq!(
        found,
        [
            ("それでは", 0),
            ("今日", 4),
            ("も", 6),
            ("一日", 7),
            ("頑張りましょう", 9)
        ]
    );
    assert!(ws[3].fixable, "一日 has kanji");
    assert!(!ws[0].fixable, "それでは is kana");
}

#[test]
fn punctuation_differences_do_not_break_the_match() {
    let ws = words(
        "買ってから、見た。",
        &[
            ("買って", "かって"),
            ("から", "から"),
            (",", ""),
            ("見た", "みた"),
            (".", ""),
        ],
    );
    let found: Vec<(&str, usize)> = ws.iter().map(|w| (w.surface.as_str(), w.start)).collect();
    assert_eq!(found, [("買って", 0), ("から", 3), ("見た", 6)]);
}

#[test]
fn changed_or_missing_words_are_rejected() {
    let bad = |pairs: &[(&str, &str)]| {
        let output: Vec<Value> = pairs
            .iter()
            .map(|(word, yomi)| json!({ "word": word, "yomi": yomi }))
            .collect();
        parse_words(&Value::Array(output), "今日は雨。")
    };
    assert!(
        bad(&[("今日", "きょう"), ("は", "は")]).is_err(),
        "雨 is missing"
    );
    assert!(
        bad(&[("今日", "きょう"), ("は", "は"), ("晴れ", "はれ")]).is_err(),
        "changed"
    );
    assert!(
        bad(&[
            ("今日", "きょう"),
            ("は", "は"),
            ("雨", "あめ"),
            ("だ", "だ")
        ])
        .is_err(),
        "added"
    );
    assert!(
        parse_words(&json!({ "word": "今日" }), "今日").is_err(),
        "not an array"
    );
}

#[test]
fn a_kanji_word_needs_a_kana_reading() {
    let out = json!([{ "word": "あの方", "yomi": "あの方" }]);
    assert!(parse_words(&out, "あの方").is_err());
    let out = json!([{ "word": "雨", "yomi": "" }]);
    assert!(parse_words(&out, "雨").is_err());
}

#[test]
fn latin_words_are_never_fixable_and_may_lack_a_reading() {
    let ws = words(
        "issueをHard2倍で",
        &[
            ("issue", "い슈"),
            ("を", "を"),
            ("Hard2倍", "はーどにばい"),
            ("で", "で"),
        ],
    );
    assert!(ws.iter().all(|w| !w.fixable), "no fixable word");
    assert!(ws[0].moras.is_empty(), "an unusable reading is dropped");
    assert_eq!(ws[2].moras.concat(), "はーどにばい");
}

#[test]
fn kana_words_use_their_own_spelling_as_the_reading() {
    let ws = words("カレーです", &[("カレー", "かれえ"), ("です", "ですう")]);
    assert_eq!(ws[0].moras.concat(), "かれー");
    assert_eq!(ws[1].moras.concat(), "です");
}

// --- compare ---

#[test]
fn a_misread_word_is_found_with_what_was_heard() {
    let heard =
        phonemes_to_moras("s o r e d e w a ky o o m o ts u i t a ch i g a N b a r i m a sh o o");
    let cmp = compare(&ichinichi_words(), &heard);
    assert_eq!(cmp.expected, "それではきょうもいちにちがんばりましょう");
    assert_eq!(cmp.heard, "それではきょうもついたちがんばりましょう");
    assert_eq!(cmp.mismatches.len(), 1);
    assert_eq!(cmp.mismatches[0].word, 3);
    assert_eq!(cmp.mismatches[0].heard, "ついたち");
}

#[test]
fn a_correct_reading_has_no_mismatch() {
    let heard =
        phonemes_to_moras("s o r e d e w a ky o o m o i ch i n i ch i g a N b a r i m a sh o o");
    let cmp = compare(&ichinichi_words(), &heard);
    assert!(
        cmp.mismatches.is_empty(),
        "は=わ, きょう=きょお, しょう=しょお"
    );
    assert_eq!(cmp.heard, cmp.expected);
}

#[test]
fn long_vowels_and_particles_written_differently_still_match() {
    let ws = words(
        "東京へ映画を",
        &[
            ("東京", "とうきょう"),
            ("へ", "へ"),
            ("映画", "えいが"),
            ("を", "を"),
        ],
    );
    let cmp = compare(&ws, &phonemes_to_moras("t o o ky o o e e e g a o"));
    assert!(cmp.mismatches.is_empty(), "heard: {}", cmp.heard);
    let cmp = compare(&ws, &phonemes_to_moras("t o u ky o u e e i g a o"));
    assert!(
        cmp.mismatches.is_empty(),
        "spelled-out vowels: {}",
        cmp.heard
    );
}

#[test]
fn extra_sounds_belong_to_the_misread_word_not_its_neighbour() {
    let ws = words(
        "実験を行った",
        &[
            ("実験", "じっけん"),
            ("を", "を"),
            ("行っ", "いっ"),
            ("た", "た"),
        ],
    );
    let cmp = compare(&ws, &phonemes_to_moras("j i cl k e N o o k o n a cl t a"));
    assert_eq!(cmp.mismatches.len(), 1, "only 行っ: {:?}", cmp.mismatches);
    assert_eq!(cmp.mismatches[0].word, 2);
    assert_eq!(cmp.mismatches[0].heard, "おこなっ");
    assert_eq!(cmp.heard, "じっけんをおこなった");
}

#[test]
fn heard_long_vowels_are_shown_in_ordinary_spelling() {
    let ws = words("紅葉の", &[("紅葉", "もみじ"), ("の", "の")]);
    let cmp = compare(&ws, &phonemes_to_moras("k o o y o o n o"));
    assert_eq!(cmp.mismatches[0].heard, "こうよう");
}

#[test]
fn differences_in_kana_and_latin_words_are_not_mismatches() {
    let ws = words(
        "キヴォトスのissueを見る",
        &[
            ("キヴォトス", "きぼとす"),
            ("の", "の"),
            ("issue", "いすー"),
            ("を", "を"),
            ("見る", "みる"),
        ],
    );
    let cmp = compare(
        &ws,
        &phonemes_to_moras("k i b o t o s u n o i sh u u o m i r u"),
    );
    assert!(cmp.mismatches.is_empty(), "{:?}", cmp.mismatches);
}

#[test]
fn sounds_of_a_word_with_no_reading_are_not_blamed_on_the_next_word() {
    let ws = words(
        "issue2倍",
        &[("issue", "い슈"), ("2", "に"), ("倍", "ばい")],
    );
    let cmp = compare(&ws, &phonemes_to_moras("i sh u u n i b a i"));
    assert!(cmp.mismatches.is_empty(), "{:?}", cmp.mismatches);
}

// --- respell ---

#[test]
fn only_the_chosen_word_is_respelled() {
    let ws = ichinichi_words();
    assert_eq!(
        respell(ICHINICHI, &ws, &[(3, Spelling::Hiragana)]),
        "それでは今日もいちにち頑張りましょう。"
    );
    assert_eq!(
        respell(
            ICHINICHI,
            &ws,
            &[(1, Spelling::Katakana), (3, Spelling::Hiragana)]
        ),
        "それではキョウもいちにち頑張りましょう。"
    );
    assert_eq!(respell(ICHINICHI, &ws, &[]), ICHINICHI);
}

#[test]
fn the_second_of_two_identical_words_can_be_respelled_alone() {
    let text = "東京に行って、実験を行った。";
    let ws = words(
        text,
        &[
            ("東京", "とうきょう"),
            ("に", "に"),
            ("行っ", "いっ"),
            ("て", "て"),
            ("実験", "じっけん"),
            ("を", "を"),
            ("行っ", "おこなっ"),
            ("た", "た"),
        ],
    );
    assert_eq!(
        respell(text, &ws, &[(6, Spelling::Hiragana)]),
        "東京に行って、実験をおこなった。"
    );
}

// --- choices ---

#[test]
fn a_fix_needs_the_same_answer_in_both_orders() {
    use Choice::{Both, First, Second};
    assert!(
        approves(Some(First), Some(Second)),
        "AI's reading both times"
    );
    assert!(!approves(Some(First), Some(First)), "always picks A");
    assert!(
        !approves(Some(Second), Some(First)),
        "heard reading both times"
    );
    assert!(!approves(Some(Both), Some(Second)), "either is fine");
    assert!(!approves(Some(First), None), "no answer");
    assert!(
        !approves(Some(Choice::Neither), Some(Second)),
        "both look wrong"
    );
}

#[test]
fn choice_is_read_from_json_or_bare_text() {
    assert_eq!(parse_choice(r#"{"answer":"A"}"#), Some(Choice::First));
    assert_eq!(parse_choice(" B\n"), Some(Choice::Second));
    assert_eq!(
        parse_choice(r#"{"answer":"どちらも正しい"}"#),
        Some(Choice::Both)
    );
    assert_eq!(
        parse_choice(r#"{"answer":"どちらも違う"}"#),
        Some(Choice::Neither)
    );
    assert_eq!(parse_choice("わかりません"), None);
}

#[test]
fn choice_prompt_marks_the_word_in_the_sentence() {
    let ws = ichinichi_words();
    let prompt = build_choice_prompt(ICHINICHI, &ws[3], "いちにち", "ついたち");
    assert!(
        prompt.contains("それでは今日も【一日】頑張りましょう。"),
        "{prompt}"
    );
    assert!(prompt.contains("A: いちにち\nB: ついたち"), "{prompt}");
}

// --- apply_reading / is_current / dismiss ---

fn fixed_line() -> LineReading {
    LineReading {
        checked_at: "2026-11-15T00:00:00.000Z".into(),
        secs: 1.0,
        segments: vec![
            SegmentReading {
                spoken: Some("それでは今日もいちにち頑張りましょう。".into()),
                status: Status::Fixed,
                ..SegmentReading::new(ICHINICHI)
            },
            SegmentReading::new("おわり。"),
        ],
    }
}

#[test]
fn a_fixed_segment_is_spoken_with_the_fixed_spelling() {
    let segments = vec![
        json!({ "text": ICHINICHI, "speed": 110 }),
        json!({ "text": "おわり。" }),
    ];
    let out = apply_reading(segments, Some(&fixed_line()));
    assert_eq!(
        out,
        [
            json!({ "text": "それでは今日もいちにち頑張りましょう。", "speed": 110 }),
            json!({ "text": "おわり。" }),
        ]
    );
}

#[test]
fn punctuation_only_differences_still_use_the_fix() {
    let segments = vec![json!({ "text": "それでは今日も一日頑張りましょう" })];
    let out = apply_reading(segments, Some(&fixed_line()));
    assert_eq!(out[0]["text"], "それでは今日もいちにち頑張りましょう。");
}

#[test]
fn changed_text_or_no_reading_is_spoken_as_it_is() {
    let changed = vec![json!({ "text": "それでは明日も一日頑張りましょう。" })];
    assert_eq!(apply_reading(changed.clone(), Some(&fixed_line())), changed);
    assert_eq!(apply_reading(changed.clone(), None), changed);
}

#[test]
fn a_line_is_current_only_while_its_segments_are_the_checked_ones() {
    let line = fixed_line();
    let same = vec![json!({ "text": ICHINICHI }), json!({ "text": "おわり。" })];
    assert!(is_current(&line, &same));
    assert!(!is_current(&line, &same[..1]), "a segment was removed");
    let edited = vec![
        json!({ "text": ICHINICHI }),
        json!({ "text": "おしまい。" }),
    ];
    assert!(!is_current(&line, &edited));
}

#[test]
fn dismissing_removes_the_fix_but_keeps_the_record() {
    let line = dismiss(fixed_line());
    assert_eq!(line.segments[0].spoken, None);
    assert_eq!(line.segments[0].status, Status::Dismissed);
    assert_eq!(line.segments[1].status, Status::Ok);
    let segments = vec![json!({ "text": ICHINICHI }), json!({ "text": "おわり。" })];
    assert!(is_current(&line, &segments), "not checked again by default");
    assert_eq!(apply_reading(segments.clone(), Some(&line)), segments);
}

#[test]
fn a_dismissed_fix_stays_dismissed_when_checked_again() {
    let old = dismiss(fixed_line());
    // 確かめ直すと同じ直しがまた出るが、外した行には掛けない。
    let again = keep_dismissals(fixed_line(), Some(&old));
    assert_eq!(again.segments[0].spoken, None);
    assert_eq!(again.segments[0].status, Status::Dismissed);
    assert_eq!(again.segments[1].status, Status::Ok);

    // 外していない行と、文が変わった行は、新しい結果のまま。
    assert_eq!(
        keep_dismissals(fixed_line(), Some(&fixed_line())),
        fixed_line()
    );
    assert_eq!(keep_dismissals(fixed_line(), None), fixed_line());
    let mut other = old.clone();
    other.segments[0].text = "別の文。".into();
    assert_eq!(keep_dismissals(fixed_line(), Some(&other)), fixed_line());
}

#[test]
fn line_reading_round_trips_through_json() {
    let line = fixed_line();
    let stored = serde_json::to_string(&line).unwrap();
    assert_eq!(LineReading::parse(Some(&stored)), Some(line));
    assert_eq!(LineReading::parse(Some("not json")), None);
    assert_eq!(LineReading::parse(None), None);
}

// --- check_segment (LLM と合成・聞き取りは台本どおりに答える偽物) ---

struct Fake {
    answers: Mutex<VecDeque<Result<String, String>>>,
    sounds: Mutex<VecDeque<Result<String, String>>>,
    asked: Mutex<Vec<String>>,
    spoken: Mutex<Vec<String>>,
}

impl Fake {
    fn new(answers: &[Result<&str, &str>], sounds: &[Result<&str, &str>]) -> Self {
        let own = |items: &[Result<&str, &str>]| {
            items
                .iter()
                .map(|r| r.map(str::to_string).map_err(str::to_string))
                .collect()
        };
        Self {
            answers: Mutex::new(own(answers)),
            sounds: Mutex::new(own(sounds)),
            asked: Mutex::new(Vec::new()),
            spoken: Mutex::new(Vec::new()),
        }
    }

    fn spoken(&self) -> Vec<String> {
        self.spoken.lock().unwrap().clone()
    }

    fn asked(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

#[async_trait]
impl Tools for Fake {
    async fn ask(&self, prompt: &str, _format: &ResponseFormat) -> Result<String, String> {
        self.asked.lock().unwrap().push(prompt.to_string());
        let next = self.answers.lock().unwrap().pop_front();
        next.unwrap_or_else(|| Err("no more answers".into()))
    }

    async fn hear(&self, segment: &Value) -> Result<String, String> {
        let text = segment["text"].as_str().unwrap_or("").to_string();
        self.spoken.lock().unwrap().push(text);
        let next = self.sounds.lock().unwrap().pop_front();
        next.unwrap_or_else(|| Err("no more sounds".into()))
    }
}

const WORDS_ICHINICHI: &str = r#"[{"word":"それでは","yomi":"それでは"},{"word":"今日","yomi":"きょう"},{"word":"も","yomi":"も"},{"word":"一日","yomi":"いちにち"},{"word":"頑張りましょう","yomi":"がんばりましょう"},{"word":"。","yomi":""}]"#;
const HEARD_TSUITACHI: &str = "s o r e d e w a ky o o m o ts u i t a ch i g a N b a r i m a sh o o";
const HEARD_ICHINICHI: &str = "s o r e d e w a ky o o m o i ch i n i ch i g a N b a r i m a sh o o";
const PICK_A: &str = r#"{"answer":"A"}"#;
const PICK_B: &str = r#"{"answer":"B"}"#;

#[tokio::test]
async fn a_misreading_is_fixed_in_hiragana_and_confirmed_by_ear() {
    let fake = Fake::new(
        &[
            Ok(WORDS_ICHINICHI),
            Ok(WORDS_ICHINICHI),
            Ok(PICK_A),
            Ok(PICK_B),
        ],
        &[Ok(HEARD_TSUITACHI), Ok(HEARD_ICHINICHI)],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI, "speed": 110 })).await;
    assert_eq!(out.status, Status::Fixed);
    assert_eq!(out.text, ICHINICHI);
    assert_eq!(
        out.spoken.as_deref(),
        Some("それでは今日もいちにち頑張りましょう。")
    );
    assert_eq!(
        out.expected.as_deref(),
        Some("それではきょうもいちにちがんばりましょう")
    );
    assert_eq!(
        out.heard.as_deref(),
        Some("それではきょうもついたちがんばりましょう")
    );
    assert_eq!(out.words.len(), 1);
    assert_eq!(out.words[0].surface, "一日");
    assert_eq!(out.words[0].heard, "ついたち");
    assert_eq!(out.words[0].action, Action::Fixed);
    assert_eq!(
        fake.spoken(),
        [ICHINICHI, "それでは今日もいちにち頑張りましょう。"]
    );
}

#[tokio::test]
async fn a_line_read_correctly_is_left_alone_without_extra_questions() {
    let fake = Fake::new(&[Ok(WORDS_ICHINICHI)], &[Ok(HEARD_ICHINICHI)]);
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Ok);
    assert_eq!(out.spoken, None);
    assert!(out.words.is_empty());
    assert_eq!(fake.asked(), 1, "no choice questions");
    assert_eq!(fake.spoken().len(), 1, "no second synthesis");
}

#[tokio::test]
async fn the_heard_reading_is_kept_when_the_ai_does_not_insist() {
    // 2 回目 (順序を入れ替えた質問) でも A と答える = 並び順で選んでいるだけ。
    let fake = Fake::new(
        &[
            Ok(WORDS_ICHINICHI),
            Ok(WORDS_ICHINICHI),
            Ok(PICK_A),
            Ok(PICK_A),
        ],
        &[Ok(HEARD_TSUITACHI)],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Kept);
    assert_eq!(out.spoken, None);
    assert_eq!(out.words[0].action, Action::Kept);
    assert_eq!(fake.spoken().len(), 1, "nothing is synthesized again");
}

#[tokio::test]
async fn a_reading_the_ai_does_not_repeat_is_not_trusted() {
    // 聞き直すと別の読みを返す = 当てずっぽう。2 択を尋ねるまでもなく変えない。
    let other = WORDS_ICHINICHI.replace("いちにち", "ひとひ");
    let fake = Fake::new(
        &[Ok(WORDS_ICHINICHI), Ok(other.as_str())],
        &[Ok(HEARD_TSUITACHI)],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Kept);
    assert_eq!(out.spoken, None);
    assert_eq!(out.words[0].action, Action::Kept);
    assert_eq!(fake.asked(), 2, "no choice questions");
    assert_eq!(fake.spoken().len(), 1, "nothing is synthesized again");
}

#[tokio::test]
async fn no_second_answer_means_no_fix() {
    let fake = Fake::new(
        &[Ok(WORDS_ICHINICHI), Err("timeout"), Err("timeout")],
        &[Ok(HEARD_TSUITACHI)],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Kept, "the first check itself succeeded");
    assert_eq!(out.spoken, None);
}

#[tokio::test]
async fn katakana_is_tried_when_hiragana_is_still_misread() {
    let fake = Fake::new(
        &[
            Ok(WORDS_ICHINICHI),
            Ok(WORDS_ICHINICHI),
            Ok(PICK_A),
            Ok(PICK_B),
        ],
        &[
            Ok(HEARD_TSUITACHI),
            Ok(HEARD_TSUITACHI),
            Ok(HEARD_ICHINICHI),
        ],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Fixed);
    assert_eq!(
        out.spoken.as_deref(),
        Some("それでは今日もイチニチ頑張りましょう。")
    );
    assert_eq!(fake.spoken().len(), 3);
}

#[tokio::test]
async fn it_gives_up_after_both_spellings_and_changes_nothing() {
    let fake = Fake::new(
        &[
            Ok(WORDS_ICHINICHI),
            Ok(WORDS_ICHINICHI),
            Ok(PICK_A),
            Ok(PICK_B),
        ],
        &[
            Ok(HEARD_TSUITACHI),
            Ok(HEARD_TSUITACHI),
            Ok(HEARD_TSUITACHI),
            Ok(HEARD_ICHINICHI),
        ],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Unresolved);
    assert_eq!(out.spoken, None);
    assert_eq!(out.words[0].action, Action::Unresolved);
    assert_eq!(
        fake.spoken().len(),
        3,
        "original, hiragana, katakana — then stop"
    );
}

#[tokio::test]
async fn a_fix_that_breaks_another_word_is_not_kept() {
    // ひらがなにしたら「今日」が別の読みになった。カタカナでは壊れない。
    let broke = "s o r e d e w a k o N n i ch i m o i ch i n i ch i g a N b a r i m a sh o o";
    let fake = Fake::new(
        &[
            Ok(WORDS_ICHINICHI),
            Ok(WORDS_ICHINICHI),
            Ok(PICK_A),
            Ok(PICK_B),
        ],
        &[Ok(HEARD_TSUITACHI), Ok(broke), Ok(HEARD_ICHINICHI)],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Fixed);
    assert_eq!(
        out.spoken.as_deref(),
        Some("それでは今日もイチニチ頑張りましょう。")
    );
}

#[tokio::test]
async fn one_word_can_be_fixed_while_another_is_given_up() {
    let text = "一日と生物。";
    let words = r#"[{"word":"一日","yomi":"いちにち"},{"word":"と","yomi":"と"},{"word":"生物","yomi":"なまもの"},{"word":"。","yomi":""}]"#;
    let both_wrong = "ts u i t a ch i t o s e e b u ts u";
    let first_right = "i ch i n i ch i t o s e e b u ts u";
    let fake = Fake::new(
        &[
            Ok(words),
            Ok(words),
            Ok(PICK_A),
            Ok(PICK_B),
            Ok(PICK_A),
            Ok(PICK_B),
        ],
        &[
            Ok(both_wrong),
            Ok(first_right),
            Ok(first_right),
            Ok(first_right),
        ],
    );
    let out = check_segment(&fake, &json!({ "text": text })).await;
    assert_eq!(out.status, Status::Fixed);
    assert_eq!(out.spoken.as_deref(), Some("いちにちと生物。"));
    let actions: Vec<Action> = out.words.iter().map(|w| w.action).collect();
    assert_eq!(actions, [Action::Fixed, Action::Unresolved]);
    assert_eq!(
        fake.spoken(),
        [
            text,
            "いちにちとなまもの。",
            "いちにちとナマモノ。",
            "いちにちと生物。"
        ],
        "the kept spelling is the one confirmed last"
    );
}

#[tokio::test]
async fn a_segment_with_nothing_to_read_is_fine_without_asking() {
    let fake = Fake::new(&[], &[]);
    let out = check_segment(&fake, &json!({ "text": "……！？" })).await;
    assert_eq!(out.status, Status::Ok);
    assert_eq!(fake.asked(), 0);
    assert!(fake.spoken().is_empty());
}

#[tokio::test]
async fn an_unusable_reading_is_asked_once_more() {
    let fake = Fake::new(
        &[Ok("すみません、できません"), Ok(WORDS_ICHINICHI)],
        &[Ok(HEARD_ICHINICHI)],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Ok);
    assert_eq!(fake.asked(), 2);
}

#[tokio::test]
async fn a_dead_ai_or_ear_leaves_the_line_unchanged() {
    let fake = Fake::new(&[Err("connection refused"), Err("connection refused")], &[]);
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Failed);
    assert_eq!(out.spoken, None);
    assert!(out
        .error
        .as_deref()
        .unwrap_or("")
        .contains("connection refused"));
    assert!(
        fake.spoken().is_empty(),
        "nothing is synthesized without a reading"
    );

    let fake = Fake::new(&[Ok(WORDS_ICHINICHI)], &[Err("listen command failed")]);
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Failed);
    assert!(out
        .error
        .as_deref()
        .unwrap_or("")
        .contains("listen command failed"));
}

#[tokio::test]
async fn an_ear_that_dies_mid_fix_leaves_the_line_unchanged() {
    let fake = Fake::new(
        &[
            Ok(WORDS_ICHINICHI),
            Ok(WORDS_ICHINICHI),
            Ok(PICK_A),
            Ok(PICK_B),
        ],
        &[Ok(HEARD_TSUITACHI), Err("listen command failed")],
    );
    let out = check_segment(&fake, &json!({ "text": ICHINICHI })).await;
    assert_eq!(out.status, Status::Failed);
    assert_eq!(out.spoken, None);
}

#[test]
fn line_status_shows_the_most_notable_segment() {
    let with = |statuses: &[Status]| LineReading {
        checked_at: String::new(),
        secs: 0.0,
        segments: statuses
            .iter()
            .map(|s| SegmentReading {
                status: *s,
                ..SegmentReading::new("x")
            })
            .collect(),
    };
    assert_eq!(
        with(&[Status::Ok, Status::Fixed, Status::Kept]).status(),
        Status::Fixed
    );
    assert_eq!(with(&[Status::Ok, Status::Failed]).status(), Status::Failed);
    assert_eq!(
        with(&[Status::Kept, Status::Unresolved]).status(),
        Status::Unresolved
    );
    assert_eq!(with(&[Status::Ok, Status::Ok]).status(), Status::Ok);
}
