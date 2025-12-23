use std::time::Instant;

use poe_data_tools::{bundle_fs::FS, bundle_loader::cdn_base_url};
use tempfile::tempdir;

#[test]
fn fetch_file_perf_cas() {
    // Setup temporary cache directory
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let cache_dir = temp_dir.path();
    println!("Temporary cache dir: {:?}", cache_dir);

    // Phase 1: Setup FS (using CDN)
    let base_url = cdn_base_url(cache_dir, "2").expect("Failed to resolve CDN URL");
    let fs = FS::from_cdn(&base_url, cache_dir).expect("Failed to init FS");

    let file_path = "data/balance/mods.datc64";

    // Phase 2: Read File (COLD) - Fetches bundle, extracts, writes to CAS
    let start_cold = Instant::now();
    let content_cold = fs.read(file_path).expect("Failed to read file (cold)");
    let duration_cold = start_cold.elapsed();
    println!("Phase 2: Read File (COLD) took {:?}", duration_cold);

    // Phase 3: Read File (WARM) - Should hit CAS
    let start_warm = Instant::now();
    let content_warm = fs.read(file_path).expect("Failed to read file (warm)");
    let duration_warm = start_warm.elapsed();
    println!("Phase 3: Read File (WARM) took {:?}", duration_warm);

    // Verify consistency
    assert_eq!(content_cold.len(), content_warm.len());
    assert_eq!(content_cold, content_warm);

    if duration_warm.as_secs_f64() > 0.0 {
        println!(
            "Speedup: {:.2}x",
            duration_cold.as_secs_f64() / duration_warm.as_secs_f64()
        );
    } else {
        println!("Speedup: Infinite (warm took 0s)");
    }
}
