//! Google Translate, no key needed: the request and reply as plain functions (no network),
//! and the HTTP calls that run on a worker thread. Talks to the same endpoint as Google's
//! own Chrome dictionary extension.

use std::time::Duration;

use serde_json::Value;

/// Google's language codes with English names, for `tr de …` and the language names in views.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("sq", "Albanian"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("hy", "Armenian"),
    ("az", "Azerbaijani"),
    ("eu", "Basque"),
    ("be", "Belarusian"),
    ("bn", "Bengali"),
    ("bs", "Bosnian"),
    ("bg", "Bulgarian"),
    ("ca", "Catalan"),
    ("ceb", "Cebuano"),
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("co", "Corsican"),
    ("hr", "Croatian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("nl", "Dutch"),
    ("en", "English"),
    ("eo", "Esperanto"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("fy", "Frisian"),
    ("gl", "Galician"),
    ("ka", "Georgian"),
    ("de", "German"),
    ("el", "Greek"),
    ("gu", "Gujarati"),
    ("ht", "Haitian Creole"),
    ("ha", "Hausa"),
    ("haw", "Hawaiian"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hmn", "Hmong"),
    ("hu", "Hungarian"),
    ("is", "Icelandic"),
    ("ig", "Igbo"),
    ("id", "Indonesian"),
    ("ga", "Irish"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jv", "Javanese"),
    ("kn", "Kannada"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("ko", "Korean"),
    ("ku", "Kurdish"),
    ("ky", "Kyrgyz"),
    ("lo", "Lao"),
    ("la", "Latin"),
    ("lv", "Latvian"),
    ("lt", "Lithuanian"),
    ("lb", "Luxembourgish"),
    ("mk", "Macedonian"),
    ("mg", "Malagasy"),
    ("ms", "Malay"),
    ("ml", "Malayalam"),
    ("mt", "Maltese"),
    ("mi", "Maori"),
    ("mr", "Marathi"),
    ("mn", "Mongolian"),
    ("my", "Myanmar (Burmese)"),
    ("ne", "Nepali"),
    ("no", "Norwegian"),
    ("ny", "Nyanja (Chichewa)"),
    ("ps", "Pashto"),
    ("fa", "Persian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("pa", "Punjabi"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sm", "Samoan"),
    ("gd", "Scots Gaelic"),
    ("sr", "Serbian"),
    ("st", "Sesotho"),
    ("sn", "Shona"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("so", "Somali"),
    ("es", "Spanish"),
    ("su", "Sundanese"),
    ("sw", "Swahili"),
    ("sv", "Swedish"),
    ("tl", "Filipino (Tagalog)"),
    ("tg", "Tajik"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("cy", "Welsh"),
    ("xh", "Xhosa"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
    ("zu", "Zulu"),
];

/// Other spellings of a language: Google's old codes, and the regional codes older configs used.
const ALIASES: &[(&str, &str)] = &[
    ("zh", "zh-CN"),
    ("zh-hans", "zh-CN"),
    ("zh-hant", "zh-TW"),
    ("en-us", "en"),
    ("en-gb", "en"),
    ("pt-br", "pt"),
    ("pt-pt", "pt"),
    ("nb", "no"),
    ("iw", "he"),
    ("jw", "jv"),
    ("fil", "tl"),
];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Translation {
    pub text: String,
    /// The source language Google recognised ("de").
    pub detected: String,
    /// The language it was translated into; `translate` fills this in.
    pub to: String,
    pub pronunciation: Option<String>,
    /// A spelling correction of the input, the changed words in `[…]`.
    pub suggestion: Option<String>,
    /// Google translated the correction, not the input as typed.
    pub corrected: bool,
}

/// What a translation is asked for: `text` empty means the selection in the app you came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub text: String,
    pub from: String,
    /// Target languages: the first, or the second when the text is already in the first.
    pub to: Vec<String>,
    /// Translate into every target at once instead.
    pub all: bool,
    pub after: After,
}

/// What happens with the answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum After {
    /// Show it in the translator view.
    Open,
    /// Put it on the clipboard.
    Copy,
    /// Paste it over the selection.
    Replace,
}

/// A language as typed ("DE", "pt-br", "auto") as the code Google wants; unknown codes pass through lowercased.
pub fn language(word: &str) -> String {
    let w = word.trim();
    if w.eq_ignore_ascii_case("auto") {
        return "auto".into();
    }
    let known = |code: &str| LANGUAGES.iter().find(|l| l.0.eq_ignore_ascii_case(code)).map(|l| l.0);
    known(w).or_else(|| ALIASES.iter().find(|a| a.0.eq_ignore_ascii_case(w)).and_then(|a| known(a.1))).map_or_else(|| w.to_lowercase(), str::to_string)
}

/// Configured targets as codes; none at all means English.
pub fn languages(words: &[String]) -> Vec<String> {
    let v: Vec<String> = words.iter().filter(|w| !w.trim().is_empty()).map(|w| language(w)).collect();
    if v.is_empty() { vec!["en".into()] } else { v }
}

pub fn language_name(code: &str) -> String {
    let c = language(code);
    LANGUAGES.iter().find(|l| l.0 == c).map_or_else(|| code.to_string(), |l| l.1.to_string())
}

/// "German", or "English / German" for several targets.
pub fn names(codes: &[String]) -> String {
    codes.iter().map(|c| language_name(c)).collect::<Vec<_>>().join(" / ")
}

/// `en` for `en-GB`: regional variants count as one language.
fn base(code: &str) -> String {
    code.split(['-', '_']).next().unwrap_or_default().to_lowercase()
}

/// The two-target rule: text already in the first target goes to the second.
pub fn pick_target<'a>(source: &str, targets: &'a [String]) -> &'a str {
    match targets {
        [] => "en",
        [only] => only,
        [first, second, ..] => {
            if !source.is_empty() && base(source) == base(first) { second } else { first }
        }
    }
}

const ENDPOINT: &str = "https://translate.google.com/translate_a/single?client=dict-chrome-ex";
/// Longer URLs go as a form body instead.
const MAX_URL: usize = 2048;

/// The request: the URL, and the form body when the text is too long for a URL (POST).
pub fn build_url(text: &str, from: &str, to: &str) -> (String, Option<String>) {
    let dt = ["at", "bd", "ex", "ld", "md", "qca", "rw", "rm", "ss", "t"].map(|d| format!("&dt={d}")).concat();
    let base = format!("{ENDPOINT}&sl={from}&tl={to}&hl={to}{dt}&ie=UTF-8&oe=UTF-8&otf=1&ssel=0&tsel=0&kc=7");
    let q = crate::search::url_encode(text);
    if base.len() + 3 + q.len() > MAX_URL { (base, Some(format!("q={q}"))) } else { (format!("{base}&q={q}"), None) }
}

pub fn parse_reply(json: &str) -> Option<Translation> {
    let r: Value = serde_json::from_str(json).ok()?;
    let segments = r[0].as_array()?;
    let text: String = segments.iter().filter_map(|s| s[0].as_str()).collect();
    if text.is_empty() {
        return None;
    }
    let mut detected = r[2].as_str().unwrap_or_default();
    if let Some(d) = r[8][0][0].as_str().filter(|d| *d != detected) {
        detected = d;
    }
    // The segment without a translation carries the romanisation.
    let pronunciation = segments.iter().rev().find(|s| s[0].is_null()).and_then(|s| s[2].as_str().or_else(|| s[3].as_str())).filter(|p| !p.is_empty()).map(str::to_string);
    let suggestion = r[7][0].as_str().map(|s| s.replace("<b><i>", "[").replace("</i></b>", "]"));
    Some(Translation { text, detected: detected.to_string(), to: String::new(), pronunciation, suggestion, corrected: r[7][5].as_bool() == Some(true) })
}

pub fn error_text(status: u16) -> String {
    match status {
        429 => "Google Translate is rate limiting, try again later".into(),
        _ => format!("Google Translate error {status}"),
    }
}

fn request_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(code) => error_text(code),
        _ => "Could not reach Google Translate".into(),
    }
}

/// One request. Blocks on the network: call from a worker thread.
fn fetch(text: &str, from: &str, to: &str) -> Result<Translation, String> {
    let (url, form) = build_url(text, from, to);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().into();
    let mut resp = match form {
        None => agent.get(&url).call(),
        Some(body) => agent.post(&url).content_type("application/x-www-form-urlencoded;charset=UTF-8").send(body),
    }
    .map_err(request_error)?;
    let body = resp.body_mut().read_to_string().map_err(|_| "Could not reach Google Translate".to_string())?;
    let mut t = parse_reply(&body).ok_or("Google Translate changed its reply")?;
    t.to = to.to_string();
    Ok(t)
}

/// Translates into the first target, or the second when the text turns out to be in the first.
pub fn translate(text: &str, from: &str, targets: &[String]) -> Result<Translation, String> {
    let first = pick_target(from, targets);
    let t = fetch(text, from, first)?;
    match pick_target(&t.detected, targets) {
        next if next != first => fetch(text, from, next),
        _ => Ok(t),
    }
}

/// Into every target at once; the targets that failed are left out unless all did.
fn translate_all(text: &str, from: &str, targets: &[String]) -> Result<Vec<Translation>, String> {
    let mut codes: Vec<&String> = vec![];
    for t in targets {
        if !codes.iter().any(|c| c.eq_ignore_ascii_case(t)) {
            codes.push(t);
        }
    }
    let results: Vec<Result<Translation, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = codes.iter().map(|to| s.spawn(move || fetch(text, from, to))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("Translation failed".into()))).collect()
    });
    let (ok, errs): (Vec<_>, Vec<_>) = results.into_iter().partition(Result::is_ok);
    if ok.is_empty() {
        Err(errs.into_iter().find_map(Result::err).unwrap_or_else(|| "Translation failed".into()))
    } else {
        Ok(ok.into_iter().flatten().collect())
    }
}

/// Runs a job; never panics out, so the worker thread always has an answer to send.
pub fn run(job: &Job) -> Result<Vec<Translation>, String> {
    std::panic::catch_unwind(|| if job.all { translate_all(&job.text, &job.from, &job.to) } else { translate(&job.text, &job.from, &job.to).map(|t| vec![t]) })
        .unwrap_or_else(|_| Err("Translation failed".into()))
}

/// `translate hola`, `tr hola` or `tr de hola` as typed in the search bar: (targets, text).
/// Without a code of its own the text goes to `default_to`.
pub fn from_query(q: &str, default_to: &[String]) -> Option<(Vec<String>, String)> {
    let (kw, rest) = q.trim().split_once(' ')?;
    let rest = rest.trim();
    if !(kw.eq_ignore_ascii_case("translate") || kw.eq_ignore_ascii_case("tr")) || rest.is_empty() {
        return None;
    }
    if let Some((first, text)) = rest.split_once(' ')
        && is_code(first)
    {
        return Some((vec![language(first)], text.trim().to_string()));
    }
    Some((languages(default_to), rest.to_string()))
}

const QUICK: &str = "quick translate ";

/// `quick translate hola`: the text, to be translated into every target.
pub fn quick_query(q: &str) -> Option<String> {
    let q = q.trim_start();
    let head = q.get(..QUICK.len())?;
    head.eq_ignore_ascii_case(QUICK).then(|| q[QUICK.len()..].trim().to_string()).filter(|t| !t.is_empty())
}

fn is_code(word: &str) -> bool {
    let c = language(word);
    LANGUAGES.iter().any(|l| l.0 == c)
}

/// A `[[translate]]` command's title: its name, else "Translate to German".
pub fn command_title(name: &str, to: &str) -> String {
    if name.is_empty() { format!("Translate to {}", language_name(to)) } else { name.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    /// A reply Google's endpoint gave for "good morningg" into German (trimmed to what is read).
    const RECORDED: &str = r#"[[["Guten Morgen","good morning",null,null,10],[null,null,null,"ˌɡo͝od ˈmôrniNG"]],[["Wendung",["Guten Morgen."],[["Guten Morgen.",["Good morning."],null,0.54]],"Good morning.",10]],"en",null,null,[["good morning",null,[["Guten Morgen",null,true,false,[10]]],[[0,12]],"good morning",0,0]],0.93,["good <b><i>morning</i></b>","good morning",[1],null,null,true],[["en"],null,[0.93],["en"]]]"#;

    #[test]
    fn short_text_is_a_get_with_every_parameter() {
        let (url, form) = build_url("good morning & more", "auto", "de");
        assert_eq!(form, None);
        for p in ["client=dict-chrome-ex", "sl=auto", "tl=de", "hl=de", "ie=UTF-8", "oe=UTF-8", "otf=1", "ssel=0", "tsel=0", "kc=7"] {
            assert!(url.contains(&format!("&{p}")) || url.contains(&format!("?{p}")), "{p} in {url}");
        }
        for d in ["at", "bd", "ex", "ld", "md", "qca", "rw", "rm", "ss", "t"] {
            assert!(url.contains(&format!("&dt={d}&")), "dt={d}");
        }
        assert!(url.starts_with("https://translate.google.com/translate_a/single?"));
        assert!(url.ends_with("&q=good%20morning%20%26%20more"), "{url}");
    }

    #[test]
    fn long_text_goes_in_a_form_body() {
        let text = "ä".repeat(900);
        let (url, form) = build_url(&text, "en", "de");
        assert!(url.len() <= MAX_URL && !url.contains("&q="), "{}", url.len());
        let body = form.unwrap();
        assert!(body.starts_with("q=%C3%A4%C3%A4") && body.len() == 2 + 900 * 6);
        let (url, form) = build_url("hi", "en", "de");
        assert!(url.contains("&q=hi") && form.is_none());
    }

    #[test]
    fn recorded_reply_gives_text_pronunciation_and_suggestion() {
        let t = parse_reply(RECORDED).unwrap();
        assert_eq!(t.text, "Guten Morgen");
        assert_eq!(t.detected, "en");
        assert_eq!(t.pronunciation.as_deref(), Some("ˌɡo͝od ˈmôrniNG"));
        assert_eq!(t.suggestion.as_deref(), Some("good [morning]"));
        assert!(t.corrected);
    }

    #[test]
    fn segments_join_and_the_detected_language_follows_the_list() {
        let body = r#"[[["Hallo Welt. ","Hello world. ",null,null,10],["Wie geht es dir?","How are you?",null,null,10],[null,null,"Hallo velt","Hello world"]],null,"en",null,null,null,null,null,[["de"],null,[0.9],["de"]]]"#;
        let t = parse_reply(body).unwrap();
        assert_eq!((t.text.as_str(), t.detected.as_str()), ("Hallo Welt. Wie geht es dir?", "de"), "r[8][0][0] wins over r[2]");
        assert_eq!(t.pronunciation.as_deref(), Some("Hallo velt"));
        assert_eq!((t.suggestion, t.corrected), (None, false));
        assert_eq!(parse_reply(r#"[[["x","y"]],null,"fr"]"#).unwrap().pronunciation, None);
    }

    #[test]
    fn unreadable_replies_are_none() {
        for body in ["<html>", "[]", "[[],null,\"en\"]", "[[[null,null,\"x\"]]]", "{}"] {
            assert_eq!(parse_reply(body), None, "{body}");
        }
    }

    #[test]
    fn two_target_rule_moves_to_the_second_target() {
        assert_eq!(pick_target("en", &v(&["en-GB", "de"])), "de");
        assert_eq!(pick_target("EN", &v(&["en", "de"])), "de");
        assert_eq!(pick_target("fr", &v(&["en", "de"])), "en");
        assert_eq!(pick_target("auto", &v(&["en", "de"])), "en");
        assert_eq!(pick_target("", &v(&["en", "de"])), "en");
        assert_eq!(pick_target("en", &v(&["en"])), "en", "one target is the only choice");
        assert_eq!(pick_target("en", &[]), "en");
    }

    #[test]
    fn errors_say_what_happened() {
        assert_eq!(error_text(429), "Google Translate is rate limiting, try again later");
        assert_eq!(error_text(500), "Google Translate error 500");
        assert_eq!(request_error(ureq::Error::StatusCode(429)), "Google Translate is rate limiting, try again later");
        assert_eq!(request_error(ureq::Error::HostNotFound), "Could not reach Google Translate");
    }

    #[test]
    fn codes_are_case_insensitive_and_old_ones_still_work() {
        assert_eq!(language("DE"), "de");
        assert_eq!(language("ZH-cn"), "zh-CN");
        assert_eq!(language("EN-US"), "en");
        assert_eq!(language("PT-BR"), "pt");
        assert_eq!(language("ZH-HANT"), "zh-TW");
        assert_eq!(language(" Auto "), "auto");
        assert_eq!(language("Xx"), "xx");
        assert_eq!(languages(&v(&["EN-GB", "DE"])), v(&["en", "de"]));
        assert_eq!(languages(&[]), v(&["en"]));
        assert_eq!(language_name("pt-br"), "Portuguese");
        assert_eq!(language_name("XX"), "XX");
    }

    #[test]
    fn query_names_a_target_with_its_first_word() {
        let en = v(&["en"]);
        assert_eq!(from_query("tr de hola", &en), Some((v(&["de"]), "hola".into())));
        assert_eq!(from_query("Translate ZH good morning", &en), Some((v(&["zh-CN"]), "good morning".into())));
        assert_eq!(from_query("tr zh-TW hola", &en), Some((v(&["zh-TW"]), "hola".into())));
        assert_eq!(from_query("tr hola mundo", &v(&["EN-GB", "de"])), Some((v(&["en", "de"]), "hola mundo".into())));
        assert_eq!(from_query("translate hola", &v(&["fr"])), Some((v(&["fr"]), "hola".into())));
        assert_eq!(from_query("tr it works", &en), Some((v(&["it"]), "works".into())), "a code word is a target");
        assert_eq!(from_query("tr de", &en), Some((en.clone(), "de".into())), "a lone code is the text");
        assert_eq!(from_query("tr ", &en), None);
        assert_eq!(from_query("tree house", &en), None);
    }

    #[test]
    fn command_titles_are_not_queries() {
        for title in ["Quick Translate", "Instant Translate", "Translate"] {
            assert_eq!(from_query(title, &v(&["en"])), None, "{title}");
            assert_eq!(quick_query(title), None, "{title}");
        }
        assert_eq!(quick_query("Quick Translate hola mundo"), Some("hola mundo".into()));
        assert_eq!(quick_query("quick translate  "), None);
        assert_eq!(quick_query("quick tr hola"), None);
        assert_eq!(quick_query("quick"), None);
        assert_eq!(quick_query("ééééééééééééééééé"), None, "no slicing inside a character");
    }

    #[test]
    fn command_titles_name_the_language() {
        assert_eq!(command_title("", "DE"), "Translate to German");
        assert_eq!(command_title("", "zh-tw"), "Translate to Chinese (Traditional)");
        assert_eq!(command_title("Deutsch", "DE"), "Deutsch");
        assert_eq!(names(&v(&["en", "de"])), "English / German");
    }

    #[test]
    #[ignore = "asks Google Translate over the network: cargo test live_ -- --ignored --nocapture"]
    fn live_translation_follows_the_two_target_rule_and_posts_long_text() {
        let t = translate("good morning", "auto", &v(&["en", "de"])).unwrap();
        println!("{t:?}");
        assert_eq!((t.text.as_str(), t.to.as_str(), t.detected.as_str()), ("Guten Morgen", "de", "en"));
        let t = translate("Guten Morgen", "auto", &v(&["en", "de"])).unwrap();
        assert_eq!((t.text.as_str(), t.to.as_str()), ("Good morning", "en"));
        let job = Job { text: "hello".into(), from: "auto".into(), to: v(&["de", "fr", "DE"]), all: true, after: After::Open };
        let all = run(&job).unwrap();
        println!("{all:?}");
        assert_eq!(all.iter().map(|t| t.to.as_str()).collect::<Vec<_>>(), ["de", "fr"]);
        let long = "good morning. ".repeat(300);
        assert!(build_url(&long, "auto", "de").1.is_some());
        let t = translate(&long, "auto", &v(&["de"])).unwrap();
        assert!(t.text.starts_with("Guten Morgen."), "{}", &t.text[..40]);
    }

    #[test]
    fn languages_are_unique_google_codes() {
        let mut codes: Vec<&str> = LANGUAGES.iter().map(|t| t.0).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), LANGUAGES.len());
        for c in ["en", "de", "zh-CN", "zh-TW", "pt", "ja", "haw", "ceb"] {
            assert!(codes.contains(&c), "{c}");
        }
        assert!(!codes.contains(&"auto"));
        assert!(ALIASES.iter().all(|a| codes.contains(&a.1)));
    }
}
