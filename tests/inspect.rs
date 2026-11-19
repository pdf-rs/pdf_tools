//! A real page from a real generator: `tests/data/weasyprint.pdf`, ten A4
//! pages out of WeasyPrint 62.3, chosen because it exercises what the
//! inspector is for and what once went wrong.
//!
//!  * Four Type0 / Identity-H fonts: one character of /ToUnicode is two bytes,
//!    and the decoder used to take them one at a time ("ISECOM" came out as
//!    "IGECB"). `text` and `layout` must agree on the string.
//!  * CFF subsets whose /FontMatrix is a real number the font parser cannot
//!    read (2048 units/em, reported as a sheared matrix): the /Widths check
//!    has to fall back to the scale the file itself implies instead of
//!    printing nonsense ratios.
//!
//! The file is a fixture, not a fixture *in git*: it is not committed, so the
//! test skips (it does not fail) when the checkout does not have it.

use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/weasyprint.pdf")
}

fn inspect(args: &[&str]) -> String {
    let exe = env!("CARGO_BIN_EXE_pdf_inspect");
    let out = Command::new(exe)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {exe}: {e}"));
    assert!(
        out.status.success(),
        "pdf_inspect {args:?} exited with {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("output is not UTF-8")
}

#[test]
fn weasyprint_identity_h_and_widths() {
    let path = fixture();
    if !path.exists() {
        eprintln!("skipping: {} is not there", path.display());
        return;
    }
    let pdf = path.to_str().expect("fixture path is UTF-8");

    // Ten A4 pages, one content stream and four fonts each.
    let pages = inspect(&["pages", pdf]);
    let rows = pages
        .lines()
        .filter(|l| l.split_whitespace().next().is_some_and(|w| w.parse::<u32>().is_ok()))
        .count();
    assert_eq!(rows, 10, "page rows:\n{pages}");
    assert!(pages.contains("595.2756 x 841.8898"), "{pages}");
    // Two-byte codes through /ToUnicode: the title sets as the words of the
    // cover, not as the low bytes of its CIDs.
    let text = inspect(&["text", pdf, "1"]);
    assert!(text.contains("ISECOM"), "{text}");
    assert!(text.contains("Our AI Future"), "{text}");
    assert!(text.contains("is Glowpunk"), "{text}");

    // The renderer's own layout agrees, and the advances are the file's.
    let layout = inspect(&["layout", pdf, "1", "--chars", "--max-chars", "2", "--check"]);
    assert!(
        layout.contains("page 1: 7 text span(s), 99 character(s)"),
        "{layout}"
    );
    assert!(layout.contains("0 char(s) whose /Widths disagrees"), "{layout}");
    assert!(layout.contains("every character has a glyph"), "{layout}");
    // Origin of the first glyph, in points from the top-left of the page.
    assert!(layout.contains("70.87"), "{layout}");

    // Four embedded CID fonts, and a file that parses without a fault.
    let fonts = inspect(&["fonts", pdf]);
    assert_eq!(fonts.matches("Type0").count(), 4, "{fonts}");
    assert_eq!(fonts.matches("cid").count(), 4, "{fonts}");
    assert!(inspect(&["verify", pdf]).starts_with("0 issue(s)"));
}
