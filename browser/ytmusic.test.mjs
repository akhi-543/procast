import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const here = (name) => new URL(name, import.meta.url);
const fixture = (name) => JSON.parse(readFileSync(here(`test-fixtures/${name}.json`), "utf8"));

function load(chrome = {}, fetch) {
  const sandbox = { crypto: webcrypto, TextEncoder, URL, URLSearchParams, chrome, fetch };
  vm.runInNewContext(readFileSync(here("ytmusic.js"), "utf8"), sandbox);
  return sandbox.ytm;
}

const ytm = load();

test("songs search", () => {
  const items = ytm.items(fixture("search-songs"));
  assert.ok(items.length >= 3);
  for (const it of items) {
    assert.equal(it.kind, "song");
    assert.equal(it.id.length, 11);
    assert.ok(it.title);
    assert.match(it.thumb, /^https:\/\//);
    assert.match(it.duration, /^\d+:\d{2}$/);
  }
});

test("albums search", () => {
  const items = ytm.items(fixture("search-albums"));
  assert.ok(items.length >= 3);
  for (const it of items) {
    assert.equal(it.kind, "album");
    assert.ok(it.id.startsWith("MPRE"));
    assert.ok(it.playlistId.startsWith("OLAK5uy_"));
  }
});

test("artists search", () => {
  const items = ytm.items(fixture("search-artists"));
  assert.ok(items.length >= 1);
  for (const it of items) {
    assert.equal(it.kind, "artist");
    assert.ok(it.id.startsWith("UC"));
  }
});

test("playlists search", () => {
  const items = ytm.items(fixture("search-playlists"));
  assert.ok(items.length >= 1);
  for (const it of items) {
    assert.equal(it.kind, "playlist");
    assert.ok(it.playlistId && !it.playlistId.startsWith("VL"));
  }
});

test("subtitles use the middle dot", () => {
  for (const it of ytm.items(fixture("search-songs"))) assert.ok(!it.subtitle.includes("•"));
});

test("album page", () => {
  const page = ytm.page(fixture("album"));
  assert.ok(page.title);
  assert.ok(page.playlistId.startsWith("OLAK5uy_"));
  const songs = page.items.filter((it) => it.kind === "song");
  assert.ok(songs.length >= 3);
  for (const song of songs) assert.match(song.thumb, /^https:\/\//);
});

test("SAPISIDHASH", async () => {
  const sha1 = createHash("sha1").update("1700000000 secret https://music.youtube.com").digest("hex");
  assert.equal(await ytm.authorization("secret", 1700000000), `SAPISIDHASH 1700000000_${sha1}`);
});

test("watch urls", () => {
  assert.equal(ytm.watchUrl({ videoId: "abc", playlistId: "PL1" }), "https://music.youtube.com/watch?v=abc&list=PL1");
  assert.equal(ytm.watchUrl({ videoId: "abc" }), "https://music.youtube.com/watch?v=abc");
  assert.equal(ytm.watchUrl({ playlistId: "PL1" }), "https://music.youtube.com/watch?list=PL1");
  assert.equal(ytm.watchUrl({ videoId: "abc", radio: true }), "https://music.youtube.com/watch?v=abc&list=RDAMVM" + "abc");
  assert.throws(() => ytm.watchUrl({}));
});

test("every reply echoes the request and reports failures", async () => {
  const signedOut = load({ cookies: { get: async () => null }, tabs: { query: async () => [] } });
  const reply = await signedOut.handle({ t: "ytm", req: 7, op: "current" });
  assert.deepEqual(JSON.parse(JSON.stringify(reply)), { t: "ytm", req: 7, op: "current", title: "", items: [], current: null, error: "Open music.youtube.com in this browser first" });
});

test("a signed-out library asks to sign in", async () => {
  const signedOut = load({ cookies: { get: async () => null } }, async () => ({ ok: true, json: async () => ({}) }));
  const reply = await signedOut.handle({ t: "ytm", req: 1, op: "browse", id: "FEmusic_liked_albums" });
  assert.match(reply.error, /^Sign in to music.youtube.com/);
});
