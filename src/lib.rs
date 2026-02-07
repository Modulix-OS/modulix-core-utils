mod core;
use core::localise_option::SettingsPosition;
use std::fs;

pub fn print_pos_option(path_file: &str, settings: &str) {
    let file_content = fs::read_to_string(path_file).unwrap();
    let ast = rnix::Root::parse(&file_content);
    let opt_pos = SettingsPosition::new(&ast.syntax(), &settings);

    if let Some(pos) = opt_pos {
        let position = format!("{:?}..{:?}",
            pos.get_pos_definition().start(),
            pos.get_pos_definition().end());
        let position_value = match pos.get_pos_definition_value() {
            Some(pos_def_v) =>
                format!("{:?}..{:?}",
                    pos_def_v.start(),
                    pos_def_v.end()),
            None => String::from("None"),
        };
        let remaining_path = match pos.get_remaining_path() {
            Some(path) => path,
            None => "None",
        };
        println!("Option: {}\nPosition: {}\nPosition valeur: {}\nPath restant {}",
            settings,
            position,
            position_value,
            remaining_path,
        );
    }
    else {
        println!("Option: {} error in file", &settings);
    }
}
