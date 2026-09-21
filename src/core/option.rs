//! Reading and writing a single NixOS option inside a configuration file.

use super::TABULATION_SIZE;
use super::transaction::file_lock::NixFile;
use crate::core::localise_option::{
    ExistingOption, SettingsPosition, collect_child_names, collect_enable_paths,
};
use crate::mx;
use std::str;

/// One NixOS option, addressed by its dotted path, read from and written to a
/// [`NixFile`].
///
/// Editing is textual and AST-guided (`rnix`): the surrounding file keeps its
/// formatting and comments, and an option that is not declared yet is inserted
/// with the nesting its path implies.
///
/// # Fields
/// * `nix_option` - dotted path of the option (e.g. `mx.programs.git.enable`),
///   matched against both the flat and the nested spellings in the file.
pub struct Option<'a> {
    nix_option: &'a str,
}

impl<'a> Option<'a> {
    /// Locates `nix_option` in `nix_file` by parsing its content.
    ///
    /// # Parameters
    /// * `nix_file` - the file to search.
    /// * `nix_option` - dotted path to locate.
    ///
    /// # Returns
    /// [`SettingsPosition::ExistingOption`] with the ranges of the declaration
    /// when it is present, else [`SettingsPosition::NewInsertion`] describing
    /// where it would have to be written.
    ///
    /// # Errors
    /// [`mx::ErrorKind::InvalidFile`] if the file does not parse as Nix, plus
    /// any error from reading it.
    fn get_pos_option_in_file(
        nix_file: &NixFile,
        nix_option: &str,
    ) -> mx::Result<SettingsPosition> {
        let ast = rnix::Root::parse(&nix_file.get_file_content()?);
        SettingsPosition::new(&ast.syntax(), nix_option)
    }

    /// Counts how many characters precede `pos` on its own line, i.e. the
    /// indentation already written before an insertion point.
    ///
    /// # Parameters
    /// * `text` - the file content.
    /// * `pos` - byte offset to measure back from; must be a char boundary
    ///   within `text`.
    ///
    /// # Returns
    /// The number of bytes between `pos` and the preceding newline, or the
    /// start of `text` when there is none.
    fn count_char_before_newline(text: &str, mut pos: usize) -> usize {
        let bytes = text.as_bytes();
        let mut count = 0;
        while pos > 0 {
            pos -= 1;
            if bytes[pos] == b'\n' {
                break;
            }
            count += 1;
        }
        count
    }

    /// Locates this option in `nix_file`.
    ///
    /// # Parameters
    /// * `nix_file` - the file to search.
    ///
    /// # Returns
    /// The same as [`Option::get_pos_option_in_file`]; used by
    /// [`super::list::List`] to tell an existing list from one to create.
    pub(super) fn get_position(&self, nix_file: &NixFile) -> mx::Result<SettingsPosition> {
        Self::get_pos_option_in_file(nix_file, self.nix_option)
    }

    /// Reads the position of a declared option without building an
    /// [`Option`] first.
    ///
    /// # Parameters
    /// * `nix_file` - the file to search.
    /// * `nix_option` - dotted path of the option.
    ///
    /// # Returns
    /// The declaration's position, ranges included.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionNotFound`] when the option is not declared -
    /// unlike [`Option::get_position`], which reports where to insert it.
    #[allow(dead_code)]
    pub fn get_option(nix_file: &NixFile, nix_option: &str) -> mx::Result<ExistingOption> {
        match Self::get_pos_option_in_file(nix_file, nix_option) {
            Ok(res) => match res {
                SettingsPosition::ExistingOption(pos) => Ok(pos),
                SettingsPosition::NewInsertion(_) => Err(mx::ErrorKind::OptionNotFound),
            },
            Err(e) => Err(e),
        }
    }

    /// Builds a handle on the option at `nix_option`.
    ///
    /// # Parameters
    /// * `nix_option` - dotted path of the option; nothing is read or
    ///   validated here, so the path need not exist in any file yet.
    ///
    /// # Returns
    /// A handle borrowing `nix_option`, usable against any [`NixFile`].
    pub fn new(nix_option: &'a str) -> Self {
        Option {
            nix_option: nix_option,
        }
    }

    /// Immediate child attribute names nested under this option's dotted path
    /// (e.g. `mxOption::new("mx")` returns every enabled/declared module name).
    /// Flat and nested spellings are merged; the value under each child is read
    /// separately with [`Option::get`].
    ///
    /// # Parameters
    /// * `nix_file` - the file to read.
    ///
    /// # Returns
    /// The first path segment below this option, once per distinct name;
    /// empty when the option is absent - not an error.
    pub fn list_children(&self, nix_file: &NixFile) -> mx::Result<Vec<String>> {
        let ast = rnix::Root::parse(nix_file.get_file_content()?);
        Ok(collect_child_names(&ast.syntax(), self.nix_option))
    }

    /// Dotted paths of the descendants of this option that declare an `enable`
    /// attribute, at any depth: `mxOption::new("mx")` on
    /// `mx.programs.studio.obs-studio.enable` yields
    /// `["programs.studio.obs-studio"]`. [`Option::list_children`] only sees the
    /// first segment (`programs`), which is not a module name. The value under
    /// each `enable` is read separately with [`Option::get`].
    ///
    /// # Parameters
    /// * `nix_file` - the file to read.
    ///
    /// # Returns
    /// One dotted path per descendant declaring `enable`, relative to this
    /// option and without the `enable` segment; empty when there is none.
    /// Whether each is set to `true` is not checked here.
    pub fn list_enable_descendants(&self, nix_file: &NixFile) -> mx::Result<Vec<String>> {
        let ast = rnix::Root::parse(nix_file.get_file_content()?);
        Ok(collect_enable_paths(&ast.syntax(), self.nix_option))
    }

    /// Assigns a value to this option, declaring it if needed.
    ///
    /// # Parameters
    /// * `nix_file` - the open file to edit, in memory only.
    /// * `option_value` - the Nix expression to assign, written verbatim: a
    ///   string value must already carry its quotes (see
    ///   [`super::utils::value_to_string_nix`]).
    ///
    /// # Post-conditions
    /// An existing declaration has its value replaced in place, formatting
    /// preserved; an absent one is inserted with the nested attribute sets its
    /// path implies, indented with [`super::TABULATION_SIZE`]. The file is only
    /// written back to disk when its transaction commits.
    ///
    /// # Returns
    /// `self`, so assignments can be chained.
    pub fn set(&self, nix_file: &mut NixFile, option_value: &str) -> mx::Result<&Self> {
        match Self::get_pos_option_in_file(&nix_file, self.nix_option)? {
            SettingsPosition::NewInsertion(pos_insert) => {
                let indent = if pos_insert.get_indent_level() > 0usize {
                    (pos_insert.get_indent_level()) as usize
                } else {
                    1usize
                };

                let insert_pos = pos_insert.get_pos_new_insertion();
                let number_previous_indent =
                    Self::count_char_before_newline(&nix_file.get_mut_file_content()?, insert_pos);

                fn write_option<'a>(
                    mut path: str::Split<'a, char>,
                    indent: usize,
                    option_value: &str,
                ) -> String {
                    if let Some(key) = path.next() {
                        let remaining = path.clone().count();
                        if remaining == 0 {
                            return format!(
                                "{}{} = {};\n{}",
                                " ".repeat(TABULATION_SIZE * indent),
                                key,
                                &option_value,
                                " ".repeat(TABULATION_SIZE * (indent - 1usize))
                            );
                        } else {
                            let prefix =
                                format!("{}{} = {{\n", " ".repeat(TABULATION_SIZE * indent), key);
                            let inner = write_option(path, indent + 1, option_value);
                            let result = format!(
                                "{}{}}};\n{}",
                                prefix,
                                inner,
                                " ".repeat(TABULATION_SIZE * (indent - 1usize))
                            );
                            return result;
                        }
                    }
                    return String::new();
                }

                let option_value = write_option(
                    pos_insert.get_remaining_path().split('.'),
                    indent,
                    option_value,
                );
                let begin = insert_pos - number_previous_indent;

                nix_file
                    .get_mut_file_content()?
                    .replace_range(begin..insert_pos, &option_value);
            }
            SettingsPosition::ExistingOption(exist_pos) => {
                let range_value = exist_pos.get_range_option_value().clone();
                nix_file
                    .get_mut_file_content()?
                    .replace_range(range_value, &option_value);
            }
        }
        return Ok(&self);
    }

    /// Reads this option's value as it is written in the file.
    ///
    /// # Parameters
    /// * `nix_file` - the file to read.
    ///
    /// # Returns
    /// The raw Nix expression, borrowed from the file content: a string value
    /// still carries its quotes, a list its brackets. Use
    /// [`super::utils::string_nix_to_value`] to unquote.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionNotFound`] when the option is not declared.
    pub fn get(&self, nix_file: &'a NixFile) -> mx::Result<&'a str> {
        match Self::get_pos_option_in_file(nix_file, self.nix_option)? {
            SettingsPosition::ExistingOption(option) => {
                Ok(&nix_file.get_file_content()?[option.get_range_option_value().clone()])
            }
            SettingsPosition::NewInsertion(_) => Err(mx::ErrorKind::OptionNotFound),
        }
    }

    /// Deletes this option's declaration, letting NixOS fall back to the
    /// option's own default.
    ///
    /// # Parameters
    /// * `nix_file` - the open file to edit.
    ///
    /// # Returns
    /// `true` if a declaration was removed, `false` if the option was not
    /// declared - which is not an error.
    ///
    /// # Post-conditions
    /// The whole assignment goes, sub-attributes included, along with the
    /// whitespace that preceded it. Only the first declaration is removed; see
    /// [`Option::set_option_all_instance_to_default`] for the rest.
    pub fn set_option_to_default(&self, nix_file: &mut NixFile) -> mx::Result<bool> {
        match Self::get_pos_option_in_file(nix_file, self.nix_option)? {
            SettingsPosition::ExistingOption(option) => {
                nix_file
                    .get_mut_file_content()?
                    .replace_range(option.get_range_option().clone(), "");
                let content = nix_file.get_mut_file_content()?;
                let start = option.get_range_option().start - 1;

                let trim_start = content[..start]
                    .trim_end_matches(|c| c == ' ' || c == '\t' || c == '\n')
                    .len();

                content.drain(trim_start..start);
                Ok(true)
            }
            SettingsPosition::NewInsertion(_) => Ok(false),
        }
    }

    /// Repeats [`Option::set_option_to_default`] until nothing is left,
    /// clearing an option declared several times in the same file.
    ///
    /// # Parameters
    /// * `nix_file` - the open file to edit.
    ///
    /// # Returns
    /// `true` if at least one declaration was removed, `false` if there was
    /// none.
    ///
    /// # Post-conditions
    /// No declaration of this option remains in the file.
    pub fn set_option_all_instance_to_default(&self, nix_file: &mut NixFile) -> mx::Result<bool> {
        let mut found = false;
        while self.set_option_to_default(nix_file)? {
            found = true;
        }
        Ok(found)
    }
}
