//! Copying pages out of one or more documents into a new one.
//!
//! Everything is built on the writing support in the `pdf` crate
//! (`PdfBuilder`, `PageBuilder`, `CatalogBuilder` and `Importer`): the pages
//! are deep cloned out of their source documents, so the result is a
//! self-contained file with no dependency on the inputs.

use std::fmt;
use std::path::{Path, PathBuf};

use pdf::build::{CatalogBuilder, Importer, PageBuilder, PdfBuilder};
use pdf::error::PdfError;
use pdf::file::{FileOptions, OpenMode};
use pdf::object::Updater;

use crate::pages;

/// One input document and the pages to take from it.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    /// 1-based page selection, as accepted by [`crate::pages::expand`].
    pub pages: String,
}

impl Source {
    pub fn new(path: impl Into<PathBuf>, pages: impl Into<String>) -> Source {
        Source {
            path: path.into(),
            pages: pages.into(),
        }
    }
}

/// Anything that can go wrong while merging.
#[derive(Debug)]
pub enum Error {
    Pdf(PdfError),
    Io { path: PathBuf, err: std::io::Error },
    PageSelection { path: PathBuf, msg: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Pdf(err) => write!(f, "{}", err),
            Error::Io { path, err } => write!(f, "{}: {}", path.display(), err),
            Error::PageSelection { path, msg } => write!(f, "{}: {}", path.display(), msg),
        }
    }
}

impl std::error::Error for Error {}

impl From<PdfError> for Error {
    fn from(err: PdfError) -> Error {
        Error::Pdf(err)
    }
}

/// Writes a new document to `output` containing the selected pages of
/// `sources`, in the order they are given, and returns the number of pages
/// written.
///
/// With `verify`, the freshly written objects are compared against the source
/// objects (slow, but catches a broken import).
pub fn merge(sources: &[Source], output: &Path, verify: bool) -> Result<usize, Error> {
    let mut builder = PdfBuilder::new(FileOptions::cached());

    // `pdf` 0.10.0 numbers the objects of a new file from 0: `XRefTable::new`
    // pushes the mandatory free object 0 and then resizes the table to
    // `num_objects`, which throws the free entry away when the table starts
    // out empty (xref.rs, `XRefTable::new`). Everything then shifts down by
    // one, and a page that ends up as object 0 is written as `0 0 R`, which
    // readers reject (object 0 is always free). Reserving object 0 with a
    // placeholder keeps the real objects at 1 and above.
    builder.create(pdf::primitive::Primitive::Null)?;

    let mut new_pages = Vec::new();
    let mut written = 0;

    for source in sources {
        let file = FileOptions::cached()
            // merge inputs can be large; map rather than read when that pays
            .open_mode(OpenMode::Auto)
            .open(&source.path)
            .map_err(|err| Error::Io {
                path: source.path.clone(),
                err: std::io::Error::other(err.to_string()),
            })?;

        let indices = pages::expand(&source.pages, file.num_pages()).map_err(|msg| {
            Error::PageSelection {
                path: source.path.clone(),
                msg,
            }
        })?;

        let mut importer = Importer::new(file.resolver(), &mut builder.storage);
        for index in indices {
            let page = file.get_page(index)?;
            new_pages.push(PageBuilder::clone_page(&page, &mut importer)?);
            written += 1;
        }

        if verify {
            importer.finish().verify(&builder.storage.resolver())?;
        } else {
            importer.finish();
        }
    }

    if written == 0 {
        return Err(Error::PageSelection {
            path: output.to_path_buf(),
            msg: "nothing to write, no pages were selected".into(),
        });
    }

    let catalog = CatalogBuilder::from_pages(new_pages);
    let data = builder.build(catalog)?;

    std::fs::write(output, data).map_err(|err| Error::Io {
        path: output.to_path_buf(),
        err,
    })?;

    Ok(written)
}
