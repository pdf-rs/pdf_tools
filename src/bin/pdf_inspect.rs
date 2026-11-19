//! `pdf_inspect` — explore every bit of a PDF file.
//!
//! The implementation lives in the library as `pdf_tools::inspect`, so that it
//! can use the text extraction here; this binary is only the entry point.

fn main() {
    pdf_tools::inspect::run_cli();
}
