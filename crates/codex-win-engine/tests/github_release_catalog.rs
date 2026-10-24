//! Opt-in network smoke for the same large response used by the version picker.
#[test]
#[ignore = "requires access to the public GitHub API"]
fn real_github_release_catalog_fits_the_capture_budget() {
    let started = std::time::Instant::now();
    let text = codex_win_engine::fetch_text(
        "https://api.github.com/repos/Wangnov/codex-app-mirror/releases?per_page=100",
    )
    .expect("fetch the version picker's first page without a pipe deadlock");
    let releases: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    assert!(!releases.is_empty());
    assert!(releases[0]["tag_name"].as_str().is_some());
    eprintln!(
        "GitHub catalog: {} releases, {} bytes, {:?}",
        releases.len(),
        text.len(),
        started.elapsed()
    );
}
