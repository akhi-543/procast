//! File search: an in-memory index of file and folder names under the configured roots,
//! searched on a worker thread. Only the newest query's results are ever delivered
//! (generation counter), so typing never waits on a slow match.
// ponytail: full re-crawl to refresh (on entering file search, at most every 2 min);
// switch to the USN journal or ReadDirectoryChangesW if crawls get slow or stale.

use std::{
    path::Path,
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
}

impl Files {
    pub fn start(roots: Vec<String>, exclude: Vec<String>, tx: UnboundedSender<Msg>) -> Self {
        let index = Arc::new(RwLock::new(Vec::new()));
        let (queries, rx) = mpsc::channel::<(u64, String, usize)>();
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
                let items = search(&worker_index.read().unwrap(), &query, max, &mut matcher);
                if tx.unbounded_send(Msg::Files(generation, items)).is_err() {
                    return;
                }
            }
        });
        let files = Self {
            index,
            queries,
            count: Arc::new(AtomicUsize::new(0)),
            indexing: Arc::new(AtomicBool::new(false)),
            built: Arc::new(RwLock::new(None)),
            roots,
            exclude,
            tx,
        };
        files.refresh_if_stale(Duration::ZERO);
        files
    }

    pub fn search(&self, generation: u64, query: &str, max: usize) {
        let _ = self.queries.send((generation, query.to_string(), max));
    }

    /// Re-crawls in the background when the index is older than `max_age`, then sends
    /// `Msg::FilesIndexed` so the UI can re-run its current query.
    pub fn refresh_if_stale(&self, max_age: Duration) {
        let fresh = self.built.read().unwrap().is_some_and(|t| t.elapsed() < max_age);
        if fresh || self.indexing.swap(true, Ordering::SeqCst) {
            return;
        }
        let (index, count, indexing, built) = (self.index.clone(), self.count.clone(), self.indexing.clone(), self.built.clone());
        let (roots, exclude, tx) = (self.roots.clone(), self.exclude.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let entries = crawl(&roots, &exclude);
            count.store(entries.len(), Ordering::SeqCst);
            *index.write().unwrap() = entries;
            *built.write().unwrap() = Some(Instant::now());
            indexing.store(false, Ordering::SeqCst);
            let _ = tx.unbounded_send(Msg::FilesIndexed);
        });
    }

    pub fn same_roots(&self, roots: &[String], exclude: &[String]) -> bool {
        self.roots == roots && self.exclude == exclude
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
            let path: Box<str> = entry.path().to_string_lossy().into();
            let name_at = path.rfind('\\').map_or(0, |i| i + 1) as u32;
            out.push(Entry { path, name_at, dir: entry.file_type.is_dir() });
        }
    }
    out.shrink_to_fit();
    out
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
    hits.into_iter()
        .take(max)
        .map(|(_, e)| {
            let parent = Path::new(&*e.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let parent = if !home.is_empty() && parent.starts_with(&home) { format!("~{}", &parent[home.len()..]) } else { parent };
            Item::new(e.name(), parent, Icon::glyph(if e.dir { "📁" } else { "📄" }), Act::File(e.path.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crawl_skips_excluded_and_search_ranks_names() {
        let root = std::env::temp_dir().join(format!("akscast-files-test-{}", std::process::id()));
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
}
