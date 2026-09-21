//! Editing the parameter set of a NixOS module, i.e. the `{ config, lib, pkgs,
//! ... }:` pattern a configuration file opens with.
//!
//! Adding an import that needs a new argument (`nixos-hardware`, for instance)
//! means adding it to that pattern first, which is what this module is for.

use rnix::{Root, ast};
use rowan::ast::AstNode as _;

use super::transaction::file_lock::NixFile;
use crate::mx;

/// Where a module's parameter pattern sits in the file.
///
/// # Fields
/// * `open_brace` - byte offset of the pattern's `{`.
/// * `close_brace` - byte offset of its `}`.
/// * `is_inline` - true when the whole pattern is written on one line, which
///   decides how a new parameter is laid out.
#[derive(Debug, Clone)]
pub struct PatternInfo {
    pub open_brace: usize,
    pub close_brace: usize,
    pub is_inline: bool,
}

impl PatternInfo {
    /// First byte inside the braces.
    ///
    /// # Returns
    /// The offset just after `{`.
    #[inline]
    pub fn inner_start(&self) -> usize {
        self.open_brace + 1
    }

    /// End of the region inside the braces.
    ///
    /// # Returns
    /// The offset of `}`, i.e. an exclusive upper bound on the contents.
    #[inline]
    pub fn inner_end(&self) -> usize {
        self.close_brace
    }
}

/// Outcome of looking for a parameter pattern in a file.
///
/// # Variants
/// * `ExistingParam` - a pattern was found, with its position.
/// * `NoPattern` - the file has no parameter pattern; nothing can be added to
///   it without writing one first.
#[derive(Debug)]
pub enum ParamPosition {
    ExistingParam(PatternInfo),
    NoPattern,
}

/// Finds the file's parameter pattern in the AST.
///
/// # Parameters
/// * `content` - the whole file content.
///
/// # Returns
/// The pattern of the first lambda that takes one - which for a NixOS module is
/// the module's own - or `None` when there is no such lambda. An unparseable
/// file also yields `None`, since `rnix` recovers rather than failing.
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

/// Measures the file's parameter pattern.
///
/// # Parameters
/// * `content` - the whole file content.
///
/// # Returns
/// The pattern's brace offsets and whether it is written inline, or `None` when
/// the file has no pattern.
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

/// Lists the parameters the file's pattern declares.
///
/// # Parameters
/// * `content` - the whole file content.
///
/// # Returns
/// The parameter names in declaration order; empty when the file has no
/// pattern. The `...` ellipsis is not a name and does not appear.
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

/// Tells whether the file's pattern ends with `...`.
///
/// # Parameters
/// * `content` - the whole file content.
///
/// # Returns
/// `true` when the pattern accepts extra arguments; `false` when it does not,
/// and also when there is no pattern at all.
fn content_has_ellipsis(content: &str) -> bool {
    find_pattern(content)
        .map(|p| p.ellipsis_token().is_some())
        .unwrap_or(false)
}

/// Entry point for reading and editing a module's parameter pattern.
///
/// Stateless: each call re-parses the file it is handed, so one value works
/// against any number of files.
pub struct NixParam;

impl NixParam {
    /// Builds the handle.
    ///
    /// # Returns
    /// A stateless value; no file is read here.
    pub fn new() -> Self {
        Self
    }

    /// Locates the parameter pattern of a file.
    ///
    /// # Parameters
    /// * `nix_file` - the file to inspect.
    ///
    /// # Returns
    /// [`ParamPosition::ExistingParam`] with the pattern's position, or
    /// [`ParamPosition::NoPattern`] when the file has none.
    #[allow(dead_code)]
    pub fn get_position(&self, nix_file: &NixFile) -> mx::Result<ParamPosition> {
        let content = nix_file.get_file_content()?;
        Ok(match locate_pattern(&content) {
            Some(info) => ParamPosition::ExistingParam(info),
            None => ParamPosition::NoPattern,
        })
    }

    /// Lists the parameters a file's module takes.
    ///
    /// # Parameters
    /// * `nix_file` - the file to inspect.
    ///
    /// # Returns
    /// The parameter names in declaration order.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionNotFound`] when the file has no parameter
    /// pattern.
    #[allow(dead_code)]
    pub fn get_all(&self, nix_file: &NixFile) -> mx::Result<Vec<String>> {
        let content = nix_file.get_file_content()?;
        match locate_pattern(&content) {
            Some(_) => Ok(parse_param_names(&content)),
            None => Err(mx::ErrorKind::OptionNotFound),
        }
    }

    /// Tells whether a module already takes a given parameter.
    ///
    /// # Parameters
    /// * `nix_file` - the file to inspect.
    /// * `name` - parameter name to look for.
    ///
    /// # Returns
    /// `true` when the pattern declares `name`; `false` when it does not, and
    /// also when the file has no pattern. An `...` ellipsis does not count as
    /// declaring anything.
    #[allow(dead_code)]
    pub fn contains(&self, nix_file: &NixFile, name: &str) -> mx::Result<bool> {
        let content = nix_file.get_file_content()?;
        Ok(match locate_pattern(&content) {
            None => false,
            Some(_) => parse_param_names(&content).iter().any(|n| n == name),
        })
    }

    /// Tells whether a module's pattern ends with `...`.
    ///
    /// # Parameters
    /// * `nix_file` - the file to inspect.
    ///
    /// # Returns
    /// `true` when the module tolerates extra arguments.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionNotFound`] when the file has no parameter
    /// pattern.
    #[allow(dead_code)]
    pub fn has_ellipsis(&self, nix_file: &NixFile) -> mx::Result<bool> {
        let content = nix_file.get_file_content()?;
        match locate_pattern(&content) {
            Some(_) => Ok(content_has_ellipsis(&content)),
            None => Err(mx::ErrorKind::OptionNotFound),
        }
    }

    /// Adds a parameter to a module's pattern.
    ///
    /// # Parameters
    /// * `nix_file` - the open file to edit, in memory only.
    /// * `name` - parameter to declare; it must be a valid Nix identifier, as
    ///   it is inserted verbatim.
    ///
    /// # Post-conditions
    /// The parameter is inserted before the `...` when there is one, so the
    /// ellipsis stays last, otherwise just before the closing brace. A
    /// multi-line pattern keeps its per-line layout and indentation. A
    /// parameter that is already declared leaves the file untouched.
    ///
    /// # Returns
    /// `self`, so several additions can be chained.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionNotFound`] when the file has no parameter
    /// pattern to add to.
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

    /// Removes a parameter from a module's pattern.
    ///
    /// # Parameters
    /// * `nix_file` - the open file to edit.
    /// * `name` - parameter to undeclare.
    ///
    /// # Post-conditions
    /// The entry goes with the comma that separated it, so the pattern stays
    /// syntactically valid either way round. A parameter that is not declared
    /// leaves the file untouched. Uses of the parameter in the module body are
    /// not cleaned up and will make the next evaluation fail.
    ///
    /// # Returns
    /// `self`, so several removals can be chained.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionNotFound`] when the file has no parameter
    /// pattern.
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

    /// Compares a module's parameters with an expected set.
    ///
    /// # Parameters
    /// * `nix_file` - the file to inspect.
    /// * `expected` - the parameter names expected.
    ///
    /// # Returns
    /// `true` when both sides declare the same names, order ignored. A file
    /// without a pattern compares equal to an empty `expected`, and the `...`
    /// ellipsis is not part of the comparison.
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
