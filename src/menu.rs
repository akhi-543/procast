//! The one dropdown menu (`.pc-pop`): entries, an optional search field, sections and rules.
//! Pure state: hosts (launcher, Settings, Notes) feed it keys and map the `Event` it returns.

/// One row. `section` groups consecutive entries under a header; `rule_after` draws a rule
/// below it; `off` shows it faint and skips it; `two_line` adds a second line under the label;
/// `always` shows it whatever the search says (a row made from the typed text).
#[derive(Clone, Default)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub icon: Option<&'static str>,
    pub section: Option<String>,
    /// Right-aligned faint text.
    pub sub: String,
    pub rule_after: bool,
    pub off: bool,
    pub two_line: Option<String>,
    pub always: bool,
}

/// Where the menu sits in its window.
pub enum Place {
    /// `POP_RIGHT_W` wide, `sp::S12` from the right, `POP_RIGHT_TOP` down: under the bar's Dropdown.
    Right,
    /// `POP_RIGHT_W` wide, `POP_RIGHT_TOP` down, its left edge here: under a list argument box.
    Under(gpui::Pixels),
    /// `POP_RIGHT_W` wide, its top-left corner here: under a form field.
    At(gpui::Point<gpui::Pixels>),
}

pub struct Menu {
    pub entries: Vec<Entry>,
    /// The search field's placeholder; None = no field.
    pub search: Option<String>,
    pub filter: String,
    /// Index into `entries`.
    pub selected: usize,
    pub place: Place,
}

/// A key the host passes on while the menu is open.
pub enum Key {
    Up,
    Down,
    Enter,
    Esc,
}

#[derive(Debug, PartialEq)]
pub enum Event {
    /// The entry at this index of `entries` was chosen; the host closes the menu.
    Picked(usize),
    /// Esc: close, nothing chosen.
    Closed,
    None,
}

/// What the menu draws, top to bottom.
#[derive(Debug, PartialEq)]
pub enum Row<'a> {
    Header(&'a str),
    Item(usize),
    Rule,
}

impl Menu {
    /// Selects the first entry that can be picked.
    pub fn new(entries: Vec<Entry>, search: Option<String>, place: Place) -> Self {
        let mut m = Menu { entries, search, filter: String::new(), selected: 0, place };
        m.reselect();
        m
    }

    /// Entries whose label contains the filter, ignoring case.
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.to_lowercase();
        (0..self.entries.len()).filter(|&i| self.entries[i].always || self.entries[i].label.to_lowercase().contains(&f)).collect()
    }

    /// The visible entries with a header before each section that still has one, and a rule
    /// after an entry that asks for one unless it is last.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let vis = self.visible();
        let mut out = vec![];
        let mut sec = None;
        for (k, &i) in vis.iter().enumerate() {
            let e = &self.entries[i];
            if e.section.as_deref() != sec {
                sec = e.section.as_deref();
                out.extend(sec.map(Row::Header));
            }
            out.push(Row::Item(i));
            if e.rule_after && k + 1 < vis.len() {
                out.push(Row::Rule);
            }
        }
        out
    }

    /// Where the selected entry is in `rows`, to scroll it into view.
    pub fn selected_row(&self) -> Option<usize> {
        self.rows().iter().position(|r| *r == Row::Item(self.selected))
    }

    fn pickable(&self) -> Vec<usize> {
        self.visible().into_iter().filter(|&i| !self.entries[i].off).collect()
    }

    /// Moves `d` over the visible entries, skipping `off` ones, wrapping at either end.
    pub fn step(&mut self, d: isize) {
        let p = self.pickable();
        if p.is_empty() {
            return;
        }
        let n = p.len() as isize;
        let k = match p.iter().position(|&i| i == self.selected) {
            Some(k) => (k as isize + d).rem_euclid(n),
            None if d < 0 => n - 1,
            None => 0,
        };
        self.selected = p[k as usize];
    }

    /// The selected entry, if it is shown and can be picked.
    pub fn picked(&self) -> Option<&Entry> {
        self.pickable().contains(&self.selected).then(|| &self.entries[self.selected])
    }

    /// New search text; the selection moves to the first entry that shows. The same text
    /// again changes nothing, so a field cleared on open keeps the opener's pre-selection.
    pub fn set_filter(&mut self, s: &str) {
        if self.filter != s {
            self.filter = s.to_string();
            self.reselect();
        }
    }

    /// New entries (the opener's list changed while open); the selection moves to the first that shows.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        self.reselect();
    }

    fn reselect(&mut self) {
        if let Some(&i) = self.pickable().first() {
            self.selected = i;
        }
    }

    /// The one key routing every host shares.
    pub fn key(&mut self, k: Key) -> Event {
        match k {
            Key::Up => self.step(-1),
            Key::Down => self.step(1),
            Key::Enter => return self.picked().map_or(Event::None, |_| Event::Picked(self.selected)),
            Key::Esc => return Event::Closed,
        }
        Event::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(label: &str, section: Option<&str>) -> Entry {
        Entry { id: label.to_lowercase(), label: label.into(), section: section.map(Into::into), ..Default::default() }
    }

    fn menu(entries: Vec<Entry>) -> Menu {
        Menu::new(entries, Some("Search…".into()), Place::Right)
    }

    #[test]
    fn visible_filters_by_label_and_keeps_a_section_only_when_a_child_matches() {
        let mut m = menu(vec![e("Manage Sets…", None), e("Messaging", Some("Focus Categories")), e("Social", Some("Focus Categories")), e("Slack", Some("Applications"))]);
        m.entries[0].rule_after = true;
        assert_eq!(m.rows(), [Row::Item(0), Row::Rule, Row::Header("Focus Categories"), Row::Item(1), Row::Item(2), Row::Header("Applications"), Row::Item(3)]);
        m.set_filter("SL");
        assert_eq!(m.visible(), [3]);
        assert_eq!(m.rows(), [Row::Header("Applications"), Row::Item(3)], "no Focus Categories header, no rule after a hidden entry");
        m.set_filter("s");
        assert_eq!(m.visible(), [0, 1, 2, 3]);
        m.set_filter("soc");
        assert_eq!(m.rows(), [Row::Header("Focus Categories"), Row::Item(2)]);
        assert_eq!(m.selected_row(), Some(1), "under its header");
    }

    #[test]
    fn step_skips_off_entries_and_wraps() {
        let mut m = menu(vec![e("A", None), e("B", None), e("C", None)]);
        m.entries[1].off = true;
        assert_eq!(m.selected, 0);
        m.step(1);
        assert_eq!(m.selected, 2, "B is off");
        m.step(1);
        assert_eq!(m.selected, 0, "wraps to the top");
        m.step(-1);
        assert_eq!(m.selected, 2, "wraps to the bottom");
        assert_eq!(m.key(Key::Up), Event::None);
        assert_eq!(m.selected, 0);
        assert_eq!(m.key(Key::Enter), Event::Picked(0));
        assert_eq!(m.key(Key::Esc), Event::Closed);
    }

    #[test]
    fn set_filter_reselects_the_first_match() {
        let mut m = menu(vec![e("English", None), e("German", None), e("Greek", None)]);
        m.step(1);
        m.step(1);
        assert_eq!(m.selected, 2);
        m.set_filter("ger");
        assert_eq!(m.selected, 1);
        assert_eq!(m.picked().map(|e| e.id.as_str()), Some("german"));
    }

    #[test]
    fn unchanged_filter_keeps_the_preselection() {
        let mut m = menu(vec![e("All", None), e("Local", None), e("Network", None)]);
        m.selected = 2;
        m.set_filter("");
        assert_eq!(m.picked().map(|e| e.id.as_str()), Some("network"), "clearing an empty field on open");
        m.set_filter("o");
        assert_eq!(m.selected, 1, "real typing still reselects");
    }

    #[test]
    fn an_always_entry_shows_whatever_is_typed_and_new_entries_reselect() {
        let mut m = menu(vec![e("Messaging", None), e("Social", None)]);
        m.set_filter("x.com");
        assert!(m.visible().is_empty());
        m.entries.insert(0, Entry { always: true, ..e("Block x.com", None) });
        m.set_filter("https://x.com");
        assert_eq!((m.visible(), m.selected), (vec![0], 0), "shown and selected though its label differs");
        m.set_filter("");
        m.step(1);
        m.set_entries(vec![e("Social", None)]);
        assert_eq!(m.picked().map(|e| e.id.as_str()), Some("social"), "never left on a stale index");
    }

    #[test]
    fn picked_is_none_when_nothing_matches() {
        let mut m = menu(vec![e("English", None), e("German", None)]);
        m.set_filter("zz");
        assert!(m.visible().is_empty());
        assert!(m.picked().is_none());
        assert_eq!(m.key(Key::Enter), Event::None);
        m.step(1);
        assert!(m.picked().is_none());
    }
}
