use std::fs;
use crate::edit_ast::edit_option_ast;

pub fn get_option(nix_file_path: &str, nix_option: &str) -> Result<String, String> {
    let file_content = match fs::read_to_string(nix_file_path) {
        Ok(c) => c,
        Err(e) => return Err(e.to_string()),
    };

    edit_option_ast::get_option(&file_content, nix_option)
}

pub fn set_option(nix_file_path: &str, nix_option: &str, option_value: &str) -> Result<(), String>{
    let mut file_content = match fs::read_to_string(nix_file_path) {
        Ok(c) => c,
        Err(e) => return Err(e.to_string()),
    };

    edit_option_ast::set_option(&mut file_content, nix_file_path, nix_option, option_value)
}

// Delete option in file
pub fn set_option_to_default(nix_file_path: &str, nix_option: &str) -> Result<bool, String> {
    let mut file_content = match fs::read_to_string(nix_file_path) {
        Ok(c) => c,
        Err(e) => return Err(e.to_string()),
    };

    edit_option_ast::set_option_to_default(&mut file_content, nix_file_path, nix_option)
}
