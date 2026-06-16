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

#[allow(dead_code)]
mod v1 {
    use rnix::{self, TextRange, TextSize};
    use std::ops::Range;

    use crate::mx;

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
        pub fn new(
            range_path: Range<usize>,
            range_value: Range<usize>,
            indent_level: usize,
        ) -> Self {
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
            Self::localise_option(&nix_ast, &settings, 0usize).ok_or(mx::ErrorKind::InvalidFile)
        }

        /// Recursively locates an option in the Nix AST.
        ///
        /// This private function is the entry point of the search algorithm.
        /// It dispatches to the specialized functions depending on the node type:
        ///
        /// - `NODE_ATTR_SET`: Attribute set (`{ ... }`)
        /// - `NODE_ATTRPATH_VALUE`: Assignment (`key = value;`)
        /// - Other: Recursive traversal of the children
        ///
        /// # Arguments
        ///
        /// * `ast` - Syntax tree node to analyze
        /// * `settings` - Path of the searched option
        ///
        /// # Algorithm
        ///
        /// 1. Identify the node type
        /// 2. Delegate to the appropriate handler
        /// 3. For other nodes, traverse the children recursively
        /// 4. Return the first match found
        fn localise_option(
            ast: &rnix::SyntaxNode,
            settings: &str,
            indent_level: usize,
        ) -> Option<SettingsPosition> {
            return match ast.kind() {
                rnix::SyntaxKind::NODE_ATTR_SET => Some(Self::localise_option_node_attr_set(
                    &ast,
                    &settings,
                    indent_level + 1usize,
                )),
                rnix::SyntaxKind::NODE_ATTRPATH_VALUE => {
                    Self::localise_option_node_attrpath_value(&ast, &settings, indent_level)
                }
                _ => {
                    for c in ast.children() {
                        if let Some(ret) = Self::localise_option(&c, settings, indent_level) {
                            return Some(ret);
                        }
                    }
                    None
                }
            };
        }

        /// Handles an attribute-set node (`NODE_ATTR_SET`).
        ///
        /// This function looks for the best match among all the children of the
        /// attribute set. It implements a search strategy that:
        ///
        /// 1. Traverses all children looking for matches
        /// 2. Keeps the match with the longest path (most specific match)
        /// 3. Returns immediately if an exact match is found (`option_path = None`)
        /// 4. Otherwise, returns the best partial match or an insertion point
        ///
        /// # Arguments
        ///
        /// * `ast` - `NODE_ATTR_SET` node to analyze
        /// * `setting` - Path of the searched option
        ///
        /// # Returns
        ///
        /// Always returns a `SettingsPosition` with three possible cases:
        ///
        /// 1. **Full match** (`option_path = None`): The option exists exactly
        /// 2. **Partial match** (`option_path = Some(...)`): Part of the path exists
        /// 3. **No match**: Returns an insertion position before the closing `}`
        ///
        /// # Examples
        ///
        /// ```text
        /// // Case 1: Full match
        /// // File: { services.nginx.enable = true; }
        /// // Search: "services.nginx.enable"
        /// // Result: option_path = None
        ///
        /// // Case 2: Partial match
        /// // File: { services.nginx = {}; }
        /// // Search: "services.nginx.enable"
        /// // Result: option_path = Some("enable")
        ///
        /// // Case 3: No match
        /// // File: { services = {}; }
        /// // Search: "network.proxy"
        /// // Result: Insertion point before the '}'
        /// ```
        fn localise_option_node_attr_set(
            ast: &rnix::SyntaxNode,
            settings: &str,
            indent_level: usize,
        ) -> SettingsPosition {
            let mut best_opt_pos: Option<NewInsertion> = None;

            // Traverse all children to find matches
            for c in ast.children() {
                let opt_pos = Self::localise_option(&c, &settings, indent_level);
                if let Some(pos) = opt_pos {
                    // If an exact match is found, return immediately
                    match pos {
                        Self::ExistingOption(p) => return Self::ExistingOption(p),
                        Self::NewInsertion(new_pos) => match &best_opt_pos {
                            None => best_opt_pos = Some(new_pos),
                            Some(best_pos) => {
                                if new_pos.get_remaining_path().len()
                                    < best_pos.get_remaining_path().len()
                                {
                                    best_opt_pos = Some(new_pos);
                                }
                            }
                        },
                    }

                    // Otherwise, keep the best match (option in the closest definition)
                }
            }

            // Return the best match or an insertion point
            match best_opt_pos {
                Some(best_pos) => SettingsPosition::NewInsertion(best_pos),
                None => SettingsPosition::NewInsertion(NewInsertion::new(
                    <TextSize as Into<usize>>::into(ast.text_range().end()) - 1,
                    settings,
                    indent_level,
                )),
            }
        }

        /// Handles an assignment node (`NODE_ATTRPATH_VALUE`).
        ///
        /// This function analyzes assignment nodes (e.g. `services.nginx.enable = true;`)
        /// by checking whether the attribute path matches the searched setting.
        ///
        /// # Arguments
        ///
        /// * `ast` - `NODE_ATTRPATH_VALUE` node to analyze
        /// * `settings` - Full path of the searched option
        ///
        /// # Algorithm
        ///
        /// 1. **Path extraction**: Get the attribute path of the node
        /// 2. **Prefix check**: Compare segment by segment with the setting
        ///    - Count the segments of each path (separated by '.')
        ///    - Check that the attr_path is a prefix of the setting
        ///    - Compare each segment individually
        /// 3. **Value analysis**:
        ///    - If `NODE_ATTR_SET`: Recursive search in the sub-set
        ///    - If a simple value: Return the position (exact match)
        ///
        /// # Returns
        ///
        /// - `Some(SettingsPosition)`: If the attribute path is a prefix of the setting
        /// - `None`: If no prefix match is found
        ///
        /// # Prefix matching
        ///
        /// An attr_path is considered a valid prefix if:
        /// - It has the same number of segments or fewer than the setting
        /// - All its segments match the corresponding segments of the setting
        ///
        /// Examples of valid prefixes:
        ///
        /// ```text
        /// Attr path: services.nginx
        /// Settings:  services.nginx.enable
        /// ✓ Valid prefix (2 ≤ 3 segments, all identical)
        ///
        /// Attr path: services.nginx.enable
        /// Settings:  services.nginx.enable
        /// ✓ Exact match (3 = 3 segments)
        ///
        /// Attr path: services.apache
        /// Settings:  services.nginx.enable
        /// ✗ Not a prefix (apache ≠ nginx)
        /// ```
        ///
        /// # Supported value types
        ///
        /// - `NODE_ATTR_SET`: Nested set (`{ ... }`)
        /// - `NODE_IDENT`: Identifier (`true`, `false`, variable)
        /// - `NODE_LITERAL`: Literal value (number, boolean)
        /// - `NODE_STRING`: String
        /// - `NODE_PATH_REL`: Relative path (`./path`)
        /// - `NODE_PATH_ABS`: Absolute path (`/path`)
        /// - `NODE_PATH_HOME`: Home path (`~/path`)
        /// - `NODE_PATH_SEARCH`: Search path (`<nixpkgs>`)
        fn localise_option_node_attrpath_value(
            ast: &rnix::SyntaxNode,
            settings: &str,
            indent_level: usize,
        ) -> Option<SettingsPosition> {
            let mut attr_path_valid: Option<String> = None;

            // Step 1: Find the matching attribute path
            for c in ast
                .children()
                .filter(|c| c.kind() == rnix::SyntaxKind::NODE_ATTRPATH)
            {
                let attr_path = c.text().to_string();

                let count_split_settings = settings.split('.').count();
                let count_split_attr_path = attr_path.split('.').count();

                // Check whether attr_path is a prefix of settings
                let is_prefix = count_split_attr_path <= count_split_settings
                    && attr_path
                        .split('.')
                        .zip(settings.split('.'))
                        .all(|(a, s)| a == s);

                if is_prefix {
                    attr_path_valid = Some(attr_path);
                    break;
                }
            }

            // If no valid prefix was found, return None
            if let None = attr_path_valid {
                return None;
            }

            // Step 2: Analyze the associated value
            let children_value = ast.children().filter(|cv| match cv.kind() {
                rnix::SyntaxKind::NODE_ATTR_SET
                | rnix::SyntaxKind::NODE_LIST
                | rnix::SyntaxKind::NODE_WITH
                | rnix::SyntaxKind::NODE_IDENT
                | rnix::SyntaxKind::NODE_PATH_REL
                | rnix::SyntaxKind::NODE_PATH_ABS
                | rnix::SyntaxKind::NODE_PATH_HOME
                | rnix::SyntaxKind::NODE_PATH_SEARCH
                | rnix::SyntaxKind::NODE_STRING
                | rnix::SyntaxKind::NODE_LITERAL => true,
                _ => false,
            });

            for c in children_value {
                if c.kind() == rnix::SyntaxKind::NODE_ATTR_SET {
                    // Case 1: The value is a nested set
                    // Strip the already-processed prefix and continue the search
                    let setting_whitout_path =
                        settings.strip_prefix(&attr_path_valid.unwrap()).unwrap();
                    let new_settings = setting_whitout_path
                        .strip_prefix('.')
                        .or_else(|| Some(""))?;
                    if new_settings == "" {
                        return Some(SettingsPosition::ExistingOption(ExistingOption::new(
                            <TextRange as Into<Range<usize>>>::into(ast.text_range()),
                            <TextRange as Into<Range<usize>>>::into(c.text_range()),
                            indent_level,
                        )));
                    }

                    // Recursive search in the sub-set
                    return Some(Self::localise_option_node_attr_set(
                        &c,
                        new_settings,
                        indent_level + 1usize,
                    ));
                } else if c.kind() == rnix::SyntaxKind::NODE_WITH {
                    for children_with in c.children() {
                        match children_with.kind() {
                            rnix::SyntaxKind::NODE_LIST => {
                                return Some(SettingsPosition::ExistingOption(
                                    ExistingOption::new(
                                        <TextRange as Into<Range<usize>>>::into(ast.text_range()),
                                        <TextRange as Into<Range<usize>>>::into(
                                            children_with.text_range(),
                                        ),
                                        indent_level,
                                    ),
                                ));
                            }
                            _ => (),
                        }
                    }
                    return None;
                } else {
                    // Case 2: Place it as best we can at the end of the set
                    return Some(SettingsPosition::ExistingOption(ExistingOption::new(
                        <TextRange as Into<Range<usize>>>::into(ast.text_range()),
                        <TextRange as Into<Range<usize>>>::into(c.text_range()),
                        indent_level,
                    )));
                }
            }

            // No value found (very rare case)
            None
        }
    }
}
