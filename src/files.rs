//! File search: an in-memory index of file and folder names under the configured roots,
//! searched on a worker thread. Only the newest query's results are ever delivered
//! (generation counter), so typing never waits on a slow match.
// The index is saved to disk after each crawl and loaded at start, re-crawled once in the
// background, then kept current by a ReadDirectoryChangesW watcher per root.
// ponytail: changes made while a crawl runs can be lost until the next crawl (hourly at most).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use futures::channel::mpsc::UnboundedSender;
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::{
    search::{Act, Icon, Item},
    win::Msg,
};

/// One indexed path; the name is its tail, so it isn't stored twice (~140k of these).
pub struct Entry {
    pub path: Box<str>,
    name_at: u32,
    pub dir: bool,
}

impl Entry {
    fn new(path: Box<str>, dir: bool) -> Self {
        let name_at = path.rfind('\\').map_or(0, |i| i + 1) as u32;
        Self { path, name_at, dir }
    }

    pub fn name(&self) -> &str {
        &self.path[self.name_at as usize..]
    }
}

pub struct Files {
    index: Arc<RwLock<Vec<Entry>>>,
    queries: mpsc::Sender<(u64, String, usize)>,
    pub count: Arc<AtomicUsize>,
    pub indexing: Arc<AtomicBool>,
    built: Arc<RwLock<Option<Instant>>>,
    roots: Vec<String>,
    exclude: Vec<String>,
    tx: UnboundedSender<Msg>,
    /// Whole-drive index from the admin helper; used instead of the folder index once ready.
    ntfs: Option<crate::ntfs::Client>,
    /// The settings this index was built for.
    cfg: crate::config::Files,
}

impl Files {
    pub fn start(cfg: &crate::config::Files, tx: UnboundedSender<Msg>) -> Self {
        let (roots, exclude) = (cfg.roots(), cfg.exclude.clone());
        let filter = crate::ntfs::Filter { exclude: exclude.clone(), skip: cfg.skip.clone(), drives: cfg.drives.clone() };
        let index = Arc::new(RwLock::new(Vec::new()));
        let (queries, rx) = mpsc::channel::<(u64, String, usize)>();
        let (count, indexing) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let ntfs = cfg.ntfs.then(|| crate::ntfs::connect(filter, count.clone(), indexing.clone(), tx.clone()));
        let worker_ntfs = ntfs.as_ref().map(|c| (c.index.clone(), c.ready.clone()));
        let worker_index = index.clone();
        let worker_tx = tx.clone();
        std::thread::spawn(move || {
            let tx = worker_tx;
            let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT.match_paths());
            while let Ok(mut job) = rx.recv() {
                // Only the latest keystroke matters.
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let (generation, query, max) = job;
                let items = match &worker_ntfs {
                    Some((ntfs, ready)) if ready.load(Ordering::SeqCst) => ntfs.read().unwrap().search(&query, max),
                    _ => search(&worker_index.read().unwrap(), &query, max, &mut matcher),
                };
                if tx.unbounded_send(Msg::Files(generation, items)).is_err() {
                    return;
                }
            }
        });
        let files = Self {
            index,
            queries,
            count,
            indexing,
            built: Arc::new(RwLock::new(None)),
            roots,
            exclude,
            tx,
            ntfs,
            cfg: cfg.clone(),
        };
        // With the helper installed its whole-drive index takes over; the folder index is
        // only built if the helper doesn't answer (on the next File Search).
        if !(files.ntfs.is_some() && crate::ntfs::installed()) {
            // Searchable at once from the saved index; the crawl below refreshes it.
            let saved = load(&cache_path(), &cache_key(&files.roots, &files.exclude)).unwrap_or_default();
            files.count.store(saved.len(), Ordering::SeqCst);
            *files.index.write().unwrap() = saved;
            files.refresh_if_stale(Duration::ZERO);
            files.watch();
        }
        files
    }

    pub fn search(&self, generation: u64, query: &str, max: usize) {
        let _ = self.queries.send((generation, query.to_string(), max));
    }

    /// Re-crawls in the background when the index is older than `max_age`, then sends
    /// `Msg::FilesIndexed` so the UI can re-run its current query.
    pub fn refresh_if_stale(&self, max_age: Duration) {
        if self.whole_drives() {
            return;
        }
        let fresh = self.built.read().unwrap().is_some_and(|t| t.elapsed() < max_age);
        if fresh || self.indexing.swap(true, Ordering::SeqCst) {
            return;
        }
        let (index, count, indexing, built) = (self.index.clone(), self.count.clone(), self.indexing.clone(), self.built.clone());
        let (roots, exclude, tx) = (self.roots.clone(), self.exclude.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let entries = crawl(&roots, &exclude);
            let _ = save(&cache_path(), &cache_key(&roots, &exclude), &entries);
            count.store(entries.len(), Ordering::SeqCst);
            *index.write().unwrap() = entries;
            *built.write().unwrap() = Some(Instant::now());
            indexing.store(false, Ordering::SeqCst);
            let _ = tx.unbounded_send(Msg::FilesIndexed);
        });
    }

    /// One watcher thread per root. Each ends at the first change after `Files` is dropped.
    fn watch(&self) {
        for root in &self.roots {
            let (root, exclude, tx) = (root.clone(), self.exclude.clone(), self.tx.clone());
            let (index, count, built) = (Arc::downgrade(&self.index), self.count.clone(), self.built.clone());
            std::thread::spawn(move || {
                crate::win::watch_dir(Path::new(&root), true, |changes| {
                    let Some(index) = index.upgrade() else { return false };
                    if changes.is_empty() {
                        // Changes were lost: the next File Search re-crawls.
                        *built.write().unwrap() = None;
                        return true;
                    }
                    let mut index = index.write().unwrap();
                    if !apply(&mut index, &root, &exclude, changes) {
                        return true;
                    }
                    count.store(index.len(), Ordering::SeqCst);
                    tx.unbounded_send(Msg::FilesIndexed).is_ok()
                })
            });
        }
    }

    /// Built for these settings (the hotkey doesn't matter to the index).
    pub fn same_settings(&self, cfg: &crate::config::Files) -> bool {
        let strip = |c: &crate::config::Files| crate::config::Files { hotkey: String::new(), ..c.clone() };
        strip(&self.cfg) == strip(cfg)
    }

    /// Searching every NTFS drive (the admin helper is connected).
    pub fn whole_drives(&self) -> bool {
        self.ntfs.as_ref().is_some_and(|c| c.ready.load(Ordering::SeqCst))
    }
}

pub fn crawl(roots: &[String], exclude: &[String]) -> Vec<Entry> {
    let exclude: Vec<String> = exclude.iter().map(|e| e.to_lowercase()).collect();
    let mut out = Vec::new();
    for root in roots {
        let skip = exclude.clone();
        let walk = jwalk::WalkDir::new(root).skip_hidden(false).follow_links(false).process_read_dir(move |_, _, _, children| {
            children.retain(|c| c.as_ref().is_ok_and(|e| !skip.contains(&e.file_name.to_string_lossy().to_lowercase())));
        });
        for entry in walk.into_iter().flatten().skip(1) {
            out.push(Entry::new(entry.path().to_string_lossy().into(), entry.file_type.is_dir()));
        }
    }
    out.shrink_to_fit();
    out
}

/// Applies one batch of watcher changes (paths relative to `root`). Returns whether the
/// index changed. Each changed path is dropped with everything under it, then added back
/// (a folder with its contents) if it exists now, so renames, moves and repeats come out right.
pub fn apply(index: &mut Vec<Entry>, root: &str, exclude: &[String], changes: Vec<PathBuf>) -> bool {
    let exclude: Vec<String> = exclude.iter().map(|e| e.to_lowercase()).collect();
    let mut dropped: HashSet<String> = HashSet::new();
    let mut added = vec![];
    for c in changes {
        if c.components().any(|p| exclude.contains(&p.as_os_str().to_string_lossy().to_lowercase())) {
            continue;
        }
        let full = Path::new(root).join(&c).to_string_lossy().into_owned();
        if !dropped.insert(full.clone()) {
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&full) {
            if meta.is_dir() {
                added.extend(crawl(std::slice::from_ref(&full), &exclude));
            }
            added.push(Entry::new(full.into(), meta.is_dir()));
        }
    }
    if dropped.is_empty() {
        return false;
    }
    dropped.extend(added.iter().map(|e| e.path.to_string()));
    let under_dropped = |path: &str| {
        let mut p = path;
        loop {
            if dropped.contains(p) {
                return true;
            }
            match p.rfind('\\') {
                Some(i) if p.len() > root.len() => p = &p[..i],
                _ => return false,
            }
        }
    };
    index.retain(|e| !under_dropped(&e.path));
    index.extend(added);
    true
}

fn cache_path() -> PathBuf {
    crate::config::dir().join("files.bin")
}

/// The cache is only used for the same roots and exclusions.
fn cache_key(roots: &[String], exclude: &[String]) -> String {
    format!("procast-files-1\n{}\n{}", roots.join("|"), exclude.join("|"))
}

/// Layout: the key, then per entry a dir flag and the path, strings as u32 length + UTF-8.
pub fn save(path: &Path, key: &str, entries: &[Entry]) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(entries.iter().map(|e| e.path.len() + 5).sum::<usize>() + key.len() + 4);
    out.extend((key.len() as u32).to_le_bytes());
    out.extend(key.as_bytes());
    for e in entries {
        out.push(e.dir as u8);
        out.extend((e.path.len() as u32).to_le_bytes());
        out.extend(e.path.as_bytes());
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(tmp, path)
}

/// The saved index, if it was saved for `key` and parses.
pub fn load(path: &Path, key: &str) -> Option<Vec<Entry>> {
    let data = std::fs::read(path).ok()?;
    let mut at = 0;
    let text = |at: &mut usize| -> Option<&str> {
        let len = u32::from_le_bytes(data.get(*at..*at + 4)?.try_into().ok()?) as usize;
        let s = std::str::from_utf8(data.get(*at + 4..*at + 4 + len)?).ok()?;
        *at += 4 + len;
        Some(s)
    };
    if text(&mut at)? != key {
        return None;
    }
    let mut out = vec![];
    while at < data.len() {
        let dir = data[at] == 1;
        at += 1;
        out.push(Entry::new(text(&mut at)?.into(), dir));
    }
    Some(out)
}

/// Matches file names; a query with a path separator matches the whole path instead.
pub fn search(index: &[Entry], query: &str, max: usize, matcher: &mut Matcher) -> Vec<Item> {
    let q = query.trim();
    if q.is_empty() {
        return vec![];
    }
    let whole_path = q.contains(['\\', '/']);
    let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    let mut hits: Vec<(u32, &Entry)> = index
        .iter()
        .filter_map(|e| Some((pattern.score(Utf32Str::new(if whole_path { &e.path } else { e.name() }, &mut buf), matcher)?, e)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.len().cmp(&b.1.path.len())));
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    hits.into_iter().take(max).map(|(_, e)| item(&e.path, e.dir, &home)).collect()
}

/// A result row: name, its folder (with ~ for your user folder), file or folder icon.
pub fn item(path: &str, dir: bool, home: &str) -> Item {
    let p = Path::new(path);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    let parent = p.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let parent = if !home.is_empty() && parent.starts_with(home) { format!("~{}", &parent[home.len()..]) } else { parent };
    use crate::design::g;
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (glyph, kind) = match ext.as_str() {
        _ if dir => (g::FOLDER, "Folder"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "heic" => (g::PICTURE, "Image"),
        "mp4" | "mkv" | "mov" | "avi" | "webm" => (g::VIDEO, "Video"),
        "mp3" | "wav" | "flac" | "m4a" | "ogg" => (g::AUDIO, "Audio"),
        "zip" | "7z" | "rar" | "tar" | "gz" => (g::ZIP, "Archive"),
        "pdf" => (g::PDF, "PDF"),
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "json" | "toml" | "yaml" | "yml" | "html" | "css" | "c" | "cpp" | "h" | "cs" | "go" | "java" | "sh" | "ps1" => (g::CODE, "Code"),
        "exe" | "msi" | "lnk" => (g::APP, "Program"),
        _ => (g::DOCUMENT, "File"),
    };
    Item::new(name, parent, Icon::Vector(glyph), Act::File(path.to_string())).acc(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crawl_skips_excluded_and_search_ranks_names() {
        let root = std::env::temp_dir().join(format!("procast-files-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for p in ["docs/Quarterly Report.docx", "docs/report-draft.txt", "node_modules/pkg/report.js", "photos/beach.jpg"] {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "").unwrap();
        }
        let index = crawl(&[root.to_string_lossy().into_owned()], &["node_modules".into()]);
        let _ = std::fs::remove_dir_all(&root);
        assert!(index.iter().all(|e| !e.path.contains("node_modules")), "excluded folders are not walked");
        assert!(index.iter().any(|e| e.dir && e.name() == "photos"));

        let mut m = Matcher::new(nucleo_matcher::Config::DEFAULT.match_paths());
        let hits = search(&index, "report", 10, &mut m);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| matches!(&h.act, Act::File(p) if p.contains("docs"))));
        assert_eq!(search(&index, "photos\\beach", 10, &mut m)[0].title, "beach.jpg");
        assert!(search(&index, "  ", 10, &mut m).is_empty());
    }

    #[test]
    fn saved_index_round_trips_and_watcher_changes_apply() {
        let root = std::env::temp_dir().join(format!("procast-live-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for p in ["a/one.txt", "a/sub/two.txt", "b.txt", "skip/x.txt"] {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "").unwrap();
        }
        let r = root.to_string_lossy().into_owned();
        let ex = vec!["skip".to_string()];
        let mut index = crawl(std::slice::from_ref(&r), &ex);
        let names = |ix: &[Entry]| {
            let mut v: Vec<String> = ix.iter().map(|e| e.path[r.len() + 1..].replace('\\', "/")).collect();
            v.sort();
            v
        };

        let file = root.with_extension("bin");
        save(&file, "k", &index).unwrap();
        assert_eq!(names(&load(&file, "k").unwrap()), names(&index));
        assert!(load(&file, "other roots").is_none());
        let _ = std::fs::remove_file(&file);

        // Folder renamed (its contents move with it), file deleted, file created twice.
        std::fs::rename(root.join("a"), root.join("c")).unwrap();
        std::fs::remove_file(root.join("b.txt")).unwrap();
        std::fs::write(root.join("new.txt"), "").unwrap();
        std::fs::write(root.join("skip/y.txt"), "").unwrap();
        let ch = PathBuf::from;
        let changed = apply(&mut index, &r, &ex, vec![ch("a"), ch("c"), ch("b.txt"), ch("new.txt"), ch("new.txt")]);
        assert!(changed);
        assert!(!apply(&mut index, &r, &ex, vec![ch("skip\\y.txt")]), "excluded folders are ignored");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(names(&index), ["c", "c/one.txt", "c/sub", "c/sub/two.txt", "new.txt"]);
    }

    #[test]
    fn watcher_reports_new_files() {
        let root = std::env::temp_dir().join(format!("procast-watch-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("deep")).unwrap();
        let (tx, rx) = mpsc::channel();
        let dir = root.clone();
        std::thread::spawn(move || crate::win::watch_dir(&dir, true, |c| tx.send(c).is_ok()));
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(root.join("deep").join("hello.txt"), "").unwrap();
        let got = rx.recv_timeout(Duration::from_secs(5)).expect("a change batch");
        let _ = std::fs::remove_dir_all(&root);
        assert!(got.contains(&PathBuf::from("deep\\hello.txt")), "{got:?}");
    }
}
