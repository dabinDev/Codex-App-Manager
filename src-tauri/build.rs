use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrandConfig {
    product_name: String,
    service_name: String,
    sites: BrandSites,
}

#[derive(Deserialize)]
struct BrandSites {
    support: String,
    service: String,
    mirror: String,
    skins: String,
}

fn export(name: &str, value: &str) {
    assert!(
        !value.trim().is_empty() && !value.contains(['\n', '\r', '\0']),
        "{name} must be a non-empty single-line value"
    );
    println!("cargo:rustc-env={name}={value}");
}

fn main() {
    let repo_root =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("..");
    let config_path = repo_root.join("brand.config.json");
    let env_path = repo_root.join(".env");
    println!("cargo:rerun-if-changed={}", config_path.display());
    println!("cargo:rerun-if-changed={}", env_path.display());
    println!("cargo:rerun-if-env-changed=BRAND_SERVICE_NAME");
    let _ = dotenvy::from_path(env_path);

    let raw = fs::read_to_string(&config_path).expect("read brand.config.json");
    let brand: BrandConfig = serde_json::from_str(&raw).expect("parse brand.config.json");
    let service_name = std::env::var("BRAND_SERVICE_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_owned())
        .unwrap_or(brand.service_name);

    export("BRAND_PRODUCT_NAME", &brand.product_name);
    export("BRAND_SERVICE_NAME", &service_name);
    export("BRAND_SUPPORT_URL", &brand.sites.support);
    export("BRAND_SERVICE_BASE_URL", &brand.sites.service);
    export("BRAND_MIRROR_BASE_URL", &brand.sites.mirror);
    export("BRAND_SKINS_BASE_URL", &brand.sites.skins);

    tauri_build::build()
}
