//! Locates a dotted NixOS option path (e.g. `"mx.programs.git.enable"`)
//! inside a parsed Nix file's `rnix`/`rowan` syntax tree, without touching
//! the file itself.
//!
//! `SettingsPosition::new` walks `AttrSet`/`AttrpathValue` nodes, matching
//! both the flat spelling (`a.b.c = x;`) and the nested spelling
//! (`a = { b = { c = x; }; };`) of the same path, and returns byte `Range`s
//! (offsets into the original source text) that `super::option::Option` and
//! `super::list::List` splice into or read from directly — this module
//! never reads or writes a file, it only inspects an already-parsed tree.
//!
//! The result is either `SettingsPosition::ExistingOption` (the path is
//! already declared; carries the range of the whole declaration and of just
//! its value) or `SettingsPosition::NewInsertion` (the path is not
//! declared; carries the byte offset at which to splice it in, the sub-path
//! that still needs to be created, and the indentation level to write it
//! at).

use rnix::TextRange;
use rnix::ast::{AttrSet, AttrpathValue, Expr, HasEntry};
use rowan::ast::AstNode;
use std::ops::Range;

use crate::mx;

/// The attribute that marks a Modulix module as enabled.
const ENABLE_ATTR: &str = "enable";

/// Converts an `rnix`/`rowan` `TextRange` into a `std::ops::Range<usize>` of
/// byte offsets into the source text that was parsed.
///
/// # Parameters
/// * `r` - the range to convert, as reported by an AST node's
///   `text_range()`.
///
/// # Returns
/// The same bounds as `r`, as a `start..end` byte range (`end` exclusive).
fn text_range_to_range(r: TextRange) -> Range<usize> {
    r.start().into()..r.end().into()
}

/// Where and how to create an option path that is not declared yet.
///
/// Produced when the deepest attrset reachable along the dotted path exists
/// but does not (fully) contain it; `super::option::Option::set` uses it to
/// synthesize the missing `key = { … };` nesting and splice it in.
#[derive(Debug, Clone)]
pub struct NewInsertion {
    /// Byte offset into the source at which the missing part must be
    /// spliced in. Always the index of an attrset's closing `}` byte
    /// (`text_range().end() - 1`): either the attrset in which no entry at
    /// all matched the path, or — when a deeper existing prefix was found —
    /// the closing `}` of that deepest matching nested attrset, forwarded
    /// unchanged from the recursive call that found it.
    pos: usize,
    /// Dotted sub-path, relative to the attrset at `pos`, that still has to
    /// be created (e.g. `"b.c"` when the full path was `"a.b.c"` and only
    /// `a` already exists as an attrset). Never includes segments already
    /// matched by an existing attrpath.
    rest_option_path: String,
    /// Nesting depth, in attrset levels, at which the new key must be
    /// written: how many `TABULATION_SIZE`-wide indents precede its first
    /// line. Counted so that a key written directly inside the file's
    /// top-level `{ … }` is level `1`; each `key = { … };` wrapper still to
    /// be synthesized from `rest_option_path` goes one level deeper.
    indent_level: usize,
}

/// Byte ranges of an option path that is already declared.
///
/// Produced when an `AttrpathValue` node's attrpath matches the searched
/// path exactly, under either the flat or the nested spelling.
#[derive(Debug, Clone)]
pub struct ExistingOption {
    /// Byte range of the whole matched declaration: the attrpath, `=`, the
    /// value, and the trailing `;` (an `AttrpathValue` node's own
    /// `text_range()`, which `rnix`'s parser always closes on the `;`
    /// token). Splicing over this range removes the entire assignment,
    /// sub-attributes included.
    range_path: Range<usize>,
    /// Byte range of just the value expression assigned to the option: the
    /// attrset, list, or scalar expression on the right of `=`, excluding
    /// both the `attrpath = ` prefix and the trailing `;`. For a
    /// `with …; [ … ]` value this is the inner list's own range only, so
    /// the `with …;` clause is left untouched. Splicing over this range
    /// replaces or reads just the value.
    range_value: Range<usize>,
    /// Nesting depth, in attrset levels, of the attrset that directly
    /// contains this declaration: how many `TABULATION_SIZE`-wide indents
    /// precede the attrpath itself.
    indent_level: usize,
}

/// Outcome of locating a dotted option path inside a parsed Nix AST: either
/// the path is already declared, or it is not and this describes where to
/// create it. Returned by `SettingsPosition::new`; consumed by
/// `super::option::Option::set`/`get` and `super::list::List` to splice a
/// value in place or synthesize the missing nesting.
#[derive(Debug, Clone)]
pub enum SettingsPosition {
    /// The path is not declared yet.
    NewInsertion(NewInsertion),
    /// The path is already declared.
    ExistingOption(ExistingOption),
}

impl NewInsertion {
    /// Builds a `NewInsertion` from its already-computed parts.
    ///
    /// # Parameters
    /// * `pos` - byte offset to splice the new declaration at; see the
    ///   `pos` field.
    /// * `rest_option_path` - dotted sub-path still to be created; see the
    ///   `rest_option_path` field.
    /// * `indent_level` - nesting depth to write the new key at; see the
    ///   `indent_level` field.
    ///
    /// # Returns
    /// The new `NewInsertion`, holding these three values unchanged.
    pub fn new(pos: usize, rest_option_path: impl Into<String>, indent_level: usize) -> Self {
        NewInsertion {
            pos,
            rest_option_path: rest_option_path.into(),
            indent_level,
        }
    }

    /// # Returns
    /// The `pos` field: the byte offset at which the missing declaration
    /// must be spliced in.
    pub fn get_pos_new_insertion(&self) -> usize {
        self.pos
    }

    /// # Returns
    /// The `rest_option_path` field: the dotted sub-path still to be
    /// created, relative to the attrset at `pos`.
    pub fn get_remaining_path(&self) -> &str {
        &self.rest_option_path
    }

    /// # Returns
    /// The `indent_level` field: the nesting depth to write the new key at.
    pub fn get_indent_level(&self) -> usize {
        self.indent_level
    }
}

impl ExistingOption {
    /// Builds an `ExistingOption` from its already-computed parts.
    ///
    /// # Parameters
    /// * `range_path` - byte range of the whole declaration; see the
    ///   `range_path` field.
    /// * `range_value` - byte range of just the value expression; see the
    ///   `range_value` field.
    /// * `indent_level` - nesting depth of the declaration; see the
    ///   `indent_level` field.
    ///
    /// # Returns
    /// The new `ExistingOption`, holding these three values unchanged.
    pub fn new(range_path: Range<usize>, range_value: Range<usize>, indent_level: usize) -> Self {
        ExistingOption {
            range_path,
            range_value,
            indent_level,
        }
    }

    /// # Returns
    /// The `range_path` field: the byte range of the whole declaration
    /// (attrpath, `=`, value and trailing `;`).
    pub fn get_range_option(&self) -> &Range<usize> {
        &self.range_path
    }

    /// # Returns
    /// The `range_value` field: the byte range of just the value
    /// expression.
    pub fn get_range_option_value(&self) -> &Range<usize> {
        &self.range_value
    }

    /// # Returns
    /// The `indent_level` field: the nesting depth of the attrset directly
    /// containing this declaration.
    pub fn get_indent_level(&self) -> usize {
        self.indent_level
    }
}

impl SettingsPosition {
    /// Locates `settings` inside the tree rooted at `nix_ast`.
    ///
    /// # Parameters
    /// * `nix_ast` - root syntax node of a parsed Nix file (e.g. the result
    ///   of `rnix::Root::parse(..).syntax()`); all returned byte ranges and
    ///   offsets are relative to the source text this node was parsed from.
    /// * `settings` - dotted option path to locate (e.g.
    ///   `"mx.programs.git.enable"`), matched against both the flat
    ///   (`a.b.c = x;`) and the nested (`a = { b = { c = x; }; };`)
    ///   spellings found under `nix_ast`.
    ///
    /// # Returns
    /// `SettingsPosition::ExistingOption` when `settings` is already
    /// declared, `SettingsPosition::NewInsertion` when it is not.
    ///
    /// # Errors
    /// `mx::ErrorKind::InvalidFile` when no `AttrSet` nor `AttrpathValue`
    /// node is reachable anywhere under `nix_ast` (e.g. an empty or
    /// degenerate file).
    pub fn new(nix_ast: &rnix::SyntaxNode, settings: &str) -> mx::Result<Self> {
        Self::localise_option(nix_ast, settings, 0).ok_or(mx::ErrorKind::InvalidFile)
    }

    /// Descends from `node` down to the first `AttrSet` or `AttrpathValue`
    /// node reachable, depth-first and left-to-right among children, then
    /// hands off to the matching-specific helpers. Used to skip past
    /// wrapper nodes (e.g. a file's `{ config, lib, pkgs, ... }:` lambda
    /// header) to reach the actual top-level attribute set to search.
    ///
    /// # Parameters
    /// * `node` - syntax node currently being inspected; not required to be
    ///   an `AttrSet` or `AttrpathValue` itself.
    /// * `settings` - dotted path still to resolve; passed through
    ///   unchanged, since this function only locates where matching should
    ///   start and consumes no segment itself.
    /// * `indent_level` - nesting depth to hand to the eventual match:
    ///   incremented by one when `node` itself casts to an `AttrSet` (its
    ///   direct entries live one level deeper), left unchanged when `node`
    ///   casts to an `AttrpathValue` or when recursing into a plain wrapper
    ///   child.
    ///
    /// # Returns
    /// `None` if no `AttrSet` nor `AttrpathValue` is reachable under
    /// `node`; otherwise the result of the first matching branch found.
    fn localise_option(
        node: &rnix::SyntaxNode,
        settings: &str,
        indent_level: usize,
    ) -> Option<SettingsPosition> {
        if let Some(attr_set) = AttrSet::cast(node.clone()) {
            return Some(Self::localise_in_attr_set(
                &attr_set,
                settings,
                indent_level + 1,
            ));
        }

        if let Some(apv) = AttrpathValue::cast(node.clone()) {
            return Self::localise_in_attrpath_value(&apv, settings, indent_level);
        }

        for child in node.children() {
            if let Some(result) = Self::localise_option(&child, settings, indent_level) {
                return Some(result);
            }
        }

        None
    }

    /// Searches the immediate entries of `attr_set` for the one whose
    /// attrpath best matches `settings`, matching both the flat and the
    /// nested spelling by delegating each entry to
    /// `Self::localise_in_attrpath_value` (which may itself recurse one
    /// level for a nested attrset value).
    ///
    /// # Parameters
    /// * `attr_set` - attribute-set node whose entries are scanned.
    /// * `settings` - dotted path to find, interpreted relative to
    ///   `attr_set` (segments already consumed by outer nesting are not
    ///   part of this string).
    /// * `indent_level` - nesting depth of `attr_set`'s own entries; used
    ///   both to build the fallback `NewInsertion` below and forwarded into
    ///   recursive calls.
    ///
    /// # Returns
    /// `SettingsPosition::ExistingOption` for the first entry, in AST
    /// order, that is or contains an exact match (the scan stops there,
    /// even if a shorter path exists among later entries). Otherwise
    /// `SettingsPosition::NewInsertion`: the best candidate among entries
    /// that only partially matched — the one whose remaining path is
    /// shortest, i.e. the deepest already-existing prefix, ties keeping the
    /// first one seen — or, when no entry is even a prefix of `settings`, a
    /// fallback pointing at the byte just before `attr_set`'s closing `}`
    /// with the whole (unconsumed) `settings` string left to create and
    /// `indent_level` unchanged.
    fn localise_in_attr_set(
        attr_set: &AttrSet,
        settings: &str,
        indent_level: usize,
    ) -> SettingsPosition {
        let mut best: Option<NewInsertion> = None;

        for entry in attr_set.entries() {
            let rnix::ast::Entry::AttrpathValue(apv) = entry else {
                continue;
            };

            let Some(pos) = Self::localise_in_attrpath_value(&apv, settings, indent_level) else {
                continue;
            };

            match pos {
                SettingsPosition::ExistingOption(p) => return SettingsPosition::ExistingOption(p),
                SettingsPosition::NewInsertion(new_pos) => {
                    let is_better = best.as_ref().map_or(true, |b| {
                        new_pos.get_remaining_path().len() < b.get_remaining_path().len()
                    });
                    if is_better {
                        best = Some(new_pos);
                    }
                }
            }
        }

        match best {
            Some(b) => SettingsPosition::NewInsertion(b),
            None => {
                let end: usize = attr_set.syntax().text_range().end().into();
                SettingsPosition::NewInsertion(NewInsertion::new(end - 1, settings, indent_level))
            }
        }
    }

    /// Tests whether one entry `apv` (e.g. `a.b = { … };`) is on the path
    /// to `settings`, and if so, whether it already contains the target or
    /// only a prefix of it.
    ///
    /// # Parameters
    /// * `apv` - the `AttrpathValue` entry to test.
    /// * `settings` - dotted path being searched for, relative to the
    ///   attrset `apv` lives in.
    /// * `indent_level` - nesting depth of `apv` itself; passed through
    ///   unchanged when it directly holds the match, incremented by one
    ///   before recursing into a nested attrset value's own entries.
    ///
    /// # Returns
    /// `None` when `apv`'s attrpath is not a literal, component-wise prefix
    /// of `settings` (longer than `settings`, or a mismatching segment), or
    /// when its value is `with …; body` and `body` is not a list — in both
    /// cases this entry is not a candidate at all, not even a
    /// `NewInsertion`.
    ///
    /// Otherwise:
    /// * value is an `AttrSet` and the attrpath exactly equals `settings`
    ///   (nothing remains): `SettingsPosition::ExistingOption` over `apv`'s
    ///   full range and the set's own range — the attrset itself is the
    ///   option's value, it is not descended into.
    /// * value is an `AttrSet` and segments remain: recurses into that set
    ///   via `Self::localise_in_attr_set` with the remaining suffix and
    ///   `indent_level + 1`, returning whatever it finds.
    /// * value is a `List`, or `with …; […]` whose body is a `List`:
    ///   `SettingsPosition::ExistingOption` over `apv`'s full range and the
    ///   list's own range (excluding a `with …;` prefix, if present).
    ///   Returned as soon as the attrpath is a prefix of `settings`, even
    ///   when not an exact match — a list found under a shorter attrpath is
    ///   treated as already containing any longer requested path.
    /// * any other value expression (string, bool, int, reference, …):
    ///   `SettingsPosition::ExistingOption` over `apv`'s full range and
    ///   that expression's own range, with the same non-exact-match
    ///   behaviour as the `List` case.
    ///
    /// # Errors
    /// None; this function is infallible, absence of a match is expressed
    /// as `None`.
    fn localise_in_attrpath_value(
        apv: &AttrpathValue,
        settings: &str,
        indent_level: usize,
    ) -> Option<SettingsPosition> {
        let attrpath = apv.attrpath()?;

        let attr_segments: Vec<String> = attrpath.attrs().map(|a| a.to_string()).collect();

        let settings_segments: Vec<&str> = settings.split('.').collect();

        let is_prefix = attr_segments.len() <= settings_segments.len()
            && attr_segments
                .iter()
                .zip(settings_segments.iter())
                .all(|(a, s)| a == s);

        if !is_prefix {
            return None;
        }

        let value = apv.value()?;

        match value {
            Expr::AttrSet(set) => {
                let remaining = settings_segments[attr_segments.len()..].join(".");

                if remaining.is_empty() {
                    return Some(SettingsPosition::ExistingOption(ExistingOption::new(
                        text_range_to_range(apv.syntax().text_range()),
                        text_range_to_range(set.syntax().text_range()),
                        indent_level,
                    )));
                }

                Some(Self::localise_in_attr_set(
                    &set,
                    &remaining,
                    indent_level + 1,
                ))
            }

            Expr::List(list) => Some(SettingsPosition::ExistingOption(ExistingOption::new(
                text_range_to_range(apv.syntax().text_range()),
                text_range_to_range(list.syntax().text_range()),
                indent_level,
            ))),

            Expr::With(with_expr) => {
                let inner_list = with_expr.body()?;
                if let Expr::List(list) = inner_list {
                    Some(SettingsPosition::ExistingOption(ExistingOption::new(
                        text_range_to_range(apv.syntax().text_range()),
                        text_range_to_range(list.syntax().text_range()),
                        indent_level,
                    )))
                } else {
                    None
                }
            }

            other => Some(SettingsPosition::ExistingOption(ExistingOption::new(
                text_range_to_range(apv.syntax().text_range()),
                text_range_to_range(other.syntax().text_range()),
                indent_level,
            ))),
        }
    }
}

/// Immediate child attribute names nested under the dotted `prefix`. Both
/// the flat (`mx.foo.enable = true;`) and the nested
/// (`mx = { foo = { … }; bar.enable = … };`) spellings are merged, so
/// `prefix = "mx"` yields `["bar", "foo"]`. Reading the value under each
/// child is left to `super::option::Option::get`.
///
/// # Parameters
/// * `node` - syntax node to walk (its whole subtree, not just its direct
///   children).
/// * `prefix` - dotted attribute path whose immediate children are
///   collected.
///
/// # Returns
/// The first path segment found immediately below `prefix`, once per
/// distinct name, sorted and deduplicated; empty when `prefix` is not
/// declared anywhere under `node`.
pub(crate) fn collect_child_names(node: &rnix::SyntaxNode, prefix: &str) -> Vec<String> {
    let prefix_segments: Vec<&str> = prefix.split('.').collect();
    let mut names = Vec::new();
    collect_child_names_rec(node, &[], &prefix_segments, &mut names);
    names.sort();
    names.dedup();
    names
}

/// Recursive worker for `collect_child_names`. Walks `node`'s whole subtree
/// — not just its direct children — since an attrpath-value naming a child
/// of `prefix` can sit nested arbitrarily deep inside intermediate attrsets
/// (the nested spelling).
///
/// # Parameters
/// * `node` - syntax node whose children are scanned.
/// * `path` - dotted segments accumulated from the original root down to
///   `node` (each nested attrset's key is prepended on the way in).
/// * `prefix` - target dotted path being searched for, already split into
///   segments.
/// * `out` - accumulator receiving one entry per attrpath-value found whose
///   full path (`path` plus its own attrpath) extends `prefix` by at least
///   one segment; only the segment immediately after `prefix` is pushed.
///
/// # Returns
/// Nothing; results accumulate in `out` and may contain duplicates —
/// deduplication is left to the caller.
fn collect_child_names_rec(
    node: &rnix::SyntaxNode,
    path: &[String],
    prefix: &[&str],
    out: &mut Vec<String>,
) {
    for child in node.children() {
        let Some(apv) = AttrpathValue::cast(child.clone()) else {
            collect_child_names_rec(&child, path, prefix, out);
            continue;
        };
        let Some(attrpath) = apv.attrpath() else {
            continue;
        };
        let mut full: Vec<String> = path.to_vec();
        full.extend(attrpath.attrs().map(|a| a.to_string()));

        if full.len() > prefix.len() && prefix.iter().enumerate().all(|(i, p)| full[i] == *p) {
            out.push(full[prefix.len()].clone());
        }

        if let Some(Expr::AttrSet(set)) = apv.value() {
            collect_child_names_rec(set.syntax(), &full, prefix, out);
        }
    }
}

/// Dotted paths under `prefix` that declare an `enable` attribute, e.g.
/// `prefix = "mx"` on `mx.programs.studio.obs-studio.enable = true;` yields
/// `["programs.studio.obs-studio"]`. Module names are dotted paths of
/// arbitrary depth, so listing the immediate children of `mx` with
/// `collect_child_names` is not enough: it would stop at `programs`. Both
/// spellings — flat and nested (`mx = { programs.studio = { obs-studio =
/// { enable = true; }; }; }`) — are merged. Reading the value under each
/// `enable` is left to `super::option::Option::get`.
///
/// A path that extends another one is dropped: modules may declare
/// sub-options of their own named `enable`, and
/// `mx.programs.steam.enable = true;` next to
/// `mx.programs.steam.gamescope.enable = true;` describes one module, not
/// two.
///
/// # Parameters
/// * `node` - syntax node to walk (its whole subtree).
/// * `prefix` - dotted attribute path under which `enable` declarations are
///   collected.
///
/// # Returns
/// One dotted path per descendant of `prefix` that declares `enable`,
/// relative to `prefix` and without the trailing `enable` segment; sorted,
/// deduplicated, and with any path that extends another kept path dropped.
/// Empty when there is none. Whether each `enable` is set to `true` is not
/// checked here. Dropping the paths that extend another one is done in a
/// single pass over the sorted, deduplicated names: sorting always puts a
/// path immediately before the paths that extend it, so keeping only the
/// shortest of each such chain needs no backtracking.
pub(crate) fn collect_enable_paths(node: &rnix::SyntaxNode, prefix: &str) -> Vec<String> {
    let prefix_segments: Vec<&str> = prefix.split('.').collect();
    let mut names = Vec::new();
    collect_enable_paths_rec(node, &[], &prefix_segments, &mut names);
    names.sort();
    names.dedup();

    let mut modules: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let nested_in_previous = modules.last().is_some_and(|kept: &String| {
            name.len() > kept.len()
                && name.starts_with(kept.as_str())
                && name.as_bytes()[kept.len()] == b'.'
        });
        if !nested_in_previous {
            modules.push(name);
        }
    }
    modules
}

/// Recursive worker for `collect_enable_paths`. Walks `node`'s whole
/// subtree for the same reason as `collect_child_names_rec`: a matching
/// `enable` attrpath-value can sit nested arbitrarily deep under `prefix`.
///
/// # Parameters
/// * `node` - syntax node whose children are scanned.
/// * `path` - dotted segments accumulated from the original root down to
///   `node`.
/// * `prefix` - target dotted path being searched for, already split into
///   segments.
/// * `out` - accumulator receiving, for each attrpath-value whose full path
///   is `prefix` plus at least one more segment and ends in `enable`, the
///   segments strictly between `prefix` and the trailing `enable`, joined
///   with `.`.
///
/// # Returns
/// Nothing; results accumulate in `out` and may contain duplicates or
/// nested paths — both are resolved by the caller.
fn collect_enable_paths_rec(
    node: &rnix::SyntaxNode,
    path: &[String],
    prefix: &[&str],
    out: &mut Vec<String>,
) {
    for child in node.children() {
        let Some(apv) = AttrpathValue::cast(child.clone()) else {
            collect_enable_paths_rec(&child, path, prefix, out);
            continue;
        };
        let Some(attrpath) = apv.attrpath() else {
            continue;
        };
        let mut full: Vec<String> = path.to_vec();
        full.extend(attrpath.attrs().map(|a| a.to_string()));

        if full.len() > prefix.len() + 1
            && full[full.len() - 1] == ENABLE_ATTR
            && prefix.iter().enumerate().all(|(i, p)| full[i] == *p)
        {
            out.push(full[prefix.len()..full.len() - 1].join("."));
        }

        if let Some(Expr::AttrSet(set)) = apv.value() {
            collect_enable_paths_rec(set.syntax(), &full, prefix, out);
        }
    }
}

#[cfg(test)]
#[path = "localise_option_tests.rs"]
mod tests;
