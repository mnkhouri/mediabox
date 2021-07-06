#![warn(clippy::all)]
use {
    anyhow::Error,
    blake2::{Blake2b, Digest},
    dialoguer::{theme::ColorfulTheme, Select},
    indicatif::{ProgressBar, ProgressStyle},
    lazy_static::lazy_static,
    log::{debug, error, info, trace, warn},
    question::{Answer, Question},
    regex::Regex,
    std::{
        collections::HashMap,
        convert::TryInto as _,
        fs, io,
        io::Read as _,
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

    println!("Walking directories to find all filesizes");
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("[{elapsed_precise}] {spinner} {wide_msg}")
            .progress_chars("#>-"),
    );
    let mut filesize_map: HashMap<u64, Vec<DirEntry>> = HashMap::new();
    let mut total_file_count = 0;
    for dir in opt.directories {
        let files = walk_directory(dir, opt.min_filesize_mb);
        for file in files {
            spinner.set_message(format!("{}", file.path().display()));
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
    spinner.finish_with_message(format!(
        "Found files with {} different sizes",
        filesize_map.len()
    ));

    println!("Filtering out irrelevant filesizes");
    let progress_bar = ProgressBar::new(filesize_map.len().try_into().unwrap());
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:.cyan/blue}] {pos}/{len} ({eta}) {wide_msg}")
            .progress_chars("#>-"),
    );
    let mut total_dupe_size = 0;
    let duplicate_sizes = filesize_map
        .into_iter()
        .filter_map(|(filesize, files)| {
            progress_bar.inc(1);
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
        })
        .collect::<Vec<Vec<DirEntry>>>();

    progress_bar.finish_with_message(format!(
        "{} filesizes have more than 1 file",
        duplicate_sizes.len()
    ));

    let mut files_to_hardlink = vec![];
    let mut files_for_manual_confirmation = vec![];

    println!("Examining files for duplicates");
    let progress_bar = ProgressBar::new(duplicate_sizes.len().try_into().unwrap());
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:.cyan/blue}] {pos}/{len} ({eta}) {wide_msg}")
            .progress_chars("#>-"),
    );
    for files in duplicate_sizes {
        progress_bar.inc(1);
        progress_bar.set_message(format!("{}", files[0].path().display()));
        match verify_duplicate(&files) {
            IsDuplicate::No => info!("Skipping dedupe due to file mismatch"),
            IsDuplicate::VeryLikely => {
                info!("Very likely duplicates: ");
                for file in files.iter() {
                    info!("\t{}", file.path().display());
                }
                files_to_hardlink.push(files);
            }
            IsDuplicate::Maybe => {
                info!("Maybe duplicates: ");
                for file in files.iter() {
                    info!("\t{}", file.path().display());
                }
                files_for_manual_confirmation.push(files);
            }
        }
    }
    progress_bar.finish_with_message(format!(
        "Very likely duplicates: {}; Questionable duplicates: {}",
        files_to_hardlink.len(),
        files_for_manual_confirmation.len()
    ));

    let mut files_to_hash = vec![];
    enum HashAmount {
        Full,
        HundredMB,
    }
    let mut idx = 0;
    let len = files_for_manual_confirmation.len();
    for files in files_for_manual_confirmation {
        println!(
            "What do you want to do with these potential duplicates? ({} / {})",
            idx, len
        );
        idx += 1;
        for file in files.iter() {
            println!("\t{}", file.path().display());
        }

        let options = vec!["skip", "hash 100MB", "hash full", "mark as dupe"];
        let selection = match Select::with_theme(&ColorfulTheme::default())
            .items(&options)
            .default(0)
            .interact_opt()
        {
            Ok(sel) => sel,
            Err(e) => {
                error!("Error getting input: {:?}", e);
                continue;
            }
        };
        match selection {
            Some(index) => match options[index] {
                "skip" => {}
                "hash 100MB" => files_to_hash.push((HashAmount::HundredMB, files)),
                "hash full" => files_to_hash.push((HashAmount::Full, files)),
                "mark as dupe" => files_to_hardlink.push(files),
                _ => error!("Unexpected input"),
            },
            None => println!("User did not select anything, skipping"),
        }
    }

    println!("Calculating partial hashes for requested files");
    let progress_bar = ProgressBar::new(files_to_hash.len().try_into().unwrap());
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:.cyan/blue}] {pos}/{len} ({eta}) {wide_msg}")
            .progress_chars("#>-"),
    );
    for (hash_amt, files) in files_to_hash {
        progress_bar.set_message(format!(
            "{}: {}",
            match hash_amt {
                HashAmount::Full => "full hash",
                HashAmount::HundredMB => "first 100MB",
            },
            files[0].path().display()
        ));
        info!("Calculating hashes for:");
        for file in files.iter() {
            info!("\t{}", file.path().display());
        }
        let all_hashes_match = files.windows(2).all(|w| match hash_amt {
            HashAmount::Full => full_hashes_match(w[0].path(), w[1].path()),
            HashAmount::HundredMB => partial_hashes_match(w[0].path(), w[1].path(), 100),
        });
        if all_hashes_match {
            files_to_hardlink.push(files);
        } else {
            warn!("Hashes differ!");
            progress_bar.println(format!("Hashes differed for {}", files[0].path().display()));
            for file in files.iter() {
                warn!("\t{}", file.path().display());
            }
        }
        progress_bar.inc(1);
    }
    progress_bar.finish_with_message(format!("Finished hashing files"));

    let answer = Question::new(&format!(
        "Are all writing programs stopped? Do you want to hardlink {} files?",
        files_to_hardlink.len()
    ))
    .yes_no()
    .until_acceptable()
    .confirm();

    if answer == Answer::YES {
        println!("Applying hardlinks");
        let progress_bar = ProgressBar::new(files_to_hardlink.len().try_into().unwrap());
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:.cyan/blue}] {pos}/{len} ({eta}) {wide_msg}")
                .progress_chars("#>-"),
        );
        for files in files_to_hardlink {
            progress_bar.inc(1);
            progress_bar.set_message(format!("{}", files[0].path().display()));
            hardlink(
                files
                    .into_iter()
                    .map(move |f| f.path().to_path_buf())
                    .collect(),
            );
        }
        progress_bar.finish_with_message(format!("Finished hardlinking files"));
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
enum IsDuplicate {
    VeryLikely,
    Maybe,
    No,
}

fn verify_duplicate(files: &[DirEntry]) -> IsDuplicate {
    let mut guessed_metadata_differs = false;

    if let Some(_air_date) = generate_probable_air_date(files[0].path()) {
        let all_dates_match = files.windows(2).all(|w| {
            generate_probable_air_date(w[0].path()) == generate_probable_air_date(w[1].path())
        });
        if !all_dates_match {
            debug!("Differing air dates guessed!");
            for file in files {
                debug!("\t{:?}", generate_probable_air_date(file.path()));
                debug!("\t\t{}", file.path().display());
            }
            guessed_metadata_differs = true;
        }
    } else if let Some(_episode) = generate_probable_episode(files[0].path()) {
        let all_episodes_match = files.windows(2).all(|w| {
            generate_probable_episode(w[0].path()) == generate_probable_episode(w[1].path())
        });
        if (!all_episodes_match) && files.iter().all(|w| !is_paw_patrol_bar_rescue(w.path())) {
            debug!("Differing episodes guessed!");
            for file in files {
                debug!("\t{:?}", generate_probable_episode(file.path()));
                debug!("\t\t{:?}", file.path().display());
            }
            guessed_metadata_differs = true;
        }
    } else {
        let all_titles_match = files
            .windows(2)
            .all(|w| generate_probable_name(w[0].path()) == generate_probable_name(w[1].path()));
        if !all_titles_match {
            debug!("Differing titles guessed!");
            for file in files {
                debug!("\t{}", generate_probable_name(file.path()));
                debug!("\t\t{}", file.path().display());
            }
            guessed_metadata_differs = true;
        }
    }

    if files.len() > 2 {
        debug!("More than 2 files at this size!");
        for file in files {
            debug!("\t\t{}", file.path().display());
        }
    }

    // Always check the partial hashes for 1MB, it's cheap
    match files
        .windows(2)
        .all(|w| partial_hashes_match(w[0].path(), w[1].path(), 1))
    {
        true => {
            if guessed_metadata_differs {
                // Check the first 10MB, that should get us past any false positive
                match files
                    .windows(2)
                    .all(|w| partial_hashes_match(w[0].path(), w[1].path(), 10))
                {
                    true => IsDuplicate::Maybe,
                    false => IsDuplicate::No,
                }
            } else {
                IsDuplicate::VeryLikely
            }
        }
        false => {
            if !guessed_metadata_differs {
                warn!("Didn't detect differing titles");
                for file in files {
                    warn!("\t\t{}", file.path().display());
                }
            }
            IsDuplicate::No
        }
    }
}

fn partial_hashes_match(path1: &Path, path2: &Path, megabytes: usize) -> bool {
    if let Ok(partial_hash1) = generate_partial_hash(path1, megabytes) {
        if let Ok(partial_hash2) = generate_partial_hash(path2, megabytes) {
            return partial_hash1 == partial_hash2;
        }
    }
    return false;
}

fn full_hashes_match(path1: &Path, path2: &Path) -> bool {
    if let Ok(hash1) = generate_full_hash(path1) {
        if let Ok(hash2) = generate_full_hash(path2) {
            return hash1 == hash2;
        }
    }
    return false;
}

fn generate_hash(mut reader: &mut impl io::Read) -> Result<Vec<u8>, Error> {
    let mut hasher = Blake2b::new();
    let _n = io::copy(&mut reader, &mut hasher)?;
    let hash = hasher.result();
    Ok(hash.to_vec())
}

fn generate_partial_hash(path: &Path, megabytes: usize) -> Result<Vec<u8>, Error> {
    const ONE_MEGABYTE: usize = 1024 * 1024;
    let mut file = fs::File::open(&path)?;
    let mut buffer = vec![0; ONE_MEGABYTE * megabytes];
    file.read(&mut buffer)?;
    generate_hash(&mut &buffer[..])
}

fn generate_full_hash(path: &Path) -> Result<Vec<u8>, Error> {
    let mut file = fs::File::open(&path)?;
    generate_hash(&mut file)
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
