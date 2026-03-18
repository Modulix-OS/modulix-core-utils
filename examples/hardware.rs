use modulix_core_utils::{CONFIG_DIRECTORY, hardware_config};

fn main() {
    hardware_config::write_hardware("/", CONFIG_DIRECTORY).unwrap();
}
