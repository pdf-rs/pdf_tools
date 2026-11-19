//! Formatting helpers: primitives as PDF syntax, hexdumps, tables, sizes.

use pdf::object::Resolve;
use pdf::primitive::Primitive;

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Escape a name for PDF syntax (`/Name`).
pub fn escape_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let ordinary = (0x21..=0x7e).contains(&b)
            && !matches!(
                b,
                b'/' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%' | b'#'
            );
        if ordinary {
            out.push(b as char);
        } else {
            out.push_str(&format!("#{b:02X}"));
        }
    }
    out
}

/// Does this look like text a human (or an agent) wants to read?
pub fn is_text(b: &[u8]) -> bool {
    if b.is_empty() {
        return true;
    }
    match std::str::from_utf8(b) {
        Ok(s) => {
            let total = s.chars().count().max(1);
            let ctl = s
                .chars()
                .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
                .count();
            ctl * 100 <= total
        }
        Err(_) => false,
    }
}

/// A PDF string literal, or hex when it is not text.
pub fn string_repr(b: &[u8], limit: usize) -> String {
    let cut = b.len() > limit;
    let b = if cut { &b[..limit] } else { b };
    if is_text(b) {
        let mut out = String::from("(");
        for c in String::from_utf8_lossy(b).chars() {
            match c {
                '(' | ')' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\{:03o}", c as u32)),
                c => out.push(c),
            }
        }
        out.push(')');
        if cut {
            out.push_str("...");
        }
        out
    } else {
        let mut out = String::from("<");
        for &x in b {
            out.push_str(&format!("{x:02x}"));
        }
        if cut {
            out.push_str("...");
        }
        out.push('>');
        out
    }
}

pub fn hexdump(data: &[u8], limit: usize) -> String {
    let mut out = String::new();
    let shown = data.len().min(limit);
    for (i, chunk) in data[..shown].chunks(16).enumerate() {
        let off = i * 16;
        out.push_str(&format!("{off:08x}  "));
        let mut ascii = String::new();
        for j in 0..16 {
            match chunk.get(j) {
                Some(&b) => {
                    out.push_str(&format!("{b:02x} "));
                    ascii.push(if (0x20..0x7f).contains(&b) {
                        b as char
                    } else {
                        '.'
                    });
                }
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push_str(&format!(" |{ascii}|\n"));
    }
    if data.len() > shown {
        out.push_str(&format!(
            "... {} more bytes ({} total)\n",
            data.len() - shown,
            data.len()
        ));
    }
    out
}

/// Aligned columns. Every row must have the same number of cells as `headers`.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let line = |cells: &[String]| {
        let mut s = String::new();
        for i in 0..cols {
            let cell = cells.get(i).map(String::as_str).unwrap_or("");
            s.push_str(cell);
            if i + 1 < cols {
                let pad = widths[i] - cell.chars().count();
                s.push_str(&" ".repeat(pad + 2));
            }
        }
        while s.ends_with(' ') {
            s.pop();
        }
        s.push('\n');
        s
    };
    let header_cells: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    let mut out = line(&header_cells);
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    out.push_str(&line(&rule));
    for row in rows {
        out.push_str(&line(row));
    }
    out
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('~');
        t
    }
}

fn inline_len(p: &Primitive) -> usize {
    match p {
        Primitive::Null => 4,
        Primitive::Boolean(b) => 5 + *b as usize,
        Primitive::Integer(i) => i.to_string().len(),
        Primitive::Number(n) => format!("{n}").len(),
        Primitive::Name(n) => n.chars().count() + 1,
        Primitive::String(s) => s.as_bytes().len() + 2,
        Primitive::Reference(r) => format!("{} {} R", r.id, r.gen).len(),
        Primitive::Array(a) => {
            2 + a.iter().map(inline_len).sum::<usize>() + a.len().saturating_sub(1)
        }
        Primitive::Dictionary(_) => 60,
        Primitive::Stream(_) => 80,
    }
}

pub fn prim_inline(p: &Primitive) -> String {
    let mut out = String::new();
    write_inline(&mut out, p);
    out
}

fn write_inline(out: &mut String, p: &Primitive) {
    match p {
        Primitive::Null => out.push_str("null"),
        Primitive::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        Primitive::Integer(i) => out.push_str(&i.to_string()),
        Primitive::Number(n) => out.push_str(&format_f32(*n)),
        Primitive::Name(n) => {
            out.push('/');
            out.push_str(&escape_name(n.as_str()));
        }
        Primitive::String(s) => out.push_str(&string_repr(s.as_bytes(), 256)),
        Primitive::Reference(r) => out.push_str(&format!("{} {} R", r.id, r.gen)),
        Primitive::Array(a) => {
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_inline(out, item);
            }
            out.push(']');
        }
        Primitive::Dictionary(d) => {
            out.push_str("<<");
            let mut first = true;
            for (k, v) in d.iter() {
                if !first {
                    out.push(' ');
                }
                first = false;
                out.push('/');
                out.push_str(&escape_name(k.as_str()));
                out.push(' ');
                write_inline(out, v);
            }
            out.push_str(">>");
        }
        Primitive::Stream(s) => {
            write_inline(out, &Primitive::Dictionary(s.info.clone()));
            out.push_str(" stream");
        }
    }
}

pub fn format_f32(n: f32) -> String {
    if n == n.trunc() && n.abs() < 1e9 {
        format!("{}", n as i64)
    } else {
        format!("{n:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// Pretty-print a primitive as PDF syntax, following references up to `depth`
/// levels (`0` prints them as `12 0 R`).
pub fn prim_text<R: Resolve>(p: &Primitive, resolve: &R, depth: u32) -> String {
    let mut out = String::new();
    write_prim(&mut out, p, resolve, depth, 0);
    out
}

pub fn write_prim<R: Resolve>(out: &mut String, p: &Primitive, r: &R, depth: u32, indent: usize) {
    let pad = "  ".repeat(indent);
    match p {
        Primitive::Dictionary(d) => write_dict(out, d, r, depth, indent),
        Primitive::Array(a) => {
            let short = a.iter().map(inline_len).sum::<usize>() + indent * 2 < 72;
            if short && depth == 0 {
                write_inline(out, p);
                return;
            }
            out.push_str("[\n");
            for item in a {
                out.push_str(&pad);
                out.push_str("  ");
                write_prim(out, item, r, depth, indent + 1);
                out.push('\n');
            }
            out.push_str(&pad);
            out.push(']');
        }
        Primitive::Stream(s) => {
            write_dict(out, &s.info, r, depth, indent);
            out.push('\n');
            out.push_str(&pad);
            match s.info.get("Length").and_then(|p| p.as_integer().ok()) {
                Some(len) => out.push_str(&format!("stream ({len} bytes as stored)")),
                None => out.push_str("stream (length indirect; see `stream` for the real size)"),
            }
        }
        Primitive::Reference(pref) => {
            if depth > 0 {
                out.push_str(&format!("{} {} R => ", pref.id, pref.gen));
                match r.resolve(*pref) {
                    Ok(obj) => write_prim(out, &obj, r, depth - 1, indent),
                    Err(e) => out.push_str(&format!("<unresolved {} {} R: {e}>", pref.id, pref.gen)),
                }
            } else {
                out.push_str(&format!("{} {} R", pref.id, pref.gen));
            }
        }
        other => write_inline(out, other),
    }
}

fn write_dict<R: Resolve>(
    out: &mut String,
    d: &pdf::primitive::Dictionary,
    r: &R,
    depth: u32,
    indent: usize,
) {
    if d.is_empty() {
        out.push_str("<<>>");
        return;
    }
    let pad = "  ".repeat(indent);
    out.push_str("<<\n");
    for (k, v) in d.iter() {
        out.push_str(&pad);
        out.push_str("  /");
        out.push_str(&escape_name(k.as_str()));
        out.push(' ');
        match v {
            Primitive::Reference(pref) if depth > 0 => {
                out.push_str(&format!("{} {} R => ", pref.id, pref.gen));
                match r.resolve(*pref) {
                    Ok(obj) => write_prim(out, &obj, r, depth - 1, indent + 1),
                    Err(e) => out.push_str(&format!("<unresolved {} {} R: {e}>", pref.id, pref.gen)),
                }
            }
            other => write_prim(out, other, r, depth, indent + 1),
        }
        out.push('\n');
    }
    out.push_str(&pad);
    out.push_str(">>");
}

/// The `/Type` + `/Subtype` of an object, for tables: "Page", "XObject/Image"...
pub fn kind_of(p: &Primitive) -> String {
    let dict = match p {
        Primitive::Dictionary(d) => d,
        Primitive::Stream(s) => &s.info,
        Primitive::Array(_) => return "array".into(),
        Primitive::Null => return "null".into(),
        Primitive::Integer(_) | Primitive::Number(_) => return "number".into(),
        Primitive::Boolean(_) => return "bool".into(),
        Primitive::String(_) => return "string".into(),
        Primitive::Name(_) => return "name".into(),
        Primitive::Reference(_) => return "ref".into(),
    };
    let name = |key: &str| -> Option<String> {
        dict.get(key).and_then(|p| match p {
            Primitive::Name(n) => Some(n.as_str().trim_start_matches('/').to_string()),
            _ => None,
        })
    };
    let ty = name("Type");
    let sub = name("Subtype");
    match (ty, sub) {
        (Some(t), Some(s)) => format!("{t}/{s}"),
        (Some(t), None) => t,
        (None, Some(s)) => s,
        (None, None) => {
            if matches!(p, Primitive::Stream(_)) {
                "stream".into()
            } else {
                "dict".into()
            }
        }
    }
}
