use std::{
    collections::HashMap,
    fs,
    hash::{BuildHasher, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use iterators_extended::bucket::Bucket;
use url::Url;

use crate::{
    bundle::{fetch_bundle_content, load_bundle_content},
    bundle_index::{fetch_index_file, load_index_file, BundleIndex},
    hasher::BuildMurmurHash64A,
    path::parse_paths,
};

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub path: String,
    pub path_hash: u64,
    pub bundle_index: u32,
    pub bundle_name: String,
    pub offset: u32,
    pub size: u32,
}

pub struct FS {
    index: BundleIndex,
    source_hash: u64,
    lut: HashMap<u64, usize>,
    steam_folder: Option<PathBuf>,
    base_url: Option<Url>,
    cache_dir: Option<PathBuf>,
}

impl FS {
    /// Initialise a file system over a steam folder
    pub fn from_steam(steam_folder: PathBuf, cache_dir: Option<PathBuf>) -> Result<FS> {
        let index_path = steam_folder.as_path().join("Bundles2/_.index.bin");
        let index = load_index_file(&index_path).context("Failed to load bundle index")?;

        // Hash the steam folder path to create a stable ID for this installation
        let mut hasher = BuildMurmurHash64A { seed: 0x1337b33f }.build_hasher();
        hasher.write(steam_folder.to_string_lossy().as_bytes());
        let source_hash = hasher.finish();

        let lut = index
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.hash, i))
            .collect();

        Ok(FS {
            index,
            source_hash,
            lut,
            steam_folder: Some(steam_folder.clone()),
            base_url: None,
            cache_dir,
        })
    }

    /// Initialise a file system using the CDN background
    pub fn from_cdn(base_url: &Url, cache_dir: &Path) -> Result<FS> {
        let index = fetch_index_file(
            base_url,
            cache_dir,
            PathBuf::from("Bundles2/_.index.bin").as_ref(),
        )
        .context("Failed to load bundle index")?;

        // Hash the base URL to create a stable ID for this CDN source
        let mut hasher = BuildMurmurHash64A { seed: 0x1337b33f }.build_hasher();
        hasher.write(base_url.as_str().as_bytes());
        let source_hash = hasher.finish();

        let lut = index
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.hash, i))
            .collect();

        Ok(FS {
            index,
            source_hash,
            lut,
            steam_folder: None,
            base_url: Some(base_url.clone()),
            cache_dir: Some(cache_dir.to_path_buf()),
        })
    }

    fn get_cache_path(&self, metadata: &FileMetadata) -> Option<PathBuf> {
        self.cache_dir.as_ref().map(|dir| {
            // Namespace by source_hash (PoE1 vs PoE2 separation based on install path/URL)
            // and bundle_name + offset + size (Content Identity)
            // This allows reusing cache across versions if the file hasn't moved/changed.
            dir.join("extracted")
                .join(format!("{:x}", self.source_hash))
                .join(&metadata.bundle_name)
                .join(format!("{}_{}", metadata.offset, metadata.size))
                .join(&metadata.path)
        })
    }

    /// Lists all paths in the index
    pub fn list(&self) -> impl Iterator<Item = String> + '_ {
        self.index
            .paths
            .iter()
            .flat_map(|p| parse_paths(&self.index.path_rep_bundle, p).get_paths())
    }

    /// Lists all files with their metadata
    pub fn list_files(&self) -> impl Iterator<Item = FileMetadata> + '_ {
        let hash_builder = BuildMurmurHash64A { seed: 0x1337b33f };

        self.index.paths.iter().flat_map(move |p| {
            parse_paths(&self.index.path_rep_bundle, p)
                .get_paths()
                .into_iter()
                .filter_map(move |path| {
                    let mut hasher = hash_builder.build_hasher();
                    hasher.write(path.to_lowercase().as_bytes());
                    let hash = hasher.finish();

                    let index = self.lut.get(&hash)?;
                    let file_info = &self.index.files[*index];
                    let bundle_name = &self.index.bundles[file_info.bundle_index as usize].name;

                    Some(FileMetadata {
                        path,
                        path_hash: hash,
                        bundle_index: file_info.bundle_index,
                        bundle_name: bundle_name.clone(),
                        offset: file_info.offset,
                        size: file_info.size,
                    })
                })
        })
    }

    /// Read many files at once, optimising batch loads. Does not preserve order of paths given.
    pub fn batch_read<'a>(
        &'a self,
        paths: &[&'a str],
    ) -> impl Iterator<Item = Result<(&'a str, Bytes), (&'a str, anyhow::Error)>> {
        // Get FileInfo's
        let hash_builder = BuildMurmurHash64A { seed: 0x1337b33f };
        let (fileinfos, errors) = paths
            .iter()
            .map(|&path| {
                // Compute hash
                let mut hasher = hash_builder.build_hasher();
                hasher.write(path.to_lowercase().as_bytes());
                let hash = hasher.finish();

                // Look up the file info for this file
                let fileinfo = self
                    .lut
                    .get(&hash)
                    .map(|i| &self.index.files[*i])
                    .with_context(|| format!("Path not found in index: {}", path))
                    .map(|f| {
                        let bundle_name = &self.index.bundles[f.bundle_index as usize].name;
                        (path, f, bundle_name)
                    })
                    .map_err(|e| (path, e));

                fileinfo
            })
            .bucket_result();

        // Batch them into their bundles
        let fileinfos = fileinfos.into_iter().fold(
            HashMap::<_, Vec<_>>::new(),
            |mut acc, (path, fileinfo, bundle_name)| {
                acc.entry(fileinfo.bundle_index)
                    .or_default()
                    .push((path, fileinfo, bundle_name));

                acc
            },
        );

        // Process files bundle-wise
        let file_contents = fileinfos.into_iter().flat_map(|(bundle_index, files)| {
            // Load the bundle
            let bundle_name = &self.index.bundles[bundle_index as usize].name;
            let bundle_path_str = format!("Bundles2/{}.bundle.bin", bundle_name);

            let bundle = if let Some(steam_folder) = &self.steam_folder {
                let bundle_path = steam_folder.join(&bundle_path_str);
                load_bundle_content(&bundle_path)
                    .with_context(|| format!("Failed to load bundle file: {:?}", bundle_path))
            } else {
                let bundle_path = PathBuf::from(&bundle_path_str);
                fetch_bundle_content(
                    self.base_url.as_ref().unwrap(),
                    self.cache_dir.as_ref().unwrap(),
                    &bundle_path,
                )
                .with_context(|| format!("Failed to fetch bundle file: {:?}", bundle_path))
            };

            // Read the file contents
            let contents: Vec<_> = match bundle {
                Ok(b) => files
                    .into_iter()
                    .map(|(path, file, bundle_name)| {
                        let content = b.read_range(file.offset as usize, file.size as usize);

                        // Write to cache in batch read too
                        if let Some(cache_path) = self.get_cache_path(&FileMetadata {
                            path: path.to_string(),
                            path_hash: 0, // Not used for cache path
                            bundle_index,
                            bundle_name: bundle_name.clone(),
                            offset: file.offset,
                            size: file.size,
                        }) {
                            if !cache_path.exists() {
                                if let Some(parent) = cache_path.parent() {
                                    let _ = fs::create_dir_all(parent);
                                }
                                let _ = fs::write(&cache_path, &content);
                            }
                        }

                        Ok((path, content))
                    })
                    .collect(),
                Err(e) => files
                    .into_iter()
                    .map(|(path, _, _)| Err((path, anyhow!("{:?}", e))))
                    .collect(),
            };

            contents
        });

        // Add on previous errors
        errors.into_iter().map(Err).chain(file_contents)
    }

    pub fn read(&self, path: &str) -> Result<Bytes> {
        // Compute the hash of this file path
        let hash_builder = BuildMurmurHash64A { seed: 0x1337b33f };
        let mut hasher = hash_builder.build_hasher();
        hasher.write(path.to_lowercase().as_bytes());
        let hash = hasher.finish();

        // Look up the file info for this file
        let index = self
            .lut
            .get(&hash)
            .with_context(|| format!("Path not found in index: {}", path))?;
        let file = &self.index.files[*index];
        let bundle_name = &self.index.bundles[file.bundle_index as usize].name;

        let metadata = FileMetadata {
            path: path.to_string(),
            path_hash: hash,
            bundle_index: file.bundle_index,
            bundle_name: bundle_name.clone(),
            offset: file.offset,
            size: file.size,
        };

        // Check internal cache
        if let Some(cache_path) = self.get_cache_path(&metadata) {
            if cache_path.exists() {
                return fs::read(&cache_path)
                    .map(Bytes::from)
                    .context("Failed to read from cache");
            }
        }

        // Load the bundle
        let bundle = if let Some(steam_folder) = &self.steam_folder {
            let bundle_path = steam_folder.join(format!("Bundles2/{}.bundle.bin", bundle_name));
            load_bundle_content(&bundle_path)
                .with_context(|| format!("Failed to load bundle file: {:?}", bundle_path))?
        } else {
            let bundle_path = PathBuf::from(format!("Bundles2/{}.bundle.bin", bundle_name));
            fetch_bundle_content(
                self.base_url.as_ref().unwrap(),
                self.cache_dir.as_ref().unwrap(),
                &bundle_path,
            )
            .with_context(|| format!("Failed to fetch bundle file: {:?}", bundle_path))?
        };

        // Pull out the file's contents
        let content = bundle.read_range(file.offset as usize, file.size as usize);

        // Write to internal cache
        if let Some(cache_path) = self.get_cache_path(&metadata) {
            if let Some(parent) = cache_path.parent() {
                fs::create_dir_all(parent).context("Failed to create cache directory")?;
            }
            fs::write(&cache_path, &content).context("Failed to write to cache")?;
        }

        Ok(content)
    }
}
