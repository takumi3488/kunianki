//! UI-independent state and progression for one quiz round.

use crate::quiz::{Question, QuestionKind, trivia_points};

/// The progress information produced by one accepted answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnswerOutcome {
    country_index: usize,
    kind: QuestionKind,
    correct: bool,
}

impl AnswerOutcome {
    /// Returns the country index associated with the answered question.
    #[must_use]
    pub const fn country_index(self) -> usize {
        self.country_index
    }

    /// Returns the kind of question that was answered.
    #[must_use]
    pub const fn kind(self) -> QuestionKind {
        self.kind
    }

    /// Returns whether the selected choice was correct.
    #[must_use]
    pub const fn correct(self) -> bool {
        self.correct
    }
}

/// The result of moving past an answered question.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Next {
    /// The next question is now current.
    Question,
    /// The round has no more questions.
    Finished,
}

/// Tracks question progression and scoring for a single quiz round.
#[derive(Debug, Default)]
pub struct Session {
    questions: Vec<Question>,
    question_index: usize,
    score: u32,
    correct_count: usize,
    missed_country_indices: Vec<usize>,
    selected_index: Option<usize>,
    hints_shown: u8,
}

impl Session {
    /// Starts a round, replacing any existing questions and resetting its progress.
    pub fn start(&mut self, questions: Vec<Question>) {
        self.questions = questions;
        self.question_index = 0;
        self.score = 0;
        self.correct_count = 0;
        self.missed_country_indices.clear();
        self.selected_index = None;
        self.hints_shown = u8::from(
            self.question()
                .is_some_and(|question| question.kind == QuestionKind::Trivia),
        );
    }

    /// Returns the current question, if the round has one.
    #[must_use]
    pub fn question(&self) -> Option<&Question> {
        self.questions.get(self.question_index)
    }

    /// Returns the zero-based index of the current question.
    #[must_use]
    pub const fn question_index(&self) -> usize {
        self.question_index
    }

    /// Returns the number of questions in the round.
    #[must_use]
    pub fn question_count(&self) -> usize {
        self.questions.len()
    }

    /// Returns the round's accumulated score.
    #[must_use]
    pub const fn score(&self) -> u32 {
        self.score
    }

    /// Returns the number of correctly answered questions.
    #[must_use]
    pub const fn correct_count(&self) -> usize {
        self.correct_count
    }

    /// Returns country indices missed so far, in answer order.
    #[must_use]
    pub fn missed_country_indices(&self) -> &[usize] {
        &self.missed_country_indices
    }

    /// Returns whether the current question has been answered.
    #[must_use]
    pub const fn answered(&self) -> bool {
        self.selected_index.is_some()
    }

    /// Returns the selected choice index, if the current question was answered.
    #[must_use]
    pub const fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    /// Returns the number of hints shown for the current trivia question.
    #[must_use]
    pub const fn hints_shown(&self) -> u8 {
        self.hints_shown
    }

    /// Records a valid answer once, returning the progress data for the caller.
    pub fn answer(&mut self, choice: usize) -> Option<AnswerOutcome> {
        let question = self.question()?;
        if self.answered() || question.choices.get(choice).is_none() {
            return None;
        }

        let country_index = question.country;
        let kind = question.kind;
        let correct = choice == question.answer;
        let points = if correct {
            match kind {
                QuestionKind::Flag | QuestionKind::Map => 1,
                QuestionKind::Trivia => trivia_points(self.hints_shown),
            }
        } else {
            0
        };

        if correct {
            self.score += points;
            self.correct_count += 1;
        } else {
            self.missed_country_indices.push(country_index);
        }
        self.selected_index = Some(choice);

        Some(AnswerOutcome {
            country_index,
            kind,
            correct,
        })
    }

    /// Reveals another hint when the current unanswered question is trivia.
    pub fn reveal_hint(&mut self) -> bool {
        let Some(question) = self.question() else {
            return false;
        };
        if question.kind != QuestionKind::Trivia || self.answered() || self.hints_shown >= 3 {
            return false;
        }
        self.hints_shown += 1;
        true
    }

    /// Advances after an answer, or reports that no question remains.
    pub fn next(&mut self) -> Next {
        if self.question().is_none() {
            return Next::Finished;
        }
        if !self.answered() {
            return Next::Question;
        }
        if self.question_index + 1 >= self.questions.len() {
            return Next::Finished;
        }

        self.question_index += 1;
        self.selected_index = None;
        self.hints_shown = u8::from(
            self.question()
                .is_some_and(|question| question.kind == QuestionKind::Trivia),
        );
        Next::Question
    }
}

#[cfg(test)]
mod tests {
    use super::{Next, Session};
    use crate::quiz::{Question, QuestionKind};
    use std::error::Error;

    fn question(country: usize, kind: QuestionKind) -> Question {
        Question {
            country,
            kind,
            choices: [0, 1, 2, 3],
            answer: 2,
        }
    }

    #[test]
    fn double_answer_is_ignored_after_first_answer() {
        let mut session = Session::default();
        session.start(vec![question(8, QuestionKind::Flag)]);

        assert!(session.answer(2).is_some());
        assert!(session.answer(2).is_none(), "second answer must be ignored");

        assert_eq!(session.score(), 1);
        assert_eq!(session.correct_count(), 1);
    }

    #[test]
    fn out_of_range_choice_is_ignored() {
        let mut session = Session::default();
        session.start(vec![question(8, QuestionKind::Flag)]);

        assert!(session.answer(4).is_none());
        assert!(!session.answered());
        assert_eq!(session.score(), 0);
    }

    #[test]
    fn trivia_points_decrease_with_each_revealed_hint() -> Result<(), Box<dyn Error>> {
        for (reveals, expected_hints, expected_points) in [(0, 1, 3), (1, 2, 2), (2, 3, 1)] {
            let mut session = Session::default();
            session.start(vec![question(8, QuestionKind::Trivia)]);
            assert_eq!(session.hints_shown(), 1, "initial trivia hint");
            for _ in 0..reveals {
                assert!(session.reveal_hint());
            }
            assert_eq!(session.hints_shown(), expected_hints);

            let Some(_outcome) = session.answer(2) else {
                return Err("valid trivia answer was ignored".into());
            };
            assert_eq!(
                session.score(),
                expected_points,
                "hints shown: {expected_hints}"
            );
        }
        Ok(())
    }

    #[test]
    fn hint_reveal_obeys_question_kind_count_and_answer_state() {
        let mut session = Session::default();
        session.start(vec![question(8, QuestionKind::Flag)]);
        assert!(!session.reveal_hint());

        session.start(vec![question(8, QuestionKind::Trivia)]);
        assert_eq!(session.hints_shown(), 1);
        assert!(session.reveal_hint());
        assert!(session.reveal_hint());
        assert_eq!(session.hints_shown(), 3);
        assert!(!session.reveal_hint());
        assert!(session.answer(2).is_some());
        assert!(!session.reveal_hint());
    }

    #[test]
    fn missed_countries_contain_only_wrong_answers_in_order() {
        let mut session = Session::default();
        session.start(vec![
            question(9, QuestionKind::Flag),
            question(4, QuestionKind::Map),
            question(7, QuestionKind::Trivia),
        ]);

        assert!(session.answer(0).is_some());
        assert_eq!(session.next(), Next::Question);
        assert!(session.answer(2).is_some());
        assert_eq!(session.next(), Next::Question);
        assert!(session.answer(1).is_some());

        assert_eq!(session.missed_country_indices(), [9, 7]);
    }

    #[test]
    fn next_finishes_after_last_answer_and_keeps_correct_count() {
        let mut session = Session::default();
        session.start(vec![
            question(9, QuestionKind::Flag),
            question(4, QuestionKind::Map),
        ]);

        assert!(session.answer(2).is_some());
        assert_eq!(session.next(), Next::Question);
        assert!(session.answer(2).is_some());
        assert_eq!(session.next(), Next::Finished);
        assert_eq!(session.correct_count(), 2);
        assert_eq!(session.score(), 2);
    }

    #[test]
    fn empty_round_is_finished_and_rejects_actions() {
        let mut session = Session::default();
        session.start(Vec::new());

        assert!(session.question().is_none());
        assert!(session.answer(0).is_none());
        assert!(!session.reveal_hint());
        assert_eq!(session.next(), Next::Finished);
        assert_eq!(session.question_count(), 0);
    }
}
