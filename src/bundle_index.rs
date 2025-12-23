use std::{
    fs,
    hash::{BuildHasher, Hasher},
    path::Path,
};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use nom::{
    bytes::complete::take,
    combinator::rest,
    multi::count,
    number::complete::{le_u32, le_u64},
    IResult,
};
use url::Url;

use crate::{
    bundle::{fetch_bundle_content, load_bundle_content, parse_bundle},
    hasher::BuildMurmurHash64A,
};

#[derive(Debug)]
pub struct BundleInfo {
    pub name: String,
    pub uncompressed_size: u32,
}

#[derive(Debug)]
pub struct FileInfo {
    pub hash: u64,
    pub bundle_index: u32,
    pub offset: u32,
    pub size: u32,
}

#[derive(Debug)]
pub struct PathRep {
    pub hash: u64,
    pub offset: u32,
    pub size: u32,
    pub recursive_size: u32,
}

#[derive(Debug)]
pub struct BundleIndex {
    pub bundles: Vec<BundleInfo>,
    pub files: Vec<FileInfo>,
    pub paths: Vec<PathRep>,
    pub path_rep_bundle: Bytes,
}

// Parser for a UTF-8 string of given length
fn parse_string(input: &[u8], length: u32) -> IResult<&[u8], String> {
    let (input, data) = take(length)(input)?;
    let string = String::from_utf8_lossy(data).to_string();
    Ok((input, string))
}

// Parser for a Bundle
fn parse_bundle_info(input: &[u8]) -> IResult<&[u8], BundleInfo> {
    let (input, name_length) = le_u32(input)?;
    let (input, name) = parse_string(input, name_length)?;
    let (input, uncompressed_size) = le_u32(input)?;
    Ok((
        input,
        BundleInfo {
            name,
            uncompressed_size,
        },
    ))
}

// Parser for a vector of Bundles
fn parse_bundles(input: &[u8]) -> IResult<&[u8], Vec<BundleInfo>> {
    let (input, bundle_count) = le_u32(input)?;
    count(parse_bundle_info, bundle_count as usize)(input)
}

// Parser for a FileInfo
fn parse_file_info(input: &[u8]) -> IResult<&[u8], FileInfo> {
    let (input, hash) = le_u64(input)?;
    let (input, bundle_index) = le_u32(input)?;
    let (input, offset) = le_u32(input)?;
    let (input, size) = le_u32(input)?;
    Ok((
        input,
        FileInfo {
            hash,
            bundle_index,
            offset,
            size,
        },
    ))
}

// Parser for a vector of FileInfo
fn parse_file_infos(input: &[u8]) -> IResult<&[u8], Vec<FileInfo>> {
    let (input, file_count) = le_u32(input)?;
    count(parse_file_info, file_count as usize)(input)
}

// Parser for a PathRep
fn parse_path_rep(input: &[u8]) -> IResult<&[u8], PathRep> {
    let (input, hash) = le_u64(input)?;
    let (input, offset) = le_u32(input)?;
    let (input, size) = le_u32(input)?;
    let (input, recursive_size) = le_u32(input)?;
    Ok((
        input,
        PathRep {
            hash,
            offset,
            size,
            recursive_size,
        },
    ))
}

// Parser for a vector of PathRep
fn parse_path_reps(input: &[u8]) -> IResult<&[u8], Vec<PathRep>> {
    let (input, path_count) = le_u32(input)?;
    count(parse_path_rep, path_count as usize)(input)
}

// Parser for the entire BundleIndex
pub fn parse_bundle_index(input: &[u8]) -> IResult<&[u8], BundleIndex> {
    let (input, bundles) = parse_bundles(input)?;
    let (input, files) = parse_file_infos(input)?;
    let (input, paths) = parse_path_reps(input)?;
    let (input, path_rep_bundle) = rest(input)?;
    let (_, path_rep_bundle) = parse_bundle(path_rep_bundle)?;

    Ok((
        input,
        BundleIndex {
            bundles,
            files,
            paths,
            path_rep_bundle: path_rep_bundle.read_all(),
        },
    ))
}

/// Load an index file from disk
pub fn load_index_file(path: &Path, cache_dir: Option<&Path>) -> Result<BundleIndex> {
    if let Some(cache_dir) = cache_dir {
        // Create a stable hash of the source path to use as the cache filename
        let mut hasher = BuildMurmurHash64A { seed: 0x1337b33f }.build_hasher();
        hasher.write(path.to_string_lossy().as_bytes());
        let hash = hasher.finish();

        let cache_path = cache_dir
            .join("decompressed_index")
            .join(format!("steam_{:x}.bin", hash));

        // Check if cache is valid (exists and is newer than source)
        let mut valid_cache = false;
        if cache_path.exists() {
            let source_metadata =
                fs::metadata(path).context("Failed to get source file metadata")?;
            let cache_metadata =
                fs::metadata(&cache_path).context("Failed to get cache file metadata")?;

            if cache_metadata.modified()? >= source_metadata.modified()? {
                valid_cache = true;
            }
        }

        if valid_cache {
            // eprintln!("Loading decompressed index from cache: {:?}", cache_path);
            let index_content = fs::read(&cache_path)?;
            let (_, index) = parse_bundle_index(&index_content)
                .map_err(|_| anyhow!("Failed to parse cached bundle index"))?;
            return Ok(index);
        }

        // Cache miss or stale
        let index_content = load_bundle_content(path)
            .context("Failed to read bundle index")?
            .read_all();

        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&cache_path, &index_content)?;

        let (_, index) = parse_bundle_index(&index_content)
            .map_err(|_| anyhow!("Failed to parse bundle as index"))?;
        Ok(index)
    } else {
        let index_content = load_bundle_content(path)
            .context("Failed to read bundle index")?
            .read_all();
        let (_, index) = parse_bundle_index(&index_content)
            .map_err(|_| anyhow!("Failed to parse bundle as index"))?;
        Ok(index)
    }
}

/// Fetch an index file from the CDN (or cache)
pub fn fetch_index_file(base_url: &Url, cache_dir: &Path, path: &Path) -> Result<BundleIndex> {
    let url = base_url.join(path.to_str().context("Failed to convert path to string")?)?;

    // Construct cache path for decompressed index
    let url_str = url.to_string();
    let relative_path = url_str
        .trim_start_matches("https://")
        .trim_start_matches("http://");

    let decompressed_cache_path = cache_dir.join("decompressed_index").join(relative_path);

    if decompressed_cache_path.exists() {
        let index_content = fs::read(&decompressed_cache_path)?;
        let (_, index) = parse_bundle_index(&index_content)
            .map_err(|_| anyhow!("Failed to parse cached bundle index"))?;
        return Ok(index);
    }

    let index_content = fetch_bundle_content(base_url, cache_dir, path)
        .context("Failed to fetch bundle index")?
        .read_all();

    // Cache the decompressed content
    if let Some(parent) = decompressed_cache_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&decompressed_cache_path, &index_content)?;

    let (_, index) = parse_bundle_index(&index_content)
        .map_err(|_| anyhow!("Failed to parse bundle as index"))?;
    Ok(index)
}
