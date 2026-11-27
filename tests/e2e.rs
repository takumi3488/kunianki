use std::{
    cell::RefCell,
    collections::HashSet,
    error::Error,
    fs, io,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
use kunianki::sound::{SoundEffect, SoundPlayer};
use kunianki::{AppConfig, AppWindow, build_app, data, progress, quiz};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model};

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

    fn config(&self, seed: u64) -> AppConfig {
        AppConfig::new(self.file.clone()).with_rng_seed(seed)
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

fn element_by_id(app: &AppWindow, id: &str) -> Result<ElementHandle, Box<dyn Error>> {
    let mut matches = ElementHandle::find_by_element_id(app, id);
    let element = matches.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no element with id {id:?}"),
        )
    })?;
    if matches.next().is_some() {
        return Err(io::Error::other(format!("multiple elements with id {id:?}")).into());
    }
    Ok(element)
}

fn map_display(app: &AppWindow) -> Result<ElementHandle, Box<dyn Error>> {
    let mut matches = ElementHandle::find_by_accessible_label(app, "地図の表示領域");
    let display = matches
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "map display is not accessible"))?;
    if matches.next().is_some() {
        return Err(io::Error::other("multiple map displays are visible").into());
    }
    Ok(display)
}

fn tap(app: &AppWindow, label: &str) -> Result<(), Box<dyn Error>> {
    accessible_button(app, label)?.mock_single_click(slint::platform::PointerEventButton::Left);
    Ok(())
}

fn settle() {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
}

fn assert_inside_window(element: &ElementHandle, width: f32, height: f32) {
    let position = element.absolute_position();
    let size = element.size();
    assert!(
        position.x >= -0.5
            && position.y >= -0.5
            && position.x + size.width <= width + 0.5
            && position.y + size.height <= height + 0.5,
        "element {:?} at ({}, {}) size ({}, {}) is outside {width}x{height}",
        element.accessible_label(),
        position.x,
        position.y,
        size.width,
        size.height
    );
}

fn assert_map_toolbar_outside_viewport(app: &AppWindow) -> Result<(), Box<dyn Error>> {
    let viewport = element_by_id(app, "MapViewer::map-flickable")?;
    let viewport_position = viewport.absolute_position();
    let viewport_size = viewport.size();
    for label in ["地図を拡大", "地図を縮小", "地図をリセット"] {
        let control = accessible_button(app, label)?;
        let position = control.absolute_position();
        let size = control.size();
        assert!(
            position.x + size.width <= viewport_position.x
                || viewport_position.x + viewport_size.width <= position.x
                || position.y + size.height <= viewport_position.y
                || viewport_position.y + viewport_size.height <= position.y,
            "{label} overlaps the map viewport"
        );
    }
    Ok(())
}

fn assert_image_centered_in_viewport(viewport: &ElementHandle, image: &ElementHandle) {
    let viewport_position = viewport.absolute_position();
    let viewport_size = viewport.size();
    let image_position = image.absolute_position();
    let image_size = image.size();
    assert!(
        (image_position.x + image_size.width / 2.0
            - viewport_position.x
            - viewport_size.width / 2.0)
            .abs()
            < 1.0,
        "the map's center target should remain horizontally visible (viewport x={}, width={}, image x={}, width={})",
        viewport_position.x,
        viewport_size.width,
        image_position.x,
        image_size.width
    );
    assert!(
        (image_position.y + image_size.height / 2.0
            - viewport_position.y
            - viewport_size.height / 2.0)
            .abs()
            < 1.0,
        "the map's center target should remain vertically visible"
    );
}

fn test_app(progress: &TestProgress) -> Result<AppWindow, Box<dyn Error>> {
    test_app_with_seed(progress, TEST_SEED)
}

fn test_app_with_seed(progress: &TestProgress, seed: u64) -> Result<AppWindow, Box<dyn Error>> {
    let app = build_app(progress.config(seed))?;
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

fn first_trivia_answer_index() -> Result<usize, Box<dyn Error>> {
    let countries = data::load_countries()?;
    let mut rng = fastrand::Rng::with_seed(TEST_SEED);
    let first_question = quiz::generate_round(
        &countries,
        quiz::Mode::Trivia,
        quiz::RegionFilter::All,
        &mut rng,
        10,
    )
    .into_iter()
    .next()
    .ok_or_else(|| io::Error::other("seeded trivia round has no questions"))?;
    Ok(first_question.answer)
}

fn first_flag_answer_index() -> Result<usize, Box<dyn Error>> {
    let countries = data::load_countries()?;
    let mut rng = fastrand::Rng::with_seed(TEST_SEED);
    let first_question = quiz::generate_round(
        &countries,
        quiz::Mode::Flag,
        quiz::RegionFilter::All,
        &mut rng,
        10,
    )
    .into_iter()
    .next()
    .ok_or_else(|| io::Error::other("seeded flag round has no questions"))?;
    Ok(first_question.answer)
}

fn first_review_answer_index(progress_data: &TestProgress) -> Result<usize, Box<dyn Error>> {
    let countries = data::load_countries()?;
    let progress = progress::Progress::load(&progress_data.file);
    let mut rng = fastrand::Rng::with_seed(TEST_SEED);
    let first_question = quiz::generate_review(&countries, &progress, &mut rng, 10)
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::other("seeded review round has no questions"))?;
    Ok(first_question.answer)
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

#[derive(Debug, Default)]
struct RecordingPlayer(RefCell<Vec<SoundEffect>>);

impl SoundPlayer for RecordingPlayer {
    fn play(&self, effect: SoundEffect) {
        self.0.borrow_mut().push(effect);
    }
}

impl RecordingPlayer {
    fn take(&self) -> Vec<SoundEffect> {
        self.0.take()
    }
}

fn sound_app(progress: &TestProgress) -> Result<(AppWindow, Rc<RecordingPlayer>), Box<dyn Error>> {
    let player = Rc::new(RecordingPlayer::default());
    let app = build_app(progress.config(TEST_SEED).with_sound_player(player.clone()))?;
    app.window().set_size(slint::PhysicalSize::new(390, 844));
    Ok((app, player))
}

type ButtonInput = fn(&AppWindow, &str) -> Result<(), Box<dyn Error>>;
const BUTTON_INPUTS: [ButtonInput; 2] = [invoke, tap];

fn assert_tap_sound(
    app: &AppWindow,
    player: &RecordingPlayer,
    press: ButtonInput,
    label: &str,
) -> Result<(), Box<dyn Error>> {
    settle();
    press(app, label)?;
    assert_eq!(player.take(), [SoundEffect::Tap], "button {label:?}");
    Ok(())
}

#[test]
fn sound_navigation_buttons_request_one_tap_for_either_input() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    for press in BUTTON_INPUTS {
        let progress = TestProgress::new()?;
        let mut saved = progress::Progress::default();
        saved.record("JP", quiz::QuestionKind::Flag, false);
        saved.save(&progress.file)?;
        let (app, player) = sound_app(&progress)?;
        for label in [
            "このアプリについて",
            "ホーム",
            "回答履歴を見る",
            "ホーム",
            "クイズを始める",
            "3ヒント",
            "全地域",
            "10問を始める",
            "次のヒントを見る",
            "クイズを中断",
            "ホームへ戻る",
            "まちがえた国を復習、1問",
        ] {
            assert_tap_sound(&app, &player, press, label)?;
        }
        assert_eq!(app.get_screen().as_str(), "quiz");
    }
    Ok(())
}

#[test]
fn sound_disabled_review_tile_is_silent_for_either_input() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let (app, player) = sound_app(&progress)?;
    for press in BUTTON_INPUTS {
        press(&app, "まちがえた国を復習、0問")?;
        assert_eq!(player.take(), [SoundEffect::Tap; 0]);
        assert_eq!(app.get_screen().as_str(), "home");
    }
    Ok(())
}

#[test]
fn sound_map_control_taps_but_map_drag_is_silent() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let (app, player) = sound_app(&progress)?;
    start_round(&app, "地図", "全地域")?;
    player.take();
    for press in BUTTON_INPUTS {
        assert_tap_sound(&app, &player, press, "地図を拡大")?;
    }
    settle();
    let viewport = element_by_id(&app, "MapViewer::map-flickable")?;
    let origin = viewport.absolute_position();
    let size = viewport.size();
    viewport.mock_drag(
        LogicalPosition::new(origin.x + size.width / 4.0, origin.y + size.height / 4.0),
        PointerEventButton::Left,
    );
    assert!(player.take().is_empty(), "map panning is not a button");
    Ok(())
}

#[test]
fn sound_only_accepted_answers_request_feedback_for_either_input() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let correct = first_flag_answer_index()?;
    for press in BUTTON_INPUTS {
        for expected in [SoundEffect::Correct, SoundEffect::Incorrect] {
            let progress = TestProgress::new()?;
            let (app, player) = sound_app(&progress)?;
            start_round(&app, "国旗", "全地域")?;
            let index = if expected == SoundEffect::Correct {
                correct
            } else {
                (correct + 1) % app.get_choices().row_count()
            };
            let choice = app
                .get_choices()
                .row_data(index)
                .ok_or_else(|| io::Error::other("seeded flag choice is absent"))?;
            let label = format!("選択肢 {}: {choice}", index + 1);
            let selected = i32::try_from(index)?;
            invoke(&app, "クイズを中断")?;
            assert!(app.get_quit_confirmation_open());
            player.take();
            settle();
            invoke(&app, &label)?;
            app.invoke_answer(selected);
            assert!(player.take().is_empty(), "modal blocks answer feedback");
            assert!(!app.get_answered());

            let position = LogicalPosition::new(4.0, 4.0);
            let button = PointerEventButton::Left;
            for event in [
                WindowEvent::PointerMoved { position },
                WindowEvent::PointerPressed { position, button },
                WindowEvent::PointerReleased { position, button },
            ] {
                app.window().dispatch_event(event);
            }
            assert!(app.get_quit_confirmation_open());
            assert!(player.take().is_empty(), "modal backdrop is not a button");

            invoke(&app, "続ける")?;
            player.take();
            settle();
            press(&app, &label)?;
            assert!(app.get_answered());
            assert_eq!(player.take(), [expected], "accepted answer has no Tap");
            press(&app, &label)?;
            app.invoke_answer(selected);
            assert!(player.take().is_empty(), "duplicate answer has no feedback");
        }
    }
    Ok(())
}

#[test]
fn sound_flag_round_plays_completion_only_after_final_next_tap() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    for press in BUTTON_INPUTS {
        assert_sound_flag_round(press)?;
    }
    Ok(())
}

fn assert_sound_flag_round(press: ButtonInput) -> Result<(), Box<dyn Error>> {
    let progress = TestProgress::new()?;
    let (app, player) = sound_app(&progress)?;
    start_round(&app, "国旗", "全地域")?;
    assert_eq!(player.take(), [SoundEffect::Tap; 4]);
    assert_eq!(app.get_total_questions(), 10);
    let mut correct_count = 0;
    let mut incorrect_count = 0;
    for index in 0..10 {
        let choice = if index == 0 {
            first_flag_answer_index()?
        } else {
            index % app.get_choices().row_count()
        };
        let (correct, _, _) = answer_question(&app, choice)?;
        let requested = player.take();
        let expected = if correct {
            SoundEffect::Correct
        } else {
            SoundEffect::Incorrect
        };
        assert_eq!(requested, [expected], "question {}", index + 1);
        if correct {
            correct_count += 1;
        } else {
            incorrect_count += 1;
        }
        assert_eq!(app.get_screen().as_str(), "quiz");
        press(&app, "次の問題へ")?;
        if index == 9 {
            assert_eq!(app.get_screen().as_str(), "result");
            assert_eq!(
                player.take(),
                [SoundEffect::Tap, SoundEffect::RoundComplete]
            );
            app.invoke_next_question();
            assert_eq!(app.get_screen().as_str(), "result");
            assert!(
                player.take().is_empty(),
                "stale next cannot replay completion"
            );
        } else {
            assert_eq!(app.get_screen().as_str(), "quiz");
            assert_eq!(player.take(), [SoundEffect::Tap]);
        }
    }
    let result_correct = usize::try_from(app.get_result_correct())?;
    assert_eq!(correct_count, result_correct);
    assert_eq!(incorrect_count, 10 - result_correct);
    Ok(())
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
    let history = fs::read_to_string(progress.directory.join("answer-history.jsonl"))?;
    assert_eq!(history.lines().count(), 1);
    let saved_record: serde_json::Value = serde_json::from_str(
        history
            .lines()
            .next()
            .ok_or_else(|| io::Error::other("answer history is empty"))?,
    )?;
    assert!(saved_record["answered_at_ms"].as_i64().is_some());
    assert_eq!(saved_record["kind"], "flag");
    assert!(saved_record["correct_iso2"].as_str().is_some());
    assert!(saved_record["selected_iso2"].as_str().is_some());
    assert!(saved_record["hints_shown"].as_u64().is_some());
    assert!(saved_record["points"].as_u64().is_some());
    assert_eq!(saved_record["review"].as_bool(), Some(false));
    assert_eq!(saved_record["correct"].as_bool(), Some(correct_index == 0));
    assert_eq!(
        saved_record["choice_iso2"].as_array().map(Vec::len),
        Some(4)
    );
    assert!(saved_record["elapsed_ms"].as_u64().is_some());
    Ok(())
}

#[test]
fn trivia_hints_limit_and_scoring_follow_revealed_count() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let history_path = progress.directory.join("answer-history.jsonl");
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

    assert!(
        !history_path.exists(),
        "revealing hints must not record an answer"
    );

    let (first_correct, _, first_score_delta) = answer_question(&app, 0)?;
    assert_eq!(first_score_delta, i32::from(first_correct));
    assert_eq!(app.get_score(), first_score_delta);
    assert_eq!(fs::read_to_string(&history_path)?.lines().count(), 1);
    invoke(&app, "次の問題へ")?;
    assert_eq!(app.get_hints().row_count(), 1);
    invoke(&app, "次のヒントを見る")?;
    assert_eq!(app.get_hints().row_count(), 2);
    let score_before_answer = app.get_score();
    let (second_correct, _, second_score_delta) = answer_question(&app, 0)?;
    assert_eq!(second_score_delta, if second_correct { 2 } else { 0 });
    assert_eq!(app.get_score() - score_before_answer, second_score_delta);
    assert_eq!(fs::read_to_string(&history_path)?.lines().count(), 2);
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
fn map_viewport_adapts_to_tablet_and_phone() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    app.window().set_size(slint::PhysicalSize::new(1376, 1032));
    start_round(&app, "地図", "全地域")?;

    let display = map_display(&app)?;
    assert_eq!(display.accessible_role(), Some(AccessibleRole::Image));
    let flickable = element_by_id(&app, "MapViewer::map-flickable")?;
    assert!(flickable.size().width > 700.0);
    for label in ["地図を拡大", "地図を縮小", "地図をリセット"] {
        assert_inside_window(&accessible_button(&app, label)?, 1376.0, 1032.0);
    }
    assert_map_toolbar_outside_viewport(&app)?;

    app.window().set_size(slint::PhysicalSize::new(1032, 1376));
    settle();
    assert!(flickable.size().width > 900.0);
    assert_map_toolbar_outside_viewport(&app)?;
    for index in 0..app.get_choices().row_count() {
        let choice = app
            .get_choices()
            .row_data(index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing map choice"))?;
        assert_inside_window(
            &accessible_button(&app, &format!("選択肢 {}: {choice}", index + 1))?,
            1032.0,
            1376.0,
        );
    }

    app.window().set_size(slint::PhysicalSize::new(320, 700));
    settle();
    assert!(flickable.size().width < 400.0);
    assert!(
        flickable.size().width > 200.0,
        "phone map should remain substantial"
    );
    assert_inside_window(&map_display(&app)?, 320.0, 700.0);
    assert_inside_window(&accessible_button(&app, "地図を拡大")?, 320.0, 700.0);
    assert_map_toolbar_outside_viewport(&app)?;
    Ok(())
}

#[test]
fn map_zoom_pan_and_reset_clamp_to_map_bounds() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    app.window().set_size(slint::PhysicalSize::new(320, 700));
    start_round(&app, "地図", "全地域")?;

    let display = map_display(&app)?;
    let flickable = element_by_id(&app, "MapViewer::map-flickable")?;
    let image = element_by_id(&app, "MapViewer::map-image-element")?;
    for label in ["地図を拡大", "地図を縮小", "地図をリセット"] {
        assert_inside_window(&accessible_button(&app, label)?, 320.0, 700.0);
    }
    assert_map_toolbar_outside_viewport(&app)?;

    for _ in 0..5 {
        tap(&app, "地図を拡大")?;
    }
    assert!(
        display
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 300％")),
        "map zoom should stop at 300 percent when the raster budget allows"
    );
    tap(&app, "地図を拡大")?;
    assert!(
        display
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 300％")),
        "zoom-in should be disabled at the maximum scale"
    );

    let viewport_position = flickable.absolute_position();
    let viewport_size = flickable.size();
    let image_size = image.size();
    assert!((image_size.width - viewport_size.width * 3.0).abs() < 1.0);
    assert!((image_size.height - viewport_size.height * 3.0).abs() < 1.0);
    assert_image_centered_in_viewport(&flickable, &image);
    flickable.mock_drag(
        LogicalPosition::new(
            viewport_position.x + viewport_size.width / 2.0 - viewport_size.width * 3.0,
            viewport_position.y + viewport_size.height / 2.0 - viewport_size.height * 3.0,
        ),
        slint::platform::PointerEventButton::Left,
    );
    let panned_position = image.absolute_position();
    assert!(
        (panned_position.x - (viewport_position.x + viewport_size.width - image_size.width)).abs()
            < 1.0
    );
    assert!(
        (panned_position.y - (viewport_position.y + viewport_size.height - image_size.height))
            .abs()
            < 1.0
    );

    tap(&app, "地図を縮小")?;
    settle();
    assert!(
        display
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 250％")),
        "zooming out should reduce the scale"
    );
    let reduced_size = image.size();
    let reduced_position = image.absolute_position();
    assert!((reduced_size.width - viewport_size.width * 2.5).abs() < 1.0);
    assert!((reduced_size.height - viewport_size.height * 2.5).abs() < 1.0);
    assert!(
        (reduced_position.x - (viewport_position.x + viewport_size.width - reduced_size.width))
            .abs()
            < 1.0
    );
    assert!(
        (reduced_position.y - (viewport_position.y + viewport_size.height - reduced_size.height))
            .abs()
            < 1.0
    );

    tap(&app, "地図をリセット")?;
    settle();
    assert!(
        display
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 100％")),
        "reset should restore the original map scale"
    );
    let reset_position = image.absolute_position();
    assert!((reset_position.x - viewport_position.x).abs() < 1.0);
    assert!((reset_position.y - viewport_position.y).abs() < 1.0);
    assert!((image.size().width - viewport_size.width).abs() < 1.0);
    assert!((image.size().height - viewport_size.height).abs() < 1.0);
    tap(&app, "地図を縮小")?;
    assert!(
        display
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 100％")),
        "zoom-out should be disabled at the minimum scale"
    );
    Ok(())
}

#[test]
fn map_answer_preserves_view_until_next_question() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    app.window().set_size(slint::PhysicalSize::new(320, 700));
    start_round(&app, "地図", "全地域")?;

    let display = map_display(&app)?;
    let flickable = element_by_id(&app, "MapViewer::map-flickable")?;
    let image = element_by_id(&app, "MapViewer::map-image-element")?;
    tap(&app, "地図を拡大")?;
    let viewport_position_before_answer = flickable.absolute_position();
    let viewport_size_before_answer = flickable.size();
    let image_position_before_answer = image.absolute_position();
    let image_size_before_answer = image.size();
    answer_question(&app, 0)?;
    assert!(
        display
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 150％")),
        "answer image refresh must preserve the current map view"
    );
    assert!(
        (flickable.absolute_position().x - viewport_position_before_answer.x).abs() < 1.0
            && (flickable.absolute_position().y - viewport_position_before_answer.y).abs() < 1.0
            && (flickable.size().width - viewport_size_before_answer.width).abs() < 1.0
            && (flickable.size().height - viewport_size_before_answer.height).abs() < 1.0
            && (image.absolute_position().x - image_position_before_answer.x).abs() < 1.0
            && (image.absolute_position().y - image_position_before_answer.y).abs() < 1.0
            && (image.size().width - image_size_before_answer.width).abs() < 1.0
            && (image.size().height - image_size_before_answer.height).abs() < 1.0,
        "answering should preserve the map viewport and focal point"
    );
    assert_map_toolbar_outside_viewport(&app)?;
    let next = accessible_button(&app, "次の問題へ")?;
    assert_inside_window(&next, 320.0, 700.0);
    let question_number = app.get_question_number();
    next.mock_single_click(slint::platform::PointerEventButton::Left);
    settle();
    assert_eq!(app.get_question_number(), question_number + 1);
    assert!(
        display
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 100％")),
        "a new map question should start at the default view"
    );
    Ok(())
}

#[test]
fn map_zoom_respects_raster_budget_when_layout_grows() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    app.window().set_size(slint::PhysicalSize::new(1376, 800));
    start_round(&app, "地図", "全地域")?;

    let display = map_display(&app)?;
    let viewport = element_by_id(&app, "MapViewer::map-flickable")?;
    let image = element_by_id(&app, "MapViewer::map-image-element")?;
    assert_map_toolbar_outside_viewport(&app)?;
    let initial_viewport_width = viewport.size().width;
    for _ in 0..5 {
        tap(&app, "地図を拡大")?;
    }
    let zoomed_size = image.size();
    let zoomed_description = display.accessible_description().unwrap_or_default();
    assert!(
        zoomed_size.width <= 1600.0 && zoomed_size.height <= 1600.0,
        "the maximum zoom should keep the rendered map within 1600 logical pixels"
    );
    assert!(
        !zoomed_description.contains("倍率 300％"),
        "a large map should stop below 300 percent"
    );
    assert_image_centered_in_viewport(&viewport, &image);

    app.window().set_size(slint::PhysicalSize::new(1032, 1376));
    settle();
    assert!(
        viewport.size().width > initial_viewport_width,
        "the portrait layout should provide a larger map base size"
    );
    let clamped_description = display.accessible_description().unwrap_or_default();
    assert_ne!(
        clamped_description, zoomed_description,
        "growing the map base should lower the current zoom"
    );
    assert!(
        image.size().width <= 1600.0 && image.size().height <= 1600.0,
        "resizing should keep the rendered map within 1600 logical pixels"
    );
    assert_image_centered_in_viewport(&viewport, &image);
    let clamped_size = image.size();
    tap(&app, "地図を拡大")?;
    assert!(
        image.size().width <= 1600.0 && image.size().height <= 1600.0,
        "zooming after resize must respect the new map-size limit"
    );
    assert!(
        (image.size().width - clamped_size.width).abs() < 1.0,
        "zoom-in must remain disabled at the resized map limit"
    );
    assert_map_toolbar_outside_viewport(&app)?;
    Ok(())
}

#[test]
fn map_long_choices_use_full_width_single_column_tiles() -> Result<(), Box<dyn Error>> {
    const LONG_LABEL: &str = "セントビンセント及びグレナディーン諸島";
    i_slint_backend_testing::init_no_event_loop();
    let countries = data::load_countries()?;
    let country_index = countries
        .iter()
        .position(|country| country.name_ja == LONG_LABEL)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "long country label missing"))?;
    let seed = (0..1024_u64)
        .find(|seed| {
            let mut rng = fastrand::Rng::with_seed(*seed);
            quiz::generate_round(
                &countries,
                quiz::Mode::Map,
                quiz::RegionFilter::All,
                &mut rng,
                10,
            )
            .first()
            .is_some_and(|question| question.choices.contains(&country_index))
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no seed exposes long map label"))?;

    let progress = TestProgress::new()?;
    let app = test_app_with_seed(&progress, seed)?;
    app.window().set_size(slint::PhysicalSize::new(1376, 1032));
    start_round(&app, "地図", "全地域")?;
    let choices = app.get_choices();
    let long_choice_index = (0..choices.row_count())
        .find(|index| choices.row_data(*index).as_deref() == Some(LONG_LABEL))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "long label not in map choices"))?;
    let mut previous_x: Option<f32> = None;
    let mut previous_bottom: Option<f32> = None;
    for index in 0..choices.row_count() {
        let choice = choices
            .row_data(index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing map choice"))?;
        let tile = accessible_button(&app, &format!("選択肢 {}: {choice}", index + 1))?;
        assert_inside_window(&tile, 1376.0, 1032.0);
        let position = tile.absolute_position();
        let size = tile.size();
        assert!((size.width - 300.0).abs() < 1.0);
        assert!((size.height - 48.0).abs() < 1.0);
        if let Some(x) = previous_x {
            assert!((position.x - x).abs() < 1.0);
        }
        if let Some(bottom) = previous_bottom {
            assert!((position.y - bottom - 8.0).abs() < 1.0);
        }
        previous_x = Some(position.x);
        previous_bottom = Some(position.y + size.height);
    }
    let long_tile = accessible_button(
        &app,
        &format!("選択肢 {}: {LONG_LABEL}", long_choice_index + 1),
    )?;
    let viewport = element_by_id(&app, "MapViewer::map-flickable")?;
    let image = element_by_id(&app, "MapViewer::map-image-element")?;
    let viewport_position_before_answer = viewport.absolute_position();
    let viewport_size_before_answer = viewport.size();
    let image_position_before_answer = image.absolute_position();
    let image_size_before_answer = image.size();
    assert_map_toolbar_outside_viewport(&app)?;
    long_tile.mock_single_click(slint::platform::PointerEventButton::Left);
    assert!(app.get_answered());
    let next = accessible_button(&app, "次の問題へ")?;
    assert_inside_window(&next, 1376.0, 1032.0);
    assert_map_toolbar_outside_viewport(&app)?;
    assert!(
        (viewport.size().width - viewport_size_before_answer.width).abs() < 1.0
            && (viewport.size().height - viewport_size_before_answer.height).abs() < 1.0
            && (viewport.absolute_position().x - viewport_position_before_answer.x).abs() < 1.0
            && (viewport.absolute_position().y - viewport_position_before_answer.y).abs() < 1.0
            && (image.size().width - image_size_before_answer.width).abs() < 1.0
            && (image.size().height - image_size_before_answer.height).abs() < 1.0
            && (image.absolute_position().x - image_position_before_answer.x).abs() < 1.0
            && (image.absolute_position().y - image_position_before_answer.y).abs() < 1.0,
        "answering a long-label question should preserve the displayed map bounds"
    );
    Ok(())
}

#[test]
fn map_answer_and_next_are_touchable_on_phone_widths() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "地図", "全地域")?;

    for (width, height) in [(320_u16, 568_u16), (320, 700), (375, 667)] {
        app.window().set_size(slint::PhysicalSize::new(
            u32::from(width),
            u32::from(height),
        ));
        settle();
        let window_width = f32::from(width);
        let window_height = f32::from(height);
        let display = map_display(&app)?;
        assert_inside_window(&display, window_width, window_height);
        assert!(display.size().width > 0.0 && display.size().height > 0.0);
        assert_map_toolbar_outside_viewport(&app)?;
        for label in ["地図を拡大", "地図を縮小", "地図をリセット"] {
            assert_inside_window(
                &accessible_button(&app, label)?,
                window_width,
                window_height,
            );
        }
        for index in 0..app.get_choices().row_count() {
            let choice = app
                .get_choices()
                .row_data(index)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing map choice"))?;
            assert_inside_window(
                &accessible_button(&app, &format!("選択肢 {}: {choice}", index + 1))?,
                window_width,
                window_height,
            );
        }

        tap(&app, "地図を拡大")?;
        let display = map_display(&app)?;
        assert!(
            display
                .accessible_description()
                .is_some_and(|description| description.contains("倍率 150％")),
            "zoom control should update the displayed scale"
        );
        let choice = app.get_choices().row_data(0).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "missing first map choice")
        })?;
        let answer_label = format!("選択肢 1: {choice}");
        let answer = accessible_button(&app, &answer_label)?;
        assert_inside_window(&answer, window_width, window_height);
        let viewport = element_by_id(&app, "MapViewer::map-flickable")?;
        let image = element_by_id(&app, "MapViewer::map-image-element")?;
        let viewport_position_before_answer = viewport.absolute_position();
        let viewport_size_before_answer = viewport.size();
        let image_position_before_answer = image.absolute_position();
        let image_size_before_answer = image.size();
        assert!(
            viewport_size_before_answer.width > 100.0,
            "the smallest phone layout should leave room for the map after controls"
        );
        answer.mock_single_click(slint::platform::PointerEventButton::Left);
        assert!(app.get_answered(), "map answer should respond to a touch");
        assert!(
            (viewport.size().width - viewport_size_before_answer.width).abs() < 1.0
                && (viewport.size().height - viewport_size_before_answer.height).abs() < 1.0
                && (viewport.absolute_position().x - viewport_position_before_answer.x).abs() < 1.0
                && (viewport.absolute_position().y - viewport_position_before_answer.y).abs() < 1.0
                && (image.size().width - image_size_before_answer.width).abs() < 1.0
                && (image.size().height - image_size_before_answer.height).abs() < 1.0
                && (image.absolute_position().x - image_position_before_answer.x).abs() < 1.0
                && (image.absolute_position().y - image_position_before_answer.y).abs() < 1.0,
            "answering should preserve the map viewport and focal point"
        );
        assert_map_toolbar_outside_viewport(&app)?;
        assert!(
            display
                .accessible_description()
                .is_some_and(|description| description.contains("倍率 150％")),
            "revealing the answer must preserve the current map view"
        );

        let next = accessible_button(&app, "次の問題へ")?;
        assert_inside_window(&next, window_width, window_height);
        let question_number = app.get_question_number();
        next.mock_single_click(slint::platform::PointerEventButton::Left);
        settle();
        assert_eq!(app.get_question_number(), question_number + 1);
        assert!(
            display
                .accessible_description()
                .is_some_and(|description| description.contains("倍率 100％")),
            "next question should reset the map view"
        );
    }
    Ok(())
}

// Surface Duo single screen (portrait, portrait minus system bars, landscape),
// Duo spanned across both screens, a small phone, a phone, a tablet, and a desktop.
const SCREEN_SIZES: [(u16, u16); 9] = [
    (540, 720),
    (540, 648),
    (720, 540),
    (1114, 720),
    (720, 1114),
    (320, 568),
    (390, 844),
    (768, 1024),
    (1280, 800),
];

#[test]
fn start_round_button_is_touchable_on_duo_and_other_screens() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    for (width, height) in SCREEN_SIZES {
        let progress = TestProgress::new()?;
        let app = test_app(&progress)?;
        app.window().set_size(slint::PhysicalSize::new(
            u32::from(width),
            u32::from(height),
        ));
        settle();
        tap(&app, "クイズを始める")?;
        settle();
        let start = accessible_button(&app, "10問を始める")?;
        assert_inside_window(&start, f32::from(width), f32::from(height));
        start.mock_single_click(PointerEventButton::Left);
        assert_eq!(
            app.get_screen().as_str(),
            "quiz",
            "10問を始める should start a round at {width}x{height}"
        );
    }
    Ok(())
}

#[test]
fn home_actions_are_touchable_on_duo_and_other_screens() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let first_app = test_app(&progress)?;
    start_round(&first_app, "国旗", "全地域")?;
    finish_flag_round(&first_app)?;
    let review_label = format!("まちがえた国を復習、{}問", first_app.get_review_count());
    assert!(
        first_app.get_review_count() > 0,
        "the review tile must be enabled"
    );
    drop(first_app);

    for (width, height) in SCREEN_SIZES {
        for (label, screen) in [
            ("クイズを始める", "select"),
            (review_label.as_str(), "quiz"),
            ("回答履歴を見る", "history"),
            ("このアプリについて", "about"),
        ] {
            let app = test_app(&progress)?;
            app.window().set_size(slint::PhysicalSize::new(
                u32::from(width),
                u32::from(height),
            ));
            settle();
            let button = accessible_button(&app, label)?;
            assert_inside_window(&button, f32::from(width), f32::from(height));
            button.mock_single_click(PointerEventButton::Left);
            assert_eq!(
                app.get_screen().as_str(),
                screen,
                "{label} should open {screen} at {width}x{height}"
            );
        }
    }
    Ok(())
}

#[test]
fn map_review_round_starts_at_default_zoom() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "地図", "全地域")?;
    let display = map_display(&app)?;
    tap(&app, "地図を拡大")?;

    let countries = data::load_countries()?;
    let mut rng = fastrand::Rng::with_seed(TEST_SEED);
    let questions = quiz::generate_round(
        &countries,
        quiz::Mode::Map,
        quiz::RegionFilter::All,
        &mut rng,
        10,
    );
    let question_count = usize::try_from(app.get_total_questions())?;
    assert_eq!(questions.len(), question_count);
    for (index, question) in questions.iter().enumerate() {
        let selected_index = usize::from(question.answer == 0);
        let (correct, _, _) = answer_question(&app, selected_index)?;
        assert!(!correct, "map review setup should miss each question");
        if index == 0 {
            assert!(
                display
                    .accessible_description()
                    .is_some_and(|description| description.contains("倍率 150％")),
                "answering must not reset a map view"
            );
        }
        invoke(&app, "次の問題へ")?;
        settle();
        if index + 1 < question_count {
            assert!(
                display
                    .accessible_description()
                    .is_some_and(|description| description.contains("倍率 100％")),
                "each new map question should reset its view"
            );
        }
    }

    let review_progress = progress::Progress::load(&progress.file);
    let review_questions = quiz::generate_review(&countries, &review_progress, &mut rng, 10);

    assert_eq!(app.get_screen().as_str(), "result");
    invoke(&app, "ホームへ戻る")?;
    invoke(
        &app,
        &format!("まちがえた国を復習、{}問", app.get_review_count()),
    )?;
    assert_eq!(app.get_screen().as_str(), "quiz");
    assert_eq!(app.get_kind().as_str(), "map");
    assert!(
        map_display(&app)?
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 100％")),
        "a fresh map review round should begin at the default view"
    );
    let review_question_count = usize::try_from(app.get_total_questions())?;
    assert_eq!(review_questions.len(), review_question_count);
    tap(&app, "地図を拡大")?;
    for question in &review_questions {
        answer_question(&app, question.answer)?;
        invoke(&app, "次の問題へ")?;
        settle();
    }
    assert_eq!(app.get_screen().as_str(), "result");
    invoke(&app, "ホームへ戻る")?;
    start_round(&app, "地図", "全地域")?;
    assert!(
        map_display(&app)?
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 100％")),
        "a new map round should begin at the default view"
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
    invoke(
        &app,
        &format!("まちがえた国を復習、{}問", app.get_review_count()),
    )?;
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

#[test]
fn about_pages_show_privacy_and_license_notices() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;

    invoke(&app, "このアプリについて")?;
    assert_eq!(app.get_screen().as_str(), "about");
    invoke(&app, "プライバシーポリシー")?;
    assert_eq!(app.get_screen().as_str(), "privacy");
    assert!(
        app.get_privacy_text()
            .contains("本アプリは個人情報を収集しません")
    );
    assert!(app.get_privacy_text().contains("answer-history.jsonl"));

    invoke(&app, "戻る")?;
    assert_eq!(app.get_screen().as_str(), "about");
    invoke(&app, "ライセンス")?;
    assert_eq!(app.get_screen().as_str(), "licenses");
    assert!(
        app.get_flag_license_text()
            .contains("The MIT License (MIT)")
    );
    assert!(
        app.get_font_license_text()
            .contains("SIL OPEN FONT LICENSE Version 1.1")
    );

    invoke(&app, "戻る")?;
    assert_eq!(app.get_screen().as_str(), "about");
    Ok(())
}

fn history_row_count(progress: &TestProgress) -> io::Result<usize> {
    match fs::read_to_string(progress.directory.join("answer-history.jsonl")) {
        Ok(history) => Ok(history.lines().count()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

#[test]
fn history_opens_with_an_empty_state() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;

    invoke(&app, "回答履歴を見る")?;
    assert_eq!(app.get_screen().as_str(), "history");
    assert!(app.get_history_empty());
    assert_eq!(app.get_history_items().row_count(), 0);
    invoke(&app, "ホーム")?;
    assert_eq!(app.get_screen().as_str(), "home");
    Ok(())
}

#[test]
fn history_pages_keep_all_answers_reachable() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "国旗", "全地域")?;
    let _missed = finish_flag_round(&app)?;
    assert_eq!(app.get_screen().as_str(), "result");
    invoke(&app, "ホームへ戻る")?;
    invoke(&app, "回答履歴を見る")?;

    assert!(!app.get_history_empty());
    let newest = app
        .get_history_items()
        .row_data(0)
        .ok_or_else(|| io::Error::other("newest history row is missing"))?;
    assert_ne!(newest.timestamp.as_str(), "");
    assert!(newest.outcome.as_str().contains("国旗"));
    assert!(newest.countries.as_str().contains("正解"));
    assert!(newest.details.as_str().contains("回答時間"));
    assert_eq!(app.get_history_items().row_count(), 8);
    assert_eq!(app.get_history_page_label().as_str(), "1 / 2");
    assert!(app.get_history_has_older());
    invoke(&app, "古い記録")?;
    assert_eq!(app.get_history_items().row_count(), 2);
    assert_eq!(app.get_history_page_label().as_str(), "2 / 2");
    assert!(app.get_history_has_newer());
    assert!(!app.get_history_has_older());
    invoke(&app, "新しい記録")?;
    assert_eq!(app.get_history_items().row_count(), 8);
    assert_eq!(app.get_history_page_label().as_str(), "1 / 2");
    Ok(())
}

#[test]
fn history_append_failure_is_visible_without_losing_progress() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    fs::create_dir(progress.directory.join("answer-history.jsonl"))?;
    let app = test_app(&progress)?;
    start_round(&app, "国旗", "全地域")?;
    answer_question(&app, 0)?;

    assert!(
        app.get_storage_warning()
            .contains("回答履歴を保存できませんでした")
    );
    assert!(progress.file.is_file(), "SRS progress must still be saved");
    Ok(())
}

#[test]
fn malformed_history_warns_and_keeps_valid_rows() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let history_path = progress.directory.join("answer-history.jsonl");
    let valid_row = r#"{"answered_at_ms":1000,"kind":"flag","correct_iso2":"JP","selected_iso2":"US","correct":false,"hints_shown":0,"points":0,"elapsed_ms":321,"choice_iso2":["JP","US","FR","CN"],"review":false}"#;
    let original = format!("{valid_row}\n{{\"incomplete\":");
    fs::write(&history_path, original.as_bytes())?;
    let app = test_app(&progress)?;

    invoke(&app, "回答履歴を見る")?;
    assert_eq!(app.get_history_items().row_count(), 1);
    assert!(
        app.get_storage_warning()
            .contains("履歴ファイルの一部を読み込めませんでした")
    );
    assert_eq!(fs::read(&history_path)?.as_slice(), original.as_bytes());
    Ok(())
}

#[test]
fn redundant_next_keeps_the_current_question_timer_running() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "国旗", "全地域")?;

    std::thread::sleep(Duration::from_millis(40));
    app.invoke_next_question();
    answer_question(&app, 0)?;

    let history = fs::read_to_string(progress.directory.join("answer-history.jsonl"))?;
    let saved_record: serde_json::Value = serde_json::from_str(
        history
            .lines()
            .next()
            .ok_or_else(|| io::Error::other("answer history is empty"))?,
    )?;
    let elapsed_ms = saved_record["elapsed_ms"]
        .as_u64()
        .ok_or_else(|| io::Error::other("elapsed time is missing"))?;
    assert!(
        elapsed_ms >= 40,
        "redundant next must not reset elapsed time, got {elapsed_ms} ms"
    );
    Ok(())
}

#[test]
fn all_malformed_history_shows_an_error_without_modifying_the_file() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let history_path = progress.directory.join("answer-history.jsonl");
    let original = b"{malformed}\n[]\n";
    fs::write(&history_path, original)?;
    let app = test_app(&progress)?;

    invoke(&app, "回答履歴を見る")?;
    assert_eq!(app.get_screen().as_str(), "history");
    assert!(app.get_history_empty());
    assert_eq!(app.get_history_items().row_count(), 0);
    assert!(
        ElementHandle::find_by_accessible_label(
            &app,
            "履歴を表示できませんでした（元データは保持しています）"
        )
        .next()
        .is_some(),
        "unreadable history should show an error instead of the welcome message"
    );
    assert!(
        app.get_storage_warning()
            .contains("履歴ファイルの一部を読み込めませんでした")
    );
    assert_eq!(fs::read(&history_path)?.as_slice(), original);
    Ok(())
}

#[test]
fn history_persists_accepted_answers_and_ignores_quit_events() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "国旗", "全地域")?;

    invoke(&app, "クイズを中断")?;
    app.invoke_answer(0);
    app.invoke_next_question();
    assert!(!app.get_answered());
    assert_eq!(history_row_count(&progress)?, 0);
    assert!(!progress.file.exists());
    invoke(&app, "続ける")?;

    let correct_index = first_flag_answer_index()?;
    let choice_count = app.get_choices().row_count();
    let wrong_index = (correct_index + 1) % choice_count;
    let (correct, _, _) = answer_question(&app, wrong_index)?;
    assert!(!correct);
    assert_eq!(history_row_count(&progress)?, 1);
    assert_eq!(app.get_review_count(), 1);

    invoke(&app, "クイズを中断")?;
    invoke(&app, "ホームへ戻る")?;
    assert_eq!(app.get_screen().as_str(), "home");
    assert_eq!(app.get_review_count(), 1);
    invoke(&app, "回答履歴を見る")?;
    assert_eq!(app.get_history_items().row_count(), 1);
    assert_eq!(
        progress::Progress::load(&progress.file).review_keys().len(),
        1
    );

    drop(app);
    let app = test_app(&progress)?;
    assert_eq!(app.get_review_count(), 1);
    invoke(&app, "回答履歴を見る")?;
    assert_eq!(app.get_history_items().row_count(), 1);
    invoke(&app, "ホーム")?;

    start_round(&app, "3ヒント", "全地域")?;
    invoke(&app, "次のヒントを見る")?;
    let shown_hints = app.get_hints().row_count();
    invoke(&app, "クイズを中断")?;
    app.invoke_answer(0);
    app.invoke_next_question();
    app.invoke_reveal_hint();
    assert!(!app.get_answered());
    assert_eq!(app.get_hints().row_count(), shown_hints);
    assert_eq!(history_row_count(&progress)?, 1);
    invoke(&app, "ホームへ戻る")?;
    app.invoke_answer(0);
    app.invoke_next_question();
    app.invoke_reveal_hint();
    invoke(&app, "回答履歴を見る")?;
    assert_eq!(app.get_history_items().row_count(), 1);
    assert_eq!(history_row_count(&progress)?, 1);
    assert_eq!(
        progress::Progress::load(&progress.file).review_keys().len(),
        1
    );
    Ok(())
}

#[test]
fn quit_confirmation_cancel_preserves_quiz_state_and_guards_actions() -> Result<(), Box<dyn Error>>
{
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "3ヒント", "全地域")?;
    invoke(&app, "次のヒントを見る")?;

    let question_number = app.get_question_number();
    let hints = app.get_hints().row_count();
    let score = app.get_score();
    let review_count = app.get_review_count();
    assert_eq!(hints, 2);
    assert!(!app.get_answered());

    invoke(&app, "クイズを中断")?;
    assert!(app.get_quit_confirmation_open());
    let first_choice = app.get_choices().row_data(0).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "choice index is out of range")
    })?;
    let first_tile_label = format!("選択肢 1: {first_choice}");
    let first_tile = accessible_button(&app, &first_tile_label)?;
    assert_eq!(first_tile.accessible_enabled(), Some(false));
    first_tile.invoke_accessible_default_action();
    assert!(!app.get_answered());
    app.invoke_answer(0);
    app.invoke_reveal_hint();
    app.invoke_next_question();
    assert_eq!(app.get_question_number(), question_number);
    assert_eq!(app.get_hints().row_count(), hints);
    assert!(!app.get_answered());
    assert_eq!(app.get_score(), score);
    assert_eq!(app.get_review_count(), review_count);

    invoke(&app, "続ける")?;
    assert!(!app.get_quit_confirmation_open());
    let first_tile_after_cancel = accessible_button(&app, &first_tile_label)?;
    assert_eq!(first_tile_after_cancel.accessible_enabled(), Some(true));
    assert_eq!(app.get_question_number(), question_number);
    assert_eq!(app.get_hints().row_count(), hints);
    assert!(!app.get_answered());
    assert_eq!(app.get_score(), score);

    let choice_count = app.get_choices().row_count();
    let correct_index = first_trivia_answer_index()?;
    let wrong_index = (correct_index + 1) % choice_count;
    let (correct, _, _) = answer_question(&app, wrong_index)?;
    assert!(!correct);
    let selected_index = app.get_selected_index();
    let answered_score = app.get_score();
    let answered_review_count = app.get_review_count();
    let answered_hints = app.get_hints().row_count();

    invoke(&app, "クイズを中断")?;
    app.invoke_answer(i32::try_from(correct_index)?);
    app.invoke_reveal_hint();
    app.invoke_next_question();
    assert_eq!(app.get_question_number(), question_number);
    assert_eq!(app.get_hints().row_count(), answered_hints);
    assert!(app.get_answered());
    assert_eq!(app.get_selected_index(), selected_index);
    assert_eq!(app.get_score(), answered_score);
    assert_eq!(app.get_review_count(), answered_review_count);

    invoke(&app, "続ける")?;
    assert_eq!(app.get_question_number(), question_number);
    assert_eq!(app.get_hints().row_count(), answered_hints);
    assert!(app.get_answered());
    assert_eq!(app.get_selected_index(), selected_index);
    assert_eq!(app.get_score(), answered_score);
    assert_eq!(app.get_review_count(), answered_review_count);
    Ok(())
}

#[test]
fn confirmed_quit_keeps_progress_and_stale_callbacks_cannot_open_results()
-> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "3ヒント", "全地域")?;
    invoke(&app, "次のヒントを見る")?;
    let first_correct = first_trivia_answer_index()?;
    let first_wrong = (first_correct + 1) % app.get_choices().row_count();
    let (correct, _, _) = answer_question(&app, first_wrong)?;
    assert!(!correct);
    assert_eq!(app.get_review_count(), 1);

    invoke(&app, "次の問題へ")?;
    invoke(&app, "次のヒントを見る")?;
    assert_eq!(app.get_question_number(), 2);
    assert_eq!(app.get_hints().row_count(), 2);
    assert!(!app.get_answered());
    invoke(&app, "クイズを中断")?;
    invoke(&app, "ホームへ戻る")?;
    assert_eq!(app.get_screen().as_str(), "home");
    assert!(!app.get_quit_confirmation_open());
    assert_eq!(app.get_review_count(), 1);

    app.invoke_answer(0);
    app.invoke_reveal_hint();
    app.invoke_next_question();
    assert_eq!(app.get_screen().as_str(), "home");
    assert_eq!(app.get_review_count(), 1);
    let saved_app = test_app(&progress)?;
    assert_eq!(saved_app.get_review_count(), 1);

    invoke(
        &saved_app,
        &format!("まちがえた国を復習、{}問", saved_app.get_review_count()),
    )?;
    assert_eq!(saved_app.get_screen().as_str(), "quiz");
    assert_eq!(saved_app.get_kind().as_str(), "trivia");
    invoke(&saved_app, "次のヒントを見る")?;
    let review_question_number = saved_app.get_question_number();
    let review_hints = saved_app.get_hints().row_count();
    let review_score = saved_app.get_score();
    invoke(&saved_app, "クイズを中断")?;
    saved_app.invoke_answer(0);
    saved_app.invoke_next_question();
    assert_eq!(saved_app.get_question_number(), review_question_number);
    assert_eq!(saved_app.get_hints().row_count(), review_hints);
    assert!(!saved_app.get_answered());
    invoke(&saved_app, "続ける")?;
    assert_eq!(saved_app.get_question_number(), review_question_number);
    assert_eq!(saved_app.get_hints().row_count(), review_hints);
    assert_eq!(saved_app.get_score(), review_score);

    let correct_index = first_review_answer_index(&progress)?;
    let (correct, _, _) = answer_question(&saved_app, correct_index)?;
    assert!(correct);
    let review_count_after_answer = saved_app.get_review_count();
    assert_eq!(review_count_after_answer, 0);
    invoke(&saved_app, "クイズを中断")?;
    invoke(&saved_app, "ホームへ戻る")?;
    assert_eq!(saved_app.get_screen().as_str(), "home");
    assert_eq!(saved_app.get_review_count(), review_count_after_answer);
    saved_app.invoke_answer(0);
    saved_app.invoke_reveal_hint();
    saved_app.invoke_next_question();
    assert_eq!(saved_app.get_screen().as_str(), "home");

    let reloaded_app = test_app(&progress)?;
    assert_eq!(reloaded_app.get_review_count(), review_count_after_answer);
    start_round(&saved_app, "3ヒント", "全地域")?;
    assert_eq!(saved_app.get_screen().as_str(), "quiz");
    assert!(!saved_app.get_quit_confirmation_open());
    assert_eq!(saved_app.get_question_number(), 1);
    assert_eq!(saved_app.get_score(), 0);
    assert!(!saved_app.get_answered());
    assert_eq!(saved_app.get_hints().row_count(), 1);
    Ok(())
}

#[test]
fn map_quit_confirmation_blocks_controls_and_preserves_view() -> Result<(), Box<dyn Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "地図", "全地域")?;

    tap(&app, "地図を拡大")?;
    tap(&app, "地図を拡大")?;
    let display = map_display(&app)?;
    let viewport = element_by_id(&app, "MapViewer::map-flickable")?;
    let image = element_by_id(&app, "MapViewer::map-image-element")?;
    let viewport_position = viewport.absolute_position();
    let viewport_size = viewport.size();
    let image_size = image.size();
    viewport.mock_drag(
        LogicalPosition::new(
            viewport_position.x + viewport_size.width / 2.0 - viewport_size.width * 3.0,
            viewport_position.y + viewport_size.height / 2.0 - viewport_size.height * 3.0,
        ),
        slint::platform::PointerEventButton::Left,
    );
    let panned_position = image.absolute_position();
    assert!(
        (panned_position.x - viewport_position.x).abs() > 1.0
            && (panned_position.y - viewport_position.y).abs() > 1.0,
        "the setup should move the zoomed map away from its origin"
    );
    let zoom_description = display.accessible_description().unwrap_or_default();
    assert!(zoom_description.contains("倍率 200％"));
    let question_number = app.get_question_number();
    let score = app.get_score();
    let review_count = app.get_review_count();

    invoke(&app, "クイズを中断")?;
    assert!(app.get_quit_confirmation_open());
    for label in ["地図を拡大", "地図を縮小", "地図をリセット"] {
        let control = accessible_button(&app, label)?;
        assert_eq!(control.accessible_enabled(), Some(false), "{label}");
        control.invoke_accessible_default_action();
        control.mock_single_click(slint::platform::PointerEventButton::Left);
    }
    let choices = app.get_choices();
    for index in 0..choices.row_count() {
        let choice = choices
            .row_data(index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing map choice"))?;
        let tile = accessible_button(&app, &format!("選択肢 {}: {choice}", index + 1))?;
        assert_eq!(tile.accessible_enabled(), Some(false), "choice {index}");
        tile.invoke_accessible_default_action();
        tile.mock_single_click(slint::platform::PointerEventButton::Left);
    }

    viewport.mock_drag(
        LogicalPosition::new(
            viewport_position.x + viewport_size.width / 2.0 + viewport_size.width * 3.0,
            viewport_position.y + viewport_size.height / 2.0 + viewport_size.height * 3.0,
        ),
        slint::platform::PointerEventButton::Left,
    );
    app.invoke_answer(0);
    app.invoke_next_question();
    assert!(!app.get_answered());
    assert_eq!(app.get_question_number(), question_number);
    assert_eq!(app.get_score(), score);
    assert_eq!(app.get_review_count(), review_count);
    assert_eq!(
        display.accessible_description().unwrap_or_default(),
        zoom_description
    );
    assert!(
        (image.absolute_position().x - panned_position.x).abs() < 1.0
            && (image.absolute_position().y - panned_position.y).abs() < 1.0
            && (image.size().width - image_size.width).abs() < 1.0
            && (image.size().height - image_size.height).abs() < 1.0
            && (viewport.absolute_position().x - viewport_position.x).abs() < 1.0
            && (viewport.absolute_position().y - viewport_position.y).abs() < 1.0
            && (viewport.size().width - viewport_size.width).abs() < 1.0
            && (viewport.size().height - viewport_size.height).abs() < 1.0,
        "blocked actions should leave the zoomed and panned map unchanged"
    );
    Ok(())
}

#[test]
fn map_quit_confirmation_cancel_and_restart_preserve_and_reset_view() -> Result<(), Box<dyn Error>>
{
    i_slint_backend_testing::init_no_event_loop();
    let progress = TestProgress::new()?;
    let app = test_app(&progress)?;
    start_round(&app, "地図", "全地域")?;

    tap(&app, "地図を拡大")?;
    tap(&app, "地図を拡大")?;
    let display = map_display(&app)?;
    let viewport = element_by_id(&app, "MapViewer::map-flickable")?;
    let image = element_by_id(&app, "MapViewer::map-image-element")?;
    let viewport_position = viewport.absolute_position();
    let viewport_size = viewport.size();
    viewport.mock_drag(
        LogicalPosition::new(
            viewport_position.x + viewport_size.width / 2.0 - viewport_size.width * 3.0,
            viewport_position.y + viewport_size.height / 2.0 - viewport_size.height * 3.0,
        ),
        slint::platform::PointerEventButton::Left,
    );
    let panned_position = image.absolute_position();
    assert!(
        (panned_position.x - viewport_position.x).abs() > 1.0
            && (panned_position.y - viewport_position.y).abs() > 1.0,
        "the setup should move the zoomed map away from its origin"
    );
    let zoom_description = display.accessible_description().unwrap_or_default();
    assert!(zoom_description.contains("倍率 200％"));
    let question_number = app.get_question_number();

    invoke(&app, "クイズを中断")?;
    let choices = app.get_choices();
    invoke(&app, "続ける")?;
    assert!(!app.get_quit_confirmation_open());
    for label in ["地図を拡大", "地図を縮小", "地図をリセット"] {
        assert_eq!(
            accessible_button(&app, label)?.accessible_enabled(),
            Some(true),
            "{label} should be re-enabled after cancel"
        );
    }
    for index in 0..choices.row_count() {
        let choice = choices
            .row_data(index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing map choice"))?;
        assert_eq!(
            accessible_button(&app, &format!("選択肢 {}: {choice}", index + 1))?
                .accessible_enabled(),
            Some(true),
            "choice {index} should be re-enabled after cancel"
        );
    }
    assert_eq!(
        display.accessible_description().unwrap_or_default(),
        zoom_description
    );
    assert!(
        (image.absolute_position().x - panned_position.x).abs() < 1.0
            && (image.absolute_position().y - panned_position.y).abs() < 1.0,
        "cancel should preserve the panned map position"
    );

    answer_question(&app, 0)?;
    assert!(app.get_answered());
    invoke(&app, "クイズを中断")?;
    let next = accessible_button(&app, "次の問題へ")?;
    assert_eq!(next.accessible_enabled(), Some(false));
    next.invoke_accessible_default_action();
    assert_eq!(app.get_question_number(), question_number);
    assert!(app.get_answered());
    invoke(&app, "続ける")?;
    assert_eq!(
        accessible_button(&app, "次の問題へ")?.accessible_enabled(),
        Some(true)
    );

    invoke(&app, "クイズを中断")?;
    invoke(&app, "ホームへ戻る")?;
    assert_eq!(app.get_screen().as_str(), "home");
    assert!(!app.get_quit_confirmation_open());
    start_round(&app, "地図", "全地域")?;
    let new_display = map_display(&app)?;
    let new_viewport = element_by_id(&app, "MapViewer::map-flickable")?;
    let new_image = element_by_id(&app, "MapViewer::map-image-element")?;
    assert!(
        new_display
            .accessible_description()
            .is_some_and(|description| description.contains("倍率 100％")),
        "a new map round should reset zoom"
    );
    assert!(
        (new_image.absolute_position().x - new_viewport.absolute_position().x).abs() < 1.0
            && (new_image.absolute_position().y - new_viewport.absolute_position().y).abs() < 1.0
            && (new_image.size().width - new_viewport.size().width).abs() < 1.0
            && (new_image.size().height - new_viewport.size().height).abs() < 1.0,
        "a new map round should reset pan and scale"
    );
    Ok(())
}
