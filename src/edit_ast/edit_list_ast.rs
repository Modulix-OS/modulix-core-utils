use crate::core::TABULATION_SIZE;
use crate::edit_ast::edit_option_ast;
use rnix::TextRange;
use std::ops::Range;

fn str_is_list(list: &str) -> bool {
    list.len() >= 2
        && list.chars().nth(0).unwrap() == '['
        && list.chars().nth_back(0).unwrap() == ']'
}

pub fn get_elem_in_list(file_content: &str, nix_list: &str) -> Result<Vec<String>, String> {
    let val_list = edit_option_ast::pos_option_in_file(&file_content, nix_list)?;

    if let Some(list_pos) = val_list.get_pos_definition_value() {
        Ok(file_content
            .get(<TextRange as Into<Range<usize>>>::into(list_pos))
            .ok_or(String::from("Impossible to read list"))?
            .strip_prefix('[')
            .ok_or(String::from("Option is not a valid list"))?
            .strip_suffix(']')
            .ok_or(String::from("Option is not a valid list"))?
            .split_ascii_whitespace()
            .map(|s| s.to_string())
            .collect())
    } else {
        Err(String::from("List not found"))
    }
}

pub fn add_in_list(
    mut file_content: &mut String,
    nix_file_path: &str,
    nix_list: &str,
    insert_value: &str,
    unique_value_in_list: bool,
) -> Result<(), String> {
    let val_list = edit_option_ast::pos_option_in_file(&file_content, nix_list)?;

    let indent_level = val_list.get_indent_level();

    if let Some(list_pos) = val_list.get_pos_definition_value() {
        let mut list = file_content
            .get(<TextRange as Into<Range<usize>>>::into(list_pos))
            .ok_or(String::from("Impossible to read list"))?
            .to_string();
        if !str_is_list(&list) {
            return Err(String::from("This option is not a list"));
        }
        if !unique_value_in_list
            || list
                .strip_prefix('[')
                .unwrap()
                .strip_suffix(']')
                .unwrap()
                .split_ascii_whitespace()
                .all(|e| e != insert_value)
        {
            let mut pos = 1;
            let newline = loop {
                match list.chars().nth_back(pos) {
                    Some('\n') => {
                        break false;
                    }
                    Some(c) if !c.is_whitespace() => {
                        break true;
                    }
                    _ => (),
                }
                pos += 1
            };
            pos -= 1;
            let str_before = format!(
                "{}{}",
                if newline { "\n" } else { "" },
                String::from(" ").repeat(TABULATION_SIZE * (indent_level as usize + 1) - pos)
            );
            let str_after = String::from(" ").repeat(TABULATION_SIZE * (indent_level as usize));
            list.insert_str(
                list.len() - 1usize,
                format!("{}{}\n{}", str_before, insert_value, str_after).as_str(),
            );
            edit_option_ast::set_option(&mut file_content, nix_file_path, nix_list, list.as_str())?
        }
    } else {
        let nb_elem_path = nix_list.split('.').count();
        edit_option_ast::set_option(
            &mut file_content,
            nix_file_path,
            nix_list,
            format!(
                "[\n{}{}\n{}]",
                String::from(" ").repeat(TABULATION_SIZE * (nb_elem_path + 1)),
                insert_value,
                String::from(" ").repeat(TABULATION_SIZE * (nb_elem_path))
            )
            .as_str(),
        )?
    }

    Ok(())
}

/// Remove first instance of value in list
pub fn remove_in_list(
    mut file_content: &mut String,
    nix_file_path: &str,
    nix_list: &str,
    insert_value: &str,
) -> Result<(), String> {
    let val_list = edit_option_ast::pos_option_in_file(&file_content, nix_list)?;

    if let Some(list_pos) = val_list.get_pos_definition_value() {
        let mut list = file_content
            .get(<TextRange as Into<Range<usize>>>::into(list_pos))
            .ok_or(String::from("Impossible to read list"))?
            .to_string();
        if !str_is_list(&list) {
            return Err(String::from("This option is not a list"));
        }

        let mut start: usize = 0;
        let mut end: usize = 0;
        let mut found = false;
        let mut _offset = 1;

        for elem in list
            .strip_prefix('[')
            .unwrap()
            .strip_suffix(']')
            .unwrap()
            .split_ascii_whitespace()
        {
            let s = list[_offset..].find(elem).unwrap() + _offset;
            let e = s + elem.len();
            if elem == insert_value {
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
                edit_option_ast::set_option_to_default(&mut file_content, nix_file_path, nix_list)?;
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
                edit_option_ast::set_option(&mut file_content, nix_file_path, nix_list, &list)?;
            }
        }
    }
    Ok(())
}
