# Import once, then Liked Songs without Chrome

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One **Import YouTube Music Session**, while the browser extension is connected and YouTube Music is signed in, writes `%APPDATA%\procast\ytmusic\session.json`. After that, the Library shelf **Liked Songs** loads in-process with Chrome closed.

**Architecture:** Keep the existing command (`Cmd::YtImportSession` → companion `exportSession` → `ytmusic_api::save_session`). Do not add a second import path. Make the saved cookies actually authorize InnerTube, then refetch the open library shelf so `VLLM` is requested with that session. Search and playback stay as they are: unsigned InnerTube search, desktop app for play.

**Tech Stack:** Rust 2024, existing `ureq` + `serde_json`, the browser companion op `exportSession` already in `browser/ytmusic.js`. No new crate.

**Spec:** This plan. The user-approved behavior is: import once with the extension connected and YouTube Music signed in; cookies land in `session.json`; Liked Songs then loads with no Chrome. Prior plan `docs/superpowers/plans/2026-11-17-ytmusic-desktop-innertube.md` already shipped the command, the file, and `Shelf::Liked` = `VLLM`. This plan only closes the two gaps that still leave that shelf empty after a successful import.

## Global Constraints

- Work only in `D:\Pro\akscast\.claude\worktrees\ytmusic` on `feat/ytmusic`. Never touch `D:\Pro\akscast` root. Never push or merge without asking.
- No new crate. No new process. Do not launch the desktop app or Chrome.
- Cookie values and the desktop token are never logged and never put in an error string.
- `session.json` stays a JSON object of cookie name → value under `%APPDATA%\procast\ytmusic\`. Refuse to write it when neither `SAPISID` nor `__Secure-3PAPISID` is non-empty. Keep the current-user ACL (`icacls`); a failed ACL does not fail the import.
- Liked Songs browse id is exactly `VLLM`. Albums stay `FEmusic_liked_albums`, Playlists `FEmusic_liked_playlists`, Artists `FEmusic_library_corpus_track_artists`.
- Unsigned library browse of those ids still returns the existing sign-in string: `Sign in to music.youtube.com in this browser to see your library`.
- Before cargo: `$env:CARGO_TARGET_DIR="E:\procast-ytmusic-target"`; `$env:TEMP=$env:TMP="E:\tmp"`.
- Few comments. Match surrounding style.

## Review Focus

- A session file with `SAPISID` but no `Origin` header is what makes a real account look like an empty library. Signed calls must send `Origin: https://music.youtube.com`.
- Import that succeeds while Library is already open must refetch that shelf. A toast alone leaves Liked Songs on the sign-in row.
- A signed-in user with zero likes is an empty list, not the sign-in row.
- Import with the extension disconnected toasts `No browser connected: run Set Up Browser Extension` and does not write a file.
- Chrome is not required on the next launch once `session.json` is present.

---

### Task 1: Signed InnerTube sends Origin

**Files:**
- Modify: `src/ytmusic_api.rs` (`call`, header helper next to `authorization`)
- Test: `src/ytmusic_api.rs` unit tests

**Interfaces:**
- Consumes: `authorization(sapisid, ts) -> String`, `Session::sapisid`, `Session::cookie_header`, `ORIGIN` = `https://music.youtube.com`
- Produces: `fn signed_headers(session: &Session, ts: u64) -> Vec<(String, String)>` — empty when `sapisid()` is `None`; otherwise `Authorization`, `Cookie`, and `Origin`

The browser companion sets `Origin` with a declarativeNetRequest rule because `fetch` cannot. `ureq` can. YouTube ignores `SAPISIDHASH` when `Origin` is missing, so a saved session still returns no library items.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn signed_headers_include_origin_and_sapisidhash() {
    let session = session_from([("SAPISID", "secret"), ("HSID", "h")]);
    let headers = signed_headers(&session, 1700000000);
    let get = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    assert_eq!(get("Authorization"), Some("SAPISIDHASH 1700000000_d2a0deeb95ea263a68778d9e9ac8840dce59d800"));
    assert_eq!(get("Origin"), Some("https://music.youtube.com"));
    assert!(get("Cookie").unwrap().contains("SAPISID=secret"));
    assert!(signed_headers(&session_from([("HSID", "h")]), 1).is_empty());
}
```

`session_from` is a test helper that builds a `Session` without reading `%APPDATA%`. Add `#[cfg(test)] pub(crate)` access or construct `Session` inside the test module. Do not read or delete the real `session.json`.

- [ ] **Step 2: Run the test**

Run: `cargo test --offline --bin procast signed_headers_include_origin -- --exact`

Expected: FAIL because `signed_headers` does not exist.

- [ ] **Step 3: Implement `signed_headers` and use it in `call`**

`call` keeps `Content-Type: application/json`, `X-Origin: https://music.youtube.com`, and `X-Goog-AuthUser: 0` on every request. When `load_session()` has a SAPISID, also set the three headers from `signed_headers`. Unsigned search sends no `Authorization`, no `Cookie`, and no `Origin`.

- [ ] **Step 4: Run the test**

Run: `cargo test --offline --bin procast signed_headers_include_origin -- --exact`

Expected: PASS. Then `cargo test --offline --bin procast ytmusic_api::tests`. Expected: the existing SAPISIDHASH and session-file tests still pass.

- [ ] **Step 5: Commit**

```bash
git commit -m "fix(ytmusic): send Origin with a saved YouTube Music session"
```

---

### Task 2: Import refetches Liked Songs

**Files:**
- Modify: `src/music_view.rs` (`music_reply` `exportSession` arm)
- Modify: `README.md` only if the Liked Songs sentence is missing (one sentence, do not rewrite the file)
- Test: `src/music_view.rs` and `src/ytmusic_api.rs`

**Interfaces:**
- Consumes: `ytmusic_api::save_session`, `Shelf::Liked.browse_id()` = `VLLM`, `Ask::Shelf`, `music_fetch`
- Produces: after `save_session` returns `Ok`, if `music.screen == Screen::Library` and no page is open, call `music_fetch(Ask::Shelf(current shelf))`. That shelf is **Liked Songs** when the user is on it, and whichever other shelf is showing otherwise.

`exportSession` today toasts `Imported YouTube Music session` and returns. The shelf stays on the sign-in row until the user leaves Library and comes back.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn vllm_with_items_is_not_a_sign_in_error() {
    let page = json!({"musicResponsiveListItemRenderer": {"playlistItemData": {"videoId": "abcdefghijk"}, "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "One More Time"}]}}}]}});
    let reply = browse_reply("VLLM", &page, true);
    assert!(reply.error.is_none());
    assert_eq!(reply.items[0].title, "One More Time");
    assert_eq!(reply.items[0].id, "abcdefghijk");
}

#[test]
fn import_success_reloads_the_open_library_shelf() {
    assert_eq!(shelf_after_import(Screen::Library, Some(Shelf::Liked), false), Some(Shelf::Liked));
    assert_eq!(shelf_after_import(Screen::Library, Some(Shelf::Albums), true), None);
    assert_eq!(shelf_after_import(Screen::Search, Some(Shelf::Liked), false), None);
}
```

`shelf_after_import(screen, shelf, page_open) -> Option<Shelf>` is a pure function. `page_open` means a page is showing on top of Library; then do not refetch.

`browse_reply` is already private. Test it through `pub(crate)` for the test module, or assert the same rule on a new `pub fn library_error(id: &str, item_count: usize, signed_in: bool) -> Option<&'static str>` that `browse_reply` calls. Prefer the small function so the test does not need a full InnerTube document. Exact rule: `Some(SIGN_IN)` only when `item_count == 0 && !signed_in && (id.starts_with("FEmusic_") || id == "VLLM")`. Otherwise `None`.

- [ ] **Step 2: Run the tests**

Run: `cargo test --offline --bin procast import_success_reloads vllm_with_items -- --test-threads=8`

`cargo test` accepts one filter. Run each name once.

Expected: FAIL because `shelf_after_import` / `library_error` do not exist.

- [ ] **Step 3: Implement the refetch**

In the `exportSession` arm, on `save_session` `Ok`: toast `Imported YouTube Music session`, then if `shelf_after_import` returns `Some(shelf)`, `music_fetch(Ask::Shelf(shelf))`. On `Err`, toast that error and do not fetch. Do not clear `session.json` on a later browse failure.

`browse_reply` uses `library_error`. A signed-in empty `VLLM` stays an empty list.

- [ ] **Step 4: Run the tests**

Run: `cargo test --offline --bin procast music_view::tests` and `cargo test --offline --bin procast ytmusic_api::tests`.

Expected: PASS, including the existing unsigned sign-in test.

- [ ] **Step 5: README sentence**

In the existing YouTube Music setup paragraph, say that after **Import YouTube Music Session**, Liked Songs loads with Chrome closed. Do not add a second setup section.

- [ ] **Step 6: Commit**

```bash
git commit -m "feat(ytmusic): reload the library after importing a session"
```

---

## Out of scope

- Building a queue UI or reading the desktop app's queue
- Liked Songs from `GET /api/v1/playlists` (that list does not include `VLLM`)
- Launching Chrome or the desktop app
- Push or merge
