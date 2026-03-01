use modulix_core_utils::user;

fn main() {
    // user::add(
    //     "modulix",
    //     "1234",
    //     "Modulix OS",
    //     "${pkgs.zsh}/bin/zsh",
    //     &["wheel"],
    //     true,
    // )
    // .unwrap();
    user::remove("modulix").unwrap();
}
