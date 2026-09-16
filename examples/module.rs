use modulix_core_utils::{CONFIG_DIRECTORY, install_module};

#[tokio::main]
async fn main() {
    install_module::install_plugin(
        CONFIG_DIRECTORY,
        "programs.obs-studio",
        "obs-studio-plugins",
        "obs-tuna",
    )
    .await
    .unwrap();
    install_module::uninstall(CONFIG_DIRECTORY, "programs.games.steam")
        .await
        .unwrap();
}
