//! The document handle: raw bytes plus the library's typed view.

use std::path::{Path, PathBuf};

use pdf::backend::Data;
use pdf::file::{File, FileOptions, NoCache, NoLog, OpenMode, Trailer};
use pdf::object::{ParseOptions, PlainRef, Resolve};
use pdf::primitive::{Dictionary, Primitive};

use super::raw::Raw;
use super::Ctx;

pub type PdfFile = File<Data, NoCache, NoCache, NoLog>;

pub struct Doc {
    pub path: PathBuf,
    pub raw: Raw,
    pub file: Option<PdfFile>,
    pub open_error: Option<String>,
    pub opts: ParseOptions,
}

impl Doc {
    pub fn open(ctx: &Ctx, path: &Path) -> Result<Doc, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let limit = ctx.max_file_mb.saturating_mul(1024 * 1024);
        if meta.len() > limit {
            return Err(format!(
                "{}: {} is larger than the {} cap (--max-file-mb)",
                path.display(),
                super::fmt::human_bytes(meta.len()),
                super::fmt::human_bytes(limit)
            ));
        }
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;

        let mut opts = if ctx.strict {
            ParseOptions::strict()
        } else {
            ParseOptions::tolerant()
        };
        opts.object_size_limit = ctx.max_stream_mb.saturating_mul(1024 * 1024) as usize;
        opts.max_img_dim = 65536;
        opts.allow_missing_endobj = true;

        let password = ctx.password.clone().unwrap_or_default();
        let opened = FileOptions::uncached()
            .password(password.as_bytes())
            .parse_options(opts.clone())
            .max_file_size(Some(limit))
            .open_mode(OpenMode::Read)
            .open(path);
        let (file, open_error) = match opened {
            Ok(f) => (Some(f), None),
            Err(e) => (None, Some(e.to_string())),
        };

        Ok(Doc {
            path: path.to_path_buf(),
            raw: Raw::new(data),
            file,
            open_error,
            opts,
        })
    }

    pub fn typed(&self) -> Result<&PdfFile, String> {
        self.file.as_ref().ok_or_else(|| {
            format!(
                "the pdf crate cannot open this file: {}",
                self.open_error.as_deref().unwrap_or("unknown error")
            )
        })
    }

    pub fn resolver(&self) -> Result<impl Resolve + '_, String> {
        Ok(self.typed()?.resolver())
    }

    pub fn resolve(&self, r: PlainRef) -> Result<Primitive, String> {
        self.resolver()?.resolve(r).map_err(|e| e.to_string())
    }

    pub fn resolve_ids(&self, id: u64, gen: u64) -> Result<Primitive, String> {
        self.resolve(PlainRef { id, gen })
    }

    pub fn trailer(&self) -> Result<&Trailer, String> {
        Ok(&self.typed()?.trailer)
    }

    /// Page objects, in order, from the library's page tree walk.
    pub fn page_refs(&self) -> Result<Vec<PlainRef>, String> {
        let file = self.typed()?;
        let mut out = Vec::new();
        for (i, page) in file.pages().enumerate() {
            match page {
                Ok(p) => out.push(p.get_ref()),
                Err(e) => return Err(format!("page tree: page {}: {e}", i + 1)),
            }
        }
        Ok(out)
    }

    pub fn page_count(&self) -> Result<u32, String> {
        Ok(self.typed()?.num_pages())
    }

    /// A 1-based page.
    pub fn page(&self, n: usize) -> Result<pdf::object::PageRc, String> {
        let count = self.page_count()?;
        if n == 0 || n > count as usize {
            return Err(format!(
                "page {n} is out of range, the document has {count} page(s)"
            ));
        }
        self.typed()?
            .get_page((n - 1) as u32)
            .map_err(|e| format!("page {n}: {e}"))
    }

    pub fn is_encrypted(&self) -> bool {
        self.file
            .as_ref()
            .map(|f| f.trailer.encrypt_dict.is_some())
            .unwrap_or(false)
    }

    /// The stream dictionary and its decoded bytes for an object.
    pub fn stream_data(
        &self,
        r: PlainRef,
    ) -> Result<(Dictionary, Vec<u8>, Option<&'static str>), String> {
        let resolve = self.resolver()?;
        let prim = resolve.resolve(r).map_err(|e| e.to_string())?;
        let stream = match prim {
            Primitive::Stream(s) => s,
            other => {
                return Err(format!(
                    "object {} {} is not a stream ({})",
                    r.id,
                    r.gen,
                    super::fmt::kind_of(&other)
                ))
            }
        };
        let dict = stream.info.clone();
        let raw = stream
            .raw_data(&resolve)
            .map_err(|e| format!("stream data: {e}"))?;
        let specs = super::raw::filter_specs(&dict, &resolve);
        let (data, codec) = super::raw::decode_chain(&raw, &specs, &self.opts)?;
        Ok((dict, data, codec))
    }
}
