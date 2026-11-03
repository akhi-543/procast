// akscast's own implementation of the @raycast/api runtime, written against the MIT type
// definitions. Components are host elements; a react-reconciler turns the tree into JSON
// and posts it to the host. Every Raycast action kind becomes an `onAction` handler, so the
// UI side only ever needs "call handler X".
const { parentPort, workerData } = require("node:worker_threads");
const fs = require("node:fs");
const path = require("node:path");
const React = require("react");
const Reconciler = require("react-reconciler");
const { DefaultEventPriority } = require("react-reconciler/constants");

const env = workerData.environment;
const unsupported = new Set();

// ---- host messaging --------------------------------------------------------------------

const send = (msg) => parentPort.postMessage(msg);
const call = (method, params = {}) => send({ t: "call", method, params });
let nextRequest = 0;
const waiting = new Map();
function request(method, params = {}) {
  const id = ++nextRequest;
  send({ t: "request", id, method, params });
  return new Promise((resolve, reject) => waiting.set(id, { resolve, reject }));
}

// ---- elements --------------------------------------------------------------------------

// Element-valued props are rendered as named slot children so the reconciler sees them.
const SLOT_PROPS = ["detail", "actions", "metadata", "searchBarAccessory", "content"];
function host(type) {
  const C = ({ children, ...props }) => {
    const slots = [];
    for (const k of SLOT_PROPS) {
      if (React.isValidElement(props[k])) {
        slots.push(React.createElement("slot", { key: `slot:${k}`, name: k }, props[k]));
        delete props[k];
      }
    }
    return React.createElement(type, props, children, ...slots);
  };
  C.displayName = type;
  return C;
}
const el = (type, props, ...children) => React.createElement(type, props, ...children);

const Metadata = host("Metadata");
Object.assign(Metadata, {
  Label: host("Metadata.Label"),
  Link: host("Metadata.Link"),
  Separator: host("Metadata.Separator"),
  TagList: Object.assign(host("Metadata.TagList"), { Item: host("Metadata.TagList.Item") }),
});

const List = host("List");
List.Item = host("List.Item");
List.Item.Detail = host("List.Item.Detail");
List.Item.Detail.Metadata = Metadata;
List.Section = host("List.Section");
List.EmptyView = host("List.EmptyView");
List.Dropdown = Object.assign(host("List.Dropdown"), { Item: host("List.Dropdown.Item"), Section: host("List.Dropdown.Section") });

const Grid = host("Grid");
Object.assign(Grid, {
  Item: host("Grid.Item"),
  Section: host("Grid.Section"),
  EmptyView: host("List.EmptyView"),
  Dropdown: List.Dropdown,
  Inset: { Small: "small", Medium: "medium", Large: "large" },
  Fit: { Contain: "contain", Fill: "fill" },
  ItemSize: { Small: "small", Medium: "medium", Large: "large" },
});

const Detail = host("Detail");
Detail.Metadata = Metadata;

const Form = host("Form");
for (const k of ["TextField", "TextArea", "PasswordField", "Checkbox", "DatePicker", "Separator", "Description", "FilePicker", "LinkAccessory"])
  Form[k] = host(`Form.${k}`);
Form.Dropdown = Object.assign(host("Form.Dropdown"), { Item: host("Form.Dropdown.Item"), Section: host("Form.Dropdown.Section") });
Form.TagPicker = Object.assign(host("Form.TagPicker"), { Item: host("Form.TagPicker.Item") });
Form.DatePicker.Type = { Date: "date", DateTime: "dateTime" };

const ActionPanel = host("ActionPanel");
ActionPanel.Section = host("ActionPanel.Section");
ActionPanel.Submenu = host("ActionPanel.Submenu");

const MenuBarExtra = host("MenuBarExtra");
Object.assign(MenuBarExtra, { Item: host("MenuBarExtra.Item"), Section: host("MenuBarExtra.Section"), Submenu: host("MenuBarExtra.Submenu"), Separator: host("MenuBarExtra.Separator") });

// ---- actions ---------------------------------------------------------------------------

const Action = ({ title = "Action", ...props }) => el("Action", { title, ...props });
const builtin = (defaultTitle, run) => ({ title = defaultTitle, onAction, ...props }) =>
  el("Action", { ...props, title, onAction: (...args) => run(props, ...args) });
Action.CopyToClipboard = builtin("Copy to Clipboard", (p) => {
  call("copy", { content: text(p.content), close: true });
  p.onCopy?.(p.content);
});
Action.Paste = builtin("Paste", (p) => {
  call("paste", { content: text(p.content) });
  p.onPaste?.(p.content);
});
Action.OpenInBrowser = builtin("Open in Browser", (p) => {
  call("open", { target: p.url });
  p.onOpen?.(p.url);
});
Action.Open = builtin("Open", (p) => {
  call("open", { target: p.target });
  p.onOpen?.(p.target);
});
Action.OpenWith = builtin("Open With", (p) => call("open", { target: p.path }));
Action.ShowInFinder = builtin("Show in Explorer", (p) => {
  call("reveal", { path: p.path });
  p.onShow?.(p.path);
});
Action.Trash = builtin("Move to Recycle Bin", (p) => {
  call("trash", { paths: [].concat(p.paths) });
  p.onTrash?.(p.paths);
});
Action.SubmitForm = builtin("Submit", (p, values) => p.onSubmit?.(values ?? {}));
Action.Push = function Push({ target, title = "Open", onPush, onPop, ...props }) {
  const nav = React.useContext(NavContext);
  return el("Action", { ...props, title, onAction: () => (nav.push(target, onPop), onPush?.()) });
};
for (const k of ["CreateQuicklink", "CreateSnippet", "ToggleQuickLook", "PickDate"])
  Action[k] = builtin(k, () => toast({ style: "failure", title: `${k} is not supported in akscast` }));
Action.Style = { Regular: "regular", Destructive: "destructive" };

function text(content) {
  if (content == null) return "";
  if (typeof content === "object") return String(content.text ?? content.html ?? content.file ?? "");
  return String(content);
}

// ---- navigation ------------------------------------------------------------------------

const NavContext = React.createContext({ push() {}, pop() {} });
let navigation = null;
function NavRoot({ root }) {
  const [stack, setStack] = React.useState([{ element: root }]);
  const nav = React.useMemo(
    () => ({
      push: (element, onPop) => setStack((s) => [...s, { element, onPop }]),
      pop: () =>
        setStack((s) => {
          if (s.length < 2) return s;
          s[s.length - 1].onPop?.();
          return s.slice(0, -1);
        }),
    }),
    [],
  );
  navigation = nav;
  // Pages below the top stay mounted so their state survives a pop, like in Raycast.
  return el(NavContext.Provider, { value: nav }, ...stack.map((page, i) => el("page", { key: i }, page.element)));
}
const useNavigation = () => React.useContext(NavContext);

// ---- feedback --------------------------------------------------------------------------

function toast(options) {
  const state = { style: "success", title: "", message: undefined, ...options };
  const sendToast = () => call("toast", { style: state.style, title: state.title, message: state.message });
  sendToast();
  // Setting style/title/message on a shown toast updates it, as in Raycast.
  return new Proxy(state, {
    get: (t, k) => (k === "show" ? async () => sendToast() : k === "hide" ? async () => call("toast", { hide: true }) : t[k]),
    set: (t, k, v) => ((t[k] = v), sendToast(), true),
  });
}
const Toast = { Style: { Success: "success", Failure: "failure", Animated: "animated" } };
async function showToast(options, title, message) {
  return typeof options === "string" ? toast({ style: options, title, message }) : toast(options);
}
const showHUD = async (title) => call("hud", { title: String(title) });

const Alert = { ActionStyle: { Default: "default", Cancel: "cancel", Destructive: "destructive" } };
async function confirmAlert(options) {
  const ok = await request("confirm", {
    title: options.title,
    message: options.message,
    primary: options.primaryAction?.title ?? "Confirm",
    destructive: options.primaryAction?.style === "destructive",
  });
  (ok ? options.primaryAction : options.dismissAction)?.onAction?.();
  return ok;
}

// ---- storage ---------------------------------------------------------------------------

function jsonFile(file) {
  let data = {};
  try {
    data = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {}
  let queued = false;
  const save = () => {
    if (queued) return;
    queued = true;
    setImmediate(() => {
      queued = false;
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, JSON.stringify(data));
    });
  };
  return { data, save };
}

const local = jsonFile(path.join(env.supportPath, ".akscast-localstorage.json"));
const LocalStorage = {
  getItem: async (k) => local.data[k],
  setItem: async (k, v) => void ((local.data[k] = v), local.save()),
  removeItem: async (k) => void (delete local.data[k], local.save()),
  allItems: async () => ({ ...local.data }),
  clear: async () => void (Object.keys(local.data).forEach((k) => delete local.data[k]), local.save()),
};

// One store per namespace, shared by every Cache instance, persisted to disk.
const caches = new Map();
class Cache {
  constructor(options = {}) {
    const ns = options.namespace ?? "default";
    if (!caches.has(ns)) caches.set(ns, { ...jsonFile(path.join(env.supportPath, ".akscast-cache", `${ns.replace(/[^\w.-]/g, "_")}.json`)), subs: new Set() });
    const store = caches.get(ns);
    // Bound methods: @raycast/utils hands cache.subscribe straight to useSyncExternalStore.
    this.get = (k) => store.data[k];
    this.has = (k) => k in store.data;
    this.set = (k, v) => {
      store.data[k] = v;
      store.save();
      store.subs.forEach((s) => s(k, v));
    };
    this.remove = (k) => {
      const had = k in store.data;
      delete store.data[k];
      store.save();
      store.subs.forEach((s) => s(k, undefined));
      return had;
    };
    this.clear = (o) => {
      Object.keys(store.data).forEach((k) => delete store.data[k]);
      store.save();
      if (o?.notifySubscribers !== false) store.subs.forEach((s) => s(undefined, undefined));
    };
    this.subscribe = (s) => (store.subs.add(s), () => store.subs.delete(s));
    Object.defineProperty(this, "isEmpty", { get: () => Object.keys(store.data).length === 0 });
  }
}

// ---- misc API --------------------------------------------------------------------------

// Raycast icon/colour constants are opaque strings; the UI maps the names it knows.
const named = (prefix) => new Proxy({}, { get: (_, k) => (typeof k === "string" ? `${prefix}:${k}` : undefined) });
const Icon = named("icon");
const Color = named("color");
const Image = { Mask: { Circle: "circle", RoundedRectangle: "roundedRectangle" } };
const Keyboard = {
  Shortcut: { Common: new Proxy({}, { get: (_, k) => ({ modifiers: [], key: String(k), common: String(k) }) }) },
};
const LaunchType = { UserInitiated: "userInitiated", Background: "background" };
const PopToRootType = { Default: "default", Immediate: "immediate", Suspended: "suspended" };
const notSupported = (what) => async () => {
  throw new Error(`${what} is not supported in akscast yet`);
};

const api = {
  List, Grid, Detail, Form, ActionPanel, Action, MenuBarExtra,
  Icon, Color, Image, Keyboard, Toast, Alert, Cache, LocalStorage, LaunchType, PopToRootType,
  environment: { ...env, launchType: workerData.launchType, canAccess: () => false },
  getPreferenceValues: () => workerData.preferences,
  useNavigation,
  showToast, showHUD, confirmAlert,
  open: async (target) => call("open", { target: String(target) }),
  showInFinder: async (p) => call("reveal", { path: String(p) }),
  trash: async (paths) => call("trash", { paths: [].concat(paths) }),
  closeMainWindow: async () => call("closeMainWindow"),
  popToRoot: async () => call("popToRoot"),
  clearSearchBar: async () => call("clearSearchBar"),
  launchCommand: async (o) => call("launchCommand", { name: o.name, type: o.type, arguments: o.arguments ?? {}, context: o.context }),
  updateCommandMetadata: async () => {},
  openExtensionPreferences: async () => call("openPreferences"),
  openCommandPreferences: async () => call("openPreferences"),
  getApplications: async () => request("getApplications"),
  getFrontmostApplication: async () => request("getFrontmostApplication"),
  getDefaultApplication: notSupported("getDefaultApplication"),
  getSelectedText: notSupported("getSelectedText"),
  getSelectedFinderItems: async () => [],
  captureException: (e) => send({ t: "log", text: String(e?.stack ?? e) }),
  Clipboard: {
    copy: async (content) => call("copy", { content: text(content), close: false }),
    paste: async (content) => call("paste", { content: text(content) }),
    clear: async () => call("copy", { content: "", close: false }),
    readText: async () => request("readClipboard"),
    read: async () => ({ text: await request("readClipboard") }),
  },
  AI: { ask: notSupported("AI"), Model: named("model"), Creativity: { None: 0, Low: 0.25, Medium: 0.5, High: 0.75, Maximum: 1 } },
  OAuth: { PKCEClient: class { constructor() { throw new Error("OAuth is not supported in akscast yet"); } }, RedirectMethod: { Web: "web", App: "app", AppURI: "appURI" } },
  BrowserExtension: { getTabs: notSupported("BrowserExtension"), getContent: notSupported("BrowserExtension") },
  Tool: {},
};

// Anything an extension reads that we don't have yet is recorded, not crashed on.
const exported = new Proxy(api, {
  get(target, key) {
    if (key in target || typeof key === "symbol" || key === "__esModule" || key === "then" || key === "default") return target[key];
    unsupported.add(key);
    return host(`Unsupported.${key}`);
  },
});

// ---- renderer --------------------------------------------------------------------------

const handlers = new Map();

// Handler ids come from the node's position, so an id from a slightly older render still
// reaches the equivalent handler after a re-render.
function serialize(node, at) {
  if (node.text !== undefined) return node.text;
  const props = {};
  for (const [k, v] of Object.entries(node.props)) {
    if (k === "children") continue;
    if (typeof v === "function") {
      const id = `${at}:${k}`;
      handlers.set(id, v);
      props[k] = { $fn: id };
    } else if (!React.isValidElement(v)) props[k] = v;
  }
  const out = { type: node.type, props };
  const kids = node.children.filter((c) => !c.hidden);
  if (kids.length) out.children = kids.map((c, i) => serialize(c, `${at}.${i}`));
  return out;
}

const root = { children: [] };
let dirty = false;
function flush() {
  if (dirty) return;
  dirty = true;
  setImmediate(() => {
    dirty = false;
    handlers.clear();
    // The context provider isn't a host node, so the pages sit directly in the container.
    const pages = root.children.filter((c) => c.type === "page");
    const top = pages[pages.length - 1];
    const tree = top ? top.children.map((c, i) => serialize(c, `p${pages.length}.${i}`)) : [];
    send({ t: "render", depth: pages.length, tree, unsupported: [...unsupported] });
  });
}

const insert = (parent, child, before) => {
  const list = parent.children;
  const i = list.indexOf(child);
  if (i >= 0) list.splice(i, 1);
  list.splice(before ? list.indexOf(before) : list.length, 0, child);
};
const remove = (parent, child) => parent.children.splice(parent.children.indexOf(child), 1);

let priority = DefaultEventPriority;
const renderer = Reconciler({
  supportsMutation: true, supportsPersistence: false, supportsHydration: false, isPrimaryRenderer: true,
  noTimeout: -1, scheduleTimeout: setTimeout, cancelTimeout: clearTimeout,
  getRootHostContext: () => ({}), getChildHostContext: (c) => c,
  createInstance: (type, props) => ({ type, props, children: [] }),
  createTextInstance: (t) => ({ text: t }),
  appendInitialChild: (p, c) => p.children.push(c),
  appendChild: (p, c) => insert(p, c), appendChildToContainer: (r, c) => insert(r, c),
  insertBefore: insert, insertInContainerBefore: insert,
  removeChild: remove, removeChildFromContainer: remove,
  commitUpdate: (inst, _type, _old, next) => { inst.props = next; },
  commitTextUpdate: (inst, _old, t) => { inst.text = t; },
  finalizeInitialChildren: () => false, shouldSetTextContent: () => false,
  prepareForCommit: () => null, resetAfterCommit: flush,
  clearContainer: (r) => { r.children.length = 0; },
  getPublicInstance: (i) => i, preparePortalMount: () => {}, detachDeletedInstance: () => {},
  hideInstance: (i) => { i.hidden = true; }, unhideInstance: (i) => { i.hidden = false; },
  hideTextInstance: () => {}, unhideTextInstance: () => {},
  getCurrentUpdatePriority: () => priority, setCurrentUpdatePriority: (p) => { priority = p; },
  resolveUpdatePriority: () => priority || DefaultEventPriority,
  shouldAttemptEagerTransition: () => false, trackSchedulerEvent: () => {},
  resolveEventType: () => null, resolveEventTimeStamp: () => -1.1,
  requestPostPaintCallback: () => {}, maySuspendCommit: () => false, preloadInstance: () => true,
  startSuspendingCommit: () => {}, suspendInstance: () => {}, waitForCommitToBeReady: () => null,
  NotPendingTransition: null, HostTransitionContext: React.createContext(null),
  resetFormInstance: () => {}, getInstanceFromNode: () => null, getInstanceFromScope: () => null,
  beforeActiveInstanceBlur: () => {}, afterActiveInstanceBlur: () => {}, prepareScopeUpdate: () => {},
});

const fail = (e) => send({ t: "error", message: String(e?.stack ?? e) });
const container = renderer.createContainer(root, 1, null, false, null, "", fail, fail, fail, () => {}, null);

function mount(Command, props) {
  renderer.updateContainer(el(NavRoot, { root: el(Command, props) }), container, null, null);
}

parentPort.on("message", (msg) => {
  if (msg.t === "event") {
    const fn = handlers.get(msg.fn);
    if (!fn) return;
    try {
      const r = fn(...(msg.args ?? []));
      if (r && typeof r.catch === "function") r.catch(fail);
    } catch (e) {
      fail(e);
    }
  } else if (msg.t === "pop") navigation?.pop();
  else if (msg.t === "reply") {
    const w = waiting.get(msg.id);
    waiting.delete(msg.id);
    if (!w) return;
    msg.error ? w.reject(new Error(msg.error)) : w.resolve(msg.value);
  }
});

module.exports = exported;
module.exports.__mount = mount;
module.exports.__fail = fail;
