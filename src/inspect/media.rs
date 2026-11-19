//! Images and fonts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pdf::enc::StreamFilter;
use pdf::font::Font;
use pdf::object::*;
use pdf::primitive::Primitive;

use super::doc::Doc;
use super::fmt::{human_bytes, prim_inline, table};

/// One image XObject turned into a file some other program can open.
fn image_file(
    img: &ImageXObject,
    resolver: &impl Resolve,
) -> Result<Option<(&'static str, Vec<u8>)>, String> {
    let (data, filter) = img.raw_image_data(resolver).map_err(|e| e.to_string())?;
    Ok(match filter {
        Some(StreamFilter::DCTDecode(_)) => Some(("jpeg", data.to_vec())),
        Some(StreamFilter::JPXDecode) => Some(("jp2", data.to_vec())),
        Some(StreamFilter::JBIG2Decode(_)) => Some(("jbig2", data.to_vec())),
        Some(StreamFilter::CCITTFaxDecode(_)) => Some((
            "tiff",
            fax::tiff::wrap(&data, img.width, img.height).into(),
        )),
        Some(StreamFilter::FlateDecode(_)) | Some(StreamFilter::BrotliDecode(_)) | None => {
            let samples = img.image_data(resolver).map_err(|e| e.to_string())?;
            match png_of(img, &samples)? {
                Some(png) => Some(("png", png)),
                None => None,
            }
        }
        Some(other) => {
            return Err(format!("no way to save an image with filter {other:?}"));
        }
    })
}

/// Wrap raw decoded samples in a PNG. `Ok(None)` when the colour space or bit
/// depth has no PNG equivalent we handle.
fn png_of(img: &ImageXObject, data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let bits = img.bits_per_component.unwrap_or(8);
    let depth = match bits {
        1 => png::BitDepth::One,
        2 => png::BitDepth::Two,
        4 => png::BitDepth::Four,
        8 => png::BitDepth::Eight,
        16 => png::BitDepth::Sixteen,
        _ => return Ok(None),
    };

    let mut palette: Option<Vec<u8>> = None;
    let color = if img.image_mask {
        png::ColorType::Grayscale
    } else {
        match &img.color_space {
            None | Some(ColorSpace::DeviceGray) | Some(ColorSpace::CalGray(_)) => {
                png::ColorType::Grayscale
            }
            Some(ColorSpace::DeviceRGB) | Some(ColorSpace::CalRGB(_)) => png::ColorType::Rgb,
            Some(ColorSpace::Indexed(base, _, lookup)) => {
                let base = match **base {
                    ColorSpace::DeviceRGB | ColorSpace::CalRGB(_) => 3,
                    ColorSpace::DeviceGray | ColorSpace::CalGray(_) => 1,
                    _ => return Ok(None),
                };
                let mut pal = Vec::with_capacity(lookup.len() / base * 3);
                for entry in lookup.chunks(base) {
                    match (base, entry) {
                        (3, [r, g, b]) => pal.extend_from_slice(&[*r, *g, *b]),
                        (1, [g]) => pal.extend_from_slice(&[*g, *g, *g]),
                        _ => break,
                    }
                }
                while pal.len() % 3 != 0 {
                    pal.push(0);
                }
                palette = Some(pal);
                png::ColorType::Indexed
            }
            _ => return Ok(None),
        }
    };

    let samples: Vec<u8> = if !img.image_mask
        && matches!(
            img.color_space,
            Some(ColorSpace::DeviceCMYK) | Some(ColorSpace::CalCMYK(_))
        ) {
        if bits != 8 {
            return Ok(None);
        }
        data.chunks(4)
            .flat_map(|px| {
                let k = 255 - px.get(3).copied().unwrap_or(0) as u32;
                let conv = |c: u8| ((255 - c as u32) * k / 255) as u8;
                [conv(px[0]), conv(px[1]), conv(px[2])]
            })
            .collect()
    } else {
        data.to_vec()
    };

    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(color);
        enc.set_depth(depth);
        if let Some(pal) = palette.as_deref() {
            enc.set_palette(pal);
        }
        let mut writer = enc.write_header().map_err(|e| format!("png header: {e}"))?;
        writer
            .write_image_data(&samples)
            .map_err(|e| format!("png data: {e}"))?;
        writer.finish().map_err(|e| format!("png: {e}"))?;
    }
    Ok(Some(out))
}

fn colorspace_name(img: &ImageXObject) -> String {
    if img.image_mask {
        return "image mask".into();
    }
    match &img.color_space {
        None => "none".into(),
        Some(cs) => {
            let s = format!("{cs:?}");
            let s = s.split(['(', ' ', '{']).next().unwrap_or(&s).to_string();
            s
        }
    }
}

struct Gathered {
    width: u32,
    height: u32,
    bpc: i32,
    cs: String,
    filters: Vec<String>,
    pages: Vec<usize>,
    names: Vec<String>,
}

pub fn images(doc: &Doc, page: Option<usize>, save: Option<&Path>) -> Result<(), String> {
    let resolve = doc.resolver()?;
    let count = doc.page_count()?;
    let pages: Vec<usize> = match page {
        Some(p) => vec![p],
        None => (1..=count as usize).collect(),
    };
    let mut found: BTreeMap<(u64, u64), Gathered> = BTreeMap::new();
    for n in pages {
        let page = doc.page(n)?;
        let resources = match page.resources() {
            Ok(r) => r,
            Err(e) => {
                println!("page {n}: no resources ({e})");
                continue;
            }
        };
        for (name, reference) in resources.xobjects.iter() {
            let object = match resolve.get(reference.clone()) {
                Ok(o) => o,
                Err(e) => {
                    println!("page {n}: /{} {} {} R: {e}", name.as_str(), reference.get_inner().id, reference.get_inner().gen);
                    continue;
                }
            };
            let img = match &*object {
                XObject::Image(img) => img,
                _ => continue,
            };
            let pref = reference.get_inner();
            let key = (pref.id, pref.gen);
            let entry = found.entry(key).or_insert_with(|| Gathered {
                width: img.width,
                height: img.height,
                bpc: img.bits_per_component.unwrap_or(1),
                cs: colorspace_name(img),
                filters: img
                    .inner
                    .info
                    .filters
                    .iter()
                    .map(|f| f.name().to_string())
                    .collect(),
                pages: Vec::new(),
                names: Vec::new(),
            });
            if !entry.pages.contains(&n) {
                entry.pages.push(n);
            }
            if !entry.names.contains(&name.as_str().to_string()) {
                entry.names.push(name.as_str().to_string());
            }
        }
    }

    if found.is_empty() {
        println!("no image XObjects on the selected page(s)");
        return Ok(());
    }

    let mut rows = Vec::new();
    for ((id, gen), g) in &found {
        let mut decoded = 0u64;
        if let Ok(prim) = doc.resolve_ids(*id, *gen) {
            if let Primitive::Stream(_) = prim {
                if let Ok((_, data, _)) = doc.stream_data(PlainRef { id: *id, gen: *gen }) {
                    decoded = data.len() as u64;
                }
            }
        }
        let pages: Vec<String> = g.pages.iter().map(|p| p.to_string()).collect();
        rows.push(vec![
            format!("{id} {gen}"),
            format!("/{}", g.names.join(" /")),
            format!("{}x{}", g.width, g.height),
            format!("{}", g.bpc),
            g.cs.clone(),
            g.filters.join(","),
            human_bytes(decoded),
            pages.join(","),
        ]);
    }
    print!(
        "{}",
        table(
            &["ref", "name", "size", "bpc", "colorspace", "filters", "decoded", "pages"],
            &rows
        )
    );

    if let Some(dir) = save {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for ((id, gen), _) in &found {
            let prim = doc.resolve_ids(*id, *gen)?;
            let img = ImageXObject::from_primitive(prim, &resolve)
                .map_err(|e| format!("image {id} {gen}: {e}"))?;
            match image_file(&img, &resolve) {
                Ok(Some((ext, data))) => {
                    let path = dir.join(format!("img_{id}_{gen}.{ext}"));
                    std::fs::write(&path, &data)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    println!("wrote {}", path.display());
                }
                Ok(None) => println!("image {id} {gen}: no file format for this one"),
                Err(e) => println!("image {id} {gen}: {e}"),
            }
        }
    }
    Ok(())
}

pub fn image(doc: &Doc, target: &str, to: Option<&Path>) -> Result<(), String> {
    let (id, gen) = super::objects::parse_target(target)?;
    let resolve = doc.resolver()?;
    let prim = doc.resolve_ids(id, gen)?;
    let img = ImageXObject::from_primitive(prim.clone(), &resolve)
        .map_err(|e| format!("object {id} {gen} is not an image: {e}"))?;
    println!("image     {id} {gen}");
    println!("size      {} x {} px", img.width, img.height);
    println!("bpc       {}", img.bits_per_component.unwrap_or(1));
    println!("cs        {}", colorspace_name(&img));
    println!("filters   {:?}", img.inner.info.filters.iter().map(|f| f.name().to_string()).collect::<Vec<_>>());
    if img.image_mask {
        println!("mask      yes");
    }
    if let Some(mask) = img.inner.info.info.other.get("SMask") {
        println!("smask     {}", prim_inline(mask));
    }
    let (ext, data) = match image_file(&img, &resolve)? {
        Some(pair) => pair,
        None => return Err("no file format for this image's colour space / bit depth".into()),
    };
    match to {
        Some(path) => {
            std::fs::write(path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
            let mismatch = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let ok = match ext {
                        "jpeg" => e == "jpg" || e == "jpeg",
                        other => e.eq_ignore_ascii_case(other),
                    };
                    !ok
                })
                .unwrap_or(false);
            println!(
                "wrote     {} ({ext}, {}){}",
                path.display(),
                human_bytes(data.len() as u64),
                if mismatch {
                    " — note: the data is not in the format the extension says"
                } else {
                    ""
                }
            );
        }
        None => {
            let out = PathBuf::from(format!("image_{id}_{gen}.{ext}"));
            std::fs::write(&out, &data).map_err(|e| format!("{}: {e}", out.display()))?;
            println!("wrote     {} ({ext}, {})", out.display(), human_bytes(data.len() as u64));
        }
    }
    Ok(())
}

pub fn fonts(doc: &Doc, page: Option<usize>) -> Result<(), String> {
    let resolve = doc.resolver()?;
    let count = doc.page_count()?;
    let pages: Vec<usize> = match page {
        Some(p) => vec![p],
        None => (1..=count as usize).collect(),
    };
    // ref -> (pages, names)
    let mut found: BTreeMap<(u64, u64), (Vec<usize>, Vec<String>)> = BTreeMap::new();
    for n in pages {
        let page = doc.page(n)?;
        let resources = match page.resources() {
            Ok(r) => r,
            Err(_) => continue,
        };
        for (name, font) in resources.fonts.iter() {
            let reference = match &font.primitive {
                Primitive::Reference(r) => (r.id, r.gen),
                _ => (0, 0),
            };
            let entry = found.entry(reference).or_default();
            if !entry.0.contains(&n) {
                entry.0.push(n);
            }
            let name = name.as_str().to_string();
            if !entry.1.contains(&name) {
                entry.1.push(name);
            }
        }
    }
    let mut rows = Vec::new();
    for ((id, gen), (pages, names)) in &found {
        let (subtype, base, encoding, embedded, tounicode, widths) =
            match resolve.resolve(PlainRef { id: *id, gen: *gen }) {
                Ok(_) => match resolve.get(Ref::<Font>::new(PlainRef { id: *id, gen: *gen })) {
                    Ok(font) => {
                        let base = font
                            .info()
                            .and_then(|i| i.base_font.as_ref().map(|n| n.as_str().to_string()))
                            .or_else(|| font.name.as_ref().map(|n| n.as_str().to_string()))
                            .unwrap_or_default();
                        let encoding = font
                            .encoding()
                            .map(|e| format!("{:?}", e.base))
                            .unwrap_or_else(|| "-".into());
                        let embedded = font
                            .embedded_data(&resolve)
                            .and_then(|r| r.ok())
                            .map(|d| human_bytes(d.len() as u64))
                            .unwrap_or_else(|| "-".into());
                        let tounicode = match font.to_unicode(&resolve) {
                            Some(Ok(map)) => format!("{} entries", map.len()),
                            Some(Err(_)) => "broken".into(),
                            None => "-".into(),
                        };
                        let widths = font
                            .widths(&resolve)
                            .ok()
                            .flatten()
                            .map(|w| format!("{w:?}").chars().take(1).collect::<String>())
                            .unwrap_or_default();
                        let _ = widths;
                        let widths = if font.is_cid() { "cid".to_string() } else { "simple".to_string() };
                        (
                            format!("{:?}", font.subtype),
                            base,
                            encoding,
                            embedded,
                            tounicode,
                            widths,
                        )
                    }
                    Err(e) => (
                        "?".into(),
                        format!("<{e}>"),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                    ),
                },
                Err(e) => (
                    "?".into(),
                    format!("<{e}>"),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                ),
            };
        let pages: Vec<String> = pages.iter().map(|p| p.to_string()).collect();
        rows.push(vec![
            format!("{id} {gen}"),
            format!("/{}", names.join(" /")),
            subtype,
            base,
            encoding,
            embedded,
            tounicode,
            widths,
            pages.join(","),
        ]);
    }
    print!(
        "{}",
        table(
            &["ref", "name", "subtype", "base font", "encoding", "embedded", "tounicode", "kind", "pages"],
            &rows
        )
    );
    Ok(())
}
