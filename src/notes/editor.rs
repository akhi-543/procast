//! Markdown editor core: a text buffer with cursor, selection and undo, plus the markdown
//! operations (marks, blocks, tasks, list continuation) and `spans` for live styling.
//! Pure logic; the notes window renders it. Offsets are bytes on char boundaries.

use std::{collections::HashMap, ops::Range, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Highlight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block {
    /// No block: `set_block` with it strips any block prefix (`#` to `######`, quote, list, task).
    Paragraph,
    H1,
    H2,
    H3,
    Quote,
    Bullet,
    Numbered,
    Task,
    Code,
}

impl Mark {
    fn pair(self) -> (&'static str, &'static str) {
        match self {
            Mark::Bold => ("**", "**"),
            Mark::Italic => ("*", "*"),
            Mark::Underline => ("<u>", "</u>"),
            Mark::Strike => ("~~", "~~"),
            Mark::Code => ("`", "`"),
            Mark::Highlight => ("==", "=="),
        }
    }
}

/// Undo steps kept; each is a full copy of the note, which is small.
const UNDO_MAX: usize = 200;

#[derive(Clone)]
struct Snap {
    text: String,
    cursor: usize,
    anchor: usize,
}

pub struct Buffer {
    text: String,
    cursor: usize,
    anchor: usize,
    /// Column (in chars) that up/down aim for, until any other move.
    want_col: Option<usize>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    /// Where the current run of typed word characters ends; the next one joins the same undo step.
    run: Option<usize>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

fn line_start(t: &str, pos: usize) -> usize {
    t[..pos].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(t: &str, pos: usize) -> usize {
    t[pos..].find('\n').map_or(t.len(), |i| pos + i)
}

fn prev_char(t: &str, pos: usize) -> usize {
    t[..pos].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next_char(t: &str, pos: usize) -> usize {
    pos + t[pos..].chars().next().map_or(0, char::len_utf8)
}

/// Where `pos` lands after `start..end` is replaced by `ins` bytes.
fn shift(pos: usize, start: usize, end: usize, ins: usize) -> usize {
    if pos >= end {
        pos - (end - start) + ins
    } else if pos > start {
        start + ins
    } else {
        pos
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// A block prefix after the indent: `(indent, prefix length, block)`.
fn parse_prefix(line: &str) -> Option<(usize, usize, Block)> {
    let ind = indent_of(line);
    let s = &line[ind..];
    let (len, block) = if ["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "* [X] "].iter().any(|p| s.starts_with(p)) {
        (6, Block::Task)
    } else if s.starts_with("- ") || s.starts_with("* ") {
        (2, Block::Bullet)
    } else if s.starts_with("### ") {
        (4, Block::H3)
    } else if (4..=6).any(|n| s.starts_with(&format!("{} ", "#".repeat(n)))) {
        // h4 to h6 have no block of their own: only `Paragraph` removes them
        (s.find(' ').unwrap_or(0) + 1, Block::Paragraph)
    } else if s.starts_with("## ") {
        (3, Block::H2)
    } else if s.starts_with("# ") {
        (2, Block::H1)
    } else if s.starts_with("> ") {
        (2, Block::Quote)
    } else {
        let d = s.bytes().take_while(u8::is_ascii_digit).count();
        if d > 0 && s[d..].starts_with(". ") {
            (d + 2, Block::Numbered)
        } else {
            return None;
        }
    };
    Some((ind, len, block))
}

fn block_prefix(block: Block, n: usize) -> String {
    match block {
        Block::H1 => "# ".into(),
        Block::H2 => "## ".into(),
        Block::H3 => "### ".into(),
        Block::Quote => "> ".into(),
        Block::Bullet => "- ".into(),
        Block::Numbered => format!("{n}. "),
        Block::Task => "- [ ] ".into(),
        Block::Paragraph | Block::Code => String::new(),
    }
}

/// What a new item after `prefix` (indent included) starts with; `None` for non-list blocks.
fn continuation(prefix: &str, block: Block) -> Option<String> {
    match block {
        Block::Bullet => Some(prefix.to_owned()),
        Block::Task => Some(prefix.replace("[x]", "[ ]").replace("[X]", "[ ]")),
        Block::Numbered => {
            let ind = indent_of(prefix);
            let d = prefix[ind..].bytes().take_while(u8::is_ascii_digit).count();
            let n: usize = prefix[ind..ind + d].parse().ok()?;
            Some(format!("{}{}{}", &prefix[..ind], n + 1, &prefix[ind + d..]))
        }
        _ => None,
    }
}

pub fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// `before` ends with the opener and `after` starts with the closer. Italic's single star must
/// not be one of a bold's two: an odd run of stars around it means italic is on.
fn wrapped(m: Mark, before: &str, after: &str) -> bool {
    let (o, c) = m.pair();
    if !(before.ends_with(o) && after.starts_with(c)) {
        return false;
    }
    m != Mark::Italic || {
        let r1 = before.bytes().rev().take_while(|&b| b == b'*').count();
        let r2 = after.bytes().take_while(|&b| b == b'*').count();
        r1.min(r2) % 2 == 1
    }
}

/// The selected text itself starts and ends with the markers.
fn wrapped_in(m: Mark, sel: &str) -> bool {
    let (o, c) = m.pair();
    if sel.len() < o.len() + c.len() || !sel.starts_with(o) || !sel.ends_with(c) {
        return false;
    }
    m != Mark::Italic || {
        let r1 = sel.bytes().take_while(|&b| b == b'*').count();
        let r2 = sel.bytes().rev().take_while(|&b| b == b'*').count();
        r1.min(r2) % 2 == 1
    }
}

impl Buffer {
    pub fn new(text: &str) -> Buffer {
        Buffer {
            text: text.to_owned(),
            cursor: text.len(),
            anchor: text.len(),
            want_col: None,
            undo: Vec::new(),
            redo: Vec::new(),
            run: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// The selected range, start first.
    pub fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    fn snap(&self) -> Snap {
        Snap { text: self.text.clone(), cursor: self.cursor, anchor: self.anchor }
    }

    fn restore(&mut self, s: Snap) {
        self.text = s.text;
        self.cursor = s.cursor;
        self.anchor = s.anchor;
        self.run = None;
        self.want_col = None;
    }

    /// Call before every edit: pushes an undo step (unless a typed word continues) and drops redo.
    fn record(&mut self, word_run: bool) {
        let join = word_run && self.run == Some(self.cursor) && self.cursor == self.anchor;
        if !join {
            self.undo.push(self.snap());
            if self.undo.len() > UNDO_MAX {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.run = None;
        self.want_col = None;
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snap());
            self.restore(s);
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snap());
            self.restore(s);
        }
    }

    /// Replaces `start..end` and carries cursor and anchor along.
    fn edit(&mut self, start: usize, end: usize, ins: &str) {
        self.text.replace_range(start..end, ins);
        self.cursor = shift(self.cursor, start, end, ins.len());
        self.anchor = shift(self.anchor, start, end, ins.len());
    }

    fn collapse(&mut self, pos: usize) {
        self.cursor = pos;
        self.anchor = pos;
    }

    fn select(&mut self, a: usize, b: usize, forward: bool) {
        (self.anchor, self.cursor) = if forward { (a, b) } else { (b, a) };
    }

    /// Moves the cursor (a mouse click or drag): clamped, and never inside a char.
    pub fn set_cursor(&mut self, pos: usize, extend: bool) {
        let mut p = pos.min(self.text.len());
        while !self.text.is_char_boundary(p) {
            p -= 1;
        }
        self.cursor = p;
        if !extend {
            self.anchor = p;
        }
        self.run = None;
        self.want_col = None;
    }

    pub fn insert(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let word = s.chars().all(is_word);
        self.record(word);
        let (a, b) = self.selection();
        self.text.replace_range(a..b, s);
        self.collapse(a + s.len());
        if s == " " {
            self.input_rule();
        }
        self.run = word.then_some(self.cursor);
    }

    /// `[] ` at the start of a line becomes an empty task, like TipTap's input rule.
    fn input_rule(&mut self) {
        let ls = line_start(&self.text, self.cursor);
        if matches!(&self.text[ls..self.cursor - 1], "[]" | "- []" | "* []") {
            self.text.replace_range(ls..self.cursor, "- [ ] ");
            self.collapse(ls + 6);
        }
    }

    pub fn backspace(&mut self) {
        let (a, b) = self.selection();
        let a = if a != b {
            a
        } else if a == 0 {
            return;
        } else {
            prev_char(&self.text, a)
        };
        self.record(false);
        self.text.replace_range(a..b, "");
        self.collapse(a);
    }

    pub fn delete(&mut self) {
        let (a, b) = self.selection();
        let b = if a != b {
            b
        } else if b == self.text.len() {
            return;
        } else {
            next_char(&self.text, b)
        };
        self.record(false);
        self.text.replace_range(a..b, "");
        self.collapse(a);
    }

    /// Enter: continues a list item, or ends the list on an empty item.
    pub fn newline(&mut self) {
        self.record(false);
        let (a, b) = self.selection();
        self.text.replace_range(a..b, "");
        self.collapse(a);
        let ls = line_start(&self.text, a);
        let le = line_end(&self.text, a);
        let in_fence = self.text[..ls].lines().filter(|l| is_fence(l)).count() % 2 == 1;
        let mut ins = "\n".to_owned();
        if !in_fence && let Some((ind, len, block)) = parse_prefix(&self.text[ls..le]) {
            let item = ls + ind + len;
            if a >= item {
                if le == item {
                    self.text.replace_range(ls..le, "");
                    self.collapse(ls);
                    return;
                }
                if let Some(next) = continuation(&self.text[ls..item], block) {
                    ins.push_str(&next);
                }
            }
        }
        self.text.insert_str(a, &ins);
        self.collapse(a + ins.len());
    }

    fn move_to(&mut self, pos: usize, extend: bool) {
        self.cursor = pos;
        if !extend {
            self.anchor = pos;
        }
        self.run = None;
        self.want_col = None;
    }

    pub fn left(&mut self, extend: bool) {
        let (a, b) = self.selection();
        let to = if !extend && a != b { a } else { prev_char(&self.text, self.cursor) };
        self.move_to(to, extend);
    }

    pub fn right(&mut self, extend: bool) {
        let (a, b) = self.selection();
        let to = if !extend && a != b { b } else { next_char(&self.text, self.cursor) };
        self.move_to(to, extend);
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(line_start(&self.text, self.cursor), extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(line_end(&self.text, self.cursor), extend);
    }

    pub fn word_left(&mut self, extend: bool) {
        let t = self.text[..self.cursor].trim_end_matches(|c| !is_word(c)).trim_end_matches(is_word);
        self.move_to(t.len(), extend);
    }

    pub fn word_right(&mut self, extend: bool) {
        let r = self.text[self.cursor..].trim_start_matches(|c| !is_word(c)).trim_start_matches(is_word);
        self.move_to(self.text.len() - r.len(), extend);
    }

    pub fn up(&mut self, extend: bool) {
        self.vertical(false, extend);
    }

    pub fn down(&mut self, extend: bool) {
        self.vertical(true, extend);
    }

    fn vertical(&mut self, down: bool, extend: bool) {
        let ls = line_start(&self.text, self.cursor);
        let col = self.want_col.unwrap_or_else(|| self.text[ls..self.cursor].chars().count());
        let target = if down {
            let le = line_end(&self.text, self.cursor);
            (le < self.text.len()).then_some(le + 1)
        } else {
            (ls > 0).then(|| line_start(&self.text, ls - 1))
        };
        let pos = match target {
            Some(t) => {
                let line = &self.text[t..line_end(&self.text, t)];
                t + line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
            }
            None if down => self.text.len(),
            None => 0,
        };
        self.move_to(pos, extend);
        self.want_col = Some(col);
    }

    /// Wraps or unwraps the selection in the mark's markers; with nothing selected, inserts the
    /// pair (or removes an empty pair around the cursor).
    pub fn toggle_mark(&mut self, mark: Mark) {
        let (o, c) = mark.pair();
        let (a, b) = self.selection();
        let forward = self.cursor >= self.anchor;
        self.record(false);
        if wrapped(mark, &self.text[..a], &self.text[b..]) {
            self.text.replace_range(b..b + c.len(), "");
            self.text.replace_range(a - o.len()..a, "");
            let (a, b) = (a - o.len(), b - o.len());
            if a == b {
                self.collapse(a);
            } else {
                self.select(a, b, forward);
            }
        } else if a != b && wrapped_in(mark, &self.text[a..b]) {
            let inner = self.text[a + o.len()..b - c.len()].to_owned();
            self.text.replace_range(a..b, &inner);
            self.select(a, a + inner.len(), forward);
        } else {
            self.text.insert_str(b, c);
            self.text.insert_str(a, o);
            if a == b {
                self.collapse(a + o.len());
            } else {
                self.select(a + o.len(), b + o.len(), forward);
            }
        }
    }

    /// Gives every line of the selection the block's prefix, replacing any other block prefix;
    /// when they all have it already, removes it. Code wraps the lines in fences instead.
    pub fn set_block(&mut self, block: Block) {
        let (a, b) = self.selection();
        let ls = line_start(&self.text, a);
        // a selection that ends at a line start does not include that line
        let last = if b > a && b == line_start(&self.text, b) { b - 1 } else { b };
        let le = line_end(&self.text, last);
        self.record(false);
        if block == Block::Code {
            return self.toggle_fence(ls, le);
        }
        let mut lines = Vec::new();
        let mut at = ls;
        for l in self.text[ls..le].split('\n') {
            lines.push((at, parse_prefix(l)));
            at += l.len() + 1;
        }
        let all = lines.iter().all(|(_, p)| matches!(p, Some((_, _, k)) if *k == block));
        for (i, (at, p)) in lines.into_iter().enumerate().rev() {
            let (from, to) = match p {
                Some((ind, len, _)) => (at + ind, at + ind + len),
                None => (at + indent_of(&self.text[at..]), at + indent_of(&self.text[at..])),
            };
            let ins = if all { String::new() } else { block_prefix(block, i + 1) };
            self.edit(from, to, &ins);
        }
    }

    fn toggle_fence(&mut self, ls: usize, le: usize) {
        let t = &self.text;
        let first = t[ls..le].split('\n').next().unwrap_or("");
        let last = t[ls..le].rsplit('\n').next().unwrap_or("");
        if ls != le && t[ls..le].contains('\n') && is_fence(first) && is_fence(last) {
            // fences are the first and last selected lines
            let (f, l) = (first.len(), last.len());
            self.edit(le - l - 1, le, "");
            // with just the two fence lines the first edit already took the newline
            self.edit(ls, (ls + f + 1).min(self.text.len()), "");
            return;
        }
        let prev = (ls > 0).then(|| line_start(t, ls - 1)).filter(|&p| is_fence(&t[p..ls - 1]));
        let next = (le < t.len()).then(|| line_end(t, le + 1)).filter(|&n| is_fence(&t[le + 1..n]));
        if let (Some(p), Some(n)) = (prev, next) {
            // fences are the lines just outside
            self.edit(le, n, "");
            self.edit(p, ls, "");
            return;
        }
        // a cursor at the end of the last line stays before the closing fence
        let (c, a) = (self.cursor, self.anchor);
        self.edit(le, le, "\n```");
        if c == le {
            self.cursor = le;
        }
        if a == le {
            self.anchor = le;
        }
        self.edit(ls, ls, "```\n");
    }

    /// Flips `[ ]`/`[x]` on the cursor's line. A newly checked task (with its indented children)
    /// moves to the end of its run of tasks when `move_checked_down`.
    pub fn toggle_task(&mut self, move_checked_down: bool) {
        let ls = line_start(&self.text, self.cursor);
        let le = line_end(&self.text, self.cursor);
        let Some((ind, _, Block::Task)) = parse_prefix(&self.text[ls..le]) else { return };
        let mark_at = ls + ind + 3;
        let checking = self.text.as_bytes()[mark_at] == b' ';
        self.record(false);
        self.text.replace_range(mark_at..mark_at + 1, if checking { "x" } else { " " });
        if !(checking && move_checked_down) {
            return;
        }
        let col = self.cursor.saturating_sub(ls);
        let idx = self.text[..ls].matches('\n').count();
        let mut lines: Vec<&str> = self.text.split('\n').collect();
        let blank = |l: &str| l.trim().is_empty();
        let mut unit_end = idx + 1;
        while unit_end < lines.len() && !blank(lines[unit_end]) && indent_of(lines[unit_end]) > ind {
            unit_end += 1;
        }
        let mut dest = unit_end;
        while dest < lines.len() && !blank(lines[dest]) {
            let l = lines[dest];
            let sibling = matches!(parse_prefix(l), Some((i, _, Block::Task)) if i == ind);
            if !(sibling || indent_of(l) > ind) {
                break;
            }
            dest += 1;
        }
        if dest == unit_end {
            return;
        }
        let unit: Vec<&str> = lines.drain(idx..unit_end).collect();
        let at = dest - unit.len();
        let new_start: usize = lines[..at].iter().map(|l| l.len() + 1).sum();
        lines.splice(at..at, unit);
        self.text = lines.join("\n");
        self.collapse(new_start + col);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    /// 1..=6 on a heading line.
    pub heading: u8,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub underline: bool,
    pub highlight: bool,
    pub link: bool,
    /// On a `> ` line.
    pub quote: bool,
    /// `Some(checked)` on a task line.
    pub task: Option<bool>,
}

/// A run of bytes in a line with one style. Marker characters belong to their span.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}

/// `open ... close` pairs outside masked bytes, as byte ranges including the markers. Content is
/// non-empty and (when `flank`) does not start or end with whitespace, so `5 * 3 * 2` is plain.
/// `solo` wants a lone `*` that is not part of `**`.
fn pairs(line: &str, open: &str, close: &str, mask: &[bool], solo: bool, flank: bool) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let ok = |p: usize, len: usize| {
        !mask[p] && (!solo || ((p == 0 || b[p - 1] != b'*') && b.get(p + len) != Some(&b'*')))
    };
    let mut out = Vec::new();
    let mut i = 0;
    // every marker starts with an ASCII byte, so `+ 1` stays on a char boundary
    while let Some(p) = line[i..].find(open).map(|x| x + i) {
        i = p + 1;
        if !ok(p, open.len()) {
            continue;
        }
        let q = p + open.len();
        let mut j = q;
        while let Some(c) = line[j..].find(close).map(|x| x + j) {
            j = c + 1;
            if c == q || !ok(c, close.len()) {
                continue;
            }
            let inner = &line[q..c];
            if flank && inner.starts_with(char::is_whitespace) {
                break;
            }
            if flank && inner.ends_with(char::is_whitespace) {
                continue;
            }
            out.push((p, c + close.len()));
            i = c + close.len();
            break;
        }
    }
    out
}

/// First `ch` at or after `from` that is not masked.
fn find_unmasked(line: &str, from: usize, ch: char, mask: &[bool]) -> Option<usize> {
    line[from..].char_indices().find(|&(i, c)| c == ch && !mask[from + i]).map(|(i, _)| from + i)
}

/// `[text](url)` as (`[`, `]`, end), with code-masked bytes never part of the link's brackets.
fn links(line: &str, mask: &[bool]) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = line[i..].find('[').map(|x| x + i) {
        i = p + 1;
        if mask[p] {
            continue;
        }
        let Some(r) = find_unmasked(line, p + 1, ']', mask) else { continue };
        if r == p + 1 || !line[r + 1..].starts_with('(') {
            continue;
        }
        if let Some(e) = find_unmasked(line, r + 2, ')', mask) {
            out.push((p, r, e + 1));
            i = e + 1;
        }
    }
    out
}

/// Longest line `spans` styles, in bytes.
const STYLE_MAX: usize = 2048;

/// What a construct adds to the style of its bytes.
type Paint = fn(&mut Style);

/// One inline construct: its bytes (markers included), its opener's and closer's lengths, and
/// the style it adds.
struct Inline {
    range: (usize, usize),
    open: usize,
    close: usize,
    paint: Paint,
}

/// Every inline construct of one line. Code is found first and masks the others' markers.
fn inlines(line: &str) -> Vec<Inline> {
    let none = vec![false; line.len()];
    let code = pairs(line, "`", "`", &none, false, false);
    let mut mask = none;
    for &(s, e) in &code {
        mask[s..e].fill(true);
    }
    let mut out: Vec<Inline> = code.into_iter().map(|range| Inline { range, open: 1, close: 1, paint: |s| s.code = true }).collect();
    let marks: [(&str, &str, bool, Paint); 6] = [
        ("***", "***", false, |s| (s.bold, s.italic) = (true, true)),
        ("**", "**", false, |s| s.bold = true),
        ("*", "*", true, |s| s.italic = true),
        ("~~", "~~", false, |s| s.strike = true),
        ("<u>", "</u>", false, |s| s.underline = true),
        ("==", "==", false, |s| s.highlight = true),
    ];
    for (o, c, solo, paint) in marks {
        out.extend(pairs(line, o, c, &mask, solo, true).into_iter().map(|range| Inline { range, open: o.len(), close: c.len(), paint }));
    }
    out.extend(links(line, &mask).into_iter().map(|(p, r, e)| Inline { range: (p, e), open: 1, close: e - r, paint: |s| s.link = true }));
    out
}

/// Styled byte ranges of one line, sorted and not overlapping; unstyled text has no span.
/// Nested marks give one span per stretch with all its styles. Code suppresses other marks
/// inside it, and a marker without its partner is plain text. Heading, quote and task cover
/// the whole line.
pub fn spans(line: &str) -> Vec<Span> {
    // ponytail: marker matching rescans to the end of the line for each unmatched opener, so
    // cost grows with the square of the line; longer lines (pasted blobs) go unstyled.
    if line.len() > STYLE_MAX {
        return Vec::new();
    }
    let mut st = vec![Style::default(); line.len()];
    for i in inlines(line) {
        st[i.range.0..i.range.1].iter_mut().for_each(i.paint);
    }
    let t = line.trim_start();
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        st.iter_mut().for_each(|s| s.heading = hashes as u8);
    }
    match parse_prefix(line) {
        Some((ind, _, Block::Task)) => {
            let checked = line.as_bytes()[ind + 3] != b' ';
            st.iter_mut().for_each(|s| s.task = Some(checked));
        }
        Some((_, _, Block::Quote)) => st.iter_mut().for_each(|s| s.quote = true),
        _ => {}
    }
    let mut out: Vec<Span> = Vec::new();
    for (i, style) in st.into_iter().enumerate() {
        if style == Style::default() {
            continue;
        }
        match out.last_mut() {
            Some(l) if l.range.end == i && l.style == style => l.range.end = i + 1,
            _ => out.push(Span { range: i..i + 1, style }),
        }
    }
    out
}

/// Byte ranges of the line's markdown markers, sorted: the block prefix (`# `, `> `, `- `,
/// `1. `, `- [ ] `) and each styled construct's opener and closer (a link's `[` and `](url)`).
/// Markers that share bytes (`***` is bold's and italic's) are one range.
pub fn marks(line: &str) -> Vec<Range<usize>> {
    if line.len() > STYLE_MAX {
        return Vec::new();
    }
    let mut all: Vec<Range<usize>> = parse_prefix(line).map(|(ind, len, _)| ind..ind + len).into_iter().collect();
    for i in inlines(line) {
        all.push(i.range.0..i.range.0 + i.open);
        all.push(i.range.1 - i.close..i.range.1);
    }
    all.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in all {
        match out.last_mut() {
            Some(l) if r.start < l.end => l.end = l.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// How a hidden bullet marker draws.
const BULLET: &str = "• ";

/// The line as drawn, and the source byte of each display byte (plus the line's end). With
/// `hide` the marks are left out, except a list's: a bullet draws as a dot and a number stays.
pub fn display(line: &str, hide: bool) -> (String, Vec<usize>) {
    let (mut out, mut map) = (String::new(), Vec::new());
    let list = parse_prefix(line).filter(|p| matches!(p.2, Block::Bullet | Block::Numbered));
    let mut at = 0;
    for r in if hide { marks(line) } else { Vec::new() } {
        out.push_str(&line[at..r.start]);
        map.extend(at..r.start);
        match list {
            Some((ind, len, block)) if r == (ind..ind + len) => {
                let drawn = if block == Block::Bullet { BULLET } else { &line[r.clone()] };
                out.push_str(drawn);
                map.extend(if block == Block::Bullet { vec![r.start; BULLET.len()] } else { r.clone().collect() });
            }
            _ => {}
        }
        at = r.end;
    }
    out.push_str(&line[at..]);
    map.extend(at..=line.len());
    (out, map)
}

/// `spans` in the display bytes of `display`'s `map`; a span of hidden markers only is gone.
pub fn to_display(spans: &[Span], map: &[usize]) -> Vec<Span> {
    let at = |src: usize| map.partition_point(|&m| m < src);
    spans
        .iter()
        .map(|s| Span { range: at(s.range.start)..at(s.range.end), style: s.style })
        .filter(|s| !s.range.is_empty())
        .collect()
}

/// One line ready to draw: its `spans` as typed, and the line with its marks hidden (`shown`,
/// the display-to-source `map`, and the `hidden` spans in display bytes).
pub struct Styled {
    pub spans: Vec<Span>,
    pub shown: String,
    pub map: Rc<[usize]>,
    pub hidden: Vec<Span>,
}

/// Styled lines keyed by their text, so a keystroke restyles only the line it changed (and the
/// lines an Enter moves down keep theirs). `sweep` after each frame drops lines it didn't draw.
#[derive(Default)]
pub struct SpanCache {
    lines: HashMap<String, (bool, Styled)>,
    #[cfg(test)]
    misses: usize,
}

impl SpanCache {
    pub fn get(&mut self, line: &str) -> &Styled {
        if !self.lines.contains_key(line) {
            #[cfg(test)]
            {
                self.misses += 1;
            }
            let spans = spans(line);
            let (shown, map) = display(line, true);
            let hidden = to_display(&spans, &map);
            self.lines.insert(line.to_owned(), (false, Styled { spans, shown, map: map.into(), hidden }));
        }
        let (used, styled) = self.lines.get_mut(line).expect("inserted above");
        *used = true;
        styled
    }

    pub fn sweep(&mut self) {
        self.lines.retain(|_, (used, _)| std::mem::take(used));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(text: &str, a: usize, b: usize) -> Buffer {
        let mut buf = Buffer::new(text);
        buf.set_cursor(a, false);
        buf.set_cursor(b, true);
        buf
    }

    fn typed(buf: &mut Buffer, s: &str) {
        for c in s.chars() {
            buf.insert(&c.to_string());
        }
    }

    #[test]
    fn backspace_and_delete_keep_utf8_valid() {
        let mut b = Buffer::new("a😀");
        b.backspace();
        assert_eq!((b.text(), b.cursor()), ("a", 1));
        let mut b = Buffer::new("😀b");
        b.set_cursor(0, false);
        b.delete();
        assert_eq!(b.text(), "b");
        let mut b = Buffer::new("é😀x");
        b.set_cursor(1, false); // inside é: rounds down
        assert_eq!(b.cursor(), 0);
        b.right(false);
        b.right(false);
        assert_eq!(b.cursor(), "é😀".len());
        b.left(false);
        assert_eq!(b.cursor(), "é".len());
        b.backspace();
        assert_eq!(b.text(), "😀x");
        b.set_cursor(0, false);
        b.backspace();
        b.delete();
        b.delete();
        b.delete();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn insert_replaces_selection_and_anchor_follows() {
        let mut b = sel("hello world", 6, 11);
        b.insert("x");
        assert_eq!((b.text(), b.cursor(), b.anchor()), ("hello x", 7, 7));
        assert_eq!(b.selection(), (7, 7));
        b.insert("");
        assert_eq!(b.text(), "hello x");
    }

    #[test]
    fn toggle_bold_round_trips() {
        let mut b = sel("hello", 0, 5);
        b.toggle_mark(Mark::Bold);
        assert_eq!((b.text(), b.selection()), ("**hello**", (2, 7)));
        b.toggle_mark(Mark::Bold);
        assert_eq!((b.text(), b.selection()), ("hello", (0, 5)));
        // markers inside the selection
        let mut b = sel("**hello**", 0, 9);
        b.toggle_mark(Mark::Bold);
        assert_eq!(b.text(), "hello");
        // empty selection: pair with the cursor between, toggled off again
        let mut b = Buffer::new("");
        b.toggle_mark(Mark::Bold);
        assert_eq!((b.text(), b.cursor()), ("****", 2));
        b.toggle_mark(Mark::Bold);
        assert_eq!((b.text(), b.cursor()), ("", 0));
        // one undo step each
        let mut b = sel("hi", 0, 2);
        b.toggle_mark(Mark::Bold);
        b.undo();
        assert_eq!((b.text(), b.selection()), ("hi", (0, 2)));
    }

    #[test]
    fn every_mark_wraps_and_unwraps() {
        for (m, o, c) in [
            (Mark::Bold, "**", "**"),
            (Mark::Italic, "*", "*"),
            (Mark::Underline, "<u>", "</u>"),
            (Mark::Strike, "~~", "~~"),
            (Mark::Code, "`", "`"),
            (Mark::Highlight, "==", "=="),
        ] {
            let mut b = sel("é hi", 3, 5);
            b.toggle_mark(m);
            assert_eq!(b.text(), format!("é {o}hi{c}"));
            b.toggle_mark(m);
            assert_eq!(b.text(), "é hi");
        }
    }

    #[test]
    fn italic_and_bold_do_not_eat_each_others_stars() {
        let mut b = sel("**x**", 2, 3);
        b.toggle_mark(Mark::Italic);
        assert_eq!(b.text(), "***x***");
        b.toggle_mark(Mark::Italic);
        assert_eq!(b.text(), "**x**");
        let mut b = sel("**x**", 0, 5);
        b.toggle_mark(Mark::Italic);
        assert_eq!(b.text(), "***x***");
        b.toggle_mark(Mark::Italic);
        assert_eq!(b.text(), "**x**");
        b.toggle_mark(Mark::Bold);
        assert_eq!(b.text(), "x");
    }

    #[test]
    fn square_brackets_become_a_task() {
        let mut b = Buffer::new("");
        typed(&mut b, "[] ");
        assert_eq!((b.text(), b.cursor()), ("- [ ] ", 6));
        b.undo();
        assert_eq!(b.text(), "[]");
        let mut b = Buffer::new("a\n");
        typed(&mut b, "- [] ");
        assert_eq!(b.text(), "a\n- [ ] ");
        let mut b = Buffer::new("a [] ");
        b.insert(" ");
        assert_eq!(b.text(), "a []  ");
        let mut b = Buffer::new("x\n[]");
        b.insert(" ");
        assert_eq!(b.text(), "x\n- [ ] ");
    }

    #[test]
    fn enter_continues_lists() {
        let mut b = Buffer::new("- a");
        b.newline();
        assert_eq!(b.text(), "- a\n- ");
        let mut b = Buffer::new("  * a");
        b.newline();
        assert_eq!(b.text(), "  * a\n  * ");
        let mut b = Buffer::new("9. a");
        b.newline();
        assert_eq!(b.text(), "9. a\n10. ");
        let mut b = Buffer::new("- [x] a");
        b.newline();
        assert_eq!(b.text(), "- [x] a\n- [ ] ");
        // mid-item split carries the tail
        let mut b = Buffer::new("- abcd");
        b.set_cursor(4, false);
        b.newline();
        assert_eq!((b.text(), b.cursor()), ("- ab\n- cd", 7));
        // inside the prefix, and on plain lines, it is a plain newline
        let mut b = Buffer::new("- a");
        b.set_cursor(1, false);
        b.newline();
        assert_eq!(b.text(), "-\n a");
        let mut b = Buffer::new("# t");
        b.newline();
        assert_eq!(b.text(), "# t\n");
        // not inside a fence
        let mut b = Buffer::new("```\n- a");
        b.newline();
        assert_eq!(b.text(), "```\n- a\n");
        // a selection is replaced first
        let mut b = sel("- abc", 3, 5);
        b.newline();
        assert_eq!(b.text(), "- a\n- ");
    }

    #[test]
    fn enter_on_empty_item_ends_the_list() {
        let mut b = Buffer::new("- a\n- ");
        b.newline();
        assert_eq!((b.text(), b.cursor()), ("- a\n", 4));
        let mut b = Buffer::new("1. ");
        b.newline();
        assert_eq!(b.text(), "");
        let mut b = Buffer::new("- [ ] ");
        b.newline();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn checked_task_moves_down() {
        let mut b = Buffer::new("- [ ] a\n- [ ] b\n- [ ] c");
        b.set_cursor(0, false);
        b.toggle_task(true);
        assert_eq!((b.text(), b.cursor()), ("- [ ] b\n- [ ] c\n- [x] a", 16));
        b.undo();
        assert_eq!((b.text(), b.cursor()), ("- [ ] a\n- [ ] b\n- [ ] c", 0));
        // off: flips in place; unchecking never moves
        b.toggle_task(false);
        assert_eq!(b.text(), "- [x] a\n- [ ] b\n- [ ] c");
        b.toggle_task(true);
        assert_eq!(b.text(), "- [ ] a\n- [ ] b\n- [ ] c");
        // the list ends at a blank line; children travel with their item
        let mut b = Buffer::new("- [ ] a\n  - n\n- [ ] b\n  - m\n\n- [ ] z\n");
        b.set_cursor(0, false);
        b.toggle_task(true);
        assert_eq!(b.text(), "- [ ] b\n  - m\n- [x] a\n  - n\n\n- [ ] z\n");
        // not a task: nothing happens
        let mut b = Buffer::new("plain");
        b.toggle_task(true);
        assert_eq!(b.text(), "plain");
    }

    #[test]
    fn undo_restores_the_previous_word_and_cursor() {
        let mut b = Buffer::new("");
        typed(&mut b, "hello world");
        b.undo();
        assert_eq!((b.text(), b.cursor()), ("hello ", 6));
        b.redo();
        assert_eq!((b.text(), b.cursor()), ("hello world", 11));
        b.undo();
        b.undo();
        assert_eq!(b.text(), "hello");
        b.undo();
        assert_eq!((b.text(), b.cursor()), ("", 0));
        b.undo();
        assert_eq!(b.text(), "");
        // a new edit drops the redo tail
        b.redo();
        b.undo();
        b.insert("x");
        b.redo();
        assert_eq!(b.text(), "x");
    }

    #[test]
    fn undo_steps_break_on_cursor_jump_and_edit_kind() {
        let mut b = Buffer::new("ab");
        b.insert("c");
        b.set_cursor(0, false);
        b.insert("d");
        b.undo();
        assert_eq!((b.text(), b.cursor()), ("abc", 0));
        b.undo();
        assert_eq!((b.text(), b.cursor()), ("ab", 2));
        let mut b = Buffer::new("");
        typed(&mut b, "ab");
        b.backspace();
        typed(&mut b, "c");
        b.undo();
        assert_eq!(b.text(), "a");
        b.undo();
        assert_eq!(b.text(), "ab");
        b.undo();
        assert_eq!(b.text(), "");
        // selection replacement is its own step and restores the selection
        let mut b = sel("hello", 0, 5);
        b.insert("x");
        b.undo();
        assert_eq!((b.text(), b.selection()), ("hello", (0, 5)));
        // moves do not create steps
        b.left(false);
        b.undo();
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn cursor_moves_and_shift_select() {
        let mut b = Buffer::new("héllo wörld");
        b.home(false);
        assert_eq!(b.cursor(), 0);
        b.end(true);
        assert_eq!(b.selection(), (0, "héllo wörld".len()));
        b.left(false); // collapses to the start of the selection
        assert_eq!((b.cursor(), b.anchor()), (0, 0));
        b.right(true);
        b.right(true);
        assert_eq!(b.selection(), (0, 3));
        b.right(false);
        assert_eq!((b.cursor(), b.anchor()), (3, 3));
        b.set_cursor(0, false);
        b.set_cursor(3, true);
        b.left(true);
        assert_eq!((b.cursor(), b.anchor()), (1, 0));
    }

    #[test]
    fn word_moves_are_unicode_aware() {
        let t = "héllo, wörld  ünï";
        let mut b = Buffer::new(t);
        b.word_left(false);
        assert_eq!(&t[b.cursor()..], "ünï");
        b.word_left(false);
        assert_eq!(&t[b.cursor()..], "wörld  ünï");
        b.word_left(true);
        assert_eq!(&t[b.cursor()..], "héllo, wörld  ünï");
        b.word_right(false);
        assert_eq!(&t[..b.cursor()], "héllo");
        b.word_right(false);
        assert_eq!(&t[..b.cursor()], "héllo, wörld");
        b.word_right(true);
        b.word_right(true);
        assert_eq!(b.cursor(), t.len());
        assert_eq!(&t[b.selection().0..b.selection().1], "  ünï");
    }

    #[test]
    fn up_down_keep_the_desired_column() {
        let mut b = Buffer::new("héllo\nab\nwörld!");
        b.set_cursor(0, false);
        b.right(false);
        b.right(false);
        b.right(false);
        b.right(false); // column 4
        b.down(false);
        assert_eq!(b.cursor(), "héllo\nab".len()); // clamped to the short line
        b.down(false);
        assert_eq!(&b.text()[..b.cursor()], "héllo\nab\nwörl"); // back to column 4
        b.down(false);
        assert_eq!(b.cursor(), b.text().len());
        b.up(false);
        b.up(false);
        b.up(false);
        assert_eq!(b.cursor(), 0);
        b.set_cursor(8, false);
        b.down(true);
        assert_eq!(b.selection(), (8, 11));
    }

    #[test]
    fn set_block_replaces_and_reverts() {
        let mut b = Buffer::new("hello");
        b.set_block(Block::H1);
        assert_eq!((b.text(), b.cursor()), ("# hello", 7));
        b.set_block(Block::H2);
        assert_eq!(b.text(), "## hello");
        b.set_block(Block::H3);
        assert_eq!(b.text(), "### hello");
        b.set_block(Block::Quote);
        assert_eq!(b.text(), "> hello");
        b.set_block(Block::Bullet);
        assert_eq!(b.text(), "- hello");
        b.set_block(Block::Numbered);
        assert_eq!(b.text(), "1. hello");
        b.set_block(Block::Task);
        assert_eq!(b.text(), "- [ ] hello");
        b.set_block(Block::Task);
        assert_eq!((b.text(), b.cursor()), ("hello", 5));
        let mut b = Buffer::new("  7. a");
        b.set_block(Block::Bullet);
        assert_eq!(b.text(), "  - a");
        b.set_block(Block::Bullet);
        assert_eq!(b.text(), "  a");
        let mut b = Buffer::new("- [x] a");
        b.set_block(Block::Task);
        assert_eq!(b.text(), "a");
        let mut b = Buffer::new("* a");
        b.set_block(Block::Bullet);
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn set_block_covers_every_selected_line() {
        let mut b = sel("a\nb\nc", 0, 5);
        b.set_block(Block::Numbered);
        assert_eq!(b.text(), "1. a\n2. b\n3. c");
        b.set_block(Block::Bullet);
        assert_eq!(b.text(), "- a\n- b\n- c");
        b.set_block(Block::Bullet);
        assert_eq!(b.text(), "a\nb\nc");
        // mixed lines: not all the block, so all become it
        let mut b = sel("- a\nb", 0, 5);
        b.set_block(Block::Bullet);
        assert_eq!(b.text(), "- a\n- b");
        // a selection ending at a line start leaves that line alone
        let mut b = sel("a\nb\n", 0, 2);
        b.set_block(Block::Quote);
        assert_eq!(b.text(), "> a\nb\n");
        b.undo();
        assert_eq!(b.text(), "a\nb\n");
    }

    #[test]
    fn code_block_wraps_and_unwraps() {
        let mut b = sel("a\nb", 0, 3);
        b.set_block(Block::Code);
        assert_eq!(b.text(), "```\na\nb\n```");
        b.set_block(Block::Code);
        assert_eq!(b.text(), "a\nb");
        let mut b = Buffer::new("x\na\ny");
        b.set_cursor(2, false);
        b.set_block(Block::Code);
        assert_eq!(b.text(), "x\n```\na\n```\ny");
        assert_eq!(b.cursor(), 6);
        b.set_block(Block::Code); // cursor between the fences: toggles off
        assert_eq!(b.text(), "x\na\ny");
        assert_eq!(b.cursor(), 2);
        let mut b = Buffer::new("a");
        b.set_block(Block::Code);
        assert_eq!((b.text(), b.cursor()), ("```\na\n```", 5));
        let mut b = Buffer::new("");
        b.set_block(Block::Code);
        assert_eq!(b.text(), "```\n\n```");
    }

    #[test]
    fn code_fence_edge_cases() {
        // an empty fenced block, fully selected, at the very start and end of the text
        let mut b = sel("```\n```", 0, 7);
        b.set_block(Block::Code);
        assert_eq!((b.text(), b.selection()), ("", (0, 0)));
        let mut b = sel("x\n```\n```", 2, 9);
        b.set_block(Block::Code);
        assert_eq!(b.text(), "x\n");
        let mut b = sel("```\n```\ny", 0, 7);
        b.set_block(Block::Code);
        assert_eq!(b.text(), "y");
        // fences just outside an empty line at the end of the text
        let mut b = Buffer::new("```\n\n```");
        b.set_cursor(4, false);
        b.set_block(Block::Code);
        assert_eq!((b.text(), b.cursor()), ("", 0));
        // a selection ending at the next line start keeps that end after the fence
        let mut b = sel("a\nb\n", 0, 2);
        b.set_block(Block::Code);
        assert_eq!((b.text(), b.selection()), ("```\na\n```\nb\n", (4, 10)));
        // the cursor at the end of the last line stays inside the block
        let mut b = Buffer::new("a\nb");
        b.set_block(Block::Code);
        assert_eq!(b.cursor(), 7);
    }

    #[test]
    fn paragraph_strips_any_block_prefix() {
        for (from, to) in [
            ("# a", "a"),
            ("## a", "a"),
            ("### a", "a"),
            ("#### a", "a"),
            ("###### a", "a"),
            ("> a", "a"),
            ("- a", "a"),
            ("* a", "a"),
            ("12. a", "a"),
            ("- [ ] a", "a"),
            ("- [x] a", "a"),
            ("  - a", "  a"),
            ("plain", "plain"),
            ("####### seven", "####### seven"),
        ] {
            let mut b = Buffer::new(from);
            b.set_block(Block::Paragraph);
            assert_eq!(b.text(), to, "{from}");
        }
        let mut b = sel("# a\n- b\nc", 0, 9);
        b.set_block(Block::Paragraph);
        assert_eq!(b.text(), "a\nb\nc");
        b.undo();
        assert_eq!(b.text(), "# a\n- b\nc");
        // an h4 is not an H3: setting H3 replaces it
        let mut b = Buffer::new("#### a");
        b.set_block(Block::H3);
        assert_eq!(b.text(), "### a");
    }

    #[test]
    fn long_lines_are_not_styled_and_stay_fast() {
        let start = std::time::Instant::now();
        for unit in ["a *b ", "**a ", "[", "[x](", "`a ", "~~a ", "<u>a "] {
            assert!(spans(&unit.repeat(200_000 / unit.len())).is_empty(), "{unit}");
        }
        // just under the cap still styles, and pathological input there is cheap too
        assert!(!spans(&format!("**b** {}", "a".repeat(STYLE_MAX - 20))).is_empty());
        for unit in ["a *b ", "**a ", "[", "[x](", "~~a "] {
            spans(&unit.repeat(STYLE_MAX / unit.len()));
        }
        assert!(start.elapsed().as_secs() < 2);
    }

    fn styled(line: &str) -> Vec<(&str, Style)> {
        spans(line).into_iter().map(|s| (&line[s.range], s.style)).collect()
    }

    #[test]
    fn spans_cover_every_construct() {
        let h = styled("## Title");
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].0, h[0].1.heading), ("## Title", 2));
        assert_eq!(styled("  # x")[0].1.heading, 1);
        assert!(styled("#nospace").is_empty());
        assert!(styled("####### seven").is_empty());

        let s = styled("a **b** c");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, "**b**");
        assert!(s[0].1.bold);
        let s = styled("x *i* y");
        assert_eq!((s[0].0, s[0].1.italic), ("*i*", true));
        let s = styled("~~s~~ <u>u</u> ==h==");
        assert_eq!(s.len(), 3);
        assert!(s[0].1.strike && s[1].1.underline && s[2].1.highlight);
        assert_eq!((s[1].0, s[2].0), ("<u>u</u>", "==h=="));
        let s = styled("see [docs](http://a.b/c_d) now");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, "[docs](http://a.b/c_d)");
        assert!(s[0].1.link);
        let s = styled("`c`");
        assert!(s[0].1.code);

        let s = styled("- [x] done **b**");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].1.task, Some(true));
        assert!(s[1].1.bold && s[1].1.task == Some(true));
        assert_eq!(styled("- [ ] todo")[0].1.task, Some(false));
        assert_eq!(styled("  * [X] caps")[0].1.task, Some(true));
        assert!(styled("- [] nope").is_empty());
    }

    #[test]
    fn spans_nest_without_overlapping() {
        let s = styled("**a *b* c**");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].0, "**a ");
        assert!(s[0].1.bold && !s[0].1.italic);
        assert_eq!(s[1].0, "*b*");
        assert!(s[1].1.bold && s[1].1.italic);
        assert_eq!(s[2].0, " c**");
        let s = styled("***x***");
        assert_eq!(s.len(), 1);
        assert!(s[0].1.bold && s[0].1.italic);
    }

    #[test]
    fn spans_code_suppresses_marks_and_unclosed_is_plain() {
        let s = styled("*i* and `**not** *x*` end");
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].0, "`**not** *x*`");
        assert!(s[1].1.code && !s[1].1.bold && !s[1].1.italic);
        // a closer inside code does not close an opener outside it
        assert!(styled("**a `b**` c").iter().all(|(_, st)| !st.bold));
        let s = styled("[a`](b)`");
        assert!(s.len() == 1 && s[0].1.code && !s[0].1.link);
        assert!(styled("**unclosed").is_empty());
        assert!(styled("a ` b").is_empty());
        assert!(styled("5 * 3 * 2").is_empty());
        assert!(styled("**  **").is_empty());
        assert!(styled("[t](u").is_empty());
        assert!(styled("[](u)").is_empty());
        assert!(styled("").is_empty());
    }

    fn marked(line: &str) -> Vec<&str> {
        marks(line).into_iter().map(|r| &line[r]).collect()
    }

    #[test]
    fn marks_find_every_marker() {
        assert_eq!(marked("# a"), ["# "]);
        assert_eq!(marked("### a"), ["### "]);
        assert_eq!(marked("> a"), ["> "]);
        assert_eq!(marked("- a"), ["- "]);
        assert_eq!(marked("  1. a"), ["1. "]);
        assert_eq!(marked("- [ ] a"), ["- [ ] "]);
        assert_eq!(marked("- [x] a"), ["- [x] "]);
        assert_eq!(marked("a **b** c"), ["**", "**"]);
        assert_eq!(marked("*i*"), ["*", "*"]);
        assert_eq!(marked("~~s~~"), ["~~", "~~"]);
        assert_eq!(marked("`c`"), ["`", "`"]);
        assert_eq!(marked("==h=="), ["==", "=="]);
        assert_eq!(marked("<u>u</u>"), ["<u>", "</u>"]);
        assert_eq!(marked("see [docs](http://a.b) now"), ["[", "](http://a.b)"]);
        // bold-italic shares its stars with bold and italic: one marker each side
        assert_eq!(marked("***x***"), ["***", "***"]);
        // a prefix right before an opener stays its own marker
        assert_eq!(marked("- **b**"), ["- ", "**", "**"]);
        assert!(marked("plain").is_empty());
        assert!(marked("**unclosed").is_empty());
        assert!(marked(&format!("**b** {}", "a".repeat(STYLE_MAX))).is_empty(), "too long to style: drawn as typed");
    }

    #[test]
    fn display_hides_marks_and_maps_back_to_the_source() {
        for (line, shown) in [
            ("# Title", "Title"),
            ("a **b** *ï* `c`", "a b ï c"),
            ("- [x] done ~~é~~", "done é"),
            ("> said [it](u)", "said it"),
            ("<u>u</u>==h==", "uh"),
            ("  3. item", "  3. item"),
        ] {
            let (text, map) = display(line, true);
            assert_eq!(text, shown, "{line}");
            assert_eq!(map.len(), text.len() + 1);
            assert_eq!(map[text.len()], line.len(), "the end maps to the line's end");
            for (d, c) in text.char_indices() {
                assert_eq!(line[map[d]..].chars().next(), Some(c), "{line} @{d}");
            }
        }
        // a bullet's marker draws as a dot, which maps to the marker
        let (text, map) = display("- **b**", true);
        assert_eq!(text, "• b");
        assert_eq!(map, [0, 0, 0, 0, 4, 7]);
        // not hidden: as typed, byte for byte
        let (text, map) = display("**b**", false);
        assert_eq!((text.as_str(), map), ("**b**", vec![0, 1, 2, 3, 4, 5]));
    }

    #[test]
    fn a_heading_keeps_its_style_with_its_marks_hidden() {
        let (text, map) = display("## **Big** day", true);
        let s = to_display(&spans("## **Big** day"), &map);
        assert_eq!(text, "Big day");
        assert!(s.iter().all(|s| s.style.heading == 2));
        assert_eq!(s.first().map(|s| s.range.start), Some(0));
        assert_eq!(s.last().map(|s| s.range.end), Some(text.len()));
        assert_eq!(s.iter().find(|s| s.style.bold).map(|s| &text[s.range.clone()]), Some("Big"));
        // a span made only of hidden markers is gone
        let (_, map) = display("****", true);
        assert!(to_display(&spans("****"), &map).iter().all(|s| !s.range.is_empty()));
    }

    #[test]
    fn quote_lines_are_styled_as_quotes() {
        let s = styled("> said **so**");
        assert!(s.iter().all(|(_, st)| st.quote));
        assert_eq!(s[0].0, "> said ");
        assert!(styled("not > a quote").is_empty());
    }

    #[test]
    fn the_cache_restyles_only_changed_lines() {
        let mut c = SpanCache::default();
        let lines = ["# Title", "**bold**", "plain"];
        for l in lines {
            c.get(l);
        }
        assert_eq!(c.misses, 3);
        c.sweep();
        for l in lines {
            c.get(l);
        }
        assert_eq!(c.misses, 3, "nothing changed: nothing restyled");
        c.sweep();
        // one line edited, and a new line typed above the rest (which all move down one)
        for l in ["", "# Title", "**bold** x", "plain"] {
            c.get(l);
        }
        assert_eq!(c.misses, 5, "only the edited and the new line");
        assert_eq!(c.get("# Title").shown, "Title");
        assert_eq!(c.get("# Title").hidden[0].style.heading, 1);
        c.sweep();
        c.sweep();
        c.get("# Title");
        assert_eq!(c.misses, 6, "a line no frame drew is dropped");
    }

    #[test]
    fn spans_are_sorted_non_overlapping_on_char_boundaries() {
        for line in ["é **ü** `ö` ~~ß~~ [é](ü) ==😀==", "# 😀 **é *ü* ö**", "- [ ] ünï `é`"] {
            let all = spans(line);
            assert!(!all.is_empty());
            for w in all.windows(2) {
                assert!(w[0].range.end <= w[1].range.start, "{line}");
            }
            for s in &all {
                assert!(line.get(s.range.clone()).is_some(), "{line}");
            }
        }
    }
}
