use std::{fs, ops::Range};
use rnix::TextRange;
use crate::edit_ast::edit_list_ast;

fn str_is_list(list: &str) -> bool {
    list.len() >= 2
    && list.chars().nth(0).unwrap() == '['
    && list.chars().nth_back(0).unwrap() == ']'
}

pub fn add_in_list(
    nix_file_path: &str,
    nix_list: &str,
    insert_value: &str,
    unique_value_in_list: bool
)
    -> Result<(), String>
{
    let mut file_content = match fs::read_to_string(nix_file_path) {
        Ok(c) => c,
        Err(e) => return Err(e.to_string()),
    };

    edit_list_ast::add_in_list(&mut file_content, nix_file_path, nix_list, insert_value, unique_value_in_list)
}

/// Remove first instance of value in list
pub fn remove_in_list(
    nix_file_path: &str,
    nix_list: &str,
    insert_value: &str
)
-> Result<(), String>
{
    let mut file_content = match fs::read_to_string(nix_file_path) {
        Ok(c) => c,
        Err(e) => return Err(e.to_string()),
    };

    edit_list_ast::remove_in_list(&mut file_content, nix_file_path, nix_list, insert_value)
}

pub fn get_elem_in_list(
    nix_file_path: &str,
    nix_list: &str
) -> Result<Vec<String>, String> {
    let file_content = match fs::read_to_string(nix_file_path) {
        Ok(c) => c,
        Err(e) => return Err(e.to_string()),
    };

    edit_list_ast::get_elem_in_list(&file_content, &nix_list)
}
