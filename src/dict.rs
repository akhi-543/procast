//! Offline dictionary: Princeton WordNet 3.1, read straight from its database files
//! (`index.*` found by binary search, `data.*` by byte offset, `*.exc` for irregular forms).
//! The files are downloaded once into `%APPDATA%\procast\wordnet`.


use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::config;

/// WordNet 3.1's database files (about 16 MB); unpacked with the tar that ships with Windows.
const URL: &str = "https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz";
/// Senses shown per part of speech (the common ones come first).
const MAX_SENSES: usize = 6;
/// The root Define row's subtitle is at most this many characters.
const SHORT_GLOSS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pos {
    Noun,
    Verb,
    Adj,
    Adv,
}

impl Pos {
    const ALL: [Pos; 4] = [Pos::Noun, Pos::Verb, Pos::Adj, Pos::Adv];

    /// The file name's ending: `index.noun`, `data.noun`, `noun.exc`.
    fn ext(self) -> &'static str {
        match self {
            Pos::Noun => "noun",
            Pos::Verb => "verb",
            Pos::Adj => "adj",
            Pos::Adv => "adv",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Pos::Noun => "Noun",
            Pos::Verb => "Verb",
            Pos::Adj => "Adjective",
            Pos::Adv => "Adverb",
        }
    }

    /// WordNet's suffix rules for finding a base form (`morphy`): (ending, replacement).
    fn rules(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Pos::Noun => &[("s", ""), ("ses", "s"), ("xes", "x"), ("zes", "z"), ("ches", "ch"), ("shes", "sh"), ("men", "man"), ("ies", "y")],
            Pos::Verb => &[("s", ""), ("ies", "y"), ("es", "e"), ("es", ""), ("ed", "e"), ("ed", ""), ("ing", "e"), ("ing", "")],
            Pos::Adj => &[("er", ""), ("est", ""), ("er", "e"), ("est", "e")],
            Pos::Adv => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sense {
    pub pos: Pos,
    pub gloss: String,
    pub examples: Vec<String>,
    pub synonyms: Vec<String>,
}

/// One part of speech: its `index.<pos>` file, whose lines are sorted by lemma so a word is
/// found by bisection, and its `<pos>.exc` irregular forms.
struct Part {
    index: String,
    /// Where each entry starts in `index` (the licence header's lines are left out).
    starts: Vec<usize>,
    /// Irregular form -> base forms.
    exc: HashMap<String, Vec<String>>,
}

impl Part {
    fn new(index: String, exc: &str) -> Self {
        let lines = index.split_inclusive('\n').scan(0, |at, l| Some((std::mem::replace(at, *at + l.len()), l)));
        let starts = lines.filter(|(_, l)| !l.trim().is_empty() && !l.starts_with(' ')).map(|(s, _)| s).collect();
        let exc = exc.lines().filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.to_string(), w.map(str::to_string).collect()))
        });
        Self { starts, exc: exc.collect(), index }
    }

    fn line(&self, start: usize) -> &str {
        self.index[start..].lines().next().unwrap_or_default()
    }

    fn entry(&self, lemma: &str) -> Option<&str> {
        let i = self.starts.binary_search_by(|&s| self.line(s).split(' ').next().unwrap_or_default().cmp(lemma)).ok()?;
        Some(self.line(self.starts[i]))
    }

    /// The word's own entry, else the first base form that has one: irregular forms, then suffix rules.
    fn find(&self, word: &str, pos: Pos) -> Option<&str> {
        let irregular = self.exc.get(word).into_iter().flatten().cloned();
        let stripped = pos.rules().iter().filter_map(|(end, with)| Some(format!("{}{with}", word.strip_suffix(end)?))).filter(|s| s.len() > 1);
        // "running" -> "runn" -> "run".
        let undoubled = pos.rules().iter().filter(|(end, with)| end.starts_with(['e', 'i']) && with.is_empty()).filter_map(|(end, _)| word.strip_suffix(end)).filter_map(|s| {
            let mut chars = s.chars().rev();
            let last = chars.next()?;
            (chars.next()? == last && chars.next().is_some()).then(|| s[..s.len() - last.len_utf8()].to_string())
        });
        std::iter::once(word.to_string()).chain(irregular).chain(stripped).chain(undoubled).find_map(|c| self.entry(&c))
    }
}

pub struct WordNet {
    dir: PathBuf,
    parts: Vec<(Pos, Part)>,
}

/// How WordNet writes a word: lowercase, underscores for spaces.
fn lemma_of(word: &str) -> String {
    word.trim().to_lowercase().replace(' ', "_")
}

impl WordNet {
    /// Reads the index files of a WordNet `dict` folder; the data files are read per lookup.
    pub fn open(dir: &Path) -> Result<Self, String> {
        if !dir.join("index.noun").is_file() {
            return Err(format!("{} holds no WordNet files", dir.display()));
        }
        let read = |name: String| fs::read_to_string(dir.join(name)).unwrap_or_default();
        let parts = Pos::ALL.map(|p| (p, Part::new(read(format!("index.{}", p.ext())), &read(format!("{}.exc", p.ext())))));
        Ok(Self { dir: dir.to_path_buf(), parts: parts.into() })
    }

    /// Every sense of `word` (or its base form), nouns first, most common sense first.
    pub fn lookup(&self, word: &str) -> Vec<Sense> {
        let word = lemma_of(word);
        if word.is_empty() {
            return vec![];
        }
        let found = self.parts.iter().filter_map(|(pos, part)| Some((*pos, part.find(&word, *pos)?)));
        found.flat_map(|(pos, entry)| self.senses(pos, entry)).collect()
    }

    /// Whether the dictionary has the word, or the word it is an inflection of.
    pub fn knows(&self, word: &str) -> bool {
        let word = lemma_of(word);
        !word.is_empty() && self.parts.iter().any(|(pos, part)| part.find(&word, *pos).is_some())
    }

    /// The words the dictionary has, lowercased, each once, in order.
    pub fn only_known(&self, words: Vec<String>) -> Vec<String> {
        let mut seen = vec![];
        for w in words.into_iter().map(|w| w.to_lowercase()) {
            if !seen.contains(&w) && self.knows(&w) {
                seen.push(w);
            }
        }
        seen
    }

    /// The synsets an index entry lists, read from `data.<pos>` by byte offset.
    fn senses(&self, pos: Pos, entry: &str) -> Vec<Sense> {
        // lemma pos synset_cnt p_cnt [ptr_symbol…] sense_cnt tagsense_cnt synset_offset…
        let fields: Vec<&str> = entry.split_whitespace().collect();
        let count: usize = fields.get(2).and_then(|c| c.parse().ok()).unwrap_or(0);
        let Some(offsets) = fields.len().checked_sub(count).map(|from| &fields[from..]) else { return vec![] };
        let Ok(file) = File::open(self.dir.join(format!("data.{}", pos.ext()))) else { return vec![] };
        let mut data = BufReader::new(file);
        let mut line = String::new();
        let senses = offsets.iter().take(MAX_SENSES).filter_map(|o| {
            line.clear();
            data.seek(SeekFrom::Start(o.parse().ok()?)).ok()?;
            data.read_line(&mut line).ok()?;
            // A line that does not start with its own offset means the file is not what the index says.
            line.starts_with(o).then(|| parse_synset(&line, pos, fields[0])).flatten()
        });
        senses.collect()
    }
}

/// `offset lex_filenum ss_type w_cnt word lex_id … p_cnt … | gloss`
fn parse_synset(line: &str, pos: Pos, lemma: &str) -> Option<Sense> {
    let (head, gloss) = line.split_once('|')?;
    let mut f = head.split_whitespace().skip(3);
    // The word count is two hex digits.
    let words = usize::from_str_radix(f.next()?, 16).ok()?;
    let own = lemma.replace('_', " ");
    let synonyms = f
        .step_by(2)
        .take(words)
        // Adjectives carry a syntactic marker: large(a), late(ip).
        .map(|w| w.split('(').next().unwrap_or(w).replace('_', " "))
        .filter(|w| !w.eq_ignore_ascii_case(&own))
        .collect();
    let (gloss, examples) = split_gloss(gloss.trim());
    Some(Sense { pos, gloss, examples, synonyms })
}

/// "definition; another; \"example\"; \"example\"" -> ("definition; another", [example, example]).
/// Examples are quoted and may hold semicolons themselves.
fn split_gloss(gloss: &str) -> (String, Vec<String>) {
    let (mut def, mut examples, mut open) = (vec![], vec![], None::<String>);
    for part in gloss.split("; ") {
        match open.take() {
            Some(e) => open = Some(format!("{e}; {part}")),
            None if part.starts_with('"') => open = Some(part.to_string()),
            None => def.push(part),
        }
        if let Some(e) = open.take_if(|e| e.len() > 1 && e.ends_with('"')) {
            examples.push(e.trim_matches('"').to_string());
        }
    }
    // A quote that never closed: keep its text.
    examples.extend(open.map(|e| e.trim_matches('"').to_string()));
    (def.join("; "), examples)
}

pub fn dir() -> PathBuf {
    config::dir().join("wordnet")
}

/// Whether `dir` holds every index and data file (a half-unpacked archive does not).
fn complete(dir: &Path) -> bool {
    Pos::ALL.iter().all(|p| dir.join(format!("index.{}", p.ext())).is_file() && dir.join(format!("data.{}", p.ext())).is_file())
}

/// The folder under `root` that holds the whole database: the extracted archive has it in `dict`.
fn locate(root: &Path) -> Option<PathBuf> {
    if complete(root) {
        return Some(root.to_path_buf());
    }
    fs::read_dir(root).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).find_map(|p| locate(&p))
}

/// Where the downloaded dictionary is, if it has been downloaded.
pub fn installed() -> Option<PathBuf> {
    locate(&dir())
}

/// Replaces `root` with `part` once `part` holds the whole database, so a download that died
/// half way never looks installed.
fn swap_in(part: &Path, root: &Path) -> Result<PathBuf, String> {
    if locate(part).is_none() {
        return Err("The download held no WordNet files".into());
    }
    if root.exists() {
        fs::remove_dir_all(root).map_err(|e| format!("Could not replace {}: {e}", root.display()))?;
    }
    // A virus scanner may still hold the new files for a moment.
    let mut moved = fs::rename(part, root);
    for _ in 0..10 {
        if moved.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
        moved = fs::rename(part, root);
    }
    moved.map_err(|e| format!("Could not install the dictionary: {e}"))?;
    locate(root).ok_or_else(|| "The download held no WordNet files".into())
}

/// Downloads WordNet, unpacks it next to [`dir`] and moves it into place. Slow: for a worker thread.
fn download() -> Result<PathBuf, String> {
    use std::os::windows::process::CommandExt;
    let root = dir();
    let (part, archive) = (root.with_extension("part"), root.with_extension("tar.gz"));
    let _ = fs::remove_dir_all(&part);
    fs::create_dir_all(&part).map_err(|e| format!("Could not create {}: {e}", part.display()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).build().into();
    let fetch = || -> Result<(), String> {
        let mut resp = agent.get(URL).call().map_err(|e| e.to_string())?;
        let mut file = File::create(&archive).map_err(|e| e.to_string())?;
        std::io::copy(&mut resp.body_mut().with_config().limit(100 << 20).reader(), &mut file).map_err(|e| e.to_string())?;
        Ok(())
    };
    let unpack = || -> Result<(), String> {
        // The tar that ships with Windows, by full path: another tar on PATH may not read `C:\` paths.
        let tar = Path::new(&std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join(r"System32\tar.exe");
        let status = std::process::Command::new(tar).arg("-xzf").arg(&archive).arg("-C").arg(&part).creation_flags(0x0800_0000).status().map_err(|e| e.to_string())?;
        status.success().then_some(()).ok_or_else(|| "tar failed".into())
    };
    let done = fetch().map_err(|e| format!("Could not download the dictionary: {e}")).and_then(|_| unpack().map_err(|e| format!("Could not unpack the dictionary: {e}")));
    let _ = fs::remove_file(&archive);
    let installed = done.and_then(|_| swap_in(&part, &root));
    let _ = fs::remove_dir_all(&part);
    installed
}

/// Whether a root-search query reads as one word, so a "Define" row is worth showing.
pub fn looks_like_word(q: &str) -> bool {
    q.chars().any(char::is_alphabetic) && q.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-')
}

/// What a lookup found: the senses (none when the word is not in the dictionary), and other
/// spellings the spell checker offers that the dictionary has, each with its first part of speech.
pub struct Answer {
    pub senses: Vec<Sense>,
    pub maybe: Vec<(String, Pos)>,
}

/// Looks `word` up; None when the dictionary is not downloaded yet and `fetch` is false, else
/// it is downloaded first. Never panics: the caller is waiting for an answer.
pub fn define(word: &str, fetch: bool) -> Result<Option<Answer>, String> {
    guarded(|| lookup_word(word, fetch))
}

/// The first sense of `word`, short (`short_gloss`), when the dictionary is installed and has it.
/// For a worker thread: it may read the index files.
pub fn first_sense(word: &str) -> Option<String> {
    guarded(|| Ok(installed().and_then(|d| wordnet(&d).ok()).and_then(|wn| wn.lookup(word).first().map(|s| short_gloss(&s.gloss))))).ok().flatten()
}

/// A panic in `f` becomes an error.
fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err("The dictionary lookup failed".into()))
}

/// The dictionary in `dir`, read once: its index files are a few MB, and root search asks per keystroke.
fn wordnet(dir: &Path) -> Result<Arc<WordNet>, String> {
    static OPEN: Mutex<Option<Arc<WordNet>>> = Mutex::new(None);
    let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(wn) = open.as_ref().filter(|wn| wn.dir == dir) {
        return Ok(wn.clone());
    }
    let wn = Arc::new(WordNet::open(dir)?);
    *open = Some(wn.clone());
    Ok(wn)
}

fn lookup_word(word: &str, fetch: bool) -> Result<Option<Answer>, String> {
    // One download at a time: a lookup that arrives meanwhile waits, then finds it installed.
    static DOWNLOADING: Mutex<()> = Mutex::new(());
    let dir = {
        let _one = DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner());
        match installed() {
            Some(d) => d,
            None if fetch => download()?,
            None => return Ok(None),
        }
    };
    let wn = wordnet(&dir)?;
    Ok(Some(Answer { senses: wn.lookup(word), maybe: suggestions(&wn, word, spelling(word)) }))
}

/// The first five spellings in `said` that the dictionary has and that are not `word` itself,
/// nor an inflection of it ("serendipities" defines just what "serendipity" does).
fn suggestions(wn: &WordNet, word: &str, said: Vec<String>) -> Vec<(String, Pos)> {
    let own = wn.lookup(word);
    let other = |w: &String| Some(wn.lookup(w)).filter(|s| *s != own).and_then(|s| Some((w.clone(), s.first()?.pos)));
    wn.only_known(said).iter().filter_map(other).take(5).collect()
}

/// Senses by part of speech, in `Pos::ALL` order: (pos, its senses); a pos without senses is left out.
pub fn group(senses: &[Sense]) -> Vec<(Pos, Vec<&Sense>)> {
    Pos::ALL.iter().map(|&p| (p, senses.iter().filter(|s| s.pos == p).collect::<Vec<_>>())).filter(|(_, v)| !v.is_empty()).collect()
}

/// A gloss up to its first `;`, cut to `SHORT_GLOSS` characters (the last one an ellipsis).
pub fn short_gloss(gloss: &str) -> String {
    let first = gloss.split(';').next().unwrap_or_default().trim();
    if first.chars().count() <= SHORT_GLOSS {
        return first.to_string();
    }
    format!("{}…", first.chars().take(SHORT_GLOSS - 1).collect::<String>().trim_end())
}

/// The detail pane's meta line: "noun · 1 sense · WordNet", `n` the senses of that part of speech.
pub fn meta_line(pos: Pos, n: usize) -> String {
    format!("{} · {n} {} · WordNet", pos.name().to_lowercase(), if n == 1 { "sense" } else { "senses" })
}

/// "Similar: fluke, luck"; None without synonyms.
pub fn similar_line(synonyms: &[String]) -> Option<String> {
    (!synonyms.is_empty()).then(|| format!("Similar: {}", synonyms.join(", ")))
}

/// The empty view's title when nothing at all is found.
pub fn missing_title(word: &str) -> String {
    format!("No definition for '{word}'")
}

/// The word's Wiktionary page (free, no key).
pub fn wiktionary(word: &str) -> String {
    format!("https://en.wiktionary.org/wiki/{}", crate::search::url_encode(word.trim()))
}

/// Windows' own spell checker (English): spellings for a misspelled word. Empty when the
/// service or its English dictionary is not there.
pub fn spelling(word: &str) -> Vec<String> {
    use windows::{
        Win32::{
            Globalization::{ISpellCheckerFactory, SpellCheckerFactory},
            System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree},
        },
        core::{HSTRING, PWSTR, w},
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let ask = || -> windows::core::Result<Vec<String>> {
            let factory: ISpellCheckerFactory = CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_ALL)?;
            let list = factory.CreateSpellChecker(w!("en-US"))?.Suggest(&HSTRING::from(word))?;
            let (mut out, mut item, mut got) = (vec![], [PWSTR::null()], 0);
            while list.Next(&mut item, Some(&mut got)).is_ok() && got == 1 {
                out.push(item[0].to_string().unwrap_or_default());
                CoTaskMemFree(Some(item[0].0.cast()));
                got = 0;
            }
            Ok(out)
        };
        ask().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    const LICENSE: &str = "  1 This software and database is being provided to you, the LICENSEE, by Princeton University.\n  2 \n";

    /// Lays a data file out the way WordNet does: every synset line starts with its own byte
    /// offset. Returns the file and the offsets.
    fn data_file(synsets: &[&str]) -> (String, Vec<usize>) {
        let (mut text, mut offsets) = (LICENSE.to_string(), vec![]);
        for s in synsets {
            let at = text.len();
            offsets.push(at);
            text += &format!("{at:08} {s}\n");
        }
        (text, offsets)
    }

    /// A few synsets of each part of speech, in WordNet's real formats.
    fn fixture(name: &str) -> PathBuf {
        let dir = crate::test_dir(&format!("wordnet-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fixture_in(&dir);
        dir
    }

    fn fixture_in(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        let (noun, n) = data_file(&[
            "05 n 02 dog 0 domestic_dog 0 001 @ 00000000 n 0000 | a member of the genus Canis; has been domesticated by man since prehistoric times; occurs in many breeds; \"the dog barked all night\"",
            "18 n 02 dog 0 hound 1 001 @ 00000000 n 0000 | informal term for a man; \"you lucky dog\"",
            "05 n 02 cat 0 true_cat 0 001 @ 00000000 n 0000 | feline mammal usually having thick soft fur; \"the cat purred; it slept\"",
        ]);
        let (verb, v) = data_file(&[
            "29 v 01 cat 0 000 | beat with a cat-o'-nine-tails",
            "38 v 02 run 0 go 0 001 @ 00000000 v 0000 01 + 02 00 | move fast by using one's feet; \"Don't run--you'll be out of breath\"; \"The children ran to the store\"",
        ]);
        let (adj, a) = data_file(&["00 a 02 big 0 large(a) 0 000 | above average in size"]);
        let index = [
            ("noun", format!("cat n 1 1 @ 1 0 {:08}\ndog n 2 1 @ 2 0 {:08} {:08}\n", n[2], n[0], n[1])),
            ("verb", format!("cat v 1 1 @ 1 0 {:08}\nrun v 1 1 @ 1 0 {:08}\n", v[0], v[1])),
            ("adj", format!("big a 1 1 & 1 0 {:08}\n", a[0])),
        ];
        for (pos, rows) in index {
            fs::write(dir.join(format!("index.{pos}")), format!("{LICENSE}{rows}")).unwrap();
        }
        for (pos, text) in [("noun", noun), ("verb", verb), ("adj", adj), ("adv", LICENSE.into())] {
            fs::write(dir.join(format!("data.{pos}")), text).unwrap();
        }
        fs::write(dir.join("index.adv"), LICENSE).unwrap();
        fs::write(dir.join("verb.exc"), "ran run\nwent go\n").unwrap();
    }

    #[test]
    fn dog_has_a_noun_sense_with_its_gloss_examples_and_synonyms() {
        let wn = WordNet::open(&fixture("dog")).unwrap();
        let senses = wn.lookup("dog");
        assert_eq!(senses.len(), 2);
        let dog = &senses[0];
        assert_eq!(dog.pos, Pos::Noun);
        assert!(dog.gloss.contains("domesticated"));
        assert_eq!(dog.gloss, "a member of the genus Canis; has been domesticated by man since prehistoric times; occurs in many breeds");
        assert_eq!(dog.examples, ["the dog barked all night"]);
        assert_eq!(dog.synonyms, ["domestic dog"], "the word itself is left out, underscores become spaces");
        assert_eq!(senses[1].synonyms, ["hound"]);
        assert_eq!(wn.lookup("DOG"), senses, "case does not matter");
    }

    #[test]
    fn inflected_words_find_their_base_form() {
        let wn = WordNet::open(&fixture("morph")).unwrap();
        let ran = wn.lookup("ran");
        assert_eq!((ran.len(), ran[0].pos), (1, Pos::Verb), "from verb.exc");
        assert!(ran[0].gloss.starts_with("move fast"));
        assert_eq!(ran[0].examples.len(), 2);
        assert_eq!(wn.lookup("running"), ran, "doubled consonant");
        assert_eq!(wn.lookup("cats").iter().map(|s| s.pos).collect::<Vec<_>>(), [Pos::Noun, Pos::Verb]);
        assert_eq!(wn.lookup("cats")[0].examples, ["the cat purred; it slept"], "a semicolon inside an example");
        assert_eq!(wn.lookup("bigger")[0].pos, Pos::Adj, "the adjective marker is not part of a word");
        assert_eq!(wn.lookup("bigger")[0].synonyms, ["large"]);
    }

    #[test]
    fn non_ascii_words_never_panic() {
        let wn = WordNet::open(&fixture("unicode")).unwrap();
        // U+1000 is E1 80 80 in UTF-8: its last two bytes are equal, but it is one character.
        for w in ["\u{1000}ed", "\u{1000}\u{1000}ing", "\u{1000}\u{1000}\u{1000}ing", "caf\u{e9}s", "\u{e9}\u{e9}ed", "\u{1f600}", "dog\u{1000}", "runn\u{1000}ing"] {
            assert!(wn.lookup(w).is_empty(), "{w}");
            assert!(!wn.knows(w));
        }
    }

    #[test]
    fn damaged_files_give_no_senses_instead_of_panicking() {
        let dir = fixture("damaged");
        // The index points at offsets the data file does not have, and some lines are cut short.
        fs::write(dir.join("data.noun"), "garbage\n").unwrap();
        fs::write(dir.join("data.verb"), "").unwrap();
        fs::write(dir.join("index.adj"), "big a 9 1 & 1 0 00000000\nlast\n").unwrap();
        let wn = WordNet::open(&dir).unwrap();
        assert!(wn.lookup("dog").is_empty() && wn.lookup("run").is_empty() && wn.lookup("big").is_empty() && wn.lookup("last").is_empty());
        for line in ["", "|", "00000000", "00000000 05 n", "00000000 05 n zz dog 0 | x"] {
            assert!(parse_synset(line, Pos::Noun, "dog").is_none(), "{line:?}");
        }
        // More words promised than listed: what is there is used.
        assert!(parse_synset("00000000 05 n 05 dog 0 | x", Pos::Noun, "dog").unwrap().synonyms.is_empty());
    }

    #[test]
    fn a_panicking_lookup_becomes_an_error() {
        assert_eq!(guarded::<()>(|| panic!("boom")).err().as_deref(), Some("The dictionary lookup failed"));
        assert_eq!(guarded(|| Ok(1)), Ok(1));
    }

    #[test]
    fn unknown_words_have_no_senses() {
        let wn = WordNet::open(&fixture("none")).unwrap();
        assert!(wn.lookup("zzzz").is_empty() && wn.lookup("").is_empty() && wn.lookup("  ").is_empty());
    }

    #[test]
    fn suggestions_keep_only_words_in_the_dictionary() {
        let wn = WordNet::open(&fixture("suggest")).unwrap();
        let said = ["dgo", "Dog", "dog", "cats", "xyz"].map(String::from).to_vec();
        assert_eq!(wn.only_known(said), ["dog", "cats"]);
    }

    #[test]
    fn open_needs_the_database_and_locate_finds_it_in_a_subfolder() {
        let empty = crate::test_dir("wordnet-empty");
        let _ = fs::remove_dir_all(&empty);
        fs::create_dir_all(&empty).unwrap();
        assert!(WordNet::open(&empty).is_err() && locate(&empty).is_none());
        let root = empty.with_file_name("wordnet-root");
        let _ = fs::remove_dir_all(&root);
        fixture_in(&root.join("dict"));
        assert_eq!(locate(&root), Some(root.join("dict")));
    }

    #[test]
    fn a_half_unpacked_archive_is_not_installed() {
        let tmp = |name: &str| {
            let dir = crate::test_dir(&format!("wordnet-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            dir
        };
        let (part, root) = (tmp("part"), tmp("live"));
        // Died after index.noun: nothing is found, and nothing is moved into place.
        fs::create_dir_all(part.join("dict")).unwrap();
        fs::write(part.join("dict").join("index.noun"), LICENSE).unwrap();
        fs::write(root.join("old"), "stale").unwrap();
        assert!(locate(&part).is_none());
        assert!(swap_in(&part, &root).is_err() && root.join("old").exists(), "the old folder stays until the new one is whole");
        // Whole: replaces the stale folder.
        fs::remove_dir_all(&part).unwrap();
        fixture_in(&part.join("dict"));
        assert_eq!(swap_in(&part, &root), Ok(root.join("dict")));
        assert!(!root.join("old").exists() && !part.exists());
        assert!(WordNet::open(&locate(&root).unwrap()).unwrap().knows("dog"));
    }

    #[test]
    fn single_words_get_a_define_row() {
        assert!(looks_like_word("dog") && looks_like_word("don't") && looks_like_word("well-known"));
        assert!(!looks_like_word("dog food") && !looks_like_word("2+2") && !looks_like_word("") && !looks_like_word("c++"));
    }

    #[test]
    fn short_gloss_stops_at_the_first_semicolon_and_60_characters() {
        assert_eq!(short_gloss("a member of the genus Canis; has been domesticated"), "a member of the genus Canis");
        let long = short_gloss(&"word ".repeat(30));
        assert_eq!(long.chars().count(), 60);
        assert!(long.ends_with('…'));
        assert_eq!(short_gloss("short"), "short");
    }

    #[test]
    fn the_detail_lines_say_what_they_show() {
        assert_eq!(meta_line(Pos::Noun, 1), "noun · 1 sense · WordNet");
        assert_eq!(meta_line(Pos::Adj, 2), "adjective · 2 senses · WordNet");
        assert_eq!(similar_line(&["fluke".into(), "luck".into()]).as_deref(), Some("Similar: fluke, luck"));
        assert_eq!(similar_line(&[]), None);
        assert_eq!(missing_title("xyz"), "No definition for 'xyz'");
        assert_eq!(wiktionary("dog"), "https://en.wiktionary.org/wiki/dog");
    }

    #[test]
    fn group_orders_senses_by_part_of_speech_with_counts() {
        let s = |pos, gloss: &str| Sense { pos, gloss: gloss.into(), examples: vec![], synonyms: vec![] };
        let senses = [s(Pos::Adj, "a1"), s(Pos::Noun, "n1"), s(Pos::Adj, "a2"), s(Pos::Adv, "r1")];
        let got: Vec<(Pos, Vec<&str>)> = group(&senses).into_iter().map(|(p, v)| (p, v.iter().map(|s| s.gloss.as_str()).collect())).collect();
        assert_eq!(got, [(Pos::Noun, vec!["n1"]), (Pos::Adj, vec!["a1", "a2"]), (Pos::Adv, vec!["r1"])]);
    }

    #[test]
    fn suggestions_leave_out_the_word_itself_and_say_their_part_of_speech() {
        let wn = WordNet::open(&fixture("maybe")).unwrap();
        let said = ["Dog", "dgo", "dogs", "run", "cats"].map(String::from).to_vec();
        assert_eq!(suggestions(&wn, "dog", said), [("run".to_string(), Pos::Verb), ("cats".to_string(), Pos::Noun)]);
    }

    /// Needs the Windows spelling service and an English dictionary.
    #[test]
    #[ignore]
    fn windows_suggests_spellings() {
        assert!(spelling("dgo").iter().any(|s| s == "dog"));
    }
}
