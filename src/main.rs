use std::path::PathBuf;

use anyhow::ensure;
use clap::Parser;
use nautica_downloader_rs::Downloader;

/// Downloads songs from Nautica (ksm.dev)
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Destination directory
    #[arg(default_value = PathBuf::from("./nautica").into_os_string())]
    dest: PathBuf,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    ensure!(
        args.dest.exists(),
        "Destination directory must exist: {}",
        args.dest.to_string_lossy()
    );

    let downloader = Downloader::builder().dest(args.dest).build();
    downloader.download_all()?;
    Ok(())
}
