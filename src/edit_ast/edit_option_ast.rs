use crate::core::{TABULATION_SIZE, localise_option::SettingsPosition, write_file};
use rnix::{TextRange, TextSize};
use std::ops::Range;

pub fn pos_option_in_file<'a>(file_content: &str, nix_option: &'a str) -> Result<SettingsPosition<'a>, String> {
    let ast = rnix::Root::parse(&file_content);
    match SettingsPosition::new(&ast.syntax(), nix_option) {
        Some(p) => Ok(p),
        None => return Err(String::from("Impossible to get position option in file")),
    }
}

fn count_space_before_newline(text: &str, mut initial_pos: usize) -> usize {
    initial_pos += 1;
    let mut number_indent = 0;
    while initial_pos > 0 && text.chars().nth(initial_pos-1).unwrap_or('\n') != '\n' {
        initial_pos -= 1;
        number_indent += 1;
    }
    number_indent
}

pub fn get_option(file_content: &str, nix_option: &str) -> Result<String, String> {
    let pos = pos_option_in_file(&file_content, nix_option)?;

    if let Some(value) = pos.get_pos_definition_value() {
        let range = <TextRange as Into<Range<usize>>>::into(value);

        return Ok(file_content.get(range).ok_or(String::from("Range value not found in file"))?.to_string());
    }
    Err(String::from("Value not defined in this file"))
}

pub fn set_option(
    file_content: &mut String,
    nix_file_path: &str,
    nix_option: &str,
    option_value: &str
) -> Result<(), String>
{
    let pos = pos_option_in_file(&file_content, nix_option)?;

    if let Some(path) = pos.get_remaining_path() {
        let indent = if pos.get_indent_level() > 0u8 {
            (pos.get_indent_level()) as usize
        } else {
            1usize
        };

        let insert_pos = <TextSize as Into<usize>>::into(pos.get_pos_definition().start());

        let number_indent = count_space_before_newline(&file_content, insert_pos-1)/TABULATION_SIZE;

        println!("{}: {}, indent: {}, number_already indent {}", path, option_value, indent, number_indent);
        file_content.insert_str(
            insert_pos,
            format!("{}{} = {};\n{}",
                " ".repeat(TABULATION_SIZE*(indent - number_indent)),
                &path,
                &option_value,
                " ".repeat(TABULATION_SIZE*(indent-1usize))
            ).as_str());

    } else {
        if let Some(value) = pos.get_pos_definition_value() {
            file_content.replace_range(<TextRange as Into<Range<usize>>>::into(value), &option_value);
        }
        else {
            return Err(String::from("Unknow error"));
        }
    }
    write_file::write_file(nix_file_path, file_content.as_str())?;
    return Ok(());
}

pub fn set_option_to_default(
    file_content: &mut String,
    nix_file_path: &str,
    nix_option: &str
) -> Result<bool, String> {
    let pos = pos_option_in_file(&file_content, nix_option)?;

    if let Some(_) = pos.get_pos_definition_value() {
        file_content.replace_range(<TextRange as Into<Range<usize>>>::into( pos.get_pos_definition()), "");
        let mut pos = <TextSize as Into<usize>>::into(pos.get_pos_definition().start());
        while pos > 0 && match file_content.chars().nth(pos-1usize) {
            Some(' ') | Some('\t') | Some('\n') => true,
            Some(_) | _ => false,
        } {
            file_content.remove(pos-1usize);
            pos-=1;
        }
        write_file::write_file(nix_file_path, file_content.as_str())?;
        Ok(true)
    } else {
        Ok(false)
    }
}
