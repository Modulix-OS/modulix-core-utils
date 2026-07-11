use rnix::TextRange;
use rnix::ast::{AttrSet, AttrpathValue, Expr, HasEntry};
use rowan::ast::AstNode;
use std::ops::Range;

use crate::mx;

fn text_range_to_range(r: TextRange) -> Range<usize> {
    r.start().into()..r.end().into()
}

#[derive(Debug, Clone)]
pub struct NewInsertion {
    pos: usize,
    rest_option_path: String,
    indent_level: usize,
}

#[derive(Debug, Clone)]
pub struct ExistingOption {
    range_path: Range<usize>,
    range_value: Range<usize>,
    indent_level: usize,
}

#[derive(Debug, Clone)]
pub enum SettingsPosition {
    NewInsertion(NewInsertion),
    ExistingOption(ExistingOption),
}

impl NewInsertion {
    pub fn new(pos: usize, rest_option_path: impl Into<String>, indent_level: usize) -> Self {
        NewInsertion {
            pos,
            rest_option_path: rest_option_path.into(),
            indent_level,
        }
    }

    pub fn get_pos_new_insertion(&self) -> usize {
        self.pos
    }

    pub fn get_remaining_path(&self) -> &str {
        &self.rest_option_path
    }

    pub fn get_indent_level(&self) -> usize {
        self.indent_level
    }
}

impl ExistingOption {
    pub fn new(range_path: Range<usize>, range_value: Range<usize>, indent_level: usize) -> Self {
        ExistingOption {
            range_path,
            range_value,
            indent_level,
        }
    }

    pub fn get_range_option(&self) -> &Range<usize> {
        &self.range_path
    }

    pub fn get_range_option_value(&self) -> &Range<usize> {
        &self.range_value
    }

    pub fn get_indent_level(&self) -> usize {
        self.indent_level
    }
}

impl SettingsPosition {
    pub fn new(nix_ast: &rnix::SyntaxNode, settings: &str) -> mx::Result<Self> {
        Self::localise_option(nix_ast, settings, 0).ok_or(mx::ErrorKind::InvalidFile)
    }

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

/// Immediate child attribute names nested under the dotted `prefix`. Both the flat
/// (`mx.foo.enable = true;`) and the nested (`mx = { foo = { … }; bar.enable = … };`)
/// spellings are merged, so `prefix = "mx"` yields `["bar", "foo"]`. The result is
/// sorted and deduplicated. Reading the value under each child is left to
/// [`super::option::Option::get`].
pub(crate) fn collect_child_names(node: &rnix::SyntaxNode, prefix: &str) -> Vec<String> {
    let prefix_segments: Vec<&str> = prefix.split('.').collect();
    let mut names = Vec::new();
    collect_child_names_rec(node, &[], &prefix_segments, &mut names);
    names.sort();
    names.dedup();
    names
}

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

        // Record the segment right after `prefix` when this path dives past it.
        if full.len() > prefix.len() && prefix.iter().enumerate().all(|(i, p)| full[i] == *p) {
            out.push(full[prefix.len()].clone());
        }

        // A nested attribute set is a container: recurse to reach its children.
        if let Some(Expr::AttrSet(set)) = apv.value() {
            collect_child_names_rec(set.syntax(), &full, prefix, out);
        }
    }
}
