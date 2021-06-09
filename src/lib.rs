extern crate pdf;
use log::warn;

use std::collections::HashMap;
use std::convert::TryInto;

use pdf::content::*;
use pdf::encoding::BaseEncoding;
use pdf::error::PdfError;
use pdf::font::*;
use pdf::object::*;
use pdf::parser::parse_with_lexer;
use pdf::parser::Lexer;
use pdf::primitive::Primitive;
use pdf_encoding::{self, ForwardMap};

use byteorder::BE;
use utf16_ext::Utf16ReadExt;

fn utf16be_to_string(mut data: &[u8]) -> String {
    (&mut data)
        .utf16_chars::<BE>()
        .map(|c| c.unwrap())
        .collect()
}

// totally not a steaming pile of hacks
fn parse_cmap(data: &[u8]) -> HashMap<u16, String> {
    let mut lexer = Lexer::new(data);
    let mut map = HashMap::new();
    while let Ok(substr) = lexer.next() {
        match substr.as_slice() {
            b"beginbfchar" => loop {
                let a = parse_with_lexer(&mut lexer, &NoResolve);
                let b = parse_with_lexer(&mut lexer, &NoResolve);
                match (a, b) {
                    (Ok(Primitive::String(cid_data)), Ok(Primitive::String(unicode_data))) => {
                        let data = cid_data.as_bytes();
                        let cid = match data.len() {
                            1 => data[0] as u16,
                            2 => u16::from_be_bytes(data.try_into().unwrap()),
                            _ => {
                                dbg!(data, unicode_data);
                                continue;
                            }
                        };
                        let unicode = utf16be_to_string(unicode_data.as_bytes());
                        map.insert(cid, unicode);
                    }
                    _ => break,
                }
            },
            b"beginbfrange" => loop {
                let a = parse_with_lexer(&mut lexer, &NoResolve);
                let b = parse_with_lexer(&mut lexer, &NoResolve);
                let c = parse_with_lexer(&mut lexer, &NoResolve);
                match (a, b, c) {
                    (
                        Ok(Primitive::String(cid_start_data)),
                        Ok(Primitive::String(cid_end_data)),
                        Ok(Primitive::String(unicode_data)),
                    ) => {
                        let cid_start =
                            u16::from_be_bytes(cid_start_data.as_bytes().try_into().unwrap());
                        let cid_end =
                            u16::from_be_bytes(cid_end_data.as_bytes().try_into().unwrap());
                        let mut unicode_data = unicode_data.into_bytes();

                        for cid in cid_start..=cid_end {
                            let unicode = utf16be_to_string(&unicode_data);
                            map.insert(cid, unicode);
                            *unicode_data.last_mut().unwrap() += 1;
                        }
                    }
                    (
                        Ok(Primitive::String(cid_start_data)),
                        Ok(Primitive::String(cid_end_data)),
                        Ok(Primitive::Array(unicode_data_arr)),
                    ) => {
                        let cid_start =
                            u16::from_be_bytes(cid_start_data.as_bytes().try_into().unwrap());
                        let cid_end =
                            u16::from_be_bytes(cid_end_data.as_bytes().try_into().unwrap());

                        for (cid, unicode_data) in (cid_start..=cid_end).zip(unicode_data_arr) {
                            let unicode =
                                utf16be_to_string(&unicode_data.as_string().unwrap().as_bytes());
                            map.insert(cid, unicode);
                        }
                    }
                    _ => break,
                }
            },
            b"endcmap" => break,
            _ => {}
        }
    }

    map
}

enum Decoder {
    Map(&'static ForwardMap),
    Cmap(HashMap<u16, String>),
}

struct FontInfo {
    decoder: Decoder,
}
struct Cache {
    fonts: HashMap<String, FontInfo>,
}
impl Cache {
    fn new() -> Self {
        Cache {
            fonts: HashMap::new(),
        }
    }
    fn add_font(&mut self, name: impl Into<String>, font: RcRef<Font>) {
        let decoder = if let Some(to_unicode) = font.to_unicode() {
            let cmap = parse_cmap(to_unicode.data().unwrap());
            Decoder::Cmap(cmap)
        } else if let Some(encoding) = font.encoding() {
            let map = match encoding.base {
                BaseEncoding::StandardEncoding => &pdf_encoding::STANDARD,
                BaseEncoding::SymbolEncoding => &pdf_encoding::SYMBOL,
                BaseEncoding::WinAnsiEncoding => &pdf_encoding::WINANSI,
                ref e => {
                    warn!("unsupported pdf encoding {:?}", e);
                    return;
                }
            };
            Decoder::Map(map)
        } else {
            return;
        };

        self.fonts.insert(name.into(), FontInfo { decoder });
    }
    fn get_font(&self, name: &str) -> Option<&FontInfo> {
        self.fonts.get(name)
    }
}

fn add_string(data: &[u8], out: &mut String, info: &FontInfo) {
    match info.decoder {
        Decoder::Cmap(ref cmap) => {
            for w in data.windows(2) {
                let cp = u16::from_be_bytes(w.try_into().unwrap());
                if let Some(s) = cmap.get(&cp) {
                    out.push_str(s);
                }
            }
        }
        Decoder::Map(map) => out.extend(data.iter().filter_map(|&b| map.get(b))),
    }
}

fn add_array(arr: &[Primitive], out: &mut String, info: &FontInfo) {
    // println!("p: {:?}", p);
    for p in arr.iter() {
        match p {
            Primitive::String(s) => add_string(&s.data, out, info),
            _ => {}
        }
    }
}

pub fn page_text(page: &Page, resolve: &impl Resolve) -> Result<String, PdfError> {
    let resources = page.resources.as_ref().unwrap();
    let mut cache = Cache::new();
    let mut out = String::new();

    // make sure all fonts are in the cache, so we can reference them
    for (name, &font) in &resources.fonts {
        cache.add_font(name, resolve.get(font)?);
    }
    for gs in resources.graphics_states.values() {
        if let Some((font, _)) = gs.font {
            let font = resolve.get(font)?;
            cache.add_font(font.name.clone(), font);
        }
    }
    let mut current_font = None;
    let contents = page.contents.as_ref().unwrap();
    let mut font_size = 0.0;
    let mut text_leading = 1.0;
    let mut text_matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    for op in &contents.operations {
        match *op {
            Op::GraphicsState { ref name } => {
                let gs = resources.graphics_states.get(name).unwrap();

                if let Some((font, size)) = gs.font {
                    let font = resolve.get(font)?;
                    current_font = cache.get_font(&font.name);
                    font_size = size;
                }
            }
            Op::Leading { leading } => text_leading = leading,
            Op::TextFont { ref name, size } => {
                current_font = cache.get_font(name);
                font_size = size;
            }
            Op::TextDraw { ref text } => {
                if let Some(font) = current_font {
                    add_string(&text.data, &mut out, font);
                }
            }
            Op::TextDrawAdjusted { ref array } => {
                if let Some(font) = current_font {
                    add_array(array, &mut out, font);
                }
            }
            Op::TextNewline => {
                out.push('\n');
                text_matrix.f -= text_leading * text_matrix.d;
            }
            Op::MoveTextPosition { translation } => {
                text_matrix.f += translation.y * text_matrix.d;

                if translation.y != 0.0 {
                    out.push('\n');
                }
            }
            Op::SetTextMatrix { matrix } => {
                if matrix.f != text_matrix.f {
                    out.push('\n');
                } else {
                    out.push('\t');
                }
                text_matrix = matrix;
            }
            _ => {}
        }
    }
    Ok(out)
}
