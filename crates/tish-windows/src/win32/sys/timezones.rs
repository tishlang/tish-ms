//! `windows.timeZones`: the IANA time-zone database (tish-ms-common's zones, compiled in), in
//! tish-macos's shape.
//!
//! - `names()` -> every zone id ("Asia/Tokyo", …)
//! - `local()` -> the local zone's id
//! - `at(id, unix)` -> `{ offset, abbreviation }` (seconds east of UTC, "JST"), or null for an
//!   unknown id (bind it first: `let zoneAt = windows.timeZones.at`, as with tish-macos)
//! - `byAbbreviation("CET")` -> a zone id, or null

use tishlang_core::Value;
use tishlang_ms_common::zones;

use super::{arr, obj, s, str_arg};

pub(super) fn names(_a: &[Value]) -> Value {
    arr(zones::names().into_iter().map(s).collect())
}

pub(super) fn local(_a: &[Value]) -> Value {
    s(&zones::local())
}

pub(super) fn at(args: &[Value]) -> Value {
    let unix = args.get(1).and_then(|v| v.as_number()).unwrap_or(0.0) as i64;
    match zones::at(&str_arg(args, 0), unix) {
        Some((offset, abbr)) => obj(vec![("offset", Value::Number(offset as f64)), ("abbreviation", s(&abbr))]),
        None => Value::Null,
    }
}

pub(super) fn by_abbreviation(args: &[Value]) -> Value {
    zones::by_abbreviation(&str_arg(args, 0)).map_or(Value::Null, |z| s(&z))
}
