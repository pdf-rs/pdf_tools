//! Raw, byte-level view of a PDF file.
//!
//! This layer does not trust the library's (sanitised) cross-reference table:
//! it reads the file the way a forensic tool would — startxref, the /Prev
//! chain, the objects as they are physically laid out — so that damaged,
//! hostile or merely unusual files can still be looked at.

use std::ops::Range;

use pdf::enc::{decode, StreamFilter};
use pdf::object::{NoResolve, ParseOptions};
use pdf::parser::{parse_with_lexer, Lexer, ParseFlags};
use pdf::primitive::{Dictionary, Primitive};

pub fn is_ws(b: u8) -> bool {
    matches!(b, 0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

pub fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() || from >= hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len())
        .rev()
        .find(|&i| &hay[i..i + needle.len()] == needle)
}

/// A stream as it is stored in the file: where its bytes are, and what
/// `/Length` claimed.
#[allow(dead_code)] // the whole struct is the raw layer's public record
#[derive(Clone, Debug)]
pub struct RawStream {
    pub data: Range<usize>,
    pub declared: Option<i64>,
    pub length_ok: bool,
    pub has_endstream: bool,
}

impl RawStream {
    pub fn len(&self) -> usize {
        self.data.end - self.data.start
    }
}

/// An indirect object as it appears in the file.
#[derive(Clone)]
#[allow(dead_code)] // see RawStream
pub struct RawObj {
    pub id: u64,
    pub gen: u64,
    /// Offset of the `N` of `N G obj`.
    pub start: usize,
    /// Offset just past the dictionary / value.
    pub value_end: usize,
    /// Offset just past `endobj` (or the stream, when `endobj` is missing).
    pub end: usize,
    pub prim: Primitive,
    pub stream: Option<RawStream>,
    pub problems: Vec<String>,
}

#[allow(dead_code)]
impl RawObj {
    pub fn dict(&self) -> Option<&Dictionary> {
        match &self.prim {
            Primitive::Dictionary(d) => Some(d),
            Primitive::Stream(s) => Some(&s.info),
            _ => None,
        }
    }
    pub fn stream_bytes<'a>(&self, raw: &'a Raw) -> Option<&'a [u8]> {
        self.stream.as_ref().map(|s| &raw.data[s.data.clone()])
    }
}

pub struct Raw {
    pub data: Vec<u8>,
}

/// The end (just past the closing `>>`) of the dictionary that starts at
/// `from`. Strings and nested dictionaries are skipped, so a `>>` inside a
/// string does not end it.
fn scan_dict_end(data: &[u8], from: usize) -> Result<usize, String> {
    let mut i = from;
    let mut depth = 0i32;
    while i < data.len() {
        match data[i] {
            b'(' => {
                i += 1;
                let mut nest = 1i32;
                while i < data.len() && nest > 0 {
                    match data[i] {
                        b'\\' => i += 2,
                        b'(' => {
                            nest += 1;
                            i += 1;
                        }
                        b')' => {
                            nest -= 1;
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'<' if data.get(i + 1) == Some(&b'<') => {
                depth += 1;
                i += 2;
            }
            b'>' if data.get(i + 1) == Some(&b'>') => {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    return Ok(i);
                }
            }
            b'<' => {
                i += 1;
                while i < data.len() && data[i] != b'>' {
                    i += 1;
                }
                i += 1;
            }
            b'%' => {
                while i < data.len() && data[i] != b'\n' {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    Err("the dictionary is not terminated by >>".to_string())
}



impl Raw {
    pub fn new(data: Vec<u8>) -> Raw {
        Raw { data }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// The `%PDF-x.y` header: its offset and the version that follows.
    pub fn header(&self) -> Option<(usize, String)> {
        let at = find_from(&self.data, b"%PDF-", 0)?;
        let start = at + 5;
        let mut end = start;
        while end < self.data.len()
            && (self.data[end].is_ascii_digit() || self.data[end] == b'.')
            && end - start < 8
        {
            end += 1;
        }
        Some((
            at,
            String::from_utf8_lossy(&self.data[start..end]).to_string(),
        ))
    }

    pub fn eof_positions(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(p) = find_from(&self.data, b"%%EOF", i) {
            out.push(p);
            i = p + 5;
        }
        out
    }

    /// The last `startxref`: (offset of the word, the value it gives).
    pub fn startxref(&self) -> Option<(usize, usize)> {
        let pos = rfind(&self.data, b"startxref")?;
        let mut lx = Lexer::with_offset(&self.data[pos..], pos);
        lx.next().ok()?;
        let v: u64 = lx.next().ok()?.to().ok()?;
        Some((pos, v as usize))
    }

    #[allow(dead_code)]
    pub fn trailer_dicts(&self) -> Vec<(usize, Dictionary)> {
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(p) = find_from(&self.data, b"trailer", i) {
            let mut lx = Lexer::with_offset(&self.data[p..], p);
            if lx.next().is_ok() {
                if let Ok(Primitive::Dictionary(d)) =
                    parse_with_lexer(&mut lx, &NoResolve, ParseFlags::DICT)
                {
                    out.push((p, d));
                }
            }
            i = p + 7;
        }
        out
    }

    /// Parse the indirect object that starts at `off` (the `N` of `N G obj`).
    pub fn object_at(&self, off: usize) -> Result<RawObj, String> {
        if off >= self.data.len() {
            return Err(format!("offset {off} is past the end of the file"));
        }
        if !self.data[off].is_ascii_digit() {
            return Err(format!("no object starts at offset {off}"));
        }
        let mut problems = Vec::new();
        let mut lx = Lexer::with_offset(&self.data[off..], off);
        let id = lx
            .next_as::<u64>()
            .map_err(|e| format!("no object number at {off}: {e}"))?;
        let gen = lx
            .next_as::<u64>()
            .map_err(|e| format!("object {id} has no generation number: {e}"))?;
        let kw = lx
            .next()
            .map_err(|e| format!("object {id} {gen}: no 'obj' keyword: {e}"))?;
        if !kw.equals(b"obj") {
            return Err(format!(
                "at {off}: expected 'obj', found {:?}",
                String::from_utf8_lossy(kw.as_slice())
            ));
        }
        let mut value_start = off + lx.get_pos();
        while value_start < self.data.len() && is_ws(self.data[value_start]) {
            value_start += 1;
        }

        // A dictionary is read from a slice that ends at its closing `>>`, so
        // the parser never walks into a stream body: `/Length` may be indirect
        // or wrong, and neither must decide where the object ends.
        let (prim, value_end) = if self.data[value_start..].starts_with(b"<<") {
            let end = scan_dict_end(&self.data, value_start)?;
            let mut dl = Lexer::with_offset(&self.data[value_start..end], value_start);
            let dict = parse_with_lexer(&mut dl, &NoResolve, ParseFlags::DICT)
                .map_err(|e| format!("object {id} {gen}: {e}"))?;
            (dict, end)
        } else {
            let prim =
                parse_with_lexer(&mut lx, &NoResolve, ParseFlags::ANY ^ ParseFlags::STREAM)
                    .map_err(|e| format!("object {id} {gen}: {e}"))?;
            (prim, off + lx.get_pos())
        };

        // Is there a stream?
        let mut p = value_end;
        while p < self.data.len() && is_ws(self.data[p]) {
            p += 1;
        }
        let mut stream = None;
        let mut end;
        if self.data[p..].starts_with(b"stream") {
            let mut ds = p + 6;
            if self.data.get(ds) == Some(&b'\r') {
                ds += 1;
            }
            if self.data.get(ds) == Some(&b'\n') {
                ds += 1;
            }
            let declared = match &prim {
                Primitive::Dictionary(d) => d.get("Length").and_then(|v| match v {
                    Primitive::Integer(i) => Some(*i),
                    _ => None,
                }),
                Primitive::Stream(s) => s.info.get("Length").and_then(|v| match v {
                    Primitive::Integer(i) => Some(*i),
                    _ => None,
                }),
                _ => None,
            };
            let mut data_end = None;
            let mut length_ok = false;
            if let Some(l) = declared {
                if l >= 0 && ds + l as usize <= self.data.len() {
                    let de = ds + l as usize;
                    let mut q = de;
                    while q < self.data.len() && is_ws(self.data[q]) {
                        q += 1;
                    }
                    if self.data[q..].starts_with(b"endstream") {
                        data_end = Some(de);
                        length_ok = true;
                    }
                }
                if !length_ok {
                    problems.push(format!("/Length says {l}, the data does not end there"));
                }
            } else if declared.is_none() {
                // An indirect /Length is ordinary (and resolving it here would
                // mean a second parse); the lookup below finds the end anyway.
            }
            let has_endstream;
            let data_end = match data_end {
                Some(e) => {
                    has_endstream = true;
                    e
                }
                None => match find_from(&self.data, b"endstream", ds) {
                    Some(i) => {
                        let mut e = i;
                        while e > ds && matches!(self.data[e - 1], b'\n' | b'\r' | b' ' | b'\t') {
                            e -= 1;
                        }
                        has_endstream = true;
                        e
                    }
                    None => {
                        has_endstream = false;
                        problems.push("no endstream".to_string());
                        self.data.len()
                    }
                },
            };
            let after = find_from(&self.data, b"endstream", data_end)
                .map(|i| i + 9)
                .unwrap_or(data_end);
            end = find_from(&self.data, b"endobj", after)
                .map(|i| i + 6)
                .unwrap_or(after);
            stream = Some(RawStream {
                data: ds..data_end.max(ds),
                declared,
                length_ok,
                has_endstream,
            });
        } else {
            end = find_from(&self.data, b"endobj", value_end)
                .map(|i| i + 6)
                .unwrap_or(value_end);
        }

        while end < self.data.len() && is_ws(self.data[end]) {
            end += 1;
        }

        Ok(RawObj {
            id,
            gen,
            start: off,
            value_end,
            end,
            prim,
            stream,
            problems,
        })
    }

    /// The offset of the next `N G obj` at or after `from`, if any.
    pub fn next_marker(&self, from: usize) -> Option<usize> {
        let d = &self.data;
        let mut i = from;
        while let Some(p) = find_from(d, b"obj", i) {
            i = p + 3;
            let after_ok = p + 3 >= d.len() || is_ws(d[p + 3]) || d[p + 3] == b'<';
            if !after_ok {
                continue;
            }
            // generation number
            let mut k = p;
            while k > 0 && is_ws(d[k - 1]) {
                k -= 1;
            }
            let gen_end = k;
            let mut k2 = k;
            while k2 > 0 && d[k2 - 1].is_ascii_digit() {
                k2 -= 1;
            }
            if k2 == gen_end {
                continue;
            }
            // whitespace between the numbers
            let mut k3 = k2;
            while k3 > 0 && is_ws(d[k3 - 1]) {
                k3 -= 1;
            }
            if k3 == k2 {
                continue;
            }
            let id_end = k3;
            let mut k4 = id_end;
            while k4 > 0 && d[k4 - 1].is_ascii_digit() {
                k4 -= 1;
            }
            if k4 == id_end {
                continue;
            }
            if k4 > 0 && !is_ws(d[k4 - 1]) && !b"<>[](){}%".contains(&d[k4 - 1]) {
                continue;
            }
            return Some(k4);
        }
        None
    }

    /// Walk the file body: every object that can be parsed, in file order,
    /// plus the byte ranges that are not part of any object.
    pub fn walk(&self) -> (Vec<RawObj>, Vec<Range<usize>>) {
        let mut objs = Vec::new();
        let mut gaps = Vec::new();
        let mut pos = self.header().map(|(o, _)| o).unwrap_or(0);
        while pos < self.data.len() {
            match self.object_at(pos) {
                Ok(o) => {
                    let end = o.end.max(pos + 1);
                    pos = end;
                    objs.push(o);
                }
                _ => match self.next_marker(pos) {
                    Some(next) => {
                        if next > pos {
                            gaps.push(pos..next);
                            pos = next;
                        } else {
                            // `pos` is a marker that does not parse; step over
                            // it rather than finding the same one again.
                            pos += 1;
                        }
                    }
                    None => {
                        if pos < self.data.len() {
                            gaps.push(pos..self.data.len());
                        }
                        break;
                    }
                },
            }
        }
        (objs, gaps)
    }
}

// ---------------------------------------------------------------------------
// Cross-reference sections
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Free { next: u64, gen: u64 },
    InUse { off: u64, gen: u64 },
    Compressed { container: u64, index: u64 },
}

impl Entry {
    pub fn gen(&self) -> u64 {
        match self {
            Entry::Free { gen, .. } | Entry::InUse { gen, .. } => *gen,
            Entry::Compressed { .. } => 0,
        }
    }
    #[allow(dead_code)]
    pub fn kind(&self) -> char {
        match self {
            Entry::Free { .. } => 'f',
            Entry::InUse { .. } => 'n',
            Entry::Compressed { .. } => 'c',
        }
    }
}

pub struct XrefSection {
    pub offset: usize,
    pub kind: &'static str,
    /// (first object number, entries) per subsection / /Index pair.
    pub subsections: Vec<(u64, Vec<Entry>)>,
    pub trailer: Dictionary,
    pub prev: Option<usize>,
    pub xrefstm: Option<usize>,
    pub problems: Vec<String>,
}

impl XrefSection {
    pub fn num_entries(&self) -> usize {
        self.subsections.iter().map(|(_, e)| e.len()).sum()
    }
}

pub struct Xref {
    pub sections: Vec<XrefSection>,
    pub problems: Vec<String>,
}

impl Xref {
    /// Object number -> (entry, section index), newest section winning.
    pub fn map(&self) -> std::collections::BTreeMap<u64, (Entry, usize)> {
        let mut out = std::collections::BTreeMap::new();
        for (idx, sec) in self.sections.iter().enumerate() {
            for (first, entries) in &sec.subsections {
                for (i, e) in entries.iter().enumerate() {
                    out.entry(*first + i as u64).or_insert((*e, idx));
                }
            }
        }
        out
    }
}

impl Raw {
    pub fn xref(&self) -> Xref {
        let mut sections = Vec::new();
        let mut problems = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut next = self.startxref().map(|(_, v)| v);
        if next.is_none() {
            problems.push("no startxref at the end of the file".to_string());
        }
        while let Some(off) = next {
            if !seen.insert(off) {
                problems.push(format!("xref chain loops back to {off}"));
                break;
            }
            if off >= self.data.len() {
                problems.push(format!("xref offset {off} is past the end of the file"));
                break;
            }
            match self.read_xref_section(off) {
                Ok(sec) => {
                    next = sec.prev;
                    sections.push(sec);
                }
                Err(e) => {
                    problems.push(format!("xref section at {off}: {e}"));
                    break;
                }
            }
        }
        Xref { sections, problems }
    }

    fn read_xref_section(&self, off: usize) -> Result<XrefSection, String> {
        let lx = Lexer::with_offset(&self.data[off..], off);
        let first = lx.peek().map_err(|e| e.to_string())?;
        if first.equals(b"xref") {
            self.read_table(off)
        } else {
            self.read_stream_xref(off)
        }
    }

    fn read_table(&self, off: usize) -> Result<XrefSection, String> {
        let mut lx = Lexer::with_offset(&self.data[off..], off);
        lx.next_expect("xref").map_err(|e| e.to_string())?;
        let mut subsections = Vec::new();
        let mut problems = Vec::new();
        loop {
            let peeked = lx.peek().map_err(|e| e.to_string())?;
            if peeked.equals(b"trailer") {
                break;
            }
            let start: u64 = lx
                .next_as()
                .map_err(|e| format!("subsection start: {e}"))?;
            let count: u64 = lx
                .next_as()
                .map_err(|e| format!("subsection count: {e}"))?;
            let remaining = lx.get_remaining_slice().len() as u64;
            if count > remaining / 2 {
                return Err(format!(
                    "subsection {start} declares {count} entries but only {remaining} bytes are left"
                ));
            }
            let mut entries = Vec::new();
            for i in 0..count {
                let f1 = lx.next().map_err(|e| e.to_string())?;
                if f1.equals(b"trailer") {
                    problems.push(format!(
                        "subsection {start} promised {count} entries, {i} present"
                    ));
                    break;
                }
                let f2 = lx.next().map_err(|e| e.to_string())?;
                let f3 = lx.next().map_err(|e| e.to_string())?;
                let off1: u64 = f1.to().map_err(|e| format!("entry {i}: {e}"))?;
                let gen: u64 = f2.to().map_err(|e| format!("entry {i} gen: {e}"))?;
                if f3.equals(b"n") {
                    entries.push(Entry::InUse { off: off1, gen });
                } else if f3.equals(b"f") {
                    entries.push(Entry::Free { next: off1, gen });
                } else {
                    problems.push(format!("entry {i}: type {f3:?} is neither n nor f"));
                    entries.push(Entry::Free { next: 0, gen: 0 });
                }
            }
            subsections.push((start, entries));
        }
        lx.next_expect("trailer").map_err(|e| e.to_string())?;
        let trailer = parse_with_lexer(&mut lx, &NoResolve, ParseFlags::DICT)
            .map_err(|e| format!("trailer: {e}"))?;
        let trailer = trailer.into_dictionary().map_err(|e| e.to_string())?;
        let prev = trailer
            .get("Prev")
            .and_then(|p| p.as_integer().ok())
            .map(|i| i as usize);
        let xrefstm = trailer
            .get("XRefStm")
            .and_then(|p| p.as_integer().ok())
            .map(|i| i as usize);
        Ok(XrefSection {
            offset: off,
            kind: "table",
            subsections,
            trailer,
            prev,
            xrefstm,
            problems,
        })
    }

    fn read_stream_xref(&self, off: usize) -> Result<XrefSection, String> {
        let obj = self.object_at(off)?;
        let dict = match &obj.prim {
            Primitive::Dictionary(d) => d.clone(),
            Primitive::Stream(s) => s.info.clone(),
            _ => return Err("not a dictionary".into()),
        };
        let mut problems = obj.problems.clone();
        let width: Vec<usize> = dict
            .get("W")
            .and_then(|p| p.clone().into_array().ok())
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_i64().ok().map(|i| i.max(0) as usize))
                    .collect()
            })
            .unwrap_or_default();
        if width.len() != 3 {
            return Err(format!("/W must have 3 elements, found {}", width.len()));
        }
        let size = dict.get("Size").and_then(|p| p.as_i64().ok()).unwrap_or(0);
        let index: Vec<u64> = dict
            .get("Index")
            .and_then(|p| p.clone().into_array().ok())
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_i64().ok().map(|i| i as u64))
                    .collect()
            })
            .unwrap_or_else(|| vec![0, size.max(0) as u64]);

        let data = match obj.stream.as_ref() {
            Some(s) => &self.data[s.data.clone()],
            None => return Err("xref stream has no stream body".into()),
        };
        let specs = filter_specs(&dict, &NoResolve);
        let opts = ParseOptions::tolerant();
        let (decoded, codec) = decode_chain(data, &specs, &opts)?;
        if codec.is_some() {
            return Err("xref stream uses an image filter".into());
        }
        let row: usize = width.iter().sum();
        if row == 0 {
            return Err("/W sums to 0".into());
        }
        let mut subsections = Vec::new();
        let mut cursor = 0usize;
        for pair in index.chunks(2) {
            if pair.len() < 2 {
                problems.push("/Index has an odd number of elements".into());
                break;
            }
            let (first, count) = (pair[0], pair[1] as usize);
            let mut entries = Vec::new();
            for _ in 0..count {
                if cursor + row > decoded.len() {
                    problems.push(format!(
                        "xref stream is {} bytes short at object {first}",
                        cursor + row - decoded.len()
                    ));
                    break;
                }
                let mut fields = [0u64; 3];
                let mut at = cursor;
                for (f, w) in width.iter().enumerate() {
                    let mut v = 0u64;
                    for _ in 0..*w {
                        v = (v << 8) | decoded[at] as u64;
                        at += 1;
                    }
                    fields[f] = v;
                }
                cursor += row;
                let ty = if width[0] == 0 { 1 } else { fields[0] };
                let entry = match ty {
                    0 => Entry::Free {
                        next: fields[1],
                        gen: fields[2],
                    },
                    1 => Entry::InUse {
                        off: fields[1],
                        gen: fields[2],
                    },
                    2 => Entry::Compressed {
                        container: fields[1],
                        index: fields[2],
                    },
                    other => {
                        problems.push(format!("entry type {other} is not valid"));
                        Entry::Free { next: 0, gen: 0 }
                    }
                };
                entries.push(entry);
            }
            subsections.push((first, entries));
        }
        let prev = dict
            .get("Prev")
            .and_then(|p| p.as_i64().ok())
            .map(|i| i as usize);
        Ok(XrefSection {
            offset: off,
            kind: "stream",
            subsections,
            trailer: dict,
            prev,
            xrefstm: None,
            problems,
        })
    }
}

// ---------------------------------------------------------------------------
// Filter chains
// ---------------------------------------------------------------------------

/// (filter name, decode parameters) for a stream dictionary.
pub fn filter_specs(dict: &Dictionary, r: &impl pdf::object::Resolve) -> Vec<(String, Dictionary)> {
    let mut names = Vec::new();
    let filter = dict
        .get("Filter")
        .or_else(|| dict.get("F"))
        .map(|p| p.clone().resolve(r).unwrap_or(Primitive::Null));
    match filter {
        Some(Primitive::Name(n)) => names.push(n.as_str().to_string()),
        Some(Primitive::Array(a)) => {
            for p in a {
                if let Primitive::Name(n) = p {
                    names.push(n.as_str().to_string());
                }
            }
        }
        _ => {}
    }
    let mut parms: Vec<Dictionary> = Vec::new();
    match dict
        .get("DecodeParms")
        .or_else(|| dict.get("DP"))
        .map(|p| p.clone().resolve(r).unwrap_or(Primitive::Null))
    {
        Some(Primitive::Dictionary(d)) => parms.push(d),
        Some(Primitive::Array(a)) => {
            for p in a {
                match p {
                    Primitive::Dictionary(d) => parms.push(d),
                    _ => parms.push(Dictionary::new()),
                }
            }
        }
        _ => {}
    }
    names
        .into_iter()
        .enumerate()
        .map(|(i, n)| (n, parms.get(i).cloned().unwrap_or_else(Dictionary::new)))
        .collect()
}

/// Filters that produce an image file rather than bytes to look at.
pub fn image_codec(name: &str) -> Option<&'static str> {
    match name {
        "DCTDecode" | "DCT" => Some("jpeg"),
        "JPXDecode" => Some("jp2"),
        "JBIG2Decode" => Some("jbig2"),
        "CCITTFaxDecode" | "CCF" => Some("fax"),
        _ => None,
    }
}

/// Apply a filter chain. Image codecs stop the chain and are reported instead
/// of decoded, because their output is a picture file, not bytes.
pub fn decode_chain(
    data: &[u8],
    specs: &[(String, Dictionary)],
    opts: &ParseOptions,
) -> Result<(Vec<u8>, Option<&'static str>), String> {
    let mut buf = data.to_vec();
    for (i, (name, parms)) in specs.iter().enumerate() {
        if let Some(codec) = image_codec(name) {
            if i + 1 != specs.len() {
                return Err(format!(
                    "image filter {name} is followed by {} more filter(s)",
                    specs.len() - i - 1
                ));
            }
            return Ok((buf, Some(codec)));
        }
        let f = StreamFilter::from_kind_and_params(name, parms.clone(), &NoResolve)
            .map_err(|e| format!("filter {name}: {e}"))?;
        buf = decode(&buf, &f, opts).map_err(|e| format!("filter {name}: {e}"))?;
    }
    Ok((buf, None))
}
