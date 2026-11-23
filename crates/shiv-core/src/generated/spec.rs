//! What a CompLock-generated mod stores, and the copy-link hash the HP Bar
//! builder puts in the address bar.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Mod id of a hitsound built from the user's clips.
pub const HITSOUND_ID: &str = "complock-gen-hitsound";

/// Mod id of an HP bar built from a copy-link hash.
pub const HPBAR_ID: &str = "complock-gen-hpbar";

/// Whether `mod_id` is one of the two generated CompLock mods.
#[must_use]
pub fn is_id(mod_id: &str) -> bool {
    mod_id == HITSOUND_ID || mod_id == HPBAR_ID
}

/// The inputs a generated mod was built from. Stored on the sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "builder", rename_all = "snake_case")]
pub enum Spec {
    Hitsound {
        name: String,
        body: Clip,
        head: Clip,
    },
    HpBar {
        name: String,
        /// Options that differ from the builder's defaults, as the copy
        /// link's hash parses them.
        options: BTreeMap<String, Value>,
    },
}

/// One hitsound slot: mono 44.1 kHz little-endian f32 in `pcm`, and the
/// trim the builder applies before it encodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub pcm: String,
    pub source_name: String,
    pub start: u32,
    pub end: u32,
    pub db: f64,
    pub pitch: f64,
}

impl Spec {
    /// The stable mod id this spec installs as.
    #[must_use]
    pub fn mod_id(&self) -> &'static str {
        match self {
            Self::Hitsound { .. } => HITSOUND_ID,
            Self::HpBar { .. } => HPBAR_ID,
        }
    }

    /// The name recorded on the sidecar.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Hitsound { name, .. } | Self::HpBar { name, .. } => name,
        }
    }
}

/// The copy-link hash (with or without the leading `#`) as the option
/// object `buildPak` receives. Unknown keys, values the controls reject,
/// and values equal to the builder's defaults are left out.
#[must_use]
pub fn options_from_hash(hash: &str) -> BTreeMap<String, Value> {
    let hash = hash.trim().trim_start_matches('#');
    let mut out = BTreeMap::new();
    if hash.is_empty() {
        return out;
    }
    for pair in hash.split('&') {
        let Some((key, raw)) = pair.split_once('=') else {
            continue;
        };
        let Some(control) = CONTROLS.iter().find(|c| c.key() == key) else {
            continue;
        };
        let Ok(text) = decode_component(raw) else {
            continue;
        };
        let Some(value) = control.parse(&text) else {
            continue;
        };
        out.insert(key.to_owned(), value);
    }
    out.retain(|key, value| !equals_default(key, value));
    out
}

/// The copy-link hash for `options`, without a leading `#`.
#[must_use]
pub fn hash_from_options(options: &BTreeMap<String, Value>) -> String {
    options
        .iter()
        .filter(|(key, value)| !equals_default(key, value))
        .map(|(key, value)| format!("{key}={}", encode_component(&value_text(value))))
        .collect::<Vec<_>>()
        .join("&")
}

fn equals_default(key: &str, value: &Value) -> bool {
    default_text(key).is_some_and(|def| def.eq_ignore_ascii_case(&value_text(value)))
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Bool(true) => "true".to_owned(),
        Value::Bool(false) => "false".to_owned(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                js_number(f)
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn js_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "n is verified finite, integral, and < 1e15"
        )]
        return format!("{}", n as i64);
    }
    let mut s = format!("{n:.12}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

fn default_text(key: &str) -> Option<&'static str> {
    Some(match key {
        "w" => "54.18",
        "h" => "6.3",
        "enemy" => "#FFFF00",
        "ally" | "numColor" | "critColor" => "#FFFFFF",
        "fill" => "flat",
        "bg" | "outlineColor" | "numOutlineColor" | "tickColor" => "#000000",
        "bgAlpha" => "100",
        "outline" | "glow" | "barX" | "barY" | "numX" | "numY" | "iconX" | "iconY" | "staX"
        | "staY" | "critX" | "critY" => "0",
        "num" => "above",
        "numSize" => "16.38",
        "numAlly" | "hitNum" | "low" | "deltaAll" => "false",
        "numOutline" => "4",
        "font" => "numericOracle",
        "hitNumColor" => "#FFD23F",
        "lowColor" => "#FF3030",
        "deltaColor" => "#E62020",
        "ticks" => "off",
        "tickW" => "1",
        "stamina" => "dots",
        "pip" => "5.04",
        "icon" | "crit" => "true",
        "critSize" => "12",
        "critGlow" => "#FF410D",
        _ => return None,
    })
}

enum Control {
    Number {
        key: &'static str,
        min: f64,
        max: f64,
    },
    Choice {
        key: &'static str,
        options: &'static [&'static str],
    },
    Bool {
        key: &'static str,
    },
    Color {
        key: &'static str,
    },
}

impl Control {
    fn key(&self) -> &'static str {
        match self {
            Self::Number { key, .. }
            | Self::Choice { key, .. }
            | Self::Bool { key }
            | Self::Color { key } => key,
        }
    }

    fn parse(&self, text: &str) -> Option<Value> {
        match self {
            Self::Number { min, max, .. } => {
                if text.is_empty() {
                    return None;
                }
                let n: f64 = text.parse().ok()?;
                if !n.is_finite() {
                    return None;
                }
                Some(number(n.clamp(*min, *max)))
            }
            Self::Choice { options, .. } => options
                .contains(&text)
                .then(|| Value::String(text.to_owned())),
            Self::Bool { .. } => match text {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            Self::Color { .. } => {
                let bytes = text.as_bytes();
                if bytes.len() == 7
                    && bytes[0] == b'#'
                    && bytes[1..].iter().all(u8::is_ascii_hexdigit)
                {
                    Some(Value::String(text.to_ascii_uppercase()))
                } else {
                    None
                }
            }
        }
    }
}

fn number(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9_223_372_036_854_775_808.0 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "n is verified finite, integral, and strictly below 2^63"
        )]
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

const CONTROLS: &[Control] = &[
    Control::Number {
        key: "w",
        min: 36.0,
        max: 84.0,
    },
    Control::Number {
        key: "h",
        min: 2.5,
        max: 14.0,
    },
    Control::Color { key: "enemy" },
    Control::Color { key: "ally" },
    Control::Choice {
        key: "fill",
        options: &["flat", "shaded", "gloss"],
    },
    Control::Color { key: "bg" },
    Control::Number {
        key: "bgAlpha",
        min: 0.0,
        max: 100.0,
    },
    Control::Number {
        key: "outline",
        min: 0.0,
        max: 3.0,
    },
    Control::Color {
        key: "outlineColor",
    },
    Control::Number {
        key: "glow",
        min: 0.0,
        max: 10.0,
    },
    Control::Choice {
        key: "num",
        options: &["above", "inside", "right", "off"],
    },
    Control::Number {
        key: "numSize",
        min: 9.0,
        max: 24.0,
    },
    Control::Bool { key: "numAlly" },
    Control::Color { key: "numColor" },
    Control::Number {
        key: "numOutline",
        min: 0.0,
        max: 6.0,
    },
    Control::Color {
        key: "numOutlineColor",
    },
    Control::Choice {
        key: "font",
        options: &["numericOracle", "numericBlock", "numericSans", "sansMono"],
    },
    Control::Bool { key: "hitNum" },
    Control::Color { key: "hitNumColor" },
    Control::Bool { key: "low" },
    Control::Color { key: "lowColor" },
    Control::Color { key: "deltaColor" },
    Control::Bool { key: "deltaAll" },
    Control::Choice {
        key: "ticks",
        options: &["off", "large", "all"],
    },
    Control::Number {
        key: "tickW",
        min: 1.0,
        max: 3.0,
    },
    Control::Color { key: "tickColor" },
    Control::Choice {
        key: "stamina",
        options: &["dots", "squares", "bars", "off"],
    },
    Control::Number {
        key: "pip",
        min: 2.5,
        max: 8.0,
    },
    Control::Bool { key: "icon" },
    Control::Bool { key: "crit" },
    Control::Number {
        key: "critSize",
        min: 8.0,
        max: 20.0,
    },
    Control::Color { key: "critColor" },
    Control::Color { key: "critGlow" },
    Control::Number {
        key: "barX",
        min: -40.0,
        max: 40.0,
    },
    Control::Number {
        key: "barY",
        min: -30.0,
        max: 30.0,
    },
    Control::Number {
        key: "numX",
        min: -40.0,
        max: 40.0,
    },
    Control::Number {
        key: "numY",
        min: -30.0,
        max: 30.0,
    },
    Control::Number {
        key: "iconX",
        min: -40.0,
        max: 40.0,
    },
    Control::Number {
        key: "iconY",
        min: -30.0,
        max: 30.0,
    },
    Control::Number {
        key: "staX",
        min: -40.0,
        max: 40.0,
    },
    Control::Number {
        key: "staY",
        min: -30.0,
        max: 30.0,
    },
    Control::Number {
        key: "critX",
        min: -60.0,
        max: 60.0,
    },
    Control::Number {
        key: "critY",
        min: -45.0,
        max: 45.0,
    },
];

fn decode_component(raw: &str) -> Result<String, ()> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(());
                }
                let hi = hex_val(bytes[i + 1]).ok_or(())?;
                let lo = hex_val(bytes[i + 2]).ok_or(())?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(char::from(b"0123456789ABCDEF"[usize::from(b >> 4)]));
            out.push(char::from(b"0123456789ABCDEF"[usize::from(b & 0xf)]));
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
