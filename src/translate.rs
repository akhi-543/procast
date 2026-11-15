//! DeepL translation: the request and reply as plain functions (no network), and the one
//! HTTP call that runs on a worker thread.

use std::time::Duration;

use serde_json::{Value, json};

/// DeepL's target languages (`GET /v2/languages?type=target`), by code. Kept here so the
/// language picker and `tr de …` work offline.
pub const TARGETS: &[(&str, &str)] = &[
    ("AR", "Arabic"),
    ("BG", "Bulgarian"),
    ("CS", "Czech"),
    ("DA", "Danish"),
    ("DE", "German"),
    ("EL", "Greek"),
    ("EN-GB", "English (British)"),
    ("EN-US", "English (American)"),
    ("ES", "Spanish"),
    ("ET", "Estonian"),
    ("FI", "Finnish"),
    ("FR", "French"),
    ("HU", "Hungarian"),
    ("ID", "Indonesian"),
    ("IT", "Italian"),
    ("JA", "Japanese"),
    ("KO", "Korean"),
    ("LT", "Lithuanian"),
    ("LV", "Latvian"),
    ("NB", "Norwegian (Bokmal)"),
    ("NL", "Dutch"),
    ("PL", "Polish"),
    ("PT-BR", "Portuguese (Brazil)"),
    ("PT-PT", "Portuguese (Portugal)"),
    ("RO", "Romanian"),
    ("RU", "Russian"),
    ("SK", "Slovak"),
    ("SL", "Slovenian"),
    ("SV", "Swedish"),
    ("TR", "Turkish"),
    ("UK", "Ukrainian"),
    ("ZH-HANS", "Chinese (Simplified)"),
    ("ZH-HANT", "Chinese (Traditional)"),
];

pub const NO_KEY: &str = "Add deepl_key to config.toml ([translator])";

#[derive(Debug, Clone, PartialEq)]
pub struct Translation {
    pub text: String,
    /// The source language DeepL recognised ("ES").
    pub detected: String,
}

/// What a translation is asked for: `text` empty means the selection in the app you came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub text: String,
    pub from: String,
    pub to: String,
    /// Paste the result over the selection instead of showing it.
    pub replace: bool,
}

/// Free keys end in `:fx` and use their own host.
pub fn endpoint(key: &str) -> &'static str {
    if key.ends_with(":fx") { "https://api-free.deepl.com/v2/translate" } else { "https://api.deepl.com/v2/translate" }
}

pub fn build_request(text: &str, from: &str, to: &str) -> Value {
    let mut body = json!({"text": [text], "target_lang": to});
    if !from.eq_ignore_ascii_case("auto") {
        body["source_lang"] = from.to_uppercase().into();
    }
    body
}

pub fn parse_reply(body: &str) -> Option<Translation> {
    let v: Value = serde_json::from_str(body).ok()?;
    let t = v["translations"].get(0)?;
    Some(Translation { text: t["text"].as_str()?.to_string(), detected: t["detected_source_language"].as_str().unwrap_or_default().to_string() })
}

pub fn error_text(status: u16) -> String {
    match status {
        403 => "DeepL key rejected: check deepl_key in config.toml".into(),
        456 => "DeepL quota used up this month".into(),
        429 => "DeepL busy, try again".into(),
        _ => format!("DeepL error {status}"),
    }
}

/// Blocks on the network: call from a worker thread.
pub fn translate(key: &str, text: &str, from: &str, to: &str) -> Result<Translation, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err(NO_KEY.into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().into();
    let mut resp = agent
        .post(endpoint(key))
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .content_type("application/json")
        .send(build_request(text, from, to).to_string())
        .map_err(|e| match e {
            ureq::Error::StatusCode(code) => error_text(code),
            e => format!("Could not reach DeepL: {e}"),
        })?;
    let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    parse_reply(&body).ok_or_else(|| "DeepL sent a reply procast could not read".into())
}

/// A target code as typed ("de", "pt-br"); a bare "en", "pt" or "zh" picks the common variant.
fn target_code(word: &str) -> Option<String> {
    let up = word.to_uppercase();
    let up = match up.as_str() {
        "EN" => "EN-US",
        "PT" => "PT-BR",
        "ZH" => "ZH-HANS",
        other => other,
    };
    TARGETS.iter().find(|t| t.0 == up).map(|t| t.0.to_string())
}

/// `translate hola`, `tr hola` or `tr de hola` as typed in the search bar: (target code, text).
/// Without a code of its own the text goes to `default_to`.
pub fn from_query(q: &str, default_to: &str) -> Option<(String, String)> {
    let (kw, rest) = q.trim().split_once(' ')?;
    let rest = rest.trim();
    if !(kw.eq_ignore_ascii_case("translate") || kw.eq_ignore_ascii_case("tr")) || rest.is_empty() {
        return None;
    }
    if let Some((first, text)) = rest.split_once(' ')
        && let Some(code) = target_code(first)
    {
        return Some((code, text.trim().to_string()));
    }
    Some((default_to.to_uppercase(), rest.to_string()))
}

pub fn language_name(code: &str) -> String {
    TARGETS.iter().find(|t| t.0.eq_ignore_ascii_case(code)).map_or_else(|| code.to_string(), |t| t.1.to_string())
}

/// A `[[translate]]` command's title: its name, else "Translate to German".
pub fn command_title(name: &str, to: &str) -> String {
    if name.is_empty() { format!("Translate to {}", language_name(to)) } else { name.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_keys_use_the_free_host() {
        assert_eq!(endpoint("abc:fx"), "https://api-free.deepl.com/v2/translate");
        assert_eq!(endpoint("abc"), "https://api.deepl.com/v2/translate");
    }

    #[test]
    fn request_leaves_out_an_automatic_source() {
        assert_eq!(build_request("hola", "auto", "DE"), serde_json::json!({"text": ["hola"], "target_lang": "DE"}));
        assert_eq!(build_request("hola", "es", "DE"), serde_json::json!({"text": ["hola"], "target_lang": "DE", "source_lang": "ES"}));
    }

    #[test]
    fn reply_gives_text_and_detected_language() {
        let body = r#"{"translations":[{"detected_source_language":"ES","text":"hello"}]}"#;
        assert_eq!(parse_reply(body), Some(Translation { text: "hello".into(), detected: "ES".into() }));
        assert_eq!(parse_reply(r#"{"translations":[]}"#), None);
        assert_eq!(parse_reply("<html>"), None);
    }

    #[test]
    fn errors_say_what_to_do() {
        assert_eq!(error_text(403), "DeepL key rejected: check deepl_key in config.toml");
        assert_eq!(error_text(456), "DeepL quota used up this month");
        assert_eq!(error_text(429), "DeepL busy, try again");
        assert_eq!(error_text(500), "DeepL error 500");
        assert_eq!(translate("  ", "hola", "auto", "DE"), Err("Add deepl_key to config.toml ([translator])".into()));
    }

    #[test]
    fn query_names_a_target_with_its_first_word() {
        assert_eq!(from_query("tr de hola", "EN-US"), Some(("DE".into(), "hola".into())));
        assert_eq!(from_query("Translate ZH good morning", "DE"), Some(("ZH-HANS".into(), "good morning".into())));
        assert_eq!(from_query("tr pt-br hola", "DE"), Some(("PT-BR".into(), "hola".into())));
        assert_eq!(from_query("tr hola mundo", "EN-GB"), Some(("EN-GB".into(), "hola mundo".into())));
        assert_eq!(from_query("translate hola", "FR"), Some(("FR".into(), "hola".into())));
        assert_eq!(from_query("tr de", "FR"), Some(("FR".into(), "de".into())), "a lone code is the text");
        assert_eq!(from_query("tr ", "FR"), None);
        assert_eq!(from_query("tree house", "FR"), None);
    }

    #[test]
    fn command_titles_name_the_language() {
        assert_eq!(command_title("", "DE"), "Translate to German");
        assert_eq!(command_title("", "pt-br"), "Translate to Portuguese (Brazil)");
        assert_eq!(command_title("Deutsch", "DE"), "Deutsch");
        assert_eq!(language_name("XX"), "XX");
    }

    #[test]
    fn targets_are_unique_deepl_codes() {
        let mut codes: Vec<&str> = TARGETS.iter().map(|t| t.0).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), TARGETS.len());
        for c in ["EN-US", "EN-GB", "PT-BR", "PT-PT", "ZH-HANS", "ZH-HANT"] {
            assert!(codes.contains(&c), "{c}");
        }
    }
}
