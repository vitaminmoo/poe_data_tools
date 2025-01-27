use anyhow::ensure;
use anyhow::Result;
use clap::{ArgGroup, Parser, Subcommand};
use glob::Pattern;
use poe_game_data_parser::{
    bundle_fs::{from_cdn, from_steam},
    bundle_loader::cdn_base_url,
};
use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::PathBuf,
};

#[derive(Debug, Clone)]
enum Patch {
    One,
    Two,
    Specific(String),
}

impl std::str::FromStr for Patch {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "1" => Ok(Patch::One),
            "2" => Ok(Patch::Two),
            _ => Ok(Patch::Specific(s.to_string())),
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List files
    List {
        /// Glob pattern to filter the list of files
        #[clap(default_value = "*")]
        glob: Pattern,
    },
    /// Extract matched files to a folder
    Extract {
        /// Path to the folder to output the extracted files
        output_folder: PathBuf,
        /// Glob pattern to filter the list of files
        #[clap(default_value = "*")]
        glob: Pattern,
    },
    /// Extract a single file to stdout
    Cat {
        /// Path to the file to extract
        path: String,
    },
}

/// A simple CLI tool that extracts the virtual filenames from PoE data files.
/// File paths are printed to stdout.
#[derive(Parser, Debug)]
#[command(
    name = "poe_files",
    group(
        ArgGroup::new("source")
        .args(&["steam", "cache_dir"])
        .required(false) // At least one is not required, but they are mutually exclusive
        .multiple(false) // Only one can be used at a time
    )
)]
struct Cli {
    /// Specify the patch version (1, 2, or specific_patch)
    #[arg(long, required = true)]
    patch: Patch,

    /// Specify the Steam folder path (optional)
    #[arg(long)]
    steam: Option<PathBuf>,

    /// Specify the cache directory (optional)
    #[arg(long)]
    cache_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug)]
enum Source {
    Cdn { cache_dir: PathBuf },
    Steam { steam_folder: PathBuf },
}

#[derive(Debug)]
struct Args {
    patch: Patch,
    source: Source,
    command: Command,
}

/// Validates user input and constructs a valid input state
fn parse_args() -> Result<Args> {
    let cli = Cli::parse();

    let source = if let Some(steam_folder) = cli.steam {
        ensure!(steam_folder.exists(), "Steam folder doesn't exist");
        Source::Steam { steam_folder }
    } else {
        let cache_dir = cli
            .cache_dir
            .unwrap_or_else(|| dirs::cache_dir().unwrap().join("poe_data_tools"));

        Source::Cdn { cache_dir }
    };

    if matches!(source, Source::Steam { .. }) {
        ensure!(
            !matches!(cli.patch, Patch::Specific { .. }),
            "When using steam, specific patch versions are not supported."
        );
    }

    Ok(Args {
        patch: cli.patch,
        source,
        command: cli.command,
    })
}

fn main() -> Result<()> {
    let args = parse_args()?;

    let mut fs = match args.source {
        Source::Cdn { cache_dir } => {
            let version_string = match args.patch {
                Patch::One => "1".to_string(),
                Patch::Two => "2".to_string(),
                Patch::Specific(v) => v,
            };
            from_cdn(&cdn_base_url(&version_string), &cache_dir)
        }
        Source::Steam { steam_folder } => from_steam(steam_folder),
    };

    match args.command {
        Command::List { glob } => {
            // Use a buffered writer since we're dumping a lot of data
            let stdout = io::stdout().lock();
            let mut out = BufWriter::new(stdout);

            fs.list().iter().filter(|p| glob.matches(p)).for_each(|p| {
                writeln!(out, "{}", p).expect("Failed to write to stdout");
            });

            out.flush().expect("Failed to flush stdout");
        }
        Command::Cat { path } => {
            let result = fs.read(path).expect("Failed to read file");
            let stdout = io::stdout().lock();
            let mut out = BufWriter::new(stdout);
            out.write_all(&result).expect("Failed to write to stdout");
            out.flush().expect("Failed to flush stdout");
        }
        Command::Extract {
            glob,
            output_folder,
        } => {
            fs.list().iter().filter(|p| glob.matches(p)).for_each(|p| {
                // Dump it to disk
                let contents = fs.read(p.to_string()).expect("Failed to read file");
                let out_filename = output_folder.as_path().join(p);
                fs::create_dir_all(out_filename.parent().unwrap())
                    .expect("Failed to create folder");
                let mut out_file = File::create(out_filename).expect("Failed to create file.");
                out_file
                    .write_all(&contents)
                    .expect("Failed to write to file.");
                eprintln!("{}", p);
            });
        }
    }

    Ok(())
}
