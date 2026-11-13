use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::quiz::QuestionKind;

/// Saved Leitner-box progress indexed by country code and question kind.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Progress {
    entries: BTreeMap<String, BTreeMap<QuestionKind, ProgressEntry>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ProgressEntry {
    box_number: u8,
}

impl Progress {
    /// Records an answer, resetting wrong answers to box one and advancing correct ones.
    pub fn record(&mut self, iso2: &str, kind: QuestionKind, correct: bool) {
        let entry = self
            .entries
            .entry(iso2.to_owned())
            .or_default()
            .entry(kind)
            .or_insert(ProgressEntry { box_number: 1 });
        if correct {
            entry.box_number = entry.box_number.saturating_add(1).min(3);
        } else {
            entry.box_number = 1;
        }
    }

    /// Returns the Leitner box for an item, using box one for unseen items.
    #[must_use]
    pub fn box_of(&self, iso2: &str, kind: QuestionKind) -> u8 {
        self.entries
            .get(iso2)
            .and_then(|kinds| kinds.get(&kind))
            .map_or(1, |entry| entry.box_number)
    }

    /// Returns items in box one, suitable for review questions.
    #[must_use]
    pub fn review_keys(&self) -> Vec<(String, QuestionKind)> {
        self.entries
            .iter()
            .flat_map(|(iso2, kinds)| {
                kinds
                    .iter()
                    .filter_map(move |(kind, entry)| match entry.box_number {
                        1 => Some((iso2.clone(), *kind)),
                        _ => None,
                    })
            })
            .collect()
    }

    /// Loads progress from JSON, treating missing, invalid, or out-of-range data as empty.
    pub fn load(path: impl AsRef<Path>) -> Self {
        fs::read(path)
            .ok()
            .and_then(|contents| serde_json::from_slice(&contents).ok())
            .filter(Self::has_valid_boxes)
            .unwrap_or_default()
    }

    /// Atomically saves progress as JSON by writing a temporary sibling file and renaming it.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the directory, writing the file, or renaming it fails.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let temporary_path = parent.join(".progress.json.tmp");
        let mut file = File::create(&temporary_path)?;
        let write_result = serde_json::to_writer(&mut file, self)
            .map_err(io::Error::other)
            .and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temporary_path, path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        Ok(())
    }

    fn has_valid_boxes(&self) -> bool {
        self.entries.values().all(|kinds| {
            kinds
                .values()
                .all(|entry| (1..=3).contains(&entry.box_number))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs, io,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestProgress {
        directory: PathBuf,
        file: PathBuf,
    }

    impl TestProgress {
        fn new() -> io::Result<Self> {
            let root = std::env::temp_dir();
            loop {
                let serial = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
                let directory =
                    root.join(format!("kunianki-progress-{}-{serial}", std::process::id()));
                match fs::create_dir(&directory) {
                    Ok(()) => {
                        return Ok(Self {
                            file: directory.join("progress.json"),
                            directory,
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }

    impl Drop for TestProgress {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn box_transitions_and_review_items_follow_box_number() {
        let mut progress = Progress::default();
        assert_eq!(progress.box_of("JP", QuestionKind::Flag), 1);
        assert_eq!(progress.review_keys(), Vec::new());

        progress.record("JP", QuestionKind::Flag, false);
        assert_eq!(progress.box_of("JP", QuestionKind::Flag), 1);
        assert_eq!(
            progress.review_keys(),
            vec![("JP".to_owned(), QuestionKind::Flag)]
        );

        progress.record("JP", QuestionKind::Flag, true);
        assert_eq!(progress.box_of("JP", QuestionKind::Flag), 2);
        assert_eq!(progress.review_keys(), Vec::new());
        progress.record("JP", QuestionKind::Flag, true);
        assert_eq!(progress.box_of("JP", QuestionKind::Flag), 3);
        progress.record("JP", QuestionKind::Flag, true);
        assert_eq!(progress.box_of("JP", QuestionKind::Flag), 3);
        progress.record("JP", QuestionKind::Flag, false);
        assert_eq!(progress.box_of("JP", QuestionKind::Flag), 1);
        assert_eq!(
            progress.review_keys(),
            vec![("JP".to_owned(), QuestionKind::Flag)]
        );
    }

    #[test]
    fn save_and_load_round_trip() -> Result<(), Box<dyn Error>> {
        let temporary = TestProgress::new()?;
        let mut progress = Progress::default();
        progress.record("JP", QuestionKind::Flag, true);
        progress.record("US", QuestionKind::Trivia, false);

        progress.save(&temporary.file)?;
        progress.save(&temporary.file)?;
        assert_eq!(Progress::load(&temporary.file), progress);

        let mut filenames = fs::read_dir(&temporary.directory)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<io::Result<Vec<_>>>()?;
        filenames.sort_unstable();
        assert_eq!(filenames.as_slice(), &[String::from("progress.json")]);
        Ok(())
    }

    #[test]
    fn missing_corrupt_and_invalid_files_load_as_default() -> Result<(), Box<dyn Error>> {
        let temporary = TestProgress::new()?;
        let missing = temporary.directory.join("missing.json");
        assert_eq!(Progress::load(&missing), Progress::default());

        let corrupt = temporary.directory.join("corrupt.json");
        fs::write(&corrupt, b"not json")?;
        assert_eq!(Progress::load(&corrupt), Progress::default());

        let invalid = temporary.directory.join("invalid.json");
        fs::write(&invalid, br#"{"entries":{"JP":{"flag":{"box_number":7}}}}"#)?;
        assert_eq!(Progress::load(&invalid), Progress::default());
        Ok(())
    }

    #[test]
    fn legacy_seen_field_is_ignored() -> Result<(), Box<dyn Error>> {
        let temporary = TestProgress::new()?;
        fs::write(
            &temporary.file,
            br#"{"entries":{"JP":{"flag":{"box_number":1,"seen":true}}}}"#,
        )?;

        let mut expected = Progress::default();
        expected.record("JP", QuestionKind::Flag, false);
        assert_eq!(Progress::load(&temporary.file), expected);
        Ok(())
    }
}
