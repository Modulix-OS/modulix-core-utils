use modulix_core_utils::{CONFIG_DIRECTORY, user};

fn main() {
    user::add(
        CONFIG_DIRECTORY,
        "modulix",
        "1234",
        "Modulix OS",
        "${pkgs.bash}/bin/bash",
        &["wheel"],
        true,
    )
    .unwrap();
}
