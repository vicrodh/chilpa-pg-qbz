use super::{json, plain, text, Body, RpcFault, Value};

fn scalar(value: &Value) -> String {
  value
    .as_str()
    .map(plain)
    .or_else(|| value.as_number().map(ToString::to_string))
    .unwrap_or_default()
}

impl Body {
  pub(super) fn embed(&mut self, element: &Value) -> Result<(), RpcFault> {
    let kind = element["subtype"].as_str().unwrap_or_default();
    if !matches!(kind, "qobuz-albums" | "qobuz-pros-cons" | "qobuz-features") {
      return Ok(());
    }
    let config = &element["embed"]["config"];
    let title = text(config, "/title");
    if !title.is_empty() {
      self.push(json!({"type":"heading","text":title,"level":2}))?;
    }
    match kind {
      "qobuz-albums" => {
        let subtitle = text(config, "/subtitle");
        if !subtitle.is_empty() {
          self.push(json!({"type":"paragraph","text":subtitle}))?;
        }
        let albums = config["objects"].as_array().map(Vec::as_slice).unwrap_or_default();
        let names: Vec<_> = albums
          .iter()
          .map(|album| {
            let title = text(album, "/title");
            let artist = text(album, "/artist/name/display");
            let artist = if artist.is_empty() { text(album, "/artist/name") } else { artist };
            if artist.is_empty() {
              title
            } else {
              format!("{title} — {artist}")
            }
          })
          .filter(|title| !title.is_empty())
          .collect();
        if !names.is_empty() {
          self.push(json!({"type":"list","items":names,"ordered":false}))?;
        }
        if let Some(album) = albums.first() {
          if let Some(url) = crate::projection::image(album) {
            self.image(&json!({"url":url,"caption":text(album,"/title")}))?;
          }
        }
      }
      "qobuz-pros-cons" => {
        let description = text(config, "/desc");
        if !description.is_empty() {
          self.push(json!({"type":"paragraph","text":description}))?;
        }
        for (key, prefix) in [("p", "+ "), ("c", "− ")] {
          let items: Vec<_> = config[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(scalar)
            .filter(|item| !item.is_empty())
            .map(|item| format!("{prefix}{item}"))
            .collect();
          if !items.is_empty() {
            self.push(json!({"type":"list","items":items,"ordered":false}))?;
          }
        }
        let rating = scalar(&config["rating"]);
        if let Ok(rating) = rating.parse::<f64>() {
          if (0.0..=5.0).contains(&rating) {
            self.push(json!({"type":"paragraph","text":format!("{rating} / 5")}))?;
          }
        }
      }
      "qobuz-features" => {
        let rows: Vec<_> = config["el"]
          .as_array()
          .into_iter()
          .flatten()
          .map(|row| {
            let key = scalar(&row["k"]);
            let value = scalar(&row["v"]);
            if key.is_empty() {
              value
            } else {
              format!("{key}: {value}")
            }
          })
          .filter(|row| !row.is_empty())
          .collect();
        if !rows.is_empty() {
          self.push(json!({"type":"list","items":rows,"ordered":false}))?;
        }
      }
      _ => {}
    }
    Ok(())
  }
}
