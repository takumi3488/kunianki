//! Shared application entrypoint and platform integration for kunianki.

use std::{cell::RefCell, error::Error, io, path::PathBuf, rc::Rc};

use slint::ComponentHandle;

#[expect(clippy::unwrap_used, reason = "Slint-generated UI code uses unwrap")]
mod slint_generated {
    ::slint::include_modules!();
}

pub use slint_generated::AppWindow;
pub mod assets;
pub mod data;
pub mod progress;
pub mod quiz;
mod session;

const ROUND_LENGTH: usize = 10;

/// Settings for constructing an application window without starting its event loop.
#[derive(Debug)]
pub struct AppConfig {
    progress_path: PathBuf,
    rng_seed: Option<u64>,
}

impl AppConfig {
    /// Creates application settings for the progress JSON file at `progress_path`.
    #[must_use]
    pub fn new(progress_path: PathBuf) -> Self {
        Self {
            progress_path,
            rng_seed: None,
        }
    }

    /// Uses a deterministic random-number seed for quiz generation.
    #[must_use]
    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.rng_seed = Some(seed);
        self
    }
}

struct AppState {
    countries: Vec<data::Country>,
    progress: progress::Progress,
    progress_path: PathBuf,
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
    let ui = build_app(AppConfig::new(progress_path))?;
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
    let progress = progress::Progress::load(&progress_path);
    let rng = match config.rng_seed {
        Some(seed) => fastrand::Rng::with_seed(seed),
        None => fastrand::Rng::new(),
    };
    let ui = AppWindow::new()?;
    let state = Rc::new(RefCell::new(AppState {
        countries,
        progress,
        progress_path,
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
            state.session.start(questions);
            if state.session.question().is_none() {
                set_review_count(&ui, &state.progress);
                return;
            }
            set_question(&ui, &state);
            ui.set_screen("quiz".into());
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_answer(move |selected| {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let Ok(selected) = usize::try_from(selected) else {
                return;
            };
            let mut state = state.borrow_mut();
            {
                let AppState {
                    countries,
                    progress,
                    progress_path,
                    session,
                    ..
                } = &mut *state;
                let Some(outcome) = session.answer(selected) else {
                    return;
                };
                let Some(country) = countries.get(outcome.country_index()) else {
                    return;
                };
                progress.record(&country.iso2, outcome.kind(), outcome.correct());
                if let Err(error) = progress.save(progress_path) {
                    eprintln!("failed to save quiz progress: {error}");
                }
            }
            set_review_count(&ui, &state.progress);
            set_question(&ui, &state);
        });
    }

    {
        let weak_ui = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_reveal_hint(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
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
            let mut state = state.borrow_mut();
            match state.session.next() {
                session::Next::Question => set_question(&ui, &state),
                session::Next::Finished => {
                    set_result(&ui, &state);
                    ui.set_screen("result".into());
                }
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
            state.session = session::Session::default();
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
