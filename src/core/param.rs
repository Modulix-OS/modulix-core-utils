use rnix::{Root, ast};
use rowan::ast::AstNode as _;

use super::transaction::file_lock::NixFile;
use crate::mx;

// ── Position ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PatternInfo {
    pub open_brace: usize,
    pub close_brace: usize,
    pub is_inline: bool,
}

impl PatternInfo {
    #[inline]
    pub fn inner_start(&self) -> usize {
        self.open_brace + 1
    }
    #[inline]
    pub fn inner_end(&self) -> usize {
        self.close_brace
    }
}

#[derive(Debug)]
pub enum ParamPosition {
    ExistingParam(PatternInfo),
    NoPattern,
}

// ── Helpers internes ──────────────────────────────────────────────────────────

/// Extrait le `Pattern` depuis l'AST en passant par `Lambda::param()`.
/// C'est la seule approche fiable : `Pattern` n'apparaît dans l'AST que
/// comme variant de `Param`, lui-même accessible via `Lambda`.
fn find_pattern(content: &str) -> Option<ast::Pattern> {
    let root = Root::parse(content).tree();
    for node in root.syntax().descendants() {
        if let Some(lambda) = ast::Lambda::cast(node) {
            if let Some(ast::Param::Pattern(pattern)) = lambda.param() {
                return Some(pattern);
            }
        }
    }
    None
}

fn locate_pattern(content: &str) -> Option<PatternInfo> {
    let pattern = find_pattern(content)?;
    let range = pattern.syntax().text_range();
    let open_brace = usize::from(range.start());
    let close_brace = usize::from(range.end()) - 1;
    let is_inline = !content[open_brace..=close_brace].contains('\n');
    Some(PatternInfo {
        open_brace,
        close_brace,
        is_inline,
    })
}

fn parse_param_names(content: &str) -> Vec<String> {
    match find_pattern(content) {
        Some(pattern) => pattern
            .pat_entries()
            .filter_map(|e| e.ident())
            .map(|id| id.to_string())
            .collect(),
        None => Vec::new(),
    }
}

fn content_has_ellipsis(content: &str) -> bool {
    find_pattern(content)
        .map(|p| p.ellipsis_token().is_some())
        .unwrap_or(false)
}

// ── Struct principale ─────────────────────────────────────────────────────────

pub struct NixParam;

impl NixParam {
    pub fn new() -> Self {
        Self
    }

    #[allow(dead_code)]
    pub fn get_position(&self, nix_file: &NixFile) -> mx::Result<ParamPosition> {
        let content = nix_file.get_file_content()?;
        Ok(match locate_pattern(&content) {
            Some(info) => ParamPosition::ExistingParam(info),
            None => ParamPosition::NoPattern,
        })
    }

    #[allow(dead_code)]
    pub fn get_all(&self, nix_file: &NixFile) -> mx::Result<Vec<String>> {
        let content = nix_file.get_file_content()?;
        match locate_pattern(&content) {
            Some(_) => Ok(parse_param_names(&content)),
            None => Err(mx::ErrorKind::OptionNotFound),
        }
    }

    #[allow(dead_code)]
    pub fn contains(&self, nix_file: &NixFile, name: &str) -> mx::Result<bool> {
        let content = nix_file.get_file_content()?;
        Ok(match locate_pattern(&content) {
            None => false,
            Some(_) => parse_param_names(&content).iter().any(|n| n == name),
        })
    }

    #[allow(dead_code)]
    pub fn has_ellipsis(&self, nix_file: &NixFile) -> mx::Result<bool> {
        let content = nix_file.get_file_content()?;
        match locate_pattern(&content) {
            Some(_) => Ok(content_has_ellipsis(&content)),
            None => Err(mx::ErrorKind::OptionNotFound),
        }
    }

    // ── Mutations ─────────────────────────────────────────────────────────────

    pub fn add(&self, nix_file: &mut NixFile, name: &str) -> mx::Result<&Self> {
        let content = nix_file.get_mut_file_content()?;

        let info = match locate_pattern(content) {
            Some(i) => i,
            None => return Err(mx::ErrorKind::OptionNotFound),
        };

        if parse_param_names(&content).iter().any(|n| n == name) {
            return Ok(self);
        }

        if info.is_inline {
            let inner = &content[info.inner_start()..info.inner_end()];
            let insert_offset = inner
                .find("...")
                .map(|pos| info.inner_start() + pos)
                .unwrap_or(info.inner_end());
            content.insert_str(insert_offset, &format!("{}, ", name));
        } else {
            let inner = &content[info.inner_start()..=info.close_brace];
            let insert_offset = if let Some(rel) = inner.find("...") {
                let abs = info.inner_start() + rel;
                content[..abs].rfind('\n').map(|i| i + 1).unwrap_or(0)
            } else {
                info.close_brace
            };
            let line_start = content[..insert_offset]
                .rfind('\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            let indent: String = content[line_start..]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            content.insert_str(insert_offset, &format!(", {}\n{}", name, indent));
        }
        Ok(self)
    }

    #[allow(dead_code)]
    pub fn remove(&self, nix_file: &mut NixFile, name: &str) -> mx::Result<&Self> {
        let content = nix_file.get_mut_file_content()?;

        if !parse_param_names(&content).iter().any(|n| n == name) {
            return Ok(self);
        }
        if locate_pattern(&content).is_none() {
            return Err(mx::ErrorKind::OptionNotFound);
        }

        let root = Root::parse(&content).tree();
        for node in root.syntax().descendants() {
            if let Some(lambda) = ast::Lambda::cast(node) {
                let Some(ast::Param::Pattern(pattern)) = lambda.param() else {
                    continue;
                };
                for entry in pattern.pat_entries() {
                    let ident_text = entry.ident().map(|id| id.to_string()).unwrap_or_default();

                    if ident_text != name {
                        continue;
                    }

                    let entry_range = entry.syntax().text_range();
                    let mut start = usize::from(entry_range.start());
                    let mut end = usize::from(entry_range.end());

                    let after = content[end..].trim_start_matches([' ', '\t']);
                    if after.starts_with(',') {
                        end += content[end..].find(',').unwrap() + 1;
                        let skip = content[end..]
                            .chars()
                            .take_while(|c| *c == ' ' || *c == '\t')
                            .count();
                        end += skip;
                    } else {
                        if let Some(comma_pos) = content[..start].rfind(',') {
                            if content[comma_pos + 1..start]
                                .chars()
                                .all(|c| c.is_whitespace())
                            {
                                start = comma_pos;
                                if start > 0 && content.as_bytes()[start - 1] == b'\n' {
                                    start -= 1;
                                }
                            }
                        }
                    }

                    content.replace_range(start..end, "");
                    return Ok(self);
                }
            }
        }
        Ok(self)
    }

    // ── Comparaison ───────────────────────────────────────────────────────────

    #[allow(dead_code)]
    pub fn eq(&self, nix_file: &NixFile, expected: &[&str]) -> mx::Result<bool> {
        let content = nix_file.get_file_content()?;
        let current = parse_param_names(&content);
        use std::collections::HashSet;
        let a: HashSet<&str> = current.iter().map(String::as_str).collect();
        let b: HashSet<&str> = expected.iter().copied().collect();
        Ok(a == b)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_inline_pattern() {
        let content = "{ pkgs, lib, ... }:\npkgs.hello";
        let info = locate_pattern(content).expect("pattern attendu");
        assert_eq!(
            &content[info.open_brace..=info.close_brace],
            "{ pkgs, lib, ... }"
        );
        assert!(info.is_inline);
    }

    #[test]
    fn locate_multiline_pattern() {
        let content = "{ pkgs\n, lib\n, ...\n}:\npkgs.hello";
        let info = locate_pattern(content).expect("pattern attendu");
        assert!(!info.is_inline);
    }

    #[test]
    fn parse_names() {
        let content = "{ pkgs, lib, config, ... }:\n{}";
        let names = parse_param_names(content);
        assert_eq!(names, vec!["pkgs", "lib", "config"]);
    }

    #[test]
    fn has_ellipsis_true() {
        assert!(content_has_ellipsis("{ pkgs, ... }:\n{}"));
    }

    #[test]
    fn has_ellipsis_false() {
        assert!(!content_has_ellipsis("{ pkgs, lib }:\n{}"));
    }

    #[test]
    fn locate_with_leading_comments() {
        let content = "# Do not modify this file!\n{ config, lib, pkgs, ... }:\n{}";
        let info = locate_pattern(content).expect("pattern attendu");
        assert!(info.is_inline);
    }
}
