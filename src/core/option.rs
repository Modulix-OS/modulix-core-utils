use super::transaction::file_lock::NixFile;
use crate::core::TABULATION_SIZE;
use crate::core::localise_option::{ExistingOption, SettingsPosition};
use crate::mx;
use std::str;

pub struct Option<'a> {
    nix_option: &'a str,
}

impl<'a> Option<'a> {
    fn get_pos_option_in_file(
        nix_file: &NixFile,
        nix_option: &str,
    ) -> mx::Result<SettingsPosition> {
        let ast = rnix::Root::parse(&nix_file.get_file_content()?);
        SettingsPosition::new(&ast.syntax(), nix_option)
    }

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

    pub(super) fn get_position(&self, nix_file: &NixFile) -> mx::Result<SettingsPosition> {
        Self::get_pos_option_in_file(nix_file, self.nix_option)
    }

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

    pub fn new(nix_option: &'a str) -> Self {
        Option {
            nix_option: nix_option,
        }
    }

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

    pub fn get(&self, nix_file: &'a NixFile) -> mx::Result<&'a str> {
        match Self::get_pos_option_in_file(nix_file, self.nix_option)? {
            SettingsPosition::ExistingOption(option) => {
                Ok(&nix_file.get_file_content()?[option.get_range_option_value().clone()])
            }
            SettingsPosition::NewInsertion(_) => Err(mx::ErrorKind::OptionNotFound),
        }
    }

    pub fn set_option_to_default(&self, nix_file: &mut NixFile) -> mx::Result<bool> {
        match Self::get_pos_option_in_file(nix_file, self.nix_option)? {
            SettingsPosition::ExistingOption(option) => {
                nix_file
                    .get_mut_file_content()?
                    .replace_range(option.get_range_option().clone(), "");
                let content = nix_file.get_mut_file_content()?;
                let start = option.get_range_option().start - 1;

                // Trouver jusqu'où remonter en une seule passe
                let trim_start = content[..start]
                    .trim_end_matches(|c| c == ' ' || c == '\t' || c == '\n')
                    .len();

                // Supprimer en une seule opération
                content.drain(trim_start..start);
                Ok(true)
            }
            SettingsPosition::NewInsertion(_) => Ok(false),
        }
    }

    pub fn set_option_all_instance_to_default(&self, nix_file: &mut NixFile) -> mx::Result<bool> {
        let mut found = false;
        while self.set_option_to_default(nix_file)? {
            found = true;
        }
        Ok(found)
    }
}
