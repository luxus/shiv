//! One QuickJS runtime per build, with no filesystem and no network.
//! The scripts are the pinned copies only.

use std::collections::BTreeMap;

use rquickjs::{ArrayBuffer, Context, Ctx, Runtime, TypedArray};
use serde_json::Value;

use super::templates::{BASE_PAK, HIT_BUILDER, HPBARS_JS, LAME_JS};

const HOST: &str = r#"
globalThis.console = { log() {}, error() {}, warn() {} };
globalThis.window = globalThis;
globalThis.module = { exports: {} };
globalThis.atob = function (s) {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const clean = String(s).replace(/=+$/, "");
  let out = "";
  let buf = 0;
  let bits = 0;
  for (let i = 0; i < clean.length; i++) {
    const v = chars.indexOf(clean[i]);
    if (v < 0) continue;
    buf = (buf << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out += String.fromCharCode((buf >> bits) & 255);
    }
  }
  return out;
};
function TextEncoder() {}
TextEncoder.prototype.encode = function (s) {
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 255;
  return out;
};
globalThis.TextEncoder = TextEncoder;
function TextDecoder() {}
TextDecoder.prototype.decode = function (u8) {
  let s = "";
  for (let i = 0; i < u8.length; i++) s += String.fromCharCode(u8[i]);
  return s;
};
globalThis.TextDecoder = TextDecoder;
"#;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct JsError(String);

pub struct Levels {
    pub db: f64,
    pub pitch: f64,
}

pub struct Slot<'a> {
    pub samples: &'a [f32],
    pub start: u32,
    pub end: u32,
    pub levels: Levels,
}

/// Trim is applied by the site's `processSamples` from sample indexes, using
/// the same millisecond conversion the page uses (`sample / 44100 * 1000`).
pub fn build_hitsound(body: Slot<'_>, head: Slot<'_>) -> Result<Vec<u8>, JsError> {
    with_runtime(|ctx| {
        eval(&ctx, HOST, "host")?;
        eval(&ctx, LAME_JS, "lame.min.js")?;
        eval(&ctx, HIT_BUILDER, "hit_builder.js")?;
        set_samples(&ctx, "HOST_BODY", body.samples)?;
        set_samples(&ctx, "HOST_HEAD", head.samples)?;
        let globals = ctx.globals();
        globals
            .set("HOST_BODY_START", body.start)
            .map_err(|e| err(&ctx, e))?;
        globals
            .set("HOST_BODY_END", body.end)
            .map_err(|e| err(&ctx, e))?;
        globals
            .set("HOST_HEAD_START", head.start)
            .map_err(|e| err(&ctx, e))?;
        globals
            .set("HOST_HEAD_END", head.end)
            .map_err(|e| err(&ctx, e))?;
        globals
            .set("HOST_BODY_DB", body.levels.db)
            .map_err(|e| err(&ctx, e))?;
        globals
            .set("HOST_BODY_PITCH", body.levels.pitch)
            .map_err(|e| err(&ctx, e))?;
        globals
            .set("HOST_HEAD_DB", head.levels.db)
            .map_err(|e| err(&ctx, e))?;
        globals
            .set("HOST_HEAD_PITCH", head.levels.pitch)
            .map_err(|e| err(&ctx, e))?;
        let built: TypedArray<u8> = ctx
            .eval(
                r#"
                (() => {
                  const rate = 44100;
                  const body = HS.processSamples(new Float32Array(HOST_BODY), {
                    startMs: HOST_BODY_START / rate * 1000,
                    endMs: HOST_BODY_END / rate * 1000
                  });
                  const head = HS.processSamples(new Float32Array(HOST_HEAD), {
                    startMs: HOST_HEAD_START / rate * 1000,
                    endMs: HOST_HEAD_END / rate * 1000
                  });
                  return HS.buildPak(
                    body,
                    { db: HOST_BODY_DB, pitch: HOST_BODY_PITCH },
                    head,
                    { db: HOST_HEAD_DB, pitch: HOST_HEAD_PITCH }
                  ).vpk;
                })()
                "#,
            )
            .map_err(|e| err(&ctx, e))?;
        copy_bytes(&built)
    })
}

pub fn build_hpbar(options: &BTreeMap<String, Value>) -> Result<Vec<u8>, JsError> {
    let json = serde_json::to_string(options).map_err(|e| JsError(e.to_string()))?;
    with_runtime(|ctx| {
        eval(&ctx, HOST, "host")?;
        eval(&ctx, HPBARS_JS, "hp_hpbars.js")?;
        let base = ArrayBuffer::new_copy(ctx.clone(), BASE_PAK).map_err(|e| err(&ctx, e))?;
        ctx.globals()
            .set("HOST_BASE", base)
            .map_err(|e| err(&ctx, e))?;
        ctx.globals()
            .set("HOST_OPTIONS", json)
            .map_err(|e| err(&ctx, e))?;
        let built: TypedArray<u8> = ctx
            .eval(
                r#"
                (() => {
                  const H = module.exports;
                  return H.buildPak(new Uint8Array(HOST_BASE), JSON.parse(HOST_OPTIONS));
                })()
                "#,
            )
            .map_err(|e| err(&ctx, e))?;
        copy_bytes(&built)
    })
}

pub fn build_hpbar_preset(id: &str) -> Result<Vec<u8>, JsError> {
    with_runtime(|ctx| {
        eval(&ctx, HOST, "host")?;
        eval(&ctx, HPBARS_JS, "hp_hpbars.js")?;
        let base = ArrayBuffer::new_copy(ctx.clone(), BASE_PAK).map_err(|e| err(&ctx, e))?;
        ctx.globals()
            .set("HOST_BASE", base)
            .map_err(|e| err(&ctx, e))?;
        ctx.globals()
            .set("HOST_PRESET", id.to_owned())
            .map_err(|e| err(&ctx, e))?;
        let built: TypedArray<u8> = ctx
            .eval(
                r#"
                (() => {
                  const H = module.exports;
                  const preset = H.PRESETS.find(p => p.id === HOST_PRESET);
                  if (!preset) throw new Error("unknown preset " + HOST_PRESET);
                  return H.buildPak(new Uint8Array(HOST_BASE), preset.o);
                })()
                "#,
            )
            .map_err(|e| err(&ctx, e))?;
        copy_bytes(&built)
    })
}

pub fn hp_presets() -> Result<Vec<(String, String, String)>, JsError> {
    with_runtime(|ctx| {
        eval(&ctx, HOST, "host")?;
        eval(&ctx, HPBARS_JS, "hp_hpbars.js")?;
        let rows: Vec<String> = ctx
            .eval(
                r#"
                module.exports.PRESETS.map(p => p.id + "\t" + p.title + "\t" + JSON.stringify(p.o))
                "#,
            )
            .map_err(|e| err(&ctx, e))?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let (id, rest) = row.split_once('\t')?;
                let (title, options) = rest.split_once('\t')?;
                Some((id.to_owned(), title.to_owned(), options.to_owned()))
            })
            .collect())
    })
}

fn set_samples(ctx: &Ctx<'_>, name: &str, samples: &[f32]) -> Result<(), JsError> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    let buffer = ArrayBuffer::new_copy(ctx.clone(), &bytes).map_err(|e| err(ctx, e))?;
    ctx.globals().set(name, buffer).map_err(|e| err(ctx, e))
}

fn copy_bytes(value: &TypedArray<'_, u8>) -> Result<Vec<u8>, JsError> {
    let bytes = unsafe { value.as_bytes() }
        .ok_or_else(|| JsError("the builder returned no bytes".into()))?;
    Ok(bytes.to_vec())
}

fn eval(ctx: &Ctx<'_>, source: &str, name: &str) -> Result<(), JsError> {
    ctx.eval(source.to_owned()).map_err(|e| {
        let detail = err(ctx, e);
        JsError(format!("{name}: {detail}"))
    })
}

fn err(ctx: &Ctx<'_>, error: rquickjs::Error) -> JsError {
    let caught = format!("{:?}", ctx.catch());
    JsError(format!("{error}: {caught}"))
}

fn with_runtime<T>(f: impl for<'js> FnOnce(Ctx<'js>) -> Result<T, JsError>) -> Result<T, JsError> {
    let runtime = Runtime::new().map_err(|e| JsError(e.to_string()))?;
    runtime.set_memory_limit(256 * 1024 * 1024);
    runtime.set_max_stack_size(8 * 1024 * 1024);
    let context = Context::full(&runtime).map_err(|e| JsError(e.to_string()))?;
    context.with(f)
}
