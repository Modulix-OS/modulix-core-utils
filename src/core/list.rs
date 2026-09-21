//! Element-wise editing of a NixOS option holding a list.

use std::collections::HashSet;
use std::str::SplitAsciiWhitespace;

use super::option::Option as mxOption;
use super::transaction::file_lock::NixFile;
use super::{TABULATION_SIZE, localise_option::SettingsPosition};
use crate::mx;

/// A NixOS option holding a list (`environment.systemPackages`,
/// `swapDevices`, a module's `plugins`, …), edited element by element.
///
/// Elements are handled as raw text: an element is whatever Nix expression the
/// caller passes, and two elements are "the same" when their text matches, so
/// spelling must stay consistent between adds and removes.
///
/// # Fields
/// * `opt_list` - the underlying option this list is stored in.
/// * `unique_value_in_list` - when true, [`List::add`] skips an element the
///   list already holds, turning the list into a set.
pub struct List<'a> {
    opt_list: mxOption<'a>,
    unique_value_in_list: bool,
}

impl<'a> List<'a> {
    /// Cheap check that a raw option value looks like a Nix list.
    ///
    /// # Parameters
    /// * `list` - the raw value read from the file.
    ///
    /// # Returns
    /// `true` when `list` starts with `[` and ends with `]`; the contents are
    /// not validated.
    fn str_is_list(list: &str) -> bool {
        list.len() >= 2
            && list.chars().nth(0).unwrap() == '['
            && list.chars().nth_back(0).unwrap() == ']'
    }

    /// Builds a handle on the list option at `nix_list`.
    ///
    /// # Parameters
    /// * `nix_list` - dotted path of the option holding the list.
    /// * `unique_value` - whether [`List::add`] must refuse duplicates.
    ///
    /// # Returns
    /// A handle borrowing `nix_list`; nothing is read yet, and the option need
    /// not exist.
    pub fn new(nix_list: &'a str, unique_value: bool) -> Self {
        List {
            opt_list: mxOption::new(nix_list),
            unique_value_in_list: unique_value,
        }
    }

    /// Appends an element to the list, declaring the option if needed.
    ///
    /// # Parameters
    /// * `nix_file` - the open file to edit, in memory only.
    /// * `insert_value` - the element as a Nix expression, inserted verbatim;
    ///   it must contain no whitespace, since elements are told apart by
    ///   whitespace splitting.
    ///
    /// # Post-conditions
    /// An absent option is first declared as `[]`. The element lands on its own
    /// line, indented one level deeper than the option. When
    /// `unique_value_in_list` is set and the element is already there, nothing
    /// changes.
    ///
    /// # Returns
    /// `self`, so several additions can be chained.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionIsNotList`] when the option exists but holds
    /// something other than a list.
    pub fn add(&self, nix_file: &mut NixFile, insert_value: &str) -> mx::Result<&Self> {
        match self.opt_list.get_position(nix_file)? {
            SettingsPosition::ExistingOption(option) => {
                let indent_level = option.get_indent_level();
                let mut list = self.opt_list.get(nix_file)?.to_string();
                if !Self::str_is_list(&list) {
                    return Err(mx::ErrorKind::OptionIsNotList);
                }
                if !self.unique_value_in_list
                    || list
                        .strip_prefix('[')
                        .unwrap()
                        .strip_suffix(']')
                        .unwrap()
                        .split_ascii_whitespace()
                        .all(|e| e != insert_value)
                {
                    let bytes = list.as_bytes();
                    let mut back = 2;
                    let newline = loop {
                        if back > bytes.len() {
                            break false;
                        }
                        let b = bytes[bytes.len() - back];
                        if b == b'\n' {
                            break false;
                        }
                        if !(b as char).is_whitespace() {
                            break true;
                        }
                        back += 1;
                    };
                    back -= TABULATION_SIZE;
                    let str_before = format!(
                        "{}{}",
                        if newline { "\n" } else { "" },
                        " ".repeat(TABULATION_SIZE * (indent_level as usize + 1) - back)
                    );
                    let str_after =
                        String::from(" ").repeat(TABULATION_SIZE * (indent_level as usize));
                    list.insert_str(
                        list.len() - 1usize,
                        format!("{}{}\n{}", str_before, insert_value, str_after).as_str(),
                    );
                    self.opt_list.set(nix_file, &list)?;
                }
            }
            SettingsPosition::NewInsertion(_) => {
                self.opt_list.set(nix_file, "[]")?;
                self.add(nix_file, insert_value)?;
            }
        }
        Ok(self)
    }

    /// Removes an element from the list.
    ///
    /// # Parameters
    /// * `nix_file` - the open file to edit.
    /// * `value` - the element to remove, matched against the file text
    ///   exactly as it was added.
    ///
    /// # Post-conditions
    /// Only the first occurrence goes. When it was the only element, the whole
    /// option declaration is dropped rather than left as `[]`. An element that
    /// is not there, or an option that is not declared, leaves the file
    /// untouched without an error.
    ///
    /// # Returns
    /// `self`, so several removals can be chained.
    pub fn remove(&self, nix_file: &mut NixFile, value: &str) -> mx::Result<&Self> {
        match self.opt_list.get_position(nix_file)? {
            SettingsPosition::ExistingOption(_) => {
                let mut list = self.opt_list.get(nix_file)?.to_string();

                let mut start: usize = 0;
                let mut end: usize = 0;
                let mut found = false;
                let mut _offset = 1;

                for elem in self.get_element_in_list(nix_file)? {
                    let s = list[_offset..].find(elem).unwrap() + _offset;
                    let e = s + elem.len();
                    if elem == value {
                        start = s;
                        end = e;
                        _offset = end;
                        found = true;
                        break;
                    }
                }

                if found {
                    if list
                        .strip_prefix('[')
                        .unwrap()
                        .strip_suffix(']')
                        .unwrap()
                        .split_ascii_whitespace()
                        .count()
                        == 1
                    {
                        self.opt_list.set_option_to_default(nix_file)?;
                    } else {
                        list.replace_range(start..end, "");
                        let mut pos = start - 1;
                        while pos > 0
                            && match list.chars().nth(pos) {
                                Some(' ') | Some('\t') | Some('\n') => true,
                                Some(_) | _ => false,
                            }
                        {
                            list.remove(pos);
                            pos -= 1;
                        }
                        self.opt_list.set(nix_file, &list)?;
                    }
                }
            }
            SettingsPosition::NewInsertion(_) => (),
        }
        Ok(self)
    }

    /// Iterates over the list's elements.
    ///
    /// # Parameters
    /// * `nix_file` - the file to read.
    ///
    /// # Returns
    /// The elements as raw Nix expressions, borrowed from the file content, in
    /// declaration order - obtained by whitespace splitting, so an element
    /// written across several tokens comes back split.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionNotFound`] when the option is absent, and
    /// [`mx::ErrorKind::OptionIsNotList`] when it holds something else.
    pub fn get_element_in_list(
        &self,
        nix_file: &'a NixFile,
    ) -> mx::Result<SplitAsciiWhitespace<'a>> {
        let list = self.opt_list.get(nix_file)?;
        if !Self::str_is_list(&list) {
            return Err(mx::ErrorKind::OptionIsNotList);
        }
        Ok(list
            .strip_prefix('[')
            .unwrap()
            .strip_suffix(']')
            .unwrap()
            .split_ascii_whitespace())
    }

    /// Compares the list's contents with an expected set of elements.
    ///
    /// # Parameters
    /// * `nix_file` - the file to read.
    /// * `desired_value` - the expected elements, spelled as in the file.
    ///
    /// # Returns
    /// `true` when both hold the same elements, order and duplicates ignored
    /// (both sides are compared as sets).
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionNotFound`] when the option is absent, and
    /// [`mx::ErrorKind::OptionIsNotList`] when it holds something else.
    #[allow(dead_code)]
    pub fn eq(&self, nix_file: &NixFile, desired_value: &[&str]) -> mx::Result<bool> {
        let set_current_list: HashSet<&str> = self
            .opt_list
            .get(nix_file)?
            .strip_prefix('[')
            .ok_or(mx::ErrorKind::OptionIsNotList)?
            .strip_suffix(']')
            .ok_or(mx::ErrorKind::OptionIsNotList)?
            .split_ascii_whitespace()
            .collect();

        let set_desired_value: HashSet<&str> = desired_value.iter().copied().collect();

        Ok(set_desired_value == set_current_list)
    }

    /// Tells whether the list holds a given element.
    ///
    /// # Parameters
    /// * `nix_file` - the file to read.
    /// * `desired_value` - the element to look for, spelled as in the file.
    ///
    /// # Returns
    /// `true` when the element is present; `false` when it is not, and also
    /// when the option is not declared at all.
    ///
    /// # Errors
    /// [`mx::ErrorKind::OptionIsNotList`] when the option holds something
    /// other than a list.
    #[allow(dead_code)]
    pub fn countains(&self, nix_file: &NixFile, desired_value: &str) -> mx::Result<bool> {
        Ok(match self.opt_list.get(nix_file) {
            Ok(list) => list
                .strip_prefix('[')
                .ok_or(mx::ErrorKind::OptionIsNotList)?
                .strip_suffix(']')
                .ok_or(mx::ErrorKind::OptionIsNotList)?
                .split_ascii_whitespace()
                .any(|v| v == desired_value),
            Err(mx::ErrorKind::OptionNotFound) => false,
            Err(e) => return Err(e),
        })
    }
}
