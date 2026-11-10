//! Tokenizing and parsing of literal query values: sizes, ages, dates and depths.

use super::*;

/// Split an expression into tokens, each flagged with whether it opened with
/// a quote.
pub(super) fn tokenize(expression: &str) -> Result<Vec<(String, bool)>, QueryError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut quoted_start = false;
    let mut chars = expression.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            // Outside quotes `\"` and `\'` are literal quote characters.
            let escapable = chars.peek().is_some_and(|next| {
                next.is_whitespace()
                    || *next == '\\'
                    || Some(*next) == quote
                    || (quote.is_none() && matches!(*next, '"' | '\''))
            });
            if escapable {
                current.push(chars.next().unwrap());
            } else {
                // Preserve regex and path escapes such as `\.` and `\d`.
                current.push(ch);
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            if current.is_empty() {
                quoted_start = true;
            }
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push((std::mem::take(&mut current), quoted_start));
            }
            quoted_start = false;
        } else {
            current.push(ch);
        }
    }
    if let Some(delimiter) = quote {
        return Err(QueryError::new(delimiter.to_string(), "unterminated quote"));
    }
    if !current.is_empty() {
        tokens.push((current, quoted_start));
    }
    Ok(tokens)
}

pub(super) fn parse_kind(value: &str) -> Option<Kind> {
    match value.to_ascii_lowercase().as_str() {
        "folder" | "folders" | "dir" | "directory" => Some(Kind::Folder),
        "image" | "images" | "photo" | "photos" => Some(Kind::Image),
        "video" | "videos" => Some(Kind::Video),
        "audio" | "music" => Some(Kind::Audio),
        "document" | "documents" | "doc" | "docs" | "pdf" => Some(Kind::Document),
        "code" | "source" => Some(Kind::Code),
        "archive" | "archives" => Some(Kind::Archive),
        "other" => Some(Kind::Other),
        _ => None,
    }
}

pub(super) fn parse_depth(value: &str) -> Result<Vec<Predicate>, &'static str> {
    let number = |value: &str| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("depth must be a non-negative integer");
        }
        value.parse::<usize>().map_err(|_| "depth is too large")
    };
    if let Some((min, max)) = value.split_once("..") {
        let (min, max) = (number(min)?, number(max)?);
        if min > max {
            return Err("depth range is reversed");
        }
        return Ok(vec![Predicate::MinDepth(min), Predicate::MaxDepth(max)]);
    }
    if let Some(value) = value.strip_prefix("<=") {
        return Ok(vec![Predicate::MaxDepth(number(value)?)]);
    }
    if let Some(value) = value.strip_prefix(">=") {
        return Ok(vec![Predicate::MinDepth(number(value)?)]);
    }
    let depth = number(value)?;
    Ok(vec![Predicate::MinDepth(depth), Predicate::MaxDepth(depth)])
}

pub(super) fn parse_links(value: &str) -> Result<Vec<Predicate>, &'static str> {
    let number = |raw: &str| {
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err("links expects an unsigned decimal integer");
        }
        raw.parse::<u64>().map_err(|_| "link count is too large")
    };
    if let Some((lo, hi)) = value.split_once("..") {
        let (lo, hi) = (number(lo)?, number(hi)?);
        if lo > hi {
            return Err("link count range is reversed");
        }
        return Ok(vec![Predicate::MinLinks(lo), Predicate::MaxLinks(hi)]);
    }
    if let Some(raw) = value.strip_prefix('=') {
        let n = number(raw)?;
        return Ok(vec![Predicate::MinLinks(n), Predicate::MaxLinks(n)]);
    }
    let (op, raw) = split_operator(value);
    let n = number(raw)?;
    match op {
        "" | "=" => Ok(vec![Predicate::MinLinks(n), Predicate::MaxLinks(n)]),
        ">=" => Ok(vec![Predicate::MinLinks(n)]),
        "<=" => Ok(vec![Predicate::MaxLinks(n)]),
        ">" => n
            .checked_add(1)
            .map(|v| vec![Predicate::MinLinks(v)])
            .ok_or("no link count exceeds u64::MAX"),
        "<" => n
            .checked_sub(1)
            .map(|v| vec![Predicate::MaxLinks(v)])
            .ok_or("no link count is below zero"),
        _ => Err("invalid link count operator"),
    }
}

pub(super) fn parse_size(value: &str) -> Result<Vec<Predicate>, &'static str> {
    if let Some(exact) = value.strip_prefix('=') {
        let bytes = parse_bytes(exact)?;
        return Ok(vec![Predicate::MinSize(bytes), Predicate::MaxSize(bytes)]);
    }
    if let Some((min, max)) = value.split_once("..") {
        let min = parse_bytes(min)?;
        let max = parse_bytes(max)?;
        if min > max {
            return Err("size range is reversed");
        }
        return Ok(vec![Predicate::MinSize(min), Predicate::MaxSize(max)]);
    }
    let (operator, number) = split_operator(value);
    let bytes = parse_bytes(number)?;
    // Size predicates are inclusive, so strict operators shift by one byte.
    match operator {
        "<" => bytes
            .checked_sub(1)
            .map(|max| vec![Predicate::MaxSize(max)])
            .ok_or("no size is below zero"),
        "<=" => Ok(vec![Predicate::MaxSize(bytes)]),
        ">" => bytes
            .checked_add(1)
            .map(|min| vec![Predicate::MinSize(min)])
            .ok_or("no size exceeds the maximum byte count"),
        ">=" | "" => Ok(vec![Predicate::MinSize(bytes)]),
        _ => Err("invalid size operator"),
    }
}

fn parse_bytes(value: &str) -> Result<u64, &'static str> {
    let upper = value.trim().to_ascii_uppercase();
    let split = upper
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .unwrap_or(upper.len());
    let numeric = &upper[..split];
    let (whole, fraction) = numeric.split_once('.').unwrap_or((numeric, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("invalid size");
    }
    let multiplier = match upper[split..].trim() {
        "" | "B" => 1,
        "K" | "KB" | "KIB" => 1_u64 << 10,
        "M" | "MB" | "MIB" => 1_u64 << 20,
        "G" | "GB" | "GIB" => 1_u64 << 30,
        "T" | "TB" | "TIB" => 1_u64 << 40,
        _ => return Err("unknown size unit"),
    };
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u64>().map_err(|_| "size is too large")?
    };
    // Multiply decimal digits right-to-left. Carry stays below the unit,
    // regardless of fractional length; its last discarded digit decides rounding.
    let mut fractional_bytes = 0;
    let mut round_up = false;
    for digit in fraction.bytes().rev() {
        let product = u64::from(digit - b'0') * multiplier + fractional_bytes;
        fractional_bytes = product / 10;
        round_up = product % 10 >= 5;
    }
    whole
        .checked_mul(multiplier)
        .and_then(|bytes| bytes.checked_add(fractional_bytes))
        .and_then(|bytes| bytes.checked_add(u64::from(round_up)))
        .ok_or("size is too large")
}

pub(super) fn parse_age(value: &str) -> Result<Predicate, &'static str> {
    let value = value.trim();
    let (comparison, amount) = [
        ("<=", AgeComparison::AtMost),
        (">=", AgeComparison::AtLeast),
        ("<", AgeComparison::Less),
        (">", AgeComparison::Greater),
        ("=", AgeComparison::Equal),
    ]
    .into_iter()
    .find_map(|(operator, comparison)| {
        value
            .strip_prefix(operator)
            .map(|amount| (comparison, amount))
    })
    .unwrap_or((AgeComparison::AtMost, value));
    let amount = amount.trim().to_ascii_lowercase();
    let split = amount
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(amount.len());
    let number = amount[..split]
        .parse::<u64>()
        .map_err(|_| "age expects an unsigned whole duration")?;
    let multiplier = match amount[split..].trim() {
        "s" | "sec" | "second" | "seconds" => 1,
        "m" | "min" | "minute" | "minutes" => 60,
        "h" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        "w" | "week" | "weeks" => 604800,
        _ => return Err("age expects seconds, minutes, hours, days or weeks"),
    };
    let seconds = number
        .checked_mul(multiplier)
        .ok_or("age duration is too large")?;
    Ok(Predicate::AgeSeconds {
        comparison,
        seconds,
    })
}

pub(super) fn parse_date(value: &str) -> Result<Predicate, &'static str> {
    let (operator, amount) = split_operator(value);
    let lower = amount.trim().to_ascii_lowercase();
    let split = lower
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(lower.len());
    let number = lower[..split]
        .parse::<u64>()
        .map_err(|_| "invalid date age")?;
    let multiplier = match lower[split..].trim() {
        "" | "d" | "day" | "days" => 1,
        "w" | "week" | "weeks" => 7,
        "m" | "month" | "months" => 30,
        "y" | "year" | "years" => 365,
        _ => return Err("unknown date unit"),
    };
    let days = number
        .checked_mul(multiplier)
        .ok_or("date age is too large")?;
    match operator {
        "<" | "<=" | "" => Ok(Predicate::MaxAgeDays(days)),
        ">" | ">=" => Ok(Predicate::MinAgeDays(days)),
        _ => Err("invalid date operator"),
    }
}

fn split_operator(value: &str) -> (&str, &str) {
    for operator in [">=", "<=", ">", "<"] {
        if let Some(rest) = value.strip_prefix(operator) {
            return (operator, rest);
        }
    }
    ("", value)
}

pub(super) fn format_bytes(bytes: u64) -> String {
    for (unit, divisor) in [
        ("TB", 1_u64 << 40),
        ("GB", 1_u64 << 30),
        ("MB", 1_u64 << 20),
        ("KB", 1_u64 << 10),
    ] {
        if bytes >= divisor && bytes.is_multiple_of(divisor) {
            return format!("{}{unit}", bytes / divisor);
        }
    }
    format!("{bytes}B")
}

pub(super) fn quote_if_needed(value: &str) -> String {
    if value
        .chars()
        .all(|ch| !ch.is_whitespace() && !matches!(ch, '\\' | '"' | '\''))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}
