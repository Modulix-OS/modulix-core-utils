use modulix_core_utils::filesystem;

fn main() {
    filesystem::add_entry(
        "/mnt/Games",
        "/dev/disk/by-uuid/1b35568b-4447-4c80-9880-4b359d4ecb6c",
        "ext4",
        &[],
        false,
    )
    .unwrap();
}
