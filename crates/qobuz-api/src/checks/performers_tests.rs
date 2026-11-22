use super::*;

#[test]
fn test_parse_single_performer() {
  let result = parse_performers("John Coltrane, Saxophone");
  assert_eq!(result.len(), 1);
  assert_eq!(result[0].name, "John Coltrane");
  assert_eq!(result[0].roles, vec!["Saxophone"]);
}

#[test]
fn test_parse_multiple_performers() {
  let result = parse_performers(
    "John Coltrane, Saxophone, MainArtist - McCoy Tyner, Piano - Elvin Jones, Drums",
  );
  assert_eq!(result.len(), 3);
  assert_eq!(result[0].name, "John Coltrane");
  assert_eq!(result[0].roles, vec!["Saxophone", "MainArtist"]);
  assert_eq!(result[1].name, "McCoy Tyner");
  assert_eq!(result[1].roles, vec!["Piano"]);
  assert_eq!(result[2].name, "Elvin Jones");
  assert_eq!(result[2].roles, vec!["Drums"]);
}

#[test]
fn test_parse_empty_string() {
  let result = parse_performers("");
  assert!(result.is_empty());
}

#[test]
fn test_parse_performer_no_roles() {
  let result = parse_performers("John Coltrane");
  assert_eq!(result.len(), 1);
  assert_eq!(result[0].name, "John Coltrane");
  assert!(result[0].roles.is_empty());
}

#[test]
fn test_group_by_role() {
  let performers = vec![
    Performer::new("John".to_string(), vec!["Saxophone".to_string()]),
    Performer::new("Jane".to_string(), vec!["Saxophone".to_string(), "Vocals".to_string()]),
  ];
  let grouped = group_by_role(&performers);
  assert_eq!(grouped.get("Saxophone").unwrap().len(), 2);
  assert_eq!(grouped.get("Vocals").unwrap().len(), 1);
}

const NOTHING_ELSE_MATTERS: &str = "Watt, FeaturedArtist - Robert Trujillo, Bass, FeaturedArtist - Elton John, Piano, FeaturedArtist - ALAN MOULDER, MixingEngineer - Paul Lamalfa, Engineer - Yo-Yo Ma, Cello, FeaturedArtist - Miley Cyrus, Vocalist, MainArtist - Chad Smith, FeaturedArtist, DrumKit - James Hetfield, ComposerLyricist - Lars Ulrich, ComposerLyricist - Mike Block, StringArranger - Andrew Watt, Producer, Guitar, StringArranger - Caesar Edmunds, MixingSecondEngineer - Randy Merrill, MasteringEngineer";

/// The real search sample: MainArtist mid-string, featured artists with
/// instruments, crew roles to ignore.
#[test]
fn test_featured_artists_keeps_featured_and_extra_mains_in_string_order_minus_the_row_artist() {
  assert_eq!(
    featured_artists(NOTHING_ELSE_MATTERS, "Miley Cyrus", "Nothing Else Matters"),
    vec!["Watt", "Robert Trujillo", "Elton John", "Yo-Yo Ma", "Chad Smith"]
  );
}

#[test]
fn test_featured_artists_skips_names_the_row_or_the_title_already_credits() {
  let p = "Alice, MainArtist - Bob, MainArtist - Carol, FeaturedArtist";
  assert_eq!(featured_artists(p, "Alice & Bob", "Song"), vec!["Carol"]);
  assert_eq!(featured_artists(p, "Alice", "Song (feat. Carol)"), vec!["Bob"]);
  assert!(featured_artists(p, "Alice, Bob & Carol", "Song").is_empty());
}

#[test]
fn test_same_artist_name_folds_case_diacritics_and_apostrophes() {
  assert!(same_artist_name("Émilie D'Angelo", "emilie dangelo"));
  assert!(same_artist_name("Yo-Yo Ma", "YO-YO  MA"));
  assert!(!same_artist_name("Watt", "Andrew Watt"));
  assert!(!same_artist_name("", ""));
}

#[test]
fn test_featured_artists_dedupes_case_and_diacritics_and_ignores_crew() {
  let p =
    "Émilie, FeaturedArtist - emilie, Vocalist, FeaturedArtist - Dan, Producer - Main, MainArtist";
  assert_eq!(featured_artists(p, "Main", ""), vec!["Émilie"]);
  assert!(featured_artists("", "Main", "").is_empty());
  assert!(featured_artists("Solo, MainArtist", "Solo", "Song").is_empty());
}
