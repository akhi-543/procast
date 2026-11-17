// YouTube Music for procast: InnerTube calls with the browser's own signed-in session, and
// control of the music.youtube.com tab. Loaded by background.js (importScripts) and by tests.
//   {t: "ytm", req, op: "search" | "browse" | "like" | "dislike" | "unlike" | "addToPlaylist" | "play" | "control" | "current", ...}
//     -> {t: "ytm", req, op, title, items: [{kind, id, playlistId, title, subtitle, thumb, duration}], current, error?}

(() => {
  const ORIGIN = "https://music.youtube.com";
  const RULE_ID = 1;
  const NO_TAB = "Open music.youtube.com in this browser first";
  const NOT_READY = "YouTube Music isn't ready in the tab yet";
  const SIGN_IN = "Sign in to music.youtube.com in this browser to see your library";
  const CONTEXT = { client: { clientName: "WEB_REMIX", clientVersion: "1.20241111.01.00", hl: "en" } };
  const FILTERS = {
    songs: "EgWKAQIIAWoMEA4QChADEAQQCRAF",
    albums: "EgWKAQIYAWoMEA4QChADEAQQCRAF",
    artists: "EgWKAQIgAWoMEA4QChADEAQQCRAF",
    playlists: "EgWKAQIoAWoMEA4QChADEAQQCRAF",
  };
  const LIKES = { like: "like/like", dislike: "like/dislike", unlike: "like/removelike" };
  const CONTROLS = ["volume", "volumeUp", "volumeDown", "mute", "unmute", "shuffle", "repeat"];
  const RENDERERS = ["musicTwoRowItemRenderer", "musicResponsiveListItemRenderer"];
  const PAGE_KINDS = {
    MUSIC_PAGE_TYPE_ALBUM: "album",
    MUSIC_PAGE_TYPE_AUDIOBOOK: "album",
    MUSIC_PAGE_TYPE_ARTIST: "artist",
    MUSIC_PAGE_TYPE_USER_CHANNEL: "artist",
    MUSIC_PAGE_TYPE_PLAYLIST: "playlist",
  };
  const ID_KINDS = [["MPRE", "album"], ["UC", "artist"], ["VL", "playlist"]];

  // ---- InnerTube responses -> items --------------------------------------------------

  // Depth-first, document order. `visit` returning false skips that value's subtree.
  function walk(node, visit) {
    if (Array.isArray(node)) {
      node.forEach((n) => walk(n, visit));
    } else if (node && typeof node === "object") {
      for (const [k, v] of Object.entries(node)) if (visit(k, v) !== false) walk(v, visit);
    }
  }

  function collect(node, ...keys) {
    const out = [];
    walk(node, (k, v) => {
      if (keys.includes(k)) out.push(v);
    });
    return out;
  }

  const first = (node, ...keys) => collect(node, ...keys)[0];
  const runsOf = (text) => text?.runs ?? [];
  const joined = (runs) => runs.map((r) => r.text).join("");
  const lastThumb = (node) => first(node, "thumbnails")?.at(-1)?.url ?? "";
  const playlistOf = (node) => collect(node, "watchPlaylistEndpoint", "watchEndpoint").find((e) => e.playlistId)?.playlistId ?? "";

  function fields(r) {
    if (!r.flexColumns) return { titleRun: runsOf(r.title)[0], columns: [runsOf(r.subtitle)] };
    const columns = r.flexColumns.map((c) => runsOf(c.musicResponsiveListItemFlexColumnRenderer?.text));
    return { titleRun: columns[0]?.[0], columns: columns.slice(1) };
  }

  function duration(r, columns) {
    const fixed = joined(runsOf(r.fixedColumns?.[0]?.musicResponsiveListItemFixedColumnRenderer?.text));
    if (fixed) return fixed;
    return columns.flat().find((run) => /^\d+:\d{2}(:\d{2})?$/.test(run.text))?.text ?? "";
  }

  function kindOf(browse) {
    const type = browse.browseEndpointContextSupportedConfigs?.browseEndpointContextMusicConfig?.pageType;
    return PAGE_KINDS[type] ?? ID_KINDS.find(([prefix]) => browse.browseId.startsWith(prefix))?.[1];
  }

  function identity(r, titleRun) {
    const watch = titleRun?.navigationEndpoint?.watchEndpoint ?? r.navigationEndpoint?.watchEndpoint;
    const videoId = r.playlistItemData?.videoId ?? watch?.videoId;
    if (videoId) return { kind: "song", id: videoId, playlistId: watch?.playlistId ?? "" };
    const browse = r.navigationEndpoint?.browseEndpoint ?? titleRun?.navigationEndpoint?.browseEndpoint;
    const kind = browse?.browseId && kindOf(browse);
    if (!kind) return null;
    const id = browse.browseId;
    return { kind, id, playlistId: playlistOf(r) || (kind === "playlist" ? id.replace(/^VL/, "") : "") };
  }

  function item(r) {
    const { titleRun, columns } = fields(r);
    const title = titleRun?.text?.trim();
    const who = title && identity(r, titleRun);
    if (!who) return null;
    const subtitle = columns.map(joined).filter(Boolean).join(" · ").replaceAll(" • ", " · ").trim();
    return { ...who, title, subtitle, thumb: lastThumb(r.thumbnail) || lastThumb(r.thumbnailRenderer), duration: duration(r, columns) };
  }

  function items(json) {
    const seen = new Set();
    return collect(json, ...RENDERERS).flatMap((r) => {
      const it = item(r);
      const key = it && it.kind + it.id;
      if (!it || seen.has(key)) return [];
      seen.add(key);
      return [it];
    });
  }

  // The page's own header; the headers of the shelves below it don't count.
  function header(json) {
    let found;
    walk(json, (k, v) => {
      if (found || k.endsWith("ShelfRenderer")) return false;
      if (k.endsWith("HeaderRenderer") && runsOf(v?.title).length) found = v;
    });
    return found;
  }

  function page(json) {
    const head = header(json);
    const thumb = lastThumb(head);
    return {
      title: runsOf(head?.title)[0]?.text ?? "",
      thumb,
      playlistId: playlistOf(head),
      items: items(json).map((it) => (it.kind === "song" && !it.thumb ? { ...it, thumb } : it)),
    };
  }

  // ---- InnerTube requests ------------------------------------------------------------

  async function authorization(sapisid, ts) {
    const digest = await crypto.subtle.digest("SHA-1", new TextEncoder().encode(`${ts} ${sapisid} ${ORIGIN}`));
    const hex = Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
    return `SAPISIDHASH ${ts}_${hex}`;
  }

  async function sapisid() {
    for (const name of ["SAPISID", "__Secure-3PAPISID"]) {
      const cookie = await chrome.cookies.get({ url: ORIGIN, name });
      if (cookie) return cookie.value;
    }
    return "";
  }

  async function call(endpoint, body) {
    const headers = { "Content-Type": "application/json", "X-Origin": ORIGIN, "X-Goog-AuthUser": "0" };
    const sid = await sapisid();
    if (sid) headers.Authorization = await authorization(sid, Math.floor(Date.now() / 1000));
    const res = await fetch(`${ORIGIN}/youtubei/v1/${endpoint}?prettyPrint=false`, {
      method: "POST",
      headers,
      credentials: "include",
      body: JSON.stringify({ context: CONTEXT, ...body }),
    });
    if (!res.ok) throw new Error(`YouTube Music answered ${res.status}`);
    return res.json();
  }

  // fetch can't set Origin, and InnerTube wants it.
  async function setup() {
    try {
      await chrome.declarativeNetRequest?.updateSessionRules({
        removeRuleIds: [RULE_ID],
        addRules: [
          {
            id: RULE_ID,
            priority: 1,
            action: { type: "modifyHeaders", requestHeaders: [{ header: "Origin", operation: "set", value: ORIGIN }] },
            condition: { urlFilter: `|${ORIGIN}/youtubei/`, initiatorDomains: [chrome.runtime.id], resourceTypes: ["xmlhttprequest", "other"] },
          },
        ],
      });
    } catch (e) {
      console.warn("procast: couldn't set the Origin rule", e);
    }
  }

  // ---- the music.youtube.com tab -----------------------------------------------------

  async function findTab() {
    const tabs = await chrome.tabs.query({ url: `${ORIGIN}/*` });
    return tabs.find((t) => t.audible) ?? tabs[0];
  }

  async function requireTab() {
    const tab = await findTab();
    if (!tab) throw new Error(NO_TAB);
    return tab;
  }

  async function inTab(tab, func, ...args) {
    const [injected] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, world: "MAIN", func, args });
    return injected.result;
  }

  // Injected into the page: no access to anything outside the function.
  function readPlayer() {
    const player = document.getElementById("movie_player");
    if (!player?.getVideoData) return null;
    const data = player.getVideoData();
    return {
      videoId: data.video_id || "",
      title: data.title || "",
      artist: data.author || "",
      playlistId: data.list || new URL(location.href).searchParams.get("list") || "",
      volume: Math.round(player.getVolume()),
      muted: player.isMuted(),
    };
  }

  function applyControl(action, value) {
    const player = document.getElementById("movie_player");
    if (!player?.getVideoData) return false;
    const click = (selector) => document.querySelector(selector)?.click();
    const step = (sign) => Math.max(0, Math.min(100, player.getVolume() + sign * value));
    switch (action) {
      case "volume":
        player.setVolume(value);
        if (value > 0) player.unMute();
        break;
      case "volumeUp":
        player.setVolume(step(1));
        break;
      case "volumeDown":
        player.setVolume(step(-1));
        break;
      case "mute":
        player.mute();
        break;
      case "unmute":
        player.unMute();
        break;
      case "shuffle":
        click("ytmusic-player-bar .shuffle");
        break;
      case "repeat":
        click("ytmusic-player-bar .repeat");
        break;
    }
    return true;
  }

  async function readCurrent(tab) {
    const current = await inTab(tab, readPlayer);
    if (!current) throw new Error(NOT_READY);
    return current;
  }

  async function currentTrack() {
    return readCurrent(await requireTab());
  }

  async function videoIdOf(req) {
    if (req.videoId) return req.videoId;
    const current = await currentTrack();
    if (!current.videoId) throw new Error("Nothing is playing in YouTube Music");
    return current.videoId;
  }

  function watchUrl({ videoId, playlistId, radio }) {
    if (!videoId && !playlistId) throw new Error("Nothing to play");
    const params = new URLSearchParams();
    if (videoId) params.set("v", videoId);
    if (radio && videoId) params.set("list", `RDAMVM${videoId}`);
    else if (playlistId) params.set("list", playlistId);
    return `${ORIGIN}/watch?${params}`;
  }

  // ---- requests ----------------------------------------------------------------------

  async function search({ query, filter }) {
    if (!FILTERS[filter]) throw new Error(`Unknown search filter: ${filter}`);
    return { items: items(await call("search", { query, params: FILTERS[filter] })) };
  }

  async function browse({ id }) {
    const { title, items } = page(await call("browse", { browseId: id }));
    if (!items.length && !(await sapisid()) && (id.startsWith("FEmusic_") || id === "VLLM")) throw new Error(SIGN_IN);
    return { title, items };
  }

  async function addToPlaylist(req) {
    const edit = await call("browse/edit_playlist", {
      playlistId: req.playlistId,
      actions: [{ action: "ACTION_ADD_VIDEO", addedVideoId: await videoIdOf(req) }],
    });
    if (edit.status !== "STATUS_SUCCEEDED") throw new Error("Couldn't add to the playlist");
    return {};
  }

  async function play(req) {
    const videoId = req.videoId || (req.radio ? await videoIdOf(req) : "");
    const url = watchUrl({ videoId, playlistId: req.playlistId, radio: req.radio });
    const tab = await findTab();
    if (tab) await chrome.tabs.update(tab.id, { url });
    else await chrome.tabs.create({ url, active: false });
    return {};
  }

  async function control({ action, value }) {
    if (!CONTROLS.includes(action)) throw new Error(`Unknown control: ${action}`);
    if (action === "volume" && typeof value !== "number") throw new Error("Missing volume");
    const tab = await requireTab();
    if (!(await inTab(tab, applyControl, action, value))) throw new Error(NOT_READY);
    return { current: await readCurrent(tab) };
  }

  async function run(req) {
    switch (req.op) {
      case "search":
        return search(req);
      case "browse":
        return browse(req);
      case "like":
      case "dislike":
      case "unlike":
        await call(LIKES[req.op], { target: { videoId: await videoIdOf(req) } });
        return {};
      case "addToPlaylist":
        return addToPlaylist(req);
      case "play":
        return play(req);
      case "control":
        return control(req);
      case "current":
        return { current: await currentTrack() };
      default:
        throw new Error("unknown request");
    }
  }

  async function handle(req) {
    const reply = { t: "ytm", req: req.req, op: req.op, title: "", items: [], current: null };
    try {
      return { ...reply, ...(await run(req)) };
    } catch (e) {
      return { ...reply, error: e.message || String(e) };
    }
  }

  globalThis.ytm = { handle, setup, items, page, authorization, watchUrl };
})();
