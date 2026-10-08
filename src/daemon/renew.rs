//! Renew: a live resident started again on the claude installed now, into its own conversation,
//! in place (its id, slot, name, lead and screen stay). It waits until the resident rests, so no
//! turn, dialog or half-typed prompt is cut short; what only lives in the session (background
//! tasks, a Monitor, a `/loop`) goes with the old process.

use super::log::log;
use super::resident::Handle;
use super::shrine::{EXIT_WAIT, Entry, Shared, Shrine, come_back, departed, find, stir, touch};
use serde_json::json;
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Renew {
    /// Waiting for it to rest.
    Asked,
    /// `/exit` typed in: its exit is the renew's to see to.
    Going,
}

/// How long it must have rested, with no hook, before it goes: the user may be reading the turn
/// that just ended, or about to answer it.
const QUIET: Duration = Duration::from_secs(2);

/// How often a resident that is not resting is looked at again with nothing else changing: a
/// draft or a card's mark wears off without a word to the shrine.
const LOOK: Duration = Duration::from_secs(2);

/// `/exit`s that may come to nothing before the renew gives up.
const TRIES: u32 = 3;

/// The claude version a renew starts residents on, when known.
pub(super) fn installed(sh: &Shrine) -> Option<&str> {
    sh.installed.as_ref().and_then(|i| i.version.as_deref())
}

/// Live and running another claude than `installed`, by its status line (an older one, or a
/// newer one rolled back from), and not a ritual run kept only until it has rested a while,
/// which is left to end on its own: the marker, and whom a renew with nobody named takes.
pub(super) fn behind(e: &Entry, installed: Option<&str>) -> bool {
    let runs = e.tele.as_ref().and_then(|t| t.version.as_deref());
    let brief = e.rec.ritual.is_some() && e.rec.keep.is_some();
    e.handle.is_some() && !brief && installed.is_some_and(|i| runs.is_some_and(|v| v != i))
}

/// Whether `e` runs as `h`: not since departed, recalled or renewed.
fn runs_as(e: &Entry, h: &Rc<Handle>) -> bool {
    e.handle.as_ref().is_some_and(|x| Rc::ptr_eq(x, h))
}

/// `who` renewed, each as it rests; with nobody named, everyone behind the installed claude
/// but brief ritual runs. Says who goes now, who once they rest, and who was already going.
pub(super) fn ask(shrine: &Shared, who: &[String]) -> Result<String, String> {
    let mut sh = shrine.borrow_mut();
    if sh.quitting {
        return Err("the daemon is stopping".into());
    }
    let picked: Vec<usize> = match who.is_empty() {
        true => (0..sh.entries.len()).filter(|&i| behind(&sh.entries[i], installed(&sh))).collect(),
        false => {
            let mut v = Vec::new();
            for w in who {
                let i = find(&sh, w).ok_or_else(|| format!("no resident {w}"))?;
                let e = &sh.entries[i];
                if e.handle.as_ref().is_none_or(|h| h.exit().is_some()) {
                    return Err(format!(
                        "{} has departed: a recall starts it on the claude installed now",
                        e.rec.name
                    ));
                }
                if !v.contains(&i) {
                    v.push(i);
                }
            }
            v
        }
    };
    if picked.is_empty() {
        return Ok(match installed(&sh) {
            Some(v) => format!("nobody here is behind claude {v}"),
            None => "the installed claude's version is not known yet: name whom to renew".into(),
        });
    }
    let (mut now, mut later, mut already) = (Vec::new(), Vec::new(), Vec::new());
    for i in picked {
        let e = &mut sh.entries[i];
        let name = e.rec.name.clone();
        let Some(h) = e.handle.clone() else { continue };
        if e.renew.is_some() {
            already.push(name);
            continue;
        }
        e.renew = Some(Renew::Asked);
        match e.aware.idle() && e.aware.dialog().is_none() {
            true => now.push(name),
            false => later.push(name),
        }
        log(json!({"ev": "renew", "id": e.rec.id, "name": e.rec.name}));
        tokio::task::spawn_local(renewal(shrine.clone(), e.rec.id.clone(), h));
    }
    touch(&sh);
    let mut said = Vec::new();
    if !now.is_empty() {
        said.push(format!("renewing {}", now.join(", ")));
    }
    if !later.is_empty() {
        let lead = if now.is_empty() { "renewing " } else { "" };
        said.push(format!("{lead}{} once they rest", later.join(", ")));
    }
    if !already.is_empty() {
        said.push(format!("{} already on the way", already.join(", ")));
    }
    Ok(said.join("; "))
}

/// Waits for `id`, running as `h`, to rest, then starts it again. Given up when it leaves, is
/// closed, banished or recalled, or the daemon stops meanwhile, and after `TRIES` `/exit`s that
/// came to nothing: a turn began each time.
async fn renewal(shrine: Shared, id: String, h: Rc<Handle>) {
    let mut tries = 0;
    while rested(&shrine, &id, &h).await {
        match go(&shrine, &id, &h).await {
            Went::Done => return,
            Went::NotYet => {}
            Went::Again => tries += 1,
        }
        if tries >= TRIES {
            let mut sh = shrine.borrow_mut();
            if let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == id && runs_as(e, &h)) {
                let name = e.rec.name.clone();
                e.renew = None;
                log(json!({"ev": "renew", "id": id, "gave_up": tries}));
                sh.notice(&format!(
                    "{name} was not renewed: it began a turn each time it was asked to /exit; \
                     renew it again once it rests"
                ));
                touch(&sh);
            }
            return;
        }
    }
    let mut sh = shrine.borrow_mut();
    let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == id && e.renew.is_some()) else {
        return;
    };
    // Departed with its renew still asked for: after an `/exit` that seemed to come to nothing,
    // it left late, and nothing started it again.
    let left = e.handle.is_none();
    if left || runs_as(e, &h) {
        e.renew = None;
        let name = e.rec.name.clone();
        if left && tries > 0 {
            sh.notice(&format!(
                "{name} left late for its renew, and was not started again: `gensokyo resume \
                 {name}` brings it back"
            ));
        }
        touch(&sh);
    }
}

/// Resting and settled for `QUIET` with no hook. False once it no longer runs as `h` or is no
/// longer to be renewed, or the daemon is stopping.
async fn rested(shrine: &Shared, id: &str, h: &Rc<Handle>) -> bool {
    let mut changed = shrine.borrow().changed.subscribe();
    loop {
        changed.borrow_and_update();
        let Some(heard) = still(shrine, id, h) else { return false };
        if let Some(heard) = heard {
            tokio::time::sleep(QUIET).await;
            match still(shrine, id, h) {
                None => return false,
                Some(Some(t)) if t == heard => return true,
                Some(_) => continue,
            }
        }
        let _ = tokio::time::timeout(LOOK, changed.changed()).await;
    }
}

/// None when the renew is off; else when it was last heard from, if it rests now.
fn still(shrine: &Shared, id: &str, h: &Rc<Handle>) -> Option<Option<i64>> {
    let sh = shrine.borrow();
    let e = sh.entries.iter().find(|e| e.rec.id == id)?;
    if sh.quitting || !runs_as(e, h) || h.exit().is_some() || e.renew != Some(Renew::Asked) {
        return None;
    }
    Some((e.aware.idle() && e.aware.dialog().is_none()).then(|| e.aware.heard()))
}

enum Went {
    Done,
    /// Not resting after all: wait for it again.
    NotYet,
    /// The `/exit` came to nothing.
    Again,
}

/// `/exit`, then the same start a recall makes, in place. A close or a banish meanwhile takes
/// it over: it leaves, and stays departed.
async fn go(shrine: &Shared, id: &str, h: &Rc<Handle>) -> Went {
    let mark = {
        let mut sh = shrine.borrow_mut();
        let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == id) else { return Went::Done };
        if !runs_as(e, h) || h.exit().is_some() || e.renew != Some(Renew::Asked) {
            return Went::Done;
        }
        if !(e.aware.idle() && e.aware.dialog().is_none()) {
            return Went::NotYet;
        }
        // Taken, so no card or ritual prompt goes in meanwhile.
        let mark = e.aware.mark();
        e.renew = Some(Renew::Going);
        log(json!({"ev": "renew", "id": id, "pid": h.pid, "going": true}));
        touch(&sh);
        mark
    };
    // Bounded as a whole: a resident that stopped reading its tty blocks the keystrokes too.
    let most = LEAVE_WAIT + Duration::from_secs(3);
    let _ = tokio::time::timeout(most, exit(shrine, h)).await;
    let mut sh = shrine.borrow_mut();
    // Gone from the shrine, or departed by `watch_exit` after a close took it over.
    let Some(i) = sh.entries.iter().position(|e| e.rec.id == id && runs_as(e, h)) else {
        return Went::Done;
    };
    let going = sh.entries[i].renew == Some(Renew::Going);
    match (h.exit(), going) {
        (Some(_), true) => {
            relaunch(shrine, &mut sh, i, h);
            Went::Done
        }
        // Closed or banished while going: it left as they asked, which `watch_exit` left to us.
        (Some(x), false) => {
            departed(&mut sh, i, x);
            Went::Done
        }
        (None, going) => {
            let e = &mut sh.entries[i];
            e.aware.unmark(mark);
            log(json!({"ev": "renew", "id": id, "left": false}));
            if !going {
                return Went::Done;
            }
            e.renew = Some(Renew::Asked);
            touch(&sh);
            Went::Again
        }
    }
}

/// How long an `/exit` that went in may take to end the process: two of close's waits, for
/// Claude Code's own SessionEnd hooks.
const LEAVE_WAIT: Duration = Duration::from_secs(2 * EXIT_WAIT.as_secs());

/// `/exit` typed in and, once nothing has stirred, Enter: no Esc or Ctrl-C first, which would
/// deny a dialog or cut short a turn that began meanwhile. When one did, or the user began
/// typing, the `/exit` is erased.
async fn exit(shrine: &Shared, h: &Handle) {
    let calm = (stir(shrine, h), drafted(shrine, h));
    h.input(b"/exit").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    if (stir(shrine, h), drafted(shrine, h)) != calm {
        h.input(&[0x7f; 5]).await;
        return;
    }
    h.input(b"\r").await;
    let _ = tokio::time::timeout(LEAVE_WAIT, h.exited()).await;
}

/// Something in the way of an Enter on the resident at `h`: a dialog, or what the user typed.
fn drafted(shrine: &Shared, h: &Handle) -> bool {
    let sh = shrine.borrow();
    let e = sh.entries.iter().find(|e| e.handle.as_ref().is_some_and(|x| x.pid == h.pid));
    e.is_some_and(|e| e.aware.in_way().is_some())
}

/// The one at `i`, which has left for its renew, started again in place; departed as any other
/// that left when that fails, or when the daemon is stopping (which retires it).
fn relaunch(shrine: &Shared, sh: &mut Shrine, i: usize, old: &Rc<Handle>) {
    let exit = old.exit().expect("it has left");
    if sh.quitting {
        sh.entries[i].renew = None;
        return;
    }
    // Its last news counted before the new start forgets it.
    super::ingest::tally(sh, i, None);
    let rec = sh.entries[i].rec.clone();
    let mode = sh.entries[i].aware.mode.clone();
    match come_back(shrine, sh, &rec, mode) {
        Ok((program, argv, handle, resumed)) => {
            let e = &mut sh.entries[i];
            log(json!({"ev": "renewed", "id": rec.id, "name": rec.name, "pid": handle.pid,
                       "resumed": resumed}));
            e.rec.pid = super::shrine::running_as(&handle);
            (e.rec.program, e.rec.argv, e.rec.launched) = (program, argv, super::store::now());
            e.aware = e.aware.renewed(resumed);
            e.handle = Some(handle);
            e.renew = None;
            // Until it reports, it is not known to be behind.
            if let Some(t) = e.tele.as_mut() {
                t.version = None;
            }
            let rec = e.rec.clone();
            let _ = sh.store.save(&rec);
            touch(sh);
        }
        Err(err) => {
            sh.entries[i].renew = None;
            departed(sh, i, exit);
            let name = &rec.name;
            log(json!({"ev": "renewed", "id": rec.id, "error": err}));
            sh.notice(&format!(
                "{name} left to be renewed and could not come back: {err}; `gensokyo resume \
                 {name}` tries again"
            ));
        }
    }
}
