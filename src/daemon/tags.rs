//! Tags: a few `key=value` tokens on a resident's record, shown under it in the sidebar and in
//! `list`. Only tokens, as a branch ritual's facts are, so a lead reading another resident's
//! tags reads no free text. The user tags anyone; a resident, itself and its helpers.

use super::shrine::{Shared, Shrine, find, touch};
use crate::proto::Tags;

/// The most a resident has.
const MOST: usize = 4;

fn key_ok(k: &str) -> bool {
    let ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-".contains(c);
    !k.is_empty() && k.len() <= 12 && k.chars().all(ok)
}

/// `tags` with `set` applied, each `key=value` or `key=` to remove one, after all go when
/// `clear`; or why not, changing nothing. A key set again keeps its place; a new one goes last.
pub(super) fn apply(tags: &mut Tags, set: &[String], clear: bool) -> Result<(), String> {
    let mut t = if clear { Tags::new() } else { tags.clone() };
    for s in set {
        let (k, v) = s.split_once('=').ok_or_else(|| format!("{s}: a tag is key=value"))?;
        if !key_ok(k) {
            return Err(format!("{k:?} is not a key: a key is up to 12 of a-z, 0-9, _ and -"));
        }
        if !v.is_empty() && !super::branch::token(v, "_.:/#@-", 24) {
            return Err(format!(
                "{k}'s value is not a token: up to 24 letters, digits and _.:/#@-"
            ));
        }
        let at = t.iter().position(|(have, _)| have == k);
        match (at, v.is_empty()) {
            (Some(i), true) => _ = t.remove(i),
            (Some(i), false) => t[i].1 = v.into(),
            (None, true) => {}
            (None, false) => t.push((k.into(), v.into())),
        }
    }
    if t.len() > MOST {
        return Err(format!("a resident has at most {MOST} tags; remove one with key="));
    }
    *tags = t;
    Ok(())
}

/// What a `tag` answers: the resident's tags as they are now.
fn said(name: &str, t: &Tags) -> String {
    match t.is_empty() {
        true => format!("{name} has no tags"),
        false => {
            let t: Vec<String> = t.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!("{name}: {}", t.join(" "))
        }
    }
}

/// Resident `who`'s tags, changed first when asked; one departed out of the shrine too.
pub(super) fn tag(
    shrine: &Shared,
    who: &str,
    set: &[String],
    clear: bool,
) -> Result<String, String> {
    let mut sh = shrine.borrow_mut();
    let change = clear || !set.is_empty();
    if let Some(i) = find(&sh, who) {
        if change {
            let mut rec = sh.entries[i].rec.clone();
            apply(&mut rec.tags, set, clear)?;
            sh.store.save(&rec).map_err(|e| format!("could not keep its tags: {e}"))?;
            sh.entries[i].rec.tags = rec.tags;
            touch(&sh);
        }
        let rec = &sh.entries[i].rec;
        return Ok(said(&rec.name, &rec.tags));
    }
    let mut rec = sh.store.find_departed(who).ok_or_else(|| format!("no resident {who}"))?;
    if change {
        apply(&mut rec.tags, set, clear)?;
        sh.store.save_departed(&rec).map_err(|e| format!("could not keep its tags: {e}"))?;
    }
    Ok(said(&rec.name, &rec.tags))
}

/// Whether resident `caller` may change `who`'s tags: its own, or a helper's it leads.
pub(super) fn may(sh: &Shrine, caller: &str, who: &str) -> Result<(), String> {
    match super::lead::record(sh, who) {
        Some(r) if r.id == caller || r.owner.as_deref() == Some(caller) => Ok(()),
        Some(r) => {
            Err(format!("{} is not you or one of your helpers; only the user can tag it", r.name))
        }
        None => Err(format!("no resident {who}")),
    }
}
