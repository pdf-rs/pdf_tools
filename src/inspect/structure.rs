//! Commands that look at the file as a whole: info, xref, objects, scan,
//! verify.

use std::collections::{BTreeMap, BTreeSet};

use pdf::object::Resolve;
use pdf::primitive::Primitive;

use super::doc::Doc;
use super::fmt::{human_bytes, kind_of, prim_inline, prim_text, table, truncate};
use super::raw::Entry;

pub fn info(doc: &Doc) -> Result<(), String> {
    let raw = &doc.raw;
    let mut out = String::new();
    out.push_str(&format!("file      {}\n", doc.path.display()));
    out.push_str(&format!(
        "size      {} ({} bytes)\n",
        human_bytes(raw.len() as u64),
        raw.len()
    ));

    match raw.header() {
        Some((0, v)) => out.push_str(&format!("header    %PDF-{v}\n")),
        Some((off, v)) => out.push_str(&format!(
            "header    %PDF-{v} at offset {off} ({} bytes before it)\n",
            off
        )),
        None => out.push_str("header    missing — no %PDF- in the file\n"),
    }

    let eofs = raw.eof_positions();
    match eofs.last() {
        Some(&last) => {
            let after = &raw.data[last + 5..];
            let junk = after.iter().filter(|b| !b.is_ascii_whitespace()).count();
            out.push_str(&format!(
                "eof       {} %%EOF marker(s), last at {}; {} non-space byte(s) after it\n",
                eofs.len(),
                last,
                junk
            ));
        }
        None => out.push_str("eof       no %%EOF marker\n"),
    }

    let xref = raw.xref();
    let entries: u64 = xref
        .sections
        .iter()
        .map(|s| s.num_entries() as u64)
        .sum();
    let kinds: Vec<&str> = xref.sections.iter().map(|s| s.kind).collect();
    out.push_str(&format!(
        "xref      {} section(s) [{}], {} entries, {} problem(s)\n",
        xref.sections.len(),
        if kinds.is_empty() {
            "none".to_string()
        } else {
            kinds.join(", ")
        },
        entries,
        xref.problems.len()
    ));
    let map = xref.map();
    let in_use = map
        .values()
        .filter(|(e, _)| matches!(e, Entry::InUse { .. }))
        .count();
    let compressed = map
        .values()
        .filter(|(e, _)| matches!(e, Entry::Compressed { .. }))
        .count();
    let free = map
        .values()
        .filter(|(e, _)| matches!(e, Entry::Free { .. }))
        .count();
    out.push_str(&format!(
        "objects   {in_use} in use, {compressed} in object streams, {free} free, {} entr(ies) total\n",
        map.len()
    ));

    if let Some((_, v)) = raw.startxref() {
        let at_obj = if v >= raw.len() {
            "past the end of the file".to_string()
        } else if raw.data[v..].starts_with(b"xref") {
            "classic xref table".to_string()
        } else {
            match raw.object_at(v) {
                Ok(o) => format!("object {} {} (an xref stream)", o.id, o.gen),
                Err(e) => format!("neither a table nor an object: {e}"),
            }
        };
        out.push_str(&format!("startxref {v} -> {at_obj}\n"));
    }

    if doc.is_encrypted() {
        out.push_str("encrypted yes\n");
    }
    if let Ok(file) = doc.typed() {
        out.push_str(&format!("trailer   /Size {}", file.trailer.size));
        out.push_str(&format!(" /Root {} {} R", file.trailer.root.get_ref().get_inner().id, file.trailer.root.get_ref().get_inner().gen));
        if let Some(p) = file.trailer.prev_trailer_pos {
            out.push_str(&format!(" /Prev {p}"));
        }
        if !file.trailer.id.is_empty() {
            let id: Vec<String> = file
                .trailer
                .id
                .iter()
                .map(|s| super::fmt::string_repr(s.as_bytes(), 48))
                .collect();
            out.push_str(&format!(" /ID [{}]", id.join(" ")));
        }
        out.push('\n');
        match doc.page_count() {
            Ok(n) => out.push_str(&format!("pages     {n}\n")),
            Err(e) => out.push_str(&format!("pages     unusable page tree: {e}\n")),
        }
        if let Some(info) = &file.trailer.info_dict {
            for (label, value) in [
                ("Title", info.title.as_ref().map(|s| s.to_string_lossy())),
                ("Author", info.author.as_ref().map(|s| s.to_string_lossy())),
                ("Subject", info.subject.as_ref().map(|s| s.to_string_lossy())),
                ("Creator", info.creator.as_ref().map(|s| s.to_string_lossy())),
                (
                    "Producer",
                    info.producer.as_ref().map(|s| s.to_string_lossy()),
                ),
            ] {
                if let Some(v) = value {
                    out.push_str(&format!("{:<9} {}\n", label.to_lowercase(), truncate(&v, 90)));
                }
            }
        }
        let root = file.get_root();
        if let Some(forms) = &root.forms {
            out.push_str(&format!("forms     {} field(s)\n", forms.fields.len()));
        }
        let catalog_ref = file.trailer.root.get_ref().get_inner();
        if let Ok(Primitive::Dictionary(catalog)) = doc.resolve(catalog_ref) {
            if let Some(Primitive::Reference(r)) = catalog.get("Metadata") {
                let size = doc.stream_data(*r).map(|(_, d, _)| d.len()).unwrap_or(0);
                out.push_str(&format!("xmp       {} bytes in {} {} R\n", size, r.id, r.gen));
            }
            if catalog.get("Names").is_some() {
                out.push_str("names     /Names tree present (see `attachments`)\n");
            }
        }
    } else if let Some(e) = &doc.open_error {
        out.push_str(&format!("typed     unusable: {e}\n"));
    }

    for p in xref.problems.iter().take(8) {
        out.push_str(&format!("problem   {p}\n"));
    }
    for sec in &xref.sections {
        for p in sec.problems.iter().take(4) {
            out.push_str(&format!("problem   xref at {}: {p}\n", sec.offset));
        }
    }
    print!("{out}");
    Ok(())
}

pub fn xref(doc: &Doc, entries: bool, free: bool, trailers: bool) -> Result<(), String> {
    let xref = doc.raw.xref();
    println!(
        "{} cross-reference section(s), newest first",
        xref.sections.len()
    );
    for (i, sec) in xref.sections.iter().enumerate() {
        println!(
            "\nsection {} at offset {} ({})",
            i + 1,
            sec.offset,
            sec.kind
        );
        if let Some(p) = sec.prev {
            println!("  /Prev {p}");
        }
        if let Some(p) = sec.xrefstm {
            println!("  /XRefStm {p} (hybrid file: the entries of that stream are used too)");
        }
        for key in ["Size", "Root", "Info", "Encrypt", "ID"] {
            if let Some(v) = sec.trailer.get(key) {
                println!("  /{key} {}", prim_inline(v));
            }
        }
        let mut rows = Vec::new();
        for (first, list) in &sec.subsections {
            let last = first + list.len() as u64;
            let mut n = 0;
            let mut f = 0;
            let mut c = 0;
            for e in list {
                match e {
                    Entry::InUse { .. } => n += 1,
                    Entry::Free { .. } => f += 1,
                    Entry::Compressed { .. } => c += 1,
                }
            }
            rows.push(vec![
                format!("{first}..{}", last.saturating_sub(1)),
                format!("{}", list.len()),
                format!("{n}"),
                format!("{f}"),
                format!("{c}"),
            ]);
        }
        println!(
            "  subsections (object range, entries, in use, free, compressed):"
        );
        print!("{}", table(&["objects", "n", "in-use", "free", "cmp"], &rows));

        if entries {
            let mut rows = Vec::new();
            for (first, list) in &sec.subsections {
                for (j, e) in list.iter().enumerate() {
                    let id = first + j as u64;
                    match e {
                        Entry::InUse { off, gen } => rows.push(vec![
                            id.to_string(),
                            "in use".into(),
                            gen.to_string(),
                            format!("offset {off}"),
                        ]),
                        Entry::Free { next, gen } => {
                            if free {
                                rows.push(vec![
                                    id.to_string(),
                                    "free".into(),
                                    gen.to_string(),
                                    format!("next free {next}"),
                                ]);
                            }
                        }
                        Entry::Compressed { container, index } => rows.push(vec![
                            id.to_string(),
                            "compressed".into(),
                            "0".into(),
                            format!("object stream {container}, index {index}"),
                        ]),
                    }
                }
            }
            print!("{}", table(&["id", "type", "gen", "where"], &rows));
        }
        if trailers {
            println!("  trailer:");
            let r = doc.resolver();
            let text = match &r {
                Ok(res) => prim_text(&Primitive::Dictionary(sec.trailer.clone()), res, 0),
                Err(_) => prim_text(
                    &Primitive::Dictionary(sec.trailer.clone()),
                    &pdf::object::NoResolve,
                    0,
                ),
            };
            for line in text.lines() {
                println!("    {line}");
            }
        }
        for p in &sec.problems {
            println!("  problem: {p}");
        }
    }
    for p in &xref.problems {
        println!("\nproblem: {p}");
    }
    Ok(())
}

pub fn objects(
    doc: &Doc,
    ty: Option<&str>,
    has_key: &[String],
    streams_only: bool,
    sort: &str,
    limit: Option<usize>,
) -> Result<(), String> {
    let xref = doc.raw.xref();
    let map = xref.map();
    if map.is_empty() {
        return Err("no cross-reference information; try `scan`".into());
    }
    let resolve = doc.resolver()?;
    let needle = ty.map(|t| t.to_lowercase());

    struct Row {
        id: u64,
        gen: u64,
        where_: String,
        type_: String,
        len: u64,
        filters: String,
        has_stream: bool,
    }
    let mut rows: Vec<Row> = Vec::new();
    for (id, (entry, _)) in &map {
        if matches!(entry, Entry::Free { .. }) {
            continue;
        }
        let gen = entry.gen();
        let pref = pdf::object::PlainRef { id: *id, gen };
        // Two views: the library's (which handles object streams and
        // encryption) and the file's own bytes (which say where it is).
        let at = match entry {
            Entry::InUse { off, .. } => doc.raw.object_at(*off as usize).ok(),
            _ => None,
        };
        let where_ = match entry {
            Entry::InUse { off, .. } => match &at {
                Some(o) if o.id == *id => format!("{off}"),
                Some(o) => format!("{off} (!object {} {})", o.id, o.gen),
                None => format!("{off} (!)"),
            },
            Entry::Compressed { container, index } => format!("objstm {container}[{index}]"),
            Entry::Free { .. } => unreachable!(),
        };
        let prim = match resolve.resolve(pref) {
            Ok(p) => Ok(p),
            Err(e) => match &at {
                Some(o) => Ok(o.prim.clone()),
                None => Err(e),
            },
        };

        let is_stream = matches!(&prim, Ok(Primitive::Stream(_)))
            || at.as_ref().map(|o| o.stream.is_some()).unwrap_or(false);
        let dict = match &prim {
            Ok(Primitive::Stream(s)) => Some(s.info.clone()),
            _ => at.as_ref().and_then(|o| o.dict().cloned()),
        };
        let (len, filters) = match &dict {
            Some(d) => {
                let specs = super::raw::filter_specs(d, &resolve);
                let names: Vec<String> = specs.iter().map(|(n, _)| n.clone()).collect();
                let declared = d.get("Length").and_then(|p| p.as_integer().ok());
                let actual = at.as_ref().and_then(|o| o.stream.as_ref()).map(|s| s.len());
                let len = match (declared, actual) {
                    (Some(n), _) if n >= 0 => n as u64,
                    (_, Some(n)) => n as u64,
                    _ => 0,
                };
                (len, names.join(","))
            }
            None => (0, String::new()),
        };
        let mut type_ = match &prim {
            Ok(p) => kind_of(p),
            Err(e) => format!("? {}", truncate(&e.to_string(), 30)),
        };
        if is_stream && type_ == "dict" {
            type_ = "stream".into();
        }

        if let Some(ref needle) = needle {
            if !type_.to_lowercase().contains(needle.as_str()) {
                continue;
            }
        }
        if streams_only && !is_stream {
            continue;
        }
        if !has_key.is_empty() {
            let ok = dict
                .as_ref()
                .map(|d| has_key.iter().all(|k| d.get(k).is_some()))
                .unwrap_or(false);
            if !ok {
                continue;
            }
        }
        rows.push(Row {
            id: *id,
            gen,
            where_,
            type_,
            len,
            filters,
            has_stream: is_stream,
        });
    }

    match sort {
        "size" => rows.sort_by(|a, b| b.len.cmp(&a.len).then(a.id.cmp(&b.id))),
        "type" => rows.sort_by(|a, b| a.type_.cmp(&b.type_).then(a.id.cmp(&b.id))),
        _ => rows.sort_by_key(|r| r.id),
    }
    let total = rows.len();
    if let Some(n) = limit {
        rows.truncate(n);
    }
    let mut table_rows = Vec::new();
    for r in &rows {
        table_rows.push(vec![
            r.id.to_string(),
            r.gen.to_string(),
            r.where_.clone(),
            r.type_.clone(),
            if r.has_stream {
                human_bytes(r.len)
            } else {
                String::new()
            },
            r.filters.clone(),
        ]);
    }
    print!(
        "{}",
        table(
            &["id", "gen", "where", "type", "stream", "filters"],
            &table_rows
        )
    );
    if total > rows.len() {
        println!("({} of {total} object(s))", rows.len());
    } else {
        println!("({total} object(s))");
    }
    Ok(())
}

pub fn scan(doc: &Doc) -> Result<(), String> {
    let raw = &doc.raw;
    let (objs, gaps) = raw.walk();
    let xref = raw.xref();
    let map = xref.map();

    if let Some((0, _)) = raw.header() {
        println!("header    at offset 0");
    } else if let Some((off, v)) = raw.header() {
        println!("header    %PDF-{v} at offset {off}, {} byte(s) of junk before it", off);
        println!("          {:?}", String::from_utf8_lossy(&raw.data[..off.min(200)]));
    } else {
        println!("header    missing");
    }

    println!(
        "objects   {} found by scanning the body, {} in the xref table",
        objs.len(),
        map.len()
    );
    println!(
        "          {} byte(s) not part of any object, in {} gap(s)",
        gaps.iter().map(|g| g.end - g.start).sum::<usize>(),
        gaps.len()
    );

    // Objects in the file that the xref does not mention.
    let mut orphans = Vec::new();
    for o in &objs {
        match map.get(&o.id) {
            None => orphans.push(format!("{} {} at {}", o.id, o.gen, o.start)),
            Some((Entry::InUse { off, .. }, _)) if *off as usize != o.start => orphans.push(format!(
                "{} {} at {} (xref says {off})",
                o.id, o.gen, o.start
            )),
            Some((Entry::Compressed { .. }, _)) => orphans.push(format!(
                "{} {} at {} (xref says it is compressed)",
                o.id, o.gen, o.start
            )),
            _ => {}
        }
    }
    println!("orphans   {} marker(s) the xref does not point at", orphans.len());
    for line in orphans.iter().take(20) {
        println!("          {line}");
    }

    let mut dangling = Vec::new();
    for (id, (entry, _)) in &map {
        if let Entry::InUse { off, gen } = entry {
            match raw.object_at(*off as usize) {
                Ok(o) if o.id == *id => {}
                Ok(o) => dangling.push(format!("{id} {gen}: offset {off} holds object {} {}", o.id, o.gen)),
                Err(e) => dangling.push(format!("{id} {gen}: offset {off}: {e}")),
            }
        }
    }
    println!("dangling  {} xref entr(ies) that point at nothing", dangling.len());
    for line in dangling.iter().take(20) {
        println!("          {line}");
    }

    let mut lengths = Vec::new();
    for o in &objs {
        for p in &o.problems {
            lengths.push(format!("{} {} at {}: {p}", o.id, o.gen, o.start));
        }
    }
    println!("length    {} stream(s) whose /Length disagrees with the data", lengths.len());
    for line in lengths.iter().take(10) {
        println!("          {line}");
    }

    if !gaps.is_empty() {
        println!("gaps:");
        for g in gaps.iter().take(10) {
            let snippet = &raw.data[g.start..(g.start + 40).min(g.end)];
            println!(
                "          {}..{} ({} bytes) {:?}",
                g.start,
                g.end,
                g.end - g.start,
                String::from_utf8_lossy(snippet)
            );
        }
    }
    Ok(())
}

pub fn verify(doc: &Doc) -> Result<(), String> {
    let raw = &doc.raw;
    let mut issues: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    match raw.header() {
        Some((0, _)) => {}
        Some((off, _)) => issues.push(format!("%PDF- header is at offset {off}, not 0")),
        None => issues.push("no %PDF- header".into()),
    }
    let eofs = raw.eof_positions();
    if eofs.is_empty() {
        issues.push("no %%EOF marker".into());
    } else {
        let last = *eofs.last().unwrap();
        let junk: Vec<&u8> = raw.data[last + 5..]
            .iter()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        if !junk.is_empty() {
            issues.push(format!(
                "{} non-space byte(s) after the last %%EOF",
                junk.len()
            ));
        }
        if eofs.len() > 1 {
            notes.push(format!("{} %%EOF markers: the file has been updated in place", eofs.len()));
        }
    }

    let xref = raw.xref();
    for p in &xref.problems {
        issues.push(format!("xref: {p}"));
    }
    for sec in &xref.sections {
        for p in &sec.problems {
            issues.push(format!("xref at {}: {p}", sec.offset));
        }
    }
    let map = xref.map();

    // /Size against the largest object number seen anywhere.
    let mut max_id = 0u64;
    for id in map.keys() {
        max_id = max_id.max(*id);
    }
    if let Some(sec) = xref.sections.first() {
        if let Some(size) = sec.trailer.get("Size").and_then(|p| p.as_i64().ok()) {
            if (size as u64) <= max_id {
                issues.push(format!(
                    "/Size is {size}, but object {max_id} exists; /Size must be larger than every object number"
                ));
            }
        } else {
            issues.push("the newest trailer has no /Size".into());
        }
    }

    // xref entries and their objects; collect references on the way.
    let (objs, _) = raw.walk();
    let mut referenced: BTreeSet<u64> = BTreeSet::new();
    let mut length_problems = 0;
    for o in &objs {
        if !o.problems.is_empty() {
            length_problems += 1;
        }
        collect_refs(&o.prim, &mut referenced);
    }
    if length_problems > 0 {
        issues.push(format!(
            "{length_problems} stream(s) whose data does not end where /Length says"
        ));
    }

    // Dangling references.
    let mut missing: BTreeMap<u64, u32> = BTreeMap::new();
    for id in &referenced {
        match map.get(id) {
            Some((Entry::Free { .. }, _)) | None => *missing.entry(*id).or_insert(0) += 1,
            _ => {}
        }
    }
    if !missing.is_empty() {
        let list: Vec<String> = missing
            .iter()
            .take(12)
            .map(|(id, n)| format!("{id} ({n}x)"))
            .collect();
        issues.push(format!(
            "{} object number(s) referenced but not present: {}",
            missing.len(),
            list.join(", ")
        ));
    }

    // Streams that do not decode.
    let resolve = doc.resolver();
    let mut decode_failures = 0;
    let mut decode_examples = Vec::new();
    if let Ok(resolve) = &resolve {
        for (id, (entry, _)) in map.iter().take(20000) {
            let gen = entry.gen();
            if let Ok(Primitive::Stream(s)) =
                resolve.resolve(pdf::object::PlainRef { id: *id, gen })
            {
                let specs = super::raw::filter_specs(&s.info, resolve);
                if specs.is_empty() {
                    continue;
                }
                let raw_bytes = match s.raw_data(resolve) {
                    Ok(d) => d,
                    Err(e) => {
                        decode_failures += 1;
                        if decode_examples.len() < 5 {
                            decode_examples.push(format!("{id}: data: {e}"));
                        }
                        continue;
                    }
                };
                if let Err(e) = super::raw::decode_chain(&raw_bytes, &specs, &doc.opts) {
                    decode_failures += 1;
                    if decode_examples.len() < 5 {
                        decode_examples.push(format!("{id}: {e}"));
                    }
                }
            }
        }
    }
    if decode_failures > 0 {
        issues.push(format!("{decode_failures} stream(s) do not decode"));
        for e in decode_examples {
            issues.push(format!("  {e}"));
        }
    }

    // Page tree.
    if let Ok(n) = doc.page_count() {
        let page_objects = objs
            .iter()
            .filter(|o| {
                o.dict()
                    .and_then(|d| d.get("Type"))
                    .map(|t| prim_inline(t) == "/Page")
                    .unwrap_or(false)
            })
            .count();
        if page_objects != 0 && page_objects != n as usize {
            notes.push(format!(
                "the page tree yields {n} page(s); {page_objects} object(s) in the body have /Type /Page"
            ));
        }
        notes.push(format!("{n} page(s)"));
    } else if let Err(e) = doc.page_count() {
        issues.push(format!("page tree: {e}"));
    }

    // Encryption.
    if doc.is_encrypted() {
        notes.push("the file is encrypted".into());
    }

    println!("{} issue(s), {} note(s)", issues.len(), notes.len());
    for i in &issues {
        println!("issue     {i}");
    }
    for n in &notes {
        println!("note      {n}");
    }
    Ok(())
}

fn collect_refs(p: &Primitive, out: &mut BTreeSet<u64>) {
    match p {
        Primitive::Reference(r) => {
            out.insert(r.id);
        }
        Primitive::Array(a) => {
            for i in a {
                collect_refs(i, out);
            }
        }
        Primitive::Dictionary(d) => {
            for (_, v) in d.iter() {
                collect_refs(v, out);
            }
        }
        Primitive::Stream(s) => {
            for (_, v) in s.info.iter() {
                collect_refs(v, out);
            }
        }
        _ => {}
    }
}
