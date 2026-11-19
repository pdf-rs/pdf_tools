//! Document-level metadata, outline, annotations, attachments, filters and
//! full-text search.

use std::path::Path;

use pdf::object::Resolve;
use pdf::primitive::Primitive;

use super::doc::Doc;
use super::fmt::{human_bytes, prim_inline, prim_text, table, truncate};

pub fn meta(doc: &Doc) -> Result<(), String> {
    let file = doc.typed()?;
    let trailer = &file.trailer;
    println!("trailer   /Size {}", trailer.size);
    println!(
        "          /Root {} {} R",
        trailer.root.get_ref().get_inner().id,
        trailer.root.get_ref().get_inner().gen
    );
    if !trailer.id.is_empty() {
        let id: Vec<String> = trailer
            .id
            .iter()
            .map(|s| super::fmt::string_repr(s.as_bytes(), 64))
            .collect();
        println!("          /ID [{}]", id.join(" "));
    }
    if trailer.encrypt_dict.is_some() {
        println!("          /Encrypt present (the file is encrypted)");
    }
    if let Some(p) = trailer.prev_trailer_pos {
        println!("          /Prev {p} (incremental update)");
    }

    if let Some(info) = &trailer.info_dict {
        println!("info");
        for (key, value) in [
            ("Title", info.title.as_ref().map(|s| s.to_string_lossy())),
            ("Author", info.author.as_ref().map(|s| s.to_string_lossy())),
            ("Subject", info.subject.as_ref().map(|s| s.to_string_lossy())),
            ("Keywords", info.keywords.as_ref().map(|s| s.to_string_lossy())),
            ("Creator", info.creator.as_ref().map(|s| s.to_string_lossy())),
            ("Producer", info.producer.as_ref().map(|s| s.to_string_lossy())),
        ] {
            if let Some(v) = value {
                println!("  {key:<10} {}", truncate(&v, 110));
            }
        }
        for (key, date) in [
            ("Created", info.creation_date.as_ref()),
            ("Modified", info.mod_date.as_ref()),
        ] {
            if let Some(d) = date {
                println!("  {key:<10} {:?}", d);
            }
        }
    } else {
        println!("info      none");
    }

    let resolve = doc.resolver()?;
    let catalog_ref = trailer.root.get_ref().get_inner();
    if let Ok(Primitive::Dictionary(catalog)) = doc.resolve(catalog_ref) {
        println!("catalog");
        for key in [
            "Version",
            "PageMode",
            "PageLayout",
            "Lang",
            "OpenAction",
            "ViewerPreferences",
            "AcroForm",
            "Outlines",
            "Names",
            "StructTreeRoot",
            "OutputIntents",
            "OCProperties",
        ] {
            if let Some(v) = catalog.get(key) {
                println!("  /{key:<17} {}", truncate(&prim_inline(v), 100));
            }
        }
        if let Some(Primitive::Reference(r)) = catalog.get("Metadata") {
            match doc.stream_data(*r) {
                Ok((_, data, _)) => {
                    println!("xmp       {} byte(s) in {} {} R", data.len(), r.id, r.gen);
                    let text = String::from_utf8_lossy(&data);
                    let start = text.find("<x:xmpmeta").or_else(|| text.find("<rdf:RDF")).unwrap_or(0);
                    println!("          {}", truncate(text[start..].trim(), 200).replace('\n', " "));
                }
                Err(e) => println!("xmp       {e}"),
            }
        }
        if let Ok(p) = doc.resolve(catalog_ref) {
            if let Primitive::Dictionary(d) = p {
                let text = prim_text(&Primitive::Dictionary(d), &resolve, 0);
                let _ = text;
            }
        }
    }
    Ok(())
}

pub fn outline(doc: &Doc) -> Result<(), String> {
    let trailer = doc.trailer()?;
    let catalog_ref = trailer.root.get_ref().get_inner();
    let catalog = match doc.resolve(catalog_ref)? {
        Primitive::Dictionary(d) => d,
        _ => return Err("the catalog is not a dictionary".into()),
    };
    let outlines_ref = match catalog.get("Outlines") {
        Some(Primitive::Reference(r)) => Some(*r),
        _ => None,
    };
    let outlines_ref = match outlines_ref {
        Some(r) => r,
        None => {
            println!("no outline");
            return Ok(());
        }
    };
    println!("outline at {} {} R", outlines_ref.id, outlines_ref.gen);

    let mut seen = std::collections::HashSet::new();
    seen.insert((outlines_ref.id, outlines_ref.gen));
    let resolve = doc.resolver()?;
    let mut lines = Vec::new();
    let mut count = 0usize;
    walk_outline(&resolve, outlines_ref, 0, &mut seen, &mut count, &mut lines);
    for line in lines {
        println!("{line}");
    }
    println!("({count} item(s))");
    Ok(())
}

fn walk_outline(
    resolve: &impl Resolve,
    node: pdf::object::PlainRef,
    depth: usize,
    seen: &mut std::collections::HashSet<(u64, u64)>,
    count: &mut usize,
    out: &mut Vec<String>,
) {
    if depth > 32 || *count > 5000 {
        return;
    }
    let dict = match resolve.resolve(node) {
        Ok(Primitive::Dictionary(d)) => d,
        _ => return,
    };
    let mut child = match dict.get("First") {
        Some(Primitive::Reference(r)) => Some(*r),
        _ => None,
    };
    while let Some(r) = child {
        if !seen.insert((r.id, r.gen)) || *count > 5000 {
            return;
        }
        *count += 1;
        let d = match resolve.resolve(r) {
            Ok(Primitive::Dictionary(d)) => d,
            _ => return,
        };
        let title = d
            .get("Title")
            .and_then(|t| match t {
                Primitive::String(s) => Some(s.to_string_lossy()),
                _ => None,
            })
            .unwrap_or_else(|| "<no title>".into());
        let mut extra = String::new();
        if let Some(dest) = d.get("Dest") {
            extra.push_str(&format!(" -> {}", truncate(&prim_inline(dest), 40)));
        } else if let Some(action) = d.get("A") {
            if let Ok(Primitive::Dictionary(a)) = action.clone().resolve(resolve) {
                let s = a.get("S").map(prim_inline).unwrap_or_default();
                let uri = a.get("URI").map(prim_inline).unwrap_or_default();
                extra.push_str(&format!(" -> {} {}", s, truncate(&uri, 40)));
            }
        }
        out.push(format!(
            "{}{}  [{} {} R]{}",
            "  ".repeat(depth),
            title,
            r.id,
            r.gen,
            extra
        ));
        walk_outline(resolve, r, depth + 1, seen, count, out);
        child = match d.get("Next") {
            Some(Primitive::Reference(n)) => Some(*n),
            _ => None,
        };
    }
}

pub fn annots(doc: &Doc, page: Option<usize>) -> Result<(), String> {
    let count = doc.page_count()?;
    let pages: Vec<usize> = match page {
        Some(p) => vec![p],
        None => (1..=count as usize).collect(),
    };
    let resolve = doc.resolver()?;
    let mut rows = Vec::new();
    for n in pages {
        let page = doc.page(n)?;
        let reference = page.get_ref();
        let dict = match doc.resolve(reference) {
            Ok(Primitive::Dictionary(d)) => d,
            _ => continue,
        };
        let annots = match dict.get("Annots") {
            Some(p) => p.clone().resolve(&resolve).unwrap_or(Primitive::Null),
            None => continue,
        };
        let list = match annots {
            Primitive::Array(a) => a,
            _ => continue,
        };
        for (i, item) in list.iter().enumerate() {
            let r = match item {
                Primitive::Reference(r) => Some(*r),
                _ => None,
            };
            let d = match item.clone().resolve(&resolve) {
                Ok(Primitive::Dictionary(d)) => d,
                _ => continue,
            };
            let subtype = d
                .get("Subtype")
                .map(prim_inline)
                .unwrap_or_else(|| "?".into());
            let rect = d.get("Rect").map(prim_inline).unwrap_or_default();
            let contents = d
                .get("Contents")
                .and_then(|p| match p {
                    Primitive::String(s) => Some(s.to_string_lossy()),
                    _ => None,
                })
                .unwrap_or_default();
            let mut extra = d
                .get("T")
                .and_then(|p| match p {
                    Primitive::String(s) => Some(format!("field {}", s.to_string_lossy())),
                    _ => None,
                })
                .unwrap_or_default();
            if let Some(Primitive::Dictionary(a)) = d
                .get("A")
                .and_then(|p| p.clone().resolve(&resolve).ok())
            {
                if let Some(uri) = a.get("URI") {
                    extra.push_str(&format!(" uri {}", truncate(&prim_inline(uri), 50)));
                }
            }
            if let Some(flags) = d.get("F") {
                extra.push_str(&format!(" flags {}", prim_inline(flags)));
            }
            rows.push(vec![
                n.to_string(),
                i.to_string(),
                r.map(|r| format!("{} {} R", r.id, r.gen))
                    .unwrap_or_else(|| "inline".into()),
                subtype,
                rect,
                truncate(&contents.replace('\n', " "), 60),
                truncate(&extra, 60),
            ]);
        }
    }
    if rows.is_empty() {
        println!("no annotations");
        return Ok(());
    }
    print!(
        "{}",
        table(
            &["page", "#", "ref", "subtype", "rect", "contents", "other"],
            &rows
        )
    );
    Ok(())
}

pub fn attachments(doc: &Doc, save: Option<&Path>) -> Result<(), String> {
    let trailer = doc.trailer()?;
    let catalog_ref = trailer.root.get_ref().get_inner();
    let catalog = match doc.resolve(catalog_ref)? {
        Primitive::Dictionary(d) => d,
        _ => return Err("the catalog is not a dictionary".into()),
    };
    let resolve = doc.resolver()?;
    let names = match catalog.get("Names") {
        Some(p) => p.clone().resolve(&resolve).map_err(|e| e.to_string())?,
        None => {
            println!("no /Names tree: no embedded files");
            return Ok(());
        }
    };
    let names = match names {
        Primitive::Dictionary(d) => d,
        _ => {
            println!("no /Names tree: no embedded files");
            return Ok(());
        }
    };
    let mut entries = Vec::new();
    if let Some(tree) = names.get("EmbeddedFiles") {
        if let Ok(Primitive::Dictionary(tree)) = tree.clone().resolve(&resolve) {
            walk_name_tree(&resolve, &tree, &mut entries, 0);
        }
    }
    if entries.is_empty() {
        println!("no embedded files");
        return Ok(());
    }

    let mut rows = Vec::new();
    let mut files: Vec<(String, pdf::object::PlainRef, u64)> = Vec::new();
    for (name, spec) in &entries {
        let d = match spec.clone().resolve(&resolve) {
            Ok(Primitive::Dictionary(d)) => d,
            _ => continue,
        };
        let file_name = d
            .get("UF")
            .or_else(|| d.get("F"))
            .and_then(|p| match p {
                Primitive::String(s) => Some(s.to_string_lossy()),
                _ => None,
            })
            .unwrap_or_else(|| name.clone());
        let desc = d
            .get("Desc")
            .and_then(|p| match p {
                Primitive::String(s) => Some(s.to_string_lossy()),
                _ => None,
            })
            .unwrap_or_default();
        let mut size = 0u64;
        let mut stream_ref = None;
        if let Some(Primitive::Dictionary(ef)) = d
            .get("EF")
            .and_then(|p| p.clone().resolve(&resolve).ok())
        {
            if let Some(Primitive::Reference(r)) = ef.get("F") {
                stream_ref = Some(*r);
                if let Ok((_, data, _)) = doc.stream_data(*r) {
                    size = data.len() as u64;
                }
            }
        }
        if let Some(r) = stream_ref {
            files.push((file_name.clone(), r, size));
        }
        rows.push(vec![
            file_name,
            human_bytes(size),
            truncate(&desc, 50),
            stream_ref
                .map(|r| format!("{} {} R", r.id, r.gen))
                .unwrap_or_default(),
        ]);
    }
    print!("{}", table(&["name", "size", "description", "stream"], &rows));

    if let Some(dir) = save {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for (name, r, _) in files {
            let safe: String = name
                .chars()
                .map(|c| if c.is_alphanumeric() || ". _-".contains(c) { c } else { '_' })
                .collect();
            let path = dir.join(if safe.is_empty() { format!("file_{}", r.id) } else { safe });
            match doc.stream_data(r) {
                Ok((_, data, _)) => {
                    std::fs::write(&path, &data)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    println!("wrote {}", path.display());
                }
                Err(e) => println!("{}: {e}", path.display()),
            }
        }
    }
    Ok(())
}

fn walk_name_tree(
    resolve: &impl Resolve,
    node: &pdf::primitive::Dictionary,
    out: &mut Vec<(String, Primitive)>,
    depth: usize,
) {
    if depth > 32 || out.len() > 10000 {
        return;
    }
    if let Some(Primitive::Array(names)) = node.get("Names") {
        for pair in names.chunks(2) {
            if pair.len() == 2 {
                let key = match &pair[0] {
                    Primitive::String(s) => s.to_string_lossy(),
                    other => prim_inline(other),
                };
                out.push((key, pair[1].clone()));
            }
        }
    }
    if let Some(Primitive::Array(kids)) = node.get("Kids") {
        for kid in kids {
            if let Ok(Primitive::Dictionary(d)) = kid.clone().resolve(resolve) {
                walk_name_tree(resolve, &d, out, depth + 1);
            }
        }
    }
}

pub fn filters(doc: &Doc) -> Result<(), String> {
    let known = [
        ("FlateDecode", "inflate (+ PNG/TIFF predictors)"),
        ("LZWDecode", "LZW (+ predictors)"),
        ("ASCIIHexDecode", "hex to bytes"),
        ("ASCII85Decode", "base 85"),
        ("RunLengthDecode", "run length"),
        ("BrotliDecode", "brotli (a PDF extension, read by pdf.js too)"),
        ("CCITTFaxDecode", "CCITT group 3/4 - kept as a .tiff for images"),
        ("DCTDecode", "JPEG - kept as a .jpeg for images"),
        ("JPXDecode", "JPEG 2000 - no decoder in this build"),
        ("JBIG2Decode", "JBIG2 - no decoder in this build"),
        ("Crypt", "identity only (encryption is handled by the library)"),
    ];
    println!("filters this tool knows about:");
    let mut rows = Vec::new();
    for (name, what) in known {
        rows.push(vec![name.to_string(), what.to_string()]);
    }
    print!("{}", table(&["filter", "decoded by"], &rows));

    // Which ones this file actually uses.
    let mut used: std::collections::BTreeMap<String, usize> = Default::default();
    let xref = doc.raw.xref();
    let map = xref.map();
    if let Ok(resolve) = doc.resolver() {
        for (id, (entry, _)) in map.iter() {
            if let Ok(Primitive::Stream(s)) = resolve.resolve(pdf::object::PlainRef {
                id: *id,
                gen: entry.gen(),
            }) {
                for (name, _) in super::raw::filter_specs(&s.info, &resolve) {
                    *used.entry(name).or_insert(0) += 1;
                }
            }
        }
    }
    if !used.is_empty() {
        println!("filters used in {}:", doc.path.display());
        let mut rows = Vec::new();
        for (name, n) in &used {
            rows.push(vec![name.clone(), n.to_string()]);
        }
        print!("{}", table(&["filter", "streams"], &rows));
    }
    Ok(())
}

pub fn search(
    doc: &Doc,
    pattern: &str,
    ignore_case: bool,
    limit: usize,
    context: usize,
) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("empty pattern".into());
    }
    let needle: Vec<u8> = if ignore_case {
        pattern.to_lowercase().into_bytes()
    } else {
        pattern.as_bytes().to_vec()
    };
    let mut shown = 0usize;
    let mut scanned = 0usize;

    // Page text first: it is what a reader would see.
    if let Ok(count) = doc.page_count() {
        if let Ok(resolve) = doc.resolver() {
            for n in 1..=count as usize {
                if shown >= limit {
                    break;
                }
                let Ok(page) = doc.page(n) else { continue };
                let Ok(text) = crate::page_text(&page, &resolve) else {
                    continue;
                };
                if text.is_empty() {
                    continue;
                }
                scanned += 1;
                let bytes = text.as_bytes();
                let data = if ignore_case {
                    text.to_lowercase().into_bytes()
                } else {
                    bytes.to_vec()
                };
                let mut from = 0;
                while let Some(p) = super::raw::find_from(&data, &needle, from) {
                    if shown >= limit {
                        break;
                    }
                    let start = p.saturating_sub(context);
                    let end = (p + needle.len() + context).min(text.len());
                    let snippet = String::from_utf8_lossy(&text.as_bytes()[start..end]);
                    println!("page {n}: ...{}...", snippet.replace('\n', " "));
                    shown += 1;
                    from = p + needle.len().max(1);
                }
            }
        }
    }

    // Then every decoded stream.
    let xref = doc.raw.xref();
    let map = xref.map();
    if let Ok(resolve) = doc.resolver() {
        for (id, (entry, _)) in map.iter() {
            if shown >= limit {
                break;
            }
            let gen = entry.gen();
            let pref = pdf::object::PlainRef { id: *id, gen };
            match resolve.resolve(pref) {
                Ok(Primitive::Stream(s)) => {
                    let Ok(raw) = s.raw_data(&resolve) else { continue };
                    let specs = super::raw::filter_specs(&s.info, &resolve);
                    let Ok((data, _codec)) = super::raw::decode_chain(&raw, &specs, &doc.opts)
                    else {
                        continue;
                    };
                    scanned += 1;
                    if data.len() > 32 * 1024 * 1024 {
                        continue;
                    }
                    let lower;
                    let hay_data: &[u8] = if ignore_case {
                        lower = data.to_ascii_lowercase();
                        &lower
                    } else {
                        &data
                    };
                    let mut from = 0;
                    while let Some(p) = super::raw::find_from(hay_data, &needle, from) {
                        if shown >= limit {
                            break;
                        }
                        let start = p.saturating_sub(context);
                        let end = (p + needle.len() + context).min(data.len());
                        let snippet = String::from_utf8_lossy(&data[start..end]);
                        println!(
                            "obj {id} {gen}: ...{}...",
                            snippet.replace('\n', " ").replace('\r', "")
                        );
                        shown += 1;
                        from = p + needle.len().max(1);
                    }
                }
                Ok(Primitive::Dictionary(d)) => {
                    let text = prim_text(&Primitive::Dictionary(d), &pdf::object::NoResolve, 0);
                    if let Some(p) = super::raw::find_from(
                        &if ignore_case {
                            text.to_lowercase().into_bytes()
                        } else {
                            text.clone().into_bytes()
                        },
                        &needle,
                        0,
                    ) {
                        if shown < limit {
                            println!(
                                "obj {id} {gen}: ...{}...",
                                truncate(text[p.saturating_sub(context)..].lines().next().unwrap_or(""), 120)
                            );
                            shown += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    println!("({shown} hit(s) in {scanned} searched stream(s)/page(s))");
    if shown == 0 {
        println!("note: text inside compressed content streams is searched after decoding; glyphs");
        println!("      are decoded through each font's /ToUnicode or /Encoding, so a match");
        println!("      usually means the page displays that text.");
    }
    Ok(())
}
