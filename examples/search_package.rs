use modulix_core_utils::{AppInfoGui, AppInfoMinimal, package_info::NixPackage};

#[tokio::main]
async fn main() {
    let res = NixPackage::search("firefox", 2).await;

    for pkg in res.unwrap_or_default() {
        println!(
            "Pkg name = {}\nname = {}\nicon = {}\nsummary = {}\ndescription = {}\nscreenshot = {:#?}\nMain program = {}\n\n",
            pkg.package_name(),
            pkg.display_name(),
            pkg.icon().unwrap_or("None"),
            pkg.summary(),
            pkg.description().await,
            pkg.screenshots().await,
            pkg.main_program().await.unwrap(),
        );
    }
}
