use modulix_core_utils::{CONFIG_DIRECTORY, flake_input::add_input};

fn main() {
    match add_input(
        CONFIG_DIRECTORY,
        "qhorgues-config",
        "github:qhorgues/NixOS-config",
        None,
    ) {
        Ok(()) => println!("Input added successfully"),
        Err(e) => println!("Error adding input: {}", e.to_string()),
    }
}
