#![warn(clippy::all)]
use {
    anyhow::Error,
    blake2::{Blake2b, Digest},
    lazy_static::lazy_static,
    log::{debug, info, trace, warn},
    question::{Answer, Question},
    regex::Regex,
    std::{
        collections::HashMap,
        fs, io,
        path::{Path, PathBuf},
    },
    structopt::StructOpt,
    walkdir::{DirEntry, DirEntryExt, WalkDir},
};

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

    /// Hash uncertain files (if not present, will skip these files)
    #[structopt(long)]
    hash: bool,

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
        if let Ok(_is_duplicate) = verify_duplicate(&files, opt.hash) {
            if opt.hardlink {
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
        } else {
            info!("Skipping dedupe due to file mismatch");
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

fn verify_duplicate(files: &[DirEntry], do_expensive_check: bool) -> Result<(), ()> {
    let mut danger = false;

    if let Some(_air_date) = generate_probable_air_date(files[0].path()) {
        let all_dates_match = files.windows(2).all(|w| {
            generate_probable_air_date(w[0].path()) == generate_probable_air_date(w[1].path())
        });
        if !all_dates_match {
            warn!("Differing air dates guessed!");
            for file in files {
                warn!("\t{:?}", generate_probable_air_date(file.path()));
                warn!("\t\t{}", file.path().display());
            }
            danger = true;
        }
    } else if let Some(_episode) = generate_probable_episode(files[0].path()) {
        let all_episodes_match = files.windows(2).all(|w| {
            generate_probable_episode(w[0].path()) == generate_probable_episode(w[1].path())
        });
        if (!all_episodes_match) && files.iter().all(|w| !is_paw_patrol_bar_rescue(w.path())) {
            warn!("Differing episodes guessed!");
            for file in files {
                warn!("\t{:?}", generate_probable_episode(file.path()));
                warn!("\t\t{:?}", file.path().display());
            }
            danger = true;
        }
    } else {
        let all_titles_match = files
            .windows(2)
            .all(|w| generate_probable_name(w[0].path()) == generate_probable_name(w[1].path()));
        if !all_titles_match {
            warn!("Differing titles guessed!");
            for file in files {
                warn!("\t{}", generate_probable_name(file.path()));
                warn!("\t\t{}", file.path().display());
            }
            danger = true;
        }
    }

    if files.len() > 2 {
        warn!("More than 2 files at this size!");
        for file in files {
            warn!("\t\t{}", file.path().display());
        }
        danger = true;
    }

    // If we have danger, check the hash
    if danger {
        if !do_expensive_check {
            return Err(());
        }
        warn!("Need to calculate hash:");
        let all_hashes_match = files.windows(2).all(|w| {
            if let Ok(hash1) = generate_hash(w[0].path()) {
                if let Ok(hash2) = generate_hash(w[1].path()) {
                    return hash1 == hash2;
                }
            }
            false
        });
        if !all_hashes_match {
            warn!("Hashes differ!");
            return Err(());
        }
    }

    Ok(())
}

fn generate_hash(path: &Path) -> Result<Vec<u8>, Error> {
    let mut file = fs::File::open(&path)?;
    let mut hasher = Blake2b::new();
    let _n = io::copy(&mut file, &mut hasher)?;
    let hash = hasher.result();
    Ok(hash.to_vec())
}

fn generate_probable_name(path: &Path) -> String {
    let filename = path.file_stem().unwrap();
    let title_guess = filename.to_str().unwrap();
    let title_guess = title_guess.replace(".", " "); // convert all periods to spaces
    let title_guess = title_guess.replace("cls", ""); // stupid release group
    let title_guess = title_guess.replace("-", " "); // remove all punctuation
    let title_guess = title_guess.replace(",", " "); // remove all punctuation
    let title_guess = title_guess.replace("!", " "); // remove all punctuation
    let title_guess = title_guess.replace("  ", " "); // remove all duplicate spaces
    let title_guess = title_guess.split(" (").next().unwrap(); // only keep up to the first "(2019)"
    let title_guess = title_guess.split(char::is_numeric).next().unwrap(); // only keep up to the first number
    let title_guess = title_guess.to_ascii_lowercase();
    let title_guess = title_guess.trim();
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

fn generate_probable_air_date(path: &Path) -> Option<String> {
    let filename = path.file_stem().unwrap();
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(\d\d\d\d)[.-](\d\d)[.-](\d\d)").unwrap();
    }
    let date_guess = RE
        .captures(filename.to_str().unwrap())
        .map(|re_match| "".to_owned() + &re_match[1] + &re_match[2] + &re_match[3]);
    trace!("Guessing {:?} for {}", date_guess, path.display());
    date_guess
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
