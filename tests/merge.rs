//! End-to-end tests for [`pdf_tools::merge`]: build small PDFs, cut pages out
//! of them and read the result back.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use pdf::build::{CatalogBuilder, PageBuilder, PdfBuilder};
use pdf::content::{Content, Op, Point};
use pdf::file::FileOptions;
use pdf::object::{NoResolve, Rectangle};

use pdf_tools::merge::{self, Source};

/// A short-lived file name that no other test run shares.
fn temp_path(tag: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "pdf-cut-test-{}-{}-{}.pdf",
        std::process::id(),
        tag,
        n
    ));
    path
}

/// Writes a PDF with `pages` pages; page `i` (0-based) is `400 + i` units wide
/// and has a line drawn on it, so the pages can be told apart afterwards.
fn write_test_pdf(path: &Path, pages: usize) -> Result<(), pdf::error::PdfError> {
    let mut builder = PdfBuilder::new(FileOptions::cached());
    let mut builders = Vec::new();

    for i in 0..pages {
        let size = 400.0 + i as f32;
        let content = Content::from_ops(
            vec![
                Op::MoveTo {
                    p: Point { x: 10.0, y: 10.0 },
                },
                Op::LineTo {
                    p: Point {
                        x: size / 2.0,
                        y: size / 2.0,
                    },
                },
                Op::Stroke,
            ],
            &mut builder,
        )?;

        let mut page = PageBuilder::from_content(content, &NoResolve)?;
        page.media_box = Some(Rectangle {
            top: size,
            left: 0.0,
            bottom: 0.0,
            right: size,
        });
        builders.push(page);
    }

    let data = builder.build(CatalogBuilder::from_pages(builders))?;
    std::fs::write(path, data)?;
    Ok(())
}

/// The width of every page of `path`, in order.
fn page_widths(path: &Path) -> Vec<f32> {
    let file = FileOptions::cached().open(path).expect("cannot reopen output");
    (0..file.num_pages())
        .map(|n| {
            file.get_page(n)
                .expect("cannot read page")
                .media_box()
                .expect("no media box")
                .right
        })
        .collect()
}

#[test]
fn extracts_pages_in_the_given_order() {
    let input = temp_path("in");
    let output = temp_path("out");
    write_test_pdf(&input, 3).unwrap();
    assert_eq!(page_widths(&input), vec![400.0, 401.0, 402.0]);

    let written = merge::merge(&[Source::new(&input, "3,1")], &output, false).unwrap();

    assert_eq!(written, 2);
    assert_eq!(page_widths(&output), vec![402.0, 400.0]);

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

#[test]
fn merges_pages_of_several_documents() {
    let a = temp_path("a");
    let b = temp_path("b");
    let output = temp_path("ab");
    write_test_pdf(&a, 2).unwrap();
    write_test_pdf(&b, 2).unwrap();

    let written = merge::merge(
        &[Source::new(&a, "all"), Source::new(&b, "2")],
        &output,
        false,
    )
    .unwrap();

    assert_eq!(written, 3);
    assert_eq!(page_widths(&output), vec![400.0, 401.0, 401.0]);

    for path in [&a, &b, &output] {
        std::fs::remove_file(path).ok();
    }
}

#[test]
fn verifies_the_imported_objects() {
    let input = temp_path("verify-in");
    let output = temp_path("verify-out");
    write_test_pdf(&input, 2).unwrap();

    merge::merge(&[Source::new(&input, "2")], &output, true).unwrap();
    assert_eq!(page_widths(&output), vec![401.0]);

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

#[test]
fn reports_pages_that_do_not_exist() {
    let input = temp_path("short");
    let output = temp_path("short-out");
    write_test_pdf(&input, 2).unwrap();

    let err = merge::merge(&[Source::new(&input, "3")], &output, false).unwrap_err();
    assert!(
        err.to_string().contains("out of range"),
        "unexpected error: {}",
        err
    );
    assert!(!output.exists(), "no output should be written on error");

    std::fs::remove_file(&input).ok();
}
