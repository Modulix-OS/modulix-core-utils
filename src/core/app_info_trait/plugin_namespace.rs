/// nixpkgs attribute-set prefixes that hold plugin packages rather than
/// standalone applications.
///
/// These are hidden from package search: every plugin set is surfaced through a
/// dedicated module (`mx.<module>.plugins`) instead of appearing as an
/// installable package.
pub static PLUGIN_NAMESPACE_PREFIXES: &[&str] = &["vscode-extensions", "obs-studio-plugins"];
