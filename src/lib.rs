//! Shared application entrypoint and platform integration for kunianki.

use std::{cell::RefCell, error::Error, io, path::PathBuf, rc::Rc, time::Instant};

use slint::ComponentHandle;
use sound::SoundEffect;

#[expect(clippy::unwrap_used, reason = "Slint-generated UI code uses unwrap")]
mod slint_generated {
    ::slint::include_modules!();
}

pub use slint_generated::AppWindow;
pub mod assets;
pub mod data;
mod history;
pub mod progress;
pub mod quiz;
mod session;
pub mod sound;

const ROUND_LENGTH: usize = 10;
/// Maximum records displayed on one navigable history page.
const HISTORY_PAGE_SIZE: usize = 8;

/// Settings for constructing an application window without starting its event loop.
#[derive(Debug)]
pub struct AppConfig {
    progress_path: PathBuf,
    rng_seed: Option<u64>,
    sound: Rc<dyn sound::SoundPlayer>,
}

impl AppConfig {
    /// Creates application settings for the progress JSON file at `progress_path`.
    #[must_use]
    pub fn new(progress_path: PathBuf) -> Self {
        Self {
            progress_path,
            rng_seed: None,
            sound: Rc::new(sound::SilentPlayer),
        }
    }

    /// Uses a deterministic random-number seed for quiz generation.
    #[must_use]
    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.rng_seed = Some(seed);
        self
    }

    /// Selects the player that receives sound effect requests.
    #[must_use]
    pub fn with_sound_player(mut self, player: Rc<dyn sound::SoundPlayer>) -> Self {
        self.sound = player;
        self
    }
}

struct AppState {
    countries: Vec<data::Country>,
    progress: progress::Progress,
    progress_path: PathBuf,
    history_path: PathBuf,
    history_records: Vec<history::AnswerRecord>,
    history_page: usize,
    history_unreadable: bool,
    storage_warnings: Vec<String>,
    question_started_at: Instant,
    rng: fastrand::Rng,
    session: session::Session,
}

/// Launches the Slint application window using an optional platform data directory.
///
/// # Errors
///
/// Returns an error if the platform data directory, application setup, or event loop fails.
pub fn run(data_dir: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let progress_path = match data_dir {
        Some(directory) => directory.join("progress.json"),
        None => default_data_dir()?.join("progress.json"),
    };
    let config =
        AppConfig::new(progress_path).with_sound_player(Rc::new(sound::SystemPlayer::start()));
    let ui = build_app(config)?;
    ui.run()?;
    Ok(())
}

/// Builds the application window and connects its callbacks without starting the event loop.
///
/// The progress file path and optional random seed allow callers to control persistence and
/// quiz generation, respectively.
///
/// # Errors
///
/// Returns an error if embedded country data is invalid or Slint cannot create the window.
#[expect(
    clippy::too_many_lines,
    reason = "The builder wires the related UI callbacks in one place"
)]
pub fn build_app(config: AppConfig) -> Result<AppWindow, Box<dyn Error>> {
    let countries = data::load_countries()?;
    let progress_path = config.progress_path;
    let history_path = history::path_for_progress(&progress_path);
    let progress = progress::Progress::load(&progress_path);
    let rng = match config.rng_seed {
        Some(seed) => fastrand::Rng::with_seed(seed),
        None => fastrand::Rng::new(),
    };
    let ui = AppWindow::new()?;
    let sound = config.sound;
    {
        let sound = Rc::clone(&sound);
        ui.global::<slint_generated::SoundEvents>()
            .on_tap(move || sound.play(SoundEffect::Tap));
    }
    ui.set_privacy_text(include_str!("../docs/privacy.md").into());
    ui.set_flag_license_text(include_str!("../assets/flags/LICENSE").into());
    ui.set_font_license_text(include_str!("../assets/fonts/OFL.txt").into());
    let state = Rc::new(RefCell::new(AppState {
        countries,
        progress,
        progress_path,
        history_path,
        history_records: Vec::new(),
        history_page: 0,
        history_unreadable: false,
        storage_warnings: Vec::new(),
        question_started_at: Instant::now(),
        rng,
        session: session::Session::default(),
    }));

    set_review_count(&ui, &state.borrow().progress);

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_start_round(move |mode, region| {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            let mode = match mode {
                0 => quiz::Mode::Flag,
                1 => quiz::Mode::Map,
                2 => quiz::Mode::Trivia,
                _ => quiz::Mode::Mix,
            };
            let region = match region {
                1 => quiz::RegionFilter::Only(data::Region::Asia),
                2 => quiz::RegionFilter::Only(data::Region::Europe),
                3 => quiz::RegionFilter::Only(data::Region::Africa),
                4 => quiz::RegionFilter::Only(data::Region::NorthAmerica),
                5 => quiz::RegionFilter::Only(data::Region::SouthAmerica),
                6 => quiz::RegionFilter::Only(data::Region::Oceania),
                _ => quiz::RegionFilter::All,
            };
            let questions = {
                let AppState { countries, rng, .. } = &mut *state;
                quiz::generate_round(countries, mode, region, rng, ROUND_LENGTH)
            };
            state.session.start(questions);
            if state.session.question().is_none() {
                return;
            }
            state.question_started_at = Instant::now();
            ui.set_quit_confirmation_open(false);
            set_question(&ui, &state);
            ui.set_screen("quiz".into());
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_start_review(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            let questions = {
                let AppState {
                    countries,
                    progress,
                    rng,
                    ..
                } = &mut *state;
                quiz::generate_review(countries, progress, rng, ROUND_LENGTH)
            };
            state.session.start_review(questions);
            if state.session.question().is_none() {
                set_review_count(&ui, &state.progress);
                return;
            }
            state.question_started_at = Instant::now();
            ui.set_quit_confirmation_open(false);
            set_question(&ui, &state);
            ui.set_screen("quiz".into());
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        let sound = Rc::clone(&sound);
        ui.on_answer(move |selected| {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            if ui.get_screen().as_str() != "quiz" || ui.get_quit_confirmation_open() {
                return;
            }

            let Ok(selected) = usize::try_from(selected) else {
                return;
            };
            let mut state = state.borrow_mut();
            let Some(outcome) = state.session.answer(selected) else {
                return;
            };
            let feedback = if outcome.correct() {
                SoundEffect::Correct
            } else {
                SoundEffect::Incorrect
            };
            let answered_at_ms = chrono::Utc::now().timestamp_millis();
            let elapsed_ms =
                u64::try_from(state.question_started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            record_accepted_answer(&mut state, outcome, answered_at_ms, elapsed_ms);
            set_review_count(&ui, &state.progress);
            set_storage_warning(&ui, &state);
            set_question(&ui, &state);
            sound.play(feedback);
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_reveal_hint(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            if ui.get_screen().as_str() != "quiz" || ui.get_quit_confirmation_open() {
                return;
            }

            let mut state = state.borrow_mut();
            if state.session.reveal_hint() {
                set_question(&ui, &state);
            }
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_next_question(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            if ui.get_screen().as_str() != "quiz" || ui.get_quit_confirmation_open() {
                return;
            }

            let mut state = state.borrow_mut();
            let current_question_index = state.session.question_index();
            match state.session.next() {
                session::Next::Question => {
                    if state.session.question_index() != current_question_index {
                        state.question_started_at = Instant::now();
                    }
                    set_question(&ui, &state);
                }
                session::Next::Finished => {
                    set_result(&ui, &state);
                    ui.set_screen("result".into());
                    sound.play(SoundEffect::RoundComplete);
                }
            }
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_open_history(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            if let Ok(history) = history::load(&state.history_path) {
                state.history_unreadable = history.records.is_empty() && history.warning.is_some();
                state.history_records = history.records;
                if let Some(warning) = history.warning {
                    add_storage_warning(&mut state, &warning);
                }
            } else {
                state.history_unreadable = true;
                state.history_records.clear();
                add_storage_warning(
                    &mut state,
                    "回答履歴を読み込めませんでした。元データは変更していません。",
                );
            }
            state.history_page = 0;
            set_history_page(&ui, &state);
            set_storage_warning(&ui, &state);
            ui.set_screen("history".into());
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_history_older(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            let last_page = state.history_records.len().saturating_sub(1) / HISTORY_PAGE_SIZE;
            if state.history_page < last_page {
                state.history_page += 1;
                set_history_page(&ui, &state);
            }
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_history_newer(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            if state.history_page > 0 {
                state.history_page -= 1;
                set_history_page(&ui, &state);
            }
        });
    }
    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_go_home(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            ui.set_quit_confirmation_open(false);

            state.session = session::Session::default();
            state.history_records.clear();
            state.history_page = 0;
            set_storage_warning(&ui, &state);
            set_review_count(&ui, &state.progress);
            ui.set_screen("home".into());
        });
    }

    Ok(ui)
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
extern "Rust" fn android_main(app: slint::android::AndroidApp) {
    let data_dir = app.internal_data_path();
    if let Err(error) = slint::android::init(app) {
        eprintln!("failed to initialize Slint on Android: {error}");
        return;
    }
    if let Err(error) = run(data_dir) {
        eprintln!("failed to run kunianki: {error}");
    }
}

fn default_data_dir() -> io::Result<PathBuf> {
    #[cfg(target_os = "android")]
    {
        Err(io::Error::other(
            "Android internal data path was not provided",
        ))
    }
    #[cfg(target_os = "ios")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| {
                home.join("Library")
                    .join("Application Support")
                    .join("kunianki")
            })
            .ok_or_else(|| io::Error::other("HOME is not set"))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        dirs::data_dir()
            .map(|directory| directory.join("kunianki"))
            .ok_or_else(|| io::Error::other("platform data directory is unavailable"))
    }
}

fn set_review_count(ui: &slint_generated::AppWindow, progress: &progress::Progress) {
    ui.set_review_count(ui_int(progress.review_keys().len()));
}

fn ui_int(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn ui_score(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn set_question(ui: &slint_generated::AppWindow, state: &AppState) {
    let Some(question) = state.session.question() else {
        return;
    };
    let Some(country) = state.countries.get(question.country) else {
        return;
    };
    let choices = question
        .choices
        .iter()
        .filter_map(|&index| {
            state
                .countries
                .get(index)
                .map(|choice| slint::SharedString::from(choice.name_ja.as_str()))
        })
        .collect::<Vec<_>>();
    let hints = if question.kind == quiz::QuestionKind::Trivia {
        quiz::hints(country)
            .into_iter()
            .take(usize::from(state.session.hints_shown()))
            .map(slint::SharedString::from)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let question_image = country_image(country, question.kind);
    ui.set_question_number(ui_int(state.session.question_index() + 1));
    ui.set_total_questions(ui_int(state.session.question_count()));
    ui.set_score(ui_score(state.session.score()));
    ui.set_kind(kind_name(question.kind).into());
    ui.set_question_image(question_image);
    ui.set_hints(slint::ModelRc::new(slint::VecModel::from(hints)));
    ui.set_can_reveal_hint(
        question.kind == quiz::QuestionKind::Trivia
            && !state.session.answered()
            && state.session.hints_shown() < 3,
    );
    ui.set_choices(slint::ModelRc::new(slint::VecModel::from(choices)));
    ui.set_answered(state.session.answered());
    ui.set_selected_index(state.session.selected_index().map_or(-1, ui_int));
    ui.set_correct_index(if state.session.answered() {
        ui_int(question.answer)
    } else {
        -1
    });
}

fn set_result(ui: &slint_generated::AppWindow, state: &AppState) {
    let missed = state
        .session
        .missed_country_indices()
        .iter()
        .filter_map(|&index| state.countries.get(index))
        .map(|country| slint_generated::MissedItem {
            name: country.name_ja.as_str().into(),
            capital: country.capital_ja.as_str().into(),
            flag: country_image(country, quiz::QuestionKind::Flag),
        })
        .collect::<Vec<_>>();
    ui.set_result_correct(ui_int(state.session.correct_count()));
    ui.set_result_score(ui_score(state.session.score()));
    ui.set_missed(slint::ModelRc::new(slint::VecModel::from(missed)));
}

fn record_accepted_answer(
    state: &mut AppState,
    outcome: session::AnswerOutcome,
    answered_at_ms: i64,
    elapsed_ms: u64,
) {
    let correct_iso2 = state
        .countries
        .get(outcome.country_index())
        .map(|country| country.iso2.clone());
    if let Some(iso2) = correct_iso2.as_deref() {
        state
            .progress
            .record(iso2, outcome.kind(), outcome.correct());
    } else {
        add_storage_warning(
            state,
            "学習進捗を更新できませんでした。国データを確認してください。",
        );
    }
    if state.progress.save(&state.progress_path).is_err() {
        add_storage_warning(
            state,
            "学習進捗を保存できませんでした。端末の保存領域を確認してください。",
        );
    }

    let selected_iso2 = state
        .countries
        .get(outcome.selected_country_index())
        .map(|country| country.iso2.clone());
    let choice_iso2 = outcome
        .choice_country_indices()
        .into_iter()
        .map(|index| {
            state
                .countries
                .get(index)
                .map(|country| country.iso2.clone())
        })
        .collect::<Option<Vec<_>>>();
    if let (Some(correct_iso2), Some(selected_iso2), Some(choice_iso2)) =
        (correct_iso2, selected_iso2, choice_iso2)
    {
        let record = history::AnswerRecord {
            answered_at_ms,
            kind: outcome.kind(),
            correct_iso2,
            selected_iso2,
            correct: outcome.correct(),
            hints_shown: outcome.hints_shown(),
            points: outcome.points(),
            elapsed_ms,
            choice_iso2,
            review: outcome.review(),
        };
        if history::append(&state.history_path, &record).is_err() {
            add_storage_warning(
                state,
                "回答履歴を保存できませんでした。端末の保存領域を確認してください。",
            );
        }
    } else {
        add_storage_warning(
            state,
            "回答履歴を保存できませんでした。国データを確認してください。",
        );
    }
}

fn add_storage_warning(state: &mut AppState, warning: &str) {
    if !state
        .storage_warnings
        .iter()
        .any(|existing| existing == warning)
    {
        state.storage_warnings.push(warning.into());
    }
}

fn set_storage_warning(ui: &slint_generated::AppWindow, state: &AppState) {
    ui.set_storage_warning(state.storage_warnings.join("\n").into());
}

fn set_history_page(ui: &slint_generated::AppWindow, state: &AppState) {
    let record_count = state.history_records.len();
    let page_count = record_count.div_ceil(HISTORY_PAGE_SIZE);
    let page = state.history_page.min(page_count.saturating_sub(1));
    let items = state
        .history_records
        .iter()
        .skip(page * HISTORY_PAGE_SIZE)
        .take(HISTORY_PAGE_SIZE)
        .map(|record| {
            let correct_country = history_country_name(&state.countries, &record.correct_iso2);
            let selected_country = history_country_name(&state.countries, &record.selected_iso2);
            let review = if record.review { " / 復習" } else { "" };
            slint_generated::HistoryItem {
                timestamp: history::local_timestamp(record.answered_at_ms).into(),
                outcome: format!(
                    "{} / {}",
                    if record.correct {
                        "正解"
                    } else {
                        "不正解"
                    },
                    history_kind_name(record.kind)
                )
                .into(),
                countries: format!("正解: {correct_country} / 選択: {selected_country}").into(),
                details: format!(
                    "ヒント: {}  得点: {}  回答時間: {}.{:03}秒{}",
                    record.hints_shown,
                    record.points,
                    record.elapsed_ms / 1_000,
                    record.elapsed_ms % 1_000,
                    review
                )
                .into(),
            }
        })
        .collect::<Vec<_>>();
    ui.set_history_items(slint::ModelRc::new(slint::VecModel::from(items)));
    ui.set_history_empty(record_count == 0);
    ui.set_history_unreadable(state.history_unreadable);
    ui.set_history_has_newer(page > 0);
    ui.set_history_has_older(page + 1 < page_count);
    ui.set_history_page_label(if record_count == 0 {
        "0件".into()
    } else {
        format!("{} / {}", page + 1, page_count).into()
    });
}

fn history_country_name(countries: &[data::Country], iso2: &str) -> String {
    countries
        .iter()
        .find(|country| country.iso2 == iso2)
        .map_or_else(|| iso2.to_owned(), |country| country.name_ja.clone())
}

fn history_kind_name(kind: quiz::QuestionKind) -> &'static str {
    match kind {
        quiz::QuestionKind::Flag => "国旗",
        quiz::QuestionKind::Map => "地図",
        quiz::QuestionKind::Trivia => "ヒント",
    }
}

fn kind_name(kind: quiz::QuestionKind) -> &'static str {
    match kind {
        quiz::QuestionKind::Flag => "flag",
        quiz::QuestionKind::Map => "map",
        quiz::QuestionKind::Trivia => "trivia",
    }
}

fn country_image(country: &data::Country, kind: quiz::QuestionKind) -> slint::Image {
    let iso2_lower = country.iso2.to_ascii_lowercase();
    let svg = match kind {
        quiz::QuestionKind::Flag => assets::flag_svg(&iso2_lower),
        quiz::QuestionKind::Map => assets::map_svg(&iso2_lower),
        quiz::QuestionKind::Trivia => None,
    };
    svg.and_then(|svg| slint::Image::load_from_svg_data(svg).ok())
        .unwrap_or_default()
}
