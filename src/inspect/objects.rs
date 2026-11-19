//! One object at a time: `obj` and `stream`.

use std::path::Path;

use pdf::object::Resolve;
use pdf::primitive::Primitive;

use super::doc::Doc;
use super::fmt::{hexdump, human_bytes, is_text, kind_of, prim_text};
use super::raw::Entry;

/// `12`, `12 0`, `12.0`, `12/0`, `#12` -> (id, gen).
pub fn parse_target(s: &str) -> Result<(u64, u64), String> {
    let cleaned = s.trim().trim_start_matches('#');
    let parts: Vec<&str> = cleaned
        .split(|c: char| c.is_whitespace() || c == '.' || c == '/' || c == ':')
        .filter(|p| !p.is_empty())
        .collect();
    match parts.as_slice() {
        [id] => Ok((
            id.parse().map_err(|_| format!("{s:?} is not an object number"))?,
            0,
        )),
        [id, gen, ..] => Ok((
            id.parse().map_err(|_| format!("{s:?}: bad object number"))?,
            gen.parse().map_err(|_| format!("{s:?}: bad generation number"))?,
        )),
        [] => Err(format!("{s:?} is not an object reference")),
    }
}

pub fn obj(doc: &Doc, target: &str, depth: u32, raw_mode: bool) -> Result<(), String> {
    let (id, gen) = parse_target(target)?;
    let xref = doc.raw.xref();
    let map = xref.map();
    let entry = map.get(&id).copied();

    // Where it lives (offset or object stream).
    let mut where_: Option<String> = None;
    let mut offset: Option<usize> = None;
    match entry {
        Some((Entry::InUse { off, gen: g }, _)) => {
            where_ = Some(format!("offset {off}, generation {g}"));
            offset = Some(off as usize);
        }
        Some((Entry::Compressed { container, index }, _)) => {
            where_ = Some(format!("compressed: object stream {container}, index {index}"));
        }
        Some((Entry::Free { next, gen: g }, _)) => {
            println!("{id} {gen} is a free entry (next free {next}, generation {g})");
        }
        None => {
            println!("{id} {gen} is not in the cross-reference table");
        }
    }

    if raw_mode {
        let off = offset.ok_or_else(|| {
            format!("object {id} {gen} has no offset in the file (it is compressed)")
        })?;
        let o = doc.raw.object_at(off)?;
        println!(
            "object {} {} at {}..{} ({} bytes as stored)",
            o.id,
            o.gen,
            o.start,
            o.end,
            o.end - o.start
        );
        for p in &o.problems {
            println!("problem: {p}");
        }
        let bytes = &doc.raw.data[o.start..o.end];
        if is_text(bytes) {
            print!("{}", String::from_utf8_lossy(bytes));
            if !bytes.ends_with(b"\n") {
                println!();
            }
        } else {
            print!("{}", hexdump(bytes, 4096));
        }
        return Ok(());
    }

    let resolve = doc.resolver()?;
    let prim = match resolve.resolve(pdf::object::PlainRef { id, gen }) {
        Ok(p) => p,
        Err(e) => {
            // Fall back to the bytes in the file: a broken object is exactly
            // what someone runs this for.
            if let Some(off) = offset {
                if let Ok(o) = doc.raw.object_at(off) {
                    println!("(the library cannot resolve it: {e}; showing the bytes as stored)");
                    if let Some(where_) = where_ {
                        println!("at {where_}");
                    }
                    for p in &o.problems {
                        println!("problem: {p}");
                    }
                    println!("{}", prim_text(&o.prim, &resolve, depth));
                    return Ok(());
                }
            }
            return Err(format!("object {id} {gen}: {e}"));
        }
    };
    if let Some(where_) = where_ {
        println!("{id} {gen}: {} ({where_})", kind_of(&prim));
    } else {
        println!("{id} {gen}: {}", kind_of(&prim));
    }
    println!("{}", prim_text(&prim, &resolve, depth));
    Ok(())
}

pub fn stream(
    doc: &Doc,
    target: &str,
    raw_mode: bool,
    print: bool,
    hexdump_mode: bool,
    head: Option<usize>,
    to: Option<&Path>,
) -> Result<(), String> {
    let (id, gen) = parse_target(target)?;
    let pref = pdf::object::PlainRef { id, gen };
    let resolve = doc.resolver()?;
    let prim = resolve.resolve(pref).map_err(|e| format!("object {id} {gen}: {e}"))?;
    let stream = match prim {
        Primitive::Stream(s) => s,
        other => {
            return Err(format!(
                "object {id} {gen} is not a stream, it is {}",
                kind_of(&other)
            ))
        }
    };
    let dict = stream.info.clone();
    let specs = super::raw::filter_specs(&dict, &resolve);
    let raw_bytes = stream
        .raw_data(&resolve)
        .map_err(|e| format!("stream data: {e}"))?;
    let declared = dict
        .get("Length")
        .and_then(|p| p.as_integer().ok())
        .map(|i| i.to_string())
        .unwrap_or_else(|| "indirect".to_string());

    println!("object    {id} {gen}");
    let ty = kind_of(&Primitive::Dictionary(dict.clone()));
    println!("type      {}", if ty == "dict" { "stream" } else { &ty });
    println!("length    {declared} declared, {} stored", raw_bytes.len());
    if specs.is_empty() {
        println!("filters   none");
    } else {
        let names: Vec<String> = specs
            .iter()
            .map(|(n, p)| {
                if p.is_empty() {
                    n.clone()
                } else {
                    format!("{n} {}", super::fmt::prim_inline(&Primitive::Dictionary(p.clone())))
                }
            })
            .collect();
        println!("filters   {}", names.join(" -> "));
    }

    let (data, codec) = if raw_mode {
        (raw_bytes.to_vec(), None)
    } else {
        super::raw::decode_chain(&raw_bytes, &specs, &doc.opts)?
    };
    match codec {
        Some(c) => println!(
            "decoded   {} — an image codec ({c}); use `image` for a picture file",
            human_bytes(data.len() as u64)
        ),
        None => println!("decoded   {}", human_bytes(data.len() as u64)),
    }

    if let Some(path) = to {
        std::fs::write(path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("wrote     {} ({})", path.display(), human_bytes(data.len() as u64));
        return Ok(());
    }
    let limit = head.unwrap_or(if print { 4 * 1024 * 1024 } else { 2048 });
    if hexdump_mode || !is_text(&data[..data.len().min(4096)]) {
        print!("{}", hexdump(&data, limit));
    } else if print {
        print!("{}", String::from_utf8_lossy(&data[..data.len().min(limit)]));
        if !data.ends_with(b"\n") {
            println!();
        }
    } else {
        let shown = data.len().min(limit);
        println!("--- first {shown} byte(s) ---");
        print!("{}", String::from_utf8_lossy(&data[..shown]));
        if !data[..shown].ends_with(b"\n") {
            println!();
        }
        if data.len() > shown {
            println!("... {} more bytes (use --print, --hexdump or --to)", data.len() - shown);
        }
    }
    Ok(())
}
