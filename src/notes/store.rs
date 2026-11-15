//! Notes on disk: `<root>/<id>.md` per note, `notes.json` with the metadata, and
//! `history/<id>/<unix>.md` snapshots. Times are unix seconds and passed in by the caller.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::focus::write_atomic;

pub type Id = String;

/// One snapshot per this many seconds of editing, and this many kept per note.
const SNAPSHOT_EVERY: i64 = 5 * 60;
const SNAPSHOT_KEEP: usize = 50;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteMeta {
    pub id: Id,
    #[serde(default)]
    pub pinned: bool,
    pub created: i64,
    pub modified: i64,
    #[serde(default)]
    pub deleted_at: Option<i64>,
    /// First line without markdown; kept in memory only, rebuilt from the file.
    #[serde(skip)]
    pub title: String,
}

/// `create` found an empty note already: that is the one to open.
#[derive(Debug, PartialEq)]
pub struct Exists(pub Id);

pub struct Store {
    root: PathBuf,
    metas: Vec<NoteMeta>,
    /// Largest id handed out or found, as a number; ids only ever go up.
    last: u64,
}

impl Store {
    /// Opens (creating if needed) the notes folder. A missing or broken `notes.json` is rebuilt
    /// from the `.md` files, and files and entries that disagree are reconciled, so no note is lost.
    pub fn open(root: PathBuf) -> io::Result<Store> {
        fs::create_dir_all(&root)?;
        let read: Option<Vec<NoteMeta>> = fs::read_to_string(root.join("notes.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        let had = read.is_some();
        let mut metas = read.unwrap_or_default();
        let before = metas.len();
        metas.retain(|m| root.join(format!("{}.md", m.id)).is_file());
        let mut changed = !had || metas.len() != before;
        for e in fs::read_dir(&root)?.flatten() {
            let path = e.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else { continue };
            let md = path.extension().is_some_and(|x| x == "md");
            if !md || id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) || metas.iter().any(|m| m.id == id) {
                continue;
            }
            let t = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            metas.push(NoteMeta { id, pinned: false, created: t, modified: t, deleted_at: None, title: String::new() });
            changed = true;
        }
        for m in &mut metas {
            m.title = title_of(&fs::read_to_string(root.join(format!("{}.md", m.id))).unwrap_or_default());
        }
        let last = metas.iter().filter_map(|m| u64::from_str_radix(&m.id, 36).ok()).max().unwrap_or(0);
        let store = Store { root, metas, last };
        if changed {
            store.persist()?;
        }
        Ok(store)
    }

    /// Live notes: pinned first, then newest edit first.
    pub fn list(&self) -> Vec<NoteMeta> {
        let mut v: Vec<NoteMeta> = self.metas.iter().filter(|m| m.deleted_at.is_none()).cloned().collect();
        v.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.modified.cmp(&a.modified)).then(b.id.cmp(&a.id)));
        v
    }

    /// Recently deleted, newest deletion first.
    pub fn deleted(&self) -> Vec<NoteMeta> {
        let mut v: Vec<NoteMeta> = self.metas.iter().filter(|m| m.deleted_at.is_some()).cloned().collect();
        v.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at).then(b.id.cmp(&a.id)));
        v
    }

    pub fn text(&self, id: &str) -> String {
        fs::read_to_string(self.path(id)).unwrap_or_default()
    }

    /// A new empty note, unless one exists already (then its id comes back as `Exists`).
    pub fn create(&mut self, now: i64) -> io::Result<Result<Id, Exists>> {
        let empty = self.list().into_iter().find(|m| self.text(&m.id).trim().is_empty());
        if let Some(m) = empty {
            return Ok(Err(Exists(m.id)));
        }
        self.add("", now).map(Ok)
    }

    pub fn duplicate(&mut self, id: &str, now: i64) -> io::Result<Id> {
        self.index(id)?;
        let text = self.text(id);
        self.add(&text, now)
    }

    pub fn save(&mut self, id: &str, text: &str, now: i64) -> io::Result<()> {
        let i = self.index(id)?;
        write_atomic(&self.path(id), text)?;
        self.metas[i].modified = now;
        self.metas[i].title = title_of(text);
        self.snapshot(id, text, now)?;
        self.persist()
    }

    pub fn pin(&mut self, id: &str, pinned: bool) -> io::Result<()> {
        let i = self.index(id)?;
        self.metas[i].pinned = pinned;
        self.persist()
    }

    /// Soft delete: the note moves to `deleted()` until restored or purged.
    pub fn delete(&mut self, id: &str, now: i64) -> io::Result<()> {
        let i = self.index(id)?;
        self.metas[i].deleted_at = Some(now);
        self.persist()
    }

    pub fn restore(&mut self, id: &str) -> io::Result<()> {
        let i = self.index(id)?;
        self.metas[i].deleted_at = None;
        self.persist()
    }

    /// Removes the note for good: file, history and metadata.
    pub fn purge(&mut self, id: &str) -> io::Result<()> {
        let i = self.index(id)?;
        let md = self.path(id);
        if md.exists() {
            fs::remove_file(md)?;
        }
        let _ = fs::remove_dir_all(self.history_dir(id));
        self.metas.remove(i);
        self.persist()
    }

    /// Snapshots, newest first, as (unix seconds, text).
    pub fn history(&self, id: &str) -> Vec<(i64, String)> {
        let mut v: Vec<(i64, String)> = self
            .snapshots(id)
            .into_iter()
            .filter_map(|t| Some((t, fs::read_to_string(self.history_dir(id).join(format!("{t}.md"))).ok()?)))
            .collect();
        v.reverse();
        v
    }

    /// Live notes whose title or text contains `q`, ignoring case; ordered like `list()`.
    pub fn search(&self, q: &str) -> Vec<NoteMeta> {
        let q = q.trim().to_lowercase();
        self.list()
            .into_iter()
            .filter(|m| q.is_empty() || m.title.to_lowercase().contains(&q) || self.text(&m.id).to_lowercase().contains(&q))
            .collect()
    }

    fn add(&mut self, text: &str, now: i64) -> io::Result<Id> {
        let n = ((now.max(0) as u64) * 1000).max(self.last + 1);
        let id = base36(n);
        write_atomic(&self.root.join(format!("{id}.md")), text)?;
        self.last = n;
        self.metas.push(NoteMeta { id: id.clone(), pinned: false, created: now, modified: now, deleted_at: None, title: title_of(text) });
        self.persist()?;
        Ok(id)
    }

    fn index(&self, id: &str) -> io::Result<usize> {
        self.metas
            .iter()
            .position(|m| m.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such note"))
    }

    fn path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.md"))
    }

    fn history_dir(&self, id: &str) -> PathBuf {
        self.root.join("history").join(id)
    }

    fn persist(&self) -> io::Result<()> {
        let json = serde_json::to_string_pretty(&self.metas).map_err(io::Error::other)?;
        write_atomic(&self.root.join("notes.json"), &json)
    }

    /// Snapshot times of a note, oldest first.
    fn snapshots(&self, id: &str) -> Vec<i64> {
        let mut v: Vec<i64> = fs::read_dir(self.history_dir(id))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.path().file_stem()?.to_str()?.parse().ok())
            .collect();
        v.sort_unstable();
        v
    }

    /// Writes a snapshot of `text` unless the last one is under five minutes old; keeps the newest 50.
    fn snapshot(&self, id: &str, text: &str, now: i64) -> io::Result<()> {
        let times = self.snapshots(id);
        if times.last().is_some_and(|t| now - t < SNAPSHOT_EVERY) || text.trim().is_empty() {
            return Ok(());
        }
        let dir = self.history_dir(id);
        fs::create_dir_all(&dir)?;
        write_atomic(&dir.join(format!("{now}.md")), text)?;
        let total = times.len() + 1;
        for t in times.iter().take(total.saturating_sub(SNAPSHOT_KEEP)) {
            let _ = fs::remove_file(dir.join(format!("{t}.md")));
        }
        Ok(())
    }
}

fn base36(mut n: u64) -> String {
    let mut out = Vec::new();
    loop {
        out.push(char::from_digit((n % 36) as u32, 36).unwrap_or('0'));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.iter().rev().collect()
}

/// First line with something in it, without the markdown around it; "Untitled" for an empty note.
pub fn title_of(text: &str) -> String {
    text.lines().map(strip_line).find(|l| !l.is_empty()).unwrap_or_else(|| "Untitled".into())
}

fn strip_line(line: &str) -> String {
    let mut s = line.trim();
    loop {
        let before = s;
        s = s.trim_start_matches('#').trim_start();
        s = s.strip_prefix('>').unwrap_or(s).trim_start();
        for m in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
            s = s.strip_prefix(m).unwrap_or(s).trim_start();
        }
        let digits = s.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            s = s[digits..].strip_prefix(". ").unwrap_or(s).trim_start();
        }
        if s == before {
            break;
        }
    }
    let mut out = s.to_owned();
    for t in ["**", "~~", "==", "`", "<u>", "</u>", "*"] {
        out = out.replace(t, "");
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("procast-notes-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn open(name: &str) -> Store {
        Store::open(tmp(name)).unwrap()
    }

    fn made(r: io::Result<Result<Id, Exists>>) -> Id {
        r.unwrap().unwrap()
    }

    #[test]
    fn create_twice_returns_the_empty_note() {
        let mut s = open("create");
        let a = made(s.create(100));
        assert_eq!(s.create(101).unwrap(), Err(Exists(a.clone())));
        s.save(&a, "hello", 102).unwrap();
        let b = made(s.create(103));
        assert_ne!(a, b);
        assert_eq!(s.list().len(), 2);
    }

    #[test]
    fn ids_are_monotonic_even_within_one_second() {
        let mut s = open("ids");
        let a = made(s.create(100));
        s.save(&a, "x", 100).unwrap();
        let b = s.duplicate(&a, 100).unwrap();
        let c = s.duplicate(&a, 50).unwrap();
        let n = |i: &str| u64::from_str_radix(i, 36).unwrap();
        assert!(n(&a) < n(&b) && n(&b) < n(&c));
        assert!(a.bytes().all(|b| b.is_ascii_alphanumeric()));
    }

    #[test]
    fn delete_restore_purge() {
        let mut s = open("delete");
        let a = made(s.create(1));
        s.save(&a, "one", 2).unwrap();
        s.delete(&a, 3).unwrap();
        assert!(s.list().is_empty());
        assert_eq!(s.deleted().len(), 1);
        assert_eq!(s.deleted()[0].deleted_at, Some(3));
        s.restore(&a).unwrap();
        assert_eq!(s.list().len(), 1);
        assert!(s.deleted().is_empty());
        s.delete(&a, 4).unwrap();
        s.purge(&a).unwrap();
        assert!(!s.path(&a).exists());
        assert!(!s.history_dir(&a).exists());
        assert!(s.list().is_empty() && s.deleted().is_empty());
        assert!(s.purge(&a).is_err());
        // purged ids are not handed out again
        let b = made(s.create(0));
        assert_ne!(a, b);
    }

    #[test]
    fn list_order_pin_then_modified() {
        let mut s = open("order");
        let a = made(s.create(1));
        s.save(&a, "a", 1).unwrap();
        let b = made(s.create(2));
        s.save(&b, "b", 2).unwrap();
        let c = made(s.create(3));
        s.save(&c, "c", 3).unwrap();
        let ids = |s: &Store| s.list().into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(&s), [c.clone(), b.clone(), a.clone()]);
        s.pin(&a, true).unwrap();
        assert_eq!(ids(&s), [a.clone(), c.clone(), b.clone()]);
        s.save(&b, "b2", 9).unwrap();
        assert_eq!(ids(&s), [a.clone(), b.clone(), c.clone()]);
        s.pin(&a, false).unwrap();
        assert_eq!(ids(&s), [b, c, a]);
    }

    #[test]
    fn duplicate_copies_text_unpinned() {
        let mut s = open("dup");
        let a = made(s.create(1));
        s.save(&a, "# Hi", 2).unwrap();
        s.pin(&a, true).unwrap();
        let b = s.duplicate(&a, 3).unwrap();
        assert_eq!(s.text(&b), "# Hi");
        let m = s.list().into_iter().find(|m| m.id == b).unwrap();
        assert!(!m.pinned);
        assert_eq!(m.title, "Hi");
        assert!(s.duplicate("nope", 4).is_err());
    }

    #[test]
    fn titles_are_stripped_of_markdown() {
        assert_eq!(title_of("# Hello **world**"), "Hello world");
        assert_eq!(title_of("\n\n  ## ~~Old~~ ==new== `code` <u>under</u> *it*\nbody"), "Old new code under it");
        assert_eq!(title_of("- [x] done"), "done");
        assert_eq!(title_of("- [ ] todo"), "todo");
        assert_eq!(title_of("> quote"), "quote");
        assert_eq!(title_of("* item"), "item");
        assert_eq!(title_of("12. numbered"), "numbered");
        assert_eq!(title_of("# \n---\nreal"), "---");
        assert_eq!(title_of("##\n  \nsecond"), "second");
        assert_eq!(title_of("# Ünï **çödé** 日本語"), "Ünï çödé 日本語");
        assert_eq!(title_of(""), "Untitled");
        assert_eq!(title_of("  \n\n"), "Untitled");
        assert_eq!(title_of("# "), "Untitled");
    }

    #[test]
    fn history_throttles_to_one_per_five_minutes() {
        let mut s = open("history");
        let a = made(s.create(1000));
        s.save(&a, "v1", 1000).unwrap();
        s.save(&a, "v2", 1100).unwrap();
        s.save(&a, "v3", 1299).unwrap();
        assert_eq!(s.history(&a), [(1000, "v1".to_string())]);
        s.save(&a, "v4", 1300).unwrap();
        assert_eq!(s.history(&a), [(1300, "v4".to_string()), (1000, "v1".to_string())]);
        assert_eq!(s.text(&a), "v4");
    }

    #[test]
    fn history_keeps_the_newest_fifty() {
        let mut s = open("keep");
        let a = made(s.create(0));
        for i in 0..60 {
            s.save(&a, &format!("v{i}"), 1000 + i * 300).unwrap();
        }
        let h = s.history(&a);
        assert_eq!(h.len(), 50);
        assert_eq!(h[0].1, "v59");
        assert_eq!(h[49].1, "v10");
    }

    #[test]
    fn purge_removes_history() {
        let mut s = open("purgehist");
        let a = made(s.create(0));
        s.save(&a, "x", 10).unwrap();
        assert_eq!(s.history(&a).len(), 1);
        s.purge(&a).unwrap();
        assert!(s.history(&a).is_empty());
    }

    #[test]
    fn search_is_case_insensitive_and_non_ascii_safe() {
        let mut s = open("search");
        let a = made(s.create(1));
        s.save(&a, "# Groceries\nbuy ÄPFEL and milk", 1).unwrap();
        let b = made(s.create(2));
        s.save(&b, "# Plans\nmeeting notes", 2).unwrap();
        let c = made(s.create(3));
        s.save(&c, "Äpfel in the title", 3).unwrap();
        let ids = |s: &Store, q: &str| s.search(q).into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(&s, "äpfel"), [c.clone(), a.clone()]);
        assert_eq!(ids(&s, "PLANS"), std::slice::from_ref(&b));
        assert_eq!(ids(&s, "MEETING"), std::slice::from_ref(&b));
        assert!(ids(&s, "nothing").is_empty());
        assert_eq!(ids(&s, "").len(), 3);
        s.delete(&b, 4).unwrap();
        assert!(ids(&s, "plans").is_empty());
    }

    #[test]
    fn reopening_keeps_everything() {
        let dir = tmp("reopen");
        let mut s = Store::open(dir.clone()).unwrap();
        let a = made(s.create(5));
        s.save(&a, "# Kept", 6).unwrap();
        s.pin(&a, true).unwrap();
        let b = s.duplicate(&a, 7).unwrap();
        s.delete(&b, 8).unwrap();
        let s2 = Store::open(dir).unwrap();
        assert_eq!(s2.list().len(), 1);
        assert!(s2.list()[0].pinned);
        assert_eq!(s2.list()[0].title, "Kept");
        assert_eq!(s2.deleted()[0].id, b);
        assert!(s2.last >= u64::from_str_radix(&b, 36).unwrap());
    }

    #[test]
    fn corrupt_or_missing_metadata_is_rebuilt_from_the_files() {
        let dir = tmp("rebuild");
        let mut s = Store::open(dir.clone()).unwrap();
        let a = made(s.create(5));
        s.save(&a, "# Alpha", 6).unwrap();
        let b = made(s.create(7));
        s.save(&b, "Beta", 8).unwrap();
        drop(s);
        fs::write(dir.join("notes.json"), "{ not json").unwrap();
        let s = Store::open(dir.clone()).unwrap();
        let mut ids: Vec<_> = s.list().into_iter().map(|m| m.id).collect();
        ids.sort();
        let mut want = vec![a.clone(), b.clone()];
        want.sort();
        assert_eq!(ids, want);
        assert!(s.list().iter().any(|m| m.title == "Alpha"));
        drop(s);
        fs::remove_file(dir.join("notes.json")).unwrap();
        assert_eq!(Store::open(dir.clone()).unwrap().list().len(), 2);
        // the rebuilt file is valid json again
        assert!(serde_json::from_str::<Vec<NoteMeta>>(&fs::read_to_string(dir.join("notes.json")).unwrap()).is_ok());
    }

    #[test]
    fn metadata_and_files_are_reconciled() {
        let dir = tmp("reconcile");
        let mut s = Store::open(dir.clone()).unwrap();
        let a = made(s.create(5));
        s.save(&a, "gone soon", 6).unwrap();
        let b = made(s.create(7));
        s.save(&b, "stays", 8).unwrap();
        drop(s);
        fs::remove_file(dir.join(format!("{a}.md"))).unwrap();
        fs::write(dir.join("stray.md"), "# Stray").unwrap();
        fs::write(dir.join("notes.md.bak"), "ignored").unwrap();
        let s = Store::open(dir).unwrap();
        let mut ids: Vec<_> = s.list().into_iter().map(|m| m.id).collect();
        ids.sort();
        let mut want = vec![b, "stray".to_string()];
        want.sort();
        assert_eq!(ids, want);
    }

    #[test]
    fn unknown_ids_are_errors_not_panics() {
        let mut s = open("unknown");
        assert!(s.save("../x", "t", 1).is_err());
        assert!(s.pin("zz", true).is_err());
        assert!(s.delete("zz", 1).is_err());
        assert!(s.restore("zz").is_err());
        assert_eq!(s.text("zz"), "");
    }

    #[test]
    fn base36_roundtrip() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        assert_eq!(u64::from_str_radix(&base36(1_700_000_000_123), 36).unwrap(), 1_700_000_000_123);
    }
}
