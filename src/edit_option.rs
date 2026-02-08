use std::{fs, ops::Range};
use rnix::{TextRange, TextSize};

use crate::{core::localise_option::SettingsPosition, core::write_file};

pub fn set_option(nix_file_path: &str, nix_option: &str, option_value: &str) -> Result<(), String>{

    let mut file_content = match fs::read_to_string(nix_file_path) {
        Ok(c) => c,
        Err(e) => return Err(e.to_string()),
    };
    let ast = rnix::Root::parse(&file_content);

    let pos = match SettingsPosition::new(&ast.syntax(), nix_option) {
        Some(p) => p,
        None => return Err(String::from("Impossible to set option in file")),
    };

    if let Some(path) = pos.get_remaining_path() {
        let indent = if pos.get_indent_level() > 0u8 {
            (pos.get_indent_level()-1u8) as usize
        } else {
            0usize
        };
        println!("{}", indent);
        file_content.insert_str(
            <TextSize as Into<usize>>::into(pos.get_pos_definition().start()),
            format!("{}{} = {};\n{}",
                "  ".repeat(indent),
                &path,
                &option_value,
                "  ".repeat(indent)
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
