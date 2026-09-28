//! Cast: a spell card, then who gets it, then, for a pair card, whom its target talks to.

use super::{Key, Modal};
use crate::client::app::App;
use crate::client::render::{Model, Row, mark, window};
use crate::paths::tilde;
use crate::proto::{self, Card, Request};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cast {
    /// None until the daemon has said.
    pub cards: Option<Vec<Card>>,
    /// Card files left out for their names.
    pub unusable: Vec<String>,
    /// The card picked, then the target picked (a group or a resident's id).
    pub card: Option<Card>,
    pub target: Option<String>,
    pub selected: usize,
}

/// The cast modal's choices where it stands: what each sends, and its line. Residents are read
/// from the model as it is now, so one that left meanwhile is simply not offered.
pub fn cast_choices(m: &Model, c: &Cast) -> Vec<(String, String)> {
    let Some(card) = &c.card else {
        let cards = c.cards.iter().flatten();
        let pair = |k: &Card| if k.pair { "  (pair)" } else { "" };
        return cards.map(|k| (k.slug.clone(), format!("{}{}", k.title, pair(k)))).collect();
    };
    let groups =
        [("all", "everyone"), ("awaiting", "everyone who needs you"), ("idle", "everyone resting")];
    let groups = groups.iter().filter(|_| !card.pair && c.target.is_none());
    let live = m.residents.iter().filter(|r| r.departed.is_none());
    let live = live.filter(|r| c.target.as_ref() != Some(&r.id)).map(|r| {
        let slot = r.slot.map_or("-".into(), |s| s.to_string());
        let line = format!("{slot} {} {:<12} {}", r.state.glyph(), r.name, tilde(&r.cwd, &m.home));
        (r.id.clone(), line)
    });
    groups.map(|(v, l)| (v.to_string(), l.to_string())).chain(live).collect()
}

impl Cast {
    pub(super) fn view(&self, m: &Model) -> (String, Vec<Row>) {
        if self.cards.as_ref().is_none_or(Vec::is_empty) {
            let dir = tilde(&crate::paths::config_dir().to_string_lossy(), &m.home);
            let say = match self.cards {
                None => "loading the spell cards…".into(),
                Some(_) => format!("no spell cards: put one in {dir}/spellcards"),
            };
            return (" cast ".into(), vec![Row::dim(&say), Row::close()]);
        }
        let choices = cast_choices(m, self);
        let sel = self.selected.min(choices.len().saturating_sub(1));
        let (title, head) = match (&self.card, &self.target) {
            (None, _) => (" cast ".to_string(), "Which spell card?".to_string()),
            (Some(k), None) => (format!(" {} ", k.title), "On whom?".into()),
            (Some(k), Some(t)) => {
                let who = m.residents.iter().find(|r| &r.id == t).map_or(t.as_str(), |r| &r.name);
                (format!(" {} ", k.title), format!("{who} talks to whom?"))
            }
        };
        let mut rows = vec![Row::text(&head)];
        if choices.is_empty() {
            rows.push(Row::dim("Nobody is here to cast at."));
        }
        rows.extend(
            window(choices.len(), sel, 10)
                .map(|i| Row::Item(i, format!("{}{}", mark(i == sel), choices[i].1), i == sel)),
        );
        match self.cards.iter().flatten().nth(sel).filter(|_| self.card.is_none()) {
            Some(k) if !k.summary.is_empty() => rows.push(Row::dim(&k.summary)),
            _ => {}
        }
        if self.card.is_none() && !self.unusable.is_empty() {
            let names: Vec<_> =
                self.unusable.iter().map(|p| p.rsplit('/').next().unwrap_or(p)).collect();
            rows.push(Row::dim(&format!("file names not usable: {}", names.join(" "))));
        }
        rows.push(match &self.card {
            Some(k) if !k.pair || self.target.is_some() => Row::yes_no("[cast ⏎]", "[cancel esc]"),
            _ => Row::yes_no("[next ⏎]", "[cancel esc]"),
        });
        (title, rows)
    }
}

impl App {
    pub(super) fn cast_key(&mut self, k: Key) {
        let Some(Modal::Cast(c)) = &self.m.modal else { return };
        let last = cast_choices(&self.m, c).len().saturating_sub(1);
        let Some(Modal::Cast(c)) = &mut self.m.modal else { return };
        match k {
            Key::Up | Key::Text('k') => c.selected = c.selected.min(last).saturating_sub(1),
            Key::Down | Key::Text('j') => c.selected = (c.selected + 1).min(last),
            Key::Enter => self.confirm(),
            _ => {}
        }
    }

    /// The card picked moves on to its target, a pair card's target to its peer, and the last
    /// pick casts.
    pub(super) fn cast_confirm(&mut self, mut c: Cast) {
        let choices = cast_choices(&self.m, &c);
        let Some((pick, _)) = choices.get(c.selected.min(choices.len().saturating_sub(1))) else {
            self.m.modal = Some(Modal::Cast(c));
            return;
        };
        let pick = pick.clone();
        let card = match &c.card {
            None => {
                c.card = c.cards.iter().flatten().find(|k| k.slug == pick).cloned();
                c.selected = 0;
                self.m.modal = Some(Modal::Cast(c));
                return;
            }
            Some(k) if k.pair && c.target.is_none() => {
                (c.target, c.selected) = (Some(pick), 0);
                self.m.modal = Some(Modal::Cast(c));
                return;
            }
            Some(k) => k.clone(),
        };
        let (targets, peer) = match c.target {
            Some(t) => (vec![t], Some(pick)),
            None => (vec![pick], None),
        };
        self.send(Request::Cast(proto::Cast { card: card.slug, targets, peer }));
        self.m.message = Some(format!("casting {}…", card.title));
    }
}
