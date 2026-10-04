use modulix_core_utils::{
    CONFIG_DIRECTORY,
    filesystem::{self, MountDevice},
};

fn main() {
    filesystem::add_mount(
        CONFIG_DIRECTORY,
        "/mnt/Games",
        &MountDevice::Plain {
            device: "/dev/disk/by-uuid/1b35568b-4447-4c80-9880-4b359d4ecb6c",
        },
        "ext4",
        &["noatime", "nofail"],
    )
    .unwrap();

    // An encrypted volume: the caller states the container and the mapper it
    // is open as, nothing else. The `boot.initrd.luks.devices` entry, the
    // mapper name and the `/dev/mapper/…` the mount point lands on all follow
    // from that — and `remove_mount` undoes the whole thing.
    filesystem::add_mount(
        CONFIG_DIRECTORY,
        "/mnt/Vault",
        &MountDevice::Luks {
            container: "/dev/disk/by-uuid/208b9468-df96-4f4a-b381-3275e42a77c6",
            mapper_device: Some("/dev/mapper/luks-208b9468-df96-4f4a-b381-3275e42a77c6"),
            tpm2: false,
        },
        "ext4",
        &[],
    )
    .unwrap();

    filesystem::remove_mount(CONFIG_DIRECTORY, "/mnt/Vault").unwrap();
}
