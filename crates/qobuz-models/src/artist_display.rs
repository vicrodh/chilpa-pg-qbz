//! Select track artists separately from technical credits and legacy casing.
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub struct DisplayArtist {
  pub name: String,
  pub id: Option<u64>,
}

fn name(value: &Value) -> Option<&str> {
  value
    .pointer("/name/display")
    .or_else(|| value.get("name"))?
    .as_str()
    .map(str::trim)
    .filter(|name| !name.is_empty())
}

fn id(value: &Value) -> Option<u64> {
  let value = value.get("id")?;
  value.as_u64().or_else(|| value.as_str()?.parse().ok()).filter(|id| *id != 0)
}

fn artist_role(value: &Value) -> bool {
  value.get("roles").and_then(Value::as_array).is_none_or(|roles| {
    roles.is_empty()
      || roles.iter().filter_map(Value::as_str).any(|role| {
        matches!(
          role.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str(),
          "mainartist" | "performer" | "featuredartist"
        )
      })
  })
}

fn same_name(left: &str, right: &str) -> bool {
  left.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
    == right.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn canonical_artist<'a>(
  artist: &Value,
  raw: &'a Value,
  parent: Option<&'a Value>,
) -> Option<&'a Value> {
  let original = name(artist)?;
  let own = raw.get("artist").into_iter().chain(raw.pointer("/album/artist"));
  let parent_artist = parent.and_then(|album| album.get("artist"));
  let lists = raw
    .pointer("/album/artists")
    .into_iter()
    .chain(parent.and_then(|album| album.get("artists")))
    .filter_map(Value::as_array)
    .flatten();
  own
    .chain(parent_artist)
    .chain(lists)
    .filter(|candidate| !std::ptr::eq(*candidate, artist))
    .filter(|candidate| {
      name(candidate).is_some_and(|candidate_name| {
        (id(artist).is_some() && id(artist) == id(candidate)) || same_name(original, candidate_name)
      })
    })
    .max_by_key(|candidate| {
      (
        name(candidate).is_some_and(|name| name.chars().any(char::is_lowercase)),
        id(candidate).is_some(),
      )
    })
}

/// Preserve established names and acronyms; soften only multiword uppercase fallbacks.
fn fallback_case(value: &str) -> String {
  if value.chars().any(char::is_lowercase)
    || value.split_whitespace().filter(|word| word.chars().any(char::is_alphabetic)).count() < 2
  {
    return value.to_owned();
  }
  value
    .split_whitespace()
    .map(|word| {
      let letters = word.chars().filter(|c| c.is_alphabetic()).count();
      if letters <= 3 && !matches!(word, "THE" | "AND" | "LOS" | "LAS" | "LES") {
        return word.to_owned();
      }
      let mut initial = true;
      let mut result = String::new();
      for character in word.chars() {
        if character.is_alphabetic() {
          if initial {
            result.extend(character.to_uppercase());
          } else {
            result.extend(character.to_lowercase());
          }
          initial = false;
        } else {
          result.push(character);
          initial = matches!(character, '-' | '\'' | '’');
        }
      }
      result
    })
    .collect::<Vec<_>>()
    .join(" ")
}

/// Prefer structured performers; keep their navigation ID paired with their name.
pub fn track_artist(raw: &Value, parent: Option<&Value>) -> Option<DisplayArtist> {
  let mut names: Vec<String> = Vec::new();
  let mut ids: Vec<Option<u64>> = Vec::new();
  for artist in
    raw.get("artists").and_then(Value::as_array).into_iter().flatten().filter(|a| artist_role(a))
  {
    let Some(original) = name(artist) else { continue };
    let canonical = canonical_artist(artist, raw, parent);
    let candidate = canonical.and_then(name).unwrap_or(original);
    let artist_id = id(artist).or_else(|| canonical.and_then(id));
    if let Some(index) = names.iter().enumerate().find_map(|(index, previous)| {
      ((artist_id.is_some() && ids[index] == artist_id) || same_name(previous, candidate))
        .then_some(index)
    }) {
      if !names[index].chars().any(char::is_lowercase) && candidate.chars().any(char::is_lowercase)
      {
        names[index] = candidate.to_owned();
      }
      continue;
    }
    names.push(candidate.to_owned());
    ids.push(artist_id);
  }
  if !names.is_empty() {
    return Some(DisplayArtist {
      name: names.iter().map(|name| fallback_case(name)).collect::<Vec<_>>().join(", "),
      id: ids[0],
    });
  }
  let artist = raw
    .get("performer")
    .filter(|artist| name(artist).is_some())
    .or_else(|| raw.get("artist").filter(|artist| name(artist).is_some()))
    .or_else(|| raw.pointer("/album/artist").filter(|artist| name(artist).is_some()))
    .or_else(|| parent?.get("artist").filter(|artist| name(artist).is_some()))
    .or_else(|| {
      raw
        .pointer("/album/artists")?
        .as_array()?
        .iter()
        .find(|artist| artist_role(artist) && name(artist).is_some())
    })
    .or_else(|| {
      parent?
        .get("artists")?
        .as_array()?
        .iter()
        .find(|artist| artist_role(artist) && name(artist).is_some())
    })?;
  let original = name(artist)?;
  let canonical = canonical_artist(artist, raw, parent);
  let display = canonical
    .and_then(name)
    .filter(|candidate| candidate.chars().any(char::is_lowercase))
    .map(str::to_owned)
    .unwrap_or_else(|| fallback_case(original));
  Some(DisplayArtist { name: display, id: id(artist).or_else(|| canonical.and_then(id)) })
}

/// Adapt catalogue JSON before typed decoding so every consumer sees the same artists.
pub fn from_catalog_value<T: DeserializeOwned>(mut value: Value) -> serde_json::Result<T> {
  fn visit(value: &mut Value) {
    if value.get("performer").is_some()
      || value.get("track_number").is_some()
      || value.pointer("/physical_support/track_number").is_some()
    {
      if let Some(artist) = track_artist(value, None) {
        value["performer"] = json!({"name":artist.name,"id":artist.id.unwrap_or(0)});
      }
    }
    match value {
      Value::Object(object) => {
        for child in object.values_mut() {
          visit(child);
        }
      }
      Value::Array(array) => {
        for child in array {
          visit(child);
        }
      }
      _ => {}
    }
  }
  visit(&mut value);
  serde_json::from_value(value)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn structured_artists_keep_performers_and_exclude_technical_credits() {
    let raw = json!({"performer":{"id":99,"name":"George Harrison, Phil McDonald, Big Sky Music"},
      "artists":[{"id":1,"name":"George Harrison","roles":["main-artist","performer"]},
        {"id":2,"name":"Phil McDonald","roles":["engineer"]},
        {"id":3,"name":"Big Sky Music","roles":["publisher","composer"]},
        {"id":4,"name":{"display":"Eric Clapton"},"roles":["performer"]}]});
    assert_eq!(
      track_artist(&raw, None),
      Some(DisplayArtist { name: "George Harrison, Eric Clapton".into(), id: Some(1) })
    );
  }

  #[test]
  fn canonical_album_names_correct_legacy_casing_without_changing_artist_ids() {
    let raw = json!({"performer":{"id":1,"name":"GEORGE HARRISON"},
      "album":{"artist":{"id":1,"name":"George Harrison"}}});
    assert_eq!(
      track_artist(&raw, None).unwrap(),
      DisplayArtist { name: "George Harrison".into(), id: Some(1) }
    );
    let parent = json!({"artist":{"id":2,"name":"Paul McCartney"}});
    let raw = json!({"performer":{"id":2,"name":"PAUL MCCARTNEY"}});
    assert_eq!(track_artist(&raw, Some(&parent)).unwrap().name, "Paul McCartney");
    let other = json!({"artist":{"id":3,"name":"Various Artists"}});
    assert_eq!(track_artist(&raw, Some(&other)).unwrap().id, Some(2));
  }

  #[test]
  fn uppercase_fallback_keeps_acronyms_and_mixed_case_names() {
    for (input, expected) in [
      ("GEORGE HARRISON", "George Harrison"),
      ("ÉDITH PIAF", "Édith Piaf"),
      ("THE BEATLES", "The Beatles"),
      ("LCD SOUNDSYSTEM", "LCD Soundsystem"),
      ("AC/DC", "AC/DC"),
      ("ABBA", "ABBA"),
      ("XTC", "XTC"),
      ("k.d. lang", "k.d. lang"),
      ("deadmau5", "deadmau5"),
      ("Paul McCartney", "Paul McCartney"),
    ] {
      assert_eq!(fallback_case(input), expected);
    }
  }

  #[test]
  fn duplicate_artist_ids_merge_without_overwriting_display_names() {
    let raw = json!({"artists":[{"id":1,"name":"GEORGE HARRISON","roles":["main-artist"]},
      {"id":1,"name":"George Harrison","roles":["performer"]},
      {"id":2,"name":"Tom Petty","roles":["performer"]}]});
    assert_eq!(track_artist(&raw, None).unwrap().name, "George Harrison, Tom Petty");
  }

  #[test]
  fn an_unknown_track_artist_never_borrows_another_artists_id() {
    let parent = json!({"artist":{"id":8,"name":"Performer"}});
    let raw = json!({"artist":{"name":"Another artist"}});
    assert_eq!(track_artist(&raw, Some(&parent)).unwrap().id, None);
    let raw = json!({"artist":{"name":"PERFORMER"}});
    assert_eq!(
      track_artist(&raw, Some(&parent)).unwrap(),
      DisplayArtist { name: "Performer".into(), id: Some(8) }
    );
  }

  #[test]
  fn typed_catalogue_decoding_keeps_credits_membership_and_quality() {
    let input = json!({"tracks":{"items":[{"id":17,"title":"Song","track_number":1,
      "playlist_track_id":71,"performer":{"id":1,"name":"GEORGE HARRISON"},
      "artists":[{"id":1,"name":"George Harrison","roles":["performer"]}],
      "performers":"Phil McDonald, Engineer - George Harrison, MainArtist",
      "maximum_bit_depth":24,"maximum_sampling_rate":96.0}]}});
    let output: Value = from_catalog_value(input.clone()).unwrap();
    let expected = json!({"name":"George Harrison","id":1});
    assert_eq!(output["tracks"]["items"][0]["performer"], expected);
    for key in [
      "id",
      "playlist_track_id",
      "artists",
      "performers",
      "maximum_bit_depth",
      "maximum_sampling_rate",
    ] {
      assert_eq!(output["tracks"]["items"][0][key], input["tracks"]["items"][0][key]);
    }
    let track: crate::Track = from_catalog_value(output["tracks"]["items"][0].clone()).unwrap();
    assert_eq!(track.performer.unwrap().name, "George Harrison");
  }
}
