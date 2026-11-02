//! Page-at-a-time PDF search. Results never retain document text.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub const MAX_PAGES: usize = 4096;
const MAX_MATCHES_PER_PAGE: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub smart_case: bool,
    pub regex: bool,
    pub whole_words: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub page: usize,
    pub count: usize,
    pub limited: bool,
}
#[derive(Debug)]
pub struct Report {
    pub query: Query,
    pub total: usize,
    pub hits: Vec<Hit>,
}

pub fn scan(
    total: usize,
    query: &Query,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicUsize,
    mut page_text: impl FnMut(usize) -> Result<Arc<str>, String>,
) -> Result<Report, String> {
    if query.text.is_empty() || query.text.chars().count() > 256 {
        return Err("Enter between 1 and 256 characters for PDF search".into());
    }
    if total == 0 || total > MAX_PAGES {
        return Err("Whole-document PDF search is limited to 4096 pages".into());
    }
    let pattern = crate::preview_find::compile_search_pattern(
        &query.text,
        crate::preview_find::search_case_sensitive(
            &query.text,
            query.case_sensitive,
            query.smart_case,
        ),
        query.regex,
        query.whole_words,
    )
    .map_err(|e| e.to_string())?;
    let mut hits = Vec::new();
    progress.store(0, Ordering::Relaxed);
    for page in 1..=total {
        if cancelled() {
            return Err("PDF document search cancelled".into());
        }
        let text = page_text(page).map_err(|error| format!("Page {page}: {error}"))?;
        if cancelled() {
            return Err("PDF document search cancelled".into());
        }
        if text.len() > crate::pdf_text::MAX_TEXT {
            return Err(format!(
                "Page {page}: PDF text exceeds the 256 KiB page limit"
            ));
        }
        if text.trim().is_empty() {
            progress.store(page, Ordering::Relaxed);
            continue;
        }
        let count = pattern
            .find_iter(&text)
            .take(MAX_MATCHES_PER_PAGE + 1)
            .count();
        if count > 0 {
            hits.push(Hit {
                page,
                count: count.min(MAX_MATCHES_PER_PAGE),
                limited: count > MAX_MATCHES_PER_PAGE,
            });
        }
        progress.store(page, Ordering::Relaxed);
    }
    if cancelled() {
        return Err("PDF document search cancelled".into());
    }
    Ok(Report {
        query: query.clone(),
        total,
        hits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query(text: &str, case_sensitive: bool) -> Query {
        Query {
            text: text.into(),
            case_sensitive,
            ..Default::default()
        }
    }

    #[test]
    fn literal_unicode_search_reports_only_matching_pages_and_uses_page_local_bounds() {
        let progress = AtomicUsize::new(0);
        let pages = ["one.[x]", "ёж ЁЖ ёж", "K K k", "split", "word"];
        let report = scan(
            pages.len(),
            &query("ёж", false),
            &|| false,
            &progress,
            |page| Ok(Arc::from(pages[page - 1])),
        )
        .unwrap();
        assert_eq!(
            report.hits,
            [Hit {
                page: 2,
                count: 3,
                limited: false
            }]
        );
        assert_eq!(progress.load(Ordering::Relaxed), 5);
        assert_eq!(
            scan(5, &query("ёж", true), &|| false, &progress, |p| Ok(
                Arc::from(pages[p - 1])
            ))
            .unwrap()
            .hits[0]
                .count,
            2
        );
        assert_eq!(
            scan(5, &query("k", false), &|| false, &progress, |p| Ok(
                Arc::from(pages[p - 1])
            ))
            .unwrap()
            .hits[0]
                .count,
            3
        );
        assert_eq!(
            scan(5, &query(".[x]", true), &|| false, &progress, |p| Ok(
                Arc::from(pages[p - 1])
            ))
            .unwrap()
            .hits[0]
                .page,
            1
        );
        assert!(
            scan(5, &query("splitword", false), &|| false, &progress, |p| Ok(
                Arc::from(pages[p - 1])
            ))
            .unwrap()
            .hits
            .is_empty()
        );
    }

    #[test]
    fn regex_words_and_page_highlights_share_identical_compilation() {
        let progress = AtomicUsize::new(0);
        let pages = ["xitem-12 ITEM-34 item-56z", "item-78", "ёж xёж ЁЖ"];
        let mut q = query(r"item-\d+", false);
        q.regex = true;
        q.whole_words = true;
        let report = scan(3, &q, &|| false, &progress, |p| Ok(Arc::from(pages[p - 1]))).unwrap();
        assert_eq!(
            report
                .hits
                .iter()
                .map(|h| (h.page, h.count))
                .collect::<Vec<_>>(),
            [(1, 1), (2, 1)]
        );
        let pattern = crate::preview_find::compile_search_pattern(
            &q.text,
            q.case_sensitive,
            q.regex,
            q.whole_words,
        )
        .unwrap();
        for hit in &report.hits {
            assert_eq!(pattern.find_iter(pages[hit.page - 1]).count(), hit.count);
        }
        q = query("ёж", false);
        q.whole_words = true;
        assert_eq!(
            scan(3, &q, &|| false, &progress, |p| Ok(Arc::from(pages[p - 1])))
                .unwrap()
                .hits[0]
                .count,
            2
        );
    }

    #[test]
    fn smart_case_preserves_unicode_manual_override_and_inline_regex_flags() {
        let progress = AtomicUsize::new(0);
        for (text, regex, manual, expected) in [
            ("foo", false, false, 3),
            ("Foo", false, false, 1),
            ("ёж", false, false, 3),
            ("Ёж", false, false, 1),
            ("foo", false, true, 1),
            (r"foo\b", true, false, 3),
            (r"Foo\b", true, false, 1),
            (r"(?i)Foo\b", true, true, 3),
            (r"foo\B", true, false, 1),
        ] {
            let q = Query {
                text: text.into(),
                regex,
                case_sensitive: manual,
                smart_case: true,
                ..Default::default()
            };
            let report = scan(1, &q, &|| false, &progress, |_| {
                Ok(Arc::from("foo Foo FOO ёж Ёж ЁЖ foobar"))
            })
            .unwrap();
            // Literal foo also matches foobar; word mode is opt-in.
            let expected = expected + usize::from(text == "foo");
            assert_eq!(report.hits[0].count, expected, "{text}, manual={manual}");
            assert!(report.query.smart_case);
            assert_eq!(report.query.case_sensitive, manual);
        }
        let mut q = query("Foo", false);
        q.smart_case = false;
        assert_eq!(
            scan(1, &q, &|| false, &progress, |_| Ok(Arc::from(
                "foo Foo FOO"
            )))
            .unwrap()
            .hits[0]
                .count,
            3
        );
    }

    #[test]
    fn invalid_regex_fails_before_page_reads_and_free_spacing_comments_remain_valid() {
        let progress = AtomicUsize::new(0);
        let mut q = query("[", false);
        q.regex = true;
        assert!(
            scan(2, &q, &|| false, &progress, |_| panic!(
                "invalid pattern must not read pages"
            ))
            .is_err()
        );
        q.text = "(?x)item # trailing comment".into();
        q.whole_words = true;
        assert_eq!(
            scan(1, &q, &|| false, &progress, |_| Ok(Arc::from(
                "item xitem itemz"
            )))
            .unwrap()
            .hits[0]
                .count,
            1
        );
        q.text = "^|$".into();
        q.whole_words = false;
        let report = scan(3, &q, &|| false, &progress, |p| {
            Ok(Arc::from(["ab", "", " \n "][p - 1]))
        })
        .unwrap();
        assert_eq!(
            report.hits,
            [Hit {
                page: 1,
                count: 2,
                limited: false
            }]
        );
    }

    #[test]
    fn cancellation_and_page_errors_never_return_partial_matches() {
        let progress = AtomicUsize::new(0);
        assert!(
            scan(
                3,
                &query("a", false),
                &|| progress.load(Ordering::Relaxed) >= 1,
                &progress,
                |_| Ok(Arc::from("a"))
            )
            .unwrap_err()
            .contains("cancelled")
        );
        assert_eq!(progress.load(Ordering::Relaxed), 1);
        assert!(
            scan(3, &query("a", false), &|| false, &progress, |page| {
                if page == 2 {
                    Err("unreadable".into())
                } else {
                    Ok(Arc::from("a"))
                }
            })
            .unwrap_err()
            .contains("Page 2: unreadable")
        );
    }

    #[test]
    fn query_page_and_text_bounds_are_explicit_and_high_counts_are_labelled() {
        let progress = AtomicUsize::new(0);
        for text in ["".to_string(), "я".repeat(257)] {
            assert!(
                scan(1, &query(&text, false), &|| false, &progress, |_| panic!(
                    "invalid query must not read pages"
                ))
                .is_err()
            );
        }
        assert!(
            scan(
                MAX_PAGES + 1,
                &query("a", false),
                &|| false,
                &progress,
                |_| panic!("invalid count must not read pages")
            )
            .is_err()
        );
        assert!(
            scan(1, &query("a", false), &|| false, &progress, |_| Ok(
                Arc::from("a".repeat(crate::pdf_text::MAX_TEXT + 1))
            ))
            .is_err()
        );
        let report = scan(1, &query("a", false), &|| false, &progress, |_| {
            Ok(Arc::from("a".repeat(5000)))
        })
        .unwrap();
        assert_eq!(
            report.hits,
            [Hit {
                page: 1,
                count: 4096,
                limited: true
            }]
        );
    }
}
