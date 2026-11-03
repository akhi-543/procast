#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod commands;
mod config;
mod search;
mod win;

use std::time::{Duration, Instant};

use futures::{
    StreamExt,
    channel::mpsc::{self, UnboundedSender},
};
use gpui::{
    App, Bounds, ClipboardItem, Context, Entity, Focusable, KeyBinding, QuitMode, Subscription, Window, WindowBounds,
    WindowKind, WindowOptions, actions, colors::Colors, div, img, prelude::*, px, rgb, rgb_to_hsla, size,
};
use gpui_ce_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged,
    actions::{DEFAULT_INPUT_CONTEXT, default_bindings},
    text_input,
};
use nucleo_matcher::Matcher;
use tray_icon::{
    Icon as TrayImage, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use windows::Win32::Foundation::HWND;

use commands::Cmd;
use config::Config;
use search::{Act, Frecency, Icon, Item};
use win::{Bind, Msg, TrayCmd};

actions!(akscast, [SelectPrev, SelectNext, Dismiss, Launch]);

const BAR: f32 = 56.;
const ROW: f32 = 44.;
const FOOTER: f32 = 34.;

fn window_height(cfg: &Config) -> f32 {
    BAR + ROW * cfg.max_results.max(1) as f32 + 12. + FOOTER
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Root,
    Clipboard,
    Emoji,
}

impl Mode {
    fn of(bind: Bind) -> Self {
        match bind {
            Bind::Launcher => Mode::Root,
            Bind::Clipboard => Mode::Clipboard,
            Bind::Emoji => Mode::Emoji,
        }
    }

    fn label(self) -> Option<&'static str> {
        match self {
            Mode::Root => None,
            Mode::Clipboard => Some("Clipboard History"),
            Mode::Emoji => Some("Emoji"),
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Mode::Root => "Search apps and commands, calculate, > to run",
            Mode::Clipboard => "Search clipboard history",
            Mode::Emoji => "Search emoji",
        }
    }
}

fn hotkey_specs(cfg: &Config) -> [String; 3] {
    [cfg.hotkey.clone(), cfg.clipboard.hotkey.clone(), cfg.emoji.hotkey.clone()]
}

fn action_label(act: &Act) -> &'static str {
    match act {
        Act::App(_) | Act::Open(_) => "Open",
        Act::Run(_) => "Run",
        Act::Copy(_) => "Copy",
        Act::Paste(_) | Act::Snippet(_) => "Paste",
        Act::Cmd(Cmd::ClipboardHistory | Cmd::Emoji) => "Open",
        Act::Cmd(_) => "Run Command",
        Act::Fill(_) => "Type Query",
    }
}

struct Launcher {
    input: Entity<EditableTextState>,
    cfg: Config,
    hotkeys: win::Hotkeys,
    tx: UnboundedSender<Msg>,
    apps: Vec<win::App>,
    catalog: Vec<Item>,
    scanned: Option<Instant>,
    frec: Frecency,
    history: clipboard::History,
    matcher: Matcher,
    mode: Mode,
    items: Vec<Item>,
    selected: usize,
    /// Destructive command waiting for a second Enter.
    armed: Option<usize>,
    /// Config problem, shown above the results.
    notice: Option<String>,
    hotkey_errors: [Option<String>; 3],
    hwnd: HWND,
    prev: HWND,
    visible: bool,
    _subs: Vec<Subscription>,
}

impl Launcher {
    fn new(cfg: Config, notice: Option<String>, tx: UnboundedSender<Msg>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        let subs = vec![
            cx.subscribe(&input, |this, _, _: &TextChanged, cx| this.refresh(cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                if this.visible && !window.is_window_active() {
                    this.hide(false, cx);
                }
            }),
        ];
        // ponytail: ignore_apps is read once at start; changing it needs a restart.
        clipboard::watch(cfg.clipboard.ignore_apps.clone(), tx.clone());
        Self {
            input,
            hotkeys: win::Hotkeys::spawn(hotkey_specs(&cfg), tx.clone()),
            history: clipboard::History::load(config::dir().join("clipboard.bin"), cfg.clipboard.history),
            catalog: search::catalog(&[], &cfg),
            cfg,
            tx,
            apps: vec![],
            scanned: None,
            frec: Frecency::load(config::dir().join("frecency.json")),
            matcher: Matcher::new(nucleo_matcher::Config::DEFAULT),
            mode: Mode::Root,
            items: vec![],
            selected: 0,
            armed: None,
            notice,
            hotkey_errors: Default::default(),
            hwnd: HWND::default(),
            prev: HWND::default(),
            visible: false,
            _subs: subs,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).as_str().to_string();
        let max = self.cfg.max_results.max(1);
        self.items = match self.mode {
            Mode::Root => search::root(&query, &self.catalog, &self.cfg, &self.frec, &mut self.matcher),
            Mode::Clipboard => search::clipboard(&query, &self.history.entries, max, &mut self.matcher),
            Mode::Emoji => search::emoji(&query, max, &mut self.matcher),
        };
        self.selected = 0;
        self.armed = None;
        cx.notify();
    }

    fn rebuild_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog = search::catalog(&self.apps, &self.cfg);
        self.refresh(cx);
    }

    /// Off-thread: names arrive first (~2 s), icons follow.
    // ponytail: no disk cache, so the app list is empty for the first seconds after start.
    fn scan_apps(&mut self, force: bool) {
        if !force && self.scanned.is_some_and(|t| t.elapsed() < Duration::from_secs(300)) {
            return;
        }
        self.scanned = Some(Instant::now());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut apps = win::scan_apps();
            let _ = tx.unbounded_send(Msg::Apps(apps.clone()));
            win::load_icons(&mut apps);
            let _ = tx.unbounded_send(Msg::Apps(apps));
        });
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.input.update(cx, |s, cx| s.emplace("", cx));
        self.refresh(cx);
    }

    fn show(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let fg = win::foreground();
        if fg != self.hwnd {
            self.prev = fg;
        }
        self.set_mode(mode, cx);
        win::cloak(self.hwnd, false);
        win::focus(self.hwnd);
        window.activate();
        window.focus(&self.input.focus_handle(cx), cx);
        self.visible = true;
        self.scan_apps(false);
    }

    /// `restore` hands focus back to the window that was active before we opened;
    /// false only when focus already moved elsewhere (deactivation).
    fn hide(&mut self, restore: bool, cx: &mut Context<Self>) {
        win::cloak(self.hwnd, true);
        self.visible = false;
        if restore && !self.prev.is_invalid() {
            win::focus(self.prev);
        }
        cx.notify();
    }

    fn paste(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.hide(true, cx);
        clipboard::paste_soon();
    }

    fn launch(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.selected).cloned() else { return };
        if let Act::Cmd(cmd) = item.act
            && cmd.destructive()
            && self.armed != Some(self.selected)
        {
            self.armed = Some(self.selected);
            return cx.notify();
        }
        if let Some(key) = &item.key {
            self.frec.record(key, search::now());
        }
        match item.act {
            Act::App(id) => win::launch_app(&id),
            Act::Open(target) => win::open(&target),
            Act::Run(cmdline) => win::run(&cmdline),
            Act::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Act::Paste(text) => return self.paste(text, cx),
            Act::Snippet(template) => {
                let clip = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                return self.paste(search::expand(&template, &clip, win::local_date_time()), cx);
            }
            Act::Fill(text) => return self.input.update(cx, |s, cx| s.emplace(&text, cx)),
            Act::Cmd(Cmd::ClipboardHistory) => return self.set_mode(Mode::Clipboard, cx),
            Act::Cmd(Cmd::Emoji) => return self.set_mode(Mode::Emoji, cx),
            Act::Cmd(Cmd::ClearClipboardHistory) => self.history.clear(),
            Act::Cmd(Cmd::EditConfig) => win::open(&config::path().to_string_lossy()),
            Act::Cmd(Cmd::ReloadApps) => self.scan_apps(true),
            Act::Cmd(Cmd::Quit) => return cx.quit(),
            Act::Cmd(cmd) => {
                // Hide first so window commands act on the window you came from.
                self.hide(true, cx);
                let target = self.prev.0 as isize;
                std::thread::spawn(move || commands::run(cmd, HWND(target as _)));
                return;
            }
        }
        // Hand focus back even when launching: a cloaked launcher that stays foreground
        // would swallow keys typed before the new app's window appears.
        self.hide(true, cx);
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        match config::load() {
            Ok(cfg) => {
                if hotkey_specs(&cfg) != hotkey_specs(&self.cfg) {
                    self.hotkeys.set(hotkey_specs(&cfg));
                }
                if cfg.start_at_login != self.cfg.start_at_login {
                    win::set_start_at_login(cfg.start_at_login);
                }
                self.cfg = cfg;
                self.notice = None;
            }
            Err(e) => self.notice = Some(e),
        }
        self.rebuild_catalog(cx);
    }

    fn on_msg(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            Msg::Hotkey(bind) if self.visible && self.mode == Mode::of(bind) => self.hide(true, cx),
            Msg::Hotkey(bind) => self.show(Mode::of(bind), window, cx),
            Msg::Tray(TrayCmd::Open) => self.show(Mode::Root, window, cx),
            Msg::Tray(TrayCmd::EditConfig) => win::open(&config::path().to_string_lossy()),
            Msg::Tray(TrayCmd::Quit) => cx.quit(),
            Msg::Apps(apps) => {
                self.apps = apps;
                self.rebuild_catalog(cx);
            }
            Msg::Clip(text) => {
                if self.cfg.clipboard.enabled {
                    self.history.push(text, search::now());
                    if self.visible && self.mode == Mode::Clipboard {
                        self.refresh(cx);
                    }
                }
            }
            Msg::HotkeyStatus(bind, status) => {
                self.hotkey_errors[bind as usize - 1] = status.err();
                // Without a working main hotkey the user can't reach us, so open and say why.
                if bind == Bind::Launcher && self.hotkey_errors[0].is_some() {
                    self.show(Mode::Root, window, cx);
                }
                cx.notify();
            }
        }
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Colors::for_appearance(window);
        let selected = self.selected;
        let notices: Vec<String> = self.notice.iter().chain(self.hotkey_errors.iter().flatten()).cloned().collect();
        let footer_action = match self.items.get(selected) {
            Some(it) if self.armed == Some(selected) => format!("Press ↵ again to {}", it.title),
            Some(it) => format!("{}  ↵", action_label(&it.act)),
            None => String::new(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors.background)
            .text_color(colors.text)
            .border_1()
            .border_color(colors.border)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| {
                this.selected = (this.selected + 1).min(this.items.len().saturating_sub(1));
                this.armed = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| {
                this.selected = this.selected.saturating_sub(1);
                this.armed = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                if this.mode == Mode::Root {
                    this.hide(true, cx)
                } else {
                    this.set_mode(Mode::Root, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Launch, _, cx| this.launch(cx)))
            .child(
                div()
                    .h(px(BAR))
                    .flex_none()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(colors.separator)
                    .child(
                        text_input("query")
                            .state(self.input.downgrade())
                            .placeholder(self.mode.placeholder())
                            .placeholder_color(colors.disabled)
                            .caret_color(rgb_to_hsla(colors.text))
                            .caret_blink_interval_500ms()
                            .flex_1()
                            .text_lg()
                            .whitespace_nowrap(),
                    )
                    .children(self.mode.label().map(|l| {
                        div().flex_none().px_2().py_0p5().rounded_md().text_xs().bg(colors.container).text_color(colors.disabled).child(l)
                    })),
            )
            .children(notices.into_iter().map(|n| div().px_4().py_1().text_sm().text_color(rgb(0xff6b6b)).child(n)))
            .child(div().flex_1().flex().flex_col().p_1p5().children(self.items.iter().enumerate().map(|(i, item)| {
                let on = i == selected;
                let icon = match &item.icon {
                    Icon::Image(image) => img(image.clone()).size(px(24.)).into_any_element(),
                    Icon::Glyph(g) => div().size(px(24.)).flex().items_center().justify_center().child(*g).into_any_element(),
                };
                div()
                    .id(i)
                    .h(px(ROW))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded_md()
                    .when(on, |d| d.bg(colors.selected).text_color(colors.selected_text))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = i;
                        this.launch(cx);
                    }))
                    .child(div().flex_none().child(icon))
                    .child(div().flex_1().truncate().child(item.title.clone()))
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .text_color(if on { colors.selected_text } else { colors.disabled })
                            .child(item.subtitle.clone()),
                    )
            })))
            .child(
                div()
                    .h(px(FOOTER))
                    .flex_none()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(colors.separator)
                    .text_xs()
                    .text_color(colors.disabled)
                    .child(self.mode.label().unwrap_or("akscast"))
                    .child(footer_action),
            )
    }
}

/// A plain accent-coloured disc; no third-party artwork.
fn tray_icon_image() -> TrayImage {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            if dx * dx + dy * dy <= 15.0 * 15.0 {
                rgba[(y * N + x) * 4..][..4].copy_from_slice(&[0x5b, 0x8c, 0xff, 0xff]);
            }
        }
    }
    TrayImage::from_rgba(rgba, N as u32, N as u32).expect("valid icon")
}

fn tray(tx: UnboundedSender<Msg>) -> Option<TrayIcon> {
    let open = MenuItem::new("Open akscast", true, None);
    let edit = MenuItem::new("Edit config.toml", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &edit, &quit]).ok()?;
    let ids = (open.id().clone(), edit.id().clone(), quit.id().clone());
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let cmd = match &e.id {
            id if *id == ids.0 => TrayCmd::Open,
            id if *id == ids.1 => TrayCmd::EditConfig,
            id if *id == ids.2 => TrayCmd::Quit,
            _ => return,
        };
        let _ = menu_tx.unbounded_send(Msg::Tray(cmd));
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            let _ = tx.unbounded_send(Msg::Tray(TrayCmd::Open));
        }
    }));
    TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("akscast").with_icon(tray_icon_image()).build().ok()
}

fn main() {
    // ponytail: a second launch just exits; forward "show" to the running instance later.
    if !win::single_instance() {
        return;
    }
    gpui_ce_platform::application().with_quit_mode(QuitMode::Explicit).run(|cx: &mut App| {
        // Defaults first; ours are added later at the same depth, so they win for these keys.
        cx.bind_keys(default_bindings().as_keybindings(Some(DEFAULT_INPUT_CONTEXT)));
        cx.bind_keys([
            KeyBinding::new("up", SelectPrev, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("down", SelectNext, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("escape", Dismiss, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("enter", Launch, Some(DEFAULT_INPUT_CONTEXT)),
        ]);

        let (cfg, notice) = match config::load() {
            Ok(cfg) => (cfg, None),
            Err(e) => (Config::default(), Some(e)),
        };
        win::set_start_at_login(cfg.start_at_login);
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let tray = tray(tx.clone());

        let bounds = Bounds::centered(None, size(px(cfg.width), px(window_height(&cfg))), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            show: false,
            focus: false,
            ..Default::default()
        };
        let handle = cx.open_window(options, |window, cx| cx.new(|cx| Launcher::new(cfg, notice, tx, window, cx))).unwrap();

        // Shown once, cloaked: from now on hotkey -> un-cloak -> next frame.
        let _ = handle.update(cx, |this, window, _| {
            this.hwnd = win::hwnd(window);
            win::make_tool_window(this.hwnd);
            win::cloak(this.hwnd, true);
            window.set_visible(true);
            this.scan_apps(true);
        });

        cx.spawn(async move |cx| {
            let _tray = tray;
            while let Some(msg) = rx.next().await {
                // SetWindowPos sends WM_SIZE/WM_DPICHANGED synchronously and GPUI's window
                // proc re-borrows the app, so the window is moved here, between updates.
                let opens = matches!(msg, Msg::Hotkey(_) | Msg::Tray(TrayCmd::Open) | Msg::HotkeyStatus(Bind::Launcher, Err(_)));
                let geometry = handle.update(cx, |this, _, _| {
                    (!this.visible).then(|| (this.hwnd, this.cfg.monitor.clone(), this.cfg.width, window_height(&this.cfg)))
                });
                if opens && let Ok(Some((hwnd, monitor, width, height))) = geometry {
                    win::place(hwnd, &monitor, width, height);
                }
                let _ = handle.update(cx, |this, window, cx| this.on_msg(msg, window, cx));
            }
        })
        .detach();

        // ponytail: 2 s mtime poll instead of a file watcher; good enough for a hand-edited file.
        cx.spawn(async move |cx| {
            let mut last = config::modified();
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let now = config::modified();
                if now != last {
                    last = now;
                    let _ = handle.update(cx, |this, _, cx| this.reload(cx));
                }
            }
        })
        .detach();
    });
}
