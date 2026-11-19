//! Parsing of page selections such as `2-5`, `1,3,7-9`, `-3`, `4-` or `all`.
//!
//! Page numbers in a selection are 1-based, the way a PDF viewer shows them.
//! [`expand`] turns a selection into 0-based page indices for a document with a
//! given number of pages, rejecting anything out of bounds.

/// One element of a page selection, with 1-based page numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSel {
    /// A single page (`3`).
    Single(u32),
    /// An inclusive range (`2-5`); `None` means "from the first page" /
    /// "to the last page" (`-5`, `2-`).
    Range(Option<u32>, Option<u32>),
}

/// Parses a page selection into its elements without looking at a document.
///
/// Accepts `all` / `*`, single pages, inclusive ranges, and comma separated
/// lists of those. Whitespace around the separator and the numbers is ignored.
pub fn parse_spec(spec: &str) -> Result<Vec<PageSel>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty page selection".into());
    }
    if spec == "all" || spec == "*" {
        return Ok(vec![PageSel::Range(None, None)]);
    }

    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("empty page in selection {spec:?}"));
        }

        match part.split_once('-') {
            None => out.push(PageSel::Single(parse_page_number(part)?)),
            Some((a, b)) => {
                let a = a.trim();
                let b = b.trim();
                if a.is_empty() && b.is_empty() {
                    return Err(format!("page range {part:?} has no bounds"));
                }
                let a = if a.is_empty() { None } else { Some(parse_page_number(a)?) };
                let b = if b.is_empty() { None } else { Some(parse_page_number(b)?) };
                if let (Some(a), Some(b)) = (a, b) {
                    if a > b {
                        return Err(format!("page range {part:?} is backwards"));
                    }
                }
                out.push(PageSel::Range(a, b));
            }
        }
    }

    Ok(out)
}

fn parse_page_number(s: &str) -> Result<u32, String> {
    let n: u32 = s
        .parse()
        .map_err(|_| format!("{s:?} is not a page number"))?;
    if n == 0 {
        return Err("page numbers start at 1".into());
    }
    Ok(n)
}

/// Expands a page selection to 0-based page indices of a document with
/// `num_pages` pages, in the order the selection lists them.
pub fn expand(spec: &str, num_pages: u32) -> Result<Vec<u32>, String> {
    if num_pages == 0 {
        return Err("the document has no pages".into());
    }
    let mut out = Vec::new();
    for sel in parse_spec(spec)? {
        let (from, to) = match sel {
            PageSel::Single(n) => (n, n),
            PageSel::Range(a, b) => (a.unwrap_or(1), b.unwrap_or(num_pages)),
        };
        if to > num_pages {
            return Err(format!(
                "page {to} is out of range, the document has {num_pages} page(s)"
            ));
        }
        for n in from..=to {
            out.push(n - 1);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_pages_and_ranges() {
        assert_eq!(parse_spec("3").unwrap(), vec![PageSel::Single(3)]);
        assert_eq!(parse_spec(" 2 - 5 ").unwrap(), vec![PageSel::Range(Some(2), Some(5))]);
        assert_eq!(
            parse_spec("1,3-4,7").unwrap(),
            vec![
                PageSel::Single(1),
                PageSel::Range(Some(3), Some(4)),
                PageSel::Single(7),
            ]
        );
        assert_eq!(parse_spec("all").unwrap(), vec![PageSel::Range(None, None)]);
        assert_eq!(parse_spec("*").unwrap(), vec![PageSel::Range(None, None)]);
    }

    #[test]
    fn rejects_nonsense() {
        for bad in ["", "0", "x", "5-3", "-", "1,,2", "1-2-3"] {
            assert!(parse_spec(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn expands_to_zero_based_indices() {
        assert_eq!(expand("1", 5).unwrap(), vec![0]);
        assert_eq!(expand("2-4", 5).unwrap(), vec![1, 2, 3]);
        assert_eq!(expand("4-", 5).unwrap(), vec![3, 4]);
        assert_eq!(expand("-2", 5).unwrap(), vec![0, 1]);
        assert_eq!(expand("all", 3).unwrap(), vec![0, 1, 2]);
        assert_eq!(expand("3,1", 5).unwrap(), vec![2, 0]);
    }

    #[test]
    fn rejects_out_of_range() {
        assert!(expand("6", 5).is_err());
        assert!(expand("4-6", 5).is_err());
        assert!(expand("all", 0).is_err());
    }
}
