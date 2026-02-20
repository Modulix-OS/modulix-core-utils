use crate::core::TABULATION_SIZE;
use crate::core::localise_option::{ExistingOption, NewInsertion, SettingsPosition};
use crate::mx;
use crate::mx::ErrorType;
use crate::transaction::file_lock::NixFile;
use crate::utils::count_chr::count_char_before_newline;
use std::{ops, str};

pub struct Option {
    position: SettingsPosition,
}

impl Option {
    fn get_pos_option_in_file(
        nix_file: &NixFile,
        nix_option: &str,
    ) -> mx::Result<SettingsPosition> {
        let ast = rnix::Root::parse(&nix_file.get_file_content()?);
        SettingsPosition::new(&ast.syntax(), nix_option)
    }

    pub fn get_option(nix_file: &NixFile, nix_option: &str) -> mx::Result<ExistingOption> {
        match Self::get_pos_option_in_file(nix_file, nix_option) {
            Ok(res) => match res {
                SettingsPosition::ExistingOption(pos) => Ok(pos),
                SettingsPosition::NewInsertion(_) => Err(ErrorType::OptionNotFound),
            },
            Err(e) => Err(e),
        }
    }

    pub fn new(nix_file: &NixFile, nix_option: &str) -> mx::Result<Self> {
        let opt = Self::get_pos_option_in_file(nix_file, nix_option)?;
        Ok(Option { position: opt })
    }

    pub fn set(mut self, nix_file: &mut NixFile, option_value: &str) -> mx::Result<()> {
        match self.position {
            SettingsPosition::NewInsertion(pos_insert) => {
                let indent = if pos_insert.get_indent_level() > 0usize {
                    (pos_insert.get_indent_level()) as usize
                } else {
                    1usize
                };

                let insert_pos = pos_insert.get_pos_new_insertion();

                let number_previous_indent =
                    count_char_before_newline(&nix_file.get_mut_file_content()?, insert_pos - 1);

                fn write_option<'a>(
                    mut path: str::Split<'a, char>,
                    indent: usize,
                    option_value: &str,
                ) -> (String, ops::Range<usize>, usize) {
                    if let Some(key) = path.next() {
                        let remaining = path.clone().count();
                        if remaining == 0 {
                            let prefix =
                                format!("{}{} = ", " ".repeat(TABULATION_SIZE * indent), key);
                            let start = prefix.len();
                            let end = start + option_value.len();
                            let result = format!(
                                "{}{};\n{}",
                                prefix,
                                &option_value,
                                " ".repeat(TABULATION_SIZE * (indent - 1usize))
                            );
                            return (result, start..end, indent);
                        } else {
                            let prefix =
                                format!("{}{} = {{\n", " ".repeat(TABULATION_SIZE * indent), key);
                            let (inner, inner_range, ind) =
                                write_option(path, indent + 1, option_value);

                            let result = format!(
                                "{}{}}};\n{}",
                                prefix,
                                inner,
                                " ".repeat(TABULATION_SIZE * (indent - 1usize))
                            );
                            let adjusted_range = (prefix.len() + inner_range.start)
                                ..(prefix.len() + inner_range.end);
                            return (result, adjusted_range, ind);
                        }
                    }
                    return (String::new(), 0..0, 0);
                }

                let (option_value, range_value, value_indent) = write_option(
                    pos_insert.get_remaining_path().split('.'),
                    indent,
                    option_value,
                );

                let begin = insert_pos - number_previous_indent;

                nix_file
                    .get_mut_file_content()?
                    .replace_range(begin..insert_pos, &option_value);
                let end = begin + option_value.len();
                self.position = SettingsPosition::ExistingOption(ExistingOption::new(
                    begin..end,
                    ops::Range {
                        start: range_value.start + begin,
                        end: range_value.end + begin,
                    },
                    value_indent,
                ));
            }
            SettingsPosition::ExistingOption(exist_pos) => {
                let range_value = exist_pos.get_range_option_value().clone();
                nix_file
                    .get_mut_file_content()?
                    .replace_range(range_value, &option_value);
            }
        }
        return Ok(());
    }

    pub fn get<'a>(self, nix_file: &'a NixFile) -> mx::Result<&'a str> {
        match self.position {
            SettingsPosition::ExistingOption(option) => {
                Ok(&nix_file.get_file_content()?[option.get_range_option_value().clone()])
            }
            SettingsPosition::NewInsertion(_) => Err(mx::ErrorType::OptionNotFound),
        }
    }

    pub fn set_option_to_default(mut self, nix_file: &mut NixFile) -> mx::Result<bool> {
        match self.position {
            SettingsPosition::ExistingOption(option) => {
                let option_path: String = nix_file.get_mut_file_content()?
                    [option.get_range_option().clone()]
                .split('=')
                .next()
                .unwrap()
                .trim()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();

                nix_file
                    .get_mut_file_content()?
                    .replace_range(option.get_range_option().clone(), "");
                let mut pos = option.get_range_option().start;
                while pos > 0
                    && match nix_file.get_mut_file_content()?.chars().nth(pos - 1usize) {
                        Some(' ') | Some('\t') | Some('\n') => true,
                        Some(_) | _ => false,
                    }
                {
                    pos -= 1;
                    nix_file.get_mut_file_content()?.remove(pos);
                }

                self.position = SettingsPosition::NewInsertion(NewInsertion::new(
                    pos,
                    &option_path,
                    option.get_indent_level(),
                ));
                Ok(true)
            }
            SettingsPosition::NewInsertion(_) => Ok(false),
        }
    }
}
