extern crate pdf;

pub mod inspect;
pub mod merge;
pub mod pages;

use log::warn;
use pdf::primitive::Name;

use std::collections::HashMap;
use std::convert::TryInto;
use std::rc::Rc;

use pdf::content::*;
use pdf::encoding::BaseEncoding;
use pdf::error::{PdfError, Result};
use pdf::font::*;
use pdf::object::*;
use pdf_encoding::{self, DifferenceForwardMap};

use euclid::Transform2D;

#[derive(Clone, Default)]
enum Decoder {
    Map(DifferenceForwardMap),
    Cmap(ToUnicodeMap),
    #[default]
    None,
}

#[derive(Default, Clone)]
pub struct FontInfo {
    decoder: Decoder,
    /// Glyph advance per character code, in 1/1000 text space units.
    widths: std::collections::HashMap<u16, f32>,
    /// Codes are two bytes (Identity-H and the other CID encodings).
    two_byte: bool,
}

impl FontInfo {
    /// How far the text matrix moves after showing `data` at `size`.
    ///
    /// PDF text is not kerned: the advance is exactly the sum of the glyph
    /// widths (plus `Tc`/`Tw`, which this does not see). Comparing it with the
    /// x of the next run is what shows a font whose widths the renderer and
    /// the document disagree about.
    pub fn advance(&self, data: &[u8], size: f32) -> f32 {
        let mut w = 0.0;
        if self.two_byte {
            for pair in data.chunks(2) {
                if pair.len() == 2 {
                    let code = u16::from_be_bytes([pair[0], pair[1]]);
                    w += self.widths.get(&code).copied().unwrap_or(0.0);
                }
            }
        } else {
            for &b in data {
                w += self.widths.get(&(b as u16)).copied().unwrap_or(0.0);
            }
        }
        w / 1000.0 * size
    }

    pub fn two_byte(&self) -> bool {
        self.two_byte
    }

    pub fn decode(&self, data: &[u8], out: &mut String) -> Result<()> {
        match &self.decoder {
            Decoder::Cmap(ref cmap) => {
                // FIXME: not sure the BOM is obligatory
                if data.starts_with(&[0xfe, 0xff]) {
                    // FIXME: really windows not chunks!?
                    for w in data.windows(2) {
                        let cp = u16::from_be_bytes(w.try_into().unwrap());
                        if let Some(s) = cmap.get(cp) {
                            out.push_str(s);
                        }
                    }
                } else {
                    out.extend(
                        data.iter()
                            .filter_map(|&b| cmap.get(b.into()).map(|v| v.to_owned())),
                    );
                }
                Ok(())
            }
            Decoder::Map(map) => {
                out.extend(
                    data.iter()
                        .filter_map(|&b| map.get(b).map(|v| v.to_owned())),
                );
                Ok(())
            }
            Decoder::None => {
                if data.starts_with(&[0xfe, 0xff]) {
                    utf16be_to_char(&data[2..]).try_for_each(|r| {
                        r.map_or(Err(PdfError::Utf16Decode), |c| {
                            out.push(c);
                            Ok(())
                        })
                    })
                } else if let Ok(text) = std::str::from_utf8(data) {
                    out.push_str(text);
                    Ok(())
                } else {
                    Err(PdfError::Utf16Decode)
                }
            }
        }
    }
}

struct FontCache<'src, T: Resolve> {
    fonts: HashMap<Name, Rc<FontInfo>>,
    page: &'src Page,
    resolve: &'src T,
    default_font: Rc<FontInfo>,
}

impl<'src, T: Resolve> FontCache<'src, T> {
    fn new(page: &'src Page, resolve: &'src T) -> Self {
        let mut cache = FontCache {
            fonts: HashMap::new(),
            page,
            resolve,
            default_font: Rc::new(FontInfo::default()),
        };

        cache.populate();

        cache
    }

    fn populate(&mut self) {
        let resolve = self.resolve;
        if let Ok(resources) = self.page.resources() {
            for (name, font) in resources.fonts.iter() {
                match font.load(resolve) {
                    Ok(font) => self.add_font(name.clone(), &font, resolve),
                    Err(err) => warn!("ignoring unusable font {:?}: {}", name, err),
                }
            }

            for (font, _) in resources.graphics_states.values().filter_map(|gs| gs.font) {
                if let Ok(font) = self.resolve.get(font) {
                    if let Some(name) = &font.name {
                        self.add_font(name.clone(), &font, resolve);
                    }
                }
            }
        }
    }

    fn add_font(&mut self, name: impl Into<Name>, font: &Font, resolve: &'src T) {
        let decoder = match font.to_unicode(resolve) {
            Some(Ok(cmap)) => Decoder::Cmap(cmap),
            Some(Err(err)) => {
                warn!("ignoring font with a broken /ToUnicode map: {}", err);
                return;
            }
            None => {
                let encoding = match font.encoding() {
                    Some(encoding) => encoding,
                    None => return,
                };

                let map = match encoding.base {
                    BaseEncoding::StandardEncoding => Some(&pdf_encoding::STANDARD),
                    BaseEncoding::SymbolEncoding => Some(&pdf_encoding::SYMBOL),
                    BaseEncoding::WinAnsiEncoding => Some(&pdf_encoding::WINANSI),
                    BaseEncoding::MacRomanEncoding => Some(&pdf_encoding::MACROMAN),
                    BaseEncoding::None => None,
                    ref e => {
                        warn!("unsupported pdf encoding {:?}", e);
                        return;
                    }
                };

                Decoder::Map(DifferenceForwardMap::new(
                    map,
                    encoding
                        .differences
                        .iter()
                        .map(|(k, v)| (*k, v.to_string()))
                        .collect(),
                ))
            }
        };

        let mut widths = std::collections::HashMap::new();
        let two_byte = font.is_cid();
        if let Ok(Some(w)) = font.widths(resolve) {
            let last: u32 = if two_byte { 0xffff } else { 0xff };
            for code in 0..=last {
                widths.insert(code as u16, w.get(code as usize));
            }
        }

        self.fonts.insert(
            name.into(),
            Rc::new(FontInfo {
                decoder,
                widths,
                two_byte,
            }),
        );
    }

    fn get_by_font_name(&self, name: &Name) -> Rc<FontInfo> {
        self.fonts.get(name).unwrap_or(&self.default_font).clone()
    }

    fn get_by_graphic_state_name(&self, name: &Name) -> Option<(Rc<FontInfo>, f32)> {
        self.page
            .resources()
            .ok()
            .and_then(|resources| resources.graphics_states.get(name))
            .and_then(|gs| gs.font)
            .map(|(font, font_size)| {
                let font = self
                    .resolve
                    .get(font)
                    .ok()
                    .and_then(|font| Some(self.get_by_font_name(font.name.as_ref()?)))
                    .unwrap_or_else(|| self.default_font.clone());

                (font, font_size)
            })
    }
}

#[derive(Clone, Default)]
pub struct TextState {
    pub font: Rc<FontInfo>,
    pub font_size: f32,
    pub text_leading: f32,
    pub text_matrix: Transform2D<f32, PdfSpace, PdfSpace>,
}

pub fn ops_with_text_state<'src, T: Resolve>(
    page: &'src Page,
    resolve: &'src T,
) -> impl Iterator<Item = (Op, Rc<TextState>)> + 'src {
    page.contents.iter().flat_map(move |contents| {
        contents.operations(resolve).unwrap().into_iter().scan(
            (Rc::new(TextState::default()), FontCache::new(page, resolve)),
            |(state, font_cache), op| {
                let mut update_state = |update_fn: &dyn Fn(&mut TextState)| {
                    let old_state: &TextState = state;
                    let mut new_state = old_state.clone();

                    update_fn(&mut new_state);

                    *state = Rc::new(new_state);
                };

                match op {
                    Op::BeginText => {
                        *state = Default::default();
                    }
                    Op::GraphicsState { ref name } => {
                        update_state(&|state: &mut TextState| {
                            if let Some((font, font_size)) =
                                font_cache.get_by_graphic_state_name(name)
                            {
                                state.font = font;
                                state.font_size = font_size;
                            }
                        });
                    }
                    Op::TextFont { ref name, size } => {
                        update_state(&|state: &mut TextState| {
                            state.font = font_cache.get_by_font_name(name);
                            state.font_size = size;
                        });
                    }
                    Op::Leading { leading } => {
                        update_state(&|state: &mut TextState| state.text_leading = leading);
                    }
                    Op::TextNewline => {
                        update_state(&|state: &mut TextState| {
                            state.text_matrix = state.text_matrix.pre_translate(
                                Point {
                                    x: 0.0f32,
                                    y: state.text_leading,
                                }
                                .into(),
                            );
                        });
                    }
                    Op::MoveTextPosition { translation } => {
                        update_state(&|state: &mut TextState| {
                            state.text_matrix = state.text_matrix.pre_translate(translation.into());
                        });
                    }
                    Op::SetTextMatrix { matrix } => {
                        update_state(&|state: &mut TextState| {
                            state.text_matrix = matrix.into();
                        });
                    }
                    _ => {}
                }

                Some((op, state.clone()))
            },
        )
    })
}

pub fn page_text(page: &Page, resolve: &impl Resolve) -> Result<String, PdfError> {
    let mut out = String::new();

    for (op, text_state) in ops_with_text_state(page, resolve) {
        match op {
            Op::TextDraw { ref text } => text_state.font.decode(&text.data, &mut out)?,
            Op::TextDrawAdjusted { ref array } => {
                for data in array {
                    if let TextDrawAdjusted::Text(text) = data {
                        text_state.font.decode(&text.data, &mut out)?;
                    }
                }
            }
            Op::TextNewline => {
                out.push('\n');
            }
            Op::MoveTextPosition { translation } => {
                if translation.y.abs() < f32::EPSILON {
                    out.push('\n');
                }
            }
            Op::SetTextMatrix { matrix } => {
                if (matrix.f - text_state.text_matrix.m32).abs() < f32::EPSILON {
                    out.push('\n');
                } else {
                    out.push('\t');
                }
            }
            _ => {}
        }
    }
    Ok(out)
}
