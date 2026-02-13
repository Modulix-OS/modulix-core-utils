use std::fs;

use crate::edit_ast::edit_list_ast;
use crate::edit_ast::edit_option_ast;

const FILE_SYSTEM_PATH: &str = "./test.nix";

pub fn filesystem_add_entry(
    mount_point: &str,
    device: &str,
    fs_type: &str,
    option: &Vec<&str>,
) {

    let root_option = format!("fileSystems.\"{}\"", mount_point);

    let mut fstab = fs::read_to_string(FILE_SYSTEM_PATH)
        .unwrap();

    edit_option_ast::set_option(
        &mut fstab,
        FILE_SYSTEM_PATH,
        format!("{}.device", root_option).as_str(),
        format!("\"{}\"", device).as_str())
    .unwrap();

    edit_option_ast::set_option(
        &mut fstab,
        FILE_SYSTEM_PATH,
        format!("{}.fsType", root_option).as_str(),
        format!("\"{}\"", fs_type).as_str()).unwrap();

    let option_path = format!("{}.options", root_option);

    edit_option_ast::set_option_to_default(&mut fstab, FILE_SYSTEM_PATH, &option_path).unwrap();

    for o in option {
        edit_list_ast::add_in_list(&mut fstab, FILE_SYSTEM_PATH, &option_path, &format!("\"{}\"", o), true).unwrap();
    }


}
