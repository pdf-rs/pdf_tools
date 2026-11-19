//! `pdf_inspect` — explore every bit of a PDF file.
//!
//! The design is two layers. `raw` reads the bytes as they are stored: the
//! `startxref` chain, xref tables and xref streams, every `N G obj` in the
//! file, every stream body bounded by `/Length` or by `endstream`. `Doc` adds
//! the library's typed view on top of that for pages, fonts, images and text.
//! Every command works on the raw layer first, so a file the library refuses
//! can still be inspected.

mod doc;
mod fmt;
mod media;
mod misc;
mod objects;
mod pages;
mod raw;
mod structure;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub const HELP: &str = "\
TARGET is an object reference in any of the forms `12`, `12 0`, `12.0` or
`12/0`. A PAGE is a 1-based page number; it also accepts a page object
reference, and `last` for the last page.

Every command prints what it finds and exits 0; a file that cannot be opened
still allows `xref`, `scan`, `obj` and `stream`, which work on the bytes.

EXAMPLES
  pdf_inspect info paper.pdf
  pdf_inspect objects paper.pdf --sort size --limit 20
  pdf_inspect obj paper.pdf 12 --depth 2
  pdf_inspect stream paper.pdf 27 --to content.txt
  pdf_inspect content paper.pdf 3 --ops
  pdf_inspect images paper.pdf --page 3 --save img/
  pdf_inspect text paper.pdf 3 --boxes
  pdf_inspect search paper.pdf \"Total\" --ignore-case
  pdf_inspect verify broken.pdf
";

/// Options that apply to every command.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub password: Option<String>,
    pub strict: bool,
    pub max_stream_mb: u64,
    pub max_file_mb: u64,
}

#[derive(Parser, Debug)]
#[command(
    name = "pdf_inspect",
    version,
    about = "Explore every bit of a PDF file",
    after_long_help = HELP
)]
struct Cli {
    /// Password for an encrypted document
    #[arg(long, global = true, value_name = "PW")]
    password: Option<String>,

    /// Parse strictly (default: tolerant)
    #[arg(long, global = true)]
    strict: bool,

    /// Cap on one decompressed stream or decoded image
    #[arg(long, global = true, value_name = "MB", default_value_t = 512)]
    max_stream_mb: u64,

    /// Refuse files larger than this
    #[arg(long, global = true, value_name = "MB", default_value_t = 2048)]
    max_file_mb: u64,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Summary: version, xref, objects, pages, encryption, metadata
    Info { file: PathBuf },

    /// The cross-reference chain: sections, subsections, trailer dictionaries
    Xref {
        file: PathBuf,
        /// List every entry, not just a per-section summary
        #[arg(long)]
        entries: bool,
        /// Include free entries when listing
        #[arg(long)]
        free: bool,
        /// Print each section's trailer dictionary
        #[arg(long)]
        trailers: bool,
    },

    /// Every object: where it lives, what it is, how big its stream is
    Objects {
        file: PathBuf,
        /// Only objects whose type contains this (e.g. Page, Image, Font)
        #[arg(long = "type", value_name = "TYPE")]
        ty: Option<String>,
        /// Only objects whose dictionary has this key (repeatable)
        #[arg(long = "has-key", value_name = "KEY")]
        has_key: Vec<String>,
        /// Only objects with a stream
        #[arg(long)]
        streams: bool,
        #[arg(long, value_name = "id|size|type", default_value = "id")]
        sort: String,
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },

    /// One object, as PDF syntax; --depth follows references
    Obj {
        file: PathBuf,
        target: String,
        #[arg(long, value_name = "N", default_value_t = 0)]
        depth: u32,
        /// Print the bytes exactly as stored instead of parsing them
        #[arg(long)]
        raw: bool,
    },

    /// A stream: filter chain, sizes, and its bytes decoded
    Stream {
        file: PathBuf,
        target: String,
        /// Do not apply the filters
        #[arg(long)]
        raw: bool,
        /// Write the decoded bytes to stdout as text
        #[arg(long)]
        print: bool,
        /// Hexdump the bytes
        #[arg(long)]
        hexdump: bool,
        /// Only this many bytes
        #[arg(long, value_name = "N")]
        head: Option<usize>,
        /// Write the bytes to this file
        #[arg(long, value_name = "PATH")]
        to: Option<PathBuf>,
    },

    /// The page tree: size, rotation, contents, resources per page
    Pages { file: PathBuf },

    /// One page in full: its dictionary, boxes, resources, streams, annots
    Page { file: PathBuf, page: String },

    /// A page's content stream: operators, or the decoded source
    Content {
        file: PathBuf,
        target: String,
        /// Print the decoded content stream source
        #[arg(long)]
        raw: bool,
        /// Count operators instead of listing them
        #[arg(long)]
        ops: bool,
        #[arg(long, value_name = "N", default_value_t = 500)]
        max_ops: usize,
    },

    /// The text on a page, decoded through the fonts
    Text {
        file: PathBuf,
        page: String,
        /// Do not insert line breaks between text runs
        #[arg(long)]
        raw: bool,
        /// One line per shown string: position, font, size, text
        #[arg(long)]
        boxes: bool,
    },

    /// Image XObjects: size, colour space, filters, where they are used
    Images {
        file: PathBuf,
        #[arg(long)]
        page: Option<usize>,
        /// Write each image to this directory
        #[arg(long, value_name = "DIR")]
        save: Option<PathBuf>,
    },

    /// One image, decoded to a picture file
    Image {
        file: PathBuf,
        target: String,
        #[arg(long, value_name = "PATH")]
        to: Option<PathBuf>,
    },

    /// Fonts: subtype, base font, encoding, embedded file, ToUnicode
    Fonts {
        file: PathBuf,
        #[arg(long)]
        page: Option<usize>,
    },

    /// The document outline (bookmarks)
    Outline { file: PathBuf },

    /// Annotations
    Annots {
        file: PathBuf,
        #[arg(long)]
        page: Option<usize>,
    },

    /// Info dictionary, XMP, document IDs, viewer preferences
    Meta { file: PathBuf },

    /// Embedded files (attachments)
    Attachments {
        file: PathBuf,
        /// Write each attachment to this directory
        #[arg(long, value_name = "DIR")]
        save: Option<PathBuf>,
    },

    /// Search decoded streams and page text
    Search {
        file: PathBuf,
        pattern: String,
        #[arg(long)]
        ignore_case: bool,
        /// Stop after this many hits
        #[arg(long, default_value_t = 40)]
        limit: usize,
        /// Characters of context around each hit
        #[arg(long, default_value_t = 60)]
        context: usize,
    },

    /// The physical layout: objects in file order, junk, orphans
    Scan { file: PathBuf },

    /// Consistency checks: xref offsets, /Length, references, page tree
    Verify { file: PathBuf },

    /// Which filters can be decoded, and by what
    Filters { file: PathBuf },
}

pub fn run_cli() -> ! {
    let cli = Cli::parse();
    let ctx = Ctx {
        password: cli.password,
        strict: cli.strict,
        max_stream_mb: cli.max_stream_mb,
        max_file_mb: cli.max_file_mb,
    };
    match run(&ctx, cli.cmd) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("pdf_inspect: {e}");
            std::process::exit(1);
        }
    }
}

fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    // The file argument is always the first field of every variant.
    let (file, rest) = match cmd {
        Cmd::Info { file } => (file, Action::Info),
        Cmd::Xref {
            file,
            entries,
            free,
            trailers,
        } => (
            file,
            Action::Xref {
                entries,
                free,
                trailers,
            },
        ),
        Cmd::Objects {
            file,
            ty,
            has_key,
            streams,
            sort,
            limit,
        } => (
            file,
            Action::Objects {
                ty,
                has_key,
                streams,
                sort,
                limit,
            },
        ),
        Cmd::Obj {
            file,
            target,
            depth,
            raw,
        } => (file, Action::Obj { target, depth, raw }),
        Cmd::Stream {
            file,
            target,
            raw,
            print,
            hexdump,
            head,
            to,
        } => (
            file,
            Action::Stream {
                target,
                raw,
                print,
                hexdump,
                head,
                to,
            },
        ),
        Cmd::Pages { file } => (file, Action::Pages),
        Cmd::Page { file, page } => (file, Action::Page { page }),
        Cmd::Content {
            file,
            target,
            raw,
            ops,
            max_ops,
        } => (
            file,
            Action::Content {
                target,
                raw,
                ops,
                max_ops,
            },
        ),
        Cmd::Text {
            file,
            page,
            raw,
            boxes,
        } => (file, Action::Text { page, raw, boxes }),
        Cmd::Images { file, page, save } => (file, Action::Images { page, save }),
        Cmd::Image { file, target, to } => (file, Action::Image { target, to }),
        Cmd::Fonts { file, page } => (file, Action::Fonts { page }),
        Cmd::Outline { file } => (file, Action::Outline),
        Cmd::Annots { file, page } => (file, Action::Annots { page }),
        Cmd::Meta { file } => (file, Action::Meta),
        Cmd::Attachments { file, save } => (file, Action::Attachments { save }),
        Cmd::Search {
            file,
            pattern,
            ignore_case,
            limit,
            context,
        } => (
            file,
            Action::Search {
                pattern,
                ignore_case,
                limit,
                context,
            },
        ),
        Cmd::Scan { file } => (file, Action::Scan),
        Cmd::Verify { file } => (file, Action::Verify),
        Cmd::Filters { file } => (file, Action::Filters),
    };

    let doc = doc::Doc::open(ctx, &file)?;
    match rest {
        Action::Info => structure::info(&doc),
        Action::Xref {
            entries,
            free,
            trailers,
        } => structure::xref(&doc, entries, free, trailers),
        Action::Objects {
            ty,
            has_key,
            streams,
            sort,
            limit,
        } => structure::objects(&doc, ty.as_deref(), &has_key, streams, &sort, limit),
        Action::Obj { target, depth, raw } => objects::obj(&doc, &target, depth, raw),
        Action::Stream {
            target,
            raw,
            print,
            hexdump,
            head,
            to,
        } => objects::stream(&doc, &target, raw, print, hexdump, head, to.as_deref()),
        Action::Pages => pages::pages(&doc),
        Action::Page { page } => pages::page(&doc, &page),
        Action::Content {
            target,
            raw,
            ops,
            max_ops,
        } => pages::content(&doc, &target, raw, ops, max_ops),
        Action::Text { page, raw, boxes } => pages::text(&doc, &page, raw, boxes),
        Action::Images { page, save } => media::images(&doc, page, save.as_deref()),
        Action::Image { target, to } => media::image(&doc, &target, to.as_deref()),
        Action::Fonts { page } => media::fonts(&doc, page),
        Action::Outline => misc::outline(&doc),
        Action::Annots { page } => misc::annots(&doc, page),
        Action::Meta => misc::meta(&doc),
        Action::Attachments { save } => misc::attachments(&doc, save.as_deref()),
        Action::Search {
            pattern,
            ignore_case,
            limit,
            context,
        } => misc::search(&doc, &pattern, ignore_case, limit, context),
        Action::Scan => structure::scan(&doc),
        Action::Verify => structure::verify(&doc),
        Action::Filters => misc::filters(&doc),
    }
}

/// The command after its file argument has been taken out.
enum Action {
    Info,
    Xref {
        entries: bool,
        free: bool,
        trailers: bool,
    },
    Objects {
        ty: Option<String>,
        has_key: Vec<String>,
        streams: bool,
        sort: String,
        limit: Option<usize>,
    },
    Obj {
        target: String,
        depth: u32,
        raw: bool,
    },
    Stream {
        target: String,
        raw: bool,
        print: bool,
        hexdump: bool,
        head: Option<usize>,
        to: Option<PathBuf>,
    },
    Pages,
    Page {
        page: String,
    },
    Content {
        target: String,
        raw: bool,
        ops: bool,
        max_ops: usize,
    },
    Text {
        page: String,
        raw: bool,
        boxes: bool,
    },
    Images {
        page: Option<usize>,
        save: Option<PathBuf>,
    },
    Image {
        target: String,
        to: Option<PathBuf>,
    },
    Fonts {
        page: Option<usize>,
    },
    Outline,
    Annots {
        page: Option<usize>,
    },
    Meta,
    Attachments {
        save: Option<PathBuf>,
    },
    Search {
        pattern: String,
        ignore_case: bool,
        limit: usize,
        context: usize,
    },
    Scan,
    Verify,
    Filters,
}
