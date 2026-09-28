//! The `---` block at the top of a ritual or a spell card: one `key: value` a line, read by hand
//! (nothing in either file can run), YAML enough for a file a person types. A quoted value ends
//! at its closing quote, so `schedule: "3 9 * * 1-5"   # weekdays` means the cron; an unquoted
//! one loses a ` #` comment. Double quotes take `\"` and `\\`, single quotes `''`.

/// A letter or digit first, then letters, digits, `.` `_` `-`: a ritual's or a card's name is a
/// file name, typed on a command line.
pub fn name_ok(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphanumeric())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

pub const NAME_RULE: &str = "a letter or digit first, then letters, digits, . _ -";

/// One `key: value` line, and the `- ` lines under it when its value is empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    /// What follows the colon, as written.
    pub raw: String,
    pub block: Vec<String>,
}

impl Field {
    pub fn text(&self) -> String {
        value(&self.raw)
    }

    /// `[a, b]`, a bare `a, b`, or the `- ` lines under an empty value.
    pub fn list(&self) -> Vec<String> {
        match self.text().is_empty() {
            true => self.block.clone(),
            false => items(&self.raw),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Front {
    /// In file order: a key given twice is here twice, and the later one wins.
    pub fields: Vec<Field>,
    /// Keys not in the caller's list, and lines that are no `key: value` at all.
    pub unknown: Vec<String>,
    /// Everything after the block, its leading blank lines and trailing whitespace gone. A file
    /// with no fence is all body.
    pub body: String,
}

/// The block and the body. The fence is a `---` line, first in the file; a block never closed
/// runs to the end, and leaves no body. CRLF reads as LF.
pub fn read(text: &str, known: &[&str]) -> Front {
    let mut f = Front::default();
    let mut lines = text.lines().peekable();
    if lines.peek() == Some(&"---") {
        lines.next();
        // Whether `- ` lines still belong to the last field.
        let mut block = false;
        for line in lines.by_ref() {
            if line == "---" {
                break;
            }
            let l = line.trim_start();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if let (true, Some(item)) = (block, l.strip_prefix('-')) {
                let item = value(item);
                if let Some(last) = f.fields.last_mut().filter(|_| !item.is_empty()) {
                    last.block.push(item);
                }
                continue;
            }
            let Some((key, raw)) = l.split_once(':').filter(|(k, _)| !k.trim().is_empty()) else {
                // A typo as far as a reader can tell: `schedule "@daily"` never fires.
                f.unknown.push(l.trim_end().into());
                block = false;
                continue;
            };
            let key = key.trim();
            if !known.contains(&key) {
                f.unknown.push(key.into());
            }
            block = value(raw).is_empty();
            f.fields.push(Field { key: key.into(), raw: raw.into(), block: Vec::new() });
        }
    }
    let body: Vec<&str> = lines.skip_while(|l| l.trim().is_empty()).collect();
    f.body = body.join("\n").trim_end().to_string();
    f
}

/// One value: the text inside its quotes, or up to a ` #` comment, trimmed. A quote never closed
/// runs to the end of the line.
pub fn value(v: &str) -> String {
    let v = v.trim_start();
    let mut cs = v.chars();
    match cs.next() {
        Some(q @ ('"' | '\'')) => {
            let mut out = String::new();
            while let Some(c) = cs.next() {
                match c {
                    '\\' if q == '"' => match cs.next() {
                        Some(e @ ('"' | '\\')) => out.push(e),
                        Some(e) => out.extend(['\\', e]),
                        None => out.push('\\'),
                    },
                    c if c == q => {
                        if q == '\'' && cs.as_str().starts_with('\'') {
                            cs.next();
                            out.push('\'');
                            continue;
                        }
                        break;
                    }
                    c => out.push(c),
                }
            }
            out
        }
        Some('#') => String::new(),
        _ => v.split(" #").next().unwrap_or("").trim().to_string(),
    }
}

/// The items of a list value: `["a", "b"]` or a bare `a, b`, each a value of its own. A comma
/// splits only outside quotes, parentheses and brackets, so `Bash(git commit -m "a, b")` is one
/// item; a quote opens only at the start of an item, so `Bash(don't)` is not one.
pub fn items(v: &str) -> Vec<String> {
    let v = v.trim();
    let (v, bracketed) = match v.strip_prefix('[') {
        Some(inner) => (inner, true),
        None => (v, false),
    };
    let (mut out, mut cur) = (Vec::new(), String::new());
    let mut depth = 0usize;
    let mut cs = v.chars().peekable();
    while let Some(c) = cs.next() {
        if cur.trim().is_empty() && (c == '"' || c == '\'') {
            // A quoted item, copied as written for `value` to read.
            cur.push(c);
            while let Some(d) = cs.next() {
                cur.push(d);
                if c == '"' && d == '\\' {
                    cur.extend(cs.next());
                } else if d == c {
                    if c == '\'' && cs.peek() == Some(&'\'') {
                        cur.extend(cs.next());
                        continue;
                    }
                    break;
                }
            }
            continue;
        }
        match c {
            '(' | '[' => depth += 1,
            ']' if depth == 0 && bracketed => break,
            ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(value(&cur));
                cur.clear();
                continue;
            }
            '#' if depth == 0 && !bracketed && cur.ends_with(char::is_whitespace) => break,
            _ => {}
        }
        cur.push(c);
    }
    out.push(value(&cur));
    out.retain(|s| !s.is_empty());
    out
}

/// A value as `value` reads it back: in double quotes, or in single ones when it holds a `"` or
/// a `\`. One line only.
pub fn quote(v: &str) -> Result<String, String> {
    if v.contains(['\n', '\r']) {
        return Err(format!("a value is one line: {v:?}"));
    }
    match v.contains(['"', '\\']) {
        true => Ok(format!("'{}'", v.replace('\'', "''"))),
        false => Ok(format!("\"{v}\"")),
    }
}
