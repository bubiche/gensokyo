//! A ritual's schedule: five cron fields in local time, the usual `@` shorthands, or
//! `every 30m`, read by croner. Every time here is epoch seconds, and the time zone is passed
//! in, so a test can run a week of ticks in any zone without touching `TZ`.

use croner::Cron;
use croner::parser::CronParser;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use std::collections::HashMap;
use std::sync::Mutex;

/// A fire this recent is due; an older one is a miss, run only as a catch-up.
pub const DUE_WINDOW: i64 = 120;

/// A miss older than this is not caught up.
pub const CATCH_UP: i64 = 7 * 86400;

pub struct Schedule {
    cron: Cron,
    /// No fire at all (30 February): croner learns that only by searching to the year 5000,
    /// about 0.1 s, so it is learnt once per spec and every search skipped after.
    never: bool,
}

/// Whether each spec seen so far never comes round.
static NEVER: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

impl Schedule {
    pub fn parse(spec: &str) -> Result<Schedule, String> {
        let five = desugar(spec.trim()).ok_or_else(|| {
            format!(
                "{spec} is not a schedule gensokyo can read (five cron fields, @hourly @daily \
                 @weekly @monthly @yearly, or \"every 30m\" with a length that divides the hour)"
            )
        })?;
        let cron = CronParser::builder()
            .build()
            .parse(&five)
            .map_err(|e| format!("{spec} is not a schedule gensokyo can read ({e})"))?;
        let seen = || NEVER.lock().unwrap_or_else(|e| e.into_inner());
        let known = seen().as_ref().and_then(|m| m.get(&five).copied());
        // Searched outside the lock, which another parse may want meanwhile. A pattern's fires
        // repeat within 28 years, so one start answers for every start.
        let never = known.unwrap_or_else(|| {
            let from = Timestamp::UNIX_EPOCH.to_zoned(TimeZone::UTC);
            let never = cron.find_next_occurrence(&from, false).is_err();
            seen().get_or_insert_with(HashMap::new).insert(five, never);
            never
        });
        Ok(Schedule { cron, never })
    }

    /// The newest fire at or before `t`.
    pub fn latest(&self, t: i64, tz: &TimeZone) -> Option<i64> {
        if self.never {
            return None;
        }
        let z = Timestamp::from_second(t).ok()?.to_zoned(tz.clone());
        self.cron.find_previous_occurrence(&z, true).ok().map(|z| z.timestamp().as_second())
    }

    /// The first fire after `t`.
    pub fn next(&self, t: i64, tz: &TimeZone) -> Option<i64> {
        if self.never {
            return None;
        }
        let z = Timestamp::from_second(t).ok()?.to_zoned(tz.clone());
        self.cron.find_next_occurrence(&z, false).ok().map(|z| z.timestamp().as_second())
    }

    /// Every fire after `from` up to and including `to`, as croner iterates them.
    pub fn between(&self, from: i64, to: i64, tz: &TimeZone) -> Vec<i64> {
        if self.never {
            return Vec::new();
        }
        let Ok(z) = Timestamp::from_second(from).map(|t| t.to_zoned(tz.clone())) else {
            return Vec::new();
        };
        self.cron
            .iter_after(z)
            .map(|z| z.timestamp().as_second())
            .take_while(|t| *t <= to)
            .collect()
    }
}

/// Five fields, or None. `every N` takes only a length that divides its unit: `every 45m`
/// would fire at :00 and :45 and then wait fifteen minutes.
fn desugar(spec: &str) -> Option<String> {
    let fixed = match spec {
        "@hourly" => "0 * * * *",
        "@daily" | "@midnight" => "0 0 * * *",
        "@weekly" => "0 0 * * 0",
        "@monthly" => "0 0 1 * *",
        "@yearly" | "@annually" => "0 0 1 1 *",
        s if s.starts_with('@') => return None,
        _ => "",
    };
    if !fixed.is_empty() {
        return Some(fixed.into());
    }
    if let Some(rest) = spec.strip_prefix("every ") {
        let rest = rest.trim_start();
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let (n, unit) = rest.split_at(digits);
        let n: u32 = n.parse().ok()?;
        return match unit.trim() {
            "m" | "min" | "mins" | "minute" | "minutes" if (1..=59).contains(&n) && 60 % n == 0 => {
                Some(format!("*/{n} * * * *"))
            }
            "h" | "hr" | "hrs" | "hour" | "hours" if (1..=23).contains(&n) && 24 % n == 0 => {
                Some(format!("0 */{n} * * *"))
            }
            _ => None,
        };
    }
    let f: Vec<&str> = spec.split_whitespace().collect();
    (f.len() == 5).then(|| f.join(" "))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Due,
    CatchUp,
    Queued,
    ByHand,
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            Why::Due => "due",
            Why::CatchUp => "catch-up",
            Why::Queued => "queued",
            Why::ByHand => "by hand",
        })
    }
}

/// What one tick does about a ritual last stamped at `stamp`: the newest fire it has not run
/// for, due while it is fresh and, with `catch`, a catch-up for up to a week. Only that one:
/// however many were missed, one run makes them up. Croner's own occurrences, so its DST rules
/// hold. No stamp is a ritual never seen before: it has missed nothing, and is due only in the
/// minute of a fire (the caller stamps it when this says nothing).
pub fn decide(
    s: &Schedule,
    stamp: Option<i64>,
    now: i64,
    catch: bool,
    tz: &TimeZone,
) -> Option<(Why, i64)> {
    let Some(stamp) = stamp else {
        let latest = s.latest(now, tz).filter(|t| now - t < 60)?;
        return Some((Why::Due, latest));
    };
    // Forward from the stamp, as croner iterates: a wall-clock minute that comes round twice at
    // the fall back is one fire, and a fixed time inside the spring gap is the first instant
    // after it. Looking back from now gives the newest when several were missed.
    let first = s.next(stamp, tz).filter(|t| *t <= now)?;
    let latest = s.latest(now, tz).filter(|t| *t > first).unwrap_or(first);
    match now - latest {
        age if age < DUE_WINDOW => Some((Why::Due, latest)),
        age if catch && age <= CATCH_UP => Some((Why::CatchUp, latest)),
        _ => None,
    }
}
