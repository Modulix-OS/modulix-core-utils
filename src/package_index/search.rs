use crate::core::app_info_trait::score;

use super::reader::{Index, RowView};

/// `score()` awards Levenshtein points on the name up to this edit distance
/// only — a candidate whose attribute length differs from the query by more
/// than this can never score above 0 through that path, so the prefilter can
/// skip it without ever running the full (allocating) `score()`.
const MAX_NAME_LEVENSHTEIN: usize = 3;

/// Same reasoning for the keyword path, whose Levenshtein bonus stops at an
/// edit distance of 2.
const MAX_KEYWORD_LEVENSHTEIN: usize = 2;

#[cfg(feature = "app-info-gui")]
fn keywords_for(attr: &str) -> &'static [&'static str] {
    crate::package_info::package_basic_info::get_keywords(attr).unwrap_or_default()
}

#[cfg(not(feature = "app-info-gui"))]
fn keywords_for(_attr: &str) -> &'static [&'static str] {
    &[]
}

/// Whether any keyword of `attr` could earn points in `score()`.
///
/// Only runs for the rows the cheap tests rejected, and only allocates for the
/// handful of attributes the keyword table actually knows — everything else
/// gets an empty slice back.
fn keyword_can_score(query_lc: &str, query_len: usize, attr: &str) -> bool {
    keywords_for(attr).iter().any(|keyword| {
        let keyword_lc = keyword.to_lowercase();
        keyword_lc.contains(query_lc)
            || keyword_lc.chars().count().abs_diff(query_len) <= MAX_KEYWORD_LEVENSHTEIN
    })
}

fn prefilter(query_lc: &str, query_len: usize, row: &RowView<'_>) -> bool {
    if row.attr_lc.contains(query_lc) || row.desc_lc.contains(query_lc) {
        return true;
    }
    if row.attr_lc.chars().count().abs_diff(query_len) <= MAX_NAME_LEVENSHTEIN {
        return true;
    }
    // A keyword hit alone is worth up to 400 points, independently of the name
    // and the description: `photoshop` has to reach `gimp`, whose attribute and
    // description say nothing about it.
    keyword_can_score(query_lc, query_len, row.attr)
}

/// Scans the whole index for `query`, scoring only the candidates the cheap
/// substring/length/keyword prefilter lets through (exact w.r.t. `score()`:
/// nothing it skips could have scored above 0 anyway). Returns `(score, row_index)`
/// pairs, highest first, truncated to `limit` — `score == 0` is filtered out
/// here since the caller no longer has `nix search`'s own regex prefilter to
/// rely on.
pub(crate) fn search<'a>(index: &'a Index, query: &str, limit: usize) -> Vec<(u32, RowView<'a>)> {
    let query_lc = query.to_lowercase();
    let query_len = query_lc.chars().count();

    let mut hits: Vec<(u32, RowView<'a>)> = (0..index.len())
        .filter_map(|i| {
            let row = index.row(i)?;
            if !prefilter(&query_lc, query_len, &row) {
                return None;
            }
            let keywords = keywords_for(row.attr);
            let s = score(row.attr, row.description, keywords, query);
            (s > 0).then_some((s, row))
        })
        .collect();

    hits.sort_unstable_by_key(|(score, _)| std::cmp::Reverse(*score));
    hits.truncate(limit);
    hits
}
