//! What a cut is allowed to write: every object at most once, and nothing the
//! selected pages do not actually use.
//!
//! The input files are built by hand so that the sharing is explicit: both
//! pages of `two_pages()` point at the very same font object, and the second
//! page has a font of its own. The drawn content is a stream, which the writer
//! may re-compress, so the checks count the plain dictionary objects (font
//! names) rather than bytes inside a stream.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use pdf_tools::merge::{self, Source};

fn temp_path(tag: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "pdf-cut-dedup-{}-{}-{}.pdf",
        std::process::id(),
        tag,
        n
    ));
    path
}

/// Builds a PDF out of ready-made object bodies, object `i` of the slice being
/// object number `i + 1`. Offsets are real, so the strict parser accepts it.
fn build_pdf(objects: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    let mut offsets = Vec::with_capacity(objects.len() + 1);
    offsets.push(0usize); // object 0 is always free
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
    }
    let xref_at = out.len();
    let size = objects.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets[1..] {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

/// A stream object with a correct `/Length`.
fn stream(content: &str) -> String {
    format!(
        "<< /Length {} >>\nstream\n{}endstream",
        content.len(),
        content
    )
}

/// Two pages. Both use font object 5 (`/SharedFontA`); the second page also
/// uses font object 7 (`/OnlyPageTwoFont`), which nothing on page one refers
/// to. Page one lists font object 9 (`/UnusedFontB`) in its resources without
/// ever selecting it.
fn two_pages() -> Vec<u8> {
    let objects = vec![
        // 1: catalog
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        // 2: page tree
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
        // 3: page one, sharing font 5, plus a font it never uses
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
         /Resources << /Font << /F1 5 0 R /F3 9 0 R >> >> /Contents 6 0 R >>"
            .to_string(),
        // 4: page two, sharing font 5, adding font 7
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
         /Resources << /Font << /F1 5 0 R /F2 7 0 R >> >> /Contents 8 0 R >>"
            .to_string(),
        // 5: the shared font
        "<< /Type /Font /Subtype /Type1 /BaseFont /SharedFontA \
         /Encoding /WinAnsiEncoding >>"
            .to_string(),
        // 6: page one content
        stream("BT /F1 12 Tf 20 100 Td (one) Tj ET\n"),
        // 7: page two only font
        "<< /Type /Font /Subtype /Type1 /BaseFont /OnlyPageTwoFont \
         /Encoding /WinAnsiEncoding >>"
            .to_string(),
        // 8: page two content, using both fonts
        stream("BT /F1 12 Tf 20 100 Td (two) Tj /F2 12 Tf ET\n"),
        // 9: listed on page one, selected by nothing
        "<< /Type /Font /Subtype /Type1 /BaseFont /UnusedFontB \
         /Encoding /WinAnsiEncoding >>"
            .to_string(),
    ];
    build_pdf(&objects)
}

fn count(haystack: &[u8], needle: &str) -> usize {
    let needle = needle.as_bytes();
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// Merges `pages` of `two_pages()` and returns the output bytes.
fn cut(tag: &str, pages: &str) -> Vec<u8> {
    let input = temp_path(&format!("in-{tag}"));
    let output = temp_path(&format!("out-{tag}"));
    fs::write(&input, two_pages()).unwrap();

    merge::merge(&[Source::new(&input, pages)], &output, false).unwrap();
    let bytes = fs::read(&output).unwrap();

    fs::remove_file(&input).ok();
    fs::remove_file(&output).ok();
    bytes
}

#[test]
fn a_shared_object_is_copied_once() {
    // Both pages point at the same font object; it must appear in the output
    // exactly once, not once per page.
    let output = cut("shared", "1,2");
    assert_eq!(
        count(&output, "/SharedFontA"),
        1,
        "the font shared by both pages was copied more than once"
    );
}

#[test]
fn an_object_only_a_selected_page_uses_is_copied() {
    // The counterpart: page two's own font is used, so it is there.
    let output = cut("page2", "1,2");
    assert_eq!(count(&output, "/OnlyPageTwoFont"), 1);
}

#[test]
fn objects_of_unselected_pages_are_not_written() {
    // Page two is not taken, so nothing that only it uses may be written.
    let output = cut("page1only", "1");
    assert_eq!(
        count(&output, "/OnlyPageTwoFont"),
        0,
        "a font of the unselected page was written"
    );
    assert_eq!(
        count(&output, "/SharedFontA"),
        1,
        "the selected page lost its font"
    );
}

#[test]
fn resources_a_page_lists_but_never_selects_are_not_written() {
    // Page one names /F3 in /Resources but no operator ever selects it, so it
    // would be dead weight in the output.
    let output = cut("unused", "1,2");
    assert_eq!(
        count(&output, "/UnusedFontB"),
        0,
        "a font that no content stream selects was written"
    );
}

#[test]
fn a_page_taken_twice_is_not_duplicated_blindly() {
    // The same page selected twice legitimately produces two page objects,
    // but the objects they share (font, content stream) stay single copies.
    let output = cut("twice", "1,1");
    assert_eq!(
        count(&output, "/SharedFontA"),
        1,
        "the font was copied once per page selection"
    );
    assert_eq!(count(&output, "/OnlyPageTwoFont"), 0);
}
