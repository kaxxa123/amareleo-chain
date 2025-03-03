use amareleo_chain::{BuildInfo, main_core};

// Obtain information on the build.
include!(concat!(env!("OUT_DIR"), "/built.rs"));

fn main() -> anyhow::Result<()> {
    let pkg_name = env!("CARGO_PKG_NAME");
    let mut features = FEATURES_LOWERCASE_STR.to_owned();
    features.retain(|c| c != ' ');

    let build = BuildInfo {
        bin: pkg_name,
        version: PKG_VERSION,
        repo: pkg_name,
        branch: GIT_HEAD_REF.unwrap_or("unknown_branch"),
        commit: GIT_COMMIT_HASH.unwrap_or("unknown_commit"),
        features: &features,
    };

    main_core(&build)
}
