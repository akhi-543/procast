//! The two things procast fetches: daily currency rates for the calculator (European
//! Central Bank reference rates, per 1 EUR) and web images extensions show.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Mutex, OnceLock, RwLock},
    time::Duration,
};

use futures::channel::mpsc::UnboundedSender;

use crate::{config, win::Msg};

const ECB: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";
const MAX_IMAGE: u64 = 5 * 1024 * 1024;

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().into()
}

// ---- currency -----------------------------------------------------------------------

/// Units of each currency per 1 EUR.
fn rates() -> &'static RwLock<HashMap<String, f64>> {
    static R: OnceLock<RwLock<HashMap<String, f64>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

/// `<Cube currency='USD' rate='1.1216'/>` lines -> {"USD": 1.1216, …, "EUR": 1}.
pub fn parse_ecb(xml: &str) -> HashMap<String, f64> {
    let attr = |line: &str, name: &str| {
        let start = line.find(&format!("{name}='"))? + name.len() + 2;
        line[start..].split('\'').next().map(str::to_string)
    };
    let mut out: HashMap<String, f64> = xml
        .lines()
        .filter_map(|l| Some((attr(l, "currency")?, attr(l, "rate")?.parse().ok()?)))
        .collect();
    if !out.is_empty() {
        out.insert("EUR".into(), 1.0);
    }
    out
}

pub fn set_rates(r: HashMap<String, f64>) {
    *rates().write().unwrap() = r;
}

/// For fend: units of `currency` per 1 unit of the base currency (EUR).
pub fn rate_of(currency: &str) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
    let r = rates().read().unwrap();
    r.get(&currency.to_uppercase()).copied().ok_or_else(|| format!("no exchange rate for {currency}").into())
}

/// Loads cached rates, and refreshes them in the background when older than a day.
pub fn load_rates() {
    let file = config::dir().join("rates.xml");
    if let Ok(xml) = std::fs::read_to_string(&file) {
        set_rates(parse_ecb(&xml));
    }
    let stale = std::fs::metadata(&file).and_then(|m| m.modified()).map_or(true, |t| t.elapsed().map_or(true, |e| e > Duration::from_secs(86400)));
    if stale {
        std::thread::spawn(move || {
            let Ok(mut resp) = agent().get(ECB).call() else { return };
            let Ok(xml) = resp.body_mut().read_to_string() else { return };
            let parsed = parse_ecb(&xml);
            if !parsed.is_empty() {
                let _ = std::fs::write(&file, &xml);
                set_rates(parsed);
            }
        });
    }
}

// ---- images -------------------------------------------------------------------------

pub fn image_path(url: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    config::dir().join("image-cache").join(format!("{:016x}", h.finish()))
}

/// Where a link's site keeps its icon: `/favicon.ico` on the same host. Only for web links
/// whose host is fixed (not a `{query}` placeholder).
pub fn favicon_url(link: &str) -> Option<String> {
    let (scheme, rest) = link.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    (!host.is_empty() && !host.contains(['{', '}'])).then(|| format!("{}://{host}/favicon.ico", scheme.to_ascii_lowercase()))
}

/// Downloads a web image once into the cache, then reports `Msg::ImageReady`.
pub fn fetch_image(url: &str, tx: &UnboundedSender<Msg>) {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let busy = IN_FLIGHT.get_or_init(Default::default);
    if !url.starts_with("https://") && !url.starts_with("http://") || !busy.lock().unwrap().insert(url.to_string()) {
        return;
    }
    let (url, tx) = (url.to_string(), tx.clone());
    std::thread::spawn(move || {
        let bytes = agent().get(&url).call().ok().and_then(|mut r| r.body_mut().with_config().limit(MAX_IMAGE).read_to_vec().ok());
        if let Some(bytes) = bytes.filter(|b| !b.is_empty()) {
            let path = image_path(&url);
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            if std::fs::write(&path, bytes).is_ok() {
                let _ = tx.unbounded_send(Msg::ImageReady(path));
            }
        }
        busy.lock().unwrap().remove(&url);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecb_rates_convert() {
        let xml = "<Cube time='2026-11-02'>\n\t<Cube currency='USD' rate='1.1216'/>\n\t<Cube currency='INR' rate='98.50'/>\n</Cube>";
        let r = parse_ecb(xml);
        assert_eq!((r["USD"], r["INR"], r["EUR"]), (1.1216, 98.5, 1.0));
        assert!(parse_ecb("<html>maintenance</html>").is_empty());
        set_rates(r);
        assert_eq!(rate_of("usd").unwrap(), 1.1216);
        assert!(rate_of("XYZ").is_err());
        assert_ne!(image_path("https://a/1.png"), image_path("https://a/2.png"));
    }

    #[test]
    fn favicons_come_from_the_links_host() {
        assert_eq!(favicon_url("https://github.com/search?q={query}").as_deref(), Some("https://github.com/favicon.ico"));
        assert_eq!(favicon_url("HTTP://user@example.com#top").as_deref(), Some("http://example.com/favicon.ico"));
        assert_eq!(favicon_url("https://{query}.example.com"), None);
        assert_eq!(favicon_url("obsidian://open?vault=x"), None);
        assert_eq!(favicon_url(r"C:\notes"), None);
    }
}
