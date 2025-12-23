use std::time::Instant;

use poe_data_tools::{bundle_index::fetch_index_file, bundle_loader::cdn_base_url};
use tempfile::tempdir;

#[test]
fn fetch_bundle_index_perf() {
    // Setup temporary cache directory
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let cache_dir = temp_dir.path();
    println!("Temporary cache dir: {:?}", cache_dir);

    // Phase 1: Resolve CDN URL
    let base_url = cdn_base_url(cache_dir, "2").expect("Failed to resolve CDN URL");
    let path = std::path::Path::new("Bundles2/_.index.bin");

    // Phase 2: Fetch and Parse Index File (COLD)
    let start_cold = Instant::now();
    let index_cold =
        fetch_index_file(&base_url, cache_dir, path).expect("Failed to fetch index file (cold)");
    let duration_cold = start_cold.elapsed();
    println!(
        "Phase 2: Fetch and Parse Index File (COLD) took {:?}",
        duration_cold
    );

    // Phase 3: Fetch and Parse Index File (WARM - Parsed Cache)
    let start_warm = Instant::now();
    let index_warm =
        fetch_index_file(&base_url, cache_dir, path).expect("Failed to fetch index file (warm)");
    let duration_warm = start_warm.elapsed();
    println!(
        "Phase 3: Fetch and Parse Index File (WARM) took {:?}",
        duration_warm
    );

    // Verify consistency
    assert_eq!(index_cold.bundles.len(), index_warm.bundles.len());
    assert_eq!(index_cold.files.len(), index_warm.files.len());

    if duration_warm.as_secs_f64() > 0.0 {
        println!(
            "Speedup: {:.2}x",
            duration_cold.as_secs_f64() / duration_warm.as_secs_f64()
        );
    } else {
        println!("Speedup: Infinite (warm took 0s)");
    }
}
