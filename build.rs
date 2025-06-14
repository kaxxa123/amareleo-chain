use std::{
    env,
    fs::{self},
    path::Path,
    process,
};
use toml::Value;

fn main() {
    // Check if locktick feature is correctly enabled.
    let locktick_enabled = env::var("CARGO_FEATURE_LOCKTICK").is_ok();
    if locktick_enabled {
        let profile = env::var("PROFILE").unwrap_or_else(|_| "".to_string());
        let manifest = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("Cargo.toml");
        let contents = fs::read_to_string(&manifest).expect("failed to read Cargo.toml");
        let doc = contents.parse::<Value>().expect("invalid TOML in Cargo.toml");

        let profile_table = doc.get("profile").and_then(|p| p.get(profile));
        if let Some(Value::Table(profile_settings)) = profile_table {
            if let Some(debug) = profile_settings.get("debug") {
                match debug {
                    Value::String(s) if s == "line-tables-only" => {
                        println!("cargo:info=manifest has debuginfo=line-tables-only");
                    }
                    _ => {
                        eprintln!(
                            "🔴 When enabling the locktick feature, the profile must have debug set to line-tables-only. Uncomment the relevant lines in Cargo.toml."
                        );
                        process::exit(1);
                    }
                }
            } else {
                eprintln!(
                    "🔴 When enabling the locktick feature, the profile must have debug set to line-tables-only. Uncomment the relevant lines in Cargo.toml."
                );
                process::exit(1);
            }
        }
    }

    // Register build-time information.
    built::write_built_file().expect("Failed to acquire build-time information");
}
