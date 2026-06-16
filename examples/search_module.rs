use modulix_core_utils::{AppInfoGui, AppInfoMinimal, module_info::ModuleInfo};

#[tokio::main]
async fn main() {
    let res = ModuleInfo::search("steam", 5).await;

    for module in res.unwrap_or_default() {
        // Resolve the remote GUI source so `icon`/`keyword` return data.
        module.resolve().await;
        println!(
            "module = {}\nname = {}\nid = {}\nicon = {}\nsummary = {}\ndescription = {}\nkeywords = {:?}\n\n",
            module.package_name(),
            module.display_name(),
            module.id().unwrap_or("None"),
            module.icon().unwrap_or("None"),
            module.summary(),
            module.description().await,
            module.keyword(),
        );
    }
}
