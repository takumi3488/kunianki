use serde::{Deserialize, Serialize};

use crate::{
    data::{Country, Region},
    progress::Progress,
};

/// The presentation and scoring style for a question.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// Identify a country from its flag.
    Flag,
    /// Identify a country from its highlighted map.
    Map,
    /// Identify a country from progressively revealed hints.
    Trivia,
}

/// A question mode chosen for a quiz round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// Use flag questions.
    Flag,
    /// Use map questions.
    Map,
    /// Use trivia questions.
    Trivia,
    /// Balance flag, map, and trivia questions in the round.
    Mix,
}

/// A region selection for a quiz round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionFilter {
    /// Include countries from every region.
    All,
    /// Include countries from one region.
    Only(Region),
}

/// One multiple-choice quiz question, using indexes into the country list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Question {
    /// Index of the country being asked about.
    pub country: usize,
    /// The question's presentation kind.
    pub kind: QuestionKind,
    /// Four distinct country indexes, one of which is the answer.
    pub choices: [usize; 4],
    /// Index into `choices` for the correct answer.
    pub answer: usize,
}

/// Generates up to `len` questions, sampling countries without replacement.
pub fn generate_round(
    countries: &[Country],
    mode: Mode,
    filter: RegionFilter,
    rng: &mut fastrand::Rng,
    len: usize,
) -> Vec<Question> {
    let mut candidates: Vec<usize> = countries
        .iter()
        .enumerate()
        .filter_map(|(index, country)| match filter {
            RegionFilter::All => Some(index),
            RegionFilter::Only(region) if country.region == region => Some(index),
            RegionFilter::Only(_) => None,
        })
        .collect();
    rng.shuffle(&mut candidates);

    let question_count = len.min(candidates.len());
    question_kinds(mode, question_count, rng)
        .into_iter()
        .zip(candidates.into_iter().take(question_count))
        .filter_map(|(kind, country)| make_question(countries, country, kind, rng))
        .collect()
}

/// Generates review questions for seen items currently in Leitner box one.
pub fn generate_review(
    countries: &[Country],
    progress: &Progress,
    rng: &mut fastrand::Rng,
    len: usize,
) -> Vec<Question> {
    let mut candidates: Vec<(usize, QuestionKind)> = progress
        .review_keys()
        .into_iter()
        .filter_map(|(iso2, kind)| {
            countries
                .iter()
                .position(|country| country.iso2 == iso2)
                .map(|country| (country, kind))
        })
        .collect();
    rng.shuffle(&mut candidates);
    candidates
        .into_iter()
        .take(len)
        .filter_map(|(country, kind)| make_question(countries, country, kind, rng))
        .collect()
}

/// Returns the score for a correct trivia answer after the given number of hints.
///
/// Zero is treated as one hint because the first trivia hint is shown from the start.
#[must_use]
pub fn trivia_points(hints_revealed: u8) -> u32 {
    match hints_revealed {
        0 | 1 => 3,
        2 => 2,
        _ => 1,
    }
}

/// Builds the three Japanese hints in their reveal order.
#[must_use]
pub fn hints(country: &Country) -> [String; 3] {
    // Languages, currencies, and capitals often contain the country name (韓国語, 韓国ウォン).
    let mask = |hint: String| hint.replace(&country.name_ja, "〇〇");
    [
        country.trivia_ja.clone(),
        mask(format!(
            "公用語：{}、通貨：{}",
            country.languages_ja.join("、"),
            country.currency_ja
        )),
        mask(format!("首都：{}", country.capital_ja)),
    ]
}

fn question_kinds(mode: Mode, len: usize, rng: &mut fastrand::Rng) -> Vec<QuestionKind> {
    let mut kinds = match mode {
        Mode::Flag => vec![QuestionKind::Flag; len],
        Mode::Map => vec![QuestionKind::Map; len],
        Mode::Trivia => vec![QuestionKind::Trivia; len],
        Mode::Mix => (0..len)
            .map(|index| match index % 3 {
                0 => QuestionKind::Flag,
                1 => QuestionKind::Map,
                _ => QuestionKind::Trivia,
            })
            .collect(),
    };
    rng.shuffle(&mut kinds);
    kinds
}

fn make_question(
    countries: &[Country],
    country_index: usize,
    kind: QuestionKind,
    rng: &mut fastrand::Rng,
) -> Option<Question> {
    let country = countries.get(country_index)?;
    let same_region_count = countries
        .iter()
        .filter(|candidate| candidate.region == country.region)
        .count();
    let same_region_only = same_region_count >= 4;
    let mut distractors: Vec<usize> = countries
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            (index != country_index && (!same_region_only || candidate.region == country.region))
                .then_some(index)
        })
        .collect();
    rng.shuffle(&mut distractors);
    let mut distractors = distractors.into_iter();
    let (Some(first), Some(second), Some(third)) =
        (distractors.next(), distractors.next(), distractors.next())
    else {
        return None;
    };
    let mut choices = [country_index, first, second, third];
    rng.shuffle(&mut choices);
    let answer = choices.iter().position(|choice| *choice == country_index)?;
    Some(Question {
        country: country_index,
        kind,
        choices,
        answer,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn country(index: usize, region: Region) -> Country {
        Country {
            iso2: format!("C{index:02}"),
            name_ja: format!("国{index}"),
            name_en: format!("Country {index}"),
            region,
            capital_ja: format!("首都{index}"),
            languages_ja: vec![format!("言語{index}")],
            currency_ja: format!("通貨{index}"),
            trivia_ja: format!("雑学{index}"),
        }
    }

    fn countries() -> Vec<Country> {
        let mut countries: Vec<Country> =
            (0..5).map(|index| country(index, Region::Asia)).collect();
        countries.extend((5..8).map(|index| country(index, Region::Europe)));
        countries
    }

    #[test]
    fn choices_are_distinct_and_include_only_same_region_when_available() {
        let countries = countries();
        let mut rng = fastrand::Rng::with_seed(42);
        let questions = generate_round(&countries, Mode::Flag, RegionFilter::All, &mut rng, 10);

        assert_eq!(questions.len(), countries.len());
        for question in &questions {
            let choices: BTreeSet<usize> = question.choices.into_iter().collect();
            assert_eq!(choices.len(), 4);
            assert!(question.choices.contains(&question.country));
            assert!(
                question
                    .choices
                    .iter()
                    .all(|choice| countries.get(*choice).is_some())
            );
            for choice in question.choices {
                let Some(choice_country) = countries.get(choice) else {
                    continue;
                };
                let Some(question_country) = countries.get(question.country) else {
                    continue;
                };
                if countries
                    .iter()
                    .filter(|country| country.region == question_country.region)
                    .count()
                    >= 4
                {
                    assert_eq!(choice_country.region, question_country.region);
                }
            }
            assert!(question.answer < question.choices.len());
            assert_eq!(
                question.choices.get(question.answer),
                Some(&question.country)
            );
        }
    }

    #[test]
    fn choices_fall_back_to_other_regions_when_needed() {
        let countries = countries();
        let mut rng = fastrand::Rng::with_seed(7);
        let questions = generate_round(
            &countries,
            Mode::Map,
            RegionFilter::Only(Region::Europe),
            &mut rng,
            1,
        );
        assert_eq!(questions.len(), 1);
        let Some(question) = questions.first() else {
            return;
        };
        assert!(question.answer < question.choices.len());
        assert_eq!(
            question.choices.get(question.answer),
            Some(&question.country)
        );
        assert!(
            question
                .choices
                .iter()
                .all(|choice| countries.get(*choice).is_some())
        );
        assert!(question.choices.iter().any(|choice| {
            countries
                .get(*choice)
                .is_some_and(|country| country.region != Region::Europe)
        }));
    }

    #[test]
    fn answer_index_is_valid_and_region_filter_is_respected() {
        let countries = countries();
        let mut rng = fastrand::Rng::with_seed(19);
        let questions = generate_round(
            &countries,
            Mode::Trivia,
            RegionFilter::Only(Region::Asia),
            &mut rng,
            8,
        );
        assert_eq!(questions.len(), 5);
        for question in questions {
            assert!(question.answer < question.choices.len());
            let Some(country) = countries.get(question.country) else {
                continue;
            };
            assert_eq!(country.region, Region::Asia);
        }
    }

    #[test]
    fn round_clamps_to_filtered_country_count_without_duplicates() {
        let countries = countries();

        for (filter, requested, available) in [
            (RegionFilter::Only(Region::Asia), 8, 5),
            (RegionFilter::Only(Region::Europe), 2, 3),
        ] {
            let mut rng = fastrand::Rng::with_seed(37);
            let questions = generate_round(&countries, Mode::Flag, filter, &mut rng, requested);
            let unique_countries: BTreeSet<usize> =
                questions.iter().map(|question| question.country).collect();

            assert_eq!(questions.len(), requested.min(available));
            assert_eq!(unique_countries.len(), questions.len());
            assert!(questions.iter().all(|question| {
                countries
                    .get(question.country)
                    .is_some_and(|country| match filter {
                        RegionFilter::All => true,
                        RegionFilter::Only(region) => country.region == region,
                    })
            }));
        }
    }

    #[test]
    fn mix_covers_all_question_kinds() {
        let countries = countries();
        let mut rng = fastrand::Rng::with_seed(23);
        let questions = generate_round(&countries, Mode::Mix, RegionFilter::All, &mut rng, 8);
        let kinds: BTreeSet<QuestionKind> =
            questions.iter().map(|question| question.kind).collect();
        assert_eq!(kinds.len(), 3);
    }

    #[test]
    fn review_round_uses_seen_box_one_items() {
        let countries = countries();
        let mut progress = Progress::default();
        progress.record("C00", QuestionKind::Flag, false);
        progress.record("C01", QuestionKind::Trivia, false);
        progress.record("C02", QuestionKind::Map, true);
        let mut rng = fastrand::Rng::with_seed(31);

        let questions = generate_review(&countries, &progress, &mut rng, 10);
        let reviewed: BTreeSet<(usize, QuestionKind)> = questions
            .iter()
            .map(|question| (question.country, question.kind))
            .collect();

        assert_eq!(
            reviewed,
            BTreeSet::from([(0, QuestionKind::Flag), (1, QuestionKind::Trivia)])
        );
    }

    #[test]
    fn trivia_points_match_visible_hints_and_clamp_out_of_range_counts() {
        assert_eq!(trivia_points(0), 3);
        assert_eq!(trivia_points(1), 3);
        assert_eq!(trivia_points(2), 2);
        assert_eq!(trivia_points(3), 1);
        assert_eq!(trivia_points(u8::MAX), 1);
    }

    #[test]
    fn hints_are_revealed_in_order() {
        let country = country(1, Region::Asia);
        assert_eq!(
            hints(&country),
            [
                "雑学1".to_owned(),
                "公用語：言語1、通貨：通貨1".to_owned(),
                "首都：首都1".to_owned(),
            ]
        );
    }

    #[test]
    fn hints_never_reveal_the_country_name() -> Result<(), Box<dyn std::error::Error>> {
        for country in crate::data::load_countries()? {
            for hint in hints(&country) {
                assert!(
                    !hint.contains(&country.name_ja),
                    "{} hint reveals its name: {hint}",
                    country.iso2
                );
            }
        }
        Ok(())
    }
}
