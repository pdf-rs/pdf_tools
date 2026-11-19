# pdf_tools
Possibly useful tools for working with PDFs that don't fit into the pdf crate.

# Contents

## Page text extractor:
See `src/bin/pdf2text.rs` for details.

## Page extractor and merger: `pdf-cut`

Copies selected pages out of one or more PDFs into a new one. Pages are deep
cloned with the writers from the pdf crate (`PdfBuilder`, `PageBuilder`,
`CatalogBuilder`, `Importer`), so the result is self-contained: fonts, images
and other resources a page refers to are copied along with it.

```text
pdf-cut -i foo.pdf -p 2-5 -i bar.pdf -p 1 -o merged.pdf
```

Pages appear in the order the inputs and selections are given, so a page can be
taken twice by naming it twice (`-i a.pdf -p 3,1` takes page 3 and then page 1).

### Options

| option | meaning |
| --- | --- |
| `-i`, `--input <file>` | input PDF, may be given several times |
| `-p`, `--pages <spec>` | pages to take from the preceding `-i` (default: all pages) |
| `-o`, `--output <file>` | the PDF to write |
| `--verify` | compare every copied object with its source (slow, catches a broken import) |
| `-q`, `--quiet` | do not print the summary |

### Page selections

Page numbers are 1-based, the way a PDF viewer counts them.

| spec | pages |
| --- | --- |
| `3` | page 3 |
| `2-5` | pages 2, 3, 4, 5 |
| `4-` | page 4 to the last page |
| `-3` | pages 1, 2, 3 |
| `1,4,7-9` | pages 1, 4, 7, 8, 9 |
| `all` / `*` | every page |

A selection that runs past the end of a document is an error rather than being
silently clipped, so a typo cannot quietly drop pages:
`pdf-cut: foo.pdf: page 9 is out of range, the document has 7 page(s)`.

### Exit codes

`0` on success, `1` when the PDFs cannot be read or written, `2` for a bad
command line.

# Notes

## pdf 0.10 object numbering

Written files number their objects from 0, because `XRefTable::new` in
`pdf/src/xref.rs` pushes the mandatory free object 0 and then resizes the table
to `num_objects`, which drops that entry again when a new document starts out
with an empty table. Object 0 is free by definition (`7.5.4`), so a reference
to it, such as a page in `/Kids`, is unreadable: `Tried to dereference free
object nr 0.`

`pdf-cut` works around this by writing an empty placeholder as object 0 before
anything else, which keeps every real object at 1 and above. The clean fix is
in the pdf crate: `entries.resize(num_objects.max(1) as usize, XRef::Invalid);`

## What the output contains

`pdf-cut` copies *pages*, not documents: a page is rebuilt from its content
stream and the resources its operators actually select.
`PageBuilder::clone_page` walks the operators and pulls in the fonts, colour
spaces and XObjects they name, so

* an object used by several selected pages is copied once (the `Importer`
  keys its copies by the source reference), and
* an object nothing selected uses — a resource a page lists but never selects,
  the content of a page that was not taken — is not written at all.

`tests/dedup.rs` pins both down: it merges two hand-built pages that share one
font and checks the font appears once, that a font only an unselected page uses
does not appear, and that a font listed in `/Resources` but never selected by
an operator does not appear either.

## Building against a local pdf checkout

`Cargo.toml` carries a `[patch.crates-io]` that points `pdf` at
`/home/hematite/task-clones/pdf/pdf`, the local checkout with the fixes this
tool was developed against. Drop that section when the fixes are upstream and
released; nothing else depends on it.

## pdf 0.9 vs 0.10

This crate now builds against `pdf` 0.10, where `/Font` entries of a page's
resources are `Lazy<Font>` instead of `MaybeRef<Font>`; `src/lib.rs` loads them
with `Lazy::load`.

## PDF inspector: `pdf_inspect`

`pdf_inspect` answers questions about a PDF file — what is in it, where, and
why it renders the way it does. It is built on two layers: a raw one that
reads the bytes as they are stored (the `startxref` chain, xref tables and
xref streams, every `N G obj` in the file, every stream bounded by `/Length`
*and* by `endstream`), and the `pdf` crate's typed view for pages, fonts,
images and text. Every command works on the raw layer first, so a file the
library refuses to open can still be inspected.

```text
cargo build --release --bin pdf_inspect
```

Global options: `--password PW` for encrypted files, `--strict` (default is
tolerant), `--max-stream-mb`, `--max-file-mb`.

| command | what it shows |
| --- | --- |
| `info FILE` | one screen: header, %%EOF, xref kind and count, objects, startxref target, trailer, pages, Info dictionary, XMP, forms |
| `xref FILE [--entries] [--free] [--trailers]` | the cross-reference chain, newest first: sections, subsections, `/Prev`, `/XRefStm`, every entry, each trailer dictionary |
| `objects FILE [--type T] [--has-key K] [--streams] [--sort id\|size\|type] [--limit N]` | every object: where it lives (offset or `objstm 12[3]`), its type, stream size, filter chain |
| `obj FILE TARGET [--depth N] [--raw]` | one object as PDF syntax; `--depth` follows references, `--raw` prints the stored bytes |
| `stream FILE TARGET [--raw] [--print] [--hexdump] [--head N] [--to PATH]` | a stream: filter chain, declared vs stored length, decoded size, and its bytes |
| `pages FILE` | the page tree: size, rotation, content parts, resource counts, annotations |
| `page FILE PAGE` | one page: the dictionary exactly as stored, then boxes, rotation, contents, resources, annotations |
| `content FILE PAGE\|REF [--raw] [--ops]` | a content stream: operators with their operands, `--raw` for the decoded source, `--ops` for counts |
| `text FILE PAGE [--boxes]` | the text, decoded through each font's `/ToUnicode` or `/Encoding`; `--boxes` gives position, size, advance and the gap to the next run |
| `images FILE [--page N] [--save DIR]` | image XObjects: size, colour space, filters, decoded size, which pages use them |
| `image FILE TARGET [--to PATH]` | one image, written as PNG / JPEG / JPEG 2000 / JBIG2 / TIFF |
| `fonts FILE [--page N]` | fonts: subtype, base font, encoding, embedded file size, `/ToUnicode`, simple or CID |
| `outline FILE`, `annots FILE [--page N]` | bookmarks, annotations |
| `meta FILE` | trailer, Info dictionary, catalog keys, XMP |
| `attachments FILE [--save DIR]` | embedded files from the `/Names` tree |
| `search FILE PATTERN [--ignore-case]` | search page text and every decoded stream, with context |
| `scan FILE` | the physical layout: objects in file order, orphans, dangling xref entries, gaps, `/Length` disagreements |
| `verify FILE` | consistency checks: xref offsets that point at the wrong object, `/Length`, decodable streams, references to missing objects, `/Size`, the page tree |
| `filters FILE` | the filters this build decodes, and which ones the file uses |

`TARGET` is an object reference: `12`, `12 0`, `12.0` or `12/0`. `PAGE` is
1-based, accepts `last`, and also accepts a page object reference.

### Examples

```text
pdf_inspect info paper.pdf
pdf_inspect objects paper.pdf --sort size --limit 20      # what is big in here?
pdf_inspect obj paper.pdf 12 --depth 2                    # follow /Contents, /Resources, ...
pdf_inspect stream paper.pdf 27 --to page3.txt            # decompress a stream
pdf_inspect content paper.pdf 3 --ops                     # what does the page draw?
pdf_inspect text paper.pdf 3 --boxes                      # where is each run, and how wide?
pdf_inspect images paper.pdf --page 3 --save img/
pdf_inspect search paper.pdf "Total" --ignore-case
pdf_inspect verify broken.pdf
```

### Checking text rendering and kerning

`text --boxes` prints, for every shown string, where it starts (`x`, `y`), the
font size, the advance the font's `/Widths` predicts, and `dx`: the difference
between where the string actually starts and where the previous string's
advance says it should. PDF text is not kerned — the advance is exactly the
sum of the glyph widths (plus `Tc`/`Tw`, which this does not see) — so a
non-zero `dx` means the widths the document declares do not match the layout,
which is what a renderer will draw. Rows with a large `dx` are marked with `!`
and counted at the end.

A font whose `/Widths` does not cover the codes in the string (some TeX subset
fonts) reports a small advance; that is the file's own information, and the
`dx` column shows the disagreement rather than hiding it.
### `layout`: what the renderer will actually draw

`text` decodes the operators itself and adds up advances — a second opinion.
`layout` instead drives `pdf_render`'s own tracer over the page: the same
`render_page` call the viewer makes, with a tracer in place of a rasteriser.
Whatever it prints is what the engine will draw.

```
pdf_inspect layout paper.pdf 3            # one row per text span
pdf_inspect layout paper.pdf 3 --chars    # one row per character
pdf_inspect layout paper.pdf 3 --check    # verdict for the whole page
```

Coordinates are PDF points from the top-left of the page as displayed
(`/Rotate` applied, page cropped to its media box) — the same ruler a
screenshot and a tap position use. That makes the numbers comparable with
another engine's word boxes, e.g. `pdftotext -bbox`:

```
<word xMin="133.768" ...>Problem</word>      poppler
0.0  133.77  134.77  0.000  11.025  ...      pdf_inspect layout --chars
```

Per span: the operator number it came from, baseline origin, font size, the
advance along the baseline in points, the distance to the next span on the
same baseline, the font and the text.

Per character (`--chars`): page position, position and advance in text space
(the numbers before the text matrix), the gap to the next character, the glyph
id, the character, and — when `/Widths` and the font program disagree — by how
much, as a percentage. `/Widths` is what every viewer places glyphs with, so a
non-zero ratio here is text that is drawn nudged relative to its own design:
exactly what "the kerning looks wrong" turns out to be.

`--check` counts, for the whole page:

* spans with no font — the text will not appear at all;
* characters with no glyph (`.notdef`) — visible boxes or blanks;
* spans that contain TJ kerning (advances that vary inside one span), which is
  normal for anything produced by TeX;
* characters whose `/Widths` entry disagrees with the font by more than 1 %.

The comparison needs to know how many font units an em is, which normally
comes from the font program's `/FontMatrix`. Some generators (WeasyPrint among
them) write CFF subsets whose matrix the font parser cannot read, and for those
the scale is taken from the file instead: every glyph gives the same
`advance / widths` ratio, so the median of it is the font's size in units, and a
glyph that disagrees still stands out. Such fonts are named in a `note:` line,
so a relative comparison is never mistaken for an absolute one.

## Tests

`tests/inspect.rs` runs the binary over `tests/data/weasyprint.pdf` — ten A4
pages of Type0 / Identity-H text from WeasyPrint, whose two-byte `/ToUnicode`
codes and unreadable CFF `/FontMatrix` both used to be decoded wrongly. The
fixture is not in git; the test skips when it is missing.

The standard fonts are taken from `$STANDARD_FONTS`; when that is unset,
`$PDF_STANDARD_FONTS` or one of the known font directories is used, because
`pdf_render` panics without it. A page using Helvetica without an embedded
font shows `<no font>` here if no such directory exists.
