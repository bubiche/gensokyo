//! The timetable: every ritual, soonest fire first; one opened shows its detail and runs,
//! pauses, resumes or removes it.

use super::{Key, Modal};
use crate::client::app::App;
use crate::client::render::{DIM, ERROR, Model, Row, Say, ago, mark, width, window, wrap};
use crate::paths::tilde;
use crate::proto::{Request, RitualInfo, RitualVerb};
use ratatui::style::{Color, Style};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timetable {
    /// Into `timetable_order`.
    pub selected: usize,
    /// The ritual whose detail is shown, by name.
    pub open: Option<String>,
    /// Its removal asked, waiting for a yes.
    pub confirm: bool,
}

/// The timetable's order: soonest fire first, then the rest by name. Indices into `rituals`.
pub fn timetable_order(rituals: &[RitualInfo]) -> Vec<usize> {
    let mut v: Vec<usize> = (0..rituals.len()).collect();
    v.sort_by(|&a, &b| {
        let (a, b) = (&rituals[a], &rituals[b]);
        let key = |r: &RitualInfo| (r.next_fire.is_none(), r.next_fire, r.name.clone());
        key(a).cmp(&key(b))
    });
    v
}

/// `YYYY-MM-DD HH:MM` said short: the time alone today, the weekday within the week, else the
/// month and day.
pub fn when_short(local: &str, today: &str) -> String {
    let Some((date, time)) = local.split_once(' ') else { return local.to_string() };
    if date == today {
        return time.to_string();
    }
    let parse = |d: &str| d.parse::<jiff::civil::Date>().ok();
    let days = parse(date).zip(parse(today)).map(|(d, t)| (d - t).get_days());
    match (days, parse(date)) {
        (Some(1..=6), Some(d)) => format!("{} {time}", d.strftime("%a")),
        _ => format!("{} {time}", date.get(5..).unwrap_or(date)),
    }
}

/// The ritual the timetable has open, if it is still there.
pub fn opened<'a>(m: &'a Model, tt: &Timetable) -> Option<&'a RitualInfo> {
    let name = tt.open.as_ref()?;
    m.rituals.iter().flatten().find(|r| &r.name == name)
}

impl Timetable {
    /// `w` is the widest a line can be in the modal: long ones are wrapped to it.
    pub(super) fn view(&self, m: &Model, w: usize) -> (String, Vec<Row>) {
        let Some(rituals) = &m.rituals else {
            return (" timetable ".into(), vec![Row::dim("loading the rituals…"), Row::close()]);
        };
        if let Some(r) = opened(m, self) {
            let mut rows = match self.confirm {
                true => confirm_rows(r, w),
                false => detail(m, r, w),
            };
            rows.push(match self.confirm {
                true => Row::yes_no("[yes y]", "[no n]"),
                false => detail_buttons(r),
            });
            return (format!(" {} ", r.name), rows);
        }
        if rituals.is_empty() {
            let dir = tilde(&crate::paths::config_dir().to_string_lossy(), &m.home);
            let empty = Row::dim(&format!("nothing is scheduled: {dir}/rituals"));
            return (" timetable ".into(), vec![empty, Row::close()]);
        }
        let order = timetable_order(rituals);
        let sel = self.selected.min(order.len() - 1);
        let nw = rituals.iter().map(|r| width(&r.name)).max().unwrap_or(0).min(20) as usize;
        let mut rows: Vec<Row> = window(order.len(), sel, 12)
            .map(|i| {
                let r = &rituals[order[i]];
                let next = match (&r.next_fire_local, r.enabled) {
                    (_, false) => "paused".to_string(),
                    (Some(l), true) => when_short(l, &m.today),
                    (None, true) => "—".into(),
                };
                let on = if r.enabled { "on " } else { "off" };
                let flag = if r.problem.is_some() { "!" } else { " " };
                let desc = r.description.as_deref().unwrap_or("");
                let name: String = match r.name.chars().count() > nw {
                    true => r.name.chars().take(nw - 1).chain(['…']).collect(),
                    false => r.name.clone(),
                };
                let line = format!("{}{name:<nw$} {on} {next:<11}{flag} {desc}", mark(i == sel));
                Row::Item(i, line, i == sel)
            })
            .collect();
        rows.push(Row::close());
        (" timetable ".into(), rows)
    }
}

fn wrapped(s: &str, w: usize, st: Style) -> impl Iterator<Item = Row> {
    wrap(s, w, 3).into_iter().map(move |l| Row::Text(l, st))
}

fn confirm_rows(r: &RitualInfo, w: usize) -> Vec<Row> {
    let mut rows = vec![Row::text(&format!("Remove {}?", r.name))];
    rows.extend(wrapped("Its file, its notes and its journal go, and do not come back.", w, DIM));
    rows
}

fn detail(m: &Model, r: &RitualInfo, w: usize) -> Vec<Row> {
    let field = |k: &str, v: &str| Row::text(&format!("{k:<9} {v}"));
    let mut rows = Vec::new();
    if let Some(d) = &r.description {
        rows.push(Row::text(d));
    }
    let on = if r.enabled { "" } else { "  (paused)" };
    rows.push(field("schedule", &format!("{}{on}", r.schedule)));
    let next = match (&r.next_fire_local, r.enabled) {
        (_, false) => "paused".to_string(),
        (Some(l), true) => l.clone(),
        (None, true) => "never".into(),
    };
    rows.push(field("next", &next));
    let headless = if r.headless { ", headless" } else { "" };
    rows.push(field("target", &format!("{}{headless}", r.target)));
    if let Some(c) = &r.cwd {
        rows.push(field("in", &tilde(c, &m.home)));
    }
    let last = r.last_run.map_or("never".into(), |t| ago(m.now - t) + " ago");
    rows.push(field("last ran", &last));
    if let Some(l) = &r.last {
        rows.extend(wrapped(l, w, DIM));
    }
    if r.running {
        rows.push(Row::Text("a run of it is going now".into(), Style::new().fg(Color::Yellow)));
    }
    if let Some(p) = &r.problem {
        rows.extend(wrapped(p, w, ERROR));
    }
    if r.shipped {
        rows.extend(wrapped("An example gensokyo ships: pause it, or edit a copy.", w, DIM));
    }
    rows
}

fn detail_buttons(r: &RitualInfo) -> Row {
    use crate::client::render::Button;
    let toggle = if r.enabled { "[pause p]" } else { "[resume p]" };
    let mut b = vec![("[run now r]", Button::RunRitual), (toggle, Button::ToggleRitual)];
    if !r.shipped {
        b.push(("[remove x]", Button::RemoveRitual));
    }
    b.push(("[back esc]", Button::No));
    Row::Buttons(b)
}

/// What the timetable's buttons do to the ritual it has open.
pub(in crate::client) enum Act {
    Run,
    /// Pause it, or resume it.
    Toggle,
    Remove,
}

impl App {
    pub(super) fn timetable_key(&mut self, k: Key) {
        let last = self.m.rituals.as_ref().map_or(0, Vec::len).saturating_sub(1);
        let Some(Modal::Timetable(tt)) = &mut self.m.modal else { return };
        match k {
            Key::Enter | Key::Text('y') if tt.confirm => self.confirm(),
            Key::Text('n') if tt.confirm => tt.confirm = false,
            _ if tt.confirm => {}
            Key::Text('r') if tt.open.is_some() => self.ritual(Act::Run),
            Key::Text('p') if tt.open.is_some() => self.ritual(Act::Toggle),
            Key::Text('x') if tt.open.is_some() => self.ritual(Act::Remove),
            _ if tt.open.is_some() => {}
            Key::Up | Key::Text('k') => tt.selected = tt.selected.min(last).saturating_sub(1),
            Key::Down | Key::Text('j') => tt.selected = (tt.selected + 1).min(last),
            Key::Enter => self.confirm(),
            _ => {}
        }
    }

    /// A yes removes the ritual it asked about; Enter on the list opens the ritual picked.
    pub(super) fn timetable_confirm(&mut self, mut tt: Timetable) {
        match (&tt.open, tt.confirm) {
            (Some(name), true) => {
                self.say(Say::Info, format!("removing {name}…"));
                let name = name.clone();
                self.send(Request::Ritual { verb: RitualVerb::Remove, name });
                (tt.open, tt.confirm) = (None, false);
            }
            (None, _) => {
                let list = self.m.rituals.as_deref().unwrap_or_default();
                let order = timetable_order(list);
                let i = order.get(tt.selected.min(order.len().saturating_sub(1)));
                tt.open = i.map(|&i| list[i].name.clone());
            }
            _ => {}
        }
        self.m.modal = Some(Modal::Timetable(tt));
    }

    /// Run, pause or resume, or ask to remove, the ritual the timetable has open.
    pub(in crate::client) fn ritual(&mut self, act: Act) {
        let Some(Modal::Timetable(tt)) = &self.m.modal else { return };
        let Some(r) = opened(&self.m, tt) else { return };
        let (name, enabled, shipped) = (r.name.clone(), r.enabled, r.shipped);
        let (verb, doing) = match act {
            Act::Run => (RitualVerb::Run, "running"),
            Act::Remove if shipped => {
                self.say(Say::Error, format!("{name} ships with gensokyo: pause it instead"));
                return;
            }
            Act::Remove => {
                if let Some(Modal::Timetable(tt)) = &mut self.m.modal {
                    tt.confirm = true;
                }
                return;
            }
            Act::Toggle if enabled => (RitualVerb::Disable, "pausing"),
            Act::Toggle => (RitualVerb::Enable, "resuming"),
        };
        self.say(Say::Info, format!("{doing} {name}…"));
        self.send(Request::Ritual { verb, name });
    }

    /// The daemon's timetable: the selection stays on the ritual it was on, and one opened that
    /// has gone is closed (nothing to act on, or to confirm removing).
    pub(in crate::client) fn rituals(&mut self, rituals: Vec<RitualInfo>) {
        let order = |m: &Model| timetable_order(m.rituals.as_deref().unwrap_or(&[]));
        let was = match &self.m.modal {
            Some(Modal::Timetable(tt)) => order(&self.m)
                .get(tt.selected)
                .and_then(|&i| self.m.rituals.as_ref()?.get(i))
                .map(|r| r.name.clone()),
            _ => None,
        };
        self.m.rituals = Some(rituals);
        let now = order(&self.m);
        let list = self.m.rituals.as_deref().unwrap_or_default();
        let at = was.and_then(|n| now.iter().position(|&i| list[i].name == n));
        if let Some(Modal::Timetable(tt)) = &mut self.m.modal {
            if let Some(at) = at {
                tt.selected = at;
            }
            if tt.open.as_ref().is_some_and(|n| !list.iter().any(|r| &r.name == n)) {
                (tt.open, tt.confirm) = (None, false);
            }
        }
    }
}
