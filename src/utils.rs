//! Utility functions (`brainglobe_atlasapi.utils`): atlas names, versions,
//! the terminology CSV, JSON and INI files, and (feature `remote`) the web
//! requests.

use std::path::Path;

use serde_json::{Map, Value};

use crate::configparser::ConfigParser;
use crate::descriptors;
use crate::error::{Error, Result};
use crate::structure_class::StructureRecord;

/// The parts of an atlas name (`atlas_repr_from_name`).
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasRepr {
    pub name: String,
    pub major_vers: Option<String>,
    pub minor_vers: Option<String>,
    pub resolution: String,
    pub unit: Option<String>,
}

/// Split an atlas name into name, version, resolution and unit
/// (`atlas_repr_from_name`).
pub fn atlas_repr_from_name(name: &str) -> Result<AtlasRepr> {
    let mut parts: Vec<&str> = name.split('_').collect();
    let last = parts.last().copied().unwrap_or("");
    // if the atlas name has no version
    let version_str = if !descriptors::RESOLUTION.iter().any(|u| last.ends_with(u)) {
        parts.pop()
    } else {
        None
    };
    let resolution_str = parts
        .pop()
        .ok_or_else(|| Error::Index("pop from empty list".into()))?;
    let atlas_name = parts.join("_");

    let (major_vers, minor_vers) = match version_str {
        Some(v) => {
            let rest: String = v.chars().skip(1).collect();
            let split: Vec<&str> = rest.split('.').collect();
            if split.len() != 2 {
                return Err(unpack_error(split.len()));
            }
            (Some(split[0].to_string()), Some(split[1].to_string()))
        }
        None => (None, None),
    };

    // separate unit from resolution
    let mut unit = None;
    let mut resolution = resolution_str.to_string();
    for res_unit in descriptors::RESOLUTION {
        if resolution.ends_with(res_unit) {
            unit = Some(res_unit.to_string());
            resolution.truncate(resolution.len() - res_unit.len());
            break;
        }
    }
    Ok(AtlasRepr {
        name: atlas_name,
        major_vers,
        minor_vers,
        resolution,
        unit,
    })
}

/// Python's message for a failed `a, b = values` unpack.
fn unpack_error(n: usize) -> Error {
    if n < 2 {
        Error::value(format!("not enough values to unpack (expected 2, got {n})"))
    } else {
        Error::value("too many values to unpack (expected 2)")
    }
}

/// An atlas name from its parts (`atlas_name_from_repr`).
pub fn atlas_name_from_repr(
    name: &str,
    resolution: &str,
    major_vers: Option<&str>,
    minor_vers: Option<&str>,
    unit: &str,
) -> String {
    if major_vers.is_none() && minor_vers.is_none() {
        format!("{name}_{resolution}{unit}")
    } else {
        format!(
            "{name}_{resolution}{unit}_v{}.{}",
            major_vers.unwrap_or("None"),
            minor_vers.unwrap_or("None")
        )
    }
}

/// The latest of a list of `major_minor` version strings
/// (`get_latest_version`); sorts the list in place, newest first.
pub fn get_latest_version(available_versions: &mut [String]) -> Result<String> {
    let mut keyed: Vec<(Vec<i64>, String)> = Vec::with_capacity(available_versions.len());
    for v in available_versions.iter() {
        let key = v
            .split('_')
            .map(|x| {
                parse_py_int(x).ok_or_else(|| {
                    Error::value(format!(
                        "invalid literal for int() with base 10: {}",
                        py_repr_str(x)
                    ))
                })
            })
            .collect::<Result<Vec<i64>>>()?;
        keyed.push((key, v.clone()));
    }
    // Python's sort is stable; reverse=True keeps equal keys in their order
    keyed.sort_by(|a, b| b.0.cmp(&a.0));
    for (slot, (_, v)) in available_versions.iter_mut().zip(&keyed) {
        *slot = v.clone();
    }
    keyed
        .into_iter()
        .next()
        .map(|(_, v)| v)
        .ok_or_else(|| Error::Index("list index out of range".into()))
}

/// `int(x)` for a decimal string: surrounding whitespace, a sign and `_`
/// digit separators are allowed.
pub fn parse_py_int(x: &str) -> Option<i64> {
    let t = x.trim();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let v: i64 = digits.replace('_', "").parse().ok()?;
    Some(if neg { -v } else { v })
}

/// Python's `repr` of a float (`100.0`, `0.5`, `1e-05`).
pub fn py_float_repr(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let abs = v.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        // shortest round-trip digits in scientific notation, Python style
        let s = format!("{v:e}");
        let (m, e) = s.split_once('e').unwrap();
        let e: i32 = e.parse().unwrap();
        return format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    let s = format!("{v:?}");
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Python's `repr()` of a string.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Read a JSON file (`read_json`), keeping the key order.
pub fn read_json(path: impl AsRef<Path>) -> Result<Map<String, Value>> {
    let text = std::fs::read_to_string(path)?;
    match serde_json::from_str::<Value>(&text)? {
        Value::Object(m) => Ok(m),
        other => Err(Error::Type(format!(
            "expected a JSON object, got {}",
            match other {
                Value::Array(_) => "list",
                Value::String(_) => "str",
                Value::Number(_) => "number",
                Value::Bool(_) => "bool",
                _ => "None",
            }
        ))),
    }
}

/// The strings pandas reads as missing here (`na_values`, with
/// `keep_default_na=False`).
const NA_VALUES: [&str; 7] = ["", "NaN", "NULL", "nan", "N/A", "na", "null"];

/// Load the structures of a terminology CSV (`load_structures_from_csv`).
///
/// The columns are renamed as Python renames them (`identifier` -> `id`,
/// `parent_identifier` -> `parent_structure_id`, `abbreviation` ->
/// `acronym`, `root_identifier_path` -> `structure_id_path`,
/// `color_hex_triplet` -> `rgb_triplet`); any other column is kept, typed
/// the way pandas infers it.
pub fn load_structures_from_csv(path: impl AsRef<Path>) -> Result<Vec<StructureRecord>> {
    let text = std::fs::read(path)?;
    load_structures_from_csv_bytes(&text)
}

/// [`load_structures_from_csv`] from the file's bytes.
pub fn load_structures_from_csv_bytes(bytes: &[u8]) -> Result<Vec<StructureRecord>> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .from_reader(bytes);
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| Error::value(e.to_string()))?
        .iter()
        .map(str::to_string)
        .collect();
    let rows: Vec<Vec<String>> = rdr
        .records()
        .map(|r| r.map(|r| r.iter().map(str::to_string).collect()))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| Error::value(e.to_string()))?;

    let col = |name: &str| headers.iter().position(|h| h == name);
    // columns pandas types by inference: int64 if every value is an int,
    // float64 if every value is a number (or missing), else strings
    let extra_cols: Vec<(usize, String, ColKind)> = headers
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            !matches!(
                h.as_str(),
                "identifier"
                    | "parent_identifier"
                    | "abbreviation"
                    | "root_identifier_path"
                    | "color_hex_triplet"
                    | "name"
            )
        })
        .map(|(i, h)| (i, h.clone(), infer_kind(rows.iter().map(|r| r[i].as_str()))))
        .collect();

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let get = |name: &str| col(name).map(|i| r[i].as_str());
        let id = get("identifier")
            .map(|v| int_cell(v, "identifier"))
            .transpose()?
            .ok_or_else(|| Error::key("identifier"))?;
        let parent = match get("parent_identifier") {
            Some(v) if NA_VALUES.contains(&v) => None,
            Some(v) => Some(int_cell(v, "parent_identifier")?),
            None => None,
        };
        let rgb = match get("color_hex_triplet") {
            Some(v) => Some(hex_triplet(v)?),
            None => None,
        };
        let path = get("root_identifier_path").map(id_path);
        let mut extra = Map::new();
        for (i, h, kind) in &extra_cols {
            extra.insert(h.clone(), kind.cell(&r[*i]));
        }
        out.push(StructureRecord {
            id,
            parent_structure_id: parent,
            name: get("name").and_then(na_string),
            acronym: get("abbreviation").and_then(na_string),
            rgb_triplet: rgb,
            structure_id_path: path,
            columns: headers.clone(),
            extra,
        });
    }
    Ok(out)
}

/// A string cell: `None` where pandas reads a missing value (NaN).
fn na_string(v: &str) -> Option<String> {
    (!NA_VALUES.contains(&v)).then(|| v.to_string())
}

fn int_cell(v: &str, column: &str) -> Result<i64> {
    parse_py_int(v)
        .ok_or_else(|| Error::value(format!("Unable to parse string \"{v}\" in column {column}")))
}

/// `[int(x.strip("#")[i : i + 2], 16) for i in (0, 2, 4)]`.
fn hex_triplet(v: &str) -> Result<[u8; 3]> {
    let h = v.trim_matches('#');
    let mut rgb = [0u8; 3];
    for (k, i) in [0usize, 2, 4].into_iter().enumerate() {
        let part = h.get(i..(i + 2).min(h.len())).unwrap_or("");
        rgb[k] = u8::from_str_radix(part, 16).map_err(|_| {
            Error::value(format!(
                "invalid literal for int() with base 16: {}",
                py_repr_str(part)
            ))
        })?;
    }
    Ok(rgb)
}

/// `np.fromstring(x.strip("[]"), sep=",", dtype=np.uint32).tolist()`: the
/// numbers up to the first one that does not parse.
fn id_path(v: &str) -> Vec<u32> {
    let inner = v.trim_matches(|c| c == '[' || c == ']');
    let mut out = vec![];
    if inner.trim().is_empty() {
        return out;
    }
    for p in inner.split(',') {
        match p.trim().parse::<u64>() {
            Ok(n) => out.push(n as u32),
            Err(_) => break,
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
enum ColKind {
    Int,
    Float,
    Str,
}

fn infer_kind<'a>(cells: impl Iterator<Item = &'a str>) -> ColKind {
    let mut kind = ColKind::Int;
    for c in cells {
        if NA_VALUES.contains(&c) {
            // a missing value makes an int column float (NaN)
            if matches!(kind, ColKind::Int) {
                kind = ColKind::Float;
            }
            continue;
        }
        if c.trim().parse::<i64>().is_ok() {
            continue;
        }
        if c.trim().parse::<f64>().is_ok() {
            if matches!(kind, ColKind::Int) {
                kind = ColKind::Float;
            }
            continue;
        }
        return ColKind::Str;
    }
    kind
}

impl ColKind {
    fn cell(self, v: &str) -> Value {
        if NA_VALUES.contains(&v) {
            return Value::Null;
        }
        match self {
            ColKind::Int => v
                .trim()
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or(Value::Null),
            ColKind::Float => v
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            ColKind::Str => Value::String(v.to_string()),
        }
    }
}

/// Read an INI file (`conf_from_file`).
pub fn conf_from_file(file_path: &Path) -> Result<ConfigParser> {
    if !file_path.exists() {
        return Err(Error::file_not_found("Last versions cache file not found."));
    }
    let text = std::fs::read_to_string(file_path)?;
    let mut config = ConfigParser::new();
    config.read_string(&text)?;
    Ok(config)
}

#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub use crate::remote::{
    check_internet_connection, check_s3_status, conf_from_url, retrieve_over_http,
};
