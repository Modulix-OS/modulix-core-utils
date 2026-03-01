use modulix_core_utils::filesystem;

fn main() {
    filesystem::add_entry(
        "/mnt/Games",
        "/dev/disk/by-uuid/1b35568b-4447-4c80-9880-4b359d4ecb6c",
        "ext4",
        &["noatime", "nodiratime", "discard", "defaults", "commit=120"],
        false,
    )
    .unwrap();

    filesystem::add_entry(
        "/",
        "/dev/disk/by-uuid/208b9468-df96-4f4a-b381-3275e42a77c6",
        "ext4",
        &["noatime", "nodiratime", "discard", "defaults", "commit=120"],
        true,
    )
    .unwrap();
}
