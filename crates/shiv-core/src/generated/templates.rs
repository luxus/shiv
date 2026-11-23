//! The pinned CompLock builder files. The watch compares a fetch to these
//! bytes. A script that does not match is never passed to the engine.

pub const HIT_BUILDER: &str = include_str!("../../assets/complock-gen/hit_builder.js");
pub const HPBARS_JS: &str = include_str!("../../assets/complock-gen/hp_hpbars.js");
pub const LAME_JS: &str = include_str!("../../assets/complock-gen/lame.min.js");
pub const BASE_PAK: &[u8] = include_bytes!("../../assets/complock-gen/hpbars_base.vpk");
const REVIEW_META: &str = include_str!("../../assets/complock-gen/builders.json");

/// Whether the pinned meta says the upstream builder scripts differ from
/// this copy. A broken meta file is treated as a review, so a bad embed
/// cannot turn into an automatic rebuild.
#[must_use]
pub fn builder_review() -> bool {
    serde_json::from_str::<serde_json::Value>(REVIEW_META)
        .ok()
        .and_then(|v| v.get("review").and_then(serde_json::Value::as_bool))
        .unwrap_or(true)
}

/// The hitsound builder script inside a page or a script body: from
/// `"use strict"` (or `const TPL`) through the `window.HS` assignment.
#[must_use]
pub fn extract_hit_builder(text: &str) -> Option<String> {
    let start = text
        .find("\"use strict\"")
        .or_else(|| text.find("const TPL"))?;
    let rest = &text[start..];
    let hs = rest.find("window.HS = ")?;
    let line_end = rest[hs..].find('\n')?;
    if !rest[..hs].contains("function buildPak") {
        return None;
    }
    let mut out = rest[..hs + line_end].to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// The two template blobs the hitsound script patches, concatenated.
#[must_use]
pub fn hitsound_template(script: &str) -> Option<Vec<u8>> {
    let mut out = decode_b64(field_b64(script, "vsnd")?)?;
    out.extend(decode_b64(field_b64(script, "events")?)?);
    Some(out)
}

/// `script` with the two template base64 strings removed, so a template
/// edit and a logic edit hash apart.
#[must_use]
pub fn logic_text(script: &str) -> Option<String> {
    let vsnd = field_b64(script, "vsnd")?;
    let events = field_b64(script, "events")?;
    let once = blank_once(script, vsnd)?;
    blank_once(&once, events)
}

/// The pinned hitsound script with its template base64 replaced by the
/// fetch's. The logic bytes stay the pinned ones.
#[must_use]
pub fn retarget_hit_builder(pinned: &str, fetched: &str) -> Option<String> {
    let vsnd = field_b64(fetched, "vsnd")?;
    let events = field_b64(fetched, "events")?;
    if !is_b64(vsnd) || !is_b64(events) {
        return None;
    }
    let with_vsnd = replace_field(pinned, "vsnd", vsnd)?;
    replace_field(&with_vsnd, "events", events)
}

/// Byte offset of a named field inside the script's `"offsets"` object
/// that follows `marker` (`"vsnd"` or `"events"`).
#[must_use]
pub fn offset(script: &str, marker: &str, field: &str) -> Option<usize> {
    let at = script.find(&format!("\"{marker}\""))?;
    let obj = script[at..].find("\"offsets\"")?;
    let brace = script[at + obj..].find('{')?;
    let start = at + obj + brace;
    let end = script[start..].find('}')? + start;
    let value: serde_json::Value = serde_json::from_str(&script[start..=end]).ok()?;
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)?
        .try_into()
        .ok()
}

/// `const BASE = { body, headSend, headRecv }` from the hitsound script.
#[must_use]
pub fn base_levels(script: &str) -> Option<(f64, f64, f64)> {
    let at = script.find("const BASE")?;
    Some((
        js_number_after(script, at, "body:")?,
        js_number_after(script, at, "headSend:")?,
        js_number_after(script, at, "headRecv:")?,
    ))
}

fn js_number_after(script: &str, from: usize, key: &str) -> Option<f64> {
    let at = script[from..].find(key)? + from + key.len();
    let rest = script[at..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn field_b64<'a>(script: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\":{{\"base64\":\"");
    let start = script.find(&marker)? + marker.len();
    let end = script[start..].find('"')? + start;
    Some(&script[start..end])
}

fn replace_field(script: &str, key: &str, b64: &str) -> Option<String> {
    let marker = format!("\"{key}\":{{\"base64\":\"");
    let start = script.find(&marker)? + marker.len();
    let end = script[start..].find('"')? + start;
    let mut out = String::with_capacity(script.len() - (end - start) + b64.len());
    out.push_str(&script[..start]);
    out.push_str(b64);
    out.push_str(&script[end..]);
    Some(out)
}

fn blank_once(script: &str, needle: &str) -> Option<String> {
    let start = script.find(needle)?;
    let mut out = String::with_capacity(script.len() - needle.len());
    out.push_str(&script[..start]);
    out.push_str(&script[start + needle.len()..]);
    Some(out)
}

fn is_b64(s: &str) -> bool {
    !s.is_empty() && decode_b64(s).is_some()
}

pub(crate) fn decode_b64(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    if !s.len().is_multiple_of(4) {
        return None;
    }
    let bytes = s.as_bytes();
    let total_chunks = bytes.len() / 4;
    let mut out = Vec::with_capacity(total_chunks * 3);
    for (idx, &[a, b, c, d]) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let is_last = idx + 1 == total_chunks;
        let av = val(a)?;
        let bv = val(b)?;
        if (c == b'=' || d == b'=') && !is_last {
            return None;
        }
        if c == b'=' && d != b'=' {
            return None;
        }
        let cv = if c == b'=' { 0 } else { val(c)? };
        let dv = if d == b'=' { 0 } else { val(d)? };
        let n =
            (u32::from(av) << 18) | (u32::from(bv) << 12) | (u32::from(cv) << 6) | u32::from(dv);
        out.push((n >> 16) as u8);
        if c != b'=' {
            out.push((n >> 8) as u8);
        }
        if d != b'=' {
            out.push(n as u8);
        }
    }
    Some(out)
}
