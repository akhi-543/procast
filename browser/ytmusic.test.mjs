import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const here = (name) => new URL(name, import.meta.url);
const fixture = (name) => JSON.parse(readFileSync(here(`test-fixtures/${name}.json`), "utf8"));

function load(chrome = {}, fetch) {
  const sandbox = { crypto: webcrypto, TextEncoder, URL, URLSearchParams, console: { warn() {} }, chrome, fetch };
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
  const reply = await load().handle({ t: "ytm", req: 7, op: "nope" });
  assert.deepEqual(JSON.parse(JSON.stringify(reply)), { t: "ytm", req: 7, op: "nope", title: "", items: [], current: null, error: "unknown request" });
});

test("exportSession returns cookie names and values", async () => {
  const chrome = {
    cookies: {
      getAll: async ({ url }) => {
        assert.equal(url, "https://music.youtube.com");
        return [
          { name: "SAPISID", value: "one", domain: ".youtube.com" },
          { name: "PREF", value: "two" },
        ];
      },
    },
  };
  const reply = await load(chrome).handle({ t: "ytm", req: 9, op: "exportSession" });
  assert.equal(reply.error, undefined);
  assert.equal(reply.req, 9);
  assert.equal(reply.op, "exportSession");
  assert.equal(reply.cookies.length, 2);
  assert.equal(reply.cookies[0].name, "SAPISID");
  assert.equal(reply.cookies[1].name, "PREF");
  assert.ok(Object.keys(reply.cookies[0]).join(",") === "name,value");
});

test("a signed-out library asks to sign in", async () => {
  const signedOut = load({ cookies: { get: async () => null } }, async () => ({ ok: true, json: async () => ({}) }));
  const reply = await signedOut.handle({ t: "ytm", req: 1, op: "browse", id: "FEmusic_liked_albums" });
  assert.match(reply.error, /^Sign in to music.youtube.com/);
});

test("an owned playlist's wrapped header is found", () => {
  const thumbnail = { musicThumbnailRenderer: { thumbnail: { thumbnails: [{ url: "https://t/small" }, { url: "https://t/big" }] } } };
  const real = { title: { runs: [{ text: "Mine" }] }, thumbnail, buttons: [{ watchPlaylistEndpoint: { playlistId: "PL1" } }] };
  const page = ytm.page({ header: { musicEditablePlaylistDetailHeaderRenderer: { header: { musicResponsiveHeaderRenderer: real } } } });
  assert.deepEqual([page.title, page.thumb, page.playlistId], ["Mine", "https://t/big", "PL1"]);
});

test("a malformed renderer is skipped", () => {
  assert.equal(ytm.items({ musicResponsiveListItemRenderer: { flexColumns: [] } }).length, 0);
});

test("thumbnailRenderer is used when thumbnail has no thumbnails", () => {
  const run = { text: "Album", navigationEndpoint: { browseEndpoint: { browseId: "MPREb_x" } } };
  const thumbnailRenderer = { musicThumbnailRenderer: { thumbnail: { thumbnails: [{ url: "https://t/a" }] } } };
  const [it] = ytm.items({ musicTwoRowItemRenderer: { title: { runs: [run] }, thumbnail: {}, thumbnailRenderer } });
  assert.equal(it.thumb, "https://t/a");
});

function recording(cookies, respond) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url, headers: init.headers });
    return respond;
  };
  const chrome = { cookies: { get: async ({ name }) => (name in cookies ? { value: cookies[name] } : null) } };
  return { ytm: load(chrome, fetch), calls };
}

const ok = (body) => ({ ok: true, json: async () => body });

test("a non-2xx answer is reported with its status", async () => {
  const { ytm } = recording({}, { ok: false, status: 403 });
  assert.equal((await ytm.handle({ t: "ytm", req: 1, op: "search", query: "x", filter: "songs" })).error, "YouTube Music answered 403");
});

test("__Secure-3PAPISID is used when SAPISID is missing", async () => {
  const { ytm, calls } = recording({ "__Secure-3PAPISID": "fallback" }, ok({}));
  await ytm.handle({ t: "ytm", req: 1, op: "search", query: "x", filter: "songs" });
  assert.match(calls[0].headers.Authorization, /^SAPISIDHASH \d+_[0-9a-f]{40}$/);
});

test("no Authorization header when signed out", async () => {
  const { ytm, calls } = recording({}, ok({}));
  await ytm.handle({ t: "ytm", req: 1, op: "search", query: "x", filter: "songs" });
  assert.equal(calls.length, 1);
  assert.ok(!("Authorization" in calls[0].headers));
});

test("a failed playlist edit is an error", async () => {
  const { ytm } = recording({}, ok({ status: "STATUS_FAILED" }));
  const reply = await ytm.handle({ t: "ytm", req: 1, op: "addToPlaylist", playlistId: "PL1", videoId: "abc" });
  assert.equal(reply.error, "Couldn't add to the playlist");
  const done = recording({}, ok({ status: "STATUS_SUCCEEDED" }));
  assert.equal((await done.ytm.handle({ t: "ytm", req: 1, op: "addToPlaylist", playlistId: "PL1", videoId: "abc" })).error, undefined);
});

test("setup never throws", async () => {
  await load({}).setup();
  const rejecting = { runtime: { id: "x" }, declarativeNetRequest: { updateSessionRules: async () => Promise.reject(new Error("no")) } };
  await load(rejecting).setup();
});
