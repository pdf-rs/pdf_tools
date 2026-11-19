//! `pdf-cut` — extract pages from PDF files and merge them into one document.
//!
//! ```text
//! pdf-cut -i foo.pdf -p 2-5 -i bar.pdf -p 1 -o merged.pdf
//! ```

use std::path::PathBuf;
use std::process::exit;

use pdf_tools::merge::{self, Source};

const USAGE: &str = "\
pdf-cut — extract pages from PDF files and merge them into a new document

USAGE:
    pdf-cut -i <input.pdf> [-p <pages>] [-i <input.pdf> [-p <pages>]]... -o <output.pdf>

OPTIONS:
    -i, --input <file>    input PDF; may be given several times
    -p, --pages <spec>    the pages to take from the preceding -i (default: all)
                          1-based, as in a PDF viewer; accepts single pages,
                          ranges and lists:
                              3           page 3
                              2-5         pages 2 to 5
                              4-          page 4 to the end
                              -3          page 1 to 3
                              1,4,7-9     pages 1, 4, 7, 8 and 9
                          all  or  *     every page (the default)
    -o, --output <file>   the PDF to write
        --verify          read back every copied object and compare it with the
                          source (slow, catches a broken import)
    -q, --quiet           do not print the summary
    -h, --help            this text

The pages appear in the order the inputs and selections are given, so a page
can be taken twice by naming it twice.

EXAMPLE:
    pdf-cut -i foo.pdf -p 2-5 -i bar.pdf -p 1 -o merged.pdf
";

struct Args {
    sources: Vec<Source>,
    output: Option<PathBuf>,
    verify: bool,
    quiet: bool,
}

fn parse_args<I: Iterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut out = Args {
        sources: Vec::new(),
        output: None,
        verify: false,
        quiet: false,
    };

    let mut args = args;
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with('-') => (flag.to_string(), Some(value.to_string())),
            _ => (arg.clone(), None),
        };

        let mut value = |name: &str| -> Result<String, String> {
            match inline.clone() {
                Some(value) => Ok(value),
                None => args
                    .next()
                    .ok_or_else(|| format!("{} needs a value", name)),
            }
        };

        match flag.as_str() {
            "-i" | "--input" => {
                let path = value(&flag)?;
                out.sources.push(Source::new(path, "all"));
            }
            "-p" | "--pages" => {
                let spec = value(&flag)?;
                match out.sources.last_mut() {
                    Some(source) => {
                        if source.pages != "all" {
                            return Err(format!(
                                "page selection given twice for {}",
                                source.path.display()
                            ));
                        }
                        source.pages = spec;
                    }
                    None => return Err("-p needs a preceding -i".into()),
                }
            }
            "-o" | "--output" => {
                if out.output.is_some() {
                    return Err("-o given twice".into());
                }
                out.output = Some(PathBuf::from(value(&flag)?));
            }
            "--verify" => out.verify = true,
            "-q" | "--quiet" => out.quiet = true,
            "-h" | "--help" => {
                print!("{}", USAGE);
                exit(0);
            }
            other => return Err(format!("unknown argument {:?}", other)),
        }
    }

    if out.sources.is_empty() {
        return Err("no input given".into());
    }
    if out.output.is_none() {
        return Err("no output given (-o)".into());
    }
    Ok(out)
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("pdf-cut: {}\n\n{}", msg, USAGE);
            exit(2);
        }
    };

    let output = args.output.as_ref().expect("checked in parse_args");
    match merge::merge(&args.sources, output, args.verify) {
        Ok(pages) => {
            if !args.quiet {
                eprintln!("pdf-cut: wrote {} page(s) to {}", pages, output.display());
            }
        }
        Err(err) => {
            eprintln!("pdf-cut: {}", err);
            exit(1);
        }
    }
}
