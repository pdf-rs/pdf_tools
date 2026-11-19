//! Pages: the page tree, one page in detail, content streams, text.

use pdf::content::{serialize_ops, Op, TextDrawAdjusted};
use pdf::object::PlainRef;
use pdf::primitive::Primitive;

use super::doc::Doc;
use super::fmt::{format_f32, human_bytes, kind_of, prim_inline, prim_text, table, truncate};

/// A page argument: `3`, `last`, or a page object reference.
pub(crate) fn page_ref_of(doc: &Doc, s: &str) -> Result<(usize, PlainRef), String> {
    let s = s.trim();
    if s == "last" {
        let n = doc.page_count()? as usize;
        let refs = doc.page_refs()?;
        let last = *refs.last().ok_or("the document has no pages")?;
        return Ok((n, last));
    }
    if let Ok(n) = s.parse::<usize>() {
        if n == 0 {
            return Err("pages are numbered from 1".into());
        }
        let refs = doc.page_refs()?;
        let r = *refs
            .get(n - 1)
            .ok_or_else(|| format!("page {n} is out of range, there are {} pages", refs.len()))?;
        return Ok((n, r));
    }
    let (id, gen) = super::objects::parse_target(s)?;
    let pref = PlainRef { id, gen };
    let refs = doc.page_refs()?;
    match refs.iter().position(|r| *r == pref) {
        Some(i) => Ok((i + 1, pref)),
        None => Err(format!("{id} {gen} is not a page of this document")),
    }
}

pub fn pages(doc: &Doc) -> Result<(), String> {
    let count = doc.page_count()?;
    let mut rows = Vec::new();
    for n in 1..=count as usize {
        let page = match doc.page(n) {
            Ok(p) => p,
            Err(e) => {
                rows.push(vec![
                    n.to_string(),
                    "?".into(),
                    "?".into(),
                    "?".into(),
                    e,
                    String::new(),
                    String::new(),
                    String::new(),
                ]);
                continue;
            }
        };
        let _ = &page;
        let reference = page.get_ref();
        let (w, h) = match page.media_box() {
            Ok(b) => (b.right - b.left, b.top - b.bottom),
            Err(_) => (0.0, 0.0),
        };
        let contents = page
            .contents
            .as_ref()
            .map(|c| c.parts.len().to_string())
            .unwrap_or_else(|| "-".into());
        let (fonts, xobjects) = match page.resources() {
            Ok(r) => (r.fonts.len().to_string(), r.xobjects.len().to_string()),
            Err(_) => ("?".into(), "?".into()),
        };
        let annots = doc
            .resolve(reference)
            .ok()
            .and_then(|p| match p {
                Primitive::Dictionary(d) => d
                    .get("Annots")
                    .and_then(|a| a.clone().into_array().ok())
                    .map(|a| a.len()),
                _ => None,
            })
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into());
        rows.push(vec![
            n.to_string(),
            format!("{} {}", reference.id, reference.gen),
            format!("{} x {}", format_f32(w), format_f32(h)),
            format!("{:.1} x {:.1}", w / 72.0 * 25.4, h / 72.0 * 25.4),
            format!("{}", page.rotate()),
            contents,
            fonts,
            xobjects,
            annots,
        ]);
    }
    print!(
        "{}",
        table(
            &[
                "page", "ref", "size (pt)", "size (mm)", "rot", "parts", "fonts", "xobj", "annots"
            ],
            &rows
        )
    );
    Ok(())
}

pub fn page(doc: &Doc, which: &str) -> Result<(), String> {
    let (n, pref) = page_ref_of(doc, which)?;
    let page = doc.page(n)?;
    println!("page      {n} = {} {} R", pref.id, pref.gen);

    // The dictionary exactly as it is stored.
    let xref = doc.raw.xref();
    let map = xref.map();
    match map.get(&pref.id).copied() {
        Some((super::raw::Entry::InUse { off, .. }, _)) => {
            println!("stored at offset {off}");
            if let Ok(o) = doc.raw.object_at(off as usize) {
                for p in &o.problems {
                    println!("problem   {p}");
                }
                println!("--- dictionary as stored ---");
                println!("{}", prim_text(&o.prim, &pdf::object::NoResolve, 0));
            }
        }
        Some((super::raw::Entry::Compressed { container, index }, _)) => {
            println!("stored in object stream {container}, index {index}");
            if let Ok(p) = doc.resolve(pref) {
                println!("--- dictionary ---");
                let r = doc.resolver()?;
                println!("{}", prim_text(&p, &r, 0));
            }
        }
        _ => {}
    }

    // What it means.
    println!("--- page ---");
    let (w, h) = match page.media_box() {
        Ok(b) => (b.right - b.left, b.top - b.bottom),
        Err(e) => {
            println!("mediabox  missing: {e}");
            (0.0, 0.0)
        }
    };
    if let Ok(b) = page.media_box() {
        println!(
            "mediabox  [{}, {}] {} x {} pt",
            format_f32(b.left),
            format_f32(b.bottom),
            format_f32(b.right - b.left),
            format_f32(b.top - b.bottom)
        );
    }
    println!(
        "size      {} x {} pt = {:.2} x {:.2} mm, rotation {}",
        format_f32(w),
        format_f32(h),
        w / 72.0 * 25.4,
        h / 72.0 * 25.4,
        page.rotate()
    );
    if let Some(crop) = page.crop_box {
        println!(
            "cropbox   [{}, {}] {} x {} pt",
            format_f32(crop.left),
            format_f32(crop.bottom),
            format_f32(crop.right - crop.left),
            format_f32(crop.top - crop.bottom)
        );
    }
    if let Some(trim) = page.trim_box {
        println!(
            "trimbox   [{}, {}] {} x {} pt",
            format_f32(trim.left),
            format_f32(trim.bottom),
            format_f32(trim.right - trim.left),
            format_f32(trim.top - trim.bottom)
        );
    }

    match &page.contents {
        Some(contents) => {
            println!("contents  {} stream(s)", contents.parts.len());
            let resolve = doc.resolver()?;
            for (i, part) in contents.parts.iter().enumerate() {
                let reference = part.get_ref().get_inner();
                let len = part.data(&resolve).map(|d| d.len()).unwrap_or(0);
                let filters: Vec<String> = part
                    .info
                    .filters
                    .iter()
                    .map(|f| f.name().to_string())
                    .collect();
                println!(
                    "            [{}] {} {} R, {} decoded, filters [{}]",
                    i,
                    reference.id,
                    reference.gen,
                    human_bytes(len as u64),
                    filters.join(", ")
                );
            }
        }
        None => println!("contents  none"),
    }

    match page.resources() {
        Ok(resources) => {
            println!("resources");
            for (name, font) in resources.fonts.iter() {
                let reference = match &font.primitive {
                    Primitive::Reference(r) => format!("{} {} R", r.id, r.gen),
                    other => prim_inline(other),
                };
                let what = match font.load(&doc.resolver()?) {
                    Ok(f) => format!("{:?} {}", f.subtype, f.name.as_ref().map(|n| n.as_str().to_string()).unwrap_or_default()),
                    Err(e) => format!("<broken: {e}>"),
                };
                println!("  /Font    /{} -> {reference}  {what}", name.as_str());
            }
            for (name, reference) in resources.xobjects.iter() {
                let what = match doc.resolve(reference.get_inner()) {
                    Ok(p) => kind_of(&p),
                    Err(e) => format!("<broken: {e}>"),
                };
                println!(
                    "  /XObject /{} -> {} {} R  {what}",
                    name.as_str(),
                    reference.get_inner().id,
                    reference.get_inner().gen
                );
            }
            for (name, _) in resources.graphics_states.iter() {
                println!("  /ExtGState /{}", name.as_str());
            }
            for (name, _) in resources.color_spaces.iter() {
                println!("  /ColorSpace /{}", name.as_str());
            }
        }
        Err(e) => println!("resources missing: {e}"),
    }

    if !page.annotations.primitive.is_null() {
        let annots = doc
            .resolve(pref)
            .ok()
            .and_then(|p| match p {
                Primitive::Dictionary(d) => d.get("Annots").cloned(),
                _ => None,
            });
        if let Some(Primitive::Array(a)) = annots {
            println!("annots    {} annotation(s)", a.len());
            for item in a.iter().take(20) {
                if let Ok(Primitive::Dictionary(d)) = item.clone().resolve(&doc.resolver()?) {
                    let sub = d.get("Subtype").map(prim_inline).unwrap_or_default();
                    let contents = d
                        .get("Contents")
                        .map(prim_inline)
                        .unwrap_or_default();
                    println!("  {sub} {contents}");
                }
            }
        }
    }
    Ok(())
}

fn op_line(op: &Op) -> String {
    match serialize_ops(std::slice::from_ref(op)) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).trim().to_string(),
        Err(e) => format!("<cannot print: {e}>"),
    }
}

pub fn content(
    doc: &Doc,
    target: &str,
    raw: bool,
    ops_count: bool,
    max_ops: usize,
) -> Result<(), String> {
    let resolve = doc.resolver()?;
    let mut sources: Vec<(String, Vec<u8>)> = Vec::new();
    let (id, gen) = match super::objects::parse_target(target) {
        Ok(pair) if !target.trim().chars().all(|c| c.is_ascii_digit()) || pair.1 > 0 => {
            (Some(pair.0), Some(pair.1))
        }
        Ok(pair) => {
            // A bare number is a page number, as everywhere else.
            match page_ref_of(doc, target) {
                Ok(_) => (None, None),
                Err(_) => (Some(pair.0), Some(pair.1)),
            }
        }
        Err(_) => (None, None),
    };
    let _ = (id, gen);

    if target.trim() == "last" || target.trim().parse::<usize>().is_ok() {
        let (n, _) = page_ref_of(doc, target)?;
        let page = doc.page(n)?;
        match &page.contents {
            Some(contents) => {
                for (i, part) in contents.parts.iter().enumerate() {
                    let data = part
                        .data(&resolve)
                        .map_err(|e| format!("content stream {i}: {e}"))?;
                    sources.push((format!("page {n}, part {i}"), data.to_vec()));
                }
            }
            None => return Err(format!("page {n} has no content")),
        }
    } else {
        let (id, gen) = super::objects::parse_target(target)?;
        let (_, data, codec) = doc.stream_data(PlainRef { id, gen })?;
        if let Some(c) = codec {
            return Err(format!("object {id} {gen} is an image stream ({c}), not content"));
        }
        sources.push((format!("object {id} {gen}"), data));
    }

    for (label, data) in sources {
        if raw {
            println!("--- {label}: {} bytes ---", data.len());
            print!("{}", String::from_utf8_lossy(&data));
            if !data.ends_with(b"\n") {
                println!();
            }
            continue;
        }
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        let mut shown = 0usize;
        let mut total = 0usize;
        let mut errors = Vec::new();
        println!("--- {label}: {} bytes ---", data.len());
        for item in pdf::content::parse_ops_iter(&data, &resolve) {
            total += 1;
            let op = match item {
                Ok(op) => op,
                Err(e) => {
                    errors.push(e.to_string());
                    break;
                }
            };
            let line = op_line(&op);
            let name = line.split_whitespace().last().unwrap_or("?").to_string();
            *counts.entry(name.clone()).or_insert(0) += 1;
            if !ops_count && shown < max_ops {
                println!("{total:6}  {}", truncate(&line, 160));
                shown += 1;
            }
            if let Op::InlineImage { image } = &op {
                let _ = image;
            }
        }
        if ops_count {
            let mut rows = Vec::new();
            for (name, n) in &counts {
                rows.push(vec![name.clone(), n.to_string()]);
            }
            print!("{}", table(&["operator", "count"], &rows));
        } else if total > shown {
            println!("... {} more operator(s)", total - shown);
        }
        println!("{total} operator(s)");
        for e in errors {
            println!("problem: {e}");
        }
    }
    Ok(())
}

pub fn text(doc: &Doc, which: &str, raw: bool, boxes: bool) -> Result<(), String> {
    let (n, _) = page_ref_of(doc, which)?;
    let page = doc.page(n)?;
    let resolve = doc.resolver()?;

    if boxes {
        // One row per shown string: where it starts, how wide the font says it
        // is, and how far the next string actually is from the predicted end.
        // A non-zero `dx` is the kerning check: it means the advance computed
        // from the font's widths does not account for the layout.
        let mut rows = Vec::new();
        let mut predicted: Option<f32> = None;
        let mut flagged = 0usize;
        let mut runs = 0usize;
        for (op, state) in crate::ops_with_text_state(&page, &resolve) {
            let mut s = String::new();
            let mut advance = 0.0f32;
            match &op {
                Op::TextDraw { text } => {
                    let _ = state.font.decode(&text.data, &mut s);
                    advance += state.font.advance(&text.data, state.font_size);
                }
                Op::TextDrawAdjusted { array } => {
                    for part in array {
                        match part {
                            TextDrawAdjusted::Text(t) => {
                                let _ = state.font.decode(&t.data, &mut s);
                                advance += state.font.advance(&t.data, state.font_size);
                            }
                            TextDrawAdjusted::Spacing(w) => {
                                // TJ moves back by -w/1000 em between strings.
                                advance += -w / 1000.0 * state.font_size;
                                if *w < -180.0 {
                                    s.push(' ');
                                }
                            }
                        }
                    }
                }
                Op::MoveTextPosition { .. } | Op::SetTextMatrix { .. } | Op::TextNewline => {
                    predicted = None;
                    continue;
                }
                _ => continue,
            }
            if s.is_empty() && advance.abs() < 0.01 {
                continue;
            }
            runs += 1;
            let x = state.text_matrix.m31;
            let dx = predicted.map(|p| x - p);
            if let Some(d) = dx {
                if d.abs() > 1.0 {
                    flagged += 1;
                }
            }
            rows.push(vec![
                format!("{:.1}", x),
                format!("{:.1}", state.text_matrix.m32),
                format!("{:.1}", state.font_size),
                format!("{:.1}", advance),
                match dx {
                    Some(d) if d.abs() > 1.0 => format!("{d:+.1} !"),
                    Some(d) => format!("{d:+.1}"),
                    None => "-".into(),
                },
                super::fmt::string_repr(s.as_bytes(), 200),
            ]);
            predicted = Some(x + advance);
        }
        print!(
            "{}",
            table(&["x", "y", "size", "advance", "dx", "text"], &rows)
        );
        if flagged > 0 {
            println!(
                "{flagged} of {runs} run(s) do not start where the previous run's advance says they should"
            );
        }
        return Ok(());
    }

    let mut out = String::new();
    for (op, state) in crate::ops_with_text_state(&page, &resolve) {
        match &op {
            Op::TextDraw { text } => {
                let _ = state.font.decode(&text.data, &mut out);
            }
            Op::TextDrawAdjusted { array } => {
                for part in array {
                    match part {
                        TextDrawAdjusted::Text(t) => {
                            let _ = state.font.decode(&t.data, &mut out);
                        }
                        TextDrawAdjusted::Spacing(w) => {
                            if *w < -180.0 && !raw {
                                out.push(' ');
                            }
                        }
                    }
                }
            }
            Op::TextNewline => {
                if !raw {
                    out.push('\n');
                }
            }
            Op::MoveTextPosition { translation } => {
                if !raw && translation.y.abs() > 0.01 {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
    print!("{out}");
    if !out.ends_with('\n') {
        println!();
    }
    Ok(())
}
