//! Append-only local records of accepted quiz answers.

use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

use crate::quiz::QuestionKind;

const HISTORY_FILE: &str = "answer-history.jsonl";

/// One accepted answer saved independently from spaced-repetition progress.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AnswerRecord {
    pub(crate) answered_at_ms: i64,
    pub(crate) kind: QuestionKind,
    pub(crate) correct_iso2: String,
    pub(crate) selected_iso2: String,
    pub(crate) correct: bool,
    pub(crate) hints_shown: u8,
    pub(crate) points: u32,
    pub(crate) elapsed_ms: u64,
    pub(crate) choice_iso2: Vec<String>,
    pub(crate) review: bool,
}

#[derive(Debug, Default)]
pub(crate) struct HistoryRead {
    pub(crate) records: Vec<AnswerRecord>,
    pub(crate) warning: Option<String>,
}

/// Returns the answer-history file beside the configured progress file.
pub(crate) fn path_for_progress(progress_path: &Path) -> PathBuf {
    progress_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(HISTORY_FILE)
}

/// Loads valid records newest first, retaining valid rows around malformed ones.
pub(crate) fn load(path: &Path) -> io::Result<HistoryRead> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(HistoryRead::default());
        }
        Err(error) => return Err(error),
    };

    let mut records = Vec::new();
    let mut malformed_rows = 0;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<AnswerRecord>(line) {
            Ok(record) => records.push(record),
            Err(_) => malformed_rows += 1,
        }
    }
    records.reverse();
    let warning = (malformed_rows > 0).then(|| {
        format!(
            "履歴ファイルの一部を読み込めませんでした（読み取れない行: {malformed_rows}）。元データは変更していません。"
        )
    });
    Ok(HistoryRead { records, warning })
}

/// Appends one serialized answer and synchronizes it without rewriting prior rows.
pub(crate) fn append(path: &Path, record: &AnswerRecord) -> io::Result<()> {
    let serialized = serde_json::to_vec(record).map_err(io::Error::other)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;
    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last_byte = [0];
        file.read_exact(&mut last_byte)?;
        if last_byte[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    file.write_all(&serialized)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

pub(crate) fn local_timestamp(timestamp_ms: i64) -> String {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map_or_else(
            || "日時不明".into(),
            |time| time.format("%Y年%m月%d日 %H:%M").to_string(),
        )
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs, io,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::{AnswerRecord, append, load, path_for_progress};
    use crate::quiz::QuestionKind;

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            let root = std::env::temp_dir();
            loop {
                let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
                let path = root.join(format!("kunianki-history-{}-{serial}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
        }

        fn file(&self) -> PathBuf {
            self.0.join("answer-history.jsonl")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record(timestamp: i64, country: &str) -> AnswerRecord {
        AnswerRecord {
            answered_at_ms: timestamp,
            kind: QuestionKind::Trivia,
            correct_iso2: country.into(),
            selected_iso2: country.into(),
            correct: true,
            hints_shown: 2,
            points: 2,
            elapsed_ms: 1_234,
            choice_iso2: vec!["JP".into(), "FR".into(), "BR".into(), "KE".into()],
            review: true,
        }
    }

    #[test]
    fn answer_fields_round_trip_and_records_load_newest_first() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let path = directory.file();
        let older = record(1_000, "JP");
        let newer = record(2_000, "FR");
        append(&path, &older)?;
        append(&path, &newer)?;

        let loaded = load(&path)?;
        assert_eq!(loaded.warning, None);
        assert_eq!(loaded.records, vec![newer, older]);
        assert_eq!(
            loaded.records[0].choice_iso2,
            vec![
                "JP".to_owned(),
                "FR".to_owned(),
                "BR".to_owned(),
                "KE".to_owned()
            ]
        );
        assert!(loaded.records[0].review);
        assert_eq!(loaded.records[0].elapsed_ms, 1_234);
        Ok(())
    }

    #[test]
    fn appending_after_a_partial_or_corrupt_tail_preserves_valid_rows() -> Result<(), Box<dyn Error>>
    {
        let directory = TestDirectory::new()?;
        let path = directory.file();
        let valid = serde_json::to_vec(&record(1_000, "JP"))?;
        let mut original = valid.clone();
        original.extend_from_slice(b"\n{\"incomplete\":");
        fs::write(&path, &original)?;

        append(&path, &record(2_000, "FR"))?;
        let bytes = fs::read(&path)?;
        assert!(bytes.starts_with(&original));
        assert_eq!(bytes[original.len()], b'\n');

        let loaded = load(&path)?;
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[0].correct_iso2, "FR");
        assert_eq!(loaded.records[1].correct_iso2, "JP");
        assert!(loaded.warning.is_some());
        Ok(())
    }

    #[test]
    fn a_valid_partial_tail_is_kept_separate_from_the_next_record() -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let path = directory.file();
        let partial = serde_json::to_vec(&record(1_000, "JP"))?;
        fs::write(&path, &partial)?;

        append(&path, &record(2_000, "FR"))?;
        let loaded = load(&path)?;
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[0].correct_iso2, "FR");
        assert_eq!(loaded.records[1].correct_iso2, "JP");
        assert_eq!(loaded.warning, None);
        Ok(())
    }

    #[test]
    fn append_errors_are_returned_and_progress_path_uses_fixed_sibling_name()
    -> Result<(), Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let target = directory.0.join("not-a-file");
        fs::create_dir(&target)?;
        assert!(append(&target, &record(1_000, "JP")).is_err());
        assert_eq!(
            path_for_progress(Path::new("save/progress.json")).as_path(),
            Path::new("save/answer-history.jsonl")
        );
        Ok(())
    }
}
