use modulix_core_utils::{CONFIG_DIRECTORY, filesystem};

fn main() {
    filesystem::add_entry(
        CONFIG_DIRECTORY,
        "/mnt/Games",
        "/dev/disk/by-uuid/1b35568b-4447-4c80-9880-4b359d4ecb6c",
        "ext4",
        &[],
        false,
    )
    .unwrap();
}
