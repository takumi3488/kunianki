use std::{
    collections::HashSet,
    error::Error,
    fs, io,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
use kunianki::{AppConfig, AppWindow, build_app, data};
use slint::{ComponentHandle, Model};

const TEST_SEED: u64 = 0x4b55_4e49;
static NEXT_TEMP_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct TestProgress {
    directory: PathBuf,
    file: PathBuf,
}

impl TestProgress {
    fn new() -> io::Result<Self> {
        let root = std::env::temp_dir();
        loop {
            let serial = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = root.join(format!("kunianki-e2e-{}-{serial}", std::process::id()));
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

    fn config(&self) -> AppConfig {
        AppConfig::new(self.file.clone()).with_rng_seed(TEST_SEED)
    }
}

impl Drop for TestProgress {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn accessible_button(app: &AppWindow, label: &str) -> Result<ElementHandle, Box<dyn Error>> {
    let mut matches = ElementHandle::find_by_accessible_label(app, label)
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Button));
    let button = matches.next().ok_or_else(|| {
        let visible_buttons = app
            .root_element()
            .query_descendants()
            .match_accessible_role(AccessibleRole::Button)
            .find_all()
            .into_iter()
            .filter_map(|element| element.accessible_label().map(|label| label.to_string()))
            .collect::<Vec<_>>();
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no button labeled {label:?}; visible buttons: {visible_buttons:?}"),
        )
    })?;
    if let Some(second) = matches.next() {
        return Err(io::Error::other(format!(
            "multiple buttons labeled {label:?}: {:?}/{:?}, {:?}/{:?}",
            button.type_name(),
            button.accessible_role(),
            second.type_name(),
            second.accessible_role()
        ))
        .into());
    }
    if button.accessible_role().is_none() {
        return Err(io::Error::other(format!("element {label:?} has no accessible role")).into());
    }
    Ok(button)
}

fn test_app(progress: &TestProgress) -> Result<AppWindow, Box<dyn Error>> {
    let app = build_app(progress.config())?;
    app.window().set_size(slint::PhysicalSize::new(390, 844));
    Ok(app)
}

fn invoke(app: &AppWindow, label: &str) -> Result<(), Box<dyn Error>> {
    accessible_button(app, label)?.invoke_accessible_default_action();
    Ok(())
}

fn start_round(app: &AppWindow, mode: &str, region: &str) -> Result<(), Box<dyn Error>> {
    invoke(app, "クイズを始める")?;
    invoke(app, mode)?;
    invoke(app, region)?;
    invoke(app, "10問を始める")?;
    assert_eq!(app.get_screen().as_str(), "quiz");
    Ok(())
}

fn answer_question(
    app: &AppWindow,
    selected_index: usize,
) -> Result<(bool, String, i32), Box<dyn Error>> {
    let selected_choice = app.get_choices().row_data(selected_index).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "choice index is out of range")
    })?;
    let label = format!("選択肢 {}: {selected_choice}", selected_index + 1);
    let score_before_answer = app.get_score();
    invoke(app, &label)?;
    assert!(
        app.get_answered(),
        "answer action should mark the question answered"
    );
    assert_eq!(
        app.get_selected_index(),
        i32::try_from(selected_index)?,
        "selected choice should match the accessible button"
    );
    let correct_index = usize::try_from(app.get_correct_index())?;
    let correct_choice = app.get_choices().row_data(correct_index).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "correct choice is out of range")
    })?;
    Ok((
        selected_index == correct_index,
        correct_choice.to_string(),
        app.get_score() - score_before_answer,
    ))
}

fn finish_flag_round(app: &AppWindow) -> Result<Vec<String>, Box<dyn Error>> {
    let total_questions = usize::try_from(app.get_total_questions())?;
    let mut missed = Vec::new();
    for question_index in 0..total_questions {
        let choice_count = app.get_choices().row_count();
        assert!(choice_count > 0, "flag question should have answer choices");
        let selected_index = question_index % choice_count;
        let score_before_answer = app.get_score();
        let (correct, correct_choice, score_delta) = answer_question(app, selected_index)?;
        let expected_score_delta = i32::from(correct);
        assert_eq!(score_delta, expected_score_delta);
        if !correct {
            missed.push(correct_choice);
        }
        assert_eq!(
            app.get_score(),
            score_before_answer + expected_score_delta,
            "flag score after question {question_index}"
        );
        invoke(app, "次の問題へ")?;
    }
    Ok(missed)
}

#[test]
fn full_flag_round_updates_results_and_review_count() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;

    assert_eq!(app.get_screen().as_str(), "home");
    start_round(&app, "国旗", "全地域")?;
    let total_questions = usize::try_from(app.get_total_questions())?;
    let missed = finish_flag_round(&app)?;

    assert_eq!(app.get_screen().as_str(), "result");
    assert_eq!(
        app.get_result_correct(),
        i32::try_from(total_questions - missed.len())?
    );
    assert_eq!(
        app.get_result_score(),
        i32::try_from(total_questions - missed.len())?
    );
    assert_eq!(app.get_missed().row_count(), missed.len());
    invoke(&app, "ホームへ戻る")?;
    assert_eq!(app.get_screen().as_str(), "home");
    assert_eq!(app.get_review_count(), i32::try_from(missed.len())?);
    Ok(())
}

#[test]
fn double_tapping_an_answer_records_it_only_once() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "国旗", "全地域")?;
    let first_choice = app.get_choices().row_data(0).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "question has no first choice")
    })?;
    let label = format!("選択肢 1: {first_choice}");
    let button = accessible_button(&app, &label)?;
    button.invoke_accessible_default_action();
    assert!(
        app.get_answered(),
        "answer action should mark the question answered"
    );
    let correct_index = usize::try_from(app.get_correct_index())?;
    assert!(
        app.get_choices().row_data(correct_index).is_some(),
        "correct index should identify a displayed choice"
    );
    let expected_score = i32::from(correct_index == 0);
    let expected_review_count = i32::from(correct_index != 0);
    assert_eq!(app.get_score(), expected_score);
    assert_eq!(app.get_review_count(), expected_review_count);
    let saved_score = app.get_score();
    let saved_review_count = app.get_review_count();

    button.invoke_accessible_default_action();
    assert_eq!(app.get_score(), saved_score);
    assert_eq!(app.get_review_count(), saved_review_count);
    Ok(())
}

#[test]
fn trivia_hints_limit_and_scoring_follow_revealed_count() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "3ヒント", "全地域")?;

    assert_eq!(app.get_hints().row_count(), 1);
    assert!(app.get_can_reveal_hint());
    invoke(&app, "次のヒントを見る")?;
    assert_eq!(app.get_hints().row_count(), 2);
    invoke(&app, "次のヒントを見る")?;
    assert_eq!(app.get_hints().row_count(), 3);
    assert!(!app.get_can_reveal_hint());
    invoke(&app, "次のヒントを見る")?;
    assert_eq!(app.get_hints().row_count(), 3);

    let (first_correct, _, first_score_delta) = answer_question(&app, 0)?;
    assert_eq!(first_score_delta, i32::from(first_correct));
    assert_eq!(app.get_score(), first_score_delta);
    invoke(&app, "次の問題へ")?;
    assert_eq!(app.get_hints().row_count(), 1);
    invoke(&app, "次のヒントを見る")?;
    assert_eq!(app.get_hints().row_count(), 2);
    let score_before_answer = app.get_score();
    let (second_correct, _, second_score_delta) = answer_question(&app, 0)?;
    assert_eq!(second_score_delta, if second_correct { 2 } else { 0 });
    assert_eq!(app.get_score() - score_before_answer, second_score_delta);
    Ok(())
}

#[test]
fn flag_and_map_questions_load_nonempty_images() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let map_app = test_app(&progress)?;
    start_round(&map_app, "地図", "全地域")?;
    assert_eq!(map_app.get_kind().as_str(), "map");
    let map_size = map_app.get_question_image().size();
    assert!(
        map_size.width > 0 && map_size.height > 0,
        "map image should be nonempty"
    );

    let flag_app = test_app(&progress)?;
    start_round(&flag_app, "国旗", "全地域")?;
    assert_eq!(flag_app.get_kind().as_str(), "flag");
    let flag_size = flag_app.get_question_image().size();
    assert!(
        flag_size.width > 0 && flag_size.height > 0,
        "flag image should be nonempty"
    );
    Ok(())
}

#[test]
fn region_tiles_start_rounds_with_matching_country_regions() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let countries = data::load_countries()?;
    for (region, label) in [
        (data::Region::Asia, "アジア"),
        (data::Region::Europe, "ヨーロッパ"),
        (data::Region::Africa, "アフリカ"),
        (data::Region::NorthAmerica, "北米・中米"),
        (data::Region::SouthAmerica, "南米"),
        (data::Region::Oceania, "オセアニア"),
    ] {
        let progress = TestProgress::new()?;
        let app = test_app(&progress)?;
        start_round(&app, "国旗", label)?;
        let (_, correct_country_name, _) = answer_question(&app, 0)?;
        let country = countries
            .iter()
            .find(|country| country.name_ja == correct_country_name)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("correct choice {correct_country_name:?} is missing from country data"),
                )
            })?;
        assert_eq!(
            country.region, region,
            "region tile {label:?} should only produce countries from its region"
        );
    }
    Ok(())
}

#[test]
fn mix_round_includes_flag_map_and_trivia_questions() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "おまかせ", "全地域")?;

    let total_questions = usize::try_from(app.get_total_questions())?;
    let mut kinds = HashSet::new();
    for question_index in 0..total_questions {
        assert_eq!(
            app.get_question_number(),
            i32::try_from(question_index + 1)?
        );
        kinds.insert(app.get_kind().to_string());
        answer_question(&app, 0)?;
        invoke(&app, "次の問題へ")?;
    }
    assert_eq!(app.get_screen().as_str(), "result");
    assert_eq!(
        kinds.len(),
        3,
        "mix round should include all question kinds"
    );
    for kind in ["flag", "map", "trivia"] {
        assert!(
            kinds.contains(kind),
            "mix round should include {kind} questions"
        );
    }
    Ok(())
}

#[test]
fn review_questions_are_selected_from_the_missed_countries() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;

    start_round(&app, "国旗", "全地域")?;
    let missed = finish_flag_round(&app)?;
    assert!(!missed.is_empty(), "round should include a missed answer");
    assert_eq!(app.get_screen().as_str(), "result");
    invoke(&app, "ホームへ戻る")?;
    invoke(&app, "まちがえた国を復習")?;
    assert_eq!(app.get_screen().as_str(), "quiz");
    let review_question_count = usize::try_from(app.get_total_questions())?;
    assert_eq!(review_question_count, missed.len());
    let missed_names: HashSet<String> = missed.into_iter().collect();

    for index in 0..review_question_count {
        let choices = app.get_choices();
        let first_choice = choices.row_data(0).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "review question has no first choice",
            )
        })?;
        let first_label = format!("選択肢 1: {first_choice}");
        invoke(&app, &first_label)?;
        let correct_index = usize::try_from(app.get_correct_index())?;
        let correct_country = choices.row_data(correct_index).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "correct choice is out of range")
        })?;
        assert!(
            missed_names.contains(correct_country.as_str()),
            "reviewed country {correct_country} was not missed in the preceding round"
        );
        invoke(&app, "次の問題へ")?;
        if index + 1 < review_question_count {
            assert_eq!(app.get_question_number(), i32::try_from(index + 2)?);
        } else {
            assert_eq!(app.get_screen().as_str(), "result");
        }
    }
    assert_eq!(app.get_screen().as_str(), "result");
    Ok(())
}

#[test]
fn progress_is_loaded_by_a_second_app_instance() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let first_app = test_app(&progress)?;
    start_round(&first_app, "国旗", "全地域")?;
    let total_questions = usize::try_from(first_app.get_total_questions())?;
    let mut expected_review_count = None;
    for question_index in 0..total_questions {
        let choice_count = first_app.get_choices().row_count();
        assert!(choice_count > 0, "flag question should have answer choices");
        let selected_index = question_index % choice_count;
        let (correct, _, _) = answer_question(&first_app, selected_index)?;
        if !correct {
            expected_review_count = Some(first_app.get_review_count());
            break;
        }
        if question_index + 1 < total_questions {
            invoke(&first_app, "次の問題へ")?;
        }
    }
    let expected_review_count = expected_review_count
        .ok_or_else(|| io::Error::other("no missed answer was recorded in the round"))?;
    assert!(expected_review_count > 0);

    let second_app = test_app(&progress)?;
    assert_eq!(second_app.get_review_count(), expected_review_count);
    Ok(())
}
