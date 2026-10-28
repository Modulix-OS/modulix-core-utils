use modulix_core_utils::detect_hardware::driver_config::DriverConfig;

fn main() {
    let driver = DriverConfig::new().unwrap();
    for e in driver.get_module() {
        println!("{}", e);
    }
    //hardware_config::write_hardware("/", CONFIG_DIRECTORY).unwrap();
}
