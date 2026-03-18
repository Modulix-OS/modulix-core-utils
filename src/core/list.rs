use std::collections::HashSet;
use std::str::SplitAsciiWhitespace;

use super::option::Option as mxOption;
use super::transaction::file_lock::NixFile;
use super::{TABULATION_SIZE, localise_option::SettingsPosition};
use crate::mx;

pub struct List<'a> {
    opt_list: mxOption<'a>,
    unique_value_in_list: bool,
}

impl<'a> List<'a> {
    fn str_is_list(list: &str) -> bool {
        list.len() >= 2
            && list.chars().nth(0).unwrap() == '['
            && list.chars().nth_back(0).unwrap() == ']'
    }

    pub fn new(nix_list: &'a str, unique_value: bool) -> Self {
        List {
            opt_list: mxOption::new(nix_list),
            unique_value_in_list: unique_value,
        }
    }

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

    #[allow(dead_code)]
    pub fn eq(&self, nix_file: &NixFile, desired_value: &[&str]) -> mx::Result<bool> {
        //let opt = get_option(file_content, list_name)?;
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
