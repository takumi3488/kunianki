use serde::{Deserialize, Serialize};

/// A UN M49 region used to group countries in quiz rounds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// Asia.
    Asia,
    /// Europe.
    Europe,
    /// Africa.
    Africa,
    /// North America, including Central America and the Caribbean.
    NorthAmerica,
    /// South America.
    SouthAmerica,
    /// Oceania.
    Oceania,
}

/// Country data used by quiz questions and Japanese hints.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Country {
    /// ISO 3166-1 alpha-2 country code.
    pub iso2: String,
    /// Short country name in Japanese.
    pub name_ja: String,
    /// Short names hidden from hints, such as マケドニア for 北マケドニア.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases_ja: Vec<String>,
    /// Country name in English.
    pub name_en: String,
    /// UN M49 region.
    pub region: Region,
    /// Capital city in Japanese.
    pub capital_ja: String,
    /// Three Japanese trivia hints in reveal order, from least to most famous.
    pub trivia_ja: [String; 3],
}

/// Loads and sorts the country records embedded from all six region files.
///
/// # Errors
///
/// Returns an error if any embedded region file contains invalid country JSON.
pub fn load_countries() -> Result<Vec<Country>, serde_json::Error> {
    let files = [
        include_str!("../data/countries/asia.json"),
        include_str!("../data/countries/europe.json"),
        include_str!("../data/countries/africa.json"),
        include_str!("../data/countries/north_america.json"),
        include_str!("../data/countries/south_america.json"),
        include_str!("../data/countries/oceania.json"),
    ];
    let mut countries = Vec::new();
    for file in files {
        countries.extend(serde_json::from_str::<Vec<Country>>(file)?);
    }
    countries.sort_unstable_by(|left, right| left.iso2.cmp(&right.iso2));
    Ok(countries)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        error::Error,
        io,
    };

    use super::*;
    use crate::assets::{flag_svg, map_svg};

    #[test]
    fn embedded_countries_and_assets_meet_data_contract() -> Result<(), Box<dyn Error>> {
        let countries = load_countries()?;
        assert_eq!(countries.len(), 193);

        let mut iso2_codes = BTreeSet::new();
        let mut region_counts = BTreeMap::new();

        for country in &countries {
            assert_eq!(country.iso2.len(), 2, "invalid ISO2: {}", country.iso2);
            assert!(
                country.iso2.bytes().all(|byte| byte.is_ascii_uppercase()),
                "ISO2 is not uppercase ASCII: {}",
                country.iso2
            );
            assert!(
                iso2_codes.insert(country.iso2.as_str()),
                "duplicate ISO2: {}",
                country.iso2
            );
            assert!(
                !country.name_ja.trim().is_empty(),
                "empty Japanese name: {}",
                country.iso2
            );
            assert!(
                !country.name_en.trim().is_empty(),
                "empty English name: {}",
                country.iso2
            );
            assert!(
                !country.capital_ja.trim().is_empty(),
                "empty capital: {}",
                country.iso2
            );
            for (index, hint) in country.trivia_ja.iter().enumerate() {
                assert!(
                    !hint.trim().is_empty(),
                    "empty trivia hint {}: {}",
                    index + 1,
                    country.iso2
                );
                assert!(
                    hint.chars().count() <= 60,
                    "trivia hint {} exceeds 60 characters: {}",
                    index + 1,
                    country.iso2
                );
                assert!(
                    !hint.contains(country.name_ja.as_str()),
                    "trivia hint {} contains country name: {}",
                    index + 1,
                    country.iso2
                );
                assert!(
                    country
                        .aliases_ja
                        .iter()
                        .all(|alias| alias.is_empty() || !hint.contains(alias.as_str())),
                    "trivia hint {} contains country alias: {}",
                    index + 1,
                    country.iso2
                );
                if index < 2 {
                    assert!(
                        !hint.contains(country.capital_ja.as_str()),
                        "trivia hint {} contains capital: {}",
                        index + 1,
                        country.iso2
                    );
                }
            }

            *region_counts.entry(country.region).or_insert(0) += 1;

            let iso2_lower = country.iso2.to_ascii_lowercase();
            let flag = flag_svg(&iso2_lower)
                .ok_or_else(|| io::Error::other(format!("missing flag SVG: {}", country.iso2)))?;
            let _flag_image = slint::Image::load_from_svg_data(flag).map_err(|error| {
                io::Error::other(format!("invalid flag SVG for {}: {error}", country.iso2))
            })?;
            let map = map_svg(&iso2_lower)
                .ok_or_else(|| io::Error::other(format!("missing map SVG: {}", country.iso2)))?;
            let _map_image = slint::Image::load_from_svg_data(map).map_err(|error| {
                io::Error::other(format!("invalid map SVG for {}: {error}", country.iso2))
            })?;
        }

        for region in [
            Region::Asia,
            Region::Europe,
            Region::Africa,
            Region::NorthAmerica,
            Region::SouthAmerica,
            Region::Oceania,
        ] {
            assert!(
                region_counts.get(&region).copied().unwrap_or_default() >= 4,
                "fewer than four countries in {region:?}"
            );
        }

        Ok(())
    }
}
