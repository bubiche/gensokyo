//! The history: every message the client said since it came, and the daemon's notices from
//! before, newest first, each whole and with how long ago it was.

use super::{Key, Modal};
use crate::client::app::App;
use crate::client::framer::Chunk;
use crate::client::keys::{self, Scrollback};
use crate::client::render::{ERROR, Model, Row, Say, ago, wrap};
use ratatui::style::{Color, Style};

/// The most rows of it shown at once.
const PAGE: usize = 20;

/// Before each message: how long ago, so wide.
const AGO_W: usize = 9;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    /// The first row shown.
    pub top: usize,
}

/// Every message's rows, wrapped to `w`: how long ago, then its text, the rest under it. Each
/// is as wide as there is room for, so the box keeps its width as it scrolls.
fn lines(m: &Model, w: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for (at, msg) in &m.history {
        let style = match msg.kind {
            Say::Info => Style::new(),
            Say::Notice => Style::new().fg(Color::Yellow),
            Say::Error => ERROR,
        };
        let mut when = format!("{:>4} ago", ago(m.now - at));
        for l in wrap(&msg.text, w.saturating_sub(AGO_W).max(1), usize::MAX) {
            rows.push(Row::Text(format!("{:<w$}", format!("{when:<AGO_W$}{l}")), style));
            when.clear();
        }
    }
    rows
}

/// Rows of it that fit in `h`, under the box's edges and above its button.
fn page(h: usize) -> usize {
    h.saturating_sub(3).clamp(1, PAGE)
}

impl History {
    pub(super) fn view(&self, m: &Model, w: usize, h: usize) -> (String, Vec<Row>) {
        let all = lines(m, w);
        if all.is_empty() {
            return (" history ".into(), vec![Row::dim("nothing said yet"), Row::close()]);
        }
        let (n, page) = (all.len(), page(h));
        let top = self.top.min(n.saturating_sub(page));
        let mut rows: Vec<Row> = all.into_iter().skip(top).take(page).collect();
        let title = match n > page {
            true => format!(" history {}–{} of {n} ", top + 1, top + rows.len()),
            false => " history ".into(),
        };
        rows.push(Row::close());
        (title, rows)
    }
}

impl App {
    /// Rows and pages as the scrollback takes them; `q` or `h` closes it.
    pub(super) fn history_key(&mut self, c: &Chunk) {
        // As `render` lays it out for the grid last drawn.
        let w = self.size.0.min(72).saturating_sub(4) as usize;
        let (n, page) = (lines(&self.m, w).len(), page(self.size.1 as usize));
        let last = n.saturating_sub(page);
        if matches!(Key::of(c), Key::Text('q' | 'h')) {
            self.m.modal = None;
            return;
        }
        let Some(Modal::History(hs)) = &mut self.m.modal else { return };
        let by = |rows: i32| hs.top.min(last).saturating_add_signed(rows as isize);
        hs.top = match keys::scrollback(c) {
            Some(Scrollback::By(r)) => by(r),
            Some(Scrollback::Pages(p)) => by(p * page as i32),
            Some(Scrollback::Top) => 0,
            Some(Scrollback::Live) => last,
            _ => return,
        }
        .min(last);
    }
}
