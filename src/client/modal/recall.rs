//! Recall: the departed, newest first, one of them brought back.

use super::Key;
use crate::client::render::{Model, Row, ago, mark, window};
use crate::paths::tilde;
use crate::proto::Resident;

#[derive(Clone, Debug, Default)]
pub struct Recall {
    pub list: Vec<Resident>,
    pub selected: usize,
}

impl Recall {
    pub(super) fn view(&self, m: &Model) -> (String, Vec<Row>) {
        if self.list.is_empty() {
            return (" recall ".into(), vec![Row::dim("Nobody has departed."), Row::close()]);
        }
        let mut rows: Vec<Row> = window(self.list.len(), self.selected, 12)
            .map(|i| {
                let r = &self.list[i];
                let when = r.departed.map_or(String::new(), |d| ago(m.now - d) + " ago");
                let home = tilde(&r.cwd, &m.home);
                let sel = i == self.selected;
                Row::Item(i, format!("{}{:<12} {:>7}  {home}", mark(sel), r.name, when), sel)
            })
            .collect();
        rows.push(Row::yes_no("[recall ⏎]", "[cancel esc]"));
        (" recall ".into(), rows)
    }

    /// A key; true for Enter.
    pub(super) fn key(&mut self, k: Key) -> bool {
        match k {
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(self.list.len().saturating_sub(1)),
            Key::Enter => return true,
            _ => {}
        }
        false
    }

    /// The departed as the daemon now lists them, the selection kept in range.
    pub(super) fn refresh(&mut self, departed: Vec<Resident>) {
        self.selected = self.selected.min(departed.len().saturating_sub(1));
        self.list = departed;
    }
}
