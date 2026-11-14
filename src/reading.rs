// 読みの確認: 生成 AI が決めた読みと、合成した音声から聞き取った読みを比べ、
// 読み違えた語だけを平仮名・カタカナに直す。ここは判断だけを持ち、生成 AI への問い合わせと
// 合成・聞き取りは `Tools` の向こうに置く。
//
// 直し過ぎを抑える決まり:
//   - 直すのは漢字・数字を含む語だけ (かな・英字の語は辞書と VOICEPEAK の読みのまま)。
//   - 生成 AI に同じ問いをもう 1 度尋ね、同じ読みを返さなかった語は直さない (当てずっぽうを信じない)。
//   - 食い違った語は、生成 AI に 2 つの読みを見せて選び直させる。並び順を入れ替えた 2 回とも
//     自分の読みを選んだ時だけ直す。
//   - 直した表記は合成し直して聞き、読みが合った時だけ残す。合わなければ元の表記に戻す。

use crate::analyze::ResponseFormat;
use crate::script::spoken_chars;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// 直した表記を合成し直して確かめる回数の上限 (平仮名 → カタカナ → 諦めた語を戻して確認)。
const MAX_ROUNDS: usize = 3;
/// 読みを生成 AI に問い合わせる回数の上限。
const MAX_READING_ATTEMPTS: usize = 2;

const VOWEL_KANA: [&str; 5] = ["あ", "い", "う", "え", "お"];
// 段ごとのかな。モーラの母音を調べるのに使う。
const VOWEL_ROWS: [&str; 5] = [
    "あかがさざただなはばぱまやらわぁゃゎ",
    "いきぎしじちぢにひびぴみりぃ",
    "うくぐすずつづぬふぶぷむゆるぅゅゔ",
    "えけげせぜてでねへべぺめれぇ",
    "おこごそぞとどのほぼぽもよろをぉょ",
];
const SMALL_KANA: &str = "ゃゅょぁぃぅぇぉゎ";

// ---------------------------------------------------------------------------
// かな・音素 → モーラ
// ---------------------------------------------------------------------------

fn is_kana_char(c: char) -> bool {
    matches!(c, 'ぁ'..='ゖ' | 'ァ'..='ヺ' | 'ー')
}

fn to_hiragana(c: char) -> char {
    match c {
        'ァ'..='ヶ' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn to_katakana(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'ぁ'..='ゖ' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// かなをモーラに分ける。カタカナは平仮名にし、小書きのかなは前のかなに付ける。かな以外は捨てる。
pub fn kana_to_moras(kana: &str) -> Vec<String> {
    let mut moras: Vec<String> = Vec::new();
    for c in kana.chars().filter(|c| is_kana_char(*c)).map(to_hiragana) {
        match moras.last_mut() {
            Some(last) if SMALL_KANA.contains(c) => last.push(c),
            _ => moras.push(c.to_string()),
        }
    }
    moras
}

/// 子音と母音 (あ=0 … お=4) から、かな 1 モーラを作る。
fn syllable(consonant: &str, vowel: usize) -> Option<String> {
    let row = |kana: &str| kana.chars().nth(vowel).map(String::from);
    let list = |kana: [&str; 5]| Some(kana[vowel].to_string());
    let palatal = |head: &str| Some(format!("{head}{}", ["ゃ", "ぃ", "ゅ", "ぇ", "ょ"][vowel]));
    match consonant {
        "k" => row("かきくけこ"),
        "g" => row("がぎぐげご"),
        "n" => row("なにぬねの"),
        "h" => row("はひふへほ"),
        "b" => row("ばびぶべぼ"),
        "p" => row("ぱぴぷぺぽ"),
        "m" => row("まみむめも"),
        "r" => row("らりるれろ"),
        "s" => list(["さ", "すぃ", "す", "せ", "そ"]),
        "z" => list(["ざ", "ずぃ", "ず", "ぜ", "ぞ"]),
        "t" => list(["た", "てぃ", "とぅ", "て", "と"]),
        "d" => list(["だ", "でぃ", "どぅ", "で", "ど"]),
        "y" => list(["や", "い", "ゆ", "いぇ", "よ"]),
        "w" => list(["わ", "うぃ", "う", "うぇ", "うぉ"]),
        "sh" => list(["しゃ", "し", "しゅ", "しぇ", "しょ"]),
        "j" => list(["じゃ", "じ", "じゅ", "じぇ", "じょ"]),
        "ch" => list(["ちゃ", "ち", "ちゅ", "ちぇ", "ちょ"]),
        "ts" => list(["つぁ", "つぃ", "つ", "つぇ", "つぉ"]),
        "f" => list(["ふぁ", "ふぃ", "ふ", "ふぇ", "ふぉ"]),
        "v" => list(["ゔぁ", "ゔぃ", "ゔ", "ゔぇ", "ゔぉ"]),
        "kw" => list(["くゎ", "くぃ", "く", "くぇ", "くぉ"]),
        "gw" => list(["ぐゎ", "ぐぃ", "ぐ", "ぐぇ", "ぐぉ"]),
        "ky" => palatal("き"),
        "gy" => palatal("ぎ"),
        "ny" => palatal("に"),
        "hy" => palatal("ひ"),
        "by" => palatal("び"),
        "py" => palatal("ぴ"),
        "my" => palatal("み"),
        "ry" => palatal("り"),
        "dy" => palatal("で"),
        "ty" => palatal("て"),
        "fy" => palatal("ふ"),
        _ => None,
    }
}

fn vowel_of_phoneme(phoneme: &str) -> Option<usize> {
    // 無声化した母音は大文字で来る (I, U)。
    ["a", "i", "u", "e", "o"]
        .iter()
        .position(|v| phoneme.eq_ignore_ascii_case(v))
}

/// 聞き取りの道具が返す音素 (`OpenJTalk` の音素を空白で区切ったもの) を、かなのモーラにする。
pub fn phonemes_to_moras(phonemes: &str) -> Vec<String> {
    let tokens: Vec<&str> = phonemes.split_whitespace().collect();
    let mut moras = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        i += 1;
        if let Some(vowel) = vowel_of_phoneme(token) {
            moras.push(VOWEL_KANA[vowel].to_string());
        } else if token == "N" {
            moras.push("ん".into());
        } else if token == "cl" {
            moras.push("っ".into());
        } else if syllable(token, 0).is_some() {
            // 母音が続かない子音は、落ちやすい母音を補う (です の す、まいにち の ち)。
            let vowel = match tokens.get(i).and_then(|next| vowel_of_phoneme(next)) {
                Some(vowel) => {
                    i += 1;
                    vowel
                }
                None => match token {
                    "sh" | "ch" | "j" => 1,
                    "t" | "d" => 4,
                    _ => 2,
                },
            };
            moras.extend(syllable(token, vowel));
        }
    }
    moras
}

fn vowel_of_mora(mora: &str) -> Option<usize> {
    let last = mora.chars().last()?;
    VOWEL_ROWS.iter().position(|row| row.contains(last))
}

/// 書き方が違っても同じ音になるモーラを、聞こえる側の形に寄せる。
fn canonical(mora: &str) -> String {
    mora.replace('を', "お")
        .replace('ぢ', "じ")
        .replace('づ', "ず")
}

/// 書いてある読みのモーラが、聞こえたモーラと同じ音か。
/// 長音 (きょう=きょお、えいが=ええが、カレー) と、助詞の は・へ の書き方の違いは同じ音とみなす。
fn sounds_like(expected: &[String], index: usize, heard: &str) -> bool {
    let mora = expected[index].as_str();
    let before = index
        .checked_sub(1)
        .and_then(|i| long_vowel_base(expected, i));
    let long = before.map(|v| VOWEL_KANA[v]);
    if mora == "ー" {
        return long == Some(heard);
    }
    canonical(mora) == heard
        || (mora == "う" && before == Some(4) && heard == "お")
        || (mora == "い" && before == Some(3) && heard == "え")
        || (mora == "は" && heard == "わ")
        || (mora == "へ" && heard == "え")
}

/// index のモーラの母音。長音記号は前のモーラの母音を引き継ぐ。
fn long_vowel_base(moras: &[String], index: usize) -> Option<usize> {
    let mut i = index;
    loop {
        if moras[i] != "ー" {
            return vowel_of_mora(&moras[i]);
        }
        i = i.checked_sub(1)?;
    }
}

/// 聞こえたモーラ列を、ふつうの書き方に寄せる (こおよお → こうよう、せえ → せい)。
fn display_heard(moras: &[&str]) -> String {
    let mut out = String::new();
    let mut before: Option<usize> = None;
    for mora in moras {
        match (*mora, before) {
            ("お", Some(4)) => out.push('う'),
            ("え", Some(3)) => out.push('い'),
            _ => out.push_str(mora),
        }
        before = vowel_of_mora(mora);
    }
    out
}

// ---------------------------------------------------------------------------
// 生成 AI の読み
// ---------------------------------------------------------------------------

/// 生成 AI が区切った語と、その読み。
#[derive(Debug, Clone)]
pub struct Word {
    /// 文の中の表記。
    pub surface: String,
    /// 文の先頭から数えた文字の位置。
    pub start: usize,
    /// 読みのモーラ。読みが分からない英字の語は空。
    pub moras: Vec<String>,
    /// 読み違いを直す対象か (漢字・数字を含み、英字を含まない語)。
    pub fixable: bool,
}

impl Word {
    fn reading(&self) -> String {
        self.moras.concat()
    }
}

fn is_kanji(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '々' | '〆')
}

fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, 'Ａ'..='Ｚ' | 'ａ'..='ｚ')
}

/// 生成 AI に読みを尋ねる時の出力の形。
pub fn words_format() -> ResponseFormat {
    ResponseFormat {
        name: "reading",
        schema: json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": { "word": { "type": "string" }, "yomi": { "type": "string" } },
                "required": ["word", "yomi"],
                "additionalProperties": false,
            },
        }),
    }
}

pub fn build_reading_prompt(text: &str) -> String {
    format!(
        r#"次の文を音声で読み上げます。文を単語に区切り、それぞれの単語を文脈に合う読み方でひらがなにしてください。

ルール:
- word を順に繋げると、元の文と一字一句同じになること (句読点や記号も 1 つの word として残す)
- yomi は、その word を声に出したときの読みをひらがなだけで書く。句読点や記号の yomi は空文字にする
- 同じ漢字でも文脈で読みが変わる語 (一日: いちにち/ついたち、人気: にんき/ひとけ など) は、文の意味から判断する
- 出力は JSON 配列のみ。説明は付けず、1 行で出力する

出力の形: [{{"word":"今日","yomi":"きょう"}},{{"word":"は","yomi":"は"}}]

文: {text}"#
    )
}

/// 生成 AI の出力 (`[{word, yomi}]`) を、文の中の位置と読みを持つ語の列にする。
/// 語を繋げたものが文と違う出力と、漢字の語にかなの読みが無い出力は受け取らない。
/// 句読点・記号・空白の違いは許す (生成 AI は「、」を「,」に替えたり落としたりする)。
pub fn parse_words(output: &Value, text: &str) -> Result<Vec<Word>, String> {
    let items = output
        .as_array()
        .ok_or("reading must be an array of words")?;
    let chars: Vec<char> = text.chars().collect();
    let mut cursor = 0;
    let mut words = Vec::new();

    for item in items {
        let surface: Vec<char> = item
            .get("word")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_matches(|c: char| !c.is_alphanumeric())
            .chars()
            .collect();
        if surface.is_empty() {
            continue; // 句読点・記号だけの語
        }
        while chars.get(cursor).is_some_and(|c| !c.is_alphanumeric()) {
            cursor += 1;
        }
        let end = cursor + surface.len();
        if chars.get(cursor..end) != Some(surface.as_slice()) {
            let word: String = surface.iter().collect();
            return Err(format!("word {word:?} is not in the text at its turn"));
        }

        let spoken = || surface.iter().filter(|c| c.is_alphanumeric());
        let latin = spoken().any(|c| is_latin(*c));
        let fixable = !latin && spoken().any(|c| is_kanji(*c) || c.is_numeric());
        let word: String = surface.iter().collect();
        let moras = if spoken().all(|c| is_kana_char(*c)) {
            kana_to_moras(&word) // かなの語は書いてあるとおりに読む
        } else {
            let yomi: String = item
                .get("yomi")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            let usable = !yomi.is_empty() && yomi.chars().all(is_kana_char);
            if fixable && !usable {
                return Err(format!("word {word:?} has no kana reading: {yomi:?}"));
            }
            if usable {
                kana_to_moras(&yomi)
            } else {
                Vec::new()
            }
        };
        words.push(Word {
            surface: word,
            start: cursor,
            moras,
            fixable,
        });
        cursor = end;
    }

    if chars[cursor.min(chars.len())..]
        .iter()
        .any(|c| c.is_alphanumeric())
    {
        return Err("reading stops before the end of the text".into());
    }
    if words.is_empty() {
        return Err("reading has no words".into());
    }
    Ok(words)
}

// ---------------------------------------------------------------------------
// 比べる
// ---------------------------------------------------------------------------

/// 読み違えた語。
#[derive(Debug, Clone)]
pub struct Mismatch {
    /// 語の番号。
    pub word: usize,
    /// その語の位置で聞こえた読み。
    pub heard: String,
}

/// 生成 AI の読みと、聞き取った読みを比べた結果。
#[derive(Debug, Clone)]
pub struct Comparison {
    /// 生成 AI の読み。
    pub expected: String,
    /// 聞き取った読み。合っている所は生成 AI の書き方、違う所は聞こえたままのかな。
    pub heard: String,
    /// 読み違えた語 (直す対象の語だけ)。
    pub mismatches: Vec<Mismatch>,
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    /// 同じ音 (expected, heard)。
    Same(usize, usize),
    /// 違う音 (expected, heard)。
    Differ(usize, usize),
    /// 聞こえなかった (expected)。
    Missing(usize),
    /// 余分に聞こえた (heard)。
    Extra(usize),
}

/// 2 つのモーラ列を、違いが最も少なくなるように対応づける (編集距離)。
fn align(expected: &[String], heard: &[String]) -> Vec<Op> {
    let (n, m) = (expected.len(), heard.len());
    let same = |i: usize, j: usize| sounds_like(expected, i, &heard[j]);
    let mut cost = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in cost.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in cost[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let step = usize::from(!same(i - 1, j - 1));
            cost[i][j] = (cost[i - 1][j - 1] + step)
                .min(cost[i - 1][j] + 1)
                .min(cost[i][j - 1] + 1);
        }
    }

    let mut ops = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let matched = same(i - 1, j - 1);
            if cost[i][j] == cost[i - 1][j - 1] + usize::from(!matched) {
                ops.push(if matched {
                    Op::Same(i - 1, j - 1)
                } else {
                    Op::Differ(i - 1, j - 1)
                });
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && cost[i][j] == cost[i - 1][j] + 1 {
            ops.push(Op::Missing(i - 1));
            i -= 1;
        } else {
            ops.push(Op::Extra(j - 1));
            j -= 1;
        }
    }
    ops.reverse();
    gather_extras(&mut ops, expected, heard);
    ops
}

/// 余分に聞こえた音を、隣の違いの区間へ寄せる。
/// 「を いっ」と「お おこなっ」のように同じ音が並ぶと、余分な「お」をどちらに置いても違いの数は
/// 同じになる。離れた所に置かれると隣の語の読み違いに見えるので、違いの続く側へずらす。
fn gather_extras(ops: &mut [Op], expected: &[String], heard: &[String]) {
    let is_extra = |op: &Op| matches!(op, Op::Extra(_));
    let mut start = 0;
    while start < ops.len() {
        if !is_extra(&ops[start]) {
            start += 1;
            continue;
        }
        let end = (start..ops.len())
            .find(|k| !is_extra(&ops[*k]))
            .unwrap_or(ops.len());
        let Op::Extra(first) = ops[start] else {
            unreachable!("start is an extra sound")
        };
        // 後ろへずらす: 余分な音の先頭が、続く「同じ音」の代わりになれる間だけ進める。
        let mut moved = 0;
        while let Some(Op::Same(i, _)) = ops.get(end + moved) {
            if !sounds_like(expected, *i, &heard[first + moved]) {
                break;
            }
            moved += 1;
        }
        let reaches_difference = ops
            .get(end + moved)
            .is_some_and(|op| !matches!(op, Op::Same(..)));
        if moved > 0 && reaches_difference {
            for k in 0..moved {
                if let Op::Same(i, _) = ops[end + k] {
                    ops[start + k] = Op::Same(i, first + k);
                }
            }
            for k in 0..end - start {
                ops[start + moved + k] = Op::Extra(first + moved + k);
            }
        }
        start = end + moved;
    }
}

/// 生成 AI の読み (語ごと) と聞き取ったモーラ列を比べ、読み違えた語を挙げる。
pub fn compare(words: &[Word], heard: &[String]) -> Comparison {
    // 読みのモーラを 1 列に並べ、各モーラがどの語のものかを覚える。
    let mut expected: Vec<String> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    for (index, word) in words.iter().enumerate() {
        owner.extend(std::iter::repeat_n(index, word.moras.len()));
        expected.extend(word.moras.iter().cloned());
    }
    let ops = align(&expected, heard);

    // 違いが続く区間ごとに、余分に聞こえた音をどの語のものとするかを決める。
    let mut blamed: Vec<Option<usize>> = vec![None; ops.len()];
    let mut start = 0;
    while start < ops.len() {
        if matches!(ops[start], Op::Same(..)) {
            start += 1;
            continue;
        }
        let end = (start..ops.len())
            .find(|k| matches!(ops[*k], Op::Same(..)))
            .unwrap_or(ops.len());
        let word_at = |k: usize| match ops[k] {
            Op::Differ(i, _) | Op::Missing(i) | Op::Same(i, _) => Some(owner[i]),
            Op::Extra(_) => None,
        };
        let inside: Vec<Option<usize>> = (start..end).map(word_at).collect();
        if inside.iter().any(Option::is_some) {
            // 違う音を持つ語がある: 余分な音は、直前の (無ければ直後の) その語に付ける。
            let mut last = inside.iter().flatten().next().copied();
            for (k, word) in (start..end).zip(&inside) {
                last = word.or(last);
                blamed[k] = last;
            }
        } else {
            // 余分な音だけ: 隣の直せる語に付ける。読みの分からない語を挟むなら、その語の音とみなす。
            let before = start.checked_sub(1).and_then(word_at);
            let after = (end < ops.len()).then(|| word_at(end)).flatten();
            let from = before.map_or(0, |w| w + 1);
            let to = after.unwrap_or(words.len());
            let unknown_between = words[from..to].iter().any(|w| w.moras.is_empty());
            if !unknown_between {
                let target = [before, after]
                    .into_iter()
                    .flatten()
                    .find(|w| words[*w].fixable);
                blamed[start..end].fill(target);
            }
        }
        start = end;
    }

    // 語ごとに、その位置で聞こえた音を集める。
    let mut heard_by_word: Vec<Vec<&str>> = vec![Vec::new(); words.len()];
    let mut wrong: BTreeSet<usize> = BTreeSet::new();
    let mut shown = String::new();
    let mut differing: Vec<&str> = Vec::new();
    for (k, op) in ops.iter().enumerate() {
        match *op {
            Op::Same(i, j) => {
                shown.push_str(&display_heard(&differing));
                differing.clear();
                shown.push_str(&expected[i]);
                heard_by_word[owner[i]].push(&heard[j]);
            }
            Op::Differ(_, j) | Op::Extra(j) => {
                differing.push(&heard[j]);
                if let Some(word) = blamed[k] {
                    heard_by_word[word].push(&heard[j]);
                    wrong.insert(word);
                }
            }
            Op::Missing(_) => {
                if let Some(word) = blamed[k] {
                    wrong.insert(word);
                }
            }
        }
    }
    shown.push_str(&display_heard(&differing));

    let mismatches = wrong
        .into_iter()
        .filter(|word| words[*word].fixable)
        .map(|word| Mismatch {
            word,
            heard: display_heard(&heard_by_word[word]),
        })
        .collect();
    Comparison {
        expected: expected.concat(),
        heard: shown,
        mismatches,
    }
}

// ---------------------------------------------------------------------------
// 直す
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
    Hiragana,
    Katakana,
}

/// 選んだ語だけを読みのかなに替えた、読み上げ用の表記を作る。
pub fn respell(text: &str, words: &[Word], fixes: &[(usize, Spelling)]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut fixes: Vec<(usize, Spelling)> = fixes.to_vec();
    fixes.sort_by_key(|(word, _)| words[*word].start);

    let mut out = String::new();
    let mut cursor = 0;
    for (index, spelling) in fixes {
        let word = &words[index];
        out.extend(&chars[cursor..word.start]);
        match spelling {
            Spelling::Hiragana => out.push_str(&word.reading()),
            Spelling::Katakana => out.push_str(&to_katakana(&word.reading())),
        }
        cursor = word.start + word.surface.chars().count();
    }
    out.extend(&chars[cursor..]);
    out
}

/// 2 つの読みを見せた時の、生成 AI の答え。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// A。
    First,
    /// B。
    Second,
    /// どちらも正しい。
    Both,
    /// どちらも違う。
    Neither,
}

pub fn choice_format() -> ResponseFormat {
    ResponseFormat {
        name: "choice",
        schema: json!({
            "type": "object",
            "properties": { "answer": { "type": "string", "enum": ["A", "B", "どちらも正しい", "どちらも違う"] } },
            "required": ["answer"],
            "additionalProperties": false,
        }),
    }
}

/// 語の読みを 2 つ見せて選ばせる問い。文の中のどの語かは【】で示す。
pub fn build_choice_prompt(text: &str, word: &Word, a: &str, b: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let end = word.start + word.surface.chars().count();
    let before: String = chars[..word.start].iter().collect();
    let after: String = chars[end..].iter().collect();
    let surface = &word.surface;
    format!(
        r#"次の文を読み上げるとき、【】で囲んだ「{surface}」の読み方として文脈に合うのはどちらですか。

文: {before}【{surface}】{after}
A: {a}
B: {b}

文の意味から判断してください。片方が明らかに合い、もう片方が明らかに合わない時だけ A か B で答えます。
どちらの読み方でも文の意味が通る時は「どちらも正しい」、どちらも日本語の読み方としておかしい時は「どちらも違う」と答えてください。
出力は JSON のみ: {{"answer":"A"}}"#
    )
}

pub fn parse_choice(output: &str) -> Option<Choice> {
    let answer = crate::script::extract_json(output)
        .ok()
        .and_then(|v| v.get("answer").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| output.to_string());
    match answer.trim() {
        "A" => Some(Choice::First),
        "B" => Some(Choice::Second),
        "どちらも正しい" => Some(Choice::Both),
        "どちらも違う" => Some(Choice::Neither),
        _ => None,
    }
}

/// 生成 AI の読みを A に置いた答えと、B に置いた答えから、直すかを決める。
/// 並び順に関わらず自分の読みを選んだ時だけ直す。
pub fn approves(ai_first: Option<Choice>, ai_second: Option<Choice>) -> bool {
    ai_first == Some(Choice::First) && ai_second == Some(Choice::Second)
}

// ---------------------------------------------------------------------------
// 結果 (行ごとに保存する)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// 読み違いなし。
    Ok,
    /// 読み違いを直し、音声で確かめた。
    Fixed,
    /// 読みは食い違ったが、生成 AI が自分の読みを選び切らなかったので変えていない。
    Kept,
    /// 直そうとしたが、平仮名でもカタカナでも読みが合わなかったので変えていない。
    Unresolved,
    /// 生成 AI か聞き取りが動かず、確かめられなかった。
    Failed,
    /// 利用者が直しを外した。
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Fixed,
    Kept,
    Unresolved,
}

/// 読みが食い違った語。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordReading {
    pub surface: String,
    /// 生成 AI の読み。
    pub expected: String,
    /// 聞き取った読み。
    pub heard: String,
    pub action: Action,
}

/// 読み上げの 1 区切り (1 文) を確かめた結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentReading {
    /// 確かめた時の読み上げ文 (辞書を適用した後の、VOICEPEAK へ渡す文)。
    pub text: String,
    pub status: Status,
    /// 生成 AI の読み。
    pub expected: Option<String>,
    /// 直す前に聞き取った読み。
    pub heard: Option<String>,
    /// 直した表記。これがある間、text の代わりに読み上げる。
    pub spoken: Option<String>,
    /// 読みが食い違った語。
    pub words: Vec<WordReading>,
    /// 確かめられなかった理由。
    pub error: Option<String>,
}

impl SegmentReading {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            status: Status::Ok,
            expected: None,
            heard: None,
            spoken: None,
            words: Vec::new(),
            error: None,
        }
    }

    fn failed(text: &str, error: String) -> Self {
        Self {
            status: Status::Failed,
            error: Some(error),
            ..Self::new(text)
        }
    }
}

/// 1 行を確かめた結果。`lines.reading` に JSON で保存する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineReading {
    pub checked_at: String,
    /// 確かめるのに掛かった秒数。
    pub secs: f64,
    pub segments: Vec<SegmentReading>,
}

impl LineReading {
    /// 保存してある JSON を読む。無い・読めない時は「確かめていない」として扱う。
    pub fn parse(stored: Option<&str>) -> Option<Self> {
        serde_json::from_str(stored?).ok()
    }

    /// 行を代表する状態。直した・直せなかった・確かめられなかった、の順に目立たせる。
    pub fn status(&self) -> Status {
        use Status::{Dismissed, Failed, Fixed, Kept, Ok, Unresolved};
        [Fixed, Unresolved, Failed, Kept, Dismissed]
            .into_iter()
            .find(|status| self.segments.iter().any(|s| s.status == *status))
            .unwrap_or(Ok)
    }
}

fn segment_text(segment: &Value) -> &str {
    segment.get("text").and_then(Value::as_str).unwrap_or("")
}

/// 読み上げる segment のうち、直した表記のあるものを差し替える。
/// 確かめた時と読む文字が同じ segment だけが対象で、文や辞書が変わった segment は今のまま読む。
pub fn apply_reading(mut segments: Vec<Value>, reading: Option<&LineReading>) -> Vec<Value> {
    let Some(reading) = reading else {
        return segments;
    };
    for segment in &mut segments {
        let key = spoken_chars(segment_text(segment));
        let fixed = reading
            .segments
            .iter()
            .filter(|checked| spoken_chars(&checked.text) == key)
            .find_map(|checked| checked.spoken.as_deref());
        if let (Some(spoken), Some(object)) = (fixed, segment.as_object_mut()) {
            object.insert("text".into(), Value::from(spoken));
        }
    }
    segments
}

/// 保存してある結果が、今の読み上げ文を確かめたものか。
pub fn is_current(reading: &LineReading, segments: &[Value]) -> bool {
    let now = segments.iter().map(|s| spoken_chars(segment_text(s)));
    let checked = reading.segments.iter().map(|s| spoken_chars(&s.text));
    now.eq(checked)
}

/// 直しを外す。確かめた記録は残すので、次の確認でも直し直さない。
pub fn dismiss(mut reading: LineReading) -> LineReading {
    for segment in &mut reading.segments {
        if segment.spoken.take().is_some() {
            segment.status = Status::Dismissed;
        }
    }
    reading
}

/// 確かめ直した結果に、前に利用者が外した直しを掛け直さない。
/// 同じ文の区切りが前に「外した」なら、新しい直しも外した状態にする。
pub fn keep_dismissals(mut new: LineReading, old: Option<&LineReading>) -> LineReading {
    let Some(old) = old else {
        return new;
    };
    let dismissed: Vec<String> = old
        .segments
        .iter()
        .filter(|segment| segment.status == Status::Dismissed)
        .map(|segment| spoken_chars(&segment.text))
        .collect();
    for segment in &mut new.segments {
        if segment.spoken.is_some() && dismissed.contains(&spoken_chars(&segment.text)) {
            segment.spoken = None;
            segment.status = Status::Dismissed;
        }
    }
    new
}

// ---------------------------------------------------------------------------
// 1 区切りを確かめる
// ---------------------------------------------------------------------------

/// 生成 AI と、合成・聞き取り。副作用はすべてここの向こうにある。
#[async_trait]
pub trait Tools: Send + Sync {
    /// 生成 AI に問い、出力をそのまま返す。
    async fn ask(&self, prompt: &str, format: &ResponseFormat) -> Result<String, String>;
    /// segment を今の設定のまま合成し、聞き取った音素 (空白区切り) を返す。
    async fn hear(&self, segment: &Value) -> Result<String, String>;
}

async fn ask_words(tools: &dyn Tools, text: &str) -> Result<Vec<Word>, String> {
    let prompt = build_reading_prompt(text);
    let format = words_format();
    let mut last_error = String::new();
    for attempt in 1..=MAX_READING_ATTEMPTS {
        let parsed = match tools.ask(&prompt, &format).await {
            Ok(output) => crate::script::extract_json(&output)
                .map_err(|e| e.0)
                .and_then(|value| parse_words(&value, text)),
            Err(e) => Err(e),
        };
        match parsed {
            Ok(words) => return Ok(words),
            Err(e) => {
                tracing::warn!("reading for {text:?} failed (attempt {attempt}): {e}");
                last_error = e;
            }
        }
    }
    Err(last_error)
}

async fn hear_with(
    tools: &dyn Tools,
    segment: &Value,
    text: &str,
    words: &[Word],
) -> Result<Comparison, String> {
    let mut spoken = segment.clone();
    if let Some(object) = spoken.as_object_mut() {
        object.insert("text".into(), Value::from(text));
    }
    let phonemes = tools.hear(&spoken).await?;
    Ok(compare(words, &phonemes_to_moras(&phonemes)))
}

/// 生成 AI に 2 つの読みを見せ、並び順を入れ替えて 2 回尋ねる。
async fn ai_insists(tools: &dyn Tools, text: &str, word: &Word, heard: &str) -> bool {
    let format = choice_format();
    let own = word.reading();
    let ai_first = tools
        .ask(&build_choice_prompt(text, word, &own, heard), &format)
        .await;
    let ai_second = tools
        .ask(&build_choice_prompt(text, word, heard, &own), &format)
        .await;
    let choice = |answer: Result<String, String>| answer.ok().and_then(|a| parse_choice(&a));
    approves(choice(ai_first), choice(ai_second))
}

/// 読み上げの 1 区切りを確かめ、読み違えた語があれば直した表記を作る。
/// 生成 AI・聞き取りが動かない時と、直しても読みが合わない時は、表記を変えない。
pub async fn check_segment(tools: &dyn Tools, segment: &Value) -> SegmentReading {
    let text = segment_text(segment);
    if !text.chars().any(char::is_alphanumeric) {
        return SegmentReading::new(text); // 記号だけで、読む語が無い
    }
    let words = match ask_words(tools, text).await {
        Ok(words) => words,
        Err(e) => return SegmentReading::failed(text, e),
    };
    let baseline = match hear_with(tools, segment, text, &words).await {
        Ok(comparison) => comparison,
        Err(e) => return SegmentReading::failed(text, e),
    };
    let mut result = SegmentReading {
        expected: Some(baseline.expected.clone()),
        heard: Some(baseline.heard.clone()),
        ..SegmentReading::new(text)
    };
    if baseline.mismatches.is_empty() {
        return result;
    }

    // 生成 AI の読みが当てずっぽうでないかを、もう 1 度尋ねて確かめる。答えが無ければ全て信じない。
    let unstable: BTreeSet<usize> = match ask_words(tools, text).await {
        Ok(again) => {
            let moras: Vec<String> = again.into_iter().flat_map(|word| word.moras).collect();
            let differing = compare(&words, &moras).mismatches;
            differing.iter().map(|m| m.word).collect()
        }
        Err(_) => baseline.mismatches.iter().map(|m| m.word).collect(),
    };

    // 食い違った語ごとに、生成 AI が自分の読みを選び切るかを確かめる。
    let mut trying: Vec<(usize, Option<Spelling>)> = Vec::new();
    for mismatch in &baseline.mismatches {
        let word = &words[mismatch.word];
        let insists = !unstable.contains(&mismatch.word)
            && ai_insists(tools, text, word, &mismatch.heard).await;
        if insists {
            trying.push((mismatch.word, Some(Spelling::Hiragana)));
        }
        result.words.push(WordReading {
            surface: word.surface.clone(),
            expected: word.reading(),
            heard: mismatch.heard.clone(),
            action: if insists {
                Action::Unresolved
            } else {
                Action::Kept
            },
        });
    }
    result.status = Status::Kept;
    if trying.is_empty() {
        return result;
    }

    // 平仮名 → カタカナの順に合成し直して聞く。合わない語は元の表記へ戻す。
    let wrong_before: BTreeSet<usize> = baseline.mismatches.iter().map(|m| m.word).collect();
    result.status = Status::Unresolved;
    for _ in 0..MAX_ROUNDS {
        let fixes: Vec<(usize, Spelling)> = trying
            .iter()
            .filter_map(|(word, spelling)| spelling.map(|s| (*word, s)))
            .collect();
        if fixes.is_empty() {
            break;
        }
        let candidate = respell(text, &words, &fixes);
        let heard = match hear_with(tools, segment, &candidate, &words).await {
            Ok(comparison) => comparison,
            Err(e) => return SegmentReading::failed(text, e),
        };
        let wrong_now: BTreeSet<usize> = heard.mismatches.iter().map(|m| m.word).collect();
        // 直す前は合っていた語が合わなくなったら、今回の表記は全て不採用にする。
        let broke_another = wrong_now.difference(&wrong_before).next().is_some();
        let failing: Vec<usize> = fixes
            .iter()
            .map(|(word, _)| *word)
            .filter(|word| broke_another || wrong_now.contains(word))
            .collect();
        if failing.is_empty() {
            let confirmed = result.words.iter_mut().zip(&baseline.mismatches);
            for (reading, mismatch) in confirmed {
                if fixes.iter().any(|(word, _)| *word == mismatch.word) {
                    reading.action = Action::Fixed;
                }
            }
            result.status = Status::Fixed;
            result.spoken = Some(candidate);
            return result;
        }
        for (word, spelling) in &mut trying {
            if failing.contains(word) {
                *spelling = match spelling {
                    Some(Spelling::Hiragana) => Some(Spelling::Katakana),
                    _ => None,
                };
            }
        }
    }
    result
}

#[cfg(test)]
mod tests;
