#![warn(clippy::all)]

use lazy_static::lazy_static;
use log::{debug, info, trace, warn};
use question::{Answer, Question};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use walkdir::{DirEntry, DirEntryExt, WalkDir};

#[derive(StructOpt, Debug)]
#[structopt(name = "dedupe")]
struct Opt {
    // The number of occurrences of the `v/verbose` flag
    /// Verbose mode (-v, -vv, -vvv, -vvvv)
    #[structopt(short, long, parse(from_occurrences))]
    verbose: usize,

    /// Perform hard links (if not present, will do a dry-run)
    #[structopt(long)]
    hardlink: bool,

    /// Minimum filesize in MB
    #[structopt(short, long, required = true)]
    min_filesize_mb: u64,

    /// Directories to process
    #[structopt(name = "directories", parse(from_os_str), required = true)]
    directories: Vec<PathBuf>,
}

fn main() {
    let opt = Opt::from_args();
    stderrlog::new()
        .module(module_path!())
        .verbosity(opt.verbose)
        .timestamp(stderrlog::Timestamp::Off)
        .init()
        .unwrap();
    ensure_conflicts_are_stopped().unwrap();

    let mut filesize_map: HashMap<u64, Vec<DirEntry>> = HashMap::new();
    let mut total_file_count = 0;
    for dir in opt.directories {
        let files = walk_directory(dir, opt.min_filesize_mb);
        for file in files {
            total_file_count += 1;
            let len = file.metadata().unwrap().len();
            let map_entry = filesize_map.entry(len).or_insert_with(|| vec![]);
            debug!(
                "Map has {} entries for size {}, adding {}",
                map_entry.len(),
                len,
                file.path().display()
            );
            map_entry.push(file);
        }
    }

    let mut total_dupe_size = 0;
    let duplicate_sizes = filesize_map.into_iter().filter_map(|(filesize, files)| {
        if files.len() < 2 {
            return None;
        }
        if files.windows(2).all(|w| w[0].ino() == w[1].ino()) {
            debug!(
                "All entries for filesize {:.0} MB have inode {}",
                filesize / 1024 / 1024,
                files[0].ino()
            );
            return None;
        }
        total_dupe_size += filesize;
        Some(files)
    });

    for files in duplicate_sizes {
        let mut danger = false;
        if files.len() > 2 {
            warn!("Multiple files here!");
            danger = true;
        }
        match generate_probable_episode(files[0].path()) {
            None => {
                let all_titles_match = files.windows(2).all(|w| {
                    generate_probable_name(w[0].path()) == generate_probable_name(w[1].path())
                });
                if !all_titles_match {
                    warn!("Differing titles guessed!");
                    danger = true;
                }
            }
            Some(_) => {
                let all_episodes_match = files.windows(2).all(|w| {
                    generate_probable_episode(w[0].path()) == generate_probable_episode(w[1].path())
                });
                if (!all_episodes_match)
                    && files.iter().all(|w| !is_paw_patrol_bar_rescue(w.path()))
                {
                    warn!("Differing episodes guessed!");
                    danger = true;
                }
            }
        };
        if danger {
            warn!("Skipping dedupe");
            for file in files {
                warn!("\t{}", file.path().display());
            }
        } else if opt.hardlink {
            hardlink(
                files
                    .into_iter()
                    .map(move |f| f.path().to_path_buf())
                    .collect(),
            );
        } else {
            info!("Likely duplicates: ");
            for file in files {
                info!("\t{}", file.path().display());
            }
        }
    }

    println!("Total files scanned: {}", total_file_count);
    println!(
        "Estimated space savings: {} GB",
        total_dupe_size / 1024 / 1024 / 1024
    );
}

fn hardlink(paths: Vec<PathBuf>) {
    let mut paths = paths.into_iter();
    let path = paths.next().unwrap();
    for other in paths {
        info!("Unlink  {}", other.display());
        fs::remove_file(other.clone()).unwrap();
        info!("Link to {}", path.display());
        fs::hard_link(path.clone(), other.clone()).unwrap();
    }
}

fn is_paw_patrol_bar_rescue(path: &Path) -> bool {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(?i)paw.patrol|bar.rescue").unwrap();
    }
    RE.is_match(path.file_stem().unwrap().to_str().unwrap())
}

fn ensure_conflicts_are_stopped() -> Result<(), &'static str> {
    let answer = Question::new("Are all writing programs stopped?")
        .default(Answer::NO)
        .show_defaults()
        .confirm();

    if answer == Answer::YES {
        Ok(())
    } else {
        Err("Stop all programs before continuing")
    }
}

fn generate_probable_name(path: &Path) -> String {
    let filename = path.file_stem().unwrap();
    let title_guess = filename
        .to_str()
        .unwrap()
        .split('.')
        .next()
        .unwrap()
        .split(" (")
        .next()
        .unwrap();
    trace!("Guessing {} for {}", title_guess, path.display());
    String::from(title_guess)
}

fn generate_probable_episode(path: &Path) -> Option<String> {
    let filename = path.file_stem().unwrap();
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(?i)s\d\de\d\d").unwrap();
    }
    let episode_guess = RE
        .captures(filename.to_str().unwrap())
        .map(|re_match| re_match[0].to_uppercase());
    trace!("Guessing {:?} for {}", episode_guess, path.display());
    episode_guess
}

fn walk_directory(path: PathBuf, min_filesize_mb: u64) -> impl Iterator<Item = DirEntry> {
    fn is_hidden(entry: &DirEntry) -> bool {
        entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with('.'))
            .unwrap_or(false)
    }
    debug!("Walking top-level directory {:?}", path);
    let walker = WalkDir::new(path).into_iter();
    let entries = walker.filter_entry(|e| !is_hidden(e));
    entries.filter_map(move |entry| {
        let entry = entry.unwrap();
        let metadata = entry.metadata().unwrap();
        if !metadata.is_file() {
            return None;
        }
        let filesize = metadata.len();
        if (filesize / 1024 / 1024) < min_filesize_mb {
            trace!(
                "Skipping small file with size {:.0} MB: {}",
                filesize / 1024 / 1024,
                entry.path().display()
            );
            return None;
        }
        trace!("Found file: {}", entry.path().display());
        Some(entry)
    })
}
