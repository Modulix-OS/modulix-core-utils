//! Shared fuzzy-search scoring used by every app-info source (packages, modules).
//!
//! `score` combines exact/substring/Levenshtein matches over the item name, its
//! description and its keywords so the different sources rank results the same way.

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 0..=m {
        dp[i][0] = i;
    }
    for j in 0..=n {
        dp[0][j] = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j].min(dp[i][j - 1]).min(dp[i - 1][j - 1])
            };
        }
    }
    dp[m][n]
}

/// Relevance score of an item for `query`, higher is better.
pub fn score(name: &str, description: &str, keywords: &[&str], query: &str) -> u32 {
    let query_lower = query.to_lowercase();
    let name_lower = name.to_lowercase();
    let desc_lower = description.to_lowercase();
    let mut score = 0u32;

    if name_lower == query_lower {
        score += 1000;
    }
    if let Some(pos) = name_lower.find(&query_lower) {
        score += match pos {
            0 => 500,
            1..=3 => 300,
            _ => 100,
        };
    }
    if let Some(pos) = desc_lower.find(&query_lower) {
        score += match pos {
            0 => 50,
            1..=10 => 30,
            _ => 10,
        };
    }
    let dist = levenshtein(name_lower.as_str(), query_lower.as_str());
    score += match dist {
        0 => 200,
        1 => 100,
        2 => 50,
        3 => 20,
        _ => 0,
    };

    for keyword in keywords {
        let keyword_lower = keyword.to_lowercase();
        if keyword_lower == query_lower {
            score += 400;
        } else if keyword_lower.contains(&query_lower) {
            score += 150;
        } else {
            let dist = levenshtein(&keyword_lower, &query_lower);
            score += match dist {
                1 => 80,
                2 => 30,
                _ => 0,
            };
        }
    }

    score
}
