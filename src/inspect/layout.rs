//! Text layout exactly as the renderer computes it.
//!
//! `text` walks the operators itself and adds up advances. This drives
//! `pdf_render`'s tracer over the page instead — the same `render_page` call
//! the viewer makes, with a tracer in place of a rasteriser — so what it
//! prints is what will be drawn: widths from `/Widths`, or from the font
//! program when the table is missing, with Tc/Tw/Tz, TJ shifts, text rise,
//! the text matrix, the graphics transform and `/Rotate` all applied.
//!
//! Coordinates are PDF points from the top-left corner of the page as it is
//! displayed: the ruler a screenshot and a tap position speak.

use std::path::PathBuf;
use std::sync::Arc;

use font::Font;
use pathfinder_geometry::vector::vec2f;
use pdf_render::tracer::{DrawItem, NullEncoder, TraceCache, Tracer};
use pdf_render::{render_page, FontEntry, TextSpan, Transform2F};

use super::doc::Doc;
use super::fmt::{table, truncate};
use super::pages::page_ref_of;

/// 1 / (25.4 / 72): the tracer works in millimetres, this tool in points.
const PT_PER_MM: f32 = 72.0 / 25.4;

pub struct Opts {
    pub chars: bool,
    pub check: bool,
    pub max_chars: usize,
}

/// `pdf_render` takes the standard fonts from `$STANDARD_FONTS` and panics if
/// it is unset. Point it at a directory with a `fonts.json` when the caller
/// has not; return what was used.
fn ensure_standard_fonts() -> Option<String> {
    if std::env::var_os("STANDARD_FONTS").is_some() {
        return std::env::var("STANDARD_FONTS").ok();
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("PDF_STANDARD_FONTS") {
        candidates.push(PathBuf::from(p));
    }
    for p in [
        "/home/hematite/fonts/pdf_fonts/pdf_fonts",
        "/home/hematite/fonts/pdf_fonts_otf/pdf_fonts",
        "/home/hematite/task-clones/pdf_render/fonts",
        "/usr/share/fonts/type1/urw-base35",
    ] {
        candidates.push(PathBuf::from(p));
    }
    let dir = candidates
        .into_iter()
        .find(|p| p.join("fonts.json").is_file())?;
    std::env::set_var("STANDARD_FONTS", &dir);
    Some(dir.display().to_string())
}

/// The character code a glyph in the span was selected by.
fn cid_at(bytes: &[u8], idx: usize, is_cid: bool) -> Option<u16> {
    if is_cid {
        let a = *bytes.get(idx)?;
        let b = *bytes.get(idx + 1)?;
        Some(u16::from_be_bytes([a, b]))
    } else {
        bytes.get(idx).map(|&b| b as u16)
    }
}

/// The affine map of a transform, spelled out: pathfinder's `Transform2F` is a
/// row-vector matrix (`p' = p * M`), and has no `transform_point` of its own.
fn apply(
    m: &Transform2F,
    p: pathfinder_geometry::vector::Vector2F,
) -> pathfinder_geometry::vector::Vector2F {
    vec2f(
        m.m11() * p.x() + m.m21() * p.y() + m.m13(),
        m.m12() * p.x() + m.m22() * p.y() + m.m23(),
    )
}

/// Text as text: the layout already holds decoded characters, so show them,
/// not the PDF string syntax `text` prints.
fn show_text(s: &str, limit: usize) -> String {
    let mut out = String::from("\"");
    let mut n = 0usize;
    for c in s.chars() {
        if n >= limit {
            out.push('\u{2026}');
            break;
        }
        n += 1;
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Line {
    span: TextSpan<NullEncoder>,
    x: f32,
    y: f32,
    /// Text space -> page points along the baseline.
    scale: f32,
}

pub fn layout(doc: &Doc, which: &str, opts: &Opts) -> Result<(), String> {
    let (n, _) = page_ref_of(doc, which)?;
    let page = doc.page(n)?;
    let resolve = doc.resolver()?;

    let fonts_dir = ensure_standard_fonts();
    if fonts_dir.is_none() && opts.check {
        eprintln!(
            "note: no $STANDARD_FONTS directory found; the base-14 fonts cannot be \
             loaded, and a page that relies on them shows no glyphs here"
        );
    }

    let mut cache = TraceCache::<NullEncoder>::new();
    let mut clip_paths = Vec::new();
    let mut tracer = Tracer::new(&mut cache, &mut clip_paths);
    // Undo the tracer's millimetre scale: the transform is applied before the
    // page's own pt -> mm step, so the numbers come out in points.
    let transform = Transform2F::from_scale(vec2f(PT_PER_MM, PT_PER_MM));
    render_page(&mut tracer, &resolve, &page, transform)
        .map_err(|e| format!("page {n}: the renderer stopped: {e}"))?;

    let mut lines: Vec<Line> = Vec::new();
    for item in tracer.finish() {
        if let DrawItem::Text(span, _clip) = item {
            // A `Tj` with no font set draws nothing, and the tracer still
            // records the empty span: not a finding about the page.
            if span.bytes.is_empty() && span.text.is_empty() {
                continue;
            }
            let p = apply(&span.transform, vec2f(0.0, 0.0));
            let scale = (span.transform.m11() * span.transform.m11()
                + span.transform.m12() * span.transform.m12())
            .sqrt();
            lines.push(Line {
                span,
                x: p.x(),
                y: p.y(),
                scale,
            });
        }
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    for i in 0..lines.len() {
        let l = &lines[i];
        let span = &l.span;
        let width_pt = span.width * l.scale;
        // Where the next span on this baseline starts, relative to the end of
        // this one: a TJ shift or a following Td, in points.
        let next = lines.iter().skip(i + 1).find(|o| (o.y - l.y).abs() < 0.5);
        let gap = next
            .map(|o| format!("{:+.2}", o.x - (l.x + width_pt)))
            .unwrap_or_else(|| "-".into());
        let font = match &span.font {
            Some(f) => truncate(&f.name, 28),
            None => "<no font>".into(),
        };
        rows.push(vec![
            span.op_nr.to_string(),
            format!("{:.2}", l.x),
            format!("{:.2}", l.y),
            format!("{:.2}", span.font_size),
            format!("{:.2}", width_pt),
            gap,
            font,
            show_text(&span.text, 120),
        ]);
    }

    let total_chars: usize = lines.iter().map(|l| l.span.chars.len()).sum();
    println!(
        "page {n}: {} text span(s), {total_chars} character(s)",
        lines.len()
    );
    if let Some(d) = &fonts_dir {
        println!("standard fonts: {d}");
    }
    print!(
        "{}",
        table(
            &["op", "x", "y", "size", "width", "next", "font", "text"],
            &rows
        )
    );

    let mut kerned = 0usize;
    let mut notdef = 0usize;
    let mut nofont = 0usize;
    let mut mismatched = 0usize;
    let mut char_rows: Vec<Vec<String>> = Vec::new();
    let mut shown = 0usize;

    for (i, l) in lines.iter().enumerate() {
        let span = &l.span;
        let font: Option<&Arc<FontEntry<NullEncoder>>> = span.font.as_ref();
        if font.is_none() {
            nofont += 1;
        }
        let n_chars = span.chars.len();

        // Advances must be even inside one span: the same Tc applies to every
        // glyph, so the gap after each one is the same unless a TJ shift sits
        // between them — and a shift inside one string is always a bug.
        let mut gaps: Vec<f32> = Vec::new();
        for c in 0..n_chars.saturating_sub(1) {
            let a = span.chars[c];
            let b = span.chars[c + 1];
            let space =
                a.offset < span.text.len() && span.text[a.offset..].chars().next() == Some(' ');
            if space {
                continue; // Tw applies to this one only
            }
            gaps.push(b.pos - (a.pos + a.width));
        }
        let (lo, hi) = (
            gaps.iter().cloned().fold(f32::INFINITY, f32::min),
            gaps.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        );
        let spread = if lo.is_finite() && hi.is_finite() {
            hi - lo
        } else {
            0.0
        };
        if spread > 0.002 {
            kerned += 1;
        }

        for (k, c) in span.chars.iter().enumerate() {
            let text: String = span.text[c.offset..].chars().take(1).collect();
            let is_cid = font.map(|f| f.is_cid).unwrap_or(false);
            let (gid, wem) = match (font, cid_at(&span.bytes, c.idx, is_cid)) {
                (Some(f), Some(cid)) => (
                    f.cmap.get(&cid).map(|(gid, _)| *gid),
                    f.widths.as_ref().map(|w| w.get(cid as usize) * 0.001),
                ),
                _ => (None, None),
            };
            let has_glyph = gid.map(|g| g.0 != 0).unwrap_or(false);
            if font.is_some() && !has_glyph {
                notdef += 1;
            }

            // The widths table is authoritative for placement, but a glyph
            // whose /Widths entry disagrees with the font's own advance is
            // drawn nudged: that is what "the text is kerned wrong" looks
            // like, so report the ratio.
            let mut ratio = String::new();
            if let (Some(f), Some(gid), Some(wem)) = (font, gid, wem) {
                if wem.abs() > 0.0005 && has_glyph {
                    if let Some(g) = f.font.glyph(gid) {
                        let from_font = f.font.font_matrix().m11() * g.metrics.advance;
                        let r = from_font / wem;
                        if (r - 1.0).abs() > 0.01 {
                            mismatched += 1;
                            ratio = format!("{:+.1}%", (r - 1.0) * 100.0);
                        }
                    }
                }
            }

            if opts.chars && shown < opts.max_chars {
                let p = apply(&span.transform, vec2f(c.pos, 0.0));
                let gap = if k + 1 < n_chars {
                    format!("{:.3}", span.chars[k + 1].pos - (c.pos + c.width))
                } else {
                    "-".into()
                };
                char_rows.push(vec![
                    format!("{i}.{k}"),
                    format!("{:.2}", p.x()),
                    format!("{:.2}", p.y()),
                    format!("{:.3}", c.pos),
                    format!("{:.3}", c.width),
                    gap,
                    gid.map(|g| g.0.to_string()).unwrap_or_else(|| "?".into()),
                    show_text(&text, 3),
                    ratio,
                ]);
                shown += 1;
            }
        }
    }

    if opts.chars {
        println!();
        print!(
            "{}",
            table(
                &[
                    "span.char",
                    "x",
                    "y",
                    "pos",
                    "advance",
                    "gap",
                    "gid",
                    "ch",
                    "widths/font"
                ],
                &char_rows
            )
        );
        if shown < total_chars {
            println!("(stopped at {shown}; raise --max-chars to see the rest)");
        }
    }

    if opts.check {
        let empty: usize = lines.iter().filter(|l| l.span.chars.is_empty()).count();
        println!(
            "check: {} span(s) ({empty} empty), {total_chars} char(s), {nofont} span(s) \
             without a font, {notdef} char(s) without a glyph, {kerned} span(s) with TJ kerning, \
             {mismatched} char(s) whose /Widths disagrees with the font",
            lines.len()
        );
        if nofont + notdef > 0 {
            println!(
                "not a clean page: text that will not appear or will not sit where the \
                 file says"
            );
        } else {
            println!("every character has a glyph; advances are the ones the file asks for");
        }
    }
    Ok(())
}
